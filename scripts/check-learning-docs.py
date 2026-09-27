#!/usr/bin/env python3

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parents[1]
LEARN_DIR = ROOT / "docs/learn"
PAGES = (
    "README.md",
    "workspace-map.md",
    "reading-paths.md",
    "rust-in-context.md",
    "config-to-running-pipeline.md",
    "message-through-wasm.md",
    "shutdown-and-failure.md",
    "plugin-boundary.md",
    "stateless-hot-swap.md",
    "evaluation-harness.md",
)
DIATAXIS_TYPES = {"Tutorial", "How-to guide", "Reference", "Explanation"}
EXPECTED_DIATAXIS_TYPES = {
    "README.md": "Explanation",
    "workspace-map.md": "Explanation",
    "reading-paths.md": "How-to guide",
    "rust-in-context.md": "Explanation",
    "config-to-running-pipeline.md": "Tutorial",
    "message-through-wasm.md": "Tutorial",
    "shutdown-and-failure.md": "Explanation",
    "plugin-boundary.md": "Tutorial",
    "stateless-hot-swap.md": "Tutorial",
    "evaluation-harness.md": "Tutorial",
}
EXPECTED_EVIDENCE = {
    "README.md": {
        ("Source", "Cargo.toml", ("[workspace]", "members = [")),
        (
            "Source",
            "crates/wafer-runtime/src/main.rs",
            ("use wafer_config::{load_config, validate}",),
        ),
        (
            "Source",
            "crates/wafer-core/src/orchestrator/launcher.rs",
            ("fn create_source", "fn create_sink"),
        ),
        (
            "Test",
            "crates/wafer-config/tests/eval_configs_load.rs",
            ("fn every_eval_config_loads_and_validates()",),
        ),
    },
    "workspace-map.md": {
        ("Source", "Cargo.toml", ("[workspace]", "members = [")),
        (
            "Source",
            "crates/wafer-config/src/lib.rs",
            (
                "pub use loader::load_config",
                "pub use validation::{UNSUPPORTED_ALLOW_INFERENCE_MESSAGE, validate}",
            ),
        ),
        ("Source", "crates/wafer-types/src/lib.rs", ("pub mod config", "pub use control::*")),
        ("Source", "crates/wafer-core/src/lib.rs", ("pub mod orchestrator", "pub mod runner")),
        (
            "Source",
            "crates/wafer-runtime/src/main.rs",
            ("use wafer_config::{load_config, validate}", "async fn main()"),
        ),
        ("Source", "crates/wafer-plugin/src/lib.rs", ("macro_rules! output_from",)),
        (
            "Source",
            "crates/wafer-loadgen/src/lib.rs",
            ("pub mod publish", "pub use publish::{PublishArgs, run_publisher}"),
        ),
        ("Source", "crates/waferctl/src/main.rs", ("enum Commands",)),
        (
            "Test",
            "crates/wafer-config/tests/eval_configs_load.rs",
            ("fn every_eval_config_loads_and_validates()",),
        ),
    },
    "reading-paths.md": {
        (
            "Source",
            "crates/wafer-runtime/src/main.rs",
            ("async fn main()", "launch_pipeline_timed"),
        ),
        (
            "Source",
            "crates/wafer-core/src/orchestrator/builder.rs",
            ("pub fn build_pipeline_with_io", "fn wire_queues"),
        ),
        (
            "Test",
            "crates/wafer-core/src/orchestrator/pipeline.rs",
            (
                "async fn test_spawn_creates_tasks()",
                "async fn test_source_sink_real_loops_process_messages()",
            ),
        ),
    },
    "rust-in-context.md": {
        ("Source", "Cargo.toml", ("[workspace]", "members = [")),
        (
            "Source",
            "crates/wafer-config/src/lib.rs",
            (
                "pub use loader::load_config",
                "pub use validation::{UNSUPPORTED_ALLOW_INFERENCE_MESSAGE, validate}",
            ),
        ),
        (
            "Source",
            "crates/wafer-config/src/loader.rs",
            ("pub fn load_config", "Result<Config, ConfigError>"),
        ),
        (
            "Source",
            "crates/wafer-types/src/config/mod.rs",
            ("pub enum NodeDef", "pub const fn category"),
        ),
        (
            "Source",
            "crates/wafer-core/src/node/traits.rs",
            ("pub trait Lifecycle", "pub trait Transform", "Box<dyn Future"),
        ),
        (
            "Source",
            "crates/wafer-core/src/queue/envelope.rs",
            ("pub struct RuntimeEnvelope", "Arc<EnvelopeHeader>", "pub payload: Bytes"),
        ),
        (
            "Source",
            "crates/wafer-core/src/node/state.rs",
            ("pub struct ProcessingGuard", "impl Drop for ProcessingGuard"),
        ),
        (
            "Source",
            "crates/wafer-core/src/orchestrator/builder.rs",
            ("fn collect_downstream_senders", "overflow: e.overflow"),
        ),
        (
            "Source",
            "crates/wafer-core/src/runner/mod.rs",
            ("async fn send_one", "sender.sender.reserve().await"),
        ),
        (
            "Test",
            "crates/wafer-core/src/queue/envelope.rs",
            ("fn test_clone_shares_header_via_arc()", "fn test_clone_shares_payload_bytes()"),
        ),
        (
            "Test",
            "crates/wafer-core/src/runner/source.rs",
            ("async fn test_source_loop_messages_flow()",),
        ),
        (
            "Test",
            "crates/wafer-config/src/validation.rs",
            ("fn test_accumulated_errors()",),
        ),
        (
            "Test",
            "crates/wafer-core/src/node/state.rs",
            ("fn test_processing_guard_clears_on_panic()",),
        ),
    },
    "config-to-running-pipeline.md": {
        (
            "Source",
            "crates/wafer-runtime/src/main.rs",
            ("let config = load_config", "validate(&config)", "launch_pipeline_timed"),
        ),
        (
            "Source",
            "crates/wafer-config/src/loader.rs",
            ("pub fn load_config", "toml::from_str"),
        ),
        (
            "Source",
            "crates/wafer-config/src/validation.rs",
            ("pub fn validate", "check_no_cycles"),
        ),
        (
            "Source",
            "crates/wafer-core/src/dag/graph.rs",
            ("pub struct DagGraph", "pub fn from_config", "pub fn topo_order"),
        ),
        (
            "Source",
            "crates/wafer-core/src/orchestrator/builder.rs",
            ("use crate::dag::graph::DagGraph", "fn wire_queues", "mpsc::channel(capacity)"),
        ),
        (
            "Source",
            "crates/wafer-core/Cargo.toml",
            ("[dev-dependencies]", "wafer-config = { path = \"../wafer-config\" }"),
        ),
        (
            "Source",
            "crates/wafer-core/src/orchestrator/launcher.rs",
            ("pub async fn launch_pipeline_timed", "build_pipeline_with_io"),
        ),
        (
            "Source",
            "crates/wafer-core/src/orchestrator/pipeline.rs",
            ("pub fn from_build_output", "fn spawn_bundles"),
        ),
        (
            "Test",
            "crates/wafer-config/tests/eval_configs_load.rs",
            ("fn every_eval_config_loads_and_validates()",),
        ),
        (
            "Test",
            "crates/wafer-core/src/orchestrator/pipeline.rs",
            ("async fn test_spawn_creates_tasks()",),
        ),
    },
    "message-through-wasm.md": {
        (
            "Source",
            "crates/wafer-core/src/queue/envelope.rs",
            ("pub struct RuntimeEnvelope", "Arc<EnvelopeHeader>", "pub payload: Bytes"),
        ),
        (
            "Source",
            "crates/wafer-core/src/runner/source.rs",
            ("pub async fn run_source_loop", "envelope.ensure_trace_id()"),
        ),
        (
            "Source",
            "crates/wafer-core/src/runner/transform.rs",
            (
                "pub async fn run_transform_loop_with_config",
                "let result = transform.process(envelope).await",
            ),
        ),
        (
            "Source",
            "crates/wafer-core/src/runner/mod.rs",
            ("pub async fn send_downstream", "async fn send_one", "sender.sender.reserve().await"),
        ),
        (
            "Source",
            "crates/wafer-core/src/node/wasm.rs",
            ("fn build_wit_message", "pub async fn process", "delete_buffer"),
        ),
        (
            "Source",
            "wit/pipeline-types.wit",
            ("record message", "payload: borrow<buffer>", "record output-message"),
        ),
        (
            "Source",
            "wit/pipeline-node.wit",
            ("interface transform", "process: func(input: message)"),
        ),
        (
            "Test",
            "crates/wafer-core/src/queue/envelope.rs",
            ("fn test_clone_shares_header_via_arc()", "fn test_clone_shares_payload_bytes()"),
        ),
        (
            "Test",
            "crates/wafer-core/src/runner/source.rs",
            ("async fn test_source_loop_messages_flow()",),
        ),
        (
            "Test",
            "crates/wafer-core/src/testing/harness.rs",
            ("fn pass_through_propagates_metadata()", "fn pass_through_survives_epoch_deadline_wraparound()"),
        ),
    },
    "shutdown-and-failure.md": {
        (
            "Source",
            "crates/wafer-core/src/orchestrator/pipeline.rs",
            ("pub async fn shutdown", "pub async fn run_until_complete", "self.tasks.shutdown().await"),
        ),
        (
            "Source",
            "crates/wafer-core/src/runner/source.rs",
            ("pub async fn run_source_loop", "source.close().await"),
        ),
        (
            "Source",
            "crates/wafer-core/src/runner/transform.rs",
            (
                "pub async fn run_transform_loop_with_config",
                "let result = transform.process(envelope).await",
                "policy.flush_to_dlq(\"shutdown\")",
            ),
        ),
        (
            "Source",
            "crates/wafer-core/src/runner/sink.rs",
            ("pub async fn run_sink_loop", "receiver.try_recv()", "sink.flush().await", "sink.close().await"),
        ),
        (
            "Source",
            "crates/wafer-core/src/runner/error_policy.rs",
            ("pub(crate) fn handle", "pub fn flush_to_dlq", "DlqReason::Shutdown"),
        ),
        (
            "Test",
            "crates/wafer-core/src/orchestrator/pipeline.rs",
            ("async fn test_shutdown_completes_all_tasks()", "async fn test_cancel_triggers_shutdown()"),
        ),
        (
            "Test",
            "crates/wafer-core/src/runner/sink.rs",
            ("async fn test_sink_loop_cancel_drains_and_flushes()",),
        ),
        (
            "Test",
            "crates/wafer-core/src/runner/error_policy.rs",
            ("fn test_flush_to_dlq_drains_all_entries()",),
        ),
        (
            "Test",
            "crates/wafer-runtime/tests/runtime_control_plane.rs",
            ("async fn api_health_nodes_and_sigterm_shutdown()",),
        ),
    },
    "plugin-boundary.md": {
        ("Source", "wit/pipeline-types.wit", ("resource buffer", "record message", "record output-message")),
        ("Source", "wit/pipeline-node.wit", ("interface lifecycle", "interface transform", "interface filter")),
        ("Source", "wit/worlds.wit", ("world transform-node", "world filter-node", "world router-node", "world inference-node")),
        ("Source", "plugins/pass-through/Cargo.toml", ("crate-type = [\"cdylib\"]", "wit-bindgen = \"0.53\"")),
        ("Source", "crates/wafer-core/src/engine/bindings.rs", ("pub mod transform_node", "pub mod filter_node", "pub(crate) mod router_node", "pub(crate) mod inference_node")),
        ("Source", "crates/wafer-core/src/engine/loader.rs", ("fn build_linker", "add_to_linker_async", "add_only_http_to_linker_async", "pub fn pre_instantiate_transform", "pub(crate) fn pre_instantiate_inference", "pub fn pre_instantiate_filter", "pub fn pre_instantiate_router")),
        ("Source", "crates/wafer-core/src/orchestrator/launcher.rs", ("fn create_source", "fn create_sink", "async fn resolve_and_load_component")),
        ("Source", "plugins/pass-through/src/lib.rs", ("wit_bindgen::generate!", "impl exports::wafer::pipeline::transform::Guest", "export!(PassThrough)")),
        ("Source", "plugins/mnist-inference/src/lib.rs", ("world: \"inference-node\"", "graph::load", "GraphExecutionContext")),
        ("Source", "crates/wafer-plugin/src/lib.rs", ("macro_rules! payload_as_str", "$input.payload.read_all()")),
        ("Source", "plugins/content-router/src/lib.rs", ("world: \"router-node\"", "let bytes = input.payload.read_all()")),
        ("Source", "crates/wafer-core/src/registry/client.rs", ("pub async fn resolve", "fn resolve_local", "async fn resolve_oci")),
        ("Source", "crates/wafer-core/src/node/wasm.rs", ("fn build_wit_message", "pub async fn validate_and_init", "delete_buffer")),
        ("Source", "crates/wafer-core/src/engine/http.rs", ("impl WasiHttpHooks for OutboundHttpHooks", "fn authorize", "async fn socket_addr")),
        ("Test", "crates/wafer-core/tests/wasi_async_runner.rs", ("async fn delay_injector_runs_without_wasi_runtime_panic()",)),
        ("Test", "crates/wafer-core/tests/wasi_http_capability.rs", ("async fn omitted_outbound_http_denies_before_loopback_connect()", "async fn exact_loopback_destination_is_allowed()")),
        ("Test", "crates/wafer-core/tests/inference_inventory.rs", ("fn root_wit_defines_the_pinned_inference_world()", "fn host_bindings_include_inference_without_changing_ordinary_worlds()")),
        ("Test", "crates/wafer-core/src/node/wasm.rs", ("fn inference_recovery_and_reconfigure_keep_real_model_live()", "fn inference_hot_swap_adopts_real_component_between_calls()")),
    },
    "stateless-hot-swap.md": {
        ("Source", "crates/wafer-core/src/orchestrator/builder.rs", ("watch::channel(None)", "watch_senders.insert")),
        ("Source", "crates/wafer-core/src/orchestrator/hotswap.rs", ("pub async fn prepare_transform_swap_timed", "SwapPayload::Transform")),
        ("Source", "crates/wafer-core/src/orchestrator/pipeline.rs", ("pub fn send_swap", "pub fn record_hotswap_phase")),
        ("Source", "crates/wafer-core/src/runner/mod.rs", ("pub struct HotSwapProgress", "pub fn report_rolled_back", "pub enum SwapPayload")),
        ("Source", "crates/wafer-core/src/runner/transform.rs", ("swap_rx.has_changed()", "transform.recover_from_cached_pre()", "metrics.record_rollback()")),
        ("Source", "crates/wafer-core/src/node/wasm.rs", ("pub async fn try_hot_swap", "self.store = old_store", "pub async fn recover_from_cached_pre")),
        ("Source", "crates/wafer-core/src/node/metrics.rs", ("pub fn record_swap", "pub fn record_rollback", "pub fn record_recovery")),
        ("Source", "crates/wafer-core/src/api/handlers.rs", ("pub async fn hot_swap", "\"status\": \"rolled_back\"", "\"replacement_adopted\": true")),
        ("Test", "crates/wafer-core/tests/hotswap_process_time_rollback.rs", ("async fn hotswap_process_time_rollback()", "async fn hotswap_bounded_rollback_thrash()")),
    },
    "evaluation-harness.md": {
        ("Source", "eval/canonical-matrix.json", ("\"enhanced_candidate\"", "\"thesis_evidence\": false", "\"e_perf_5_claim_status\": \"PENDING\"")),
        ("Source", "eval/RESULT-CONTRACT.md", ("The expanded N=5 rehearsal is diagnostic", "Intervals and events are nested observations", "E-Perf-5 remains PENDING")),
        ("Source", "eval/scripts/run-experiment.sh", ("Usage: run-experiment.sh", "eval/RESULT-CONTRACT.md", "eval/scripts/collect-results.sh")),
        ("Source", "eval/scripts/collect-results.sh", ("never overwrites an existing directory", "mkdir -p \"$target\"")),
        ("Source", "eval/scripts/lib/canonical_runner.py", ("class RunItem", "def build_schedule", "def verify_result", "def summarize_capacity_knee")),
        ("Source", "eval/analysis/src/wafer_analysis/results_layout.py", ("class ResultsLayout", "def resolve_raw_relative")),
        ("Source", "eval/analysis/src/wafer_analysis/expanded_n5.py", ("def load_expanded_n5_selection", "def build_expanded_n5_datasets", "def validate_expanded_n5_datasets")),
        ("Test", "eval/scripts/tests/test_canonical_matrix.py", ("def test_matrix_accepts_frozen_experiments", "def test_final_capacity_repetitions_cannot_drop_below_30")),
        ("Test", "eval/analysis/test_expanded_n5.py", ("def test_selection_accepts_exact_mixed_v10_v13_composite", "def test_builds_and_renders_all_completed_families_without_mutating_sources")),
    },
}
S12_REQUIRED_SECTIONS = ("Purpose", "Flow", "Rust", "Design", "Status boundaries", "Evidence", "Checkpoint")
S12_DIAGRAM_TYPES = {
    "config-to-running-pipeline.md": "flowchart TD",
    "message-through-wasm.md": "sequenceDiagram",
    "shutdown-and-failure.md": "stateDiagram-v2",
}
S12_REQUIRED_TEXT = {
    "config-to-running-pipeline.md": (
        "wafer-core::dag::graph::DagGraph",
        "wafer-config::DagGraph",
        "build_pipeline_with_io",
        "PipelineOrchestrator::from_build_output",
    ),
    "message-through-wasm.md": (
        "transform.process(envelope).await",
        "sender.reserve().await",
        "borrow<buffer>",
        "list<u8>",
    ),
    "shutdown-and-failure.md": (
        "self.tasks.shutdown().await",
        "policy.flush_to_dlq(\"shutdown\")",
        "receiver.try_recv()",
        "sink.flush().await",
    ),
}
S12_NAV_LINKS = {
    "README.md": (
        "config-to-running-pipeline.md",
        "message-through-wasm.md",
        "shutdown-and-failure.md",
    ),
    "reading-paths.md": (
        "config-to-running-pipeline.md",
        "message-through-wasm.md",
        "shutdown-and-failure.md",
    ),
}
S13_REQUIRED_SECTIONS = (
    "Purpose", "Prerequisites", "Flow", "Rust", "Design", "Status boundaries", "Evidence", "Checkpoint"
)
S13_DIAGRAM_TYPES = {
    "plugin-boundary.md": "sequenceDiagram",
    "stateless-hot-swap.md": "sequenceDiagram",
    "evaluation-harness.md": "flowchart TD",
}
S13_REQUIRED_TEXT = {
    "plugin-boundary.md": (
        "Transform, Filter, and Router", "Sources and sinks remain native", "wit_bindgen::generate!",
        "wasmtime::component::bindgen!", "WaferRegistry::resolve", "Store<WaferState>",
    ),
    "stateless-hot-swap.md": (
        "watch::channel(None)", "swap_rx.has_changed()", "try_hot_swap", "HotSwapProgress",
        "Mutable guest state is lost", "process-time rollback",
    ),
    "evaluation-harness.md": (
        "eval/canonical-matrix.json", "eval/scripts/lib/canonical_runner.py", "eval/RESULT-CONTRACT.md",
        "eval/analysis/src/wafer_analysis/expanded_n5.py", "diagnostic", "non-poolable",
        "thesis_evidence=false", "PENDING", "CENSORED",
    ),
}
S13_NAV_LINKS = {
    "README.md": ("plugin-boundary.md", "stateless-hot-swap.md", "evaluation-harness.md"),
    "reading-paths.md": ("plugin-boundary.md", "stateless-hot-swap.md", "evaluation-harness.md"),
}
FIXTURE_CONTRACT = {
    "bad-glossary-link.json": (
        "2950d1b9ec4ba9d0d7abf1daf28738f4c61b70980ced35c5708adf7f7865016f",
        "reading-paths.md: undefined glossary link: ownership -> missing-ownership",
    ),
    "broken-local-link.json": (
        "7d896c51eb13ef89bccd4ceb57403b0dbe99571372acb00223e8a119ee68d2c9",
        "reading-paths.md: broken local link: missing-workspace-map.md",
    ),
    "malformed-mermaid.json": (
        "6a70824f5efd40f983af9ebaafca825b611539437213b1a8ac66e26de1a5726d",
        "workspace-map.md: unclosed or malformed Mermaid fence",
    ),
    "missing-glossary-term.json": (
        "4df2d330f5ba0142bf800cdda4fa01ac8d89a639618b119dc470e6980621e7bd",
        "rust-in-context.md: missing glossary terms: arc",
    ),
    "missing-prerequisites.json": (
        "32c6be0946ed3a583b04b5c98f35c998872b0831282dd1a4cc444cbf9b8f49a4",
        "workspace-map.md: expected exactly one prerequisites declaration",
    ),
    "missing-source.json": (
        "76377d600891b0db83b527c81f52cfd3968f89dd85e9f04b6940ff05d08179db",
        "workspace-map.md: broken local link: ../../crates/wafer-config/src/missing.rs",
    ),
    "missing-symbol.json": (
        "6f19a610ef9e4bfdbace533b2e975e85f2bd6235117ea660c9b81097784cbdcf",
        "workspace-map.md: missing symbol 'pub use loader::load_missing' in crates/wafer-config/src/lib.rs",
    ),
    "missing-test-evidence.json": (
        "bbb7a7e000bb7b55818ac0530dcb00abd720ce7ccc827e645585e61701dbee00",
        "README.md: missing exact test evidence",
    ),
    "overflow-policy-contradiction.json": (
        "6d95a1a453851f88cd660f0a8718acb84c4f98d9060f63d907a2e255de292e95",
        "rust-in-context.md: contradicts bounded overflow implementation",
    ),
    "s12-core-graph-evidence-substitution.json": (
        "0f664bf097be984f200993540eadd5f9fe100ec7823afaf22cd0d988ecf0a816",
        "config-to-running-pipeline.md: evidence authority drift",
    ),
    "s12-guaranteed-inflight-completion.json": (
        "1d727c93dcf7a3fa82054ae388ed8fa5a0040c41edec62720718c177e462e1f8",
        "shutdown-and-failure.md: claims all in-flight work completes",
    ),
    "s12-missing-flow-section.json": (
        "9b1ad70de6c83c242bab129b2c24bd9b2af59785c55f3e3b2be6c919e441837c",
        "config-to-running-pipeline.md: missing required section: Flow",
    ),
    "s12-missing-navigation.json": (
        "d1f118daebd183ffe4898545490be0e3f03713a11610298f7393868601beae5d",
        "README.md: missing S12 navigation link: config-to-running-pipeline.md",
    ),
    "s12-production-graph-confusion.json": (
        "55abdbf4631f05c67afe9c9832ea7677229b74528bf1b00a16f0818bf5aef9da",
        "config-to-running-pipeline.md: confuses production and standalone DagGraph types",
    ),
    "s12-reverse-topology-shutdown.json": (
        "b939d1c34295fb8938372bfd7b178892cff36070f1bd5478c98476d7372b6da7",
        "shutdown-and-failure.md: claims reverse-topological shutdown",
    ),
    "s12-wasm-inside-select.json": (
        "03dae0247925f54fde518c244d56fdf854dc82b93988744295e377233b8c15f5",
        "message-through-wasm.md: places the Wasm call inside cancellation select",
    ),
    "s12-zero-copy-boundary.json": (
        "bacfd0ca3509b46e280d818da421451e123100f19eede6c70223c2d4b50220f1",
        "message-through-wasm.md: claims the whole Wasm boundary is zero-copy",
    ),
    "valid-but-wrong-diataxis-type.json": (
        "cc5e4e1fac348baff12cbbeb9c97adceb7c6807a825941bd1363e7e78ca99e8c",
        "README.md: expected Diataxis type Explanation, found Tutorial",
    ),
    "valid-evidence-substitution.json": (
        "28733d5a2f8d1203437ff764423d29abd8e2082d9e86d3f72ba7e3af322cbac5",
        "README.md: evidence authority drift",
    ),
    "wrong-diataxis-type.json": (
        "322de1c8ee1d2538253d5941988902526723d0aed477d4823f4857c058688893",
        "README.md: invalid Diataxis type: Guide",
    ),
    "wrong-source-commit.json": (
        "c97d4e4801ab65551cfc8f514fa7eeb224675556b0273f7dfffcf9f89e1e714c",
        "README.md: source commit is not pinned",
    ),
    "s13-capacity-uncensored.json": (
        "262918b7945a1611c2f13a06879bf33d4bcd5d40ade409624c4deb04c0ff2ca5",
        "evaluation-harness.md: promotes support-confounded capacity evidence",
    ),
    "s13-eperf5-promotion.json": (
        "91dccbd5ccd41f1920a9396c2fff2d008a49d8f72b20bbebb05e689f518db9be",
        "evaluation-harness.md: promotes pending E-Perf-5 evidence",
    ),
    "s13-evidence-substitution.json": (
        "1c6c3e9b78c97119a2b9fc70bc223151fd731cabf4019adbe11044ab4c369674",
        "plugin-boundary.md: evidence authority drift",
    ),
    "s13-fabricated-rollback-transition.json": (
        "1de594b6264fdc4fe87feb1e1de4d9d09f7a11db90d9fd1614ad53647934cbdd",
        "stateless-hot-swap.md: fabricates a rollback node-state transition",
    ),
    "s13-final-n30-promotion.json": (
        "e0223550c23e13846d707f376c76027cff7d5d651564d5ee7c542bc710726552",
        "evaluation-harness.md: promotes pending final N=30 evidence",
    ),
    "s13-missing-navigation.json": (
        "926adb95b80d093abb96daec6b12a481b6de7bb80264e2259c9d765228137dd1",
        "README.md: missing S13 navigation link: plugin-boundary.md",
    ),
    "s13-n5-promotion.json": (
        "60ee299f2850868c30ca5a608d74f27813ee043e5f72ab432a7c6fdfa0da587c",
        "evaluation-harness.md: promotes diagnostic N=5 evidence",
    ),
    "s13-nested-n-inflation.json": (
        "f3d72040581579aa6021c6cea0edef843fbbbca169d5cf827d4c4bae7f9ddba4",
        "evaluation-harness.md: inflates independent N with nested or aliased observations",
    ),
    "s13-post-convergence-rollback.json": (
        "dec34df30f7965cdb4c72557a02f6ba424748a59931ae4d8dbd506259725ef0c",
        "stateless-hot-swap.md: claims every canary rollback rewrites the completed API outcome",
    ),
    "s13-state-retention.json": (
        "fdb9ae4834f32ab36d3e9e16bc961d38dec20c77610561758e11472886ded21b",
        "stateless-hot-swap.md: claims mutable guest state survives replacement",
    ),
    "s13-wasm-source-sink.json": (
        "377737cebbd31d1775bef24d523dd1f7bdd92300f2c21909f7cf186c22368a9b",
        "plugin-boundary.md: claims sources or sinks execute as Wasm roles",
    ),
    "s13-wrong-diataxis-type.json": (
        "b7d221d911575a1f0572e008574b0fc016692ae7fdd0bbcce0d017074ce482da",
        "evaluation-harness.md: expected Diataxis type Tutorial, found Reference",
    ),
    "s13-wrong-source-commit.json": (
        "f07c3ad436a68e7622646d12ac2d449781c3548037ca76c4c7cd09a84b0b2a25",
        "stateless-hot-swap.md: source commit is not pinned",
    ),
}
REQUIRED_GLOSSARY_TERMS = {
    "arc",
    "async function",
    "borrow",
    "bounded channel",
    "crate",
    "enum",
    "move",
    "ownership",
    "public re-export",
    "raii",
    "result",
    "trait object",
    "workspace",
}
EVIDENCE_RE = re.compile(
    r"^- \*\*(Source|Test):\*\* \[`([^`]+)`\]\(([^)]+)\) \| "
    r"symbols?: ((?:`[^`]+`(?:, | and )?)*)$",
    re.MULTILINE,
)
MARKDOWN_LINK_RE = re.compile(r"\[[^]]+\]\(([^)]+)\)")
GLOSSARY_ENTRY_RE = re.compile(r"^- \*\*([^*]+)\*\*: (.+)$", re.MULTILINE)
GLOSSARY_LINK_RE = re.compile(r"\[([^]]+)\]\(rust-in-context\.md#([^)]+)\)")
MERMAID_RE = re.compile(r"```mermaid\n(.*?)\n```", re.DOTALL)


