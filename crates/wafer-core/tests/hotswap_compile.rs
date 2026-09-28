#![cfg(test)]
//! Hot-swap preparation compiles off the runtime's worker threads and says
//! whether the component came from the compile cache.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use wafer_core::engine::{CacheOutcome, Capabilities, WaferEngine};
use wafer_core::orchestrator::hotswap::{TimedSwapResult, prepare_transform_swap_timed};
use wafer_core::runner::HotSwapProgress;

const TRANSFORM_COMPONENT: &[u8] = include_bytes!("fixtures/transform-panics.component.bin");
const MEMORY_LIMIT: usize = 64 * 1024 * 1024;

async fn prepare(engine: &Arc<WaferEngine>) -> TimedSwapResult {
    let (progress, _completion) = HotSwapProgress::channel();
    prepare_transform_swap_timed(
        engine,
        TRANSFORM_COMPONENT,
        "transform",
        Capabilities::sandbox(),
        MEMORY_LIMIT,
        progress,
    )
    .await
    .unwrap_or_else(|e| panic!("prepare swap: {e}"))
}

#[tokio::test(flavor = "current_thread")]
async fn cold_compile_leaves_the_runtime_thread_free() {
    let engine = Arc::new(WaferEngine::new().expect("engine"));
    let ticks = Arc::new(Mutex::new(Vec::<Instant>::new()));
    let ticker = tokio::spawn({
        let ticks = Arc::clone(&ticks);
        async move {
            loop {
                tokio::time::sleep(Duration::from_millis(1)).await;
                ticks.lock().expect("ticks").push(Instant::now());
            }
        }
    });
    tokio::task::yield_now().await;

    let prepared = prepare(&engine).await;
    ticker.abort();

    let timeline = &prepared.timeline;
    assert_eq!(timeline.compile_cache, Some(CacheOutcome::Compiled));
    let compile_done = timeline.compile_done.expect("compile phase recorded");
    let ticks_during_compile = ticks
        .lock()
        .expect("ticks")
        .iter()
        .filter(|&&at| at > timeline.request_time && at < compile_done)
        .count();
    assert!(
        ticks_during_compile > 0,
        "no other task ran during a {:?} compile on the only runtime thread",
        compile_done.duration_since(timeline.request_time)
    );
}

#[tokio::test]
async fn repeated_swap_reports_a_cache_hit() {
    let engine = Arc::new(WaferEngine::new().expect("engine"));

    let first = prepare(&engine).await.timeline;
    let second = prepare(&engine).await.timeline;

    assert_eq!(first.compile_cache, Some(CacheOutcome::Compiled));
    assert_eq!(second.compile_cache, Some(CacheOutcome::MemoryHit));
    let json: serde_json::Value = serde_json::from_str(&second.to_json()).expect("timeline json");
    assert_eq!(json["compile_cache"], "memory_hit");
}
