//! Wasm node wrappers — own Store<WaferState> + typed bindings + cached InstancePre.
//!
//! These are the low-level Wasm call wrappers used by the per-type runner loops.
//! Each struct wraps a single wasmtime instance and handles:
//! - Per-call buffer resource push + fuel reset + log lifecycle
//! - Error mapping from WIT process-error → WasmProcessError
//! - Hot-swap replacement (RAII drops old Store)
//! - Recovery from cached InstancePre (~5µs re-instantiation)
//!
//! CRITICAL: These calls MUST run to completion — NEVER inside select! branches.
//! See docs/rfcs/RFC-005-orchestrator.md D5–D7.

use std::num::NonZeroU64;
use std::sync::Arc;

use bytes::Bytes;
use wasmtime::Store;

use crate::engine::bindings::filter_node::{FilterNode, FilterNodePre};
use crate::engine::bindings::router_node::{RouterNode, RouterNodePre};
use crate::engine::bindings::transform_node::{self, TransformNode, TransformNodePre};
use crate::engine::Capabilities;
use crate::engine::state::{LogLevel, WaferState};
use crate::error::WaferError;
use crate::node::traits::{FilterOutcome, RouteOutcome};
use crate::queue::RuntimeEnvelope;
use crate::runner::error_policy::WasmProcessError;

/// Maps a WIT `process-error` variant to our host-side error type.
fn map_process_error(err: transform_node::pipeline::types::types::ProcessError) -> WasmProcessError {
    use transform_node::pipeline::types::types::ProcessError as WitErr;
    match err {
        WitErr::BadInput(msg) => WasmProcessError::BadInput(msg),
        WitErr::DependencyFailed(msg) => WasmProcessError::DependencyFailed(msg),
        WitErr::ProcessingFailed(msg) => WasmProcessError::ProcessingFailed(msg),
        WitErr::TimedOut => WasmProcessError::TimedOut,
        WitErr::Unrecoverable(msg) => WasmProcessError::Unrecoverable(msg),
    }
}

/// Maps a wasmtime trap/error to `WasmProcessError`.
///
/// Epoch interruption → TimedOut; all others → Unrecoverable.
fn map_trap(err: &wasmtime::Error) -> WasmProcessError {
    // Debug repr surfaces the `Caused by:` chain (which carries the trap
    // variant); Display shows only the top frame. Missing the chain would
    // misclassify epoch interrupts as Unrecoverable and route them to
    // recovery instead of retry.
    let msg = err.to_string();
    let dbg = format!("{err:?}");
    if msg.contains("epoch") || msg.contains("interrupt") || dbg.contains("wasm trap: interrupt") {
        WasmProcessError::TimedOut
    } else {
        WasmProcessError::Unrecoverable(msg)
    }
}

/// Drains log buffer from WaferState and emits via tracing crate.
fn flush_logs(store: &mut Store<WaferState>) {
    let state = store.data_mut();
    if !state.has_logs() {
        return;
    }
    let node_id = state.node_id().to_owned();
    for entry in state.drain_logs() {
        match entry.level {
            LogLevel::Trace => tracing::trace!(node = %node_id, "{}", entry.message),
            LogLevel::Debug => tracing::debug!(node = %node_id, "{}", entry.message),
            LogLevel::Info => tracing::info!(node = %node_id, "{}", entry.message),
            LogLevel::Warn => tracing::warn!(node = %node_id, "{}", entry.message),
            LogLevel::Error => tracing::error!(node = %node_id, "{}", entry.message),
        }
    }
}

/// Builds the WIT `message` record from a `RuntimeEnvelope` after pushing the
/// buffer resource into the Store's resource table.
///
/// Returns the message struct ready for the guest call.
fn build_wit_message(
    store: &mut Store<WaferState>,
    envelope: &RuntimeEnvelope,
) -> Result<transform_node::pipeline::types::types::Message, WasmProcessError> {
    // Push payload into ResourceTable so guest gets a borrow<buffer> handle
    let resource = store
        .data_mut()
        .push_buffer(envelope.payload.clone())
        .map_err(|e| WasmProcessError::Unrecoverable(format!("failed to push buffer: {e}")))?;

    let metadata: Vec<(String, String)> = envelope
        .header
        .metadata
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    Ok(transform_node::pipeline::types::types::Message {
        id: envelope.header.id.to_string(),
        timestamp: envelope.header.timestamp,
        source: envelope.header.source.to_string(),
        content_type: envelope.header.content_type.to_string(),
        metadata,
        payload: resource,
    })
}

