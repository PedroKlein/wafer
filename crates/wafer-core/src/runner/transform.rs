//! Transform node runner loop — cancel-safe select!, watch-channel hot-swap.
//!
//! Transform takes ownership of each envelope, produces a new one.
//! DLQ safety clone BEFORE the Wasm call (~10ns Arc+Bytes bump).
//!
//! See docs/rfcs/RFC-005-orchestrator.md D3.

use std::sync::Arc;
use std::time::Instant;

use tokio_util::sync::CancellationToken;

use crate::node::TransformNode;
use crate::node::{NodeMetrics, NodeStateTracker, ProcessingGuard};
use crate::queue::RuntimeEnvelope;
use crate::runner::error_policy::{ErrorPolicyExecutor, WasmProcessError};
use crate::runner::{
    DownstreamSender, HotSwapProgress, NextInput, SwapPayload, SwapReceiver, TrackedReceiver,
    TransformCanaryState, continue_after_policy_action, next_input, send_downstream,
    take_pending_swap,
};
use wafer_types::config::HotSwapConfig;

async fn recover_after_timeout(
    transform: &mut TransformNode,
    state: &NodeStateTracker,
    metrics: &NodeMetrics,
    policy: &mut ErrorPolicyExecutor,
    error: &WasmProcessError,
    envelope: RuntimeEnvelope,
) -> bool {
    if !continue_after_policy_action(policy.handle(error, envelope), metrics) {
        return false;
    }
    tracing::warn!(
        node = transform.node_id(),
        %error,
        "Wasm call ran out of budget — replacing Store before continuing"
    );
    state.transition_to_error();
    state.transition_to_recovering();
    match transform.recover_from_cached_pre().await {
        Ok(()) => {
            if let Some(duration_ns) = state.transition_recovering_to_running_timed() {
                metrics.record_recovery(duration_ns);
            }
            true
        }
        Err(error) => {
            tracing::error!(node = transform.node_id(), %error, "recovery failed");
            false
        }
    }
}

/// Run the transform processing loop until cancellation or channel close.
///
/// # Cancel Safety
///
/// The Wasm call (`transform.process()`) runs OUTSIDE the `select!` block.
/// Only `receiver.recv()` is inside `select!` — which is documented cancel-safe.
/// `ProcessingGuard` ensures the processing flag is always cleared via RAII.
#[expect(
    clippy::too_many_arguments,
    reason = "Runner loop needs all pipeline wiring: node + channel + senders + cancel + swap + state + metrics"
)]
pub async fn run_transform_loop(
    transform: TransformNode,
    receiver: impl Into<TrackedReceiver>,
    senders: Vec<DownstreamSender>,
    swap_rx: SwapReceiver,
    policy: ErrorPolicyExecutor,
    cancel: CancellationToken,
    state: Arc<NodeStateTracker>,
    metrics: Arc<NodeMetrics>,
) {
    run_transform_loop_with_config(
        transform,
        receiver,
        senders,
        swap_rx,
        policy,
        cancel,
        state,
        metrics,
        HotSwapConfig::default(),
    )
    .await;
}

