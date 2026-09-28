//! Source adapter loop — bridges Source trait poll() to downstream channels.
//!
//! Source::poll() is native Rust I/O (mpsc recv, file read, HTTP accept), so
//! there is no Wasm Store to poison when select! cancels it. Cancelling
//! `poll()` may still drop a partially read item; adapters must be cancel-safe
//! or accept that loss at shutdown.
//! See docs/rfcs/RFC-010-io-integration.md Decision 1.

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::error::{Result, WaferError};
use crate::node::Source;
use crate::node::{NodeMetrics, NodeStateTracker};
use crate::runner::{DownstreamSender, send_downstream};

/// Run a source adapter loop until cancellation, EOF, or unrecoverable error.
///
/// # Shutdown semantics
///
/// - `cancel.cancelled()` → break immediately (biased check)
/// - `source.poll()` returns `Ok(None)` → EOF, source exhausted, break
/// - `source.poll()` returns `Err(_)` → treated as transient: log, back off
///   (doubling from [`POLL_ERROR_BACKOFF_MIN`] to [`POLL_ERROR_BACKOFF_MAX`]),
///   and poll again. After [`MAX_CONSECUTIVE_POLL_ERRORS`] errors in a row the
///   source is considered broken: the loop ends and returns `Err`, which fails
///   the run instead of spinning a worker thread.
///
/// On exit, `source.close()` is always called. When the function returns,
/// all `senders` are dropped, which propagates EOF downstream (receivers
/// see `None` on `recv()`).
///
/// # Errors
///
/// Returns the last poll error once the consecutive-error budget is spent.
pub async fn run_source_loop(
    mut source: Box<dyn Source + Send>,
    senders: Vec<DownstreamSender>,
    cancel: CancellationToken,
    state: Arc<NodeStateTracker>,
    metrics: Arc<NodeMetrics>,
) -> Result<()> {
    let mut consecutive_errors: u32 = 0;
    let mut outcome = Ok(());
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            result = source.poll() => {
                match result {
                    Ok(Some(mut envelope)) => {
                        consecutive_errors = 0;
                        envelope.ensure_trace_id();
                        // Sources don't "process" — 0ns duration
                        metrics.record_processed(0);
                        send_downstream(&senders, envelope).await;
                    }
                    Ok(None) => break, // EOF — source exhausted
                    Err(e) => {
                        metrics.record_failed();
                        consecutive_errors = consecutive_errors.saturating_add(1);
                        if consecutive_errors >= MAX_CONSECUTIVE_POLL_ERRORS {
                            tracing::error!(
                                source = source.id(),
                                error = %e,
                                consecutive_errors,
                                "source poll error budget exhausted, stopping source"
                            );
                            state.transition_to_error();
                            outcome = Err(WaferError::Runtime(format!(
                                "source '{}' stopped after {consecutive_errors} consecutive poll errors: {e}",
                                source.id()
                            )));
                            break;
                        }
                        let backoff = poll_error_backoff(consecutive_errors);
                        tracing::warn!(
                            source = source.id(),
                            error = %e,
                            consecutive_errors,
                            backoff_ms = backoff.as_millis(),
                            "source poll error, retrying after backoff"
                        );
                        tokio::select! {
                            biased;
                            () = cancel.cancelled() => break,
                            () = tokio::time::sleep(backoff) => {}
                        }
                    }
                }
            }
        }
    }

    if let Err(e) = source.close().await {
        tracing::warn!(source = source.id(), error = %e, "source close error");
    }
    outcome
}

/// Consecutive `poll()` errors after which a source is treated as broken.
pub const MAX_CONSECUTIVE_POLL_ERRORS: u32 = 20;
/// First retry delay after a `poll()` error.
pub const POLL_ERROR_BACKOFF_MIN: Duration = Duration::from_millis(1);
/// Upper bound on the retry delay between `poll()` errors.
pub const POLL_ERROR_BACKOFF_MAX: Duration = Duration::from_secs(1);