fn recovery_store(
    old_store: &Store<WaferState>,
    node_id: &str,
    capabilities: Capabilities,
    memory_limit: usize,
    epoch_deadline: Option<NonZeroU64>,
    fuel_limit: Option<NonZeroU64>,
) -> Result<Store<WaferState>, WaferError> {
    let engine = old_store.engine().clone();
    let state = WaferState::new_with_memory_limit(node_id, capabilities, memory_limit);
    let mut store = Store::new(&engine, state);
    store.limiter(|s| s.limits_mut());
    // AC F5.AC2: None skips the setter entirely — engine construction leaves
    // consume_fuel/epoch_interruption off when the limit is None, so the
    // store's default 0 deadline never triggers a trap.
    if let Some(n) = epoch_deadline {
        store.epoch_deadline_trap();
        store.set_epoch_deadline(n.get());
    }
    // M2 (safety review): reapply fuel BEFORE the caller instantiates from
    // the cached InstancePre. Component start functions can consume fuel
    // during `instantiate`; if the store's fuel is still 0 (default), the
    // guest traps on its first fuel-checking instruction. `validate_and_init`
    // resets fuel later but only AFTER instantiation completes, which is
    // too late for start-function fuel consumption on fuel-enabled configs.
    if let Some(n) = fuel_limit {
        store.set_fuel(n.get()).map_err(|e| WaferError::PluginInit {
            message: format!("recovery fuel reset for '{node_id}' failed: {e}"),
        })?;
    }
    Ok(store)
}

// =============================================================================
// WasmTransformNode — takes ownership of envelope, produces new envelope
// =============================================================================

/// Wasm transform node wrapper — owns Store + bindings + cached InstancePre.
///
/// Takes ownership of the input envelope and produces a new one with transformed
/// payload bytes.
pub struct WasmTransformNode {
    store: Store<WaferState>,
    bindings: TransformNode,
    cached_pre: Arc<TransformNodePre<WaferState>>,
    fuel_limit: Option<NonZeroU64>,
    capabilities: Capabilities,
    memory_limit: usize,
    epoch_deadline: Option<NonZeroU64>,
    config_json: String,
    /// Operator-supplied version string, passed to guest via `NodeConfig.
    /// plugin-version`. Empty string when unset in TOML.
    plugin_version: String,
}

impl WasmTransformNode {
    /// Create from a pre-instantiated transform binding.
    pub fn new(
        store: Store<WaferState>,
        bindings: TransformNode,
        cached_pre: Arc<TransformNodePre<WaferState>>,
        fuel_limit: Option<NonZeroU64>,
    ) -> Self {
        Self {
            store,
            bindings,
            cached_pre,
            fuel_limit,
            capabilities: Capabilities::sandbox(),
            memory_limit: 16 * 1024 * 1024,
            epoch_deadline: None,
            config_json: "{}".to_string(),
            plugin_version: String::new(),
        }
    }

    pub fn configure_runtime(
        &mut self,
        capabilities: Capabilities,
        memory_limit: usize,
        epoch_deadline: Option<NonZeroU64>,
        config_json: String,
    ) {
        self.capabilities = capabilities;
        self.memory_limit = memory_limit;
        self.epoch_deadline = epoch_deadline;
        self.config_json = config_json;
    }

    /// Set the plugin version string passed to the guest at `init()`.
    /// Must be called before `validate_and_init`.
    pub fn set_plugin_version(&mut self, version: impl Into<String>) {
        self.plugin_version = version.into();
    }

    /// Read the plugin version string. Empty when the operator did not
    /// supply `[nodes.<id>].plugin_version` in TOML.
    #[must_use]
    pub fn plugin_version(&self) -> &str {
        &self.plugin_version
    }

    pub fn recover_from_cached_pre(&mut self) -> Result<(), WaferError> {
        let node_id = self.node_id().to_string();
        let mut store = recovery_store(
            &self.store,
            &node_id,
            self.capabilities,
            self.memory_limit,
            self.epoch_deadline,
            self.fuel_limit,
        )?;
        let bindings = self.cached_pre.instantiate(&mut store).map_err(|e| WaferError::PluginInit {
            message: format!("transform '{node_id}' recovery instantiation failed: {e}"),
        })?;
        self.store = store;
        self.bindings = bindings;
        let config_json = self.config_json.clone();
        self.validate_and_init(&config_json)
    }

