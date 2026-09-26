use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::num::NonZeroU64;

use serde::{Deserialize, Deserializer, Serialize};

use super::source_sink::{AuthConfig, TlsConfig};

/// Custom deserializer for metering fields: missing → None (unlimited),
/// positive integer → `Some(NonZeroU64)`, zero → hard error.
///
/// Standard `Option<NonZeroU64>` silently maps `0` to `None` (TOML trait
/// semantics). We reject it instead so the P0.13 footgun stays impossible.
pub(super) fn deserialize_metering_limit<'de, D>(
    deserializer: D,
) -> Result<Option<NonZeroU64>, D::Error>
where
    D: Deserializer<'de>,
{
    // Reject `0` explicitly (would trap the Wasm call immediately — P0.13).
    // `NonZeroU64::new(0)` returns None, which we translate to the same error
    // so there's a single source of truth for the rejection.
    let Some(n) = Option::<u64>::deserialize(deserializer)? else {
        return Ok(None);
    };
    NonZeroU64::new(n).map(Some).ok_or_else(|| {
        serde::de::Error::custom(
            "metering value must not be 0 (would trap immediately); omit the field for unlimited",
        )
    })
}

/// Serialize helper: extract the inner value of `NonZeroU64` back to a plain `u64` for TOML.
#[expect(
    clippy::ref_option,
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's serialize_with contract requires fn(&T, S) -> Result<...>"
)]
pub(super) fn serialize_metering_limit<S>(
    value: &Option<NonZeroU64>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match value {
        None => serializer.serialize_none(),
        Some(n) => serializer.serialize_u64(n.get()),
    }
}

pub(super) const fn default_mqtt_port() -> u16 {
    1883
}

pub(super) const fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EngineConfig {
    /// Epoch ticks before a Wasm call traps. `None` = no epoch interrupt.
    ///
    /// `NonZeroU64` prevents the `0 = trap immediately` footgun (P0.13 lesson:
    /// a typo or off-by-one silently makes every Wasm call trap on first
    /// epoch check, indistinguishable from a plugin bug).
    #[serde(
        default,
        deserialize_with = "deserialize_metering_limit",
        serialize_with = "serialize_metering_limit"
    )]
    pub epoch_deadline: Option<NonZeroU64>,

    #[serde(default = "default_epoch_tick_ms")]
    pub epoch_tick_ms: u64,

    #[serde(default = "default_queue_capacity")]
    pub default_queue_capacity: usize,

    #[serde(default)]
    pub fuel: FuelBudgets,

    #[serde(default)]
    pub memory: MemoryLimits,

    #[serde(default)]
    pub hot_swap: HotSwapConfig,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            epoch_deadline: None,
            epoch_tick_ms: default_epoch_tick_ms(),
            default_queue_capacity: default_queue_capacity(),
            fuel: FuelBudgets::default(),
            memory: MemoryLimits::default(),
            hot_swap: HotSwapConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct FuelBudgets {
    /// Fuel per transform `process()` call. `None` = unlimited (`set_fuel` skipped).
    ///
    /// `NonZeroU64`: `0` would trap on the first fuel check (P0.13 lesson).
    #[serde(
        default,
        deserialize_with = "deserialize_metering_limit",
        serialize_with = "serialize_metering_limit"
    )]
    pub transform: Option<NonZeroU64>,

    /// Fuel per filter `apply()` call. `None` = unlimited.
    #[serde(
        default,
        deserialize_with = "deserialize_metering_limit",
        serialize_with = "serialize_metering_limit"
    )]
    pub filter: Option<NonZeroU64>,

    /// Fuel per router `route()` call. `None` = unlimited.
    #[serde(
        default,
        deserialize_with = "deserialize_metering_limit",
        serialize_with = "serialize_metering_limit"
    )]
    pub router: Option<NonZeroU64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MemoryLimits {
    #[serde(default = "default_memory_transform")]
    pub transform: usize,

    #[serde(default = "default_memory_filter")]
    pub filter: usize,

    #[serde(default = "default_memory_router")]
    pub router: usize,
}

