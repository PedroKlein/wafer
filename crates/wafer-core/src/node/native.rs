//! Native Rust baseline nodes for isolation tax measurement.
//!
//! These implement the same `Transform`/`Filter` traits as Wasm nodes but use
//! direct Rust function calls. The overhead difference between native and Wasm
//! IS the isolation tax measured in RQ1.
//!
//! See docs/decisions/2025-07-12-evaluation-harness-design.md — Session 8 D5.

use std::future::Future;
use std::pin::Pin;

use bytes::Bytes;

use crate::error::Result;
use crate::node::traits::{FilterOutcome, Lifecycle, ProcessError, ProcessResult};
use crate::node::{Filter, Transform};
use crate::queue::RuntimeEnvelope;

// =============================================================================
// NativeTransform
// =============================================================================

/// Native Rust transform node — same interface as Wasm, zero isolation overhead.
///
/// Used as the Layer 1 baseline: same Tokio tasks + mpsc channels + envelope,
/// but without Wasm boundary crossing, fuel/epoch metering, or WIT marshaling.
pub struct NativeTransform {
    id: String,
    node_type_name: String,
    process_fn: Box<dyn Fn(&[u8]) -> std::result::Result<Vec<u8>, ProcessError> + Send>,
}

impl NativeTransform {
    /// Create with a custom processing function.
    pub fn new(
        id: impl Into<String>,
        process_fn: impl Fn(&[u8]) -> std::result::Result<Vec<u8>, ProcessError> + Send + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            node_type_name: "native-transform".to_owned(),
            process_fn: Box::new(process_fn),
        }
    }

    /// Convenience: uppercase transform (matches wafer-uppercase plugin).
    #[must_use]
    pub fn uppercase(id: impl Into<String>) -> Self {
        Self::new(id, functions::uppercase)
    }

    /// Convenience: passthrough (matches pass-through plugin).
    #[must_use]
    pub fn passthrough(id: impl Into<String>) -> Self {
        Self::new(id, functions::passthrough)
    }
}

impl Lifecycle for NativeTransform {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &str {
        &self.node_type_name
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

impl Transform for NativeTransform {
    fn process(
        &mut self,
        input: RuntimeEnvelope,
    ) -> Pin<Box<dyn Future<Output = Result<ProcessResult>> + Send + '_>> {
        let result = (self.process_fn)(&input.payload);

        Box::pin(async move {
            match result {
                Ok(output_bytes) => {
                    // Preserve header (including bench metadata) but replace payload
                    let output = RuntimeEnvelope {
                        header: input.header.clone(),
                        payload: Bytes::from(output_bytes),
                        lineage: input.lineage,
                    };
                    Ok(ProcessResult::Emit(output))
                }
                Err(e) => Ok(ProcessResult::Error(e)),
            }
        })
    }
}

// =============================================================================
// NativeFilter
// =============================================================================

/// Native Rust filter node — same interface as Wasm filter, zero isolation.
pub struct NativeFilter {
    id: String,
    predicate_fn: Box<dyn Fn(&RuntimeEnvelope) -> bool + Send>,
}

impl NativeFilter {
    /// Create with a custom predicate function.
    pub fn new(
        id: impl Into<String>,
        predicate_fn: impl Fn(&RuntimeEnvelope) -> bool + Send + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            predicate_fn: Box::new(predicate_fn),
        }
    }

    /// Convenience: threshold filter on "temperature" field (matches wafer-threshold-filter plugin).
    #[must_use]
    pub fn threshold(id: impl Into<String>, threshold: f64) -> Self {
        Self::new(id, functions::threshold_filter(threshold))
    }
}

impl Lifecycle for NativeFilter {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &'static str {
        "native-filter"
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

impl Filter for NativeFilter {
    fn evaluate(
        &mut self,
        envelope: &RuntimeEnvelope,
    ) -> Pin<Box<dyn Future<Output = Result<FilterOutcome>> + Send + '_>> {
        let forward = (self.predicate_fn)(envelope);
        Box::pin(async move {
            Ok(if forward {
                FilterOutcome::Forward
            } else {
                FilterOutcome::Drop
            })
        })
    }
}

// =============================================================================
// Built-in functions (same logic as Wasm plugins, without isolation)
// =============================================================================

/// Native implementations matching Wasm plugin behavior.
pub mod functions {
    use super::*;

    /// ASCII uppercase (matches wafer-uppercase plugin).
    pub fn uppercase(payload: &[u8]) -> std::result::Result<Vec<u8>, ProcessError> {
        Ok(payload.to_ascii_uppercase())
    }