    /// Warm reconfigure: re-instantiate from the cached `InstancePre` and
    /// call `validate() + init()` with `new_config_json`. No compile/instantiate
    /// of a new plugin binary. On failure, roll back to v1 state.
    pub fn try_reconfigure(&mut self, new_config_json: &str) -> Result<(), WaferError> {
        let node_id = self.node_id().to_string();
        let mut new_store = recovery_store(
            &self.store,
            &node_id,
            self.capabilities,
            self.memory_limit,
            self.epoch_deadline,
            self.fuel_limit,
        )?;
        let new_bindings = self.cached_pre.instantiate(&mut new_store).map_err(|e| WaferError::PluginInit {
            message: format!("transform '{node_id}' reconfigure instantiation failed: {e}"),
        })?;
        let old_store = std::mem::replace(&mut self.store, new_store);
        let old_bindings = std::mem::replace(&mut self.bindings, new_bindings);
        let old_config = std::mem::replace(&mut self.config_json, new_config_json.to_string());
        match self.validate_and_init(new_config_json) {
            Ok(()) => Ok(()),
            Err(err) => {
                self.store = old_store;
                self.bindings = old_bindings;
                self.config_json = old_config;
                Err(err)
            }
        }
    }

    /// Process one message through the Wasm transform.
    ///
    /// MUST run to completion — never place in a select! branch.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "API contract: transform takes ownership of input envelope (consumed semantically even if implementation only borrows)"
    )]
    pub fn process(
        &mut self,
        envelope: RuntimeEnvelope,
    ) -> Result<RuntimeEnvelope, WasmProcessError> {
        self.store.data_mut().clear_log_buffer();

        // AC F5.AC2: None means unlimited — engine has consume_fuel disabled
        // in that case, so calling set_fuel would return an error.
        if let Some(n) = self.fuel_limit {
            self.store
                .set_fuel(n.get())
                .map_err(|e| WasmProcessError::Unrecoverable(format!("fuel reset failed: {e}")))?;
        }
        // set_epoch_deadline is relative to the engine's current epoch; without
        // a per-call reset the store's absolute deadline lapses after
        // `epoch_deadline` ticks and every subsequent call traps with
        // `wasm trap: interrupt` at the first check point.
        if let Some(n) = self.epoch_deadline {
            self.store.set_epoch_deadline(n.get());
        }

        let wit_msg = build_wit_message(&mut self.store, &envelope)?;
        // `borrow<buffer>` keeps host ownership; we must delete the resource
        // ourselves after the guest returns or the ResourceTable grows one
        // Bytes-clone entry per message.
        let payload_rep = wit_msg.payload.rep();

        let result = self
            .bindings
            .pipeline_node_transform()
            .call_process(&mut self.store, &wit_msg);

        flush_logs(&mut self.store);

        // Free even on trap; leaks would starve recovery.
        let delete_res = self
            .store
            .data_mut()
            .delete_buffer(wasmtime::component::Resource::new_own(payload_rep));
        if let Err(e) = delete_res {
            return Err(WasmProcessError::Unrecoverable(format!(
                "failed to release buffer resource: {e}"
            )));
        }

        match result {
            Ok(Ok(output)) => {
                let mut new_envelope = RuntimeEnvelope::new(
                    Box::<str>::from(output.source.as_str()),
                    Bytes::from(output.payload),
                );
                // Guest metadata must reach the sink or BenchSink cannot find
                // `bench.intended_ns` and records zero latency samples.
                for (k, v) in output.metadata {
                    new_envelope = new_envelope.with_metadata(k, v);
                }
                new_envelope.inherit_lineage_from(&envelope);
                Ok(new_envelope)
            }
            Ok(Err(wit_err)) => Err(map_process_error(wit_err)),
            Err(trap) => Err(map_trap(&trap)),
        }
    }

    /// Call guest lifecycle validate() and init() before first message processing.
    pub fn validate_and_init(&mut self, config_json: &str) -> Result<(), WaferError> {
        let node_config = transform_node::exports::pipeline::node::lifecycle::NodeConfig {
            id: self.node_id().to_string(),
            config: config_json.to_string(),
            plugin_version: self.plugin_version.clone(),
        };
        // AC F5.AC2: None → unlimited; engine has metering disabled at Config
        // level so the setter must be skipped, not called with a sentinel.
        if let Some(n) = self.fuel_limit {
            self.store.set_fuel(n.get()).map_err(|e| WaferError::PluginInit {
                message: format!("transform '{}' lifecycle fuel reset failed: {e}", self.node_id()),
            })?;
        }
        if let Some(n) = self.epoch_deadline {
            self.store.set_epoch_deadline(n.get());
        }
        if let Some(message) = self
            .bindings
            .pipeline_node_lifecycle()
            .call_validate(&mut self.store, &node_config)
            .map_err(|e| WaferError::PluginInit {
                message: format!("transform '{}' validate() trapped: {e}", self.node_id()),
            })?
        {
            return Err(WaferError::PluginInit {
                message: format!("transform '{}' validate() rejected config: {message}", self.node_id()),
            });
        }
        self.bindings
            .pipeline_node_lifecycle()
            .call_init(&mut self.store, &node_config)
            .map_err(|e| WaferError::PluginInit {
                message: format!("transform '{}' init() trapped: {e}", self.node_id()),
            })?
            .map_err(|e| WaferError::PluginInit {
                message: format!("transform '{}' init() failed: {e:?}", self.node_id()),
            })?;
        flush_logs(&mut self.store);
        Ok(())
    }

    /// Replace the node's internals for hot-swap. Old Store dropped by RAII.
    pub fn replace(
        &mut self,
        new_store: Store<WaferState>,
        new_bindings: TransformNode,
        new_pre: Arc<TransformNodePre<WaferState>>,
    ) {
        self.store = new_store;
        self.bindings = new_bindings;
        self.cached_pre = new_pre;
    }

    /// Replace transform internals with rollback if `validate()`/`init()` fails.
    pub fn try_hot_swap(
        &mut self,
        new_store: Store<WaferState>,
        new_bindings: TransformNode,
        new_pre: Arc<TransformNodePre<WaferState>>,
    ) -> Result<(), WaferError> {
        let config_json = self.config_json.clone();
        let old_store = std::mem::replace(&mut self.store, new_store);
        let old_bindings = std::mem::replace(&mut self.bindings, new_bindings);
        let old_pre = std::mem::replace(&mut self.cached_pre, new_pre);
        match self.validate_and_init(&config_json) {
            Ok(()) => Ok(()),
            Err(err) => {
                self.store = old_store;
                self.bindings = old_bindings;
                self.cached_pre = old_pre;
                Err(err)
            }
        }
    }

    /// Get the node identity from the Store state.
    pub fn node_id(&self) -> &str {
        self.store.data().node_id()
    }

    /// Access the cached InstancePre for recovery/warm-swap.
    pub const fn cached_pre(&self) -> &Arc<TransformNodePre<WaferState>> {
        &self.cached_pre
    }

    /// Replace the cached InstancePre (used by process-time rollback A17
    /// to restore v1's pre after rollback).
    pub fn set_cached_pre(&mut self, pre: Arc<TransformNodePre<WaferState>>) {
        self.cached_pre = pre;
    }

    /// Get a mutable reference to the store (for lifecycle calls like init/close).
    pub const fn store_mut(&mut self) -> &mut Store<WaferState> {
        &mut self.store
    }

    /// Get a reference to the bindings (for lifecycle calls).
    pub const fn bindings(&self) -> &TransformNode {
        &self.bindings
    }
}

