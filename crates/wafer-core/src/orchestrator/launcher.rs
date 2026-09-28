//! Pipeline launcher — single entry point from Config to running orchestrator.
//!
//! Absorbs all startup orchestration: engine creation, plugin resolution,
//! Wasm compilation, source/sink construction, and topology wiring.

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use wasmtime::Store;

use crate::config::{
    BenchSinkConfigToml, BenchSourceConfigToml, Capabilities as ConfigCapabilities, Config,
    NodeDef, SinkDef, SourceDef, WasmNodeDef,
};
use crate::engine::{Capabilities, WaferEngine, WaferState};
use crate::error::{ConfigError, Result, WaferError};
use crate::node::wasm::{WasmFilterNode, WasmRouterNode, WasmTransformNode};
use crate::node::{
    BenchBurstSchedule, BenchSink, BenchSinkConfig, BenchSource, BenchSourceConfig, FileSink,
    FileSource, HttpSink, HttpSource, MqttSink, MqttSource, Sink, Source, StdinSource, StdoutSink,
};
use crate::orchestrator::builder::{NodeBundleKind, build_pipeline_with_io};
use crate::orchestrator::pipeline::PipelineOrchestrator;
use crate::registry::{OciReference, PluginSource, RegistryConfig, ResolvedPlugin, WaferRegistry};

/// Aggregate monotonic durations for one pipeline launch.
#[derive(Debug, Clone, Copy, Default)]
pub struct LaunchTimings {
    component_load_compile: Duration,
    instantiation: Duration,
    pipeline_setup: Duration,
}

impl LaunchTimings {
    /// Time spent resolving, reading, hashing, and compiling Wasm components.
    #[must_use]
    pub fn component_load_compile_ns(self) -> u64 {
        duration_ns(self.component_load_compile)
    }

    /// Time spent pre-linking, instantiating, validating, and initializing Wasm nodes.
    #[must_use]
    pub fn instantiation_ns(self) -> u64 {
        duration_ns(self.instantiation)
    }

    /// Remaining launch time spent creating the engine, topology, I/O, and tasks.
    #[must_use]
    pub fn pipeline_setup_ns(self) -> u64 {
        duration_ns(self.pipeline_setup)
    }
}

/// A running pipeline paired with its completed launch timings.
pub struct TimedPipelineLaunch {
    orchestrator: PipelineOrchestrator,
    timings: LaunchTimings,
}

impl TimedPipelineLaunch {
    /// Split the running orchestrator from its immutable launch timing snapshot.
    #[must_use]
    pub fn into_parts(self) -> (PipelineOrchestrator, LaunchTimings) {
        (self.orchestrator, self.timings)
    }
}

fn duration_ns(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

/// Launch a fully-wired pipeline from configuration.
///
/// Performs the complete startup sequence:
/// 1. Creates `WaferEngine` with OS-thread epoch ticker
/// 2. Resolves and compiles all Wasm plugins via `WaferRegistry`
/// 3. Creates Source/Sink instances from typed config variants
/// 4. Builds pipeline topology via `build_pipeline_with_io()`
/// 5. Injects compiled Wasm node instances into `BuildOutput` bundles
/// 6. Returns a running `PipelineOrchestrator`
///
/// # Errors
///
/// Returns error if engine creation, plugin loading, source/sink creation,
/// or pipeline building fails.
#[expect(
    clippy::large_futures,
    reason = "pipeline launch holds WASM Store/Component across sequential .await points; called once at startup, not per-message hot path"
)]
pub async fn launch_pipeline(
    config: Config,
    config_path: Option<&Path>,
) -> Result<PipelineOrchestrator> {
    Ok(launch_pipeline_timed(config, config_path).await?.orchestrator)
}

