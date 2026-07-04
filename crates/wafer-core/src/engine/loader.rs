use crate::config::EngineConfig;
use crate::error::{Result, WaferError};
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::task::JoinHandle;
use wasmtime::{
    Config, Engine,
    component::{Component, Linker},
};
use wasmtime_wasi::p2::add_to_linker_async;
use wasmtime_wasi_nn::wit::add_to_linker as add_nn_to_linker;

use super::host::WaferState;

pub struct WaferEngine {
    engine: Engine,
    fuel_limit: u64,
    epoch_deadline: u64,
    epoch_tick_ms: u64,
    linker: OnceLock<Linker<WaferState>>,
    epoch_ticker: OnceLock<JoinHandle<()>>,
}

impl WaferEngine {
    #[must_use = "creating an engine without using it is expensive"]
    pub fn new() -> Result<Self> {
        Self::from_engine_config(&EngineConfig::default())
    }

    #[must_use = "creating an engine without using it is expensive"]
    pub fn from_engine_config(engine_config: &EngineConfig) -> Result<Self> {
        let mut config = Config::new();
        config.consume_fuel(true);
        config.wasm_component_model(true);
        config.epoch_interruption(true);

        let engine =
            Engine::new(&config).map_err(|e| WaferError::PluginInit { message: e.to_string() })?;

        Ok(Self {
            engine,
            fuel_limit: engine_config.fuel_limit,
            epoch_deadline: engine_config.epoch_deadline,
            epoch_tick_ms: engine_config.epoch_tick_ms,
            linker: OnceLock::new(),
            epoch_ticker: OnceLock::new(),
        })
    }

    pub fn ensure_epoch_ticker(&self) {
        self.epoch_ticker.get_or_init(|| {
            let engine = self.engine.clone();
            let interval = Duration::from_millis(self.epoch_tick_ms);
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(interval);
                ticker.tick().await;
                loop {
                    ticker.tick().await;
                    engine.increment_epoch();
                }
            })
        });
    }

    pub fn shutdown(&self) {
        if let Some(handle) = self.epoch_ticker.get() {
            handle.abort();
        }
    }

    #[must_use = "loading a component without using it is expensive"]
    pub fn load_component(&self, path: impl AsRef<Path>) -> Result<Component> {
        let path = path.as_ref();
        Component::from_file(&self.engine, path)
            .map_err(|source| WaferError::ComponentLoad { path: path.to_path_buf(), source })
    }

    #[must_use = "loading a component without using it is expensive"]
    pub fn load_component_from_bytes(&self, bytes: &[u8], name: &str) -> Result<Component> {
        Component::from_binary(&self.engine, bytes).map_err(|source| WaferError::ComponentLoad {
            path: std::path::PathBuf::from(format!("<bytes:{name}>")),
            source,
        })
    }

    pub fn linker(&self) -> Result<&Linker<WaferState>> {
        if let Some(linker) = self.linker.get() {
            return Ok(linker);
        }

        let mut linker = Linker::new(&self.engine);
        add_to_linker_async(&mut linker)
            .map_err(|e| WaferError::PluginInit { message: e.to_string() })?;
        add_nn_to_linker(&mut linker, |state: &mut WaferState| state.nn_view())
            .map_err(|e| WaferError::PluginInit { message: e.to_string() })?;

        let _ = self.linker.set(linker);

        self.linker.get().ok_or_else(|| WaferError::PluginInit {
            message: "linker initialization failed unexpectedly".to_string(),
        })
    }

    pub fn inner(&self) -> &Engine {
        &self.engine
    }

    pub fn fuel_limit(&self) -> u64 {
        self.fuel_limit
    }

    pub fn epoch_deadline(&self) -> u64 {
        self.epoch_deadline
    }
}

