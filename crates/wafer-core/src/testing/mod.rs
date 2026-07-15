//! Test utilities for WAFER pipeline integration and unit tests.
//!
//! Provides in-memory Source/Sink implementations (channel-backed) that
//! decouple tests from real I/O (MQTT, files, HTTP).

pub mod channel;

#[cfg(feature = "integration-tests")]
pub mod harness;

pub use channel::{ChannelSink, ChannelSource};

#[cfg(feature = "integration-tests")]
pub use harness::{PluginTestHarness, TransformHarness};