/// Launch a pipeline and report non-overlapping startup phase durations.
///
/// # Errors
///
/// Returns the same startup errors as [`launch_pipeline`].
#[expect(
    clippy::large_futures,
    reason = "pipeline launch holds WASM Store/Component across sequential .await points; called once at startup, not per-message hot path"
)]
pub async fn launch_pipeline_timed(
    config: Config,
    config_path: Option<&Path>,
) -> Result<TimedPipelineLaunch> {
    let launch_started = Instant::now();
    let mut timings = LaunchTimings::default();
    let engine = WaferEngine::from_engine_config(&config.engine)?;
    engine.ensure_epoch_ticker();
    let engine = Arc::new(engine);

    let registry_config =
        config.registry.as_ref().map_or_else(RegistryConfig::default, |cfg| RegistryConfig {
            cache_dir: cfg.cache_dir.as_ref().map(PathBuf::from),
            ..Default::default()
        });
    let registry = WaferRegistry::new(registry_config).map_err(WaferError::Registry)?;

    let mut sources: HashMap<String, Box<dyn Source + Send>> = HashMap::new();
    let mut sinks: HashMap<String, Box<dyn Sink + Send>> = HashMap::new();

    for (node_id, node_def) in &config.nodes {
        match node_def {
            NodeDef::Source(source_def) => {
                sources.insert(node_id.clone(), create_source(node_id, source_def)?);
            }
            NodeDef::Sink(sink_def) => {
                sinks.insert(node_id.clone(), create_sink(node_id, sink_def)?);
            }
            NodeDef::Transform(_) | NodeDef::Filter(_) | NodeDef::Router(_) => {}
        }
    }

    let mut build_output = build_pipeline_with_io(&config, sources, sinks)?;

    // Seed with the SHA256 of every Wasm plugin loaded at launch so the P0.12
    // hot-swap guard and metadata.json provenance share one source of truth.
    let mut plugin_hashes: HashMap<Box<str>, String> = HashMap::new();

    for bundle in &mut build_output.node_bundles {
        let Some(node_def) = config.nodes.get(&*bundle.node_id) else {
            continue;
        };

        match (&mut bundle.kind, node_def) {
            (NodeBundleKind::Transform { node, .. }, NodeDef::Transform(wasm)) => {
                *node = Some(
                    load_transform_node_dispatch(
                        &bundle.node_id,
                        wasm,
                        config.engine.fuel.transform,
                        config.engine.memory.transform,
                        &engine,
                        &registry,
                        config_path,
                        &mut plugin_hashes,
                        &mut timings,
                    )
                    .await?,
                );
            }
            (NodeBundleKind::Filter { node, .. }, NodeDef::Filter(wasm)) => {
                *node = Some(
                    load_filter_node_dispatch(
                        &bundle.node_id,
                        wasm,
                        config.engine.fuel.filter,
                        config.engine.memory.filter,
                        &engine,
                        &registry,
                        config_path,
                        &mut plugin_hashes,
                        &mut timings,
                    )
                    .await?,
                );
            }
            (NodeBundleKind::Router { node, .. }, NodeDef::Router(wasm)) => {
                *node = Some(
                    load_router_node(
                        &bundle.node_id,
                        wasm,
                        config.engine.fuel.router,
                        config.engine.memory.router,
                        &engine,
                        &registry,
                        config_path,
                        &mut plugin_hashes,
                        &mut timings,
                    )
                    .await?,
                );
            }
            _ => {}
        }
    }

    mark_replacement_eligible(&mut build_output);

    let orchestrator = PipelineOrchestrator::from_build_output(build_output, config, engine);
    let handle = orchestrator.handle();
    for (node_id, hash) in plugin_hashes {
        handle.record_plugin_hash(&node_id, hash);
    }
    timings.pipeline_setup = launch_started
        .elapsed()
        .saturating_sub(timings.component_load_compile)
        .saturating_sub(timings.instantiation);
    Ok(TimedPipelineLaunch { orchestrator, timings })
}

fn mark_replacement_eligible(build_output: &mut crate::orchestrator::builder::BuildOutput) {
    for bundle in &build_output.node_bundles {
        let is_loaded_wasm = match &bundle.kind {
            NodeBundleKind::Transform { node: Some(node), .. } => !node.is_native(),
            NodeBundleKind::Filter { node: Some(node), .. } => !node.is_native(),
            NodeBundleKind::Router { node: Some(_), .. } => true,
            _ => false,
        };
        if is_loaded_wasm {
            build_output.replacement_eligible.insert(bundle.node_id.clone());
        }
    }
}

// =============================================================================
// Source Factory
// =============================================================================

/// Build a source and run its `validate()`, so a bad adapter config fails the
/// launch instead of producing an empty or silently degraded run.
fn create_source(node_id: &str, source_def: &SourceDef) -> Result<Box<dyn Source + Send>> {
    let source: Box<dyn Source + Send> = match source_def {
        SourceDef::Stdin(_) => Box::new(StdinSource::new(node_id)),
        SourceDef::File(cfg) => Box::new(FileSource::new(node_id, &cfg.path)),
        SourceDef::Mqtt(cfg) => {
            let client_id = cfg.client_id.clone().unwrap_or_else(|| format!("wafer-{node_id}"));
            Box::new(MqttSource::new(
                node_id,
                &cfg.broker,
                cfg.port,
                &cfg.topic,
                cfg.qos,
                client_id,
            ))
        }
        SourceDef::Http(cfg) => Box::new(HttpSource::new(node_id, &cfg.bind, &cfg.path)),
        SourceDef::BenchSource(cfg) => Box::new(bench_source_from_toml(node_id, cfg)),
    };
    source.validate().map_err(|e| invalid_io_node("source", node_id, &e))?;
    Ok(source)
}

fn bench_source_from_toml(node_id: &str, cfg: &BenchSourceConfigToml) -> BenchSource {
    let mut core = BenchSourceConfig::new(cfg.rate, cfg.total_messages)
        .with_warmup(cfg.warmup_messages)
        .with_payload_size(cfg.payload_size)
        .with_evidence_dir(std::env::var_os("WAFER_BENCH_OUTPUT_DIR").map(PathBuf::from));
    if let Some(burst) = &cfg.burst {
        core =
            core.with_burst(BenchBurstSchedule::new(burst.rate, burst.start_secs, burst.end_secs));
    }
    BenchSource::new(core).with_id(node_id.to_owned())
}

