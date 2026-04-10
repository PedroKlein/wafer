//! HTTP-based sink implementation.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use crate::error::{ConfigError, Result, WaferError};
use crate::node::Lifecycle;
use crate::queue::RuntimeEnvelope;

use super::batch::BatchBuffer;
use super::{BatchStats, Sink};

/// Configuration for HttpSink batching behavior.
#[derive(Debug, Clone, Default)]
pub struct HttpSinkBatchConfig {
    pub batch_size: Option<usize>,
    pub batch_timeout_ms: Option<u64>,
}

/// An HTTP sink that POSTs messages to an endpoint. Supports optional batching.
pub struct HttpSink {
    id: String,
    url: String,
    client: Option<reqwest::Client>,
    timeout: Duration,
    headers: Vec<(String, String)>,
    batch_config: HttpSinkBatchConfig,
    batch_buffer: Option<BatchBuffer<RuntimeEnvelope>>,
    batch_stats: BatchStats,
}

impl HttpSink {
    #[must_use]
    pub fn new(id: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            url: url.into(),
            client: None,
            timeout: Duration::from_secs(30),
            headers: Vec::new(),
            batch_config: HttpSinkBatchConfig::default(),
            batch_buffer: None,
            batch_stats: BatchStats::default(),
        }
    }

    /// Create a new HttpSink with custom batching configuration.
    #[must_use]
    pub fn with_batching(
        id: impl Into<String>,
        url: impl Into<String>,
        batch_config: HttpSinkBatchConfig,
    ) -> Self {
        Self {
            id: id.into(),
            url: url.into(),
            client: None,
            timeout: Duration::from_secs(30),
            headers: Vec::new(),
            batch_config,
            batch_buffer: None,
            batch_stats: BatchStats::default(),
        }
    }

    /// Set the request timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Get the target URL.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    async fn send_message(&self, envelope: RuntimeEnvelope) -> Result<()> {
        let client = self.client.as_ref().ok_or_else(|| WaferError::PluginInit {
            message: "HttpSink not initialized - call init() first".to_string(),
        })?;

        let mut request = client.post(&self.url).body(envelope.payload);

        for (name, value) in &self.headers {
            request = request.header(name, value);
        }

        let response = request.send().await.map_err(|e| {
            WaferError::Io(std::io::Error::other(format!("HTTP request failed: {e}")))
        })?;

        if !response.status().is_success() {
            let status = response.status();
            return Err(WaferError::Io(std::io::Error::other(format!(
                "HTTP request failed with status: {status}"
            ))));
        }

        Ok(())
    }

    /// Send a batch of messages to the HTTP endpoint as a JSON array.
    async fn send_batch(&mut self, batch: Vec<RuntimeEnvelope>) -> Result<()> {
        let batch_size = batch.len();
        let client = self.client.as_ref().ok_or_else(|| WaferError::PluginInit {
            message: "HttpSink not initialized - call init() first".to_string(),
        })?;

        // Convert payloads to base64-encoded strings for JSON compatibility
        let payloads: Vec<String> = batch
            .into_iter()
            .map(|e| base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &e.payload))
            .collect();

        let json_body = serde_json::to_vec(&payloads).map_err(|e| {
            WaferError::Io(std::io::Error::other(format!("Failed to serialize batch: {e}")))
        })?;

        let mut request =
            client.post(&self.url).header("Content-Type", "application/json").body(json_body);

        // Add custom headers
        for (name, value) in &self.headers {
            request = request.header(name, value);
        }

        let response = request.send().await.map_err(|e| {
            WaferError::Io(std::io::Error::other(format!("HTTP batch request failed: {e}")))
        })?;

        if !response.status().is_success() {
            let status = response.status();
            return Err(WaferError::Io(std::io::Error::other(format!(
                "HTTP batch request failed with status: {status}"
            ))));
        }

        self.batch_stats.flushes_since_last_check += 1;
        self.batch_stats.last_flush_size = batch_size as u64;

        Ok(())
    }
}

impl Lifecycle for HttpSink {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &'static str {
        "sink/http"
    }

    fn validate(&self) -> Result<()> {
        if self.url.is_empty() {
            return Err(WaferError::Config(ConfigError::Message(
                "HTTP URL cannot be empty".to_string(),
            )));
        }

        if !self.url.starts_with("http://") && !self.url.starts_with("https://") {
            return Err(WaferError::Config(ConfigError::Message(
                "HTTP URL must start with http:// or https://".to_string(),
            )));
        }

        // Validate batch configuration
        if let Some(batch_size) = self.batch_config.batch_size {
            if batch_size == 0 {
                return Err(WaferError::Config(ConfigError::Message(
                    "batch_size must be greater than 0".to_string(),
                )));
            }
        }

        Ok(())
    }

    fn init(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            let client = reqwest::Client::builder().timeout(self.timeout).build().map_err(|e| {
                WaferError::PluginInit { message: format!("Failed to create HTTP client: {e}") }
            })?;

            self.client = Some(client);

            if let Some(batch_size) = self.batch_config.batch_size {
                let timeout =
                    Duration::from_millis(self.batch_config.batch_timeout_ms.unwrap_or(1000));
                self.batch_buffer = Some(BatchBuffer::new(batch_size, timeout));
            }

            tracing::info!(
                sink_id = %self.id,
                url = %self.url,
                batching = self.batch_config.batch_size.is_some(),
                "HTTP sink initialized"
            );

            Ok(())
        })
    }

    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            if let Some(ref mut buffer) = self.batch_buffer {
                let remaining = buffer.take();
                if !remaining.is_empty() {
                    self.send_batch(remaining).await?;
                }
            }

            self.client = None;
            self.batch_buffer = None;

            tracing::info!(sink_id = %self.id, "HTTP sink closed");
            Ok(())
        })
    }
}