// =============================================================================
// WasmFilterNode — borrows envelope, returns Forward/Drop
// =============================================================================

/// Wasm filter node wrapper — borrows the envelope, returns pass/drop decision.
///
/// Zero-copy path: if Forward, the original envelope is passed downstream unchanged.
pub struct WasmFilterNode {
    store: Store<WaferState>,
    bindings: FilterNode,
    cached_pre: Arc<FilterNodePre<WaferState>>,
    fuel_limit: Option<NonZeroU64>,
    capabilities: Capabilities,
    memory_limit: usize,
    epoch_deadline: Option<NonZeroU64>,
    config_json: String,
    plugin_version: String,
}

impl WasmFilterNode {
    /// Create from a pre-instantiated filter binding.
    pub fn new(
        store: Store<WaferState>,
        bindings: FilterNode,
        cached_pre: Arc<FilterNodePre<WaferState>>,
        fuel_limit: Option<NonZeroU64>,
    ) -> Self {
        Self {
            store,
            bindings,
            cached_pre,
            fuel_limit,
            capabilities: Capabilities::sandbox(),
            memory_limit: 16 * 1024 * 1024,
            epoch_deadline: None,
            config_json: "{}".to_string(),
            plugin_version: String::new(),
        }
    }

    pub fn configure_runtime(
        &mut self,
        capabilities: Capabilities,
        memory_limit: usize,
        epoch_deadline: Option<NonZeroU64>,
        config_json: String,
    ) {
        self.capabilities = capabilities;
        self.memory_limit = memory_limit;
        self.epoch_deadline = epoch_deadline;
        self.config_json = config_json;
    }

