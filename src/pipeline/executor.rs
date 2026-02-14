//! Pipeline executor - single transform execution loop.

use std::io::{self, BufRead, Write};
use tracing::{debug, error, info, warn};

use crate::engine::{pipeline, TransformInstance, WaferEngine};
use crate::error::Result;
use crate::queue::RuntimeEnvelope;

/// Executes a single transform node in a read-process-write loop.
pub struct PipelineExecutor {
    #[allow(dead_code)]
    engine: WaferEngine,
    instance: TransformInstance,
    source_name: String,
}

impl PipelineExecutor {
    /// Create a new pipeline executor.
    pub fn new(
        engine: WaferEngine,
        instance: TransformInstance,
        source_name: impl Into<String>,
    ) -> Self {
        Self {
            engine,
            instance,
            source_name: source_name.into(),
        }
    }

    /// Run the pipeline: read stdin line-by-line, process through WASM, write to stdout.
    pub async fn run(&mut self) -> Result<()> {
        let stdin = io::stdin();
        let mut stdout = io::stdout();

        info!(source = %self.source_name, "Starting pipeline executor");

        for line in stdin.lock().lines() {
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    error!(error = %e, "Error reading from stdin");
                    break;
                }
            };

            let envelope = RuntimeEnvelope::from_string(&self.source_name, &line);
            debug!(id = %envelope.id, "Processing envelope");

            let wit_envelope = runtime_to_wit_envelope(&envelope);

            match self.instance.call_process(&wit_envelope).await {
                Ok(result) => {
                    self.handle_process_result(result, &mut stdout)?;
                }
                Err(e) => {
                    error!(error = %e, "Transform process error");
                }
            }
        }

        info!("Pipeline executor finished (EOF)");
        Ok(())
    }

    fn handle_process_result(
        &self,
        result: pipeline::transform::types::ProcessResult,
        stdout: &mut io::Stdout,
    ) -> Result<()> {
        use pipeline::transform::types::ProcessResult;

        match result {
            ProcessResult::Emit(envelope) => {
                let output = wit_envelope_to_payload(&envelope);
                writeln!(stdout, "{}", String::from_utf8_lossy(&output))?;
                stdout.flush()?;
                debug!(id = %envelope.id, "Emitted envelope");
            }
            ProcessResult::Filter => {
                debug!("Message filtered (dropped)");
            }
            ProcessResult::Error(err) => {
                warn!(
                    code = err.code,
                    message = %err.message,
                    retriable = err.retriable,
                    "Transform returned error"
                );
            }
        }
        Ok(())
    }

    /// Get remaining fuel in the WASM store.
    pub fn remaining_fuel(&self) -> Result<u64> {
        self.instance.remaining_fuel()
    }
}

/// Convert a RuntimeEnvelope to WIT Envelope type.
pub(crate) fn runtime_to_wit_envelope(
    envelope: &RuntimeEnvelope,
) -> pipeline::transform::types::Envelope {
    use pipeline::transform::types::{Envelope, MetadataEntry, Payload};

    let metadata: Vec<MetadataEntry> = envelope
        .metadata
        .iter()
        .map(|(k, v)| MetadataEntry {
            key: k.clone(),
            value: v.clone(),
        })
        .collect();

    Envelope {
        id: envelope.id.clone(),
        timestamp: envelope.timestamp,
        source: envelope.source.clone(),
        metadata,
        payload: Payload::Raw(envelope.payload.clone()),
    }
}

/// Convert a WIT Envelope to RuntimeEnvelope.
#[allow(dead_code)]
pub(crate) fn wit_to_runtime_envelope(
    envelope: &pipeline::transform::types::Envelope,
) -> RuntimeEnvelope {
    use pipeline::transform::types::Payload;

    let metadata = envelope
        .metadata
        .iter()
        .map(|e| (e.key.clone(), e.value.clone()))
        .collect();

    let payload = match &envelope.payload {
        Payload::Raw(bytes) => bytes.clone(),
    };

    RuntimeEnvelope {
        id: envelope.id.clone(),
        timestamp: envelope.timestamp,
        source: envelope.source.clone(),
        metadata,
        payload,
    }
}

fn wit_envelope_to_payload(envelope: &pipeline::transform::types::Envelope) -> Vec<u8> {
    use pipeline::transform::types::Payload;

    match &envelope.payload {
        Payload::Raw(bytes) => bytes.clone(),
    }
}
