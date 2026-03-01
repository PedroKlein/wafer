//! HTTP-based source node implementation.
//!
//! Receives HTTP requests and produces [`RuntimeEnvelope`] messages from request bodies.
//! Supports webhook-style ingestion where external systems POST data to the pipeline.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::error::{ConfigError, Result, WaferError};
use crate::node::Lifecycle;
use crate::queue::RuntimeEnvelope;

use super::Source;

/// An HTTP-based source node that receives messages via HTTP POST requests.
///
/// The source starts an HTTP server that accepts POST requests on a configurable
/// path. Each request body becomes a message in the pipeline.
///
/// # Example
///
/// ```ignore
/// use wafer_core::node::{HttpSource, Lifecycle, Source};
///
/// let mut source = HttpSource::new(
///     "http-source",
///     "0.0.0.0:8081",
///     "/ingest",
/// );
/// source.validate()?;
/// source.init().await?;
///
/// // External clients can now POST to http://0.0.0.0:8081/ingest
/// while let Some(envelope) = source.poll().await? {
///     println!("Got message: {:?}", envelope.payload);
/// }
///
/// source.close().await?;
/// ```
pub struct HttpSource {
    /// Node identifier
    id: String,
    /// Address to bind the HTTP server to
    bind_addr: SocketAddr,
    /// Path to accept POST requests on
    path: String,
    /// Channel capacity for buffering incoming messages
    buffer_size: usize,
    /// Channel receiver for incoming messages
    message_rx: Option<mpsc::Receiver<RuntimeEnvelope>>,
    /// Shutdown signal sender
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    /// Server task handle
    server_handle: Option<tokio::task::JoinHandle<()>>,
}

impl HttpSource {
    /// Create a new HttpSource.
    ///
    /// The HTTP server is not started until `init()` is called.
    ///
    /// # Arguments
    ///
    /// * `id` - Node identifier
    /// * `bind_addr` - Address to bind the HTTP server to (e.g., "0.0.0.0:8081")
    /// * `path` - Path to accept POST requests on (e.g., "/ingest")
    ///
    /// # Panics
    ///
    /// Panics if the hardcoded fallback address `127.0.0.1:8081` fails to parse
    /// (this should never happen as it's a valid address literal).
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        bind_addr: impl Into<String>,
        path: impl Into<String>,
    ) -> Self {
        let bind_str = bind_addr.into();
        let bind_addr = bind_str
            .parse()
            .unwrap_or_else(|_| "127.0.0.1:8081".parse().unwrap());

        Self {
            id: id.into(),
            bind_addr,
            path: path.into(),
            buffer_size: 1000,
            message_rx: None,
            shutdown_tx: None,
            server_handle: None,
        }
    }

    /// Create a new HttpSource with custom buffer size.
    #[must_use]
    pub fn with_buffer_size(mut self, buffer_size: usize) -> Self {
        self.buffer_size = buffer_size;
        self
    }

    /// Get the bind address.
    #[must_use]
    pub fn bind_addr(&self) -> SocketAddr {
        self.bind_addr
    }

    /// Get the path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }
}

impl Lifecycle for HttpSource {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &'static str {
        "source/http"
    }

    fn validate(&self) -> Result<()> {
        if self.path.is_empty() {
            return Err(WaferError::Config(ConfigError::Message(
                "HTTP path cannot be empty".into(),
            )));
        }
        if !self.path.starts_with('/') {
            return Err(WaferError::Config(ConfigError::Message(
                "HTTP path must start with '/'".into(),
            )));
        }
        Ok(())
    }

    fn init(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            let (tx, rx) = mpsc::channel(self.buffer_size);
            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

            let bind_addr = self.bind_addr;
            let path = self.path.clone();
            let source_id = self.id.clone();

            // Create a simple HTTP server using hyper directly (lighter weight than axum)
            let handle = tokio::spawn(async move {
                run_http_server(bind_addr, path, source_id, tx, shutdown_rx).await;
            });

            self.message_rx = Some(rx);
            self.shutdown_tx = Some(shutdown_tx);
            self.server_handle = Some(handle);

            tracing::info!(
                source_id = %self.id,
                bind = %self.bind_addr,
                path = %self.path,
                "HTTP source server started"
            );

            Ok(())
        })
    }

    fn close(&mut self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            // Send shutdown signal
            if let Some(shutdown_tx) = self.shutdown_tx.take() {
                let _ = shutdown_tx.send(());
            }

            // Wait for server to stop
            if let Some(handle) = self.server_handle.take() {
                handle.abort();
                let _ = handle.await;
            }

            self.message_rx = None;

            tracing::info!(source_id = %self.id, "HTTP source server stopped");
            Ok(())
        })
    }
}

