use serde::Deserialize;

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    #[serde(default)]
    pub inherit_stdio: bool,
    #[serde(default)]
    pub inherit_env: bool,
    #[serde(default)]
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
        Self { inherit_stdio: true, inherit_env: true, allow_inference: true }
    }

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
    fn sandbox_is_all_false() {
        let caps = Capabilities::sandbox();
        assert!(!caps.inherit_stdio);
        assert!(!caps.inherit_env);
        assert!(!caps.allow_inference);
    }

    #[test]
    fn with_stdio_enables_only_stdio() {
        let caps = Capabilities::with_stdio();
        assert!(caps.inherit_stdio);
        assert!(!caps.inherit_env);
        assert!(!caps.allow_inference);
    }

    #[test]
    fn full_enables_all() {
        let caps = Capabilities::full();
        assert!(caps.inherit_stdio);
        assert!(caps.inherit_env);
        assert!(caps.allow_inference);
    }

    #[test]
    fn builder_pattern() {
        let caps = Capabilities::sandbox().stdio(true).inference(true);
        assert!(caps.inherit_stdio);
        assert!(!caps.inherit_env);
        assert!(caps.allow_inference);
    }

    #[test]
    fn deserialize_from_toml_explicit() {
        let toml_str = r#"
            inherit_stdio = true
            allow_inference = true
        "#;
        let caps: Capabilities = toml::from_str(toml_str).unwrap();
        assert!(caps.inherit_stdio);
        assert!(!caps.inherit_env);
        assert!(caps.allow_inference);
    }

    #[test]
    fn deserialize_defaults_to_sandbox() {
        let caps: Capabilities = toml::from_str("").unwrap();
        assert!(!caps.inherit_stdio);
        assert!(!caps.inherit_env);
        assert!(!caps.allow_inference);
    }

    #[test]
    fn deserialize_rejects_unknown_fields() {
        let toml_str = r#"
            inherit_stdio = true
            gpu = true
        "#;
        let result: Result<Capabilities, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }
}
