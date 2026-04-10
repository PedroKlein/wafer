//! Security capabilities for WASM plugins.
//!
//! Currently only `inherit_stdio`, `inherit_env`, and `allow_inference` are functional.
//! `allow_network` and `allow_filesystem` are placeholders for future use.

/// Capability configuration for WASM plugins.
#[derive(Debug, Clone, Default)]
pub struct Capabilities {
    pub inherit_stdio: bool,
    pub inherit_env: bool,
    /// **MVP: Placeholder only - has no effect.**
    pub allow_network: bool,
    /// **MVP: Placeholder only - has no effect.**
    pub allow_filesystem: bool,
    pub allow_inference: bool,
}

impl Capabilities {
    #[must_use]
    pub fn sandbox() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_stdio() -> Self {
        Self { inherit_stdio: true, ..Self::default() }
    }

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

    #[must_use]
    pub fn env(mut self, enabled: bool) -> Self {
        self.inherit_env = enabled;
        self
    }

    /// **MVP: Placeholder only - has no effect.**
    #[must_use]
    pub fn network(mut self, enabled: bool) -> Self {
        self.allow_network = enabled;
        self
    }

    /// **MVP: Placeholder only - has no effect.**
    #[must_use]
    pub fn filesystem(mut self, enabled: bool) -> Self {
        self.allow_filesystem = enabled;
        self
    }

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