class ValidationError(Exception):
    pass


def slugify(term: str) -> str:
    return re.sub(r"[^a-z0-9 -]", "", term.lower()).replace(" ", "-")


def load_pages() -> dict[str, str]:
    pages = {}
    for name in PAGES:
        path = LEARN_DIR / name
        if path.is_file():
            pages[name] = path.read_text()
    return pages


def local_target(page: str, target: str) -> Path | None:
    if target.startswith(("http://", "https://", "mailto:", "#")):
        return None
    clean_target = target.split("#", 1)[0]
    return (LEARN_DIR / page).parent / clean_target


def validate_metadata(name: str, text: str, errors: list[str]) -> None:
    type_lines = re.findall(r"^> \*\*Documentation type:\*\* (.+)$", text, re.MULTILINE)
    if len(type_lines) != 1:
        errors.append(f"{name}: expected exactly one documentation type")
    elif type_lines[0] not in DIATAXIS_TYPES:
        errors.append(f"{name}: invalid Diataxis type: {type_lines[0]}")
    elif type_lines[0] != EXPECTED_DIATAXIS_TYPES[name]:
        errors.append(
            f"{name}: expected Diataxis type {EXPECTED_DIATAXIS_TYPES[name]}, "
            f"found {type_lines[0]}"
        )

    prerequisite_lines = re.findall(r"^> \*\*Prerequisites:\*\* (.+)$", text, re.MULTILINE)
    if len(prerequisite_lines) != 1:
        errors.append(f"{name}: expected exactly one prerequisites declaration")


