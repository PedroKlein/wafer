use std::num::NonZeroU64;

use serde::{Deserialize, Deserializer, Serialize};

use super::source_sink::{AuthConfig, TlsConfig};

/// Custom deserializer for metering fields: missing → None (unlimited),
/// positive integer → Some(NonZeroU64), zero → hard error.
///
/// Standard `Option<NonZeroU64>` silently maps `0` to `None` (TOML trait
/// semantics). We reject it instead so the P0.13 footgun stays impossible.
pub(crate) fn deserialize_metering_limit<'de, D>(deserializer: D) -> Result<Option<NonZeroU64>, D::Error>
where
    D: Deserializer<'de>,
{
    let v = Option::<u64>::deserialize(deserializer)?;
    match v {
        None => Ok(None),
        Some(0) => Err(serde::de::Error::custom(
            "metering value must not be 0 (would trap immediately); omit the field for unlimited",
        )),
        Some(n) => Ok(Some(NonZeroU64::new(n).unwrap())),
    }
}

/// Serialize helper: unwrap the NonZeroU64 back to a plain u64 for TOML.
pub(crate) fn serialize_metering_limit<S>(value: &Option<NonZeroU64>, serializer: S) -> Result<S::Ok, S::Error>
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
    /// NonZeroU64 prevents the `0 = trap immediately` footgun (P0.13 lesson:
    /// a typo or off-by-one silently makes every Wasm call trap on first
    /// epoch check, indistinguishable from a plugin bug).
    #[serde(default, deserialize_with = "deserialize_metering_limit", serialize_with = "serialize_metering_limit")]
    pub epoch_deadline: Option<NonZeroU64>,

    #[serde(default = "default_epoch_tick_ms")]
    pub epoch_tick_ms: u64,

    #[serde(default = "default_queue_capacity")]
    pub default_queue_capacity: usize,

    #[serde(default)]
    pub fuel: FuelBudgets,

    #[serde(default)]
    pub memory: MemoryLimits,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            epoch_deadline: None,
            epoch_tick_ms: default_epoch_tick_ms(),
            default_queue_capacity: default_queue_capacity(),
            fuel: FuelBudgets::default(),
            memory: MemoryLimits::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct FuelBudgets {
    /// Fuel per transform process() call. `None` = unlimited (set_fuel skipped).
    ///
    /// NonZeroU64: `0` would trap on the first fuel check (P0.13 lesson).
    #[serde(default, deserialize_with = "deserialize_metering_limit", serialize_with = "serialize_metering_limit")]
    pub transform: Option<NonZeroU64>,

    /// Fuel per filter apply() call. `None` = unlimited.
    #[serde(default, deserialize_with = "deserialize_metering_limit", serialize_with = "serialize_metering_limit")]
    pub filter: Option<NonZeroU64>,

    /// Fuel per router route() call. `None` = unlimited.
    #[serde(default, deserialize_with = "deserialize_metering_limit", serialize_with = "serialize_metering_limit")]
    pub router: Option<NonZeroU64>,
}

impl Default for FuelBudgets {
    fn default() -> Self {
        Self {
            transform: None,
            filter: None,
            router: None,
        }
    }
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

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Capabilities {
    #[serde(default)]
    pub inherit_stdio: bool,

    #[serde(default)]
    pub inherit_env: bool,

    #[serde(default)]
    pub allow_inference: bool,
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
    RetryConfig {
        retries: 3,
        backoff_ms: 100,
        exhausted: SimpleAction::Dlq,
    }
}

const fn default_processing_failed_retry() -> RetryConfig {
    RetryConfig {
        retries: 2,
        backoff_ms: 100,
        exhausted: SimpleAction::Dlq,
    }
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
