use std::fmt::Write;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use tokio_util::sync::CancellationToken;

use crate::orchestrator::PipelineHandle;

const SAMPLE_INTERVAL_MS: u64 = 10;
const MAX_SAMPLES: usize = 131_072;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueDepthSample {
    pub elapsed_ns: u64,
    pub queue: Box<str>,
    pub depth: u64,
    pub capacity: usize,
    pub accepted: u64,
    pub dequeued: u64,
    pub processed: u64,
    pub dropped: u64,
    pub dead_lettered: u64,
    pub downstream_closed: u64,
    pub dlq_full: u64,
    pub dlq_closed: u64,
}

pub struct QueueDepthRecorder {
    handle: PipelineHandle,
    samples: Vec<QueueDepthSample>,
    start: Instant,
    start_unix_epoch_ns: u64,
    truncated: bool,
}

impl QueueDepthRecorder {
    #[must_use]
    pub fn new(handle: PipelineHandle) -> Self {
        Self {
            handle,
            samples: Vec::new(),
            start: Instant::now(),
            start_unix_epoch_ns: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, crate::util::duration_ns_saturating),
            truncated: false,
        }
    }

    pub async fn sample_loop(&mut self, cancel: CancellationToken) {
        let mut interval =
            tokio::time::interval(std::time::Duration::from_millis(SAMPLE_INTERVAL_MS));
        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    self.sample();
                    break;
                }
                _ = interval.tick() => self.sample(),
            }
        }
    }

    pub fn sample(&mut self) {
        let elapsed_ns = crate::util::duration_ns_saturating(self.start.elapsed());
        for snapshot in self.handle.queue_snapshots() {
            if self.samples.len() == MAX_SAMPLES {
                self.truncated = true;
                return;
            }
            self.samples.push(QueueDepthSample {
                elapsed_ns,
                queue: snapshot.queue,
                depth: snapshot.depth,
                capacity: snapshot.capacity,
                accepted: snapshot.accepted,
                dequeued: snapshot.dequeued,
                processed: snapshot.processed,
                dropped: snapshot.dropped,
                dead_lettered: snapshot.dead_lettered,
                downstream_closed: snapshot.downstream_closed,
                dlq_full: snapshot.dlq_full,
                dlq_closed: snapshot.dlq_closed,
            });
        }
    }

    #[must_use]
    pub const fn start_unix_epoch_ns(&self) -> u64 {
        self.start_unix_epoch_ns
    }

    #[must_use]
    pub fn samples(&self) -> &[QueueDepthSample] {
        &self.samples
    }

    #[must_use]
    pub const fn truncated(&self) -> bool {
        self.truncated
    }

    #[must_use]
    pub fn to_csv(&self) -> String {
        samples_to_csv(&self.samples)
    }
}

#[expect(clippy::let_underscore_must_use, reason = "writing to String is infallible")]
fn samples_to_csv(samples: &[QueueDepthSample]) -> String {
    let mut output = String::from(
        "elapsed_ns,queue,depth,capacity,accepted,dequeued,processed,dropped,dead_lettered,downstream_closed,dlq_full,dlq_closed\n",
    );
    for sample in samples {
        let _ = writeln!(
            output,
            "{},{},{},{},{},{},{},{},{},{},{},{}",
            sample.elapsed_ns,
            sample.queue,
            sample.depth,
            sample.capacity,
            sample.accepted,
            sample.dequeued,
            sample.processed,
            sample.dropped,
            sample.dead_lettered,
            sample.downstream_closed,
            sample.dlq_full,
            sample.dlq_closed,
        );
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_preserves_units_and_counters() {
        let samples = [QueueDepthSample {
            elapsed_ns: 10,
            queue: "slow".into(),
            depth: 60,
            capacity: 64,
            accepted: 100,
            dequeued: 41,
            processed: 40,
            dropped: 3,
            dead_lettered: 2,
            downstream_closed: 1,
            dlq_full: 4,
            dlq_closed: 5,
        }];

        assert_eq!(
            samples_to_csv(&samples),
            "elapsed_ns,queue,depth,capacity,accepted,dequeued,processed,dropped,dead_lettered,downstream_closed,dlq_full,dlq_closed\n10,slow,60,64,100,41,40,3,2,1,4,5\n"
        );
    }
}
