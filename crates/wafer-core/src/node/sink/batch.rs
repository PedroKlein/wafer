//! Batch buffer utility for sink batching.

use std::time::{Duration, Instant};

/// A buffer that collects items and flushes based on batch size or timeout.
///
/// # Panics
///
/// `new()` panics if `batch_size` is 0.
#[derive(Debug)]
pub struct BatchBuffer<T> {
    buffer: Vec<T>,
    batch_size: usize,
    timeout: Duration,
    last_flush: Instant,
}

impl<T> BatchBuffer<T> {
    pub fn new(batch_size: usize, timeout: Duration) -> Self {
        assert!(batch_size > 0, "batch_size must be greater than 0");
        Self {
            buffer: Vec::with_capacity(batch_size),
            batch_size,
            timeout,
            last_flush: Instant::now(),
        }
    }

    /// Push an item. Returns `Some(batch)` if batch size is reached.
    ///
    /// Does NOT check timeout - use `should_flush()` for that.
    pub fn push(&mut self, item: T) -> Option<Vec<T>> {
        self.buffer.push(item);
        if self.buffer.len() >= self.batch_size { Some(self.take()) } else { None }
    }

    /// Drain the buffer and reset the last flush time.
    pub fn take(&mut self) -> Vec<T> {
        self.last_flush = Instant::now();
        std::mem::take(&mut self.buffer)
    }

    /// Returns `true` if buffer is non-empty and timeout has elapsed.
    #[expect(dead_code, reason = "public API for sink implementations")]
    pub fn should_flush(&self) -> bool {
        !self.buffer.is_empty() && self.last_flush.elapsed() >= self.timeout
    }

    /// Returns `true` if the buffer contains no items.
    #[expect(dead_code, reason = "public API for sink implementations")]
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    #[expect(dead_code, reason = "public API for sink implementations")]
    pub const fn batch_size(&self) -> usize {
        self.batch_size
    }

    #[expect(dead_code, reason = "public API for sink implementations")]
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }

    #[expect(dead_code, reason = "public API for sink implementations")]
    pub fn elapsed_since_flush(&self) -> Duration {
        self.last_flush.elapsed()
    }

    /// Returns the time remaining until the timeout, or zero if already past.
    #[expect(dead_code, reason = "public API for sink implementations")]
    pub fn time_until_timeout(&self) -> Duration {
        self.timeout.saturating_sub(self.last_flush.elapsed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_push_returns_none_until_batch_size() {
        let mut buffer: BatchBuffer<i32> = BatchBuffer::new(3, Duration::from_secs(60));

        assert!(buffer.push(1).is_none());
        assert_eq!(buffer.len(), 1);

        assert!(buffer.push(2).is_none());
        assert_eq!(buffer.len(), 2);

        // Third push should return the batch
        let batch = buffer.push(3);
        assert!(batch.is_some());
        assert_eq!(batch.unwrap(), vec![1, 2, 3]);
        assert_eq!(buffer.len(), 0);
    }

    #[test]
    fn test_take_drains_buffer() {
        let mut buffer: BatchBuffer<&str> = BatchBuffer::new(10, Duration::from_secs(60));

        buffer.push("a");
        buffer.push("b");
        buffer.push("c");

        assert_eq!(buffer.len(), 3);

        let batch = buffer.take();
        assert_eq!(batch, vec!["a", "b", "c"]);
        assert!(buffer.is_empty());
        assert_eq!(buffer.len(), 0);
    }

    #[test]
    fn test_take_on_empty_buffer() {
        let mut buffer: BatchBuffer<i32> = BatchBuffer::new(5, Duration::from_secs(60));

        let batch = buffer.take();
        assert!(batch.is_empty());
    }

    #[test]
    fn test_should_flush_respects_timeout() {
        let mut buffer: BatchBuffer<i32> = BatchBuffer::new(100, Duration::from_millis(10));

        // Empty buffer should not flush
        assert!(!buffer.should_flush());

        buffer.push(1);

        // Just pushed, should not flush yet
        assert!(!buffer.should_flush());

        // Wait for timeout
        thread::sleep(Duration::from_millis(15));

        // Now should flush
        assert!(buffer.should_flush());
    }

    #[test]
    fn test_should_flush_empty_buffer_never_flushes() {
        let buffer: BatchBuffer<i32> = BatchBuffer::new(10, Duration::from_millis(1));

        // Wait past timeout
        thread::sleep(Duration::from_millis(5));

        // Empty buffer should never signal flush
        assert!(!buffer.should_flush());
    }

    #[test]
    fn test_take_resets_last_flush_time() {
        let mut buffer: BatchBuffer<i32> = BatchBuffer::new(100, Duration::from_millis(10));

        buffer.push(1);

        // Wait for timeout
        thread::sleep(Duration::from_millis(15));
        assert!(buffer.should_flush());

        // Take resets the timer
        let _ = buffer.take();

        // Push new item
        buffer.push(2);

        // Should not flush immediately after take
        assert!(!buffer.should_flush());
    }

    #[test]
    fn test_batch_triggers_on_exact_size() {
        let mut buffer: BatchBuffer<i32> = BatchBuffer::new(2, Duration::from_secs(60));

        assert!(buffer.push(1).is_none());
        let batch = buffer.push(2);
        assert_eq!(batch, Some(vec![1, 2]));
    }

    #[test]
    fn test_multiple_batches() {
        let mut buffer: BatchBuffer<i32> = BatchBuffer::new(2, Duration::from_secs(60));

        // First batch
        buffer.push(1);
        let batch1 = buffer.push(2);
        assert_eq!(batch1, Some(vec![1, 2]));

        // Second batch
        buffer.push(3);
        let batch2 = buffer.push(4);
        assert_eq!(batch2, Some(vec![3, 4]));
    }

    #[test]
    fn test_time_until_timeout() {
        let buffer: BatchBuffer<i32> = BatchBuffer::new(10, Duration::from_millis(100));

        // Just created, should have most of the timeout remaining
        let remaining = buffer.time_until_timeout();
        assert!(remaining <= Duration::from_millis(100));
        assert!(remaining > Duration::from_millis(90));
    }

    #[test]
    fn test_time_until_timeout_saturates_to_zero() {
        let buffer: BatchBuffer<i32> = BatchBuffer::new(10, Duration::from_millis(5));

        thread::sleep(Duration::from_millis(10));

        // Should be zero, not negative/overflow
        assert_eq!(buffer.time_until_timeout(), Duration::ZERO);
    }

    #[test]
    #[should_panic(expected = "batch_size must be greater than 0")]
    fn test_zero_batch_size_panics() {
        let _: BatchBuffer<i32> = BatchBuffer::new(0, Duration::from_secs(60));
    }

    #[test]
    fn test_accessors() {
        let buffer: BatchBuffer<i32> = BatchBuffer::new(42, Duration::from_secs(30));

        assert_eq!(buffer.batch_size(), 42);
        assert_eq!(buffer.timeout(), Duration::from_secs(30));
        assert!(buffer.is_empty());
        assert_eq!(buffer.len(), 0);
    }
}
