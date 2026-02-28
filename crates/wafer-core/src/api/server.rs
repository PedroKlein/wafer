//! HTTP API server setup.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    routing::{get, post},
    Router,
};
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;

use crate::control::PipelineControl;

use super::handlers;

/// Configuration for the HTTP API server.
#[derive(Debug, Clone)]
pub struct ApiConfig {
    /// Address to bind the API server to
    pub bind: SocketAddr,
    /// Whether metrics should be served on the same server
    pub serve_metrics: bool,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:9090".parse().unwrap(),
            serve_metrics: true,
        }
    }
}

/// The HTTP API server.
pub struct ApiServer {
    listener: TcpListener,
    router: Router,
}

impl ApiServer {
    /// Creates a new API server.
    pub async fn new<C: PipelineControl + 'static>(
        config: ApiConfig,
        controller: Arc<C>,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind(config.bind).await?;
        let router = create_router(controller, config.serve_metrics);
        
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
        axum::serve(self.listener, self.router)
            .with_graceful_shutdown(shutdown)
            .await
    }
}

/// Creates the Axum router with all routes.
fn create_router<C: PipelineControl + 'static>(
    controller: Arc<C>,
    serve_metrics: bool,
) -> Router {
    let mut router = Router::new()
        // Health endpoints
        .route("/health", get(handlers::health))
        .route("/ready", get(handlers::ready::<C>))
        // Pipeline endpoints
        .route("/api/v1/pipeline", get(handlers::get_pipeline::<C>))
        .route("/api/v1/pipeline/reload", post(handlers::reload_config::<C>))
        .route("/api/v1/pipeline/drain", post(handlers::drain::<C>))
        .route("/api/v1/pipeline/shutdown", post(handlers::shutdown::<C>))
        // Node endpoints
        .route("/api/v1/nodes", get(handlers::list_nodes::<C>))
        .route("/api/v1/nodes/:id", get(handlers::get_node::<C>))
        .route("/api/v1/nodes/:id/hot-swap", post(handlers::hot_swap::<C>));

    if serve_metrics {
        router = router.route("/metrics", get(handlers::metrics::<C>));
    }

    router
        .layer(TraceLayer::new_for_http())
        .with_state(controller)
}

/// Starts the API server as a convenience function.
pub async fn start_api_server<C: PipelineControl + 'static>(
    config: ApiConfig,
    controller: Arc<C>,
) -> std::io::Result<ApiServer> {
    ApiServer::new(config, controller).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ApiConfig::default();
        assert_eq!(config.bind, "127.0.0.1:9090".parse::<SocketAddr>().unwrap());
        assert!(config.serve_metrics);
    }
}
