//! Registry client for loading WASM components from OCI registries.
//!
//! This module provides support for fetching transform plugins from
//! remote registries like ghcr.io using direct OCI image references.

mod cache;
mod client;
mod types;

pub use cache::{compute_hash, CacheEntry, PackageCache};
pub use client::WaferRegistry;
pub use types::{OciReference, PluginSource, RegistryConfig, ResolvedPlugin};