    /// Set the plugin version string passed to the guest at `init()`.
    pub fn set_plugin_version(&mut self, version: impl Into<String>) {
        self.plugin_version = version.into();
    }

    pub fn recover_from_cached_pre(&mut self) -> Result<(), WaferError> {
        let node_id = self.node_id().to_string();
        let mut store = recovery_store(
            &self.store,
            &node_id,
            self.capabilities,
            self.memory_limit,
            self.epoch_deadline,
            self.fuel_limit,
        )?;
        let bindings = self.cached_pre.instantiate(&mut store).map_err(|e| WaferError::PluginInit {
            message: format!("filter '{node_id}' recovery instantiation failed: {e}"),
        })?;
        self.store = store;
        self.bindings = bindings;
        let config_json = self.config_json.clone();
        self.validate_and_init(&config_json)
    }

    /// Warm reconfigure: re-instantiate from cached InstancePre and re-run
    /// `validate() + init()` with `new_config_json`; roll back on failure.
    pub fn try_reconfigure(&mut self, new_config_json: &str) -> Result<(), WaferError> {
        let node_id = self.node_id().to_string();
        let mut new_store = recovery_store(
            &self.store,
            &node_id,
            self.capabilities,
            self.memory_limit,
            self.epoch_deadline,
            self.fuel_limit,
        )?;
        let new_bindings = self.cached_pre.instantiate(&mut new_store).map_err(|e| WaferError::PluginInit {
            message: format!("filter '{node_id}' reconfigure instantiation failed: {e}"),
        })?;
        let old_store = std::mem::replace(&mut self.store, new_store);
        let old_bindings = std::mem::replace(&mut self.bindings, new_bindings);
        let old_config = std::mem::replace(&mut self.config_json, new_config_json.to_string());
        match self.validate_and_init(new_config_json) {
            Ok(()) => Ok(()),
            Err(err) => {
                self.store = old_store;
                self.bindings = old_bindings;
                self.config_json = old_config;
                Err(err)
            }
        }
    }

    /// Call guest lifecycle validate() and init() before first message processing.
    pub fn validate_and_init(&mut self, config_json: &str) -> Result<(), WaferError> {
        let node_config = crate::engine::bindings::filter_node::exports::pipeline::node::lifecycle::NodeConfig {
            id: self.node_id().to_string(),
            config: config_json.to_string(),
            plugin_version: self.plugin_version.clone(),
        };
        // AC F5.AC2: skip metering setters when unlimited; see transform path.
        if let Some(n) = self.fuel_limit {
            self.store.set_fuel(n.get()).map_err(|e| WaferError::PluginInit {
                message: format!("filter '{}' lifecycle fuel reset failed: {e}", self.node_id()),
            })?;
        }
        if let Some(n) = self.epoch_deadline {
            self.store.set_epoch_deadline(n.get());
        }
        if let Some(message) = self
            .bindings
            .pipeline_node_lifecycle()
            .call_validate(&mut self.store, &node_config)
            .map_err(|e| WaferError::PluginInit {
                message: format!("filter '{}' validate() trapped: {e}", self.node_id()),
            })?
        {
            return Err(WaferError::PluginInit {
                message: format!("filter '{}' validate() rejected config: {message}", self.node_id()),
            });
        }
        self.bindings
            .pipeline_node_lifecycle()
            .call_init(&mut self.store, &node_config)
            .map_err(|e| WaferError::PluginInit {
                message: format!("filter '{}' init() trapped: {e}", self.node_id()),
            })?
            .map_err(|e| WaferError::PluginInit {
                message: format!("filter '{}' init() failed: {e:?}", self.node_id()),
            })?;
        flush_logs(&mut self.store);
        Ok(())
    }

