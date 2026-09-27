#[cfg(all(feature = "message", feature = "stream"))]
compile_error!("select exactly one guest world");

#[cfg(feature = "message")]
mod message {
    wit_bindgen::generate!({
        path: "../../wit/wafer-pipeline-0.2.0",
        world: "transform-message-node",
    });

    struct Guest;
    export!(Guest);

    impl exports::wafer::pipeline::message_transform::Guest for Guest {
        async fn process(
            input: exports::wafer::pipeline::message_transform::Envelope,
        ) -> Result<
            exports::wafer::pipeline::message_transform::Envelope,
            exports::wafer::pipeline::message_transform::ProcessError,
        > {
            Ok(input)
        }
    }
}

#[cfg(feature = "stream")]
mod stream {
    wit_bindgen::generate!({
        path: "../../wit/wafer-pipeline-0.2.0",
        world: "transform-stream-node",
    });

    use exports::wafer::pipeline::stream_transform::{Envelope, Guest as GuestTrait, ProcessError};
    use wit_bindgen::rt::async_support::{FutureReader, StreamReader};

    struct Guest;
    export!(Guest);

    impl GuestTrait for Guest {
        async fn process(
            mut input: StreamReader<Envelope>,
        ) -> (StreamReader<Result<Envelope, ProcessError>>, FutureReader<Result<(), ProcessError>>)
        {
            let (mut output, output_reader) = wit_stream::new();
            let (completion, completion_reader) = wit_future::new(|| Ok(()));
            wit_bindgen::spawn_local(async move {
                let (_, mut envelopes) = input.read(Vec::with_capacity(1)).await;
                if let Some(envelope) = envelopes.pop() {
                    let _ = output.write_one(Ok(envelope)).await;
                }
                drop(output);
                let _ = completion.write(Ok(())).await;
            });
            (output_reader, completion_reader)
        }
    }
}
