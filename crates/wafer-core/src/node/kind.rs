//! `NodeKind` names the closed set of node roles.
//!
//! The launcher and hot-swap path use it to pick each role's fuel budget.

/// The closed set of pipeline node roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// Wasm transform: 1→1 message transformation.
    Transform,
    /// Wasm filter: pure predicate, borrow-only, zero-copy.
    Filter,
    /// Wasm router: 1→N content-based routing.
    Router,
    /// Native source: produces messages into the pipeline.
    Source,
    /// Native sink: consumes messages from the pipeline.
    Sink,
}