// =============================================================================
// Sink Factory
// =============================================================================

/// Build a sink and run its `validate()` before anything is spawned.
fn create_sink(node_id: &str, sink_def: &SinkDef) -> Result<Box<dyn Sink + Send>> {
    let sink: Box<dyn Sink + Send> = match sink_def {
        SinkDef::Stdout(_) => Box::new(StdoutSink::new(node_id)),
        SinkDef::File(cfg) => Box::new(FileSink::new(node_id, &cfg.path)),
        SinkDef::Mqtt(cfg) => {
            let client_id = cfg.client_id.clone().unwrap_or_else(|| format!("wafer-{node_id}"));
            Box::new(MqttSink::new(node_id, &cfg.broker, cfg.port, &cfg.topic, cfg.qos, client_id))
        }
        SinkDef::Http(cfg) => Box::new(HttpSink::new(node_id, &cfg.url)),
        SinkDef::BenchSink(cfg) => Box::new(bench_sink_from_toml(node_id, cfg)),
    };
    sink.validate().map_err(|e| invalid_io_node("sink", node_id, &e))?;
    Ok(sink)
}

fn invalid_io_node(role: &str, node_id: &str, error: &WaferError) -> WaferError {
    WaferError::Config(ConfigError::Message(format!("{role} '{node_id}' is invalid: {error}")))
}

fn bench_sink_from_toml(node_id: &str, cfg: &BenchSinkConfigToml) -> BenchSink {
    let core = BenchSinkConfig {
        warmup_secs: cfg.warmup_secs,
        track_sequences: cfg.track_sequences,
        track_hotswap: cfg.track_hotswap,
        output_dir: resolve_bench_sink_output_dir(
            cfg.output_dir.as_deref(),
            std::env::var_os("WAFER_BENCH_OUTPUT_DIR").as_deref(),
        ),
    };
    BenchSink::new(core).with_id(node_id.to_owned())
}

fn resolve_bench_sink_output_dir(
    configured: Option<&str>,
    environment: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    let configured = configured.map(Path::new);
    let Some(base) = environment.filter(|path| !path.is_empty()) else {
        return configured.map(Path::to_path_buf);
    };
    if let Some(relative) = configured.filter(|path| path.is_relative()) {
        return Some(Path::new(base).join(relative));
    }
    Some(PathBuf::from(base))
}

// =============================================================================
// Wasm Node Loading
// =============================================================================

/// Dispatch: build a Wasm or Native transform depending on `wasm.plugin`.
/// This is where the `plugin.kind = "native"` schema variant is honoured
/// (P0.4 AC2). Wasm construction still goes through the original
/// `load_transform_node` helper unchanged.
#[expect(
    clippy::too_many_arguments,
    reason = "node loader params are a flat list; a config struct would add indirection for a private function"
)]
#[expect(
    clippy::large_futures,
    reason = "WASM component loading holds Store/Component across awaits; called once per node at startup"
)]
async fn load_transform_node_dispatch(
    node_id: &str,
    wasm: &WasmNodeDef,
    default_fuel: Option<NonZeroU64>,
    default_memory: usize,
    engine: &Arc<WaferEngine>,
    registry: &WaferRegistry,
    config_path: Option<&Path>,
    plugin_hashes: &mut HashMap<Box<str>, String>,
    timings: &mut LaunchTimings,
) -> Result<crate::node::TransformNode> {
    if let Some(function) = wasm.plugin.native_function() {
        let native = build_native_transform(node_id, function)?;
        return Ok(crate::node::TransformNode::Native(native));
    }
    let wasm_node = load_transform_node(
        node_id,
        wasm,
        default_fuel,
        default_memory,
        engine,
        registry,
        config_path,
        plugin_hashes,
        timings,
    )
    .await?;
    Ok(crate::node::TransformNode::from(wasm_node))
}

