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

use crate::engine::Capabilities;
use crate::engine::bindings::filter_node::{FilterNode, FilterNodePre};
use crate::engine::bindings::inference_node::{InferenceNode, InferenceNodePre};
use crate::engine::bindings::router_node::{RouterNode, RouterNodePre};
use crate::engine::bindings::transform_node::{self, TransformNode, TransformNodePre};
use crate::engine::state::{LogLevel, WaferState};
use crate::error::WaferError;
use crate::node::traits::{FilterOutcome, RouteOutcome};
use crate::queue::RuntimeEnvelope;
use crate::runner::error_policy::WasmProcessError;

/// Maps a WIT `process-error` variant to our host-side error type.
fn map_process_error(
    err: transform_node::wafer::pipeline::types::ProcessError,
) -> WasmProcessError {
    use transform_node::wafer::pipeline::types::ProcessError as WitErr;
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
    // variant); Display shows only the top frame. The timeout classification
    // preserves error-policy handling before the runner replaces the Store.
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
) -> Result<transform_node::wafer::pipeline::types::Message, WasmProcessError> {
    let resource_rep = store
        .data_mut()
        .push_buffer(envelope.payload.clone())
        .map_err(|e| WasmProcessError::Unrecoverable(format!("failed to push buffer: {e}")))?
        .rep();
    let resource = wasmtime::component::Resource::new_borrow(resource_rep);

    let metadata: Vec<(String, String)> =
        envelope.header.metadata.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();

    Ok(transform_node::wafer::pipeline::types::Message {
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

enum TransformBindings {
    Ordinary(TransformNode),
    Inference(InferenceNode),
}

#[derive(Clone)]
pub(crate) enum TransformPre {
    Ordinary(Arc<TransformNodePre<WaferState>>),
    Inference(Arc<InferenceNodePre<WaferState>>),
}

impl TransformPre {
    async fn instantiate_async(
        &self,
        store: &mut Store<WaferState>,
    ) -> wasmtime::Result<TransformBindings> {
        match self {
            Self::Ordinary(pre) => {
                pre.instantiate_async(store).await.map(TransformBindings::Ordinary)
            }
            Self::Inference(pre) => {
                pre.instantiate_async(store).await.map(TransformBindings::Inference)
            }
        }
    }
}

#[doc(hidden)]
pub struct PreparedTransformSwap {
    store: Store<WaferState>,
    bindings: TransformBindings,
    pre: TransformPre,
}

impl PreparedTransformSwap {
    pub(crate) const fn ordinary(
        store: Store<WaferState>,
        bindings: TransformNode,
        pre: Arc<TransformNodePre<WaferState>>,
    ) -> Self {
        Self {
            store,
            bindings: TransformBindings::Ordinary(bindings),
            pre: TransformPre::Ordinary(pre),
        }
    }

    pub(crate) const fn inference(
        store: Store<WaferState>,
        bindings: InferenceNode,
        pre: Arc<InferenceNodePre<WaferState>>,
    ) -> Self {
        Self {
            store,
            bindings: TransformBindings::Inference(bindings),
            pre: TransformPre::Inference(pre),
        }
    }

    pub(crate) const fn allows_inference(&self) -> bool {
        matches!(self.pre, TransformPre::Inference(_))
    }
}

impl TransformBindings {
    async fn call_process(
        &self,
        store: &mut Store<WaferState>,
        message: &transform_node::wafer::pipeline::types::Message,
    ) -> wasmtime::Result<
        Result<
            transform_node::wafer::pipeline::types::OutputMessage,
            transform_node::wafer::pipeline::types::ProcessError,
        >,
    > {
        match self {
            Self::Ordinary(bindings) => {
                bindings.wafer_pipeline_transform().call_process(store, message).await
            }
            Self::Inference(bindings) => {
                bindings.wafer_pipeline_transform().call_process(store, message).await
            }
        }
    }

    async fn call_validate(
        &self,
        store: &mut Store<WaferState>,
        id: &str,
        config: &str,
        plugin_version: &str,
    ) -> wasmtime::Result<Option<String>> {
        match self {
            Self::Ordinary(bindings) => {
                bindings
                    .wafer_pipeline_lifecycle()
                    .call_validate(
                        store,
                        &transform_node::exports::wafer::pipeline::lifecycle::NodeConfig {
                            id: id.to_string(),
                            config: config.to_string(),
                            plugin_version: plugin_version.to_string(),
                        },
                    )
                    .await
            }
            Self::Inference(bindings) => {
                bindings
                    .wafer_pipeline_lifecycle()
                    .call_validate(
                        store,
                        &crate::engine::bindings::inference_node::exports::wafer::pipeline::lifecycle::NodeConfig {
                            id: id.to_string(),
                            config: config.to_string(),
                            plugin_version: plugin_version.to_string(),
                        },
                    )
                    .await
            }
        }
    }

    async fn call_init(
        &self,
        store: &mut Store<WaferState>,
        id: &str,
        config: &str,
        plugin_version: &str,
    ) -> wasmtime::Result<Result<(), transform_node::wafer::pipeline::types::ProcessError>> {
        match self {
            Self::Ordinary(bindings) => {
                bindings
                    .wafer_pipeline_lifecycle()
                    .call_init(
                        store,
                        &transform_node::exports::wafer::pipeline::lifecycle::NodeConfig {
                            id: id.to_string(),
                            config: config.to_string(),
                            plugin_version: plugin_version.to_string(),
                        },
                    )
                    .await
            }
            Self::Inference(bindings) => {
                bindings
                    .wafer_pipeline_lifecycle()
                    .call_init(
                        store,
                        &crate::engine::bindings::inference_node::exports::wafer::pipeline::lifecycle::NodeConfig {
                            id: id.to_string(),
                            config: config.to_string(),
                            plugin_version: plugin_version.to_string(),
                        },
                    )
                    .await
            }
        }
    }
}

/// Wasm transform node wrapper — owns Store + bindings + cached InstancePre.
///
/// Takes ownership of the input envelope and produces a new one with transformed
/// payload bytes.
pub struct WasmTransformNode {
    store: Store<WaferState>,
    bindings: TransformBindings,
    cached_pre: TransformPre,
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
            bindings: TransformBindings::Ordinary(bindings),
            cached_pre: TransformPre::Ordinary(cached_pre),
            fuel_limit,
            capabilities: Capabilities::sandbox(),
            memory_limit: 16 * 1024 * 1024,
            epoch_deadline: None,
            config_json: "{}".to_string(),
            plugin_version: String::new(),
        }
    }

    pub(crate) fn new_inference(
        store: Store<WaferState>,
        bindings: InferenceNode,
        cached_pre: Arc<InferenceNodePre<WaferState>>,
        fuel_limit: Option<NonZeroU64>,
    ) -> Self {
        Self {
            store,
            bindings: TransformBindings::Inference(bindings),
            cached_pre: TransformPre::Inference(cached_pre),
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

    pub async fn recover_from_cached_pre(&mut self) -> Result<(), WaferError> {
        let node_id = self.node_id().to_string();
        let mut store = recovery_store(
            &self.store,
            &node_id,
            self.capabilities.clone(),
            self.memory_limit,
            self.epoch_deadline,
            self.fuel_limit,
        )?;
        let bindings = self.cached_pre.instantiate_async(&mut store).await.map_err(|e| {
            WaferError::PluginInit {
                message: format!("transform '{node_id}' recovery instantiation failed: {e}"),
            }
        })?;
        self.store = store;
        self.bindings = bindings;
        let config_json = self.config_json.clone();
        self.validate_and_init(&config_json).await
    }

    /// Warm reconfigure: re-instantiate from the cached `InstancePre` and
    /// call `validate() + init()` with `new_config_json`. No compile/instantiate
    /// of a new plugin binary. On failure, roll back to v1 state.
    pub async fn try_reconfigure(&mut self, new_config_json: &str) -> Result<(), WaferError> {
        let node_id = self.node_id().to_string();
        let mut new_store = recovery_store(
            &self.store,
            &node_id,
            self.capabilities.clone(),
            self.memory_limit,
            self.epoch_deadline,
            self.fuel_limit,
        )?;
        let new_bindings =
            self.cached_pre.instantiate_async(&mut new_store).await.map_err(|e| {
                WaferError::PluginInit {
                    message: format!("transform '{node_id}' reconfigure instantiation failed: {e}"),
                }
            })?;
        let old_store = std::mem::replace(&mut self.store, new_store);
        let old_bindings = std::mem::replace(&mut self.bindings, new_bindings);
        let old_config = std::mem::replace(&mut self.config_json, new_config_json.to_string());
        match self.validate_and_init(new_config_json).await {
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
    pub async fn process(
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

        let result = self.bindings.call_process(&mut self.store, &wit_msg).await;

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
                let mut new_envelope = RuntimeEnvelope::from_output_fields(
                    output.id.into_boxed_str(),
                    output.timestamp,
                    output.source.into_boxed_str(),
                    output.content_type.into_boxed_str(),
                    output
                        .metadata
                        .into_iter()
                        .map(|(key, value)| (key.into_boxed_str(), value.into_boxed_str()))
                        .collect(),
                    Bytes::from(output.payload),
                );
                new_envelope.inherit_lineage_from(&envelope);
                new_envelope.retry_count = envelope.retry_count;
                Ok(new_envelope)
            }
            Ok(Err(wit_err)) => Err(map_process_error(wit_err)),
            Err(trap) => Err(map_trap(&trap)),
        }
    }

    /// Call guest lifecycle validate() and init() before first message processing.
    pub async fn validate_and_init(&mut self, config_json: &str) -> Result<(), WaferError> {
        let node_id = self.node_id().to_string();
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
            .call_validate(&mut self.store, &node_id, config_json, &self.plugin_version)
            .await
            .map_err(|e| WaferError::PluginInit {
                message: format!("transform '{}' validate() trapped: {e}", self.node_id()),
            })?
        {
            return Err(WaferError::PluginInit {
                message: format!(
                    "transform '{}' validate() rejected config: {message}",
                    self.node_id()
                ),
            });
        }
        self.bindings
            .call_init(&mut self.store, &node_id, config_json, &self.plugin_version)
            .await
            .map_err(|e| WaferError::PluginInit {
                message: format!("transform '{}' init() trapped: {e}", self.node_id()),
            })?
            .map_err(|e| WaferError::PluginInit {
                message: format!("transform '{}' init() failed: {e:?}", self.node_id()),
            })?;
        flush_logs(&mut self.store);
        Ok(())
    }

    /// Replace transform internals with rollback if `validate()`/`init()` fails.
    pub(crate) async fn try_hot_swap(
        &mut self,
        replacement: PreparedTransformSwap,
    ) -> Result<(), WaferError> {
        if replacement.allows_inference() != self.capabilities.allow_inference {
            return Err(WaferError::Runtime(
                "hot-swap cannot change the node's inference capability".into(),
            ));
        }
        if replacement.store.data().capabilities() != &self.capabilities {
            return Err(WaferError::Runtime(
                "hot-swap cannot change the node's capabilities".into(),
            ));
        }
        let config_json = self.config_json.clone();
        let old_store = std::mem::replace(&mut self.store, replacement.store);
        let old_bindings = std::mem::replace(&mut self.bindings, replacement.bindings);
        let old_pre = std::mem::replace(&mut self.cached_pre, replacement.pre);
        match self.validate_and_init(&config_json).await {
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
    pub(crate) const fn cached_pre(&self) -> &TransformPre {
        &self.cached_pre
    }

    /// Replace the cached InstancePre (used by process-time rollback A17
    /// to restore v1's pre after rollback).
    pub(crate) fn set_cached_pre(&mut self, pre: TransformPre) {
        self.cached_pre = pre;
    }

    /// Get a mutable reference to the store (for lifecycle calls like init/close).
    pub const fn store_mut(&mut self) -> &mut Store<WaferState> {
        &mut self.store
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

    pub async fn recover_from_cached_pre(&mut self) -> Result<(), WaferError> {
        let node_id = self.node_id().to_string();
        let mut store = recovery_store(
            &self.store,
            &node_id,
            self.capabilities.clone(),
            self.memory_limit,
            self.epoch_deadline,
            self.fuel_limit,
        )?;
        let bindings = self.cached_pre.instantiate_async(&mut store).await.map_err(|e| {
            WaferError::PluginInit {
                message: format!("filter '{node_id}' recovery instantiation failed: {e}"),
            }
        })?;
        self.store = store;
        self.bindings = bindings;
        let config_json = self.config_json.clone();
        self.validate_and_init(&config_json).await
    }

    /// Warm reconfigure: re-instantiate from cached InstancePre and re-run
    /// `validate() + init()` with `new_config_json`; roll back on failure.
    pub async fn try_reconfigure(&mut self, new_config_json: &str) -> Result<(), WaferError> {
        let node_id = self.node_id().to_string();
        let mut new_store = recovery_store(
            &self.store,
            &node_id,
            self.capabilities.clone(),
            self.memory_limit,
            self.epoch_deadline,
            self.fuel_limit,
        )?;
        let new_bindings =
            self.cached_pre.instantiate_async(&mut new_store).await.map_err(|e| {
                WaferError::PluginInit {
                    message: format!("filter '{node_id}' reconfigure instantiation failed: {e}"),
                }
            })?;
        let old_store = std::mem::replace(&mut self.store, new_store);
        let old_bindings = std::mem::replace(&mut self.bindings, new_bindings);
        let old_config = std::mem::replace(&mut self.config_json, new_config_json.to_string());
        match self.validate_and_init(new_config_json).await {
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
    pub async fn validate_and_init(&mut self, config_json: &str) -> Result<(), WaferError> {
        let node_config =
            crate::engine::bindings::filter_node::exports::wafer::pipeline::lifecycle::NodeConfig {
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
            .wafer_pipeline_lifecycle()
            .call_validate(&mut self.store, &node_config)
            .await
            .map_err(|e| WaferError::PluginInit {
                message: format!("filter '{}' validate() trapped: {e}", self.node_id()),
            })?
        {
            return Err(WaferError::PluginInit {
                message: format!(
                    "filter '{}' validate() rejected config: {message}",
                    self.node_id()
                ),
            });
        }
        self.bindings
            .wafer_pipeline_lifecycle()
            .call_init(&mut self.store, &node_config)
            .await
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
    pub async fn evaluate(
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

        let result =
            self.bindings.wafer_pipeline_filter().call_evaluate(&mut self.store, &wit_msg).await;

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
    pub async fn try_hot_swap(
        &mut self,
        new_store: Store<WaferState>,
        new_bindings: FilterNode,
        new_pre: Arc<FilterNodePre<WaferState>>,
    ) -> Result<(), WaferError> {
        if new_store.data().capabilities() != &self.capabilities {
            return Err(WaferError::Runtime(
                "hot-swap cannot change the node's capabilities".into(),
            ));
        }
        let config_json = self.config_json.clone();
        let old_store = std::mem::replace(&mut self.store, new_store);
        let old_bindings = std::mem::replace(&mut self.bindings, new_bindings);
        let old_pre = std::mem::replace(&mut self.cached_pre, new_pre);
        match self.validate_and_init(&config_json).await {
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

    pub async fn recover_from_cached_pre(&mut self) -> Result<(), WaferError> {
        let node_id = self.node_id().to_string();
        let mut store = recovery_store(
            &self.store,
            &node_id,
            self.capabilities.clone(),
            self.memory_limit,
            self.epoch_deadline,
            self.fuel_limit,
        )?;
        let bindings = self.cached_pre.instantiate_async(&mut store).await.map_err(|e| {
            WaferError::PluginInit {
                message: format!("router '{node_id}' recovery instantiation failed: {e}"),
            }
        })?;
        self.store = store;
        self.bindings = bindings;
        let config_json = self.config_json.clone();
        self.validate_and_init(&config_json).await
    }

    /// Warm reconfigure: re-instantiate from cached InstancePre and re-run
    /// `validate() + init()` with `new_config_json`; roll back on failure.
    pub async fn try_reconfigure(&mut self, new_config_json: &str) -> Result<(), WaferError> {
        let node_id = self.node_id().to_string();
        let mut new_store = recovery_store(
            &self.store,
            &node_id,
            self.capabilities.clone(),
            self.memory_limit,
            self.epoch_deadline,
            self.fuel_limit,
        )?;
        let new_bindings =
            self.cached_pre.instantiate_async(&mut new_store).await.map_err(|e| {
                WaferError::PluginInit {
                    message: format!("router '{node_id}' reconfigure instantiation failed: {e}"),
                }
            })?;
        let old_store = std::mem::replace(&mut self.store, new_store);
        let old_bindings = std::mem::replace(&mut self.bindings, new_bindings);
        let old_config = std::mem::replace(&mut self.config_json, new_config_json.to_string());
        match self.validate_and_init(new_config_json).await {
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
    pub async fn validate_and_init(&mut self, config_json: &str) -> Result<(), WaferError> {
        let node_config =
            crate::engine::bindings::router_node::exports::wafer::pipeline::lifecycle::NodeConfig {
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
            .wafer_pipeline_lifecycle()
            .call_validate(&mut self.store, &node_config)
            .await
            .map_err(|e| WaferError::PluginInit {
                message: format!("router '{}' validate() trapped: {e}", self.node_id()),
            })?
        {
            return Err(WaferError::PluginInit {
                message: format!(
                    "router '{}' validate() rejected config: {message}",
                    self.node_id()
                ),
            });
        }
        self.bindings
            .wafer_pipeline_lifecycle()
            .call_init(&mut self.store, &node_config)
            .await
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
    pub async fn route(
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

        let result =
            self.bindings.wafer_pipeline_router().call_route(&mut self.store, &wit_msg).await;

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
    pub async fn try_hot_swap(
        &mut self,
        new_store: Store<WaferState>,
        new_bindings: RouterNode,
        new_pre: Arc<RouterNodePre<WaferState>>,
    ) -> Result<(), WaferError> {
        if new_store.data().capabilities() != &self.capabilities {
            return Err(WaferError::Runtime(
                "hot-swap cannot change the node's capabilities".into(),
            ));
        }
        let config_json = self.config_json.clone();
        let old_store = std::mem::replace(&mut self.store, new_store);
        let old_bindings = std::mem::replace(&mut self.bindings, new_bindings);
        let old_pre = std::mem::replace(&mut self.cached_pre, new_pre);
        match self.validate_and_init(&config_json).await {
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
    use crate::config::{EngineConfig, FuelBudgets, MemoryLimits};
    use crate::engine::{Capabilities, WaferEngine};
    use std::sync::Mutex;
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Subscriber};
    use wafer_types::config::{CanonicalHttpDestination, HttpHost, HttpScheme};

    const MNIST_FUEL: u64 = 100_000_000;

    fn loopback_http_destination(port: u16) -> CanonicalHttpDestination {
        CanonicalHttpDestination {
            scheme: HttpScheme::Http,
            host: HttpHost::Ip(std::net::Ipv4Addr::LOCALHOST.into()),
            port,
        }
    }
    const MNIST_MEMORY: usize = 64 * 1024 * 1024;
    const MNIST_COMPONENT: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../wafer-runtime/tests/fixtures/mnist-inference.component.bin"
    );
    const ORDINARY_TRAP_COMPONENT: &[u8] =
        include_bytes!("../../tests/fixtures/transform-panics.component.bin");
    const MNIST_DIGIT: &[u8] = include_bytes!("../../../../tests/fixtures/digit_7.bin");

    #[derive(Clone, Default)]
    struct EventRecorder {
        events: Arc<Mutex<Vec<String>>>,
    }

    struct EventVisitor<'a> {
        values: &'a mut Vec<String>,
    }

    impl Visit for EventVisitor<'_> {
        fn record_debug(&mut self, _field: &Field, value: &dyn std::fmt::Debug) {
            self.values.push(format!("{value:?}"));
        }
    }

    impl Subscriber for EventRecorder {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut values = self.events.lock().unwrap();
            event.record(&mut EventVisitor { values: &mut values });
        }

        fn enter(&self, _span: &Id) {}

        fn exit(&self, _span: &Id) {}
    }

    #[test]
    fn map_process_error_all_variants() {
        use transform_node::wafer::pipeline::types::ProcessError as WitErr;

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
            assert!(debug.contains(expected_variant), "Expected {expected_variant} in {debug}");
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
        assert!(!msg.payload.owned(), "borrow<buffer> must use a borrowed host handle");
    }

    #[test]
    fn flush_logs_emits_nothing_when_empty() {
        let engine = WaferEngine::new().expect("engine");
        let state = WaferState::new("test-node", Capabilities::sandbox());
        let mut store = Store::new(engine.inner(), state);
        store.limiter(|s| s.limits_mut());

        flush_logs(&mut store);
    }

    #[test]
    fn flush_logs_emits_and_drains_current_entries() {
        let engine = WaferEngine::new().expect("engine");
        let state = WaferState::new("test-node", Capabilities::sandbox());
        let mut store = Store::new(engine.inner(), state);
        store.limiter(|s| s.limits_mut());
        store.data_mut().push_log(LogLevel::Warn, "visible-current-call".into());
        let recorder = EventRecorder::default();
        let events = Arc::clone(&recorder.events);

        tracing::subscriber::with_default(recorder, || flush_logs(&mut store));

        assert!(!store.data().has_logs(), "flush must drain the current call's log entries");
        assert!(
            events.lock().unwrap().iter().any(|value| value.contains("visible-current-call")),
            "flush must emit the guest log entry"
        );
    }

    fn mnist_engine() -> anyhow::Result<WaferEngine> {
        let config = EngineConfig {
            epoch_deadline: NonZeroU64::new(1000),
            fuel: FuelBudgets { transform: NonZeroU64::new(MNIST_FUEL), ..FuelBudgets::default() },
            memory: MemoryLimits { transform: MNIST_MEMORY, ..MemoryLimits::default() },
            ..EngineConfig::default()
        };
        let engine = WaferEngine::from_engine_config(&config)?;
        engine.ensure_epoch_ticker();
        Ok(engine)
    }

    async fn mnist_node_with_engine(engine: &WaferEngine) -> anyhow::Result<WasmTransformNode> {
        let component = engine.load_component(MNIST_COMPONENT)?;
        let pre = Arc::new(engine.pre_instantiate_inference(&component)?);
        let capabilities = Capabilities::sandbox().inference(true);
        let state = WaferState::new_with_memory_limit("mnist", capabilities.clone(), MNIST_MEMORY);
        let mut store = Store::new(engine.inner(), state);
        store.limiter(|state| state.limits_mut());
        store.set_fuel(MNIST_FUEL)?;
        store.epoch_deadline_trap();
        store.set_epoch_deadline(1000);
        let bindings = pre.instantiate_async(&mut store).await?;
        let mut node =
            WasmTransformNode::new_inference(store, bindings, pre, NonZeroU64::new(MNIST_FUEL));
        node.configure_runtime(
            capabilities,
            MNIST_MEMORY,
            NonZeroU64::new(1000),
            r#"{"execution_target":"cpu"}"#.into(),
        );
        node.set_plugin_version("mnist-cpu-v1");
        node.validate_and_init(r#"{"execution_target":"cpu"}"#).await?;
        Ok(node)
    }

    async fn mnist_node() -> anyhow::Result<WasmTransformNode> {
        mnist_node_with_engine(&mnist_engine()?).await
    }

    async fn mnist_prediction(node: &mut WasmTransformNode) -> anyhow::Result<(usize, Bytes)> {
        let output = node
            .process(RuntimeEnvelope::new("fixture-source", Bytes::from_static(MNIST_DIGIT)))
            .await?;
        let logits = output
            .payload
            .as_chunks::<{ size_of::<f32>() }>()
            .0
            .iter()
            .map(|chunk| f32::from_le_bytes(*chunk))
            .collect::<Vec<_>>();
        let prediction = logits
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.total_cmp(b))
            .map(|(index, _)| index)
            .ok_or_else(|| anyhow::anyhow!("inference returned no logits"))?;
        Ok((prediction, output.payload))
    }

    async fn trapping_node_with_capabilities(
        capabilities: Capabilities,
    ) -> anyhow::Result<WasmTransformNode> {
        let fuel = NonZeroU64::new(10_000_000);
        let epoch = NonZeroU64::new(100);
        let memory_limit = 32 * 1024 * 1024;
        let config = EngineConfig {
            epoch_deadline: epoch,
            fuel: FuelBudgets { transform: fuel, ..FuelBudgets::default() },
            memory: MemoryLimits { transform: memory_limit, ..MemoryLimits::default() },
            ..EngineConfig::default()
        };
        let engine = WaferEngine::from_engine_config(&config)?;
        engine.ensure_epoch_ticker();
        let component = engine.load_component_from_bytes(ORDINARY_TRAP_COMPONENT, "trap")?;
        let pre = Arc::new(engine.pre_instantiate_transform(&component)?);
        let state = WaferState::new_with_memory_limit("trap", capabilities.clone(), memory_limit);
        let mut store = Store::new(engine.inner(), state);
        store.limiter(|state| state.limits_mut());
        store.set_fuel(fuel.unwrap().get())?;
        store.epoch_deadline_trap();
        store.set_epoch_deadline(epoch.unwrap().get());
        let bindings = pre.instantiate_async(&mut store).await?;
        let mut node = WasmTransformNode::new(store, bindings, pre, fuel);
        node.configure_runtime(capabilities, memory_limit, epoch, r#"{"mode":"trap"}"#.into());
        node.set_plugin_version("trap-v1");
        node.validate_and_init(r#"{"mode":"trap"}"#).await?;
        Ok(node)
    }

    async fn trapping_node() -> anyhow::Result<WasmTransformNode> {
        trapping_node_with_capabilities(
            Capabilities::sandbox()
                .inference(true)
                .outbound_http(vec![loopback_http_destination(8080)]),
        )
        .await
    }

    #[tokio::test]
    async fn process_clears_stale_logs_before_guest_call() -> anyhow::Result<()> {
        let mut node = mnist_node().await?;
        node.store.data_mut().push_log(LogLevel::Error, "stale-previous-call".into());
        let recorder = EventRecorder::default();
        let events = Arc::clone(&recorder.events);

        let _subscriber = tracing::subscriber::set_default(recorder);
        let result = node
            .process(RuntimeEnvelope::new("fixture-source", Bytes::from_static(MNIST_DIGIT)))
            .await;

        anyhow::ensure!(result.is_ok(), "inference call failed: {result:?}");
        anyhow::ensure!(!node.store.data().has_logs(), "call left buffered logs");
        let stale_log_emitted = events
            .lock()
            .map_err(|error| anyhow::anyhow!("event recorder lock poisoned: {error}"))?
            .iter()
            .any(|value| value.contains("stale-previous-call"));
        anyhow::ensure!(!stale_log_emitted, "stale log was emitted by the next call");
        Ok(())
    }

    #[tokio::test]
    async fn trap_recovery_rebuilds_the_runtime_contract_from_cached_pre() -> anyhow::Result<()> {
        let mut node = trapping_node().await?;
        let first_slot = node.store.data_mut().push_buffer(Bytes::from_static(b"probe"))?;
        let expected_first_slot = first_slot.rep();
        node.store.data_mut().delete_buffer(first_slot)?;
        let first = node
            .process(RuntimeEnvelope::from_string("fixture-source", "first"))
            .await
            .expect_err("fixture must trap");
        anyhow::ensure!(matches!(first, WasmProcessError::Unrecoverable(_)));
        let reused_first_slot = node.store.data_mut().push_buffer(Bytes::from_static(b"probe"))?;
        anyhow::ensure!(
            reused_first_slot.rep() == expected_first_slot,
            "trap leaked the borrowed input buffer slot"
        );
        node.store.data_mut().delete_buffer(reused_first_slot)?;

        node.store.data_mut().push_log(LogLevel::Error, "old-store".into());
        node.store.data_mut().push_buffer(Bytes::from_static(b"old-store-buffer"))?;
        let old_state = std::ptr::from_ref(node.store.data());
        let TransformPre::Ordinary(old_pre) = &node.cached_pre else {
            anyhow::bail!("trap fixture unexpectedly used inference bindings");
        };
        let old_pre = Arc::as_ptr(old_pre);

        node.recover_from_cached_pre().await?;

        anyhow::ensure!(!std::ptr::eq(old_state, node.store.data()), "Store state was reused");
        anyhow::ensure!(node.store.data().table().is_empty(), "old Store resources survived");
        anyhow::ensure!(!node.store.data().has_logs(), "old Store logs survived");
        anyhow::ensure!(node.capabilities.allow_inference, "capability grant changed");
        anyhow::ensure!(
            node.store.data().capabilities() == &node.capabilities,
            "recovery changed the outbound HTTP grant"
        );
        anyhow::ensure!(node.memory_limit == 32 * 1024 * 1024, "memory limit changed");
        anyhow::ensure!(node.fuel_limit == NonZeroU64::new(10_000_000), "fuel changed");
        anyhow::ensure!(node.epoch_deadline == NonZeroU64::new(100), "epoch changed");
        anyhow::ensure!(node.config_json == r#"{"mode":"trap"}"#, "config changed");
        anyhow::ensure!(node.plugin_version == "trap-v1", "plugin version changed");
        let reconfigure = r#"{"mode":"trap-reconfigured","outbound_http":[{"scheme":"http","host":"127.0.0.1","port":9090}]}"#;
        node.try_reconfigure(reconfigure).await?;
        anyhow::ensure!(
            node.store.data().capabilities() == &node.capabilities,
            "reconfigure changed the outbound HTTP grant"
        );
        anyhow::ensure!(node.config_json == reconfigure);
        let TransformPre::Ordinary(recovered_pre) = &node.cached_pre else {
            anyhow::bail!("recovery unexpectedly changed to inference bindings");
        };
        let recovered_pre = Arc::as_ptr(recovered_pre);
        anyhow::ensure!(old_pre == recovered_pre, "cached InstancePre changed");
        anyhow::ensure!(node.store.get_fuel()? <= 10_000_000, "fuel not configured");
        anyhow::ensure!(
            wasmtime::ResourceLimiter::memory_growing(
                node.store.data_mut().limits_mut(),
                32 * 1024 * 1024,
                32 * 1024 * 1024 + 1,
                None,
            )
            .is_err(),
            "memory limiter accepted over-limit growth"
        );

        let second_slot = node.store.data_mut().push_buffer(Bytes::from_static(b"probe"))?;
        let expected_second_slot = second_slot.rep();
        node.store.data_mut().delete_buffer(second_slot)?;
        let second = node
            .process(RuntimeEnvelope::from_string("fixture-source", "second"))
            .await
            .expect_err("recovered fixture must independently trap");
        anyhow::ensure!(matches!(second, WasmProcessError::Unrecoverable(_)));
        let reused_second_slot = node.store.data_mut().push_buffer(Bytes::from_static(b"probe"))?;
        anyhow::ensure!(
            reused_second_slot.rep() == expected_second_slot,
            "recovered trap leaked the borrowed input buffer slot"
        );
        node.store.data_mut().delete_buffer(reused_second_slot)?;
        Ok(())
    }

    #[tokio::test]
    async fn hot_swap_rejects_outbound_http_expansion() -> anyhow::Result<()> {
        let original = Capabilities::sandbox().outbound_http(vec![loopback_http_destination(8080)]);
        let mut node = trapping_node_with_capabilities(original.clone()).await?;
        let expanded = Capabilities::sandbox()
            .outbound_http(vec![loopback_http_destination(8080), loopback_http_destination(8081)]);
        let mut store = recovery_store(
            &node.store,
            "trap",
            expanded,
            node.memory_limit,
            node.epoch_deadline,
            node.fuel_limit,
        )?;
        let TransformPre::Ordinary(pre) = &node.cached_pre else {
            anyhow::bail!("trap fixture unexpectedly used inference bindings");
        };
        let bindings = pre.instantiate_async(&mut store).await?;
        let replacement = PreparedTransformSwap::ordinary(store, bindings, Arc::clone(pre));

        let error = node
            .try_hot_swap(replacement)
            .await
            .expect_err("hot-swap must reject capability expansion");

        anyhow::ensure!(error.to_string().contains("cannot change the node's capabilities"));
        anyhow::ensure!(node.capabilities == original);
        anyhow::ensure!(node.store.data().capabilities() == &original);
        Ok(())
    }

    #[tokio::test]
    async fn inference_recovery_and_reconfigure_keep_real_model_live() -> anyhow::Result<()> {
        let mut node = mnist_node().await?;
        let (initial_prediction, initial_payload) = mnist_prediction(&mut node).await?;

        node.recover_from_cached_pre().await?;
        let (recovered_prediction, recovered_payload) = mnist_prediction(&mut node).await?;
        anyhow::ensure!(recovered_prediction == 7, "recovery changed prediction");
        anyhow::ensure!(recovered_payload == initial_payload, "recovery changed logits");

        node.try_reconfigure("{}").await?;
        let (reconfigured_prediction, reconfigured_payload) = mnist_prediction(&mut node).await?;
        anyhow::ensure!(initial_prediction == 7 && reconfigured_prediction == 7);
        anyhow::ensure!(reconfigured_payload == initial_payload, "reconfigure changed logits");
        anyhow::ensure!(node.capabilities.allow_inference, "inference grant was downgraded");
        anyhow::ensure!(matches!(node.cached_pre, TransformPre::Inference(_)));
        Ok(())
    }

    #[tokio::test]
    async fn rejected_inference_reconfigure_restores_the_prior_store() -> anyhow::Result<()> {
        let mut node = mnist_node().await?;
        let (_, expected_payload) = mnist_prediction(&mut node).await?;

        let error = node.try_reconfigure(r#"{"execution_target":"cuda"}"#).await.unwrap_err();
        anyhow::ensure!(error.to_string().contains("unknown variant `cuda`"), "{error}");
        let (prediction, payload) = mnist_prediction(&mut node).await?;
        anyhow::ensure!(prediction == 7, "rollback changed prediction");
        anyhow::ensure!(payload == expected_payload, "rollback changed logits");
        anyhow::ensure!(node.config_json == r#"{"execution_target":"cpu"}"#);
        anyhow::ensure!(node.capabilities.allow_inference);
        Ok(())
    }

    #[tokio::test]
    async fn inference_hot_swap_adopts_real_component_between_calls() -> anyhow::Result<()> {
        let engine = mnist_engine()?;
        let node = mnist_node_with_engine(&engine).await?;
        let (progress, _completion) = crate::runner::HotSwapProgress::channel();
        let bytes = std::fs::read(MNIST_COMPONENT)?;
        let replacement = crate::orchestrator::hotswap::prepare_transform_swap_timed_with_fuel(
            &engine,
            &bytes,
            "mnist",
            Capabilities::sandbox().inference(true),
            MNIST_MEMORY,
            NonZeroU64::new(MNIST_FUEL),
            progress,
        )
        .await?;
        let mut node = crate::node::TransformNode::from(node);

        replacement.payload.try_apply_transform(&mut node).await?;
        let output = node
            .process(RuntimeEnvelope::new("fixture-source", Bytes::from_static(MNIST_DIGIT)))
            .await?;
        let logits = output
            .payload
            .as_chunks::<{ size_of::<f32>() }>()
            .0
            .iter()
            .map(|chunk| f32::from_le_bytes(*chunk))
            .collect::<Vec<_>>();
        anyhow::ensure!(
            logits.iter().enumerate().max_by(|(_, a), (_, b)| a.total_cmp(b)).map(|(i, _)| i)
                == Some(7),
            "replacement changed prediction"
        );
        anyhow::ensure!(node.as_wasm_mut().is_some_and(|wasm| {
            wasm.capabilities.allow_inference
                && matches!(wasm.cached_pre, TransformPre::Inference(_))
        }));
        Ok(())
    }

    #[tokio::test]
    async fn inference_hot_swap_cannot_downgrade_the_frozen_grant() -> anyhow::Result<()> {
        let engine = mnist_engine()?;
        let node = mnist_node_with_engine(&engine).await?;
        let (progress, _completion) = crate::runner::HotSwapProgress::channel();
        let replacement = crate::orchestrator::hotswap::prepare_transform_swap_timed_with_fuel(
            &engine,
            ORDINARY_TRAP_COMPONENT,
            "mnist",
            Capabilities::sandbox(),
            MNIST_MEMORY,
            NonZeroU64::new(MNIST_FUEL),
            progress,
        )
        .await?;
        let mut node = crate::node::TransformNode::from(node);

        let error = replacement
            .payload
            .try_apply_transform(&mut node)
            .await
            .expect_err("replacement cannot remove inference from a granted node");
        anyhow::ensure!(
            error.to_string().contains("cannot change the node's inference capability")
        );
        let wasm = node.as_wasm_mut().expect("Wasm transform");
        let (prediction, _) = mnist_prediction(wasm).await?;
        anyhow::ensure!(prediction == 7, "rejected replacement changed v1");
        Ok(())
    }

    #[tokio::test]
    async fn mnist_inference_preserves_envelope_contract_and_runtime_limits() -> anyhow::Result<()>
    {
        let mut node = mnist_node().await?;
        let mut input = RuntimeEnvelope::new("fixture-source", Bytes::from_static(MNIST_DIGIT));
        input.set_parent_id("parent-7");
        input.ensure_trace_id();
        input.retry_count = 3;
        let trace_id = input.trace_id().expect("trace id").to_string();
        let header = Arc::make_mut(&mut input.header);
        header.id = "fixture-id".into();
        header.timestamp = 1_700_000_000_000_000_007;
        header.content_type = "image/x-mnist".into();
        header.metadata = vec![
            ("fixture.digit".into(), "7".into()),
            ("inference.execution_target.requested".into(), "gpu".into()),
            ("plugin.version".into(), "stale".into()),
        ];

        let output = node.process(input).await?;
        anyhow::ensure!(&*output.header.id == "fixture-id", "guest id changed");
        anyhow::ensure!(output.header.timestamp == 1_700_000_000_000_000_007);
        anyhow::ensure!(&*output.header.source == "fixture-source", "guest source changed");
        anyhow::ensure!(&*output.header.content_type == "application/vnd.wafer.mnist-logits");
        anyhow::ensure!(
            output.header.metadata
                == vec![
                    ("fixture.digit".into(), "7".into()),
                    ("inference.execution_target.requested".into(), "cpu".into()),
                    ("plugin.version".into(), "mnist-cpu-v1".into()),
                ],
            "guest metadata changed"
        );
        anyhow::ensure!(output.parent_id() == Some("parent-7"), "parent lineage changed");
        anyhow::ensure!(output.trace_id() == Some(trace_id.as_str()), "trace lineage changed");
        anyhow::ensure!(output.retry_count == 3, "retry count changed");
        anyhow::ensure!(output.payload.len() == 10 * size_of::<f32>(), "wrong output size");
        let logits = output
            .payload
            .as_chunks::<{ size_of::<f32>() }>()
            .0
            .iter()
            .map(|chunk| f32::from_le_bytes(*chunk))
            .collect::<Vec<_>>();
        anyhow::ensure!(logits.iter().all(|logit| logit.is_finite()), "non-finite logit");
        anyhow::ensure!(
            logits.iter().enumerate().max_by(|(_, a), (_, b)| a.total_cmp(b)).map(|(i, _)| i)
                == Some(7),
            "wrong prediction"
        );
        anyhow::ensure!(node.store_mut().get_fuel()? < MNIST_FUEL, "no fuel consumed");
        anyhow::ensure!(node.memory_limit == MNIST_MEMORY, "memory limit changed");
        anyhow::ensure!(node.epoch_deadline == NonZeroU64::new(1000));
        anyhow::ensure!(node.capabilities.allow_inference, "inference grant missing");
        anyhow::ensure!(
            wasmtime::ResourceLimiter::memory_growing(
                node.store_mut().data_mut().limits_mut(),
                MNIST_MEMORY,
                MNIST_MEMORY + 1,
                None,
            )
            .is_err(),
            "memory limiter accepted over-limit growth"
        );
        Ok(())
    }
}
