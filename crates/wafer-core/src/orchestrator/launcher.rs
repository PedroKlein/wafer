//! Pipeline launcher — single entry point from Config to running orchestrator.
//!
//! Absorbs all startup orchestration: engine creation, plugin resolution,
//! Wasm compilation, source/sink construction, and topology wiring.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use wasmtime::Store;

use crate::config::{
    BenchSinkConfigToml, BenchSourceConfigToml, Capabilities as ConfigCapabilities, Config,
    NodeDef, SinkDef, SourceDef, WasmNodeDef,
};
use crate::engine::{Capabilities, WaferEngine, WaferState};
use crate::error::{ConfigError, Result, WaferError};
use crate::node::{
    BenchSink, BenchSinkConfig, BenchSource, BenchSourceConfig, FileSink, FileSource, HttpSink,
    HttpSource, MqttSink, MqttSource, Sink, Source, StdinSource, StdoutSink,
};
use crate::node::wasm::{WasmFilterNode, WasmRouterNode, WasmTransformNode};
use crate::orchestrator::builder::{build_pipeline_with_io, NodeBundleKind};
use crate::orchestrator::pipeline::PipelineOrchestrator;
use crate::registry::{OciReference, PluginSource, RegistryConfig, WaferRegistry};

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
pub async fn launch_pipeline(
    config: Config,
    config_path: Option<&Path>,
) -> Result<PipelineOrchestrator> {
    let engine = WaferEngine::from_engine_config(&config.engine)?;
    engine.ensure_epoch_ticker();
    let engine = Arc::new(engine);

    let registry_config = config.registry.as_ref().map_or_else(
        RegistryConfig::default,
        |cfg| RegistryConfig {
            cache_dir: cfg.cache_dir.as_ref().map(PathBuf::from),
            ..Default::default()
        },
    );
    let registry = WaferRegistry::new(registry_config).map_err(WaferError::Registry)?;

    let mut sources: HashMap<String, Box<dyn Source + Send>> = HashMap::new();
    let mut sinks: HashMap<String, Box<dyn Sink + Send>> = HashMap::new();

    for (node_id, node_def) in &config.nodes {
        match node_def {
            NodeDef::Source(source_def) => {
                sources.insert(node_id.clone(), create_source(node_id, source_def));
            }
            NodeDef::Sink(sink_def) => {
                sinks.insert(node_id.clone(), create_sink(node_id, sink_def));
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
                *node = Some(load_transform_node_dispatch(
                    &bundle.node_id,
                    wasm,
                    config.engine.fuel.transform,
                    config.engine.memory.transform,
                    &engine,
                    &registry,
                    config_path,
                    &mut plugin_hashes,
                ).await?);
            }
            (NodeBundleKind::Filter { node, .. }, NodeDef::Filter(wasm)) => {
                *node = Some(load_filter_node_dispatch(
                    &bundle.node_id,
                    wasm,
                    config.engine.fuel.filter,
                    config.engine.memory.filter,
                    &engine,
                    &registry,
                    config_path,
                    &mut plugin_hashes,
                ).await?);
            }
            (NodeBundleKind::Router { node, .. }, NodeDef::Router(wasm)) => {
                *node = Some(load_router_node(
                    &bundle.node_id,
                    wasm,
                    config.engine.fuel.router,
                    config.engine.memory.router,
                    &engine,
                    &registry,
                    config_path,
                    &mut plugin_hashes,
                ).await?);
            }
            _ => {}
        }
    }

    let orchestrator = PipelineOrchestrator::from_build_output(build_output, config, engine);
    let handle = orchestrator.handle();
    for (node_id, hash) in plugin_hashes {
        handle.record_plugin_hash(&node_id, hash);
    }
    Ok(orchestrator)
}

// =============================================================================
// Source Factory
// =============================================================================

fn create_source(node_id: &str, source_def: &SourceDef) -> Box<dyn Source + Send> {
    match source_def {
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
    }
}

fn bench_source_from_toml(node_id: &str, cfg: &BenchSourceConfigToml) -> BenchSource {
    let core = BenchSourceConfig::new(cfg.rate, cfg.total_messages)
        .with_warmup(cfg.warmup_messages)
        .with_payload_size(cfg.payload_size);
    BenchSource::new(core).with_id(node_id.to_owned())
}

