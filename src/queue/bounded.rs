//! Bounded SPSC queue wrapper around crossbeam-channel.

use crate::config::DEFAULT_QUEUE_CAPACITY;
use crossbeam_channel::{bounded, Receiver, Sender, TryRecvError, TrySendError};

/// A bounded single-producer single-consumer queue.
///
/// Wraps crossbeam_channel for backpressure-aware message passing.
#[derive(Debug)]
pub struct BoundedQueue<T> {
    sender: Sender<T>,
    receiver: Receiver<T>,
    capacity: usize,
}

impl<T> BoundedQueue<T> {
    /// Create a new bounded queue with specified capacity.
    pub fn new(capacity: usize) -> Self {
        let (sender, receiver) = bounded(capacity);
        Self {
            sender,
            receiver,
            capacity,
        }
    }

    /// Create a new bounded queue with default capacity.
    pub fn with_default_capacity() -> Self {
        Self::new(DEFAULT_QUEUE_CAPACITY)
    }

    /// Send an item, blocking if the queue is full.
    pub fn send(&self, item: T) -> Result<(), crossbeam_channel::SendError<T>> {
        self.sender.send(item)
    }

    /// Receive an item, blocking if the queue is empty.
    pub fn recv(&self) -> Result<T, crossbeam_channel::RecvError> {
        self.receiver.recv()
    }

    /// Try to send an item without blocking.
    pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
        self.sender.try_send(item)
    }

    /// Try to receive an item without blocking.
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        self.receiver.try_recv()
    }

    /// Get the current number of items in the queue.
    pub fn len(&self) -> usize {
        self.receiver.len()
    }

    /// Check if the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.receiver.is_empty()
    }

    /// Check if the queue is full.
    pub fn is_full(&self) -> bool {
        self.receiver.len() >= self.capacity
    }

    /// Get the queue capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Split into separate sender and receiver handles.
    pub fn split(self) -> (QueueSender<T>, QueueReceiver<T>) {
        (
            QueueSender {
                sender: self.sender,
            },
            QueueReceiver {
                receiver: self.receiver,
                capacity: self.capacity,
            },
        )
    }
}

impl<T> Clone for BoundedQueue<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            receiver: self.receiver.clone(),
            capacity: self.capacity,
        }
    }
}

/// Sender half of a bounded queue.
#[derive(Debug, Clone)]
pub struct QueueSender<T> {
    sender: Sender<T>,
}

impl<T> QueueSender<T> {
    /// Send an item, blocking if full.
    pub fn send(&self, item: T) -> Result<(), crossbeam_channel::SendError<T>> {
        self.sender.send(item)
    }

    /// Try to send without blocking.
    pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
        self.sender.try_send(item)
    }
}

/// Receiver half of a bounded queue.
#[derive(Debug)]
pub struct QueueReceiver<T> {
    receiver: Receiver<T>,
    capacity: usize,
}

impl<T> QueueReceiver<T> {
    /// Receive an item, blocking if empty.
    pub fn recv(&self) -> Result<T, crossbeam_channel::RecvError> {
        self.receiver.recv()
    }

    /// Try to receive without blocking.
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        self.receiver.try_recv()
    }

    /// Get current queue depth.
    pub fn len(&self) -> usize {
        self.receiver.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.receiver.is_empty()
    }

    /// Get capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_send_recv() {
        let queue: BoundedQueue<String> = BoundedQueue::new(10);
        queue.send("hello".to_string()).unwrap();
        let msg = queue.recv().unwrap();
        assert_eq!(msg, "hello");
    }

    #[test]
    fn test_capacity_limit() {
        let queue: BoundedQueue<i32> = BoundedQueue::new(2);
        queue.send(1).unwrap();
        queue.send(2).unwrap();

        // Third send should fail with try_send
        let result = queue.try_send(3);
        assert!(result.is_err());
        assert!(matches!(result, Err(TrySendError::Full(_))));
    }

    #[test]
    fn test_len_and_capacity() {
        let queue: BoundedQueue<i32> = BoundedQueue::new(5);
        assert_eq!(queue.capacity(), 5);
        assert_eq!(queue.len(), 0);
        assert!(queue.is_empty());

        queue.send(1).unwrap();
        queue.send(2).unwrap();
        assert_eq!(queue.len(), 2);
        assert!(!queue.is_empty());
    }

    #[test]
    fn test_split() {
        let queue: BoundedQueue<i32> = BoundedQueue::new(10);
        let (sender, receiver) = queue.split();

        sender.send(42).unwrap();
        let val = receiver.recv().unwrap();
        assert_eq!(val, 42);
    }
}
