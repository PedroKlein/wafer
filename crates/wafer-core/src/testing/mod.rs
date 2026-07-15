//! Test utilities for WAFER pipeline integration and unit tests.
//!
//! Provides in-memory Source/Sink implementations (channel-backed) that
//! decouple tests from real I/O (MQTT, files, HTTP).

pub mod channel;

pub use channel::{ChannelSink, ChannelSource};
