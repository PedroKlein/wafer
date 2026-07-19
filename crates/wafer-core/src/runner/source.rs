//! Source adapter loop — bridges Source trait poll() to downstream channels.
//!
//! Source::poll() is native Rust I/O (mpsc recv, file read, HTTP accept) —
//! safe inside select! because there's no Wasm Store to poison on cancellation.
//! See docs/rfcs/RFC-010-io-integration.md Decision 1.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::node::Source;
use crate::node::{NodeMetrics, NodeStateTracker};
use crate::runner::{DownstreamSender, send_downstream};

/// Run a source adapter loop until cancellation, EOF, or unrecoverable error.
///
/// # Shutdown semantics
///
/// - `cancel.cancelled()` → break immediately (biased check)
/// - `source.poll()` returns `Ok(None)` → EOF, source exhausted, break
/// - `source.poll()` returns `Err(_)` → transient I/O error, log and continue
///
/// On exit, `source.close()` is always called. When the function returns,
/// all `senders` are dropped, which propagates EOF downstream (receivers
/// see `None` on `recv()`).
pub async fn run_source_loop(
    mut source: Box<dyn Source + Send>,
    senders: Vec<DownstreamSender>,
    cancel: CancellationToken,
    _state: Arc<NodeStateTracker>,
    metrics: Arc<NodeMetrics>,
) {
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            result = source.poll() => {
                match result {
                    Ok(Some(envelope)) => {
                        // Sources don't "process" — 0ns duration
                        metrics.record_processed(0);
                        send_downstream(&senders, envelope).await;
                    }
                    Ok(None) => break, // EOF — source exhausted
                    Err(e) => {
                        metrics.record_failed();
                        tracing::warn!(
                            source = source.id(),
                            error = %e,
                            "source poll error, continuing"
                        );
                    }
                }
            }
        }
    }

    if let Err(e) = source.close().await {
        tracing::warn!(source = source.id(), error = %e, "source close error");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::channel::ChannelSource;
    use crate::queue::RuntimeEnvelope;
    use tokio::sync::mpsc;
    use std::time::Duration;

    #[tokio::test]
    async fn test_source_loop_messages_flow() {
        let (tx, source) = ChannelSource::new("test-src");
        let (out_tx, mut out_rx) = mpsc::channel(32);
        let senders = vec![DownstreamSender { sender: out_tx, port: "out".into() }];
        let cancel = CancellationToken::new();
        let state = Arc::new(NodeStateTracker::running());
        let metrics = Arc::new(NodeMetrics::new());

        let handle = tokio::spawn(run_source_loop(
            Box::new(source),
            senders,
            cancel.clone(),
            state,
            metrics.clone(),
        ));

        // Send 10 messages
        for i in 0..10 {
            tx.send(RuntimeEnvelope::from_string("s", format!("msg-{i}")))
                .await
                .unwrap();
        }
        drop(tx); // Signal EOF

        // Wait for loop to finish
        handle.await.unwrap();

        // Collect all received messages
        let mut received = Vec::new();
        while let Ok(env) = out_rx.try_recv() {
            received.push(env);
        }

        assert_eq!(received.len(), 10);
        for (i, env) in received.iter().enumerate() {
            assert_eq!(env.payload_as_string(), format!("msg-{i}"));
        }
        assert_eq!(metrics.processed(), 10);
        assert_eq!(metrics.failed(), 0);
    }

    #[tokio::test]
    async fn test_source_loop_eof_terminates() {
        let (tx, source) = ChannelSource::new("eof-src");
        let (out_tx, _out_rx) = mpsc::channel(32);
        let senders = vec![DownstreamSender { sender: out_tx, port: "out".into() }];
        let cancel = CancellationToken::new();
        let state = Arc::new(NodeStateTracker::running());
        let metrics = Arc::new(NodeMetrics::new());

        let handle = tokio::spawn(run_source_loop(
            Box::new(source),
            senders,
            cancel.clone(),
            state,
            metrics,
        ));

        // Drop sender immediately → source sees EOF on next poll
        drop(tx);

        // Loop should terminate without needing cancel
        let result = tokio::time::timeout(Duration::from_secs(1), handle).await;
        assert!(result.is_ok(), "source loop should terminate on EOF");
    }

    #[tokio::test]
    async fn test_source_loop_cancel_stops() {
        let (_tx, source) = ChannelSource::new("cancel-src");
        let (out_tx, _out_rx) = mpsc::channel(32);
        let senders = vec![DownstreamSender { sender: out_tx, port: "out".into() }];
        let cancel = CancellationToken::new();
        let state = Arc::new(NodeStateTracker::running());
        let metrics = Arc::new(NodeMetrics::new());

        let handle = tokio::spawn(run_source_loop(
            Box::new(source),
            senders,
            cancel.clone(),
            state,
            metrics,
        ));

        // Let the loop start polling (it will block on recv since _tx is held)
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Cancel should make the loop exit promptly
        cancel.cancel();

        let result = tokio::time::timeout(Duration::from_secs(1), handle).await;
        assert!(result.is_ok(), "source loop should stop on cancel");
    }
}