/// Build a native transform from a function name declared in the TOML
/// (`plugin.kind = "native"`, `plugin.function = "…"`).
fn build_native_transform(node_id: &str, function: &str) -> Result<crate::node::NativeTransform> {
    use crate::node::NativeTransform;
    match function {
        "passthrough" => Ok(NativeTransform::passthrough(node_id)),
        "uppercase" => Ok(NativeTransform::uppercase(node_id)),
        "json-parse" | "json_parse" => Ok(NativeTransform::json_parse(node_id)),
        other => Err(WaferError::Config(ConfigError::Message(format!(
            "unknown native transform function '{other}' on node '{node_id}' \
             (valid: passthrough, uppercase, json-parse)"
        )))),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "node loader params are a flat list; a config struct would add indirection for a private function"
)]
#[expect(
    clippy::large_futures,
    reason = "WASM component loading holds Store/Component across awaits; called once per node at startup"
)]
async fn load_transform_node(
    node_id: &str,
    wasm: &WasmNodeDef,
    default_fuel: Option<NonZeroU64>,
    default_memory: usize,
    engine: &Arc<WaferEngine>,
    registry: &WaferRegistry,
    config_path: Option<&Path>,
    plugin_hashes: &mut HashMap<Box<str>, String>,
    timings: &mut LaunchTimings,
) -> Result<WasmTransformNode> {
    let phase_started = Instant::now();
    let (component, plugin_hash) =
        resolve_and_load_component(node_id, wasm, engine, registry, config_path).await?;
    timings.component_load_compile =
        timings.component_load_compile.saturating_add(phase_started.elapsed());
    plugin_hashes.insert(node_id.into(), plugin_hash);

    let phase_started = Instant::now();
    let capabilities = capabilities_from_config(&wasm.capabilities)?;
    let memory_limit = wasm.memory_limit.unwrap_or(default_memory);
    let fuel_limit = wasm.fuel.or(default_fuel);
    let state = WaferState::new_with_memory_limit(node_id, capabilities.clone(), memory_limit);
    let mut store = Store::new(engine.inner(), state);
    store.limiter(|s| s.limits_mut());
    // AC F5.AC2: None → unlimited; engine construction leaves consume_fuel /
    // epoch_interruption off in that case, so calling the setter would trap
    // or error. Fuel + epoch must be set before instantiation — start functions
    // consume fuel, and the epoch ticker is running from engine init.
    if let Some(n) = fuel_limit {
        store
            .set_fuel(n.get())
            .map_err(|e| WaferError::PluginInit { message: format!("failed to set fuel: {e}") })?;
    }
    if let Some(n) = engine.epoch_deadline() {
        store.epoch_deadline_trap();
        store.set_epoch_deadline(n.get());
    }

    let mut node = if capabilities.allow_inference {
        let pre = Arc::new(engine.pre_instantiate_inference(&component)?);
        let bindings =
            pre.instantiate_async(&mut store).await.map_err(|e| WaferError::PluginInit {
                message: format!("inference transform '{node_id}' instantiation failed: {e}"),
            })?;
        WasmTransformNode::new_inference(store, bindings, pre, fuel_limit)
    } else {
        let pre = Arc::new(engine.pre_instantiate_transform(&component)?);
        let bindings =
            pre.instantiate_async(&mut store).await.map_err(|e| WaferError::PluginInit {
                message: format!("transform '{node_id}' instantiation failed: {e}"),
            })?;
        WasmTransformNode::new(store, bindings, pre, fuel_limit)
    };

    let config_json = node_config_json(wasm)?;
    node.configure_runtime(
        capabilities,
        memory_limit,
        engine.epoch_deadline(),
        config_json.clone(),
    );
    node.set_plugin_version(wasm.plugin_version.clone().unwrap_or_default());
    node.validate_and_init(&config_json).await?;
    timings.instantiation = timings.instantiation.saturating_add(phase_started.elapsed());
    Ok(node)
}

/// Build a native filter from a function name declared in the TOML
/// (`plugin.kind = "native", plugin.function = "threshold"`).
///
/// `threshold` reads `field`, `min`, `max` from the node config so pipeline
/// authors can swap `plugin.kind` between `wasm` and `native` without
/// touching the config schema. Missing keys default to the WIT plugin's
/// documented range (`field="temperature", min=0, max=+∞`).
fn build_native_filter(
    node_id: &str,
    function: &str,
    wasm: &WasmNodeDef,
) -> Result<crate::node::NativeFilter> {
    use crate::node::NativeFilter;
    match function {
        "threshold" | "threshold-filter" | "range" => {
            let (field, min, max) = threshold_native_config(node_id, wasm)?;
            Ok(NativeFilter::range(node_id, field, min, max))
        }
        other => Err(WaferError::Config(ConfigError::Message(format!(
            "unknown native filter function '{other}' on node '{node_id}' \
             (valid: threshold, threshold-filter, range)"
        )))),
    }
}

/// Read `field/min/max` from the node's optional `config` table. Matches
/// the `plugins/threshold-filter` schema exactly so a `plugin.kind` swap
/// is a one-line config edit.
fn threshold_native_config(node_id: &str, wasm: &WasmNodeDef) -> Result<(String, f64, f64)> {
    let table = wasm.config.as_ref().and_then(|v| v.as_table());
    let field = table
        .and_then(|t| t.get("field"))
        .and_then(|v| v.as_str())
        .unwrap_or("temperature")
        .to_owned();
    let min = table.and_then(|t| t.get("min")).and_then(as_f64).unwrap_or(0.0);
    let max = table.and_then(|t| t.get("max")).and_then(as_f64).unwrap_or(f64::INFINITY);
    if min > max {
        return Err(WaferError::Config(ConfigError::Message(format!(
            "native filter '{node_id}': min ({min}) > max ({max})"
        ))));
    }
    Ok((field, min, max))
}

