#[cfg(all(feature = "message", feature = "stream"))]
compile_error!("select exactly one guest world");

#[cfg(feature = "message")]
mod message {
    wit_bindgen::generate!({
        path: "../../wit/wafer-pipeline-0.2.0",
        world: "transform-message-node",
    });

    use exports::wafer::pipeline::message_transform::{
        Envelope, Guest as GuestTrait, ProcessError,
    };

    struct Guest;
    export!(Guest);

    fn metadata_value<'a>(input: &'a Envelope, key: &str) -> Option<&'a str> {
        input.metadata.iter().find(|(name, _)| name == key).map(|(_, value)| value.as_str())
    }

    async fn yield_requested(input: &Envelope) {
        let count = metadata_value(input, "p3.yields").and_then(|v| v.parse().ok()).unwrap_or(0);
        for _ in 0..count {
            wit_bindgen::yield_async().await;
        }
    }

    impl GuestTrait for Guest {
        async fn process(mut input: Envelope) -> Result<Envelope, ProcessError> {
            yield_requested(&input).await;
            match metadata_value(&input, "p3.behavior") {
                Some("bad-input") => return Err(ProcessError::BadInput("requested error".into())),
                Some("dependency-failed") => {
                    return Err(ProcessError::DependencyFailed("requested error".into()));
                }
                Some("processing-failed") => {
                    return Err(ProcessError::ProcessingFailed("requested error".into()));
                }
                Some("timed-out") => return Err(ProcessError::TimedOut),
                Some("unrecoverable") => {
                    return Err(ProcessError::Unrecoverable("requested error".into()));
                }
                Some("trap") => panic!("requested P3 trap"),
                _ => {}
            }
            if cfg!(feature = "version-v2") {
                input.metadata.push(("p3.version".into(), "v2".into()));
            }
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
    use wit_bindgen::{FutureReader, StreamReader, StreamResult};

    struct Guest;
    export!(Guest);

    fn metadata_value<'a>(input: &'a Envelope, key: &str) -> Option<&'a str> {
        input.metadata.iter().find(|(name, _)| name == key).map(|(_, value)| value.as_str())
    }

    async fn transform(mut input: Envelope) -> Option<Result<Envelope, ProcessError>> {
        let count = metadata_value(&input, "p3.yields").and_then(|v| v.parse().ok()).unwrap_or(0);
        for _ in 0..count {
            wit_bindgen::yield_async().await;
        }
        match metadata_value(&input, "p3.behavior") {
            Some("early-close") => None,
            Some("bad-input") => Some(Err(ProcessError::BadInput("requested error".into()))),
            Some("dependency-failed") => {
                Some(Err(ProcessError::DependencyFailed("requested error".into())))
            }
            Some("processing-failed") => {
                Some(Err(ProcessError::ProcessingFailed("requested error".into())))
            }
            Some("timed-out") => Some(Err(ProcessError::TimedOut)),
            Some("unrecoverable") => {
                Some(Err(ProcessError::Unrecoverable("requested error".into())))
            }
            Some("trap") => panic!("requested P3 stream trap"),
            _ => {
                if cfg!(feature = "version-v2") {
                    input.metadata.push(("p3.version".into(), "v2".into()));
                }
                Some(Ok(input))
            }
        }
    }

    impl GuestTrait for Guest {
        async fn process(
            mut input: StreamReader<Envelope>,
        ) -> (StreamReader<Result<Envelope, ProcessError>>, FutureReader<Result<(), ProcessError>>)
        {
            let (mut output, output_reader) = wit_stream::new();
            let (completion, completion_reader) = wit_future::new(|| Ok(()));
            wit_bindgen::spawn_local(async move {
                loop {
                    let (status, envelopes) = input.read(Vec::with_capacity(32)).await;
                    let mut transformed = Vec::with_capacity(envelopes.len());
                    for envelope in envelopes {
                        let Some(result) = transform(envelope).await else {
                            drop(output);
                            let _ = completion.write(Ok(())).await;
                            return;
                        };
                        transformed.push(result);
                    }
                    if !transformed.is_empty() && !output.write_all(transformed).await.is_empty() {
                        return;
                    }
                    match status {
                        StreamResult::Complete(_) => {}
                        StreamResult::Dropped => break,
                        StreamResult::Cancelled => return,
                    }
                }
                drop(output);
                let _ = completion.write(Ok(())).await;
            });
            (output_reader, completion_reader)
        }
    }
}