def validate_links_and_evidence(name: str, text: str, errors: list[str]) -> None:
    for target in MARKDOWN_LINK_RE.findall(text):
        path = local_target(name, target)
        if path is not None and not path.exists():
            errors.append(f"{name}: broken local link: {target}")

    evidence = list(EVIDENCE_RE.finditer(text))
    kinds = {match.group(1) for match in evidence}
    if "Source" not in kinds:
        errors.append(f"{name}: missing exact source evidence")
    if "Test" not in kinds:
        errors.append(f"{name}: missing exact test evidence")

    actual_evidence = {
        (
            match.group(1),
            match.group(2),
            tuple(re.findall(r"`([^`]+)`", match.group(4))),
        )
        for match in evidence
    }
    if len(evidence) != len(EXPECTED_EVIDENCE[name]) or actual_evidence != EXPECTED_EVIDENCE[name]:
        errors.append(f"{name}: evidence authority drift")

    for match in evidence:
        label, displayed, target, symbol_text = match.groups()
        path = local_target(name, target)
        if path is None or not path.is_file():
            errors.append(f"{name}: {label.lower()} evidence target is not a file: {target}")
            continue
        try:
            displayed_path = PurePosixPath(displayed)
            actual_path = PurePosixPath(path.resolve().relative_to(ROOT).as_posix())
        except ValueError:
            errors.append(f"{name}: evidence target escapes the repository: {target}")
            continue
        if displayed_path != actual_path:
            errors.append(
                f"{name}: evidence label {displayed_path} does not match target {actual_path}"
            )
        source_text = path.read_text()
        for symbol in re.findall(r"`([^`]+)`", symbol_text):
            if symbol not in source_text:
                errors.append(f"{name}: missing symbol '{symbol}' in {displayed_path}")


