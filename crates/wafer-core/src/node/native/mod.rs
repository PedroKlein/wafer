//! Native Rust baseline nodes for isolation tax measurement.
//!
//! These implement the same `Transform`/`Filter` traits as Wasm nodes but use
//! direct Rust function calls. The overhead difference between native and Wasm
//! IS the isolation tax measured in RQ1.
//!
//! See docs/rfcs/RFC-008-evaluation-harness.md — Session 8 D5.

use std::future::Future;
use std::pin::Pin;

use bytes::Bytes;

use crate::error::{Result, WaferError};
use crate::node::traits::{FilterOutcome, Lifecycle, ProcessError, ProcessResult, RouteResult};
use crate::node::{Filter, Router, Transform};
use crate::queue::RuntimeEnvelope;
use crate::runner::error_policy::WasmProcessError;

/// Type alias for the boxed transform processing function.
type TransformFn = Box<dyn Fn(&[u8]) -> std::result::Result<Vec<u8>, ProcessError> + Send>;
/// Type alias for the boxed routing function.
type RouteFn = Box<dyn Fn(&RuntimeEnvelope) -> Vec<String> + Send>;

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
    process_fn: TransformFn,
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

    /// Convenience: JSON temperature-extraction transform (matches the
    /// canonical Pipeline A json-parse Wasm plugin).
    #[must_use]
    pub fn json_parse(id: impl Into<String>) -> Self {
        Self::new(id, functions::json_parse)
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
                        retry_count: input.retry_count,
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

    /// Convenience: strict `>` threshold on `temperature`.
    ///
    /// Kept for existing call sites; new configs should use [`range`],
    /// which mirrors the WIT threshold-filter plugin exactly.
    #[must_use]
    pub fn threshold(id: impl Into<String>, threshold: f64) -> Self {
        Self::new(id, functions::threshold_filter(threshold))
    }

    /// Range filter matching `plugins/threshold-filter` semantics:
    /// forward when `value >= min && value <= max` on `field`, drop on
    /// missing / non-UTF8 / unparsable input.
    ///
    /// Used by `plugin = { kind = "native", function = "threshold" }` in
    /// the launcher; keep in sync with `plugins/threshold-filter/src/lib.rs`
    /// or `tests/native_threshold_filter.rs` will fail.
    #[must_use]
    pub fn range(
        id: impl Into<String>,
        field: impl Into<String>,
        min: f64,
        max: f64,
    ) -> Self {
        Self::new(id, functions::range_filter(field.into(), min, max))
    }

    /// Sync surface matching `WasmFilterNode::evaluate` so [`FilterNode`]
    /// dispatches uniformly. Native predicates cannot trap, hence the
    /// unused `WasmProcessError` slot.
    pub fn evaluate_sync(
        &mut self,
        envelope: &RuntimeEnvelope,
    ) -> std::result::Result<FilterOutcome, WasmProcessError> {
        Ok(if (self.predicate_fn)(envelope) {
            FilterOutcome::Forward
        } else {
            FilterOutcome::Drop
        })
    }

    /// Sync mirror of `Lifecycle::id` so [`FilterNode`] avoids the
    /// async lifecycle borrow.
    #[must_use]
    pub fn node_id(&self) -> &str {
        &self.id
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

// =============================================================================
// NativeRouter
// =============================================================================

/// Native Rust content-based router — same interface as Wasm router, zero
/// isolation overhead.
///
/// Given a routing function `Fn(&RuntimeEnvelope) -> Vec<String>`, the router
/// invokes it per envelope and multicasts to the returned port names. An
/// empty vec means drop; port names not in `ports` are silently ignored
/// (matching the WIT-guarded Wasm behaviour where the runtime discards
/// unknown-port routes).
pub struct NativeRouter {
    id: String,
    ports: Vec<String>,
    route_fn: RouteFn,
}

impl NativeRouter {
    /// Create with a custom routing function.
    pub fn new(
        id: impl Into<String>,
        ports: Vec<String>,
        route_fn: impl Fn(&RuntimeEnvelope) -> Vec<String> + Send + 'static,
    ) -> Self {
        Self { id: id.into(), ports, route_fn: Box::new(route_fn) }
    }

    /// Convenience: route by the first byte of the payload matching a
    /// prefix character to a port name; useful for the RFC-008 Pipeline A
    /// baseline where inputs already carry a type discriminator.
    ///
    /// `rules` maps each prefix byte → port name. Any envelope whose first
    /// byte is not in `rules` is dropped.
    #[must_use]
    pub fn by_first_byte(
        id: impl Into<String>,
        rules: Vec<(u8, String)>,
    ) -> Self {
        let ports: Vec<String> = rules.iter().map(|(_, p)| p.clone()).collect();
        Self::new(id, ports, move |env| {
            let Some(&first) = env.payload.first() else {
                return Vec::new();
            };
            rules
                .iter()
                .filter(|(b, _)| *b == first)
                .map(|(_, p)| p.clone())
                .collect()
        })
    }

    /// Convenience: content-router matching the `content-router` Wasm plugin.
    /// Routes envelopes with a `"level"` JSON field to `high` or `low` port
    /// based on the numeric value crossing `threshold`.
    #[must_use]
    pub fn content_router(
        id: impl Into<String>,
        threshold: f64,
        high_port: impl Into<String>,
        low_port: impl Into<String>,
    ) -> Self {
        let high = high_port.into();
        let low = low_port.into();
        let ports = vec![high.clone(), low.clone()];
        Self::new(id, ports, move |env| {
            let Ok(s) = std::str::from_utf8(&env.payload) else {
                return Vec::new();
            };
            match functions::extract_json_number(s, "level") {
                Some(v) if v >= threshold => vec![high.clone()],
                Some(_) => vec![low.clone()],
                None => Vec::new(),
            }
        })
    }
}

impl Lifecycle for NativeRouter {
    fn id(&self) -> &str {
        &self.id
    }

    fn node_type(&self) -> &'static str {
        "native-router"
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

impl Router for NativeRouter {
    fn output_ports(&self) -> Vec<String> {
        self.ports.clone()
    }

    fn route(
        &mut self,
        envelope: RuntimeEnvelope,
    ) -> Pin<Box<dyn Future<Output = Result<RouteResult>> + Send + '_>> {
        let ports = (self.route_fn)(&envelope);
        Box::pin(async move {
            ports.into_iter().next().map_or(
                Ok(RouteResult::Filter),
                |first| Ok(RouteResult::Route(first, envelope)),
            )
            }
        })
    }
}

// =============================================================================
// ProcessNode trait — sync boundary matching WasmTransformNode::process
// =============================================================================

/// Sync process-node contract that unifies WasmTransformNode and
/// NativeTransformShim under one trait for orchestrator wire-up.
///
/// This is the surface the transform runner loop consumes. The async
/// [`Transform`] trait above is the historical shape used by unit tests
/// and the async bench_pipeline harness; both surfaces coexist because
/// they answer different questions (async-friendly composition vs
/// isolation-tax measurement).
///
/// Wasm nodes carry hot-swap, reconfigure, and recover semantics.
/// Native nodes reject those with a stable error message so the runner
/// can log and continue with the untouched node.
pub trait ProcessNode: Send {
    /// Node identifier (matches `Config.nodes` key).
    fn node_id(&self) -> &str;

    /// Process one envelope and return the transformed envelope or a
    /// `WasmProcessError` variant so the runner can drive its state
    /// machine identically across Wasm and native.
    fn process(
        &mut self,
        envelope: RuntimeEnvelope,
    ) -> std::result::Result<RuntimeEnvelope, WasmProcessError>;

    /// Hot-swap: replace the underlying computation. Native nodes return
    /// `Err` so the API returns 400 to any hot-swap attempt on a native
    /// baseline (the baseline is by construction not swappable).
    fn try_apply_swap_payload(
        &mut self,
        _payload: &crate::runner::SwapPayload,
    ) -> Result<()> {
        Err(WaferError::Runtime(
            "native baseline nodes do not support hot-swap".into(),
        ))
    }

    /// Config reload. Native nodes reject the call.
    fn try_reconfigure(&mut self, _new_config_json: &str) -> Result<()> {
        Err(WaferError::Runtime(
            "native baseline nodes do not support reconfigure".into(),
        ))
    }

    /// Recovery via cached `InstancePre`. Native nodes are stateless
    /// pure functions; recovery is a no-op success.
    fn recover_from_cached_pre(&mut self) -> Result<()> {
        Ok(())
    }
}

impl ProcessNode for NativeTransform {
    fn node_id(&self) -> &str {
        &self.id
    }

    fn process(
        &mut self,
        envelope: RuntimeEnvelope,
    ) -> std::result::Result<RuntimeEnvelope, WasmProcessError> {
        match (self.process_fn)(&envelope.payload) {
            Ok(bytes) => {
                let mut out = envelope.clone();
                out.payload = Bytes::from(bytes);
                Ok(out)
            }
            Err(e) => {
                // Map the generic native ProcessError into a
                // WasmProcessError variant the runner already handles.
                // Retriable errors become ProcessingFailed (so retry
                // budget applies uniformly); non-retriable become
                // BadInput.
                if e.retriable {
                    Err(WasmProcessError::ProcessingFailed(e.message))
                } else {
                    Err(WasmProcessError::BadInput(e.message))
                }
            }
        }
    }
}

/// Native implementations matching Wasm plugin behavior.
pub mod functions {
    use super::{ProcessError, RuntimeEnvelope};

    /// ASCII uppercase (matches wafer-uppercase plugin).
    pub fn uppercase(payload: &[u8]) -> std::result::Result<Vec<u8>, ProcessError> {
        Ok(payload.to_ascii_uppercase())
    }

    /// Passthrough (matches pass-through plugin).
    pub fn passthrough(payload: &[u8]) -> std::result::Result<Vec<u8>, ProcessError> {
        Ok(payload.to_vec())
    }

    /// Strict `>` on `temperature`. Retained for `NativeFilter::threshold`.
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

    /// WIT-plugin-equivalent range predicate. Deviations break the RQ1
    /// apples-to-apples invariant (see `tests/native_threshold_filter.rs`).
    pub fn range_filter(
        field: String,
        min: f64,
        max: f64,
    ) -> impl Fn(&RuntimeEnvelope) -> bool + Send + 'static {
        move |envelope: &RuntimeEnvelope| {
            let Ok(s) = std::str::from_utf8(&envelope.payload) else {
                return false;
            };
            extract_json_number(s, &field).is_some_and(|v| v >= min && v <= max)
        }
    }

    fn extract_temperature(json: &str) -> Option<f64> {
        extract_json_number(json, "temperature")
    }

    /// Extract a numeric field from a flat JSON string, no serde dependency.
    ///
    /// Matches `"<field>"\s*:\s*<number>` and parses the number up to the
    /// first non-numeric character. Handles negative numbers and decimal
    /// points. Not a full JSON parser — mirrors the equivalent Wasm
    /// plugin logic (see plugins/threshold-filter and plugins/content-router).
    #[must_use]
    #[expect(clippy::arithmetic_side_effects, reason = "idx from find() + key.len() cannot exceed json.len()")]
    #[expect(clippy::string_slice, reason = "find-based indices guaranteed to be at char boundaries in ASCII JSON keys/numbers")]
    pub fn extract_json_number(json: &str, field: &str) -> Option<f64> {
        let key = format!("\"{field}\"");
        let idx = json.find(&key)?;
        let after_key = &json[idx + key.len()..];
        let after_colon = after_key.trim_start().strip_prefix(':')?;
        let value_str = after_colon.trim_start();
        let end = value_str
            .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
            .unwrap_or(value_str.len());
        value_str[..end].parse().ok()
    }

    /// Native json-parse transform (matches wafer-json-parse-like plugins).
    ///
    /// Parses a `"temperature"` field from the input JSON and re-emits a
    /// canonical single-field JSON object. Mirrors the Wasm plugin's
    /// behaviour of validating structure without allocating a full DOM.
    /// Returns a retriable ProcessError when the field is missing (so the
    /// error policy can decide) and a non-retriable one when the input
    /// isn't UTF-8.
    pub fn json_parse(payload: &[u8]) -> std::result::Result<Vec<u8>, ProcessError> {
        let s = std::str::from_utf8(payload).map_err(|e| ProcessError::new(
            "json.parse.non-utf8",
            format!("payload is not UTF-8: {e}"),
        ))?;
        let temp = extract_json_number(s, "temperature").ok_or_else(|| {
            ProcessError::new(
                "json.parse.missing-field",
                "payload has no numeric `temperature` field",
            )
            .retriable()
        })?;
        Ok(format!("{{\"temperature\":{temp}}}").into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn native_transform_uppercase() {
        let mut transform = NativeTransform::uppercase("test-upper");

        let input = RuntimeEnvelope::from_string("src", "hello world");
        let result = Transform::process(&mut transform, input).await.unwrap();

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
        let result = Transform::process(&mut transform, input).await.unwrap();

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
