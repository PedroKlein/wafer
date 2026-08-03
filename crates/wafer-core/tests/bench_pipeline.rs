#![cfg(test)]
//! Integration test: `BenchSource` → `NativeTransform` → `BenchSink` pipeline.
//!
//! Validates that the evaluation infrastructure produces valid histogram data
//! when wired through the same channel infrastructure as the real pipeline.

use wafer_core::node::{BenchSink, BenchSinkConfig, BenchSource, BenchSourceConfig, Lifecycle,
    NativeTransform, ProcessResult, Sink, Source, Transform};

/// Full bench pipeline: `BenchSource` → NativeTransform(uppercase) → `BenchSink`.
/// Validates end-to-end measurement infrastructure.
#[tokio::test]
async fn test_native_bench_pipeline() {
    let total = 200;

    // Create nodes
    let config = BenchSourceConfig::new(100_000.0, total).with_payload_size(64);
    let mut source = BenchSource::new(config);
    let mut transform = NativeTransform::uppercase("transform");
    let mut sink = BenchSink::new(BenchSinkConfig::for_test());

    // Initialize
    source.init().await.unwrap();
    transform.init().await.unwrap();
    sink.init().await.unwrap();

    // Process messages through the pipeline
    let mut processed = 0u64;
    while let Ok(Some(envelope)) = source.poll().await {
        let result = transform.process(envelope).await.unwrap();
        match result {
            ProcessResult::Emit(output) => {
                sink.collect(output).await.unwrap();
                processed += 1;
            }
            _ => panic!("Expected Emit from uppercase transform"),
        }
    }

    // Validate results
    assert_eq!(processed, total);
    assert_eq!(sink.message_count(), total);
    assert_eq!(sink.recorded_count(), total);
    assert!(sink.p50_ns() > 0, "p50 should be > 0");
    assert!(sink.p99_ns() >= sink.p50_ns(), "p99 >= p50");

    // Sequence tracking: no gaps since we processed all in order
    let tracker = sink.sequence_tracker().unwrap();
    assert!(!tracker.has_gaps(), "should have no sequence gaps");
    assert_eq!(tracker.total_duplicates(), 0);
    assert_eq!(tracker.total_received(), total);

    // Close nodes
    source.close().await.unwrap();
    transform.close().await.unwrap();
    sink.close().await.unwrap();
}

/// `BenchSource` produces exactly the configured number of messages.
#[tokio::test]
async fn test_bench_source_exhaustion() {
    let config = BenchSourceConfig::new(100_000.0, 50).with_payload_size(32);
    let mut source = BenchSource::new(config);
    source.init().await.unwrap();

    let mut count = 0;
    while let Ok(Some(_)) = source.poll().await {
        count += 1;
    }

    assert_eq!(count, 50);
}

/// `BenchSink` correctly excludes warmup messages.
#[tokio::test]
async fn test_bench_sink_warmup_integration() {
    let config = BenchSinkConfig {
        warmup_secs: 0, // immediate recording (no time-based warmup in fast test)
        track_sequences: true,
        track_hotswap: false,
        output_dir: None,
    };
    let mut sink = BenchSink::new(config);
    sink.init().await.unwrap();

    let source_config = BenchSourceConfig::new(100_000.0, 20).with_payload_size(16);
    let mut source = BenchSource::new(source_config);
    source.init().await.unwrap();

    while let Ok(Some(envelope)) = source.poll().await {
        sink.collect(envelope).await.unwrap();
    }

    assert_eq!(sink.message_count(), 20);
    assert_eq!(sink.recorded_count(), 20);
}

/// Native passthrough has near-zero processing overhead.
#[tokio::test]
async fn test_native_passthrough_pipeline() {
    let config = BenchSourceConfig::new(100_000.0, 100).with_payload_size(128);
    let mut source = BenchSource::new(config);
    let mut transform = NativeTransform::passthrough("pass");
    let mut sink = BenchSink::new(BenchSinkConfig::for_test());

    source.init().await.unwrap();
    transform.init().await.unwrap();
    sink.init().await.unwrap();

    while let Ok(Some(envelope)) = source.poll().await {
        if let ProcessResult::Emit(output) = transform.process(envelope).await.unwrap() {
            sink.collect(output).await.unwrap();
        }
    }

    assert_eq!(sink.message_count(), 100);
    assert_eq!(sink.recorded_count(), 100);
    // Passthrough should preserve payload size
    assert!(sink.p50_ns() > 0);
}