    /// Evaluate whether a message should be forwarded.
    ///
    /// MUST run to completion — never place in a select! branch.
    pub fn evaluate(
        &mut self,
        envelope: &RuntimeEnvelope,
    ) -> Result<FilterOutcome, WasmProcessError> {
        self.store.data_mut().clear_log_buffer();

        // AC F5.AC2: skip metering setters when unlimited.
        if let Some(n) = self.fuel_limit {
            self.store
                .set_fuel(n.get())
                .map_err(|e| WasmProcessError::Unrecoverable(format!("fuel reset failed: {e}")))?;
        }
        // Epoch and buffer-resource lifecycle: see WasmTransformNode::process.
        if let Some(n) = self.epoch_deadline {
            self.store.set_epoch_deadline(n.get());
        }

        let wit_msg = build_wit_message(&mut self.store, envelope)?;
        let payload_rep = wit_msg.payload.rep();

        let result = self
            .bindings
            .pipeline_node_filter()
            .call_evaluate(&mut self.store, &wit_msg);

        flush_logs(&mut self.store);

        let delete_res = self
            .store
            .data_mut()
            .delete_buffer(wasmtime::component::Resource::new_own(payload_rep));
        if let Err(e) = delete_res {
            return Err(WasmProcessError::Unrecoverable(format!(
                "failed to release buffer resource: {e}"
            )));
        }

        match result {
            Ok(Ok(true)) => Ok(FilterOutcome::Forward),
            Ok(Ok(false)) => Ok(FilterOutcome::Drop),
            Ok(Err(wit_err)) => Err(map_process_error(wit_err)),
            Err(trap) => Err(map_trap(&trap)),
        }
    }

    /// Replace internals for hot-swap.
    pub fn replace(
        &mut self,
        new_store: Store<WaferState>,
        new_bindings: FilterNode,
        new_pre: Arc<FilterNodePre<WaferState>>,
    ) {
        self.store = new_store;
        self.bindings = new_bindings;
        self.cached_pre = new_pre;
    }

    /// Replace filter internals with rollback if `validate()`/`init()` fails.
    pub fn try_hot_swap(
        &mut self,
        new_store: Store<WaferState>,
        new_bindings: FilterNode,
        new_pre: Arc<FilterNodePre<WaferState>>,
    ) -> Result<(), WaferError> {
        let config_json = self.config_json.clone();
        let old_store = std::mem::replace(&mut self.store, new_store);
        let old_bindings = std::mem::replace(&mut self.bindings, new_bindings);
        let old_pre = std::mem::replace(&mut self.cached_pre, new_pre);
        match self.validate_and_init(&config_json) {
            Ok(()) => Ok(()),
            Err(err) => {
                self.store = old_store;
                self.bindings = old_bindings;
                self.cached_pre = old_pre;
                Err(err)
            }
        }
    }

    /// Get the node identity.
    pub fn node_id(&self) -> &str {
        self.store.data().node_id()
    }

    /// Access cached InstancePre for recovery.
    pub const fn cached_pre(&self) -> &Arc<FilterNodePre<WaferState>> {
        &self.cached_pre
    }

    /// Mutable store access for lifecycle calls.
    pub const fn store_mut(&mut self) -> &mut Store<WaferState> {
        &mut self.store
    }

    /// Bindings access for lifecycle calls.
    pub const fn bindings(&self) -> &FilterNode {
        &self.bindings
    }
}

// =============================================================================
// WasmRouterNode — borrows envelope, returns port list
// =============================================================================

/// Wasm router node wrapper — borrows envelope, returns routing ports.
///
/// Zero-copy routing: the host clones the envelope for fan-out based on returned ports.
pub struct WasmRouterNode {
    store: Store<WaferState>,
    bindings: RouterNode,
    cached_pre: Arc<RouterNodePre<WaferState>>,
    fuel_limit: Option<NonZeroU64>,
    capabilities: Capabilities,
    memory_limit: usize,
    epoch_deadline: Option<NonZeroU64>,
    config_json: String,
    plugin_version: String,
}

impl WasmRouterNode {
    /// Create from a pre-instantiated router binding.
    pub fn new(
        store: Store<WaferState>,
        bindings: RouterNode,
        cached_pre: Arc<RouterNodePre<WaferState>>,
        fuel_limit: Option<NonZeroU64>,
    ) -> Self {
        Self {
            store,
            bindings,
            cached_pre,
            fuel_limit,
            capabilities: Capabilities::sandbox(),
            memory_limit: 16 * 1024 * 1024,
            epoch_deadline: None,
            config_json: "{}".to_string(),
            plugin_version: String::new(),
        }
    }

    pub fn configure_runtime(
        &mut self,
        capabilities: Capabilities,
        memory_limit: usize,
        epoch_deadline: Option<NonZeroU64>,
        config_json: String,
    ) {
        self.capabilities = capabilities;
        self.memory_limit = memory_limit;
        self.epoch_deadline = epoch_deadline;
        self.config_json = config_json;
    }

    /// Set the plugin version string passed to the guest at `init()`.
    pub fn set_plugin_version(&mut self, version: impl Into<String>) {
        self.plugin_version = version.into();
    }

