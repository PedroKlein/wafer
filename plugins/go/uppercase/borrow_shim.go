// borrow_shim.go — release input `borrow<buffer>` handle for the guest.
//
// # Why this file exists
//
// The pinned wit-bindgen-go generator that produced `gen/` (v0.2.x era,
// compatible with `go.bytecodealliance.org/cm@v0.3.0`) does not distinguish
// `borrow<T>` from an owned resource in generated Go types: it lifts the
// incoming `borrow<buffer>` in a `message` record as an owned
// `types.Buffer` handle. The Component Model runtime therefore tracks the
// handle as an active resource for the duration of the call; if the guest
// does not release it before returning, wasmtime aborts with:
//
//     Unrecoverable("borrow handles still remain at the end of the call")
//
// This shim releases the borrow explicitly. Modern wit-bindgen-go (post
// v0.4.0-ish) emits this release automatically as part of the generated
// record-lifting code. Upgrading requires reconciling the go.bytecodealliance.org
// module rename and the `cm.Reinterpret[cm.Rep]` vs `types.Buffer(cm.Resource)`
// type mismatch that currently blocks every published wit-bindgen-go release
// after v0.3.0 (see plugins/go/uppercase/README.md).
//
// # How to remove this shim
//
// When `plugins/go/uppercase/gen/` is regenerated with a wit-bindgen-go
// version that emits automatic borrow release:
//
//   1. Delete this file.
//   2. Delete the `defer releaseInputBorrow(...)` call in process().
//   3. Run `cargo test -p wafer-core --test polyglot_go_uppercase`.
//
// If step 3 fails with the "borrow handles still remain" trap, the shim was
// still required — the regen did not fix the underlying issue. Restore the
// files.
//
// If step 3 fails after the shim is removed with any variant of "resource
// already dropped" / "invalid resource handle" / a wasmtime trap during
// process(), it means the shim was performing a double-drop and MUST be
// removed. In that case, keep the removal and rerun.
//
// The polyglot integration test in
// `crates/wafer-core/tests/polyglot_go_uppercase.rs` is the gate: it will
// fail loudly on either failure mode above (missing drop OR double drop).
package main

import (
	"github.com/PedroKlein/wafer/plugins/go/uppercase/gen/wafer/pipeline/types"
)

// releaseInputBorrow releases the borrow<buffer> handle the host passed in
// with the input Message. See file-level comment for full rationale.
//
// This call is a no-op when the generator emits automatic release. In that
// case it becomes a double-drop and the integration tests will fail.
func releaseInputBorrow(buf types.Buffer) {
	buf.ResourceDrop()
}
