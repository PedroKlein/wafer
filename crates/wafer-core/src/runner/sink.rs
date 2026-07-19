//! Sink adapter loop — receives from channel, collects, drains on shutdown.
//!
//! Sink::collect() is native Rust I/O — safe inside the event loop.
//! Batch timeout support: if `sink.batch_timeout()` is `Some`, a timer arm
//! triggers periodic flush of buffered messages.
//!
//! See docs/rfcs/RFC-010-io-integration.md Decision 2.

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::node::Sink;
use crate::node::{NodeMetrics, NodeStateTracker};
use crate::queue::RuntimeEnvelope;

/// Run a sink adapter loop until cancellation or upstream close.
///
/// # Shutdown semantics
///
/// 1. `cancel.cancelled()` OR `receiver.recv() = None` → break from loop
/// 2. Drain remaining messages from channel buffer via `try_recv()`
/// 3. Call `flush()` (ensures batched data is written)
/// 4. Call `close()` (cleanup resources)
///
/// Steps 3–4 run unconditionally, even if drain encounters errors.
pub async fn run_sink_loop(
    mut sink: Box<dyn Sink + Send>,
    mut receiver: mpsc::Receiver<RuntimeEnvelope>,
    cancel: CancellationToken,
    _state: Arc<NodeStateTracker>,
    metrics: Arc<NodeMetrics>,
) {
    let batch_timeout = sink.batch_timeout();

    loop {
        let envelope = if let Some(timeout) = batch_timeout {
            // Batching mode: recv with timeout for periodic flush
            tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                msg = receiver.recv() => match msg {
                    Some(e) => Some(e),
                    None => break, // All senders dropped — upstream exited
                },
                () = tokio::time::sleep(timeout) => {
                    if let Err(e) = sink.flush().await {
                        tracing::warn!(sink = sink.id(), error = %e, "batch flush error");
                    }
                    continue;
                }
            }
        } else {
            // No batching: simple recv with cancel
            tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                msg = receiver.recv() => match msg {
                    Some(e) => Some(e),
                    None => break, // All senders dropped — upstream exited
                },
            }
        };

        if let Some(envelope) = envelope {
            match sink.collect(envelope).await {
                Ok(()) => metrics.record_processed(0),
                Err(e) => {
                    metrics.record_failed();
                    tracing::warn!(sink = sink.id(), error = %e, "sink collect error");
                }
            }
        }
    }

    // Drain remaining messages from the channel buffer
    while let Ok(envelope) = receiver.try_recv() {
        if let Err(e) = sink.collect(envelope).await {
            tracing::warn!(sink = sink.id(), error = %e, "sink drain error");
        }
    }

    // Always flush + close, even if drain had errors
    if let Err(e) = sink.flush().await {
        tracing::warn!(sink = sink.id(), error = %e, "sink final flush error");
    }
    if let Err(e) = sink.close().await {
        tracing::warn!(sink = sink.id(), error = %e, "sink close error");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::channel::ChannelSink;
    use std::time::Duration;

    #[tokio::test]
    async fn test_sink_loop_messages_collected() {
        let (sink, mut rx) = ChannelSink::new("test-sink");
        let (tx, receiver) = mpsc::channel(32);
        let cancel = CancellationToken::new();
        let state = Arc::new(NodeStateTracker::running());
        let metrics = Arc::new(NodeMetrics::new());

        let handle = tokio::spawn(run_sink_loop(
            Box::new(sink),
            receiver,
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
        // Drop sender to signal upstream done
        drop(tx);

        // Wait for loop to finish
        handle.await.unwrap();

        // Collect all received messages
        let mut received = Vec::new();
        while let Ok(env) = rx.try_recv() {
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
    async fn test_sink_loop_channel_close_drains() {
        let (sink, mut rx) = ChannelSink::new("drain-sink");
        let (tx, receiver) = mpsc::channel(32);
        let cancel = CancellationToken::new();
        let state = Arc::new(NodeStateTracker::running());
        let metrics = Arc::new(NodeMetrics::new());

        let handle = tokio::spawn(run_sink_loop(
            Box::new(sink),
            receiver,
            cancel.clone(),
            state,
            metrics.clone(),
        ));

        // Send some messages and drop sender
        for i in 0..5 {
            tx.send(RuntimeEnvelope::from_string("s", format!("drain-{i}")))
                .await
                .unwrap();
        }
        drop(tx);

        // Wait for loop to finish (should drain + flush + close)
        let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(result.is_ok(), "sink loop should terminate on channel close");

        // All messages should have been collected
        let mut received = Vec::new();
        while let Ok(env) = rx.try_recv() {
            received.push(env);
        }
        assert_eq!(received.len(), 5);
    }

    #[tokio::test]
    async fn test_sink_loop_cancel_drains_and_flushes() {
        let (sink, mut rx) = ChannelSink::new("cancel-sink");
        let (tx, receiver) = mpsc::channel(32);
        let cancel = CancellationToken::new();
        let state = Arc::new(NodeStateTracker::running());
        let metrics = Arc::new(NodeMetrics::new());

        let handle = tokio::spawn(run_sink_loop(
            Box::new(sink),
            receiver,
            cancel.clone(),
            state,
            metrics.clone(),
        ));

        // Send messages
        for i in 0..3 {
            tx.send(RuntimeEnvelope::from_string("s", format!("cancel-{i}")))
                .await
                .unwrap();
        }

        // Give sink time to process the messages already sent
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Send more messages that may still be in channel when cancel fires
        tx.send(RuntimeEnvelope::from_string("s", "after-delay"))
            .await
            .unwrap();

        // Cancel — should drain remaining messages
        cancel.cancel();

        let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(result.is_ok(), "sink loop should stop on cancel");

        // Verify messages were collected (at least the ones before cancel)
        let mut received = Vec::new();
        while let Ok(env) = rx.try_recv() {
            received.push(env);
        }
        // At minimum the 3 messages sent before delay should arrive;
        // the "after-delay" message should also be drained
        assert!(received.len() >= 3);
        assert!(received.len() <= 4);
    }
}