impl Drop for WaferEngine {
    fn drop(&mut self) {
        if let Some(handle) = self.epoch_ticker.get() {
            handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DEFAULT_EPOCH_DEADLINE, DEFAULT_EPOCH_TICK_MS, DEFAULT_FUEL_LIMIT};

    #[test]
    fn engine_creation_default() {
        let engine = WaferEngine::new().expect("Failed to create engine");
        assert_eq!(engine.fuel_limit(), DEFAULT_FUEL_LIMIT);
        assert_eq!(engine.epoch_deadline(), DEFAULT_EPOCH_DEADLINE);
    }

    #[test]
    fn engine_from_engine_config() {
        let cfg = EngineConfig {
            fuel_limit: 500_000,
            epoch_deadline: 50,
            epoch_tick_ms: DEFAULT_EPOCH_TICK_MS,
        };
        let engine = WaferEngine::from_engine_config(&cfg).expect("Failed to create engine");
        assert_eq!(engine.fuel_limit(), 500_000);
        assert_eq!(engine.epoch_deadline(), 50);
    }

    #[test]
    fn engine_from_default_engine_config() {
        let cfg = EngineConfig::default();
        let engine = WaferEngine::from_engine_config(&cfg).expect("Failed to create engine");
        assert_eq!(engine.fuel_limit(), DEFAULT_FUEL_LIMIT);
        assert_eq!(engine.epoch_deadline(), DEFAULT_EPOCH_DEADLINE);
    }

    #[test]
    fn engine_inner_is_valid() {
        let engine = WaferEngine::new().expect("Failed to create engine");
        let _inner = engine.inner();
    }

    #[test]
    fn load_component_nonexistent_file() {
        let engine = WaferEngine::new().expect("Failed to create engine");
        let result = engine.load_component("/nonexistent/path/to/component.wasm");
        assert!(result.is_err());

        let err = result.err().expect("Expected error");
        match err {
            WaferError::ComponentLoad { path, .. } => {
                assert_eq!(path.to_string_lossy(), "/nonexistent/path/to/component.wasm");
            }
            other => panic!("Expected ComponentLoad error, got {other:?}"),
        }
    }

    #[test]
    fn load_component_invalid_file() {
        use std::io::Write;

        let temp_dir = std::env::temp_dir();
        let invalid_wasm = temp_dir.join("invalid_test.wasm");

        let mut file = std::fs::File::create(&invalid_wasm).expect("Failed to create temp file");
        file.write_all(b"not a valid wasm file").expect("Failed to write");
        drop(file);

        let engine = WaferEngine::new().expect("Failed to create engine");
        let result = engine.load_component(&invalid_wasm);

        let _ = std::fs::remove_file(&invalid_wasm);

        assert!(result.is_err());
        let err = result.err().expect("Expected error");
        assert!(matches!(err, WaferError::ComponentLoad { .. }));
    }

    #[test]
    fn linker_is_cached() {
        let engine = WaferEngine::new().expect("Failed to create engine");
        let linker1 = engine.linker().expect("Failed to get linker");
        let linker2 = engine.linker().expect("Failed to get linker");
        assert!(std::ptr::eq(linker1, linker2));
    }

    #[tokio::test]
    async fn epoch_ticker_starts_and_runs() {
        let engine = WaferEngine::new().expect("Failed to create engine");
        engine.ensure_epoch_ticker();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!engine.epoch_ticker.get().unwrap().is_finished());
    }

    #[test]
    fn zero_fuel_limit() {
        let cfg = EngineConfig { fuel_limit: 0, ..Default::default() };
        let engine = WaferEngine::from_engine_config(&cfg).expect("Failed to create engine");
        assert_eq!(engine.fuel_limit(), 0);
    }

    #[test]
    fn max_fuel_limit() {
        let cfg = EngineConfig { fuel_limit: u64::MAX, ..Default::default() };
        let engine = WaferEngine::from_engine_config(&cfg).expect("Failed to create engine");
        assert_eq!(engine.fuel_limit(), u64::MAX);
    }

    #[test]
    fn load_component_from_bytes_invalid() {
        let engine = WaferEngine::new().expect("Failed to create engine");
        let invalid_bytes = b"not a valid wasm component";
        let result = engine.load_component_from_bytes(invalid_bytes, "test-component");

        assert!(result.is_err());
        let err = result.err().expect("Expected error");
        match err {
            WaferError::ComponentLoad { path, .. } => {
                assert_eq!(path.to_string_lossy(), "<bytes:test-component>");
            }
            other => panic!("Expected ComponentLoad error, got {other:?}"),
        }
    }

    #[test]
    fn load_component_from_bytes_empty() {
        let engine = WaferEngine::new().expect("Failed to create engine");
        let empty_bytes: &[u8] = &[];
        let result = engine.load_component_from_bytes(empty_bytes, "empty");

        assert!(result.is_err());
        let err = result.err().expect("Expected error");
        assert!(matches!(err, WaferError::ComponentLoad { .. }));
    }
}