impl Default for MemoryLimits {
    fn default() -> Self {
        Self {
            transform: default_memory_transform(),
            filter: default_memory_filter(),
            router: default_memory_router(),
        }
    }
}

/// Configuration for process-time hot-swap rollback (A17).
///
/// After a swap succeeds (ACK phase), the runner retains a rollback snapshot
/// of v1 for a bounded canary window. If v2 traps during `process()` within
/// that window, the runtime automatically rolls back to v1.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HotSwapConfig {
    /// Number of consecutive successful `process()` calls on v2 required
    /// before the rollback snapshot is dropped. Default: 32.
    #[serde(default = "default_canary_success_count")]
    pub canary_success_count: u32,

    /// Maximum wall-clock milliseconds the rollback snapshot is retained
    /// after swap ACK. Default: `10_000` (10s).
    #[serde(default = "default_canary_window_ms")]
    pub canary_window_ms: u64,

    /// Maximum number of rollback retries before escalating to the
    /// Recovery state. Default: 3.
    #[serde(default = "default_max_rollback_retries")]
    pub max_rollback_retries: u32,
}

impl Default for HotSwapConfig {
    fn default() -> Self {
        Self {
            canary_success_count: default_canary_success_count(),
            canary_window_ms: default_canary_window_ms(),
            max_rollback_retries: default_max_rollback_retries(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ErrorPolicyConfig {
    #[serde(default = "default_bad_input_action")]
    pub bad_input: SimpleAction,

    #[serde(default = "default_dependency_failed_retry")]
    pub dependency_failed: RetryConfig,

    #[serde(default = "default_processing_failed_retry")]
    pub processing_failed: RetryConfig,

    #[serde(default = "default_timed_out_action")]
    pub timed_out: SimpleAction,

    #[serde(default = "default_retry_buffer_capacity")]
    pub retry_buffer_capacity: usize,
}

impl Default for ErrorPolicyConfig {
    fn default() -> Self {
        Self {
            bad_input: default_bad_input_action(),
            dependency_failed: default_dependency_failed_retry(),
            processing_failed: default_processing_failed_retry(),
            timed_out: default_timed_out_action(),
            retry_buffer_capacity: default_retry_buffer_capacity(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SimpleAction {
    Skip,
    Dlq,
    Teardown,
}

impl std::fmt::Display for SimpleAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Skip => write!(f, "skip"),
            Self::Dlq => write!(f, "dlq"),
            Self::Teardown => write!(f, "teardown"),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RetryConfig {
    #[serde(default = "default_retries")]
    pub retries: u32,

    #[serde(default = "default_backoff_ms")]
    pub backoff_ms: u64,

    #[serde(default = "default_exhausted_action")]
    pub exhausted: SimpleAction,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            retries: default_retries(),
            backoff_ms: default_backoff_ms(),
            exhausted: default_exhausted_action(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorCategory {
    BadInput,
    DependencyFailed,
    ProcessingFailed,
    TimedOut,
    Unrecoverable,
}

impl std::fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadInput => write!(f, "bad-input"),
            Self::DependencyFailed => write!(f, "dependency-failed"),
            Self::ProcessingFailed => write!(f, "processing-failed"),
            Self::TimedOut => write!(f, "timed-out"),
            Self::Unrecoverable => write!(f, "unrecoverable"),
        }
    }
}

/// Queue overflow policy when a downstream node cannot keep up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum OverflowPolicy {
    #[default]
    Slow,
    Drop,
    DeadLetter,
}

impl std::fmt::Display for OverflowPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Slow => write!(f, "slow"),
            Self::Drop => write!(f, "drop"),
            Self::DeadLetter => write!(f, "dead-letter"),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DeadLetterConfig {
    Mqtt {
        broker: String,

        #[serde(default = "default_mqtt_port")]
        port: u16,

        topic: String,

        #[serde(default = "default_dlq_mqtt_capacity")]
        queue_capacity: usize,

        #[serde(default)]
        tls: Option<TlsConfig>,

        #[serde(default)]
        auth: Option<AuthConfig>,
    },
    File {
        path: String,

        #[serde(default = "default_dlq_file_capacity")]
        queue_capacity: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HttpScheme {
    Http,
    Https,
}

impl HttpScheme {
    #[must_use]
    pub const fn default_port(self) -> u16 {
        match self {
            Self::Http => 80,
            Self::Https => 443,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundHttpDestination {
    pub scheme: HttpScheme,
    pub host: String,
    #[serde(default)]
    pub port: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HttpHost {
    Dns(Box<str>),
    Ip(IpAddr),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CanonicalHttpDestination {
    pub scheme: HttpScheme,
    pub host: HttpHost,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OutboundHttpDestinationError {
    #[error("host must not be empty")]
    EmptyHost,
    #[error("wildcard hosts are not supported")]
    WildcardHost,
    #[error("host must be a canonical IP literal or ASCII DNS name")]
    InvalidHost,
    #[error("IP literal is prohibited")]
    ProhibitedIp,
    #[error("port must be greater than zero")]
    ZeroPort,
    #[error("duplicate normalized destination")]
    DuplicateDestination,
}

impl OutboundHttpDestination {
    /// Normalize the destination identity used by configuration validation and request policy.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed hosts, wildcard hosts, prohibited IP literals, or port zero.
    pub fn canonicalize(&self) -> Result<CanonicalHttpDestination, OutboundHttpDestinationError> {
        if self.host.is_empty() {
            return Err(OutboundHttpDestinationError::EmptyHost);
        }
        if self.host.contains('*') {
            return Err(OutboundHttpDestinationError::WildcardHost);
        }
        let host = match self.host.parse::<IpAddr>() {
            Ok(ip) if permitted_literal_ip(ip) => HttpHost::Ip(ip),
            Ok(_) => return Err(OutboundHttpDestinationError::ProhibitedIp),
            Err(_) => HttpHost::Dns(canonical_dns_name(&self.host)?.into_boxed_str()),
        };
        let port = self.port.unwrap_or_else(|| self.scheme.default_port());
        if port == 0 {
            return Err(OutboundHttpDestinationError::ZeroPort);
        }
        Ok(CanonicalHttpDestination { scheme: self.scheme, host, port })
    }
}

fn canonical_dns_name(host: &str) -> Result<String, OutboundHttpDestinationError> {
    if !host.is_ascii() || host.len() > 253 || host.ends_with('.') {
        return Err(OutboundHttpDestinationError::InvalidHost);
    }
    let valid = host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    });
    if !valid {
        return Err(OutboundHttpDestinationError::InvalidHost);
    }
    Ok(host.to_ascii_lowercase())
}

#[must_use]
pub const fn permitted_literal_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => permitted_literal_ipv4(ip),
        IpAddr::V6(ip) => permitted_literal_ipv6(ip),
    }
}

#[must_use]
pub const fn permitted_dns_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => global_ipv4(ip),
        IpAddr::V6(ip) => global_ipv6(ip),
    }
}

const fn permitted_literal_ipv4(ip: Ipv4Addr) -> bool {
    ip.is_loopback() || ip.is_private() || global_ipv4(ip)
}

const fn global_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, d] = ip.octets();
    !(a == 0
        || a == 10
        || a == 127
        || (a == 169 && b == 254)
        || (a == 172 && b >= 16 && b <= 31)
        || (a == 192 && b == 168)
        || (a == 100 && b >= 64 && b <= 127)
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224
        || (a == 255 && b == 255 && c == 255 && d == 255))
}

const fn permitted_literal_ipv6(ip: Ipv6Addr) -> bool {
    ip.is_loopback() || ip.segments()[0] & 0xfe00 == 0xfc00 || global_ipv6(ip)
}

const fn global_ipv6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    segments[0] & 0xe000 == 0x2000
        && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
        && ip.to_ipv4_mapped().is_none()
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Capabilities {
    #[serde(default)]
    pub inherit_stdio: bool,

    #[serde(default)]
    pub inherit_env: bool,

    #[serde(default)]
    pub allow_inference: bool,

    #[serde(default)]
    pub outbound_http: Vec<OutboundHttpDestination>,
}

const fn default_epoch_tick_ms() -> u64 {
    10
}

const fn default_queue_capacity() -> usize {
    1024
}

const fn default_memory_transform() -> usize {
    64 * 1024 * 1024
}

const fn default_memory_filter() -> usize {
    16 * 1024 * 1024
}

const fn default_memory_router() -> usize {
    16 * 1024 * 1024
}

const fn default_bad_input_action() -> SimpleAction {
    SimpleAction::Dlq
}

const fn default_timed_out_action() -> SimpleAction {
    SimpleAction::Skip
}

const fn default_dependency_failed_retry() -> RetryConfig {
    RetryConfig { retries: 3, backoff_ms: 100, exhausted: SimpleAction::Dlq }
}

const fn default_processing_failed_retry() -> RetryConfig {
    RetryConfig { retries: 2, backoff_ms: 100, exhausted: SimpleAction::Dlq }
}

const fn default_retry_buffer_capacity() -> usize {
    1000
}

const fn default_retries() -> u32 {
    3
}

const fn default_backoff_ms() -> u64 {
    100
}

const fn default_exhausted_action() -> SimpleAction {
    SimpleAction::Dlq
}

const fn default_dlq_mqtt_capacity() -> usize {
    10_000
}

const fn default_dlq_file_capacity() -> usize {
    5000
}

const fn default_canary_success_count() -> u32 {
    32
}

const fn default_canary_window_ms() -> u64 {
    10_000
}

const fn default_max_rollback_retries() -> u32 {
    3
}

#[cfg(test)]
mod outbound_http_tests {
    use super::*;

    #[test]
    fn destination_normalizes_dns_and_default_port() {
        let destination = OutboundHttpDestination {
            scheme: HttpScheme::Https,
            host: "API.Example.COM".to_string(),
            port: None,
        }
        .canonicalize();

        assert_eq!(
            destination,
            Ok(CanonicalHttpDestination {
                scheme: HttpScheme::Https,
                host: HttpHost::Dns("api.example.com".into()),
                port: 443,
            })
        );
    }

    #[test]
    fn literal_policy_allows_explicit_private_but_denies_link_local() {
        assert!(permitted_literal_ip(Ipv4Addr::LOCALHOST.into()));
        assert!(permitted_literal_ip(Ipv4Addr::new(10, 0, 0, 1).into()));
        assert!(!permitted_literal_ip(Ipv4Addr::new(169, 254, 169, 254).into()));
        assert!(!permitted_literal_ip(Ipv4Addr::LOCALHOST.to_ipv6_mapped().into()));
    }

    #[test]
    fn prohibited_ip_literals_cannot_be_granted() {
        for host in [
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "169.254.1.1",
            "::",
            "ff02::1",
            "fe80::1",
            "::ffff:127.0.0.1",
        ] {
            let result = OutboundHttpDestination {
                scheme: HttpScheme::Http,
                host: host.to_string(),
                port: Some(8080),
            }
            .canonicalize();
            assert_eq!(result, Err(OutboundHttpDestinationError::ProhibitedIp), "{host}");
        }
    }

    #[test]
    fn dns_policy_accepts_only_global_addresses() {
        assert!(permitted_dns_ip(Ipv4Addr::new(93, 184, 216, 34).into()));
        assert!(!permitted_dns_ip(Ipv4Addr::LOCALHOST.into()));
        assert!(!permitted_dns_ip(Ipv4Addr::new(10, 0, 0, 1).into()));
        assert!(!permitted_dns_ip(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 1).into()));
    }
}
