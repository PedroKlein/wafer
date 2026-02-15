//! Pipeline executor - single transform execution loop.
//!
//! # Design Note: Blocking I/O
//!
//! This executor uses **blocking I/O** (`BufRead::read_line`) rather than async I/O.
//! This is intentional:
//!
//! - **Simplicity**: Blocking I/O avoids async runtime complexity for stdin/stdout
//! - **Performance**: No async overhead for line-by-line processing
//! - **Compatibility**: Works with any `BufRead`/`Write` impl (files, pipes, buffers)
//! - **Testability**: Easy to inject mock readers/writers for unit tests
//!
//! The async `run()` method is async only because `call_process()` requires async
//! for WASM component model calls, not because of I/O.

use std::io::{BufRead, Stdin, Write};
use std::sync::OnceLock;
use tracing::{debug, error, info, warn};

use crate::engine::{pipeline, TransformInstance, WaferEngine};
use crate::error::Result;
use crate::metrics::{PipelineMetrics, ProcessTimer};
use crate::queue::RuntimeEnvelope;

/// Executes a single transform node in a read-process-write loop.
///
/// The executor is generic over input/output streams, making it testable
/// without requiring actual stdin/stdout.
///
/// # Type Parameters
///
/// * `R` - Input reader implementing [`BufRead`]
/// * `W` - Output writer implementing [`Write`]
///
/// # Example
///
/// ```ignore
/// // Production usage with stdin/stdout
/// let executor = PipelineExecutor::new(engine, instance, "source")
///     .with_io(std::io::stdin().lock(), std::io::stdout());
///
/// // Test usage with buffers
/// let input = std::io::Cursor::new(b"test\n");
/// let mut output = Vec::new();
/// let executor = PipelineExecutor::new(engine, instance, "source")
///     .with_io(input, &mut output);
/// ```
pub struct PipelineExecutor<R, W> {
    /// Engine must be kept alive for the instance's lifetime.
    /// The instance holds references to the engine's compiled code.
    #[allow(dead_code)]
    engine: WaferEngine,
    instance: TransformInstance,
    source_name: String,
    metrics: PipelineMetrics,
    reader: R,
    writer: W,
}

/// Builder for constructing PipelineExecutor without I/O (for deferred setup).
pub struct PipelineExecutorCore {
    engine: WaferEngine,
    instance: TransformInstance,
    source_name: String,
    metrics: PipelineMetrics,
}

impl PipelineExecutorCore {
    /// Create a new pipeline executor core without I/O.
    pub fn new(
        engine: WaferEngine,
        instance: TransformInstance,
        source_name: impl Into<String>,
    ) -> Self {
        Self {
            engine,
            instance,
            source_name: source_name.into(),
            metrics: PipelineMetrics::new(),
        }
    }

    /// Attach I/O streams to create a runnable executor.
    pub fn with_io<R: BufRead, W: Write>(self, reader: R, writer: W) -> PipelineExecutor<R, W> {
        PipelineExecutor {
            engine: self.engine,
            instance: self.instance,
            source_name: self.source_name,
            metrics: self.metrics,
            reader,
            writer,
        }
    }

    /// Create executor with default stdin/stdout.
    ///
    /// Note: This locks stdin for the lifetime of the executor.
    /// Uses a global `OnceLock` to ensure stdin is only allocated once
    /// per process, avoiding memory leaks if multiple executors are created.
    pub fn with_stdio(self) -> PipelineExecutor<std::io::StdinLock<'static>, std::io::Stdout> {
        // Use OnceLock to ensure stdin is only allocated once per process.
        // This avoids memory leaks from Box::leak if multiple executors are created.
        static STDIN: OnceLock<Stdin> = OnceLock::new();
        let stdin = STDIN.get_or_init(std::io::stdin);

        PipelineExecutor {
            engine: self.engine,
            instance: self.instance,
            source_name: self.source_name,
            metrics: self.metrics,
            reader: stdin.lock(),
            writer: std::io::stdout(),
        }
    }

    /// Get remaining fuel in the WASM store.
    pub fn remaining_fuel(&self) -> Result<u64> {
        self.instance.remaining_fuel()
    }
}

impl<R: BufRead, W: Write> PipelineExecutor<R, W> {
    /// Run the pipeline: read lines, process through WASM, write output.
    pub async fn run(&mut self) -> Result<()> {
        info!(source = %self.source_name, "Starting pipeline executor");

        let mut line = String::new();
        loop {
            line.clear();
            match self.reader.read_line(&mut line) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    // Remove trailing newline
                    let trimmed = line.trim_end();
                    if trimmed.is_empty() {
                        continue;
                    }

                    let envelope = RuntimeEnvelope::from_string(&self.source_name, trimmed);
                    debug!(id = %envelope.id, "Processing envelope");

                    let wit_envelope = runtime_to_wit_envelope(&envelope);

                    // Time the process() call and get result
                    let process_result = {
                        let _timer = ProcessTimer::start(&self.metrics);
                        self.instance.call_process(&wit_envelope).await
                    };
                    // Timer dropped here, handle result outside the timed block
                    match process_result {
                        Ok(result) => {
                            self.handle_process_result(result)?;
                        }
                        Err(e) => {
                            error!(error = %e, "Transform process error");
                        }
                    }
                }
                Err(e) => {
                    error!(error = %e, "Error reading input");
                    break;
                }
            }
        }

        // Log metrics on shutdown
        let report = self.metrics.report();
        info!(
            messages_total = report.messages_total,
            avg_process_time_ns = report.avg_process_time_ns,
            queue_depth = report.queue_depth,
            "Pipeline metrics"
        );

        info!("Pipeline executor finished (EOF)");
        Ok(())
    }

    fn handle_process_result(
        &mut self,
        result: pipeline::transform::types::ProcessResult,
    ) -> Result<()> {
        use pipeline::transform::types::ProcessResult;

        match result {
            ProcessResult::Emit(envelope) => {
                let output = wit_envelope_to_payload(&envelope);
                writeln!(self.writer, "{}", String::from_utf8_lossy(&output))?;
                self.writer.flush()?;
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

    /// Get the metrics report.
    pub fn metrics(&self) -> &PipelineMetrics {
        &self.metrics
    }
}

// Keep legacy constructor for backward compatibility
impl PipelineExecutorCore {
    /// Legacy constructor - prefer `new().with_io()` for testability.
    #[deprecated(note = "Use PipelineExecutorCore::new().with_stdio() instead")]
    pub fn new_legacy(
        engine: WaferEngine,
        instance: TransformInstance,
        source_name: impl Into<String>,
    ) -> PipelineExecutor<std::io::StdinLock<'static>, std::io::Stdout> {
        Self::new(engine, instance, source_name).with_stdio()
    }
}

/// Convert a RuntimeEnvelope to WIT Envelope type.
pub(crate) fn runtime_to_wit_envelope(
    envelope: &RuntimeEnvelope,
) -> pipeline::transform::types::Envelope {
    use pipeline::transform::types::{Envelope, Payload};

    // Metadata is now list<tuple<string, string>> = Vec<(String, String)>
    let metadata: Vec<(String, String)> = envelope
        .metadata
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
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
///
/// This function is currently unused but will be needed when implementing
/// bidirectional conversion for router/joiner nodes that receive WIT envelopes.
#[allow(dead_code)]
pub(crate) fn wit_to_runtime_envelope(
    envelope: &pipeline::transform::types::Envelope,
) -> RuntimeEnvelope {
    use pipeline::transform::types::Payload;

    // Metadata is now Vec<(String, String)> - already correct format
    let metadata = envelope.metadata.iter().cloned().collect();

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
