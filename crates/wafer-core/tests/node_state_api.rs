#![cfg(test)]
#![cfg(feature = "http-api")]
#![expect(clippy::print_stderr, reason = "integration test diagnostic output")]
#![expect(
    clippy::large_futures,
    reason = "test: launch_pipeline future is large due to WASM Store/Component loading"
)]
//! `GET /api/v1/nodes` reports every started node as `running`.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use wafer_core::api::{ApiConfig, ApiServer};
use wafer_core::orchestrator::launch_pipeline;
use wafer_types::config::Config;

const PASS_THROUGH_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/pass-through/target/wasm32-wasip2/release/wafer_pass_through.wasm"
);

fn config() -> Config {
    let toml = format!(
        r#"
[pipeline]
name = "node-state"

[nodes.source]
type = "source"
kind = "bench-source"
rate = 20.0
total_messages = 1000
warmup_messages = 0
payload_size = 16

[nodes.transform]
type = "transform"
plugin = {PASS_THROUGH_WASM:?}

[nodes.sink]
type = "sink"
kind = "stdout"

[[edges]]
from = "source"
to = "transform"

[[edges]]
from = "transform"
to = "sink"
"#,
    );
    toml::from_str(&toml).expect("inline config must parse")
}

async fn get_json(url: &str) -> serde_json::Value {
    let body = reqwest::get(url).await.expect("GET").text().await.expect("body");
    serde_json::from_str(&body).expect("json body")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn started_nodes_report_running_in_id_order() {
    if !Path::new(PASS_THROUGH_WASM).exists() {
        eprintln!("SKIP: pass-through.wasm not built at {PASS_THROUGH_WASM}");
        return;
    }

    let mut orchestrator = launch_pipeline(config(), None).await.expect("launch_pipeline");
    let handle = Arc::new(orchestrator.handle());

    let deadline = Instant::now() + Duration::from_secs(10);
    while handle.node_metrics("sink").map_or(0, |m| m.processed()) == 0 {
        assert!(Instant::now() < deadline, "no message reached the sink");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let config = ApiConfig { bind: "127.0.0.1:0".parse().expect("addr"), serve_metrics: false };
    let server = ApiServer::new(config, Arc::clone(&handle)).await.expect("bind api");
    let addr = server.local_addr().expect("api addr");
    let api = tokio::spawn(server.run());

    let body = get_json(&format!("http://{addr}/api/v1/nodes")).await;
    let nodes: Vec<(&str, &str)> = body
        .as_array()
        .expect("array")
        .iter()
        .map(|n| (n["id"].as_str().expect("id"), n["state"].as_str().expect("state")))
        .collect();
    assert_eq!(nodes, [("sink", "running"), ("source", "running"), ("transform", "running")]);

    let node = get_json(&format!("http://{addr}/api/v1/nodes/transform")).await;
    assert_eq!(node["state"], "running");

    api.abort();
    orchestrator.shutdown().await.expect("pipeline shuts down");
}
