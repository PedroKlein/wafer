use wafer_core::engine::{Capabilities, WaferEngine};
use wafer_core::orchestrator::hotswap::prepare_transform_swap_timed;
use wafer_core::runner::HotSwapProgress;

const MNIST_COMPONENT: &[u8] =
    include_bytes!("../../wafer-runtime/tests/fixtures/mnist-inference.component.bin");
const MNIST_MEMORY: usize = 64 * 1024 * 1024;

#[tokio::test]
async fn granted_inference_hot_swap_prepares_real_component() {
    let engine = WaferEngine::new().expect("engine");
    let (progress, _completion) = HotSwapProgress::channel();

    let result = prepare_transform_swap_timed(
        &engine,
        MNIST_COMPONENT,
        "mnist",
        Capabilities::sandbox().inference(true),
        MNIST_MEMORY,
        progress,
    )
    .await;

    if let Err(error) = result {
        panic!("granted inference replacement must prepare: {error}");
    }
}

#[tokio::test]
async fn ungranted_inference_hot_swap_fails_during_preparation() {
    let engine = WaferEngine::new().expect("engine");
    let (progress, _completion) = HotSwapProgress::channel();

    let Err(error) = prepare_transform_swap_timed(
        &engine,
        MNIST_COMPONENT,
        "mnist",
        Capabilities::sandbox(),
        MNIST_MEMORY,
        progress,
    )
    .await
    else {
        panic!("ungranted inference replacement must fail before signaling");
    };

    assert!(error.to_string().contains("wasi:nn/"), "unexpected denial: {error}");
}