/// TOML `Value` numeric coercion accepting both `1` (integer) and `1.0`
/// (float) so config authors don't have to remember which one serde picks.
fn as_f64(v: &toml::Value) -> Option<f64> {
    v.as_float().or_else(|| {
        v.as_integer().map(|i| {
            #[expect(
                clippy::as_conversions,
                clippy::cast_precision_loss,
                reason = "i64→f64 precision loss is acceptable for config values; TOML integers are typically small"
            )]
            let f = i as f64;
            f
        })
    })
}

/// Dispatch: build a Wasm or Native filter depending on `wasm.plugin`.
/// Mirrors [`load_transform_node_dispatch`] so the native baseline can
/// implement `type = "filter"` (RQ1 apples-to-apples — A18).
#[expect(
    clippy::too_many_arguments,
    reason = "node loader params are a flat list; a config struct would add indirection for a private function"
)]
#[expect(
    clippy::large_futures,
    reason = "WASM component loading holds Store/Component across awaits; called once per node at startup"
)]
async fn load_filter_node_dispatch(
    node_id: &str,
    wasm: &WasmNodeDef,
    default_fuel: Option<NonZeroU64>,
    default_memory: usize,
    engine: &Arc<WaferEngine>,
    registry: &WaferRegistry,
    config_path: Option<&Path>,
    plugin_hashes: &mut HashMap<Box<str>, String>,
    timings: &mut LaunchTimings,
) -> Result<crate::node::FilterNode> {
    if let Some(function) = wasm.plugin.native_function() {
        let native = build_native_filter(node_id, function, wasm)?;
        return Ok(crate::node::FilterNode::Native(native));
    }
    let wasm_node = load_filter_node(
        node_id,
        wasm,
        default_fuel,
        default_memory,
        engine,
        registry,
        config_path,
        plugin_hashes,
        timings,
    )
    .await?;
    Ok(crate::node::FilterNode::from(wasm_node))
}

/// Test helper: build a native FilterNode from a `WasmNodeDef` as parsed
/// from TOML. Exposes the launcher's native-dispatch branch so integration
/// tests can prove the wiring without spinning up the async pipeline.
/// Errors when the plugin is not a native kind. Not part of the stable API.
#[doc(hidden)]
pub fn build_native_filter_from_def(
    node_id: &str,
    wasm: &WasmNodeDef,
) -> Result<crate::node::FilterNode> {
    let function = wasm.plugin.native_function().ok_or_else(|| {
        WaferError::Config(ConfigError::Message(format!(
            "build_native_filter_from_def: node '{node_id}' is not a native plugin"
        )))
    })?;
    let native = build_native_filter(node_id, function, wasm)?;
    Ok(crate::node::FilterNode::Native(native))
}

#[expect(
    clippy::too_many_arguments,
    reason = "node loader params are a flat list; a config struct would add indirection for a private function"
)]
#[expect(
    clippy::large_futures,
    reason = "WASM component loading holds Store/Component across awaits; called once per node at startup"
)]
async fn load_filter_node(
    node_id: &str,
    wasm: &WasmNodeDef,
    default_fuel: Option<NonZeroU64>,
    default_memory: usize,
    engine: &Arc<WaferEngine>,
    registry: &WaferRegistry,
    config_path: Option<&Path>,
    plugin_hashes: &mut HashMap<Box<str>, String>,
    timings: &mut LaunchTimings,
) -> Result<WasmFilterNode> {
    let phase_started = Instant::now();
    let (component, plugin_hash) =
        resolve_and_load_component(node_id, wasm, engine, registry, config_path).await?;
    timings.component_load_compile =
        timings.component_load_compile.saturating_add(phase_started.elapsed());
    plugin_hashes.insert(node_id.into(), plugin_hash);

    let phase_started = Instant::now();
    let pre = Arc::new(engine.pre_instantiate_filter(&component)?);
    let capabilities = capabilities_from_config(&wasm.capabilities)?;

    let state = WaferState::new_with_memory_limit(
        node_id,
        capabilities.clone(),
        wasm.memory_limit.unwrap_or(default_memory),
    );
    let mut store = Store::new(engine.inner(), state);
    store.limiter(|s| s.limits_mut());
    // AC F5.AC2: skip metering setters when unlimited; see transform loader.
    if let Some(n) = engine.fuel_limit() {
        store
            .set_fuel(n.get())
            .map_err(|e| WaferError::PluginInit { message: format!("failed to set fuel: {e}") })?;
    }
    if let Some(n) = engine.epoch_deadline() {
        store.epoch_deadline_trap();
        store.set_epoch_deadline(n.get());
    }

    let bindings = pre.instantiate_async(&mut store).await.map_err(|e| WaferError::PluginInit {
        message: format!("filter '{node_id}' instantiation failed: {e}"),
    })?;

    let config_json = node_config_json(wasm)?;
    let mut node = WasmFilterNode::new(store, bindings, pre, wasm.fuel.or(default_fuel));
    node.configure_runtime(
        capabilities,
        wasm.memory_limit.unwrap_or(default_memory),
        engine.epoch_deadline(),
        config_json.clone(),
    );
    node.set_plugin_version(wasm.plugin_version.clone().unwrap_or_default());
    node.validate_and_init(&config_json).await?;
    timings.instantiation = timings.instantiation.saturating_add(phase_started.elapsed());
    Ok(node)
}

