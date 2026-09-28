#![cfg(test)]
//! A trapping guest leaves a record: with a file DLQ configured, every
//! message whose call trapped is written to the dead-letter file, counted as
//! `dlq_sent`, and the per-node accounting still closes.

use std::path::Path;
use std::time::Duration;

use wafer_core::orchestrator::launch_pipeline;
use wafer_types::config::Config;

const ATK_BUFFER_OVERFLOW: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/attacks/buffer-overflow/target/wasm32-wasip2/release/wafer_attack_buffer_overflow.wasm"
);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trapped_messages_are_written_to_the_dead_letter_file() {
    assert!(
        Path::new(ATK_BUFFER_OVERFLOW).is_file(),
        "required plugin not built: {ATK_BUFFER_OVERFLOW} (run `mise run //plugins:build-plugins`)"
    );
    let dir = tempfile::tempdir().expect("tempdir");
    let dlq_path = dir.path().join("dlq.jsonl");
    let config: Config = toml::from_str(&format!(
        r#"
[pipeline]
name = "dead-letter-evidence"

[dead_letter]
kind = "file"
path = {dlq:?}

[nodes.source]
type = "source"
kind = "bench-source"
rate = 100.0
total_messages = 3
warmup_messages = 0
payload_size = 128

[nodes.attack]
type = "transform"
plugin = {plugin:?}

[nodes.sink]
type = "sink"
kind = "bench-sink"
warmup_secs = 0
track_sequences = false
track_hotswap = false

[[edges]]
from = "source"
to = "attack"

[[edges]]
from = "attack"
to = "sink"
"#,
        dlq = dlq_path.display().to_string(),
        plugin = ATK_BUFFER_OVERFLOW,
    ))
    .expect("inline config must parse");

    let mut orchestrator =
        Box::pin(launch_pipeline(config, None)).await.expect("pipeline must launch");
    tokio::time::timeout(Duration::from_secs(10), orchestrator.run_until_complete())
        .await
        .expect("three attack messages must complete within ten seconds")
        .expect("pipeline must shut down cleanly, DLQ sink included");
    orchestrator.export_per_node_metrics(dir.path()).expect("export metrics");

    let csv = std::fs::read_to_string(dir.path().join("per_node_metrics.csv")).expect("csv");
    let mut lines = csv.lines();
    let header = lines.next().expect("header").split(',');
    let row: std::collections::HashMap<&str, u64> = header
        .zip(lines.find(|line| line.starts_with("attack,")).expect("attack row").split(','))
        .filter_map(|(column, value)| value.parse().ok().map(|value| (column, value)))
        .collect();
    for (column, expected) in [
        ("messages_in", 3),
        ("messages_out", 0),
        ("traps_memory_out_of_bounds", 3),
        ("dlq_sent", 3),
        ("dlq_lost", 0),
        ("dropped_on_recovery", 0),
        ("recovery_count", 3),
    ] {
        assert_eq!(row[column], expected, "{column} in {row:?}");
    }
    assert_eq!(
        row["messages_in"],
        row["messages_out"]
            + row["filtered_out"]
            + row["skipped"]
            + row["retry_exhausted_skips"]
            + row["dlq_sent"]
            + row["dlq_lost"]
            + row["dropped_on_recovery"]
            + row["dropped_on_teardown"],
        "every dequeued message has exactly one fate: {row:?}"
    );

    let records: Vec<serde_json::Value> = std::fs::read_to_string(&dlq_path)
        .expect("dead-letter file")
        .lines()
        .map(|line| serde_json::from_str(line).expect("one JSON record per line"))
        .collect();
    assert_eq!(records.len(), 3, "one record per trapped message, all flushed before exit");
    for record in &records {
        assert_eq!(record["source_node"], "attack");
        assert_eq!(record["reason"]["type"], "trapped");
        assert_eq!(record["reason"]["kind"], "memory_out_of_bounds");
        assert_eq!(record["error_category"], serde_json::Value::Null);
        assert!(
            record["error_message"].as_str().is_some_and(|m| m.contains("out of bounds")),
            "{record}"
        );
        assert_eq!(record["retry_count"], 0);
        assert!(record["original"]["payload"].as_str().is_some_and(|p| !p.is_empty()));
    }
}