def validate_mermaid(name: str, text: str, errors: list[str]) -> None:
    if text.count("```mermaid") != len(MERMAID_RE.findall(text)):
        errors.append(f"{name}: unclosed or malformed Mermaid fence")
    for diagram in MERMAID_RE.findall(text):
        first_line = diagram.splitlines()[0].strip() if diagram.strip() else ""
        if first_line not in {"flowchart LR", "flowchart TD", "sequenceDiagram", "stateDiagram-v2"}:
            errors.append(f"{name}: unsupported Mermaid declaration: {first_line or '<empty>'}")
        if "C4" in diagram or "architecture-beta" in diagram:
            errors.append(f"{name}: experimental Mermaid syntax is not allowed")


def validate_glossary(pages: dict[str, str], errors: list[str]) -> None:
    rust_page = pages.get("rust-in-context.md", "")
    entries = GLOSSARY_ENTRY_RE.findall(rust_page)
    terms: dict[str, str] = {}
    for term, definition in entries:
        normalized = term.strip().lower()
        if normalized in terms:
            errors.append(f"rust-in-context.md: duplicate glossary term: {normalized}")
        if not definition.strip():
            errors.append(f"rust-in-context.md: empty glossary definition: {normalized}")
        terms[normalized] = definition

    missing = sorted(REQUIRED_GLOSSARY_TERMS - terms.keys())
    if missing:
        errors.append(f"rust-in-context.md: missing glossary terms: {', '.join(missing)}")

    for name, text in pages.items():
        for label, anchor in GLOSSARY_LINK_RE.findall(text):
            normalized = label.strip().lower()
            if normalized not in terms or slugify(normalized) != anchor:
                errors.append(f"{name}: undefined glossary link: {label} -> {anchor}")