#[expect(
    clippy::too_many_arguments,
    reason = "node loader params are a flat list; a config struct would add indirection for a private function"
)]
#[expect(
    clippy::large_futures,
    reason = "WASM component loading holds Store/Component across awaits; called once per node at startup"
)]
async fn load_router_node(
    node_id: &str,
    wasm: &WasmNodeDef,
    default_fuel: Option<NonZeroU64>,
    default_memory: usize,
    engine: &Arc<WaferEngine>,
    registry: &WaferRegistry,
    config_path: Option<&Path>,
    plugin_hashes: &mut HashMap<Box<str>, String>,
    timings: &mut LaunchTimings,
) -> Result<WasmRouterNode> {
    let phase_started = Instant::now();
    let (component, plugin_hash) =
        resolve_and_load_component(node_id, wasm, engine, registry, config_path).await?;
    timings.component_load_compile =
        timings.component_load_compile.saturating_add(phase_started.elapsed());
    plugin_hashes.insert(node_id.into(), plugin_hash);

    let phase_started = Instant::now();
    let pre = Arc::new(engine.pre_instantiate_router(&component)?);
    let capabilities = capabilities_from_config(&wasm.capabilities)?;

    let state = WaferState::new_with_memory_limit(
        node_id,
        capabilities.clone(),
        wasm.memory_limit.unwrap_or(default_memory),
    );
    let mut store = Store::new(engine.inner(), state);
    store.limiter(|s| s.limits_mut());
    // AC F5.AC2: skip metering setters when unlimited; see transform loader.
    if let Some(n) = engine.fuel_limit() {
        store
            .set_fuel(n.get())
            .map_err(|e| WaferError::PluginInit { message: format!("failed to set fuel: {e}") })?;
    }
    if let Some(n) = engine.epoch_deadline() {
        store.epoch_deadline_trap();
        store.set_epoch_deadline(n.get());
    }

    let bindings = pre.instantiate_async(&mut store).await.map_err(|e| WaferError::PluginInit {
        message: format!("router '{node_id}' instantiation failed: {e}"),
    })?;

    let config_json = node_config_json(wasm)?;
    let mut node = WasmRouterNode::new(store, bindings, pre, wasm.fuel.or(default_fuel));
    node.configure_runtime(
        capabilities,
        wasm.memory_limit.unwrap_or(default_memory),
        engine.epoch_deadline(),
        config_json.clone(),
    );
    node.set_plugin_version(wasm.plugin_version.clone().unwrap_or_default());
    node.validate_and_init(&config_json).await?;
    timings.instantiation = timings.instantiation.saturating_add(phase_started.elapsed());
    Ok(node)
}

/// Resolve plugin source, read bytes, compute SHA256, and load the Wasm
/// component. Returning the hash lets the launcher seed
/// `PipelineHandle::plugin_hashes` with the same value the P0.12 guard
/// checks on hot-swap — single source of truth (metadata.json AC2).
#[expect(
    clippy::large_futures,
    reason = "OCI resolution + WASM compilation hold large intermediates across awaits; called once per node"
)]
async fn resolve_and_load_component(
    node_id: &str,
    wasm: &WasmNodeDef,
    engine: &WaferEngine,
    registry: &WaferRegistry,
    config_path: Option<&Path>,
) -> Result<(Arc<wasmtime::component::Component>, String)> {
    let plugin_path = wasm.plugin.wasm_path().ok_or_else(|| {
        WaferError::Runtime(format!(
            "resolve_and_load_component called on non-Wasm plugin for node '{node_id}'"
        ))
    })?;
    let mut source = plugin_source(plugin_path);

    if let PluginSource::Local(ref path) = source
        && path.is_relative()
        && let Some(config_dir) = config_path.and_then(Path::parent)
    {
        source = PluginSource::Local(config_dir.join(path));
    }

    let resolved = registry.resolve(&source).await.map_err(WaferError::Registry)?;

    // `resolve` already read and hashed the bytes; reuse both so the
    // `component_load_compile` phase pays for one read and one SHA-256.
    let source_tag = match &resolved.source {
        PluginSource::Local(path) => {
            tracing::debug!(node = %node_id, path = %path.display(), "loading local plugin");
            path.display().to_string()
        }
        PluginSource::Oci(oci_ref) => {
            tracing::info!(
                node = %node_id,
                reference = %oci_ref,
                cache_path = %resolved.wasm_path.display(),
                "loading OCI plugin"
            );
            oci_ref.to_string()
        }
    };
    let ResolvedPlugin { content: bytes, content_hash: plugin_hash, .. } = resolved;
    // Through the cache, so a later hot-swap back to this binary reuses the
    // compiled component instead of paying a second cold compile.
    let (component, _) = engine.compile_cached(&bytes, &source_tag)?;
    Ok((component, plugin_hash))
}