/// Delay before the next poll after `consecutive_errors` errors in a row.
fn poll_error_backoff(consecutive_errors: u32) -> Duration {
    let doublings = consecutive_errors.saturating_sub(1).min(16);
    POLL_ERROR_BACKOFF_MIN.saturating_mul(1 << doublings).min(POLL_ERROR_BACKOFF_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::RuntimeEnvelope;
    use crate::testing::channel::ChannelSource;
    use std::time::Duration;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_source_loop_messages_flow() {
        let (tx, source) = ChannelSource::new("test-src");
        let (out_tx, mut out_rx) = mpsc::channel(32);
        let senders = vec![DownstreamSender::slow(out_tx, "out", None)];
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
            tx.send(RuntimeEnvelope::from_string("s", format!("msg-{i}"))).await.unwrap();
        }
        drop(tx); // Signal EOF

        // Wait for loop to finish
        handle.await.unwrap().unwrap();

        // Collect all received messages
        let mut received = Vec::new();
        while let Ok(env) = out_rx.try_recv() {
            received.push(env);
        }

        assert_eq!(received.len(), 10);
        for (i, env) in received.iter().enumerate() {
            assert_eq!(env.payload_as_string(), format!("msg-{i}"));
            assert!(env.trace_id().is_some(), "source ingress should assign trace_id");
        }
        assert_eq!(metrics.processed(), 10);
        assert_eq!(metrics.attempts_failed(), 0);
    }

    /// A source whose every `poll()` fails immediately without awaiting.
    struct BrokenSource;

    impl crate::node::Lifecycle for BrokenSource {
        fn id(&self) -> &'static str {
            "broken-src"
        }

        fn node_type(&self) -> &'static str {
            "broken-source"
        }

        fn validate(&self) -> Result<()> {
            Ok(())
        }

        fn init(
            &mut self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + '_>> {
            Box::pin(async { Ok(()) })
        }

        fn close(
            &mut self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + '_>> {
            Box::pin(async { Ok(()) })
        }
    }

    impl Source for BrokenSource {
        fn poll(
            &mut self,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Option<RuntimeEnvelope>>> + Send + '_>,
        > {
            Box::pin(async { Err(WaferError::Runtime("EIO".into())) })
        }
    }

    #[tokio::test(start_paused = true)]
    async fn test_source_loop_persistent_errors_back_off_then_fail() {
        let (out_tx, _out_rx) = mpsc::channel(32);
        let senders = vec![DownstreamSender::slow(out_tx, "out", None)];
        let state = Arc::new(NodeStateTracker::running());
        let metrics = Arc::new(NodeMetrics::new());

        let started = tokio::time::Instant::now();
        let result = run_source_loop(
            Box::new(BrokenSource),
            senders,
            CancellationToken::new(),
            Arc::clone(&state),
            metrics.clone(),
        )
        .await;

        let err = result.expect_err("a permanently failing source must fail the run");
        assert!(err.to_string().contains("broken-src"), "{err}");
        assert_eq!(metrics.attempts_failed(), u64::from(MAX_CONSECUTIVE_POLL_ERRORS));
        assert_eq!(state.state(), wafer_types::NodeState::Error);
        assert!(
            started.elapsed() >= POLL_ERROR_BACKOFF_MAX,
            "retries must back off instead of spinning"
        );
    }

    #[test]
    fn poll_error_backoff_doubles_and_caps() {
        assert_eq!(poll_error_backoff(1), POLL_ERROR_BACKOFF_MIN);
        assert_eq!(poll_error_backoff(2), POLL_ERROR_BACKOFF_MIN * 2);
        assert_eq!(poll_error_backoff(u32::MAX), POLL_ERROR_BACKOFF_MAX);
    }

    #[tokio::test]
    async fn test_source_loop_eof_terminates() {
        let (tx, source) = ChannelSource::new("eof-src");
        let (out_tx, _out_rx) = mpsc::channel(32);
        let senders = vec![DownstreamSender::slow(out_tx, "out", None)];
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
        let senders = vec![DownstreamSender::slow(out_tx, "out", None)];
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
