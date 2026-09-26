use std::sync::Arc;

use wafer_types::config::{
    CanonicalHttpDestination, Capabilities as ConfigCapabilities, OutboundHttpDestinationError,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub inherit_stdio: bool,
    pub inherit_env: bool,
    pub allow_inference: bool,
    outbound_http: Arc<[CanonicalHttpDestination]>,
}

impl TryFrom<&ConfigCapabilities> for Capabilities {
    type Error = OutboundHttpDestinationError;

    fn try_from(config: &ConfigCapabilities) -> Result<Self, Self::Error> {
        let mut outbound_http = Vec::with_capacity(config.outbound_http.len());
        for destination in &config.outbound_http {
            let destination = destination.canonicalize()?;
            if outbound_http.contains(&destination) {
                return Err(OutboundHttpDestinationError::DuplicateDestination);
            }
            outbound_http.push(destination);
        }
        Ok(Self {
            inherit_stdio: config.inherit_stdio,
            inherit_env: config.inherit_env,
            allow_inference: config.allow_inference,
            outbound_http: outbound_http.into(),
        })
    }
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
        Self { inherit_stdio: true, inherit_env: true, ..Self::default() }
    }

    #[must_use]
    pub const fn stdio(mut self, enabled: bool) -> Self {
        self.inherit_stdio = enabled;
        self
    }

    #[must_use]
    pub const fn env(mut self, enabled: bool) -> Self {
        self.inherit_env = enabled;
        self
    }

    #[must_use]
    pub const fn inference(mut self, enabled: bool) -> Self {
        self.allow_inference = enabled;
        self
    }

    #[must_use]
    pub fn outbound_http(mut self, destinations: Vec<CanonicalHttpDestination>) -> Self {
        self.outbound_http = destinations.into();
        self
    }

    #[must_use]
    pub fn outbound_http_destinations(&self) -> &[CanonicalHttpDestination] {
        &self.outbound_http
    }

    pub(crate) fn outbound_http_grant(&self) -> Arc<[CanonicalHttpDestination]> {
        Arc::clone(&self.outbound_http)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wafer_types::config::{HttpHost, HttpScheme};

    #[test]
    fn sandbox_is_all_false() {
        let caps = Capabilities::sandbox();
        assert!(!caps.inherit_stdio);
        assert!(!caps.inherit_env);
        assert!(!caps.allow_inference);
        assert!(caps.outbound_http_destinations().is_empty());
    }

    #[test]
    fn with_stdio_enables_only_stdio() {
        let caps = Capabilities::with_stdio();
        assert!(caps.inherit_stdio);
        assert!(!caps.inherit_env);
        assert!(!caps.allow_inference);
        assert!(caps.outbound_http_destinations().is_empty());
    }

    #[test]
    fn full_enables_supported_capabilities() {
        let caps = Capabilities::full();
        assert!(caps.inherit_stdio);
        assert!(caps.inherit_env);
        assert!(!caps.allow_inference);
        assert!(caps.outbound_http_destinations().is_empty());
    }

    #[test]
    fn builders_preserve_independent_grants() {
        let destination = CanonicalHttpDestination {
            scheme: HttpScheme::Http,
            host: HttpHost::Ip(std::net::Ipv4Addr::LOCALHOST.into()),
            port: 8080,
        };
        let caps = Capabilities::sandbox()
            .stdio(true)
            .inference(true)
            .outbound_http(vec![destination.clone()]);

        assert!(caps.inherit_stdio);
        assert!(!caps.inherit_env);
        assert!(caps.allow_inference);
        assert_eq!(caps.outbound_http_destinations(), &[destination]);
    }
}
