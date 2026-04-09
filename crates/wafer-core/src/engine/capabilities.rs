//! Security capabilities for WASM plugins.
//!
//! Controls what system resources a plugin can access.
//! By default, plugins get minimal capabilities (sandbox mode).
//!
//! # Example
//!
//! ```
//! use wafer_core::engine::Capabilities;
//!
//! // Minimal sandbox (default)
//! let caps = Capabilities::sandbox();
//!
//! // Allow stdio for debugging
//! let caps = Capabilities::with_stdio();
//!
//! // Builder pattern for custom capabilities
//! let caps = Capabilities::sandbox()
//!     .stdio(true)
//!     .inference(true);
//! ```
//!
//! # MVP Limitations
//!
//! Currently only `inherit_stdio`, `inherit_env`, and `allow_inference` are functional.
//! The `allow_network` and `allow_filesystem` fields are **placeholders**
//! for future capability-based security and have no effect in this MVP.

/// Capability configuration for WASM plugins.
///
/// Controls what system resources a plugin can access.
/// By default, plugins get minimal capabilities (sandbox mode).
#[derive(Debug, Clone, Default)]
pub struct Capabilities {
    /// Allow inheriting stdin/stdout/stderr from the host process.
    pub inherit_stdio: bool,
    /// Allow access to environment variables.
    pub inherit_env: bool,
    /// Allow network access.
    ///
    /// **MVP: Placeholder only - has no effect.**
    /// Future: will support specific hosts/ports allowlist.
    pub allow_network: bool,
    /// Allow filesystem access.
    ///
    /// **MVP: Placeholder only - has no effect.**
    /// Future: will support specific paths allowlist.
    pub allow_filesystem: bool,
    /// Allow machine learning inference via wasi-nn.
    pub allow_inference: bool,
}

impl Capabilities {
    /// Create default capabilities (minimal sandbox).
    #[must_use]
    pub fn sandbox() -> Self {
        Self::default()
    }

    /// Create capabilities that inherit stdio (for debugging).
    #[must_use]
    pub fn with_stdio() -> Self {
        Self { inherit_stdio: true, ..Self::default() }
    }

    /// Create full capabilities (for trusted plugins).
    #[must_use]
    pub fn full() -> Self {
        Self {
            inherit_stdio: true,
            inherit_env: true,
            allow_network: true,
            allow_filesystem: true,
            allow_inference: true,
        }
    }

    /// Enable stdio inheritance (builder pattern).
    #[must_use]
    pub fn stdio(mut self, enabled: bool) -> Self {
        self.inherit_stdio = enabled;
        self
    }

    /// Enable environment variable access (builder pattern).
    #[must_use]
    pub fn env(mut self, enabled: bool) -> Self {
        self.inherit_env = enabled;
        self
    }

    /// Enable network access (builder pattern).
    ///
    /// **MVP: Placeholder only - has no effect.**
    #[must_use]
    pub fn network(mut self, enabled: bool) -> Self {
        self.allow_network = enabled;
        self
    }

    /// Enable filesystem access (builder pattern).
    ///
    /// **MVP: Placeholder only - has no effect.**
    #[must_use]
    pub fn filesystem(mut self, enabled: bool) -> Self {
        self.allow_filesystem = enabled;
        self
    }

    /// Enable machine learning inference via wasi-nn (builder pattern).
    #[must_use]
    pub fn inference(mut self, enabled: bool) -> Self {
        self.allow_inference = enabled;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sandbox_is_minimal() {
        let caps = Capabilities::sandbox();
        assert!(!caps.inherit_stdio);
        assert!(!caps.inherit_env);
        assert!(!caps.allow_network);
        assert!(!caps.allow_filesystem);
        assert!(!caps.allow_inference);
    }

    #[test]
    fn test_with_stdio() {
        let caps = Capabilities::with_stdio();
        assert!(caps.inherit_stdio);
        assert!(!caps.inherit_env);
    }

    #[test]
    fn test_full_capabilities() {
        let caps = Capabilities::full();
        assert!(caps.inherit_stdio);
        assert!(caps.inherit_env);
        assert!(caps.allow_network);
        assert!(caps.allow_filesystem);
        assert!(caps.allow_inference);
    }

    #[test]
    fn test_builder_pattern() {
        let caps = Capabilities::sandbox().stdio(true).inference(true);

        assert!(caps.inherit_stdio);
        assert!(!caps.inherit_env);
        assert!(caps.allow_inference);
    }
}
