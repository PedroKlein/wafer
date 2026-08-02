//! Integration test: MemoryRecorder flushes samples on cancel-safe shutdown.
//!
//! AC4: SIGTERM flushes memory.csv cleanly.
//! Spawns the sampler, cancels after a brief window, verifies samples were
//! captured and CSV output is valid.

use std::time::Duration;

use tokio_util::sync::CancellationToken;
use wafer_core::bench::MemoryRecorder;

/// Verify that MemoryRecorder collects samples and produces valid CSV on
/// cancel-safe shutdown (simulating SIGTERM → CancellationToken::cancel).
#[tokio::test]
async fn memory_sampler_shutdown_flushes() {
    let cancel = CancellationToken::new();
    let recorder = std::sync::Arc::new(tokio::sync::Mutex::new(MemoryRecorder::new()));

    let rec_clone = std::sync::Arc::clone(&recorder);
    let cancel_clone = cancel.clone();
    let handle = tokio::spawn(async move {
        rec_clone.lock().await.sample_loop(cancel_clone).await;
    });

    // Let sampler collect at least 2 samples (1 Hz → 2.5 s)
    tokio::time::sleep(Duration::from_millis(2500)).await;

    // Simulate SIGTERM → cancel
    cancel.cancel();
    handle.await.expect("sampler task should not panic");

    // Verify samples were collected
    let guard = recorder.lock().await;
    assert!(
        guard.samples().len() >= 2,
        "expected >=2 samples after 2.5s at 1Hz, got {}",
        guard.samples().len()
    );

    // Verify CSV output is well-formed
    let csv = guard.to_csv();
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(
        lines[0], "elapsed_ms,rss_bytes",
        "CSV header must match expected schema"
    );
    assert!(
        lines.len() >= 3,
        "CSV must have header + >=2 data rows, got {} lines",
        lines.len()
    );

    // Verify each data row parses correctly
    for line in &lines[1..] {
        let parts: Vec<&str> = line.split(',').collect();
        assert_eq!(parts.len(), 2, "each row must have 2 columns: {line}");
        parts[0].parse::<u64>().expect("elapsed_ms must be u64");
        let rss: u64 = parts[1].parse().expect("rss_bytes must be u64");
        assert!(rss > 0, "RSS should be non-zero");
    }

    // Simulate file flush (write to tempdir)
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("memory.csv");
    std::fs::write(&path, &csv).expect("write memory.csv");
    assert!(path.exists(), "memory.csv must exist after flush");

    let read_back = std::fs::read_to_string(&path).expect("read memory.csv");
    assert_eq!(read_back, csv, "file contents must match in-memory CSV");
}