def validate_s12_contract(pages: dict[str, str], errors: list[str]) -> None:
    for name, diagram_type in S12_DIAGRAM_TYPES.items():
        text = pages.get(name, "")
        for section in S12_REQUIRED_SECTIONS:
            if f"## {section}" not in text:
                errors.append(f"{name}: missing required section: {section}")
        diagrams = MERMAID_RE.findall(text)
        if len(diagrams) != 1 or not diagrams[0].startswith(diagram_type + "\n"):
            errors.append(f"{name}: expected exactly one {diagram_type} Mermaid diagram")
        for required in S12_REQUIRED_TEXT[name]:
            if required not in text:
                errors.append(f"{name}: missing required implementation anchor: {required}")

    for name, links in S12_NAV_LINKS.items():
        text = pages.get(name, "")
        for link in links:
            if f"]({link})" not in text:
                errors.append(f"{name}: missing S12 navigation link: {link}")

    claim_checks = (
        (
            "config-to-running-pipeline.md",
            r"production\s+(?:builder|pipeline).{0,80}(?:uses|constructs|imports).{0,40}wafer[-_]config::DagGraph",
            "confuses production and standalone DagGraph types",
        ),
        (
            "message-through-wasm.md",
            r"(?:Wasm|transform)\s+call.{0,80}(?:inside|within).{0,30}(?:cancellation\s+)?`?select!`?",
            "places the Wasm call inside cancellation select",
        ),
        (
            "message-through-wasm.md",
            r"(?:entire|whole|full)\s+(?:Wasm|Component Model|host.?guest)?\s*boundary.{0,30}(?:is|remains)\s+(?:fully\s+)?zero-copy",
            "claims the whole Wasm boundary is zero-copy",
        ),
        (
            "shutdown-and-failure.md",
            r"shuts?\s+down.{0,40}reverse\s+topological\s+order",
            "claims reverse-topological shutdown",
        ),
        (
            "shutdown-and-failure.md",
            r"(?<!not )guarantees?.{0,60}(?:every|all)\s+in-flight.{0,30}(?:completes?|finishes?)",
            "claims all in-flight work completes",
        ),
    )
    for name, pattern, diagnostic in claim_checks:
        if re.search(pattern, pages.get(name, ""), re.IGNORECASE | re.DOTALL):
            errors.append(f"{name}: {diagnostic}")


