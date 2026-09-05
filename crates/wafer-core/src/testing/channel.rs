//! Channel-backed Source and Sink implementations for testing.
//!
//! These allow tests to inject messages into and collect messages from a pipeline
//! without real I/O dependencies (no MQTT broker, no filesystem, no network).

use std::future::Future;
use std::pin::Pin;

use tokio::sync::mpsc;

use crate::error::Result;
use crate::node::Lifecycle;
use crate::node::Sink;
use crate::node::Source;
use crate::queue::RuntimeEnvelope;

const DEFAULT_CAPACITY: usize = 1024;

// =============================================================================
// ChannelSource
// =============================================================================

/// In-memory source backed by a tokio mpsc channel.
///
/// The test holds the `Sender` and pushes messages; the pipeline polls
/// via the `Source` trait. When the sender is dropped, `poll()` returns
/// `Ok(None)` (EOF), enabling finite-input pipeline tests.
pub struct ChannelSource {
    id: String,
    receiver: mpsc::Receiver<RuntimeEnvelope>,
}

impl ChannelSource {
    /// Create a channel source with default capacity (1024).
    ///
    /// Returns the sender (for the test) and the source (for the pipeline).
    pub fn new(id: impl Into<String>) -> (mpsc::Sender<RuntimeEnvelope>, Self) {
        Self::with_capacity(id, DEFAULT_CAPACITY)
    }

    /// Create a channel source with a specific capacity.
    pub fn with_capacity(
        id: impl Into<String>,
        capacity: usize,
    ) -> (mpsc::Sender<RuntimeEnvelope>, Self) {
        let (tx, rx) = mpsc::channel(capacity);
        let source = Self { id: id.into(), receiver: rx };
        (tx, source)
    }
}

impl Lifecycle for ChannelSource {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &'static str {
        "channel-source"
    }

    fn validate(&self) -> Result<()> {
        Ok(())
    }

    fn init(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }

    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }
}

impl Source for ChannelSource {
    fn poll(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<RuntimeEnvelope>>> + Send + '_>> {
        Box::pin(async { Ok(self.receiver.recv().await) })
    }
}

// =============================================================================
// ChannelSink
// =============================================================================

/// In-memory sink backed by a tokio mpsc channel.
///
/// The pipeline sends messages via the `Sink` trait; the test holds the
/// `Receiver` and collects results.
pub struct ChannelSink {
    id: String,
    sender: mpsc::Sender<RuntimeEnvelope>,
}

impl ChannelSink {
    /// Create a channel sink with default capacity (1024).
    ///
    /// Returns the sink (for the pipeline) and the receiver (for the test).
    pub fn new(id: impl Into<String>) -> (Self, mpsc::Receiver<RuntimeEnvelope>) {
        Self::with_capacity(id, DEFAULT_CAPACITY)
    }

    /// Create a channel sink with a specific capacity.
    pub fn with_capacity(
        id: impl Into<String>,
        capacity: usize,
    ) -> (Self, mpsc::Receiver<RuntimeEnvelope>) {
        let (tx, rx) = mpsc::channel(capacity);
        let sink = Self { id: id.into(), sender: tx };
        (sink, rx)
    }
}

impl Lifecycle for ChannelSink {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &'static str {
        "channel-sink"
    }

    fn validate(&self) -> Result<()> {
        Ok(())
    }

    fn init(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }

    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }
}

