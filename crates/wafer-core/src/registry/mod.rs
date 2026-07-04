//! Registry client for loading WASM components from OCI registries.

mod cache;
mod client;
mod types;

pub use cache::{CacheEntry, PackageCache, compute_hash};
pub use client::WaferRegistry;
pub use types::{OciReference, PluginSource, RegistryConfig, ResolvedPlugin};
