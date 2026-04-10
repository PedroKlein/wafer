//! Async bounded SPSC queue using `tokio::sync::mpsc`.

use crate::config::DEFAULT_QUEUE_CAPACITY;
use tokio::sync::mpsc::{self, error::SendError, error::TrySendError};

/// Wraps `tokio::sync::mpsc` for async-native backpressure-aware message passing.
///
/// Unlike crossbeam channels, this won't block executor threads.
#[derive(Debug)]
pub struct BoundedQueue<T> {
    sender: mpsc::Sender<T>,
    receiver: mpsc::Receiver<T>,
    capacity: usize,
}

impl<T> BoundedQueue<T> {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let (sender, receiver) = mpsc::channel(capacity);
        Self { sender, receiver, capacity }
    }

    #[must_use]
    pub fn with_default_capacity() -> Self {
        Self::new(DEFAULT_QUEUE_CAPACITY)
    }

    pub async fn send(&self, item: T) -> Result<(), SendError<T>> {
        self.sender.send(item).await
    }

    pub async fn recv(&mut self) -> Option<T> {
        self.receiver.recv().await
    }

    pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
        self.sender.try_send(item)
    }

    pub fn try_recv(&mut self) -> Result<T, mpsc::error::TryRecvError> {
        self.receiver.try_recv()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn split(self) -> (QueueSender<T>, QueueReceiver<T>) {
        (
            QueueSender { sender: self.sender },
            QueueReceiver { receiver: self.receiver, capacity: self.capacity },
        )
    }
}

#[derive(Debug, Clone)]
pub struct QueueSender<T> {
    sender: mpsc::Sender<T>,
}

impl<T> QueueSender<T> {
    pub async fn send(&self, item: T) -> Result<(), SendError<T>> {
        self.sender.send(item).await
    }

    pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
        self.sender.try_send(item)
    }

    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }

    /// Returns available slots, not total capacity.
    pub fn available_capacity(&self) -> usize {
        self.sender.capacity()
    }

    pub fn max_capacity(&self) -> usize {
        self.sender.max_capacity()
    }
}

/// Receiver half of a bounded queue. Cannot be cloned.
#[derive(Debug)]
pub struct QueueReceiver<T> {
    receiver: mpsc::Receiver<T>,
    capacity: usize,
}

impl<T> QueueReceiver<T> {
    pub async fn recv(&mut self) -> Option<T> {
        self.receiver.recv().await
    }

    pub fn try_recv(&mut self) -> Result<T, mpsc::error::TryRecvError> {
        self.receiver.try_recv()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Close the receiver. Pending messages can still be received.
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

        assert_eq!(receiver.recv().await, Some(1));
        assert_eq!(receiver.recv().await, None);
    }
}