impl Sink for ChannelSink {
    fn collect(
        &mut self,
        envelope: RuntimeEnvelope,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            self.sender.send(envelope).await.map_err(|_send_err| {
                crate::error::WaferError::Runtime("channel sink receiver dropped".into())
            })?;
            Ok(())
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn channel_source_receives_messages() {
        let (tx, mut source) = ChannelSource::new("test-src");

        let msg1 = RuntimeEnvelope::from_string("origin", "hello");
        let msg2 = RuntimeEnvelope::from_string("origin", "world");

        tx.send(msg1).await.unwrap();
        tx.send(msg2).await.unwrap();

        let received1 = source.poll().await.unwrap().unwrap();
        assert_eq!(received1.payload_as_string(), "hello");

        let received2 = source.poll().await.unwrap().unwrap();
        assert_eq!(received2.payload_as_string(), "world");
    }

    #[tokio::test]
    async fn channel_source_returns_none_on_sender_drop() {
        let (tx, mut source) = ChannelSource::new("test-src");

        tx.send(RuntimeEnvelope::from_string("origin", "one")).await.unwrap();
        drop(tx);

        // First poll returns the buffered message
        let first = source.poll().await.unwrap();
        assert!(first.is_some());

        // Second poll returns None (EOF) since sender is dropped
        let eof = source.poll().await.unwrap();
        assert!(eof.is_none());
    }

    #[tokio::test]
    async fn channel_sink_collects_messages() {
        let (mut sink, mut rx) = ChannelSink::new("test-sink");

        let msg = RuntimeEnvelope::from_string("origin", "payload");
        sink.collect(msg).await.unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received.payload_as_string(), "payload");
    }

    #[tokio::test]
    async fn channel_source_with_custom_capacity() {
        let (tx, mut source) = ChannelSource::with_capacity("cap-src", 2);

        // Fill to capacity
        tx.send(RuntimeEnvelope::from_string("s", "a")).await.unwrap();
        tx.send(RuntimeEnvelope::from_string("s", "b")).await.unwrap();

        // Channel is full — try_send should fail
        let overflow = tx.try_send(RuntimeEnvelope::from_string("s", "c"));
        assert!(overflow.is_err());

        // Drain
        let a = source.poll().await.unwrap().unwrap();
        assert_eq!(a.payload_as_string(), "a");
        let b = source.poll().await.unwrap().unwrap();
        assert_eq!(b.payload_as_string(), "b");
    }

    #[tokio::test]
    async fn channel_sink_with_custom_capacity() {
        let (mut sink, rx) = ChannelSink::with_capacity("cap-sink", 2);

        sink.collect(RuntimeEnvelope::from_string("s", "x")).await.unwrap();
        sink.collect(RuntimeEnvelope::from_string("s", "y")).await.unwrap();

        // Drop rx to simulate test collecting nothing — sender should fail
        drop(rx);

        let err = sink.collect(RuntimeEnvelope::from_string("s", "z")).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn channel_source_n_messages_flow() {
        let (tx, mut source) = ChannelSource::new("n-src");
        let n = 100;

        for i in 0..n {
            tx.send(RuntimeEnvelope::from_string("s", format!("msg-{i}"))).await.unwrap();
        }
        drop(tx);

        let mut count = 0;
        while let Ok(Some(env)) = source.poll().await {
            assert_eq!(env.payload_as_string(), format!("msg-{count}"));
            count += 1;
        }
        assert_eq!(count, n);
    }

    #[tokio::test]
    async fn channel_sink_flush_is_noop() {
        let (mut sink, _rx) = ChannelSink::new("flush-sink");
        // flush should succeed without error
        sink.flush().await.unwrap();
    }

    #[tokio::test]
    async fn channel_sink_batch_timeout_is_none() {
        let (sink, _rx) = ChannelSink::new("bt-sink");
        assert!(sink.batch_timeout().is_none());
    }

    #[tokio::test]
    async fn channel_source_lifecycle_methods() {
        let (_tx, mut source) = ChannelSource::new("lc-src");
        assert_eq!(source.id(), "lc-src");
        assert_eq!(source.node_type(), "channel-source");
        source.validate().unwrap();
        source.init().await.unwrap();
        source.close().await.unwrap();
    }

    #[tokio::test]
    async fn channel_sink_lifecycle_methods() {
        let (mut sink, _rx) = ChannelSink::new("lc-sink");
        assert_eq!(sink.id(), "lc-sink");
        assert_eq!(sink.node_type(), "channel-sink");
        sink.validate().unwrap();
        sink.init().await.unwrap();
        sink.close().await.unwrap();
    }
}