impl Source for HttpSource {
    fn poll(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Option<RuntimeEnvelope>>> + Send + '_>> {
        Box::pin(async move {
            let rx = self
                .message_rx
                .as_mut()
                .ok_or_else(|| WaferError::PluginInit {
                    message: "HttpSource not initialized - call init() first".into(),
                })?;

            match rx.recv().await {
                Some(envelope) => Ok(Some(envelope)),
                None => Ok(None), // Channel closed
            }
        })
    }
}

/// Run the HTTP server that receives messages.
async fn run_http_server(
    bind_addr: SocketAddr,
    path: String,
    source_id: String,
    tx: mpsc::Sender<RuntimeEnvelope>,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) {
    use hyper::body::Incoming;
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::Request;
    use hyper_util::rt::TokioIo;

    let listener = match tokio::net::TcpListener::bind(bind_addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(error = %e, "Failed to bind HTTP source server");
            return;
        }
    };

    let path = Arc::new(path);
    let source_id = Arc::new(source_id);

    loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                break;
            }
            accept_result = listener.accept() => {
                match accept_result {
                    Ok((stream, _)) => {
                        let tx = tx.clone();
                        let path = Arc::clone(&path);
                        let source_id = Arc::clone(&source_id);

                        tokio::spawn(async move {
                            let service = service_fn(move |req: Request<Incoming>| {
                                let tx = tx.clone();
                                let path = Arc::clone(&path);
                                let source_id = Arc::clone(&source_id);
                                async move {
                                    handle_request(req, &path, &source_id, tx).await
                                }
                            });

                            let io = TokioIo::new(stream);
                            if let Err(e) = http1::Builder::new()
                                .serve_connection(io, service)
                                .await
                            {
                                tracing::debug!(error = %e, "HTTP connection error");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to accept connection");
                    }
                }
            }
        }
    }
}

async fn handle_request(
    req: hyper::Request<hyper::body::Incoming>,
    expected_path: &str,
    source_id: &str,
    tx: mpsc::Sender<RuntimeEnvelope>,
) -> std::result::Result<
    hyper::Response<http_body_util::Full<hyper::body::Bytes>>,
    std::convert::Infallible,
> {
    use http_body_util::{BodyExt, Full};
    use hyper::body::Bytes;

    // Check method and path
    if req.method() != hyper::Method::POST {
        return Ok(hyper::Response::builder()
            .status(hyper::StatusCode::METHOD_NOT_ALLOWED)
            .body(Full::new(Bytes::from("Method not allowed")))
            .unwrap());
    }

    if req.uri().path() != expected_path {
        return Ok(hyper::Response::builder()
            .status(hyper::StatusCode::NOT_FOUND)
            .body(Full::new(Bytes::from("Not found")))
            .unwrap());
    }

    // Extract body
    let body_bytes = match req.collect().await {
        Ok(collected) => collected.to_bytes().to_vec(),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to read request body");
            return Ok(hyper::Response::builder()
                .status(hyper::StatusCode::BAD_REQUEST)
                .body(Full::new(Bytes::from("Failed to read body")))
                .unwrap());
        }
    };

    // Create envelope
    let envelope = RuntimeEnvelope::new(source_id, body_bytes);

    // Send to channel
    if tx.send(envelope).await.is_err() {
        return Ok(hyper::Response::builder()
            .status(hyper::StatusCode::SERVICE_UNAVAILABLE)
            .body(Full::new(Bytes::from("Source shutting down")))
            .unwrap());
    }

    Ok(hyper::Response::builder()
        .status(hyper::StatusCode::OK)
        .body(Full::new(Bytes::from("OK")))
        .unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_source_creation() {
        let source = HttpSource::new("test-http", "127.0.0.1:8081", "/ingest");
        assert_eq!(source.id(), "test-http");
        assert_eq!(source.node_type(), "source/http");
        assert_eq!(source.path(), "/ingest");
    }

    #[test]
    fn test_http_source_validate_empty_path() {
        let source = HttpSource::new("test-http", "127.0.0.1:8081", "");
        let result = source.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_http_source_validate_invalid_path() {
        let source = HttpSource::new("test-http", "127.0.0.1:8081", "no-slash");
        let result = source.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_http_source_validate_success() {
        let source = HttpSource::new("test-http", "127.0.0.1:8081", "/ingest");
        assert!(source.validate().is_ok());
    }

    #[tokio::test]
    async fn test_http_source_poll_before_init() {
        let mut source = HttpSource::new("test-http", "127.0.0.1:8081", "/ingest");
        let result = source.poll().await;
        assert!(result.is_err());
    }
}