def validate_s13_contract(pages: dict[str, str], errors: list[str]) -> None:
    for name, diagram_type in S13_DIAGRAM_TYPES.items():
        text = pages.get(name, "")
        for section in S13_REQUIRED_SECTIONS:
            if f"## {section}" not in text:
                errors.append(f"{name}: missing required section: {section}")
        diagrams = MERMAID_RE.findall(text)
        if len(diagrams) != 1 or not diagrams[0].startswith(diagram_type + "\n"):
            errors.append(f"{name}: expected exactly one {diagram_type} Mermaid diagram")
        for required in S13_REQUIRED_TEXT[name]:
            if required not in text:
                errors.append(f"{name}: missing required implementation anchor: {required}")
        if "f173151a8951736b4d82e10ce2b1c4417b02cf99" not in text:
            errors.append(f"{name}: source commit is not pinned")

    for name, links in S13_NAV_LINKS.items():
        text = pages.get(name, "")
        for link in links:
            if f"]({link})" not in text:
                errors.append(f"{name}: missing S13 navigation link: {link}")

    claim_checks = (
        ("plugin-boundary.md", r"(?:source|sink)s?.{0,40}(?:is|are|runs?|execute).{0,20}(?:Wasm|WebAssembly)", "claims sources or sinks execute as Wasm roles"),
        ("stateless-hot-swap.md", r"(?:preserves?|retains?|restores?).{0,50}(?:mutable\s+)?guest\s+state", "claims mutable guest state survives replacement"),
        ("stateless-hot-swap.md", r"rollback.{0,50}(?:transition|state).{0,30}(?:NodeState|state machine)", "fabricates a rollback node-state transition"),
        ("stateless-hot-swap.md", r"Every\s+process-time\s+rollback.{0,160}(?:API\s+)?caller.{0,80}(?:rolled_back|error outcome)", "claims every canary rollback rewrites the completed API outcome"),
        ("evaluation-harness.md", r"N\s*=\s*5\s+(?:is|counts as)\s+(?:final|thesis evidence|poolable)", "promotes diagnostic N=5 evidence"),
        ("evaluation-harness.md", r"(?:aliases|events|buckets|intervals).{0,80}(?:independent|increase|add).{0,20}N", "inflates independent N with nested or aliased observations"),
        ("evaluation-harness.md", r"Final\s+N\s*=\s*30\s+(?:is|has status)\s+(?:COMPLETE|FINAL|PASSED)", "promotes pending final N=30 evidence"),
        ("evaluation-harness.md", r"E-Perf-5\s+(?:is|has status)\s+(?:COMPLETE|FINAL|PASSED)", "promotes pending E-Perf-5 evidence"),
        ("evaluation-harness.md", r"support-confounded.{0,60}(?:final|supported|uncensored)", "promotes support-confounded capacity evidence"),
    )
    for name, pattern, diagnostic in claim_checks:
        if re.search(pattern, pages.get(name, ""), re.IGNORECASE | re.DOTALL):
            errors.append(f"{name}: {diagnostic}")


