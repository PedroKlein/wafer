//! Wasmtime [`bindgen!`] outputs for the three WAFER pipeline worlds.
//!
//! Each world (transform-node, filter-node, router-node) gets its own submodule.
//! The first invocation (`transform_node`) generates the canonical
//! `pipeline:types/types` interface bindings; subsequent invocations redirect
//! to that module via `with:` so `WaferBuffer` impls are deduplicated and
//! `WaferState` only needs one `HostBuffer` implementation.
//!
//! Directory layout:
//!   - `wit/node/` — main package `pipeline:node` (transform-node + filter-node worlds)
//!   - `wit/router/` — main package `pipeline:routing` (router-node world)
//!   - Each has `deps/` subdirectories for cross-package references.
//!
//! [`bindgen!`]: wasmtime::component::bindgen

/// Bindings for the `transform-node` world (package `pipeline:node`).
///
/// This is the "canonical" invocation that generates the `pipeline:types/types`
/// host trait definitions. Other worlds redirect to these types via `with:`.
pub(crate) mod transform_node {
    wasmtime::component::bindgen!({
        path: "wit/node",
        world: "transform-node",
        with: {
            "pipeline:types/types.buffer": crate::engine::WaferBuffer,
        },
    });
}

/// Bindings for the `filter-node` world (package `pipeline:node`).
///
/// Reuses `pipeline:types/types` from the transform-node bindings via `with:`.
pub(crate) mod filter_node {
    wasmtime::component::bindgen!({
        path: "wit/node",
        world: "filter-node",
        with: {
            "pipeline:types/types": super::transform_node::pipeline::types::types,
            "pipeline:host/logging": super::transform_node::pipeline::host::logging,
        },
    });
}

/// Bindings for the `router-node` world (package `pipeline:routing`).
///
/// Reuses `pipeline:types/types` and `pipeline:node/lifecycle` from previous bindings.
pub(crate) mod router_node {
    wasmtime::component::bindgen!({
        path: "wit/router",
        world: "router-node",
        with: {
            "pipeline:types/types": super::transform_node::pipeline::types::types,
            "pipeline:host/logging": super::transform_node::pipeline::host::logging,
            "pipeline:node/lifecycle": super::transform_node::exports::pipeline::node::lifecycle,
        },
    });
}

/// Discriminator for which world a compiled component targets.
///
/// Used by `WaferEngine` to create the correct `InstancePre` variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WasmBindings {
    Transform,
    Filter,
    Router,
}

impl std::fmt::Display for WasmBindings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transform => write!(f, "transform-node"),
            Self::Filter => write!(f, "filter-node"),
            Self::Router => write!(f, "router-node"),
        }
    }
}