    pub fn recover_from_cached_pre(&mut self) -> Result<(), WaferError> {
        let node_id = self.node_id().to_string();
        let mut store = recovery_store(
            &self.store,
            &node_id,
            self.capabilities,
            self.memory_limit,
            self.epoch_deadline,
            self.fuel_limit,
        )?;
        let bindings = self.cached_pre.instantiate(&mut store).map_err(|e| WaferError::PluginInit {
            message: format!("router '{node_id}' recovery instantiation failed: {e}"),
        })?;
        self.store = store;
        self.bindings = bindings;
        let config_json = self.config_json.clone();
        self.validate_and_init(&config_json)
    }

    /// Warm reconfigure: re-instantiate from cached InstancePre and re-run
    /// `validate() + init()` with `new_config_json`; roll back on failure.
    pub fn try_reconfigure(&mut self, new_config_json: &str) -> Result<(), WaferError> {
        let node_id = self.node_id().to_string();
        let mut new_store = recovery_store(
            &self.store,
            &node_id,
            self.capabilities,
            self.memory_limit,
            self.epoch_deadline,
            self.fuel_limit,
        )?;
        let new_bindings = self.cached_pre.instantiate(&mut new_store).map_err(|e| WaferError::PluginInit {
            message: format!("router '{node_id}' reconfigure instantiation failed: {e}"),
        })?;
        let old_store = std::mem::replace(&mut self.store, new_store);
        let old_bindings = std::mem::replace(&mut self.bindings, new_bindings);
        let old_config = std::mem::replace(&mut self.config_json, new_config_json.to_string());
        match self.validate_and_init(new_config_json) {
            Ok(()) => Ok(()),
            Err(err) => {
                self.store = old_store;
                self.bindings = old_bindings;
                self.config_json = old_config;
                Err(err)
            }
        }
    }

    /// Call guest lifecycle validate() and init() before first message processing.
    pub fn validate_and_init(&mut self, config_json: &str) -> Result<(), WaferError> {
        let node_config = crate::engine::bindings::router_node::exports::pipeline::node::lifecycle::NodeConfig {
            id: self.node_id().to_string(),
            config: config_json.to_string(),
            plugin_version: self.plugin_version.clone(),
        };
        // AC F5.AC2: skip metering setters when unlimited; see transform path.
        if let Some(n) = self.fuel_limit {
            self.store.set_fuel(n.get()).map_err(|e| WaferError::PluginInit {
                message: format!("router '{}' lifecycle fuel reset failed: {e}", self.node_id()),
            })?;
        }
        if let Some(n) = self.epoch_deadline {
            self.store.set_epoch_deadline(n.get());
        }
        if let Some(message) = self
            .bindings
            .pipeline_node_lifecycle()
            .call_validate(&mut self.store, &node_config)
            .map_err(|e| WaferError::PluginInit {
                message: format!("router '{}' validate() trapped: {e}", self.node_id()),
            })?
        {
            return Err(WaferError::PluginInit {
                message: format!("router '{}' validate() rejected config: {message}", self.node_id()),
            });
        }
        self.bindings
            .pipeline_node_lifecycle()
            .call_init(&mut self.store, &node_config)
            .map_err(|e| WaferError::PluginInit {
                message: format!("router '{}' init() trapped: {e}", self.node_id()),
            })?
            .map_err(|e| WaferError::PluginInit {
                message: format!("router '{}' init() failed: {e:?}", self.node_id()),
            })?;
        flush_logs(&mut self.store);
        Ok(())
    }

    /// Decide which port(s) the message should be routed to.
    ///
    /// MUST run to completion — never place in a select! branch.
    pub fn route(
        &mut self,
        envelope: &RuntimeEnvelope,
    ) -> Result<RouteOutcome, WasmProcessError> {
        self.store.data_mut().clear_log_buffer();

        // AC F5.AC2: skip metering setters when unlimited.
        if let Some(n) = self.fuel_limit {
            self.store
                .set_fuel(n.get())
                .map_err(|e| WasmProcessError::Unrecoverable(format!("fuel reset failed: {e}")))?;
        }
        // Epoch and buffer-resource lifecycle: see WasmTransformNode::process.
        if let Some(n) = self.epoch_deadline {
            self.store.set_epoch_deadline(n.get());
        }

        let wit_msg = build_wit_message(&mut self.store, envelope)?;
        let payload_rep = wit_msg.payload.rep();

        let result = self
            .bindings
            .pipeline_routing_router()
            .call_route(&mut self.store, &wit_msg);

        flush_logs(&mut self.store);

        let delete_res = self
            .store
            .data_mut()
            .delete_buffer(wasmtime::component::Resource::new_own(payload_rep));
        if let Err(e) = delete_res {
            return Err(WasmProcessError::Unrecoverable(format!(
                "failed to release buffer resource: {e}"
            )));
        }

        match result {
            Ok(Ok(ports)) => Ok(RouteOutcome::Ports(ports)),
            Ok(Err(wit_err)) => Err(map_process_error(wit_err)),
            Err(trap) => Err(map_trap(&trap)),
        }
    }

