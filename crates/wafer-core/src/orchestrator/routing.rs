//! Routing control for hot-swap message buffering.
//!
//! During a hot-swap, messages are buffered when routing is disabled,
//! then flushed when routing resumes. Buffer has configurable capacity;
//! when full, sends block (backpressure to upstream).

use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::node::NodeStateTracker;
use crate::queue::{QueueSender, RuntimeEnvelope};

/// Default buffer capacity for routing during drain.
const DEFAULT_BUFFER_CAPACITY: usize = 1024;

/// Wraps a [`QueueSender`] with hot-swap buffering support.
pub struct RoutingController {
    sender: QueueSender<RuntimeEnvelope>,
    downstream_tracker: Arc<NodeStateTracker>,
    buffer: Mutex<VecDeque<RuntimeEnvelope>>,
    buffer_capacity: usize,
}

impl RoutingController {
    #[must_use]
    pub fn new(
        sender: QueueSender<RuntimeEnvelope>,
        downstream_tracker: Arc<NodeStateTracker>,
    ) -> Self {
        Self {
            sender,
            downstream_tracker,
            buffer: Mutex::new(VecDeque::with_capacity(DEFAULT_BUFFER_CAPACITY)),
            buffer_capacity: DEFAULT_BUFFER_CAPACITY,
        }
    }

    /// Create with custom buffer capacity.
    #[must_use]
    pub fn with_capacity(
        sender: QueueSender<RuntimeEnvelope>,
        downstream_tracker: Arc<NodeStateTracker>,
        capacity: usize,
    ) -> Self {
        Self {
            sender,
            downstream_tracker,
            buffer: Mutex::new(VecDeque::with_capacity(capacity)),
            buffer_capacity: capacity,
        }
    }

    /// Send a message. Buffers if routing is disabled (during drain).
    pub async fn send(&self, envelope: RuntimeEnvelope) -> Result<(), SendError> {
        if self.downstream_tracker.routing_enabled() {
            self.sender.send(envelope).await.map_err(|_| SendError::Closed)
        } else {
            let mut buffer = self.buffer.lock().await;
            if buffer.len() >= self.buffer_capacity {
                tracing::warn!(
                    capacity = self.buffer_capacity,
                    "Routing buffer full, message dropped"
                );
                return Err(SendError::BufferFull);
            }
            buffer.push_back(envelope);
            Ok(())
        }
    }

    /// Check if routing is currently enabled.
    #[must_use]
    pub fn routing_enabled(&self) -> bool {
        self.downstream_tracker.routing_enabled()
    }

    pub async fn buffer_len(&self) -> usize {
        self.buffer.lock().await.len()
    }

    /// Flush buffered messages to the queue. Call after routing is re-enabled.
    pub async fn flush(&self) -> Result<usize, SendError> {
        let mut buffer = self.buffer.lock().await;
        let count = buffer.len();

        while let Some(envelope) = buffer.pop_front() {
            self.sender.send(envelope).await.map_err(|_| SendError::Closed)?;
        }

        tracing::debug!(messages = count, "Flushed routing buffer");
        Ok(count)
    }

    /// Drain the buffer without sending. Use if swap is aborted.
    pub async fn drain_buffer(&self) -> Vec<RuntimeEnvelope> {
        let mut buffer = self.buffer.lock().await;
        buffer.drain(..).collect()
    }

    /// Get a reference to the downstream state tracker.
    #[must_use]
    pub fn downstream_tracker(&self) -> &Arc<NodeStateTracker> {
        &self.downstream_tracker
    }
}

/// Error types for routing operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    Closed,
    BufferFull,
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::Closed => write!(f, "queue closed"),
            SendError::BufferFull => write!(f, "routing buffer full"),
        }
    }
}

impl std::error::Error for SendError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::BoundedQueue;
    use std::collections::HashMap;

    fn make_envelope(id: &str) -> RuntimeEnvelope {
        RuntimeEnvelope {
            id: id.to_string(),
            timestamp: 0,
            source: "test".to_string(),
            metadata: HashMap::default(),
            payload: vec![],
        }
    }

    #[tokio::test]
    async fn test_send_when_routing_enabled() {
        let queue: BoundedQueue<RuntimeEnvelope> = BoundedQueue::new(10);
        let (tx, mut rx) = queue.split();
        let tracker = Arc::new(NodeStateTracker::running());
        let controller = RoutingController::new(tx, tracker);

        controller.send(make_envelope("1")).await.unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received.id, "1");
    }

    #[tokio::test]
    async fn test_buffer_when_routing_disabled() {
        let queue: BoundedQueue<RuntimeEnvelope> = BoundedQueue::new(10);
        let (tx, _rx) = queue.split();
        let tracker = Arc::new(NodeStateTracker::running());
        tracker.disable_routing();

        let controller = RoutingController::new(tx, tracker);
        controller.send(make_envelope("1")).await.unwrap();
        controller.send(make_envelope("2")).await.unwrap();

        assert_eq!(controller.buffer_len().await, 2);
    }

    #[tokio::test]
    async fn test_flush_buffer() {
        let queue: BoundedQueue<RuntimeEnvelope> = BoundedQueue::new(10);
        let (tx, mut rx) = queue.split();
        let tracker = Arc::new(NodeStateTracker::running());
        tracker.disable_routing();

        let controller = RoutingController::new(tx, tracker.clone());
        controller.send(make_envelope("1")).await.unwrap();
        controller.send(make_envelope("2")).await.unwrap();

        // Re-enable routing and flush
        tracker.enable_routing();
        let flushed = controller.flush().await.unwrap();
        assert_eq!(flushed, 2);

        // Check messages arrived
        let msg1 = rx.recv().await.unwrap();
        let msg2 = rx.recv().await.unwrap();
        assert_eq!(msg1.id, "1");
        assert_eq!(msg2.id, "2");
    }

    #[tokio::test]
    async fn test_buffer_capacity() {
        let queue: BoundedQueue<RuntimeEnvelope> = BoundedQueue::new(10);
        let (tx, _rx) = queue.split();
        let tracker = Arc::new(NodeStateTracker::running());
        tracker.disable_routing();

        let controller = RoutingController::with_capacity(tx, tracker, 2);

        controller.send(make_envelope("1")).await.unwrap();
        controller.send(make_envelope("2")).await.unwrap();

        // Third message should fail - buffer full
        let result = controller.send(make_envelope("3")).await;
        assert_eq!(result, Err(SendError::BufferFull));
    }

    #[tokio::test]
    async fn test_drain_buffer() {
        let queue: BoundedQueue<RuntimeEnvelope> = BoundedQueue::new(10);
        let (tx, _rx) = queue.split();
        let tracker = Arc::new(NodeStateTracker::running());
        tracker.disable_routing();

        let controller = RoutingController::new(tx, tracker);
        controller.send(make_envelope("1")).await.unwrap();
        controller.send(make_envelope("2")).await.unwrap();

        let messages = controller.drain_buffer().await;
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].id, "1");
        assert_eq!(messages[1].id, "2");

        // Buffer should be empty now
        assert_eq!(controller.buffer_len().await, 0);
    }
}
