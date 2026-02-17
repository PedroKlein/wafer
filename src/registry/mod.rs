//! Registry client for loading WASM components from OCI registries.
//!
//! This module provides support for fetching transform plugins from
//! remote registries like ghcr.io using the wasm-pkg-client.

mod types;

pub use types::{PackageRef, PluginSource, RegistryConfig, ResolvedPlugin};
