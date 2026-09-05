//! Dedicated metrics server for serving Prometheus metrics on a separate port.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{Router, routing::get};
use tokio::net::TcpListener;

use crate::orchestrator::PipelineHandle;

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
    #[expect(
        clippy::unwrap_used,
        reason = "hardcoded literal \"127.0.0.1:9091\" is always a valid SocketAddr"
    )]
    fn default() -> Self {
        Self { bind: "127.0.0.1:9091".parse().unwrap(), path: "/metrics".to_string() }
    }
}

/// Dedicated metrics server for serving Prometheus metrics on a separate port.
pub struct MetricsServer {
    listener: TcpListener,
    router: Router,
}

impl MetricsServer {
    /// Creates a new metrics server.
    pub async fn new(
        config: MetricsServerConfig,
        orchestrator: Arc<PipelineHandle>,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind(config.bind).await?;
        let router = create_metrics_router(orchestrator, &config.path);

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

fn create_metrics_router(orchestrator: Arc<PipelineHandle>, path: &str) -> Router {
    Router::new().route(path, get(handlers::metrics)).with_state(orchestrator)
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
}
