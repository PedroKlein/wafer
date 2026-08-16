# Go plugin — uppercase transform

TinyGo implementation of the `transform-node-go` world (WAFER polyglot demo).

## Build

```
mise run //plugins:build-plugin-go
```

Produces `wafer-uppercase-go.wasm`.

## Verifying the polyglot boundary

```
cargo test -p wafer-core --test polyglot_go_uppercase
```

Loads the built `.wasm` through the real `WaferEngine` transform loop and
asserts an end-to-end round-trip. This is the **gating test** for host ↔ Wasm
compatibility — see `crates/wafer-core/tests/polyglot_go_uppercase.rs`.

## Regenerating Go bindings (`gen/`) — read this before running the regen task

The `gen/` directory contains generated Go bindings that are **checked in** and
must stay in sync with the canonical WIT under `../../../wit/`. It was
originally produced by a `wit-bindgen-go` v0.2.x-era generator whose output is
compatible with the pinned `go.bytecodealliance.org/cm@v0.3.0`. Every published
`wit-bindgen-go` version from v0.4.0 through v0.7.0 emits code that fails to
compile against `cm@v0.3.0` (uses `cm.Rep`-typed handles that can't assign to
the `type Buffer cm.Resource` alias in the same generated output). This is a
genuine upstream fragmentation, not something we can fix in-repo. The umbrella
`go.bytecodealliance.org@v0.7.0` still depends on `cm@v0.3.0`, so no
compatible pair exists as of this writing.

Two paths for future regeneration:

1. **Preferred — full modernization.** When upstream ships a `wit-bindgen-go`
   whose output compiles against a shipped `cm`, update `go.mod` (potentially
   migrating the module path from `go.bytecodealliance.org/cm` to the umbrella
   `go.bytecodealliance.org`), re-run `mise run //plugins:plugin-go-generate`,
   remove `borrow_shim.go`, and rerun the polyglot test suite.

2. **Fallback — hand-patch after regen.** Regenerate via option (1), then patch
   the two WIT-boundary sites that changed shape:
   - `gen/wafer/pipeline/lifecycle/{abi.go, lifecycle.wasm.go, lifecycle.wit.go}`
     if the `node-config` field count changes.
   - `gen/wafer/pipeline/types/types.wit.go` if `type Buffer` changes shape.

## `borrow_shim.go` and the enforcement gate

`borrow_shim.go` releases the input `borrow<buffer>` handle at the end of every
`process()` call because the v0.2.x generator does not emit that release
automatically. Modern generators do — when the shim is no longer needed, it
becomes a double-drop.

Both failure modes are caught by `crates/wafer-core/tests/polyglot_go_uppercase.rs`:

| Failure mode | How it surfaces |
|---|---|
| Shim removed prematurely | `Unrecoverable("borrow handles still remain at the end of the call")` from wasmtime — three of the five polyglot tests fail. |
| Shim redundant (generator now emits its own release) | `Unrecoverable("...wasm backtrace...wasmexport_Process...")` trap on double resource-table delete — same three tests fail. |
| Shim doing its job | All five tests pass. |

This makes the shim self-verifying: any change to the generator that alters
borrow handling will surface as a hard CI failure with an actionable
diagnostic. When regenerating, run the polyglot test suite as your acceptance
gate; if it goes green after removing the shim, the shim is genuinely obsolete
and the removal is safe to commit. If it goes red, restore the shim and
document what changed.

The fault-injection procedure used to validate this enforcement is captured in
the sentinel test `go_uppercase_double_drop_diagnostic_is_recognizable`, which
pins the expected error variant shape so the diagnostic can't silently drift.