impl Sink for HttpSink {
    fn collect(
        &mut self,
        envelope: RuntimeEnvelope,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            if let Some(ref mut buffer) = self.batch_buffer {
                if let Some(batch) = buffer.push(envelope) {
                    self.send_batch(batch).await?;
                }
                Ok(())
            } else {
                self.send_message(envelope).await
            }
        })
    }

    fn flush(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            if let Some(ref mut buffer) = self.batch_buffer {
                let batch = buffer.take();
                if !batch.is_empty() {
                    self.send_batch(batch).await?;
                }
            }
            Ok(())
        })
    }

    fn batch_timeout(&self) -> Option<Duration> {
        self.batch_config
            .batch_size
            .map(|_| Duration::from_millis(self.batch_config.batch_timeout_ms.unwrap_or(1000)))
    }

    fn take_batch_stats(&mut self) -> Option<BatchStats> {
        self.batch_buffer.as_ref()?;

        self.batch_stats.current_buffer_size =
            self.batch_buffer.as_ref().map_or(0, |b| b.len() as u64);

        let stats = self.batch_stats.clone();
        self.batch_stats.flushes_since_last_check = 0;

        Some(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_sink_creation() {
        let sink = HttpSink::new("test-sink", "http://localhost:8080/api");

        assert_eq!(sink.id(), "test-sink");
        assert_eq!(sink.node_type(), "sink/http");
        assert_eq!(sink.url(), "http://localhost:8080/api");
    }

    #[test]
    fn test_http_sink_validate_empty_url() {
        let sink = HttpSink::new("test-sink", "");
        assert!(sink.validate().is_err());
    }

    #[test]
    fn test_http_sink_validate_invalid_url() {
        let sink = HttpSink::new("test-sink", "not-a-url");
        assert!(sink.validate().is_err());
    }

    #[test]
    fn test_http_sink_validate_success() {
        let sink = HttpSink::new("test-sink", "http://localhost:8080/api");
        assert!(sink.validate().is_ok());

        let sink_https = HttpSink::new("test-sink", "https://example.com/api");
        assert!(sink_https.validate().is_ok());
    }

    #[test]
    fn test_http_sink_with_batching() {
        let sink = HttpSink::with_batching(
            "test-sink",
            "http://localhost:8080/api",
            HttpSinkBatchConfig { batch_size: Some(10), batch_timeout_ms: Some(500) },
        );

        assert_eq!(sink.batch_config.batch_size, Some(10));
        assert_eq!(sink.batch_config.batch_timeout_ms, Some(500));
    }

    #[test]
    fn test_http_sink_batch_timeout() {
        let sink = HttpSink::with_batching(
            "test-sink",
            "http://localhost:8080/api",
            HttpSinkBatchConfig { batch_size: Some(10), batch_timeout_ms: Some(500) },
        );

        assert_eq!(sink.batch_timeout(), Some(Duration::from_millis(500)));
    }

    #[test]
    fn test_http_sink_batch_timeout_none_without_batching() {
        let sink = HttpSink::new("test-sink", "http://localhost:8080/api");
        assert_eq!(sink.batch_timeout(), None);
    }

    #[test]
    fn test_http_sink_validate_zero_batch_size() {
        let sink = HttpSink::with_batching(
            "test-sink",
            "http://localhost:8080/api",
            HttpSinkBatchConfig { batch_size: Some(0), batch_timeout_ms: Some(1000) },
        );

        let result = sink.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("batch_size"));
    }

    #[test]
    fn test_http_sink_with_custom_headers() {
        let sink = HttpSink::new("test-sink", "http://localhost:8080/api")
            .with_header("Authorization", "Bearer token123")
            .with_header("X-Custom", "value");

        assert_eq!(sink.headers.len(), 2);
        assert_eq!(sink.headers[0], ("Authorization".to_string(), "Bearer token123".to_string()));
    }

    #[test]
    fn test_http_sink_with_custom_timeout() {
        let sink = HttpSink::new("test-sink", "http://localhost:8080/api")
            .with_timeout(Duration::from_secs(60));

        assert_eq!(sink.timeout, Duration::from_secs(60));
    }

    #[tokio::test]
    async fn test_http_sink_collect_before_init() {
        let mut sink = HttpSink::new("test-sink", "http://localhost:8080/api");
        let env = RuntimeEnvelope::from_string("test", "data");

        let result = sink.collect(env).await;
        assert!(result.is_err());
    }
}