// =============================================================================
// Sink Factory
// =============================================================================

fn create_sink(node_id: &str, sink_def: &SinkDef) -> Box<dyn Sink + Send> {
    match sink_def {
        SinkDef::Stdout(_) => Box::new(StdoutSink::new(node_id)),
        SinkDef::File(cfg) => Box::new(FileSink::new(node_id, &cfg.path)),
        SinkDef::Mqtt(cfg) => {
            let client_id = cfg.client_id.clone().unwrap_or_else(|| format!("wafer-{node_id}"));
            Box::new(MqttSink::new(
                node_id,
                &cfg.broker,
                cfg.port,
                &cfg.topic,
                cfg.qos,
                client_id,
            ))
        }
        SinkDef::Http(cfg) => Box::new(HttpSink::new(node_id, &cfg.url)),
        SinkDef::BenchSink(cfg) => Box::new(bench_sink_from_toml(node_id, cfg)),
    }
}

fn bench_sink_from_toml(node_id: &str, cfg: &BenchSinkConfigToml) -> BenchSink {
    let mut core = BenchSinkConfig {
        warmup_secs: cfg.warmup_secs,
        track_sequences: cfg.track_sequences,
        track_hotswap: cfg.track_hotswap,
        output_dir: cfg.output_dir.as_ref().map(std::path::PathBuf::from),
    };
    // If the environment (eval scripts) supplied a WAFER_BENCH_OUTPUT_DIR,
    // let it override the TOML value — same convention used by other
    // eval-facing knobs. Keeps configs portable across hosts.
    if let Ok(env_dir) = std::env::var("WAFER_BENCH_OUTPUT_DIR") {
        if !env_dir.is_empty() {
            core.output_dir = Some(std::path::PathBuf::from(env_dir));
        }
    }
    BenchSink::new(core).with_id(node_id.to_owned())
}

// =============================================================================
// Wasm Node Loading
// =============================================================================