def validate_pages(pages: dict[str, str]) -> list[str]:
    errors: list[str] = []
    for name in PAGES:
        text = pages.get(name)
        if text is None:
            errors.append(f"missing learning page: docs/learn/{name}")
            continue
        validate_metadata(name, text, errors)
        validate_links_and_evidence(name, text, errors)
        validate_mermaid(name, text, errors)
        claim_text = "\n".join(
            line
            for line in text.splitlines()
            if not line.startswith(
                ("**Intended design:**", "**Known drift:**", "**Historical context:**")
            )
        )
        if re.search(
            r"configured\s+`?Drop`?\s+(?:and|or|/)\s+`?DeadLetter`?\s+overflow policies\s+"
            r"(?:currently take effect|are currently (?:applied|honored|dispatched))",
            claim_text,
            re.IGNORECASE,
        ):
            errors.append(f"{name}: contradicts bounded overflow implementation")
        for label in ("Current implementation", "Intended design", "Known drift"):
            if label not in text:
                errors.append(f"{name}: missing status label: {label}")
        if "—" in text:
            errors.append(f"{name}: em dash is not allowed")
        if re.search(r"state[- ]preserv(?:ing|ation)", text, re.IGNORECASE):
            errors.append(f"{name}: state-preserving replacement claim is not allowed")

    readme = pages.get("README.md", "")
    if "f173151a8951736b4d82e10ce2b1c4417b02cf99" not in readme:
        errors.append("README.md: source commit is not pinned")
    if "## Orientation" not in readme:
        errors.append("README.md: must remain an orientation page")

    validate_glossary(pages, errors)
    validate_s12_contract(pages, errors)
    validate_s13_contract(pages, errors)
    return errors