    /// Replace internals for hot-swap.
    pub fn replace(
        &mut self,
        new_store: Store<WaferState>,
        new_bindings: RouterNode,
        new_pre: Arc<RouterNodePre<WaferState>>,
    ) {
        self.store = new_store;
        self.bindings = new_bindings;
        self.cached_pre = new_pre;
    }

    /// Replace router internals with rollback if `validate()`/`init()` fails.
    pub fn try_hot_swap(
        &mut self,
        new_store: Store<WaferState>,
        new_bindings: RouterNode,
        new_pre: Arc<RouterNodePre<WaferState>>,
    ) -> Result<(), WaferError> {
        let config_json = self.config_json.clone();
        let old_store = std::mem::replace(&mut self.store, new_store);
        let old_bindings = std::mem::replace(&mut self.bindings, new_bindings);
        let old_pre = std::mem::replace(&mut self.cached_pre, new_pre);
        match self.validate_and_init(&config_json) {
            Ok(()) => Ok(()),
            Err(err) => {
                self.store = old_store;
                self.bindings = old_bindings;
                self.cached_pre = old_pre;
                Err(err)
            }
        }
    }

    /// Get the node identity.
    pub fn node_id(&self) -> &str {
        self.store.data().node_id()
    }

    /// Access cached InstancePre for recovery.
    pub const fn cached_pre(&self) -> &Arc<RouterNodePre<WaferState>> {
        &self.cached_pre
    }

    /// Mutable store access for lifecycle calls.
    pub const fn store_mut(&mut self) -> &mut Store<WaferState> {
        &mut self.store
    }

    /// Bindings access for lifecycle calls.
    pub const fn bindings(&self) -> &RouterNode {
        &self.bindings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Capabilities, WaferEngine};

    #[test]
    fn map_process_error_all_variants() {
        use transform_node::pipeline::types::types::ProcessError as WitErr;

        let cases = [
            (WitErr::BadInput("bad".into()), "BadInput"),
            (WitErr::DependencyFailed("dep".into()), "DependencyFailed"),
            (WitErr::ProcessingFailed("proc".into()), "ProcessingFailed"),
            (WitErr::TimedOut, "TimedOut"),
            (WitErr::Unrecoverable("fatal".into()), "Unrecoverable"),
        ];

        for (wit_err, expected_variant) in cases {
            let mapped = map_process_error(wit_err);
            let debug = format!("{mapped:?}");
            assert!(
                debug.contains(expected_variant),
                "Expected {expected_variant} in {debug}"
            );
        }
    }

    #[test]
    fn map_trap_epoch_interrupt() {
        let err = wasmtime::Error::msg("wasm trap: epoch interruption");
        let mapped = map_trap(&err);
        assert!(matches!(mapped, WasmProcessError::TimedOut));
    }

    #[test]
    fn map_trap_other() {
        let err = wasmtime::Error::msg("wasm trap: unreachable instruction");
        let mapped = map_trap(&err);
        assert!(matches!(mapped, WasmProcessError::Unrecoverable(_)));
    }

    #[test]
    fn build_wit_message_creates_valid_message() {
        use crate::engine::WaferEngine;

        let engine = WaferEngine::new().expect("engine");
        let state = WaferState::new("test-node", Capabilities::sandbox());
        let mut store = Store::new(engine.inner(), state);
        store.limiter(|s| s.limits_mut());

        let envelope = RuntimeEnvelope::from_string("source-1", "hello world");
        let msg = build_wit_message(&mut store, &envelope).expect("should build message");

        assert_eq!(msg.source, envelope.header.source.to_string());
        assert_eq!(msg.timestamp, envelope.header.timestamp);
        assert_eq!(msg.content_type, "application/octet-stream");
    }

    #[test]
    fn flush_logs_emits_nothing_when_empty() {
        let engine = WaferEngine::new().expect("engine");
        let state = WaferState::new("test-node", Capabilities::sandbox());
        let mut store = Store::new(engine.inner(), state);
        store.limiter(|s| s.limits_mut());

        // Should not panic even with no logs
        flush_logs(&mut store);
    }
}
