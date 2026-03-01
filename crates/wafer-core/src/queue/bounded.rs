//! Async bounded SPSC queue using tokio::sync::mpsc.
//!
//! This module provides async-native bounded channels for inter-node
//! communication in the DAG pipeline. Using tokio channels ensures
//! proper cooperative scheduling without blocking the async runtime.

use crate::config::DEFAULT_QUEUE_CAPACITY;
use tokio::sync::mpsc::{self, error::SendError, error::TrySendError};

/// A bounded single-producer single-consumer queue.
///
/// Wraps `tokio::sync::mpsc` for async-native backpressure-aware message passing.
/// Unlike crossbeam channels, this integrates properly with the tokio runtime
/// and won't block executor threads.
#[derive(Debug)]
pub struct BoundedQueue<T> {
    sender: mpsc::Sender<T>,
    receiver: mpsc::Receiver<T>,
    capacity: usize,
}

impl<T> BoundedQueue<T> {
    /// Create a new bounded queue with specified capacity.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let (sender, receiver) = mpsc::channel(capacity);
        Self {
            sender,
            receiver,
            capacity,
        }
    }

    /// Create a new bounded queue with default capacity.
    #[must_use]
    pub fn with_default_capacity() -> Self {
        Self::new(DEFAULT_QUEUE_CAPACITY)
    }

    /// Send an item asynchronously, waiting if the queue is full.
    ///
    /// This is the preferred method for sending in async contexts.
    /// It will yield to other tasks while waiting for capacity.
    pub async fn send(&self, item: T) -> Result<(), SendError<T>> {
        self.sender.send(item).await
    }

    /// Receive an item asynchronously, waiting if the queue is empty.
    ///
    /// Returns `None` if all senders have been dropped (channel closed).
    pub async fn recv(&mut self) -> Option<T> {
        self.receiver.recv().await
    }

    /// Try to send an item without waiting.
    ///
    /// Returns immediately with an error if the queue is full.
    pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
        self.sender.try_send(item)
    }

    /// Try to receive an item without waiting.
    ///
    /// Returns immediately with an error if the queue is empty.
    pub fn try_recv(&mut self) -> Result<T, mpsc::error::TryRecvError> {
        self.receiver.try_recv()
    }

    /// Get the queue capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Split into separate sender and receiver handles.
    ///
    /// This consumes the queue and returns owned handles that can be
    /// moved to different tasks.
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

/// Sender half of a bounded queue.
///
/// Can be cloned to create multiple senders (though SPSC pattern
/// typically uses only one).
#[derive(Debug, Clone)]
pub struct QueueSender<T> {
    sender: mpsc::Sender<T>,
}

impl<T> QueueSender<T> {
    /// Send an item asynchronously, waiting if full.
    pub async fn send(&self, item: T) -> Result<(), SendError<T>> {
        self.sender.send(item).await
    }

    /// Try to send without waiting.
    pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
        self.sender.try_send(item)
    }

    /// Check if the receiver has been dropped.
    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }

    /// Get the current available capacity (number of items that can be sent without blocking).
    ///
    /// This is the number of slots available, not the total capacity.
    pub fn available_capacity(&self) -> usize {
        self.sender.capacity()
    }

    /// Get the maximum capacity of the channel.
    pub fn max_capacity(&self) -> usize {
        self.sender.max_capacity()
    }
}

/// Receiver half of a bounded queue.
///
/// Cannot be cloned - there is exactly one receiver per queue.
#[derive(Debug)]
pub struct QueueReceiver<T> {
    receiver: mpsc::Receiver<T>,
    capacity: usize,
}

impl<T> QueueReceiver<T> {
    /// Receive an item asynchronously, waiting if empty.
    ///
    /// Returns `None` when all senders are dropped (channel closed).
    pub async fn recv(&mut self) -> Option<T> {
        self.receiver.recv().await
    }

    /// Try to receive without waiting.
    pub fn try_recv(&mut self) -> Result<T, mpsc::error::TryRecvError> {
        self.receiver.try_recv()
    }

    /// Get capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Close the receiver, preventing further sends.
    ///
    /// Any pending messages can still be received.
    pub fn close(&mut self) {
        self.receiver.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_send_recv() {
        let mut queue: BoundedQueue<String> = BoundedQueue::new(10);
        queue.send("hello".to_string()).await.unwrap();
        let msg = queue.recv().await.unwrap();
        assert_eq!(msg, "hello");
    }

    #[tokio::test]
    async fn test_capacity_limit() {
        let queue: BoundedQueue<i32> = BoundedQueue::new(2);
        queue.send(1).await.unwrap();
        queue.send(2).await.unwrap();

        // Third send should fail with try_send
        let result = queue.try_send(3);
        assert!(result.is_err());
        assert!(matches!(result, Err(TrySendError::Full(_))));
    }

    #[tokio::test]
    async fn test_capacity() {
        let queue: BoundedQueue<i32> = BoundedQueue::new(5);
        assert_eq!(queue.capacity(), 5);
    }

    #[tokio::test]
    async fn test_split() {
        let queue: BoundedQueue<i32> = BoundedQueue::new(10);
        let (sender, mut receiver) = queue.split();

        sender.send(42).await.unwrap();
        let val = receiver.recv().await.unwrap();
        assert_eq!(val, 42);
    }

    #[tokio::test]
    async fn test_channel_close_on_sender_drop() {
        let queue: BoundedQueue<i32> = BoundedQueue::new(10);
        let (sender, mut receiver) = queue.split();

        sender.send(1).await.unwrap();
        drop(sender);

        // Should still receive pending message
        assert_eq!(receiver.recv().await, Some(1));
        // Now should return None (channel closed)
        assert_eq!(receiver.recv().await, None);
    }
}