def validate_fixtures(pages: dict[str, str], fixtures_dir: Path) -> tuple[list[str], int]:
    errors = []
    paths = {path.name: path for path in fixtures_dir.glob("*.json")}
    actual_names = set(paths)
    expected_names = set(FIXTURE_CONTRACT)
    missing = sorted(expected_names - actual_names)
    unexpected = sorted(actual_names - expected_names)
    if missing:
        errors.append(f"missing learning-doc fixtures: {', '.join(missing)}")
    if unexpected:
        errors.append(f"unexpected learning-doc fixtures: {', '.join(unexpected)}")

    count = 0
    for name in sorted(expected_names & actual_names):
        path = paths[name]
        fixture_bytes = path.read_bytes()
        expected_hash, expected_diagnostic = FIXTURE_CONTRACT[name]
        if hashlib.sha256(fixture_bytes).hexdigest() != expected_hash:
            errors.append(f"{name}: fixture content does not match contract")
            continue
        fixture = json.loads(fixture_bytes)
        if fixture.get("expected_error") != expected_diagnostic:
            errors.append(f"{name}: intended diagnostic does not match contract")
            continue
        page = fixture["page"]
        if page not in pages:
            errors.append(f"{name}: unknown fixture page: {page}")
            continue
        find = fixture["find"]
        if pages[page].count(find) != 1:
            errors.append(f"{name}: fixture find text is not unique")
            continue
        candidate = dict(pages)
        candidate[page] = pages[page].replace(find, fixture["replace"], 1)
        diagnostics = validate_pages(candidate)
        if expected_diagnostic not in diagnostics:
            errors.append(
                f"{name}: expected diagnostic '{expected_diagnostic}', got: {diagnostics}"
            )
        count += 1
    return errors, count


def main() -> int:
    parser = argparse.ArgumentParser(description="Validate docs/learn source evidence and structure")
    parser.add_argument(
        "--fixtures",
        type=Path,
        default=ROOT / "scripts/fixtures/learning-docs",
    )
    args = parser.parse_args()

    try:
        pages = load_pages()
        errors = validate_pages(pages)
        fixture_errors, fixture_count = validate_fixtures(pages, args.fixtures)
        errors.extend(fixture_errors)
    except (OSError, json.JSONDecodeError, KeyError, TypeError, ValidationError) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        return 1

    if errors:
        print("FAIL: learning documentation audit", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1

    print(
        f"PASS: learning documentation; pages={len(pages)}; "
        f"negative fixtures={fixture_count}; source links, Diataxis metadata, "
        "Mermaid fences, and glossary checked"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