fn node_config_json(wasm: &WasmNodeDef) -> Result<String> {
    let Some(config) = &wasm.config else {
        return Ok("{}".to_string());
    };

    serde_json::to_string(config).map_err(|e| {
        WaferError::Config(ConfigError::Message(format!(
            "failed to serialize node config for lifecycle init: {e}"
        )))
    })
}

pub(crate) fn capabilities_from_config(config: &ConfigCapabilities) -> Result<Capabilities> {
    Capabilities::try_from(config).map_err(|error| {
        WaferError::Config(ConfigError::Message(format!(
            "invalid outbound HTTP capability: {error}"
        )))
    })
}

fn plugin_source(plugin: &str) -> PluginSource {
    if let Some(oci_ref) = OciReference::parse(plugin) {
        return PluginSource::Oci(oci_ref);
    }

    PluginSource::Local(PathBuf::from(plugin))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        BenchSinkConfigToml, BenchSourceConfigToml, HttpHost, HttpScheme, OutboundHttpDestination,
    };
    use crate::node::Lifecycle;

    #[tokio::test]
    async fn invalid_bench_source_fails_launch_before_spawning() {
        let config: Config = toml::from_str(
            r#"
[nodes.source]
type = "source"
kind = "bench-source"
rate = 0.0
total_messages = 10

[nodes.sink]
type = "sink"
kind = "stdout"

[[edges]]
from = "source"
to = "sink"
"#,
        )
        .expect("config");

        let Err(err) = Box::pin(launch_pipeline(config, None)).await else {
            panic!("a zero-rate bench source must fail launch");
        };
        assert!(matches!(err, WaferError::Config(_)), "{err}");
        assert!(err.to_string().contains("source 'source' is invalid"), "{err}");
    }

    /// Every bench/MQTT/HTTP adapter block in the evaluation configs must pass
    /// the `validate()` that launch now runs, so no evaluation config starts
    /// failing at launch. (File adapters are skipped: their paths are
    /// resolved at run time relative to the harness working directory.)
    #[test]
    fn evaluation_config_adapters_pass_validation() {
        fn toml_files(dir: &Path, out: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).expect("read dir").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    toml_files(&path, out);
                } else if path.extension().is_some_and(|ext| ext == "toml") {
                    out.push(path);
                }
            }
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../eval/configs");
        let mut files = Vec::new();
        toml_files(&root, &mut files);
        let mut checked = 0;
        for file in files {
            let Ok(config) =
                toml::from_str::<Config>(&std::fs::read_to_string(&file).expect("read"))
            else {
                continue; // not a pipeline config
            };
            for (node_id, def) in &config.nodes {
                let result = match def {
                    NodeDef::Source(
                        source @ (SourceDef::BenchSource(_)
                        | SourceDef::Mqtt(_)
                        | SourceDef::Http(_)),
                    ) => create_source(node_id, source).map(drop),
                    NodeDef::Sink(
                        sink @ (SinkDef::BenchSink(_) | SinkDef::Mqtt(_) | SinkDef::Http(_)),
                    ) => create_sink(node_id, sink).map(drop),
                    _ => continue,
                };
                result.unwrap_or_else(|e| panic!("{}: {e}", file.display()));
                checked += 1;
            }
        }
        assert!(checked > 0, "no evaluation config adapters found under {}", root.display());
    }

    #[test]
    fn bench_sink_relative_output_directory_is_scoped_to_run() {
        let run_dir = Path::new("/tmp/e-iso-7/control");
        assert_eq!(
            resolve_bench_sink_output_dir(Some("branch-a"), Some(run_dir.as_os_str())),
            Some(run_dir.join("branch-a"))
        );
        assert_eq!(
            resolve_bench_sink_output_dir(None, Some(run_dir.as_os_str())),
            Some(run_dir.to_path_buf())
        );
        assert_eq!(
            resolve_bench_sink_output_dir(Some("/configured/output"), Some(run_dir.as_os_str())),
            Some(run_dir.to_path_buf())
        );
    }

    #[tokio::test]
    #[expect(clippy::large_futures, reason = "test launches the real Wasm-capable pipeline")]
    async fn native_transform_and_filter_are_not_replacement_eligible() {
        let config: Config = toml::from_str(
            r#"
[pipeline]
name = "native-replacement-eligibility"

[nodes.source]
type = "source"
kind = "stdin"

[nodes.transform]
type = "transform"
plugin = { kind = "native", function = "passthrough" }

[nodes.filter]
type = "filter"
plugin = { kind = "native", function = "threshold" }

[nodes.sink]
type = "sink"
kind = "stdout"

[[edges]]
from = "source"
to = "transform"

[[edges]]
from = "transform"
to = "filter"

[[edges]]
from = "filter"
to = "sink"
"#,
        )
        .expect("native config");
        let mut orchestrator = launch_pipeline(config, None).await.expect("launch native pipeline");
        let handle = orchestrator.handle();

        assert!(handle.swappable_nodes().is_empty());
        let _ = handle.try_begin_swap("transform").expect_err("native transform rejected");
        let _ = handle.try_begin_swap("filter").expect_err("native filter rejected");
        let _ = handle.try_begin_swap("source").expect_err("source rejected");
        let _ = handle.try_begin_swap("sink").expect_err("sink rejected");

        orchestrator.shutdown().await.expect("shutdown native pipeline");
    }

    #[tokio::test]
    #[expect(clippy::large_futures, reason = "test launches the real Wasm pipeline")]
    async fn loaded_wasm_transform_filter_and_router_are_replacement_eligible() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let transform =
            root.join("plugins/pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm");
        let filter = root.join(
            "plugins/threshold-filter/target/wasm32-wasip2/release/wafer_threshold_filter.wasm",
        );
        let router = root
            .join("plugins/content-router/target/wasm32-wasip2/release/wafer_content_router.wasm");
        if !transform.exists() || !filter.exists() || !router.exists() {
            return;
        }
        let config: Config = toml::from_str(&format!(
            r#"
[pipeline]
name = "wasm-replacement-eligibility"

[nodes.source]
type = "source"
kind = "stdin"

[nodes.transform]
type = "transform"
plugin = {transform:?}

[nodes.filter]
type = "filter"
plugin = {filter:?}

[nodes.filter.config]
field = "temperature"
min = 0.0
max = 100.0

[nodes.router]
type = "router"
plugin = {router:?}

[nodes.router.config]
route_field = "type"
default_port = "default"
routes = {{ telemetry = "default" }}

[nodes.sink]
type = "sink"
kind = "stdout"

[[edges]]
from = "source"
to = "transform"

[[edges]]
from = "transform"
to = "filter"

[[edges]]
from = "filter"
to = "router"

[[edges]]
from = "router"
to = "sink"
port = "default"
"#,
            transform = transform.display(),
            filter = filter.display(),
            router = router.display(),
        ))
        .expect("wasm config");
        let mut orchestrator = launch_pipeline(config, None).await.expect("launch wasm pipeline");
        let handle = orchestrator.handle();
        let eligible = handle.swappable_nodes();

        assert!(eligible.contains(&"transform"));
        assert!(eligible.contains(&"filter"));
        assert!(eligible.contains(&"router"));
        assert!(!eligible.contains(&"source"));
        assert!(!eligible.contains(&"sink"));

        orchestrator.shutdown().await.expect("shutdown wasm pipeline");
    }

    #[test]
    fn inference_grant_reaches_the_engine_capability() {
        let config = ConfigCapabilities {
            inherit_stdio: true,
            allow_inference: true,
            outbound_http: vec![OutboundHttpDestination {
                scheme: HttpScheme::Https,
                host: "API.EXAMPLE.COM".to_string(),
                port: None,
            }],
            ..Default::default()
        };

        let capabilities = capabilities_from_config(&config).expect("valid capabilities");
        assert!(capabilities.inherit_stdio);
        assert!(!capabilities.inherit_env);
        assert!(capabilities.allow_inference);
        assert_eq!(capabilities.outbound_http_destinations().len(), 1);
        assert_eq!(
            capabilities
                .outbound_http_destinations()
                .first()
                .map(|destination| (&destination.host, destination.port)),
            Some((&HttpHost::Dns("api.example.com".into()), 443))
        );
    }

    #[test]
    fn launch_bench_source_and_sink() {
        // Verify the TOML → concrete node conversion for BenchSource/BenchSink.
        // This is a pure factory test; full pipeline wiring is exercised by
        // the smoke test in tests/end_to_end.rs.
        let src_cfg = BenchSourceConfigToml {
            rate: 1000.0,
            total_messages: 100,
            warmup_messages: 10,
            payload_size: 200,
            burst: None,
        };
        let src = bench_source_from_toml("my-src", &src_cfg);
        assert_eq!(src.id(), "my-src");
        assert_eq!(src.node_type(), "bench-source");

        let snk_cfg = BenchSinkConfigToml {
            warmup_secs: 5,
            track_sequences: true,
            track_hotswap: false,
            output_dir: None,
        };
        let snk = bench_sink_from_toml("my-snk", &snk_cfg);
        assert_eq!(snk.id(), "my-snk");
        assert_eq!(snk.node_type(), "bench-sink");
    }
}
