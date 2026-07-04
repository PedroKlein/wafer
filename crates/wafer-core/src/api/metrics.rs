//! Dedicated metrics server for serving Prometheus metrics on a separate port.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{Router, routing::get};
use tokio::net::TcpListener;

use crate::orchestrator::PipelineOrchestrator;

use super::handlers;

/// Configuration for the dedicated metrics server.
#[derive(Debug, Clone)]
pub struct MetricsServerConfig {
    /// Address to bind the metrics server to
    pub bind: SocketAddr,
    /// Path for the metrics endpoint (default: "/metrics")
    pub path: String,
}

impl Default for MetricsServerConfig {
    fn default() -> Self {
        Self { bind: "127.0.0.1:9091".parse().unwrap(), path: "/metrics".to_string() }
    }
}

/// Dedicated metrics server for serving Prometheus metrics on a separate port.
///
/// This is used when `metrics.bind` is configured to a different address than `api.bind`.
pub struct MetricsServer {
    listener: TcpListener,
    router: Router,
}

impl MetricsServer {
    /// Creates a new metrics server.
    pub async fn new(
        config: MetricsServerConfig,
        controller: Arc<PipelineOrchestrator>,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind(config.bind).await?;
        let router = create_metrics_router(controller, &config.path);

        Ok(Self { listener, router })
    }

    /// Returns the bound address.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Runs the server until shutdown.
    pub async fn run(self) -> std::io::Result<()> {
        axum::serve(self.listener, self.router).await
    }

    /// Runs the server with graceful shutdown.
    pub async fn run_with_shutdown(
        self,
        shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> std::io::Result<()> {
        axum::serve(self.listener, self.router).with_graceful_shutdown(shutdown).await
    }
}

/// Creates a minimal router with just the metrics endpoint.
fn create_metrics_router(controller: Arc<PipelineOrchestrator>, path: &str) -> Router {
    Router::new().route(path, get(handlers::metrics)).with_state(controller)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = MetricsServerConfig::default();
        assert_eq!(config.bind, "127.0.0.1:9091".parse::<SocketAddr>().unwrap());
        assert_eq!(config.path, "/metrics");
    }

    #[test]
    fn test_custom_path() {
        let config = MetricsServerConfig {
            bind: "0.0.0.0:9100".parse().unwrap(),
            path: "/prometheus/metrics".to_string(),
        };
        assert_eq!(config.bind, "0.0.0.0:9100".parse::<SocketAddr>().unwrap());
        assert_eq!(config.path, "/prometheus/metrics");
    }
}