    /// Passthrough (matches pass-through plugin).
    pub fn passthrough(payload: &[u8]) -> std::result::Result<Vec<u8>, ProcessError> {
        Ok(payload.to_vec())
    }

    /// Temperature threshold filter factory (matches wafer-threshold-filter plugin).
    pub fn threshold_filter(
        threshold: f64,
    ) -> impl Fn(&RuntimeEnvelope) -> bool + Send + 'static {
        move |envelope: &RuntimeEnvelope| {
            let Ok(s) = std::str::from_utf8(&envelope.payload) else {
                return false;
            };
            extract_temperature(s).is_some_and(|t| t > threshold)
        }
    }

    /// Simple manual JSON temperature extraction (no serde dependency).
    fn extract_temperature(json: &str) -> Option<f64> {
        // Find "temperature" key and extract the numeric value after the colon
        let key = "\"temperature\"";
        let idx = json.find(key)?;
        let after_key = &json[idx + key.len()..];
        // Skip whitespace and colon
        let after_colon = after_key.trim_start().strip_prefix(':')?;
        let value_str = after_colon.trim_start();
        // Parse the number (stops at first non-numeric char)
        let end = value_str
            .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
            .unwrap_or(value_str.len());
        value_str[..end].parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn native_transform_uppercase() {
        let mut transform = NativeTransform::uppercase("test-upper");

        let input = RuntimeEnvelope::from_string("src", "hello world");
        let result = transform.process(input).await.unwrap();

        match result {
            ProcessResult::Emit(env) => {
                assert_eq!(env.payload.as_ref(), b"HELLO WORLD");
            }
            other => panic!("expected Emit, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn native_transform_passthrough() {
        let mut transform = NativeTransform::passthrough("test-pass");

        let input = RuntimeEnvelope::from_string("src", "unchanged");
        let result = transform.process(input).await.unwrap();

        match result {
            ProcessResult::Emit(env) => {
                assert_eq!(env.payload.as_ref(), b"unchanged");
            }
            other => panic!("expected Emit, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn native_filter_threshold_forwards() {
        let mut filter = NativeFilter::threshold("test-filter", 50.0);

        let input = RuntimeEnvelope::from_string("src", r#"{"temperature": 75.0}"#);
        let outcome = filter.evaluate(&input).await.unwrap();
        assert_eq!(outcome, FilterOutcome::Forward);
    }

    #[tokio::test]
    async fn native_filter_threshold_drops() {
        let mut filter = NativeFilter::threshold("test-filter", 50.0);

        let input = RuntimeEnvelope::from_string("src", r#"{"temperature": 25.0}"#);
        let outcome = filter.evaluate(&input).await.unwrap();
        assert_eq!(outcome, FilterOutcome::Drop);
    }

    #[tokio::test]
    async fn native_filter_invalid_json_drops() {
        let mut filter = NativeFilter::threshold("test-filter", 50.0);

        let input = RuntimeEnvelope::from_string("src", "not json");
        let outcome = filter.evaluate(&input).await.unwrap();
        assert_eq!(outcome, FilterOutcome::Drop);
    }

    #[tokio::test]
    async fn native_transform_lifecycle() {
        let mut transform = NativeTransform::passthrough("lc-test");
        assert_eq!(transform.id(), "lc-test");
        assert_eq!(transform.node_type(), "native-transform");
        transform.validate().unwrap();
        transform.init().await.unwrap();
        transform.close().await.unwrap();
    }

    #[tokio::test]
    async fn native_filter_lifecycle() {
        let mut filter = NativeFilter::threshold("lc-filter", 42.0);
        assert_eq!(filter.id(), "lc-filter");
        assert_eq!(filter.node_type(), "native-filter");
        filter.validate().unwrap();
        filter.init().await.unwrap();
        filter.close().await.unwrap();
    }

    #[test]
    fn extract_temperature_works() {
        use super::functions::threshold_filter;
        // Test via the filter itself
        let filter_fn = threshold_filter(40.0);
        let high = RuntimeEnvelope::from_string("s", r#"{"temperature": 42.5}"#);
        let low = RuntimeEnvelope::from_string("s", r#"{"temperature": 10.0}"#);
        let no_temp = RuntimeEnvelope::from_string("s", r#"{"humidity": 80}"#);
        assert!(filter_fn(&high));
        assert!(!filter_fn(&low));
        assert!(!filter_fn(&no_temp));
    }
}