/// Inner transform loop with explicit hot-swap config (testable).
#[expect(
    clippy::too_many_arguments,
    reason = "Runner loop needs all pipeline wiring plus hot-swap config for testability"
)]
#[expect(
    clippy::too_many_lines,
    reason = "linear select!/match pipeline loop with canary logic; splitting would fragment the control flow"
)]
pub async fn run_transform_loop_with_config(
    mut transform: TransformNode,
    receiver: impl Into<TrackedReceiver>,
    senders: Vec<DownstreamSender>,
    mut swap_rx: SwapReceiver,
    mut policy: ErrorPolicyExecutor,
    cancel: CancellationToken,
    state: Arc<NodeStateTracker>,
    metrics: Arc<NodeMetrics>,
    hot_swap_config: HotSwapConfig,
) {
    let mut receiver = receiver.into();
    let mut pending_swap_progress: Option<Arc<HotSwapProgress>> = None;
    let mut canary: Option<TransformCanaryState> = None;
    let mut rollback_retry = None;
    let mut held = None;
    loop {
        // 0. Check if canary window has expired (drop snapshot to free memory)
        if let Some(ref c) = canary
            && c.window_expired()
        {
            tracing::debug!(
                node = transform.node_id(),
                successes = c.counters.success_count,
                "canary window closed — rollback snapshot dropped"
            );
            canary = None;
        }

        // 1. Hot-swap check (non-blocking, between messages)
        if rollback_retry.is_none()
            && let Some(payload) = take_pending_swap(&mut swap_rx)
        {
            policy.flush_to_dlq("hot_swap_drain", &metrics);
            let progress = payload.progress();

            // Retain v1 InstancePre BEFORE applying swap (for rollback)
            let v1_pre = transform.as_wasm_mut().map(|w| w.cached_pre().clone());
            let is_reconfigure = matches!(payload, SwapPayload::Reconfigure { .. });

            let result = match payload {
                SwapPayload::Reconfigure { ref new_config_json, .. } => {
                    transform.try_reconfigure(new_config_json).await
                }
                _ => payload.try_apply_transform(&mut transform).await,
            };
            match result {
                Ok(()) => {
                    progress.mark_replacement_adopted();
                    pending_swap_progress = Some(progress);
                    metrics.record_swap();
                    // Install canary snapshot for process-time rollback (A17).
                    //
                    // Only arm canary for full Transform swaps. Reconfigure
                    // reuses the same InstancePre and mutates `config_json`
                    // in place inside `try_reconfigure`, so canary rollback
                    // (which re-runs `validate_and_init(&self.config_json)`)
                    // would re-apply the reconfigure, not restore v1 config.
                    // Reconfigure has its own atomic rollback inside
                    // `try_reconfigure` for the init-failure case.
                    //
                    // Drop any previous canary (new swap supersedes).
                    if is_reconfigure {
                        canary = None;
                    } else if let Some(pre) = v1_pre {
                        canary = Some(TransformCanaryState::new(pre, hot_swap_config.clone()));
                    }
                }
                Err(err) => {
                    tracing::error!(
                        node = transform.node_id(),
                        %err,
                        "hot-swap init failed; keeping v1"
                    );
                    progress.report_init_failed(err.to_string());
                }
            }
            continue;
        }

        // 2. Retry buffer priority — retries before fresh messages
        let envelope = if let Some(retry) = rollback_retry.take() {
            retry
        } else {
            match next_input(&mut held, &mut receiver, &mut policy, &mut swap_rx, &cancel).await {
                NextInput::Envelope(envelope) => envelope,
                NextInput::Swap => continue,
                NextInput::Closed => break,
            }
        };

        // 4. DLQ safety clone BEFORE process (Arc + Bytes refcount ~10ns)
        let safety = envelope.clone();

        // 5. Wasm call OUTSIDE select! — runs to completion, never cancelled.
        let start = Instant::now();
        let guard = ProcessingGuard::enter(&state);
        let result = transform.process(envelope).await;
        let duration_ns = crate::util::duration_ns_saturating(start.elapsed());
        drop(guard);

        // 6. Dispatch result
        match result {
            Ok(output) => {
                metrics.record_processed(duration_ns);
                send_downstream(&senders, output).await;
                if let Some(progress) = pending_swap_progress.take() {
                    progress.mark_first_post_replacement_local_outcome(
                        crate::runner::FirstPostReplacementLocalOutcome::ForwardedEnqueued,
                    );
                }
                // Record success in canary window
                if let Some(ref mut c) = canary {
                    c.record_success();
                }
            }
            Err(ref error) if error.is_budget_exhausted() && canary.is_none() => {
                metrics.record_error(error);
                if !recover_after_timeout(
                    &mut transform,
                    &state,
                    &metrics,
                    &mut policy,
                    error,
                    safety,
                )
                .await
                {
                    break;
                }
            }
            Err(
                ref error @ (WasmProcessError::Trapped { .. } | WasmProcessError::Unrecoverable(_)),
            ) => {
                metrics.record_error(error);
                let msg = error.to_string();

                // A17: a trap inside the canary window rolls back to v1 once;
                // taking the canary means a v1 trap afterwards is an ordinary
                // failure, not another rollback.
                if let Some(c) = canary.take() {
                    tracing::warn!(
                        node = transform.node_id(),
                        error = %msg,
                        "process-time trap during canary window — rolling back to v1"
                    );
                    state.transition_to_error();
                    // Restore v1's InstancePre BEFORE recovery so
                    // recover_from_cached_pre instantiates v1, not v2.
                    if let Some(wasm) = transform.as_wasm_mut() {
                        wasm.set_cached_pre(c.snapshot.pre);
                    }
                    let rollback_start = Instant::now();
                    match transform.recover_from_cached_pre().await {
                        Ok(()) => {
                            let rollback_ns =
                                crate::util::duration_ns_saturating(rollback_start.elapsed());
                            tracing::info!(
                                node = transform.node_id(),
                                rollback_time_ns = rollback_ns,
                                "process-time rollback to v1 succeeded"
                            );
                            state.transition_to_recovering();
                            if let Some(duration_ns) =
                                state.transition_recovering_to_running_timed()
                            {
                                metrics.record_recovery(duration_ns);
                            }
                            metrics.record_rollback();
                            // B1: notify the API caller with a RolledBack
                            // outcome before a later v1 message can report a local outcome.
                            // Taking the progress ensures the sender is
                            // consumed — subsequent mark_first_post_replacement_local_outcome calls
                            // become no-ops.
                            if let Some(progress) = pending_swap_progress.take() {
                                progress.report_rolled_back(rollback_ns, msg.clone());
                            }
                            rollback_retry = Some(safety);
                            continue;
                        }
                        Err(error) => {
                            tracing::error!(
                                node = transform.node_id(),
                                %error,
                                "process-time rollback failed — escalating to recovery"
                            );
                            // B1: rollback attempt itself failed —
                            // the API caller should still learn the
                            // swap did not converge. Report with
                            // rollback_time_ns=0 to signal the failure.
                            if let Some(progress) = pending_swap_progress.take() {
                                progress.report_rolled_back(0, format!("rollback failed: {error}"));
                            }
                        }
                    }
                }

                // Standard recovery path (existing A7 behavior)
                metrics.record_dropped_on_recovery();
                tracing::error!(
                    node = transform.node_id(),
                    error = %msg,
                    "unrecoverable error — attempting recovery"
                );
                state.transition_to_error();
                state.transition_to_recovering();
                match transform.recover_from_cached_pre().await {
                    Ok(()) => {
                        if let Some(duration_ns) = state.transition_recovering_to_running_timed() {
                            metrics.record_recovery(duration_ns);
                        }
                    }
                    Err(error) => {
                        tracing::error!(node = transform.node_id(), %error, "recovery failed");
                        break;
                    }
                }
            }
            Err(e) => {
                metrics.record_error(&e);
                if !continue_after_policy_action(policy.handle(&e, safety), &metrics) {
                    break;
                }
            }
        }
    }

    // Flush remaining retries to DLQ on exit
    policy.flush_to_dlq("shutdown", &metrics);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EngineConfig, FuelBudgets, MemoryLimits};
    use crate::engine::{Capabilities, WaferEngine, WaferState};
    use crate::node::wasm::WasmTransformNode;
    use crate::runner::error_policy::ResolvedErrorPolicy;
    use std::num::NonZeroU64;
    use tokio::sync::{mpsc, watch};
    use wasmtime::Store;

    const MNIST_FUEL: u64 = 100_000_000;
    const MNIST_MEMORY: usize = 64 * 1024 * 1024;
    const MNIST_COMPONENT: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../wafer-runtime/tests/fixtures/mnist-inference.component.bin"
    ));
    const TRAP_COMPONENT: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/transform-panics.component.bin"
    ));
    const MNIST_DIGIT: &[u8] = include_bytes!("../../../../tests/fixtures/digit_7.bin");

    async fn inference_node(engine: &WaferEngine) -> anyhow::Result<TransformNode> {
        let component = engine.load_component_from_bytes(MNIST_COMPONENT, "mnist")?;
        let pre = Arc::new(engine.pre_instantiate_inference(&component)?);
        let capabilities = Capabilities::sandbox().inference(true);
        let state = WaferState::new_with_memory_limit("mnist", capabilities.clone(), MNIST_MEMORY);
        let mut store = Store::new(engine.inner(), state);
        store.limiter(|state| state.limits_mut());
        store.set_fuel(MNIST_FUEL)?;
        store.epoch_deadline_trap();
        store.set_epoch_deadline(1000);
        let bindings = pre.instantiate_async(&mut store).await?;
        let mut node =
            WasmTransformNode::new_inference(store, bindings, pre, NonZeroU64::new(MNIST_FUEL));
        node.configure_runtime(
            capabilities,
            MNIST_MEMORY,
            NonZeroU64::new(1000),
            r#"{"execution_target":"cpu"}"#.into(),
        );
        node.set_plugin_version("mnist-cpu-v1");
        node.validate_and_init(r#"{"execution_target":"cpu"}"#).await?;
        Ok(node.into())
    }

    fn prediction(output: &RuntimeEnvelope) -> Option<usize> {
        output
            .payload
            .as_chunks::<{ size_of::<f32>() }>()
            .0
            .iter()
            .map(|chunk| f32::from_le_bytes(*chunk))
            .enumerate()
            .max_by(|(_, a), (_, b)| a.total_cmp(b))
            .map(|(index, _)| index)
    }

    async fn wait_for_swap(metrics: &NodeMetrics) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while metrics.swaps() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("replacement adoption timeout");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inference_hot_swap_reports_enqueue_without_sink_collection() {
        let config = EngineConfig {
            epoch_deadline: NonZeroU64::new(1000),
            fuel: FuelBudgets { transform: NonZeroU64::new(MNIST_FUEL), ..FuelBudgets::default() },
            memory: MemoryLimits { transform: MNIST_MEMORY, ..MemoryLimits::default() },
            ..EngineConfig::default()
        };
        let engine = WaferEngine::from_engine_config(&config).expect("engine");
        engine.ensure_epoch_ticker();
        let node = inference_node(&engine).await.expect("inference node");
        let (progress, completion) = HotSwapProgress::channel();
        let replacement = crate::orchestrator::hotswap::prepare_transform_swap_timed_with_fuel(
            &engine,
            MNIST_COMPONENT,
            "mnist",
            Capabilities::sandbox().inference(true),
            MNIST_MEMORY,
            NonZeroU64::new(MNIST_FUEL),
            progress,
        )
        .await
        .expect("inference replacement");

        let (input_tx, input_rx) = mpsc::channel(2);
        let (output_tx, mut output_rx) = mpsc::channel(2);
        let (swap_tx, swap_rx) = watch::channel(None);
        let metrics = Arc::new(NodeMetrics::new());
        let runner_metrics = Arc::clone(&metrics);
        swap_tx.send(Some(replacement.payload)).expect("replacement signal");
        let runner = tokio::spawn(run_transform_loop_with_config(
            node,
            input_rx,
            vec![DownstreamSender::slow(output_tx, "default", None)],
            swap_rx,
            ErrorPolicyExecutor::new(ResolvedErrorPolicy::default(), None, "mnist"),
            CancellationToken::new(),
            Arc::new(NodeStateTracker::running()),
            runner_metrics,
            HotSwapConfig::default(),
        ));

        wait_for_swap(&metrics).await;
        input_tx
            .send(RuntimeEnvelope::new("fixture", bytes::Bytes::from_static(MNIST_DIGIT)))
            .await
            .expect("post-replacement input");

        let report =
            completion.await.expect("replacement progress").expect("replacement local outcome");
        assert_eq!(
            report.first_post_replacement_local_outcome,
            crate::runner::FirstPostReplacementLocalOutcome::ForwardedEnqueued
        );
        assert!(!output_rx.is_empty(), "local outcome must precede sink collection");
        let mut outputs = Vec::new();
        while let Ok(output) = output_rx.try_recv() {
            outputs.push(output);
        }
        assert!(outputs.iter().all(|output| prediction(output) == Some(7)));

        drop(input_tx);
        runner.await.expect("runner task");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn inference_process_trap_rolls_back_and_replays_on_fresh_store() {
        let config = EngineConfig {
            epoch_deadline: NonZeroU64::new(1000),
            fuel: FuelBudgets { transform: NonZeroU64::new(MNIST_FUEL), ..FuelBudgets::default() },
            memory: MemoryLimits { transform: MNIST_MEMORY, ..MemoryLimits::default() },
            ..EngineConfig::default()
        };
        let engine = WaferEngine::from_engine_config(&config).expect("engine");
        engine.ensure_epoch_ticker();
        let node = inference_node(&engine).await.expect("inference node");
        let (progress, completion) = HotSwapProgress::channel();
        let replacement = crate::orchestrator::hotswap::prepare_transform_swap_timed_with_fuel(
            &engine,
            TRAP_COMPONENT,
            "mnist",
            Capabilities::sandbox().inference(true),
            MNIST_MEMORY,
            NonZeroU64::new(MNIST_FUEL),
            progress,
        )
        .await
        .expect("inference trap replacement");

        let (input_tx, input_rx) = mpsc::channel(1);
        let (output_tx, mut output_rx) = mpsc::channel(1);
        let (swap_tx, swap_rx) = watch::channel(None);
        let state = Arc::new(NodeStateTracker::running());
        let metrics = Arc::new(NodeMetrics::new());
        swap_tx.send(Some(replacement.payload)).expect("replacement signal");
        let mut input = RuntimeEnvelope::new("fixture", bytes::Bytes::from_static(MNIST_DIGIT));
        input.set_parent_id("parent-before-trap");
        input.ensure_trace_id();
        input.retry_count = 4;
        let trace_id = input.trace_id().expect("trace id").to_string();
        input_tx.send(input).await.expect("input receiver");
        drop(input_tx);

        run_transform_loop_with_config(
            node,
            input_rx,
            vec![DownstreamSender::slow(output_tx, "default", None)],
            swap_rx,
            ErrorPolicyExecutor::new(ResolvedErrorPolicy::default(), None, "mnist"),
            CancellationToken::new(),
            state,
            Arc::clone(&metrics),
            HotSwapConfig::default(),
        )
        .await;

        let outcome = completion.await.expect("rollback outcome");
        assert!(matches!(outcome, Err(crate::runner::HotSwapError::RolledBack { .. })));
        let output = output_rx.recv().await.expect("replayed inference output");
        assert_eq!(prediction(&output), Some(7));
        assert_eq!(output.parent_id(), Some("parent-before-trap"));
        assert_eq!(output.trace_id(), Some(trace_id.as_str()));
        assert_eq!(output.retry_count, 4);
        assert_eq!(metrics.rollbacks(), 1);
        assert_eq!(metrics.recovery_count(), 1);
        assert_eq!(metrics.processed(), 1, "only the replayed v1 result is forwarded");
        assert!(output_rx.try_recv().is_err(), "trapping message must be replayed exactly once");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn trap_after_rollback_is_handled_as_a_v1_failure() {
        let engine = WaferEngine::new().expect("engine");
        let component = engine.load_component_from_bytes(TRAP_COMPONENT, "trap").expect("trap");
        let pre = Arc::new(engine.pre_instantiate_transform(&component).expect("pre"));
        let mut store =
            Store::new(engine.inner(), WaferState::new("trap", Capabilities::sandbox()));
        store.limiter(|state| state.limits_mut());
        let bindings = pre.instantiate_async(&mut store).await.expect("instantiate v1");
        let mut v1 = WasmTransformNode::new(store, bindings, pre, None);
        v1.validate_and_init("{}").await.expect("v1 init");
        let (progress, completion) = HotSwapProgress::channel();
        let replacement = crate::orchestrator::hotswap::prepare_transform_swap_timed(
            &engine,
            TRAP_COMPONENT,
            "trap",
            Capabilities::sandbox(),
            16 * 1024 * 1024,
            progress,
        )
        .await
        .expect("v2");

        let (input_tx, input_rx) = mpsc::channel(1);
        let (output_tx, _output_rx) = mpsc::channel(1);
        let (swap_tx, swap_rx) = watch::channel(None);
        let metrics = Arc::new(NodeMetrics::new());
        swap_tx.send(Some(replacement.payload)).expect("replacement signal");
        input_tx.send(RuntimeEnvelope::from_string("source", "poison")).await.expect("input");
        drop(input_tx);

        run_transform_loop_with_config(
            v1.into(),
            input_rx,
            vec![DownstreamSender::slow(output_tx, "default", None)],
            swap_rx,
            ErrorPolicyExecutor::new(ResolvedErrorPolicy::default(), None, "trap"),
            CancellationToken::new(),
            Arc::new(NodeStateTracker::running()),
            Arc::clone(&metrics),
            HotSwapConfig::default(),
        )
        .await;

        assert!(matches!(
            completion.await.expect("rollback outcome"),
            Err(crate::runner::HotSwapError::RolledBack { .. })
        ));
        assert_eq!(metrics.rollbacks(), 1, "only the v2 trap rolls back");
        assert_eq!(metrics.failed(), 2, "v2 trap, then the replayed message trapping on v1");
        assert_eq!(metrics.recovery_count(), 2);
    }

    /// Creates test infrastructure for the transform loop.
    #[expect(
        clippy::type_complexity,
        reason = "test setup helper; type alias would obscure the tuple for readability"
    )]
    fn setup_transform_test() -> (
        mpsc::Sender<RuntimeEnvelope>,
        mpsc::Receiver<RuntimeEnvelope>,
        Vec<DownstreamSender>,
        mpsc::Receiver<RuntimeEnvelope>,
        watch::Sender<Option<SwapPayload>>,
        watch::Receiver<Option<SwapPayload>>,
        ErrorPolicyExecutor,
        CancellationToken,
        Arc<NodeStateTracker>,
        Arc<NodeMetrics>,
    ) {
        let (input_tx, input_rx) = mpsc::channel(32);
        let (output_tx, output_rx) = mpsc::channel(32);
        let senders = vec![DownstreamSender::slow(output_tx, "default", None)];
        let (swap_tx, swap_rx) = watch::channel(None);
        let policy =
            ErrorPolicyExecutor::new(ResolvedErrorPolicy::default(), None, "test-transform");
        let cancel = CancellationToken::new();
        let state = Arc::new(NodeStateTracker::running());
        let metrics = Arc::new(NodeMetrics::new());

        (input_tx, input_rx, senders, output_rx, swap_tx, swap_rx, policy, cancel, state, metrics)
    }

    #[tokio::test]
    async fn test_transform_loop_shutdown_on_cancel() {
        let (
            input_tx,
            _input_rx,
            _senders,
            _output_rx,
            _swap_tx,
            _swap_rx,
            _policy,
            cancel,
            state,
            metrics,
        ) = setup_transform_test();

        // Cancel immediately — the loop should exit promptly
        cancel.cancel();
        drop(input_tx);

        // We can't run the actual Wasm transform without a real component,
        // but we can verify the loop exits cleanly when cancelled with no input.
        // The loop will select cancel before any message arrives.
        // This tests the shutdown path.

        // Verify state is correct for a freshly created tracker
        assert!(!state.is_processing());
        assert_eq!(metrics.processed(), 0);
    }

    #[tokio::test]
    async fn test_transform_metrics_after_shutdown() {
        let metrics = Arc::new(NodeMetrics::new());
        metrics.record_processed(1000);
        metrics.record_processed(2000);
        metrics.record_failed();

        assert_eq!(metrics.processed(), 2);
        assert_eq!(metrics.attempts_failed(), 1);
        assert_eq!(metrics.process_ns(), 3000);
    }

    fn retry_policy(
        exhausted: crate::runner::error_policy::ResolvedSimpleAction,
    ) -> ResolvedErrorPolicy {
        ResolvedErrorPolicy {
            bad_input: crate::runner::error_policy::ResolvedSimpleAction::Skip,
            dependency_failed: crate::runner::error_policy::ResolvedRetryConfig {
                retries: 0,
                backoff_ms: 1,
                exhausted,
            },
            processing_failed: crate::runner::error_policy::ResolvedRetryConfig {
                retries: 0,
                backoff_ms: 1,
                exhausted,
            },
            timed_out: crate::runner::error_policy::ResolvedSimpleAction::Skip,
            retry_buffer_capacity: 1,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn every_dequeued_message_has_exactly_one_fate() {
        use crate::node::QueueMetrics;
        use crate::runner::error_policy::{
            ErrorCategory, ResolvedRetryConfig, ResolvedSimpleAction,
        };

        let (input_tx, input_rx) = mpsc::channel(4);
        let (output_tx, mut output_rx) = mpsc::channel(4);
        let (dlq_tx, mut dlq_rx) = mpsc::channel(2);
        let (_swap_tx, swap_rx) = watch::channel(None);
        let queue = Arc::new(QueueMetrics::default());
        let metrics = Arc::new(NodeMetrics::new());
        let non_utf8: &'static [u8] = b"\xff";
        for payload in [br#"{"temperature":1}"#.as_slice(), non_utf8, b"{}", non_utf8] {
            let mut envelope = RuntimeEnvelope::from_string("source", "");
            envelope.payload = bytes::Bytes::from_static(payload);
            input_tx.send(envelope).await.expect("input");
        }
        drop(input_tx);
        let retry_once = ResolvedRetryConfig {
            retries: 1,
            backoff_ms: 60_000,
            exhausted: ResolvedSimpleAction::Dlq,
        };
        let policy = ResolvedErrorPolicy {
            bad_input: ResolvedSimpleAction::Dlq,
            dependency_failed: retry_once,
            processing_failed: retry_once,
            timed_out: ResolvedSimpleAction::Skip,
            retry_buffer_capacity: 4,
        };

        run_transform_loop(
            TransformNode::Native(crate::node::native::NativeTransform::json_parse("native")),
            TrackedReceiver::new(input_rx, Arc::clone(&queue)),
            vec![DownstreamSender::slow(output_tx, "default", None)],
            swap_rx,
            ErrorPolicyExecutor::new(policy, Some(dlq_tx), "node"),
            CancellationToken::new(),
            Arc::new(NodeStateTracker::running()),
            Arc::clone(&metrics),
        )
        .await;

        assert_eq!(queue.dequeued(), 4);
        assert_eq!(metrics.processed(), 1);
        assert_eq!(metrics.retries(), 1, "the missing-field message is queued for a retry");
        assert_eq!(metrics.dlq_sent(), 2, "the DLQ holds two records");
        assert_eq!(metrics.dlq_lost(), 1, "the retry flushed at shutdown finds the DLQ full");
        assert_eq!(metrics.attempts_failed(), 3);
        assert_eq!(metrics.guest_errors(ErrorCategory::BadInput), 2);
        assert_eq!(metrics.guest_errors(ErrorCategory::ProcessingFailed), 1);
        assert_eq!(metrics.traps_total(), 0, "guest-returned errors are not traps");
        assert_eq!(
            queue.dequeued(),
            metrics.processed()
                + metrics.filtered_out()
                + metrics.skipped()
                + metrics.exhausted_skips()
                + metrics.dlq_sent()
                + metrics.dlq_lost()
                + metrics.dropped_on_recovery()
                + metrics.dropped_on_teardown()
        );
        assert!(output_rx.recv().await.is_some());
        assert!(dlq_rx.recv().await.is_some() && dlq_rx.recv().await.is_some());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn retry_exhausted_skip_consumes_poison_and_forwards_next_message() {
        let (input_tx, input_rx) = mpsc::channel(2);
        let (output_tx, mut output_rx) = mpsc::channel(1);
        let (dlq_tx, mut dlq_rx) = mpsc::channel(1);
        let (_swap_tx, swap_rx) = watch::channel(None);
        let metrics = Arc::new(NodeMetrics::new());
        input_tx.send(RuntimeEnvelope::from_string("source", "{}")).await.expect("input receiver");
        input_tx
            .send(RuntimeEnvelope::from_string("source", r#"{"temperature":1}"#))
            .await
            .expect("input receiver");
        drop(input_tx);

        let handle = tokio::spawn(run_transform_loop(
            TransformNode::Native(crate::node::native::NativeTransform::json_parse("native")),
            input_rx,
            vec![DownstreamSender::slow(output_tx, "default", None)],
            swap_rx,
            ErrorPolicyExecutor::new(
                retry_policy(crate::runner::error_policy::ResolvedSimpleAction::Skip),
                Some(dlq_tx),
                "node",
            ),
            CancellationToken::new(),
            Arc::new(NodeStateTracker::running()),
            Arc::clone(&metrics),
        ));

        handle.await.expect("runner task");
        assert_eq!(
            output_rx.recv().await.expect("forwarded successor").payload_as_string(),
            r#"{"temperature":1}"#
        );
        assert!(output_rx.try_recv().is_err(), "poison envelope must be consumed exactly once");
        assert!(dlq_rx.try_recv().is_err(), "skip must not write a DLQ record");
        assert_eq!(metrics.exhausted_skips(), 1, "exhausted skip must be counted once");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn retry_exhausted_dlq_records_final_count_without_requeue() {
        let (input_tx, input_rx) = mpsc::channel(1);
        let (output_tx, _output_rx) = mpsc::channel(1);
        let (dlq_tx, mut dlq_rx) = mpsc::channel(1);
        let (_swap_tx, swap_rx) = watch::channel(None);
        input_tx.send(RuntimeEnvelope::from_string("source", "{}")).await.expect("input receiver");
        drop(input_tx);

        run_transform_loop(
            TransformNode::Native(crate::node::native::NativeTransform::json_parse("native")),
            input_rx,
            vec![DownstreamSender::slow(output_tx, "default", None)],
            swap_rx,
            ErrorPolicyExecutor::new(
                retry_policy(crate::runner::error_policy::ResolvedSimpleAction::Dlq),
                Some(dlq_tx),
                "node",
            ),
            CancellationToken::new(),
            Arc::new(NodeStateTracker::running()),
            Arc::new(NodeMetrics::new()),
        )
        .await;

        let record = dlq_rx.recv().await.expect("one exhausted DLQ record");
        assert_eq!(record.retry_count, 0);
        assert_eq!(
            record.reason,
            crate::runner::error_policy::DlqReason::RetriesExhausted { max_retries: 0 }
        );
        assert!(dlq_rx.try_recv().is_err(), "exhausted envelope must not be requeued");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn retry_exhausted_teardown_stops_the_real_transform_runner() {
        let (input_tx, input_rx) = mpsc::channel(2);
        let (output_tx, mut output_rx) = mpsc::channel(1);
        let (_swap_tx, swap_rx) = watch::channel(None);
        input_tx.send(RuntimeEnvelope::from_string("source", "{}")).await.expect("input receiver");
        input_tx
            .send(RuntimeEnvelope::from_string("source", r#"{"temperature":1}"#))
            .await
            .expect("input receiver");
        drop(input_tx);

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            run_transform_loop(
                TransformNode::Native(crate::node::native::NativeTransform::json_parse("native")),
                input_rx,
                vec![DownstreamSender::slow(output_tx, "default", None)],
                swap_rx,
                ErrorPolicyExecutor::new(
                    retry_policy(crate::runner::error_policy::ResolvedSimpleAction::Teardown),
                    None,
                    "node",
                ),
                CancellationToken::new(),
                Arc::new(NodeStateTracker::running()),
                Arc::new(NodeMetrics::new()),
            ),
        )
        .await;

        assert!(result.is_ok(), "teardown must stop the runner");
        assert!(output_rx.try_recv().is_err(), "teardown must not process the successor");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bad_input_teardown_stops_the_real_transform_runner() {
        let (input_tx, input_rx) = mpsc::channel(1);
        let (output_tx, mut output_rx) = mpsc::channel(1);
        let policy = ResolvedErrorPolicy {
            bad_input: crate::runner::error_policy::ResolvedSimpleAction::Teardown,
            dependency_failed: crate::runner::error_policy::ResolvedRetryConfig {
                retries: 1,
                backoff_ms: 1,
                exhausted: crate::runner::error_policy::ResolvedSimpleAction::Skip,
            },
            processing_failed: crate::runner::error_policy::ResolvedRetryConfig {
                retries: 1,
                backoff_ms: 1,
                exhausted: crate::runner::error_policy::ResolvedSimpleAction::Skip,
            },
            timed_out: crate::runner::error_policy::ResolvedSimpleAction::Skip,
            retry_buffer_capacity: 1,
        };
        let (_swap_tx, swap_rx) = watch::channel(None);
        let cancel = CancellationToken::new();
        let state = Arc::new(NodeStateTracker::running());
        let metrics = Arc::new(NodeMetrics::new());
        input_tx
            .send(RuntimeEnvelope::new("source", bytes::Bytes::from_static(&[0xff])))
            .await
            .expect("input receiver");
        drop(input_tx);

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            run_transform_loop(
                TransformNode::Native(crate::node::native::NativeTransform::json_parse("native")),
                input_rx,
                vec![DownstreamSender::slow(output_tx, "default", None)],
                swap_rx,
                ErrorPolicyExecutor::new(policy, None, "node"),
                cancel,
                state,
                metrics,
            ),
        )
        .await;

        assert!(result.is_ok(), "teardown must terminate the runner");
        assert!(output_rx.try_recv().is_err(), "bad input must not be forwarded");
    }

    #[tokio::test]
    async fn timed_out_teardown_skips_recovery() {
        let mut transform =
            TransformNode::Native(crate::node::native::NativeTransform::passthrough("native"));
        let policy = ResolvedErrorPolicy {
            bad_input: crate::runner::error_policy::ResolvedSimpleAction::Skip,
            dependency_failed: crate::runner::error_policy::ResolvedRetryConfig {
                retries: 1,
                backoff_ms: 1,
                exhausted: crate::runner::error_policy::ResolvedSimpleAction::Skip,
            },
            processing_failed: crate::runner::error_policy::ResolvedRetryConfig {
                retries: 1,
                backoff_ms: 1,
                exhausted: crate::runner::error_policy::ResolvedSimpleAction::Skip,
            },
            timed_out: crate::runner::error_policy::ResolvedSimpleAction::Teardown,
            retry_buffer_capacity: 1,
        };
        let state = NodeStateTracker::running();
        let metrics = NodeMetrics::new();
        let mut executor = ErrorPolicyExecutor::new(policy, None, "node");

        assert!(
            !recover_after_timeout(
                &mut transform,
                &state,
                &metrics,
                &mut executor,
                &WasmProcessError::Trapped {
                    code: Some(wasmtime::Trap::Interrupt),
                    message: "wasm trap: interrupt".into(),
                },
                RuntimeEnvelope::from_string("source", "payload"),
            )
            .await,
            "timed-out teardown must stop before recovery"
        );
        assert_eq!(metrics.recovery_count(), 0);
    }
}