/// Dispatch: build a Wasm or Native transform depending on `wasm.plugin`.
/// This is where the `plugin.kind = "native"` schema variant is honoured
/// (P0.4 AC2). Wasm construction still goes through the original
/// `load_transform_node` helper unchanged.
async fn load_transform_node_dispatch(
    node_id: &str,
    wasm: &WasmNodeDef,
    default_fuel: u64,
    default_memory: usize,
    engine: &Arc<WaferEngine>,
    registry: &WaferRegistry,
    config_path: Option<&Path>,
    plugin_hashes: &mut HashMap<Box<str>, String>,
) -> Result<crate::node::TransformNode> {
    if let Some(function) = wasm.plugin.native_function() {
        let native = build_native_transform(node_id, function)?;
        return Ok(crate::node::TransformNode::Native(native));
    }
    let wasm_node = load_transform_node(
        node_id, wasm, default_fuel, default_memory, engine, registry, config_path,
        plugin_hashes,
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

async fn load_transform_node(
    node_id: &str,
    wasm: &WasmNodeDef,
    default_fuel: u64,
    default_memory: usize,
    engine: &Arc<WaferEngine>,
    registry: &WaferRegistry,
    config_path: Option<&Path>,
    plugin_hashes: &mut HashMap<Box<str>, String>,
) -> Result<WasmTransformNode> {
    let (component, plugin_hash) =
        resolve_and_load_component(node_id, wasm, engine, registry, config_path).await?;
    plugin_hashes.insert(node_id.into(), plugin_hash);
    let pre = Arc::new(engine.pre_instantiate_transform(&component)?);

    let state = WaferState::new_with_memory_limit(
        node_id,
        capabilities_from_config(&wasm.capabilities),
        wasm.memory_limit.unwrap_or(default_memory),
    );
    let mut store = Store::new(engine.inner(), state);
    store.limiter(|s| s.limits_mut());
    store.epoch_deadline_trap();
    store.set_epoch_deadline(engine.epoch_deadline());

    let bindings = pre.instantiate(&mut store).map_err(|e| WaferError::PluginInit {
        message: format!("transform '{node_id}' instantiation failed: {e}"),
    })?;

    let config_json = node_config_json(wasm)?;
    let mut node = WasmTransformNode::new(store, bindings, pre, wasm.fuel.unwrap_or(default_fuel));
    node.configure_runtime(
        capabilities_from_config(&wasm.capabilities),
        wasm.memory_limit.unwrap_or(default_memory),
        engine.epoch_deadline(),
        config_json.clone(),
    );
    node.set_plugin_version(wasm.plugin_version.clone().unwrap_or_default());
    node.validate_and_init(&config_json)?;
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
             (valid: threshold)"
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
    let min = table
        .and_then(|t| t.get("min"))
        .and_then(as_f64)
        .unwrap_or(0.0);
    let max = table
        .and_then(|t| t.get("max"))
        .and_then(as_f64)
        .unwrap_or(f64::INFINITY);
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
    v.as_float().or_else(|| v.as_integer().map(|i| i as f64))
}

/// Dispatch: build a Wasm or Native filter depending on `wasm.plugin`.
/// Mirrors [`load_transform_node_dispatch`] so the native baseline can
/// implement `type = "filter"` (RQ1 apples-to-apples — A18).
async fn load_filter_node_dispatch(
    node_id: &str,
    wasm: &WasmNodeDef,
    default_fuel: u64,
    default_memory: usize,
    engine: &Arc<WaferEngine>,
    registry: &WaferRegistry,
    config_path: Option<&Path>,
    plugin_hashes: &mut HashMap<Box<str>, String>,
) -> Result<crate::node::FilterNode> {
    if let Some(function) = wasm.plugin.native_function() {
        let native = build_native_filter(node_id, function, wasm)?;
        return Ok(crate::node::FilterNode::Native(native));
    }
    let wasm_node = load_filter_node(
        node_id, wasm, default_fuel, default_memory, engine, registry, config_path,
        plugin_hashes,
    )
    .await?;
    Ok(crate::node::FilterNode::from(wasm_node))
}

async fn load_filter_node(
    node_id: &str,
    wasm: &WasmNodeDef,
    default_fuel: u64,
    default_memory: usize,
    engine: &Arc<WaferEngine>,
    registry: &WaferRegistry,
    config_path: Option<&Path>,
    plugin_hashes: &mut HashMap<Box<str>, String>,
) -> Result<WasmFilterNode> {
    let (component, plugin_hash) =
        resolve_and_load_component(node_id, wasm, engine, registry, config_path).await?;
    plugin_hashes.insert(node_id.into(), plugin_hash);
    let pre = Arc::new(engine.pre_instantiate_filter(&component)?);

    let state = WaferState::new_with_memory_limit(
        node_id,
        capabilities_from_config(&wasm.capabilities),
        wasm.memory_limit.unwrap_or(default_memory),
    );
    let mut store = Store::new(engine.inner(), state);
    store.limiter(|s| s.limits_mut());
    store.epoch_deadline_trap();
    store.set_epoch_deadline(engine.epoch_deadline());

    let bindings = pre.instantiate(&mut store).map_err(|e| WaferError::PluginInit {
        message: format!("filter '{node_id}' instantiation failed: {e}"),
    })?;

    let config_json = node_config_json(wasm)?;
    let mut node = WasmFilterNode::new(store, bindings, pre, wasm.fuel.unwrap_or(default_fuel));
    node.configure_runtime(
        capabilities_from_config(&wasm.capabilities),
        wasm.memory_limit.unwrap_or(default_memory),
        engine.epoch_deadline(),
        config_json.clone(),
    );
    node.set_plugin_version(wasm.plugin_version.clone().unwrap_or_default());
    node.validate_and_init(&config_json)?;
    Ok(node)
}

async fn load_router_node(
    node_id: &str,
    wasm: &WasmNodeDef,
    default_fuel: u64,
    default_memory: usize,
    engine: &Arc<WaferEngine>,
    registry: &WaferRegistry,
    config_path: Option<&Path>,
    plugin_hashes: &mut HashMap<Box<str>, String>,
) -> Result<WasmRouterNode> {
    let (component, plugin_hash) =
        resolve_and_load_component(node_id, wasm, engine, registry, config_path).await?;
    plugin_hashes.insert(node_id.into(), plugin_hash);
    let pre = Arc::new(engine.pre_instantiate_router(&component)?);

    let state = WaferState::new_with_memory_limit(
        node_id,
        capabilities_from_config(&wasm.capabilities),
        wasm.memory_limit.unwrap_or(default_memory),
    );
    let mut store = Store::new(engine.inner(), state);
    store.limiter(|s| s.limits_mut());
    store.epoch_deadline_trap();
    store.set_epoch_deadline(engine.epoch_deadline());

    let bindings = pre.instantiate(&mut store).map_err(|e| WaferError::PluginInit {
        message: format!("router '{node_id}' instantiation failed: {e}"),
    })?;

    let config_json = node_config_json(wasm)?;
    let mut node = WasmRouterNode::new(store, bindings, pre, wasm.fuel.unwrap_or(default_fuel));
    node.configure_runtime(
        capabilities_from_config(&wasm.capabilities),
        wasm.memory_limit.unwrap_or(default_memory),
        engine.epoch_deadline(),
        config_json.clone(),
    );
    node.set_plugin_version(wasm.plugin_version.clone().unwrap_or_default());
    node.validate_and_init(&config_json)?;
    Ok(node)
}

/// Resolve plugin source, read bytes, compute SHA256, and load the Wasm
/// component. Returning the hash lets the launcher seed
/// `PipelineHandle::plugin_hashes` with the same value the P0.12 guard
/// checks on hot-swap — single source of truth (metadata.json AC2).
async fn resolve_and_load_component(
    node_id: &str,
    wasm: &WasmNodeDef,
    engine: &WaferEngine,
    registry: &WaferRegistry,
    config_path: Option<&Path>,
) -> Result<(wasmtime::component::Component, String)> {
    let plugin_path = wasm.plugin.wasm_path().ok_or_else(|| {
        WaferError::Runtime(format!(
            "resolve_and_load_component called on non-Wasm plugin for node '{node_id}'"
        ))
    })?;
    let mut source = plugin_source(plugin_path)?;

    if let PluginSource::Local(ref path) = source
        && path.is_relative()
        && let Some(config_dir) = config_path.and_then(Path::parent)
    {
        source = PluginSource::Local(config_dir.join(path));
    }

    let resolved = registry.resolve(&source).await.map_err(WaferError::Registry)?;

    // Always read bytes so we can hash once and reuse for both metering the
    // guest and building metadata.json provenance. Doubling I/O for local
    // plugins is negligible (< 1 MB) and keeps a single load path.
    let (bytes, source_tag) = match &resolved.source {
        PluginSource::Local(path) => {
            tracing::debug!(node = %node_id, path = %path.display(), "loading local plugin");
            let bytes = std::fs::read(path).map_err(|e| {
                WaferError::Config(ConfigError::Message(format!(
                    "failed to read local plugin for '{node_id}' at {}: {e}",
                    path.display()
                )))
            })?;
            (bytes, path.display().to_string())
        }
        PluginSource::Oci(oci_ref) => {
            tracing::info!(
                node = %node_id,
                reference = %oci_ref,
                cache_path = %resolved.wasm_path.display(),
                "loading OCI plugin"
            );
            let bytes = std::fs::read(&resolved.wasm_path).map_err(|e| {
                WaferError::Config(ConfigError::Message(format!(
                    "failed to read cached plugin for '{node_id}': {e}"
                )))
            })?;
            (bytes, oci_ref.to_string())
        }
    };

    let plugin_hash = {
        use sha2::Digest;
        hex::encode(sha2::Sha256::digest(&bytes))
    };
    let component = engine.load_component_from_bytes(&bytes, &source_tag)?;
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

pub(crate) const fn capabilities_from_config(config: &ConfigCapabilities) -> Capabilities {
    Capabilities {
        inherit_stdio: config.inherit_stdio,
        inherit_env: config.inherit_env,
        allow_inference: config.allow_inference,
    }
}

fn plugin_source(plugin: &str) -> Result<PluginSource> {
    if let Some(oci_ref) = OciReference::parse(plugin) {
        return Ok(PluginSource::Oci(oci_ref));
    }

    Ok(PluginSource::Local(PathBuf::from(plugin)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BenchSinkConfigToml, BenchSourceConfigToml};
    use crate::node::Lifecycle;

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
