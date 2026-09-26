# Experimental `wafer:pipeline@0.2.0` WIT

This directory contains the P1-T2 interface candidates for the isolated WASI P3 PoC. It does not replace or extend production `wafer:pipeline@0.1.0`.

## Selected payload shape

The vertical slice uses owned `list<u8>` payloads. An `envelope` carries:

- `id`, Unix-epoch nanosecond `timestamp`, `source`, and `content-type`;
- ordered, duplicate-capable `metadata: list<tuple<string, string>>`;
- `parent-id` and `trace-id` lineage;
- `retry-count`; and
- owned `payload: list<u8>`.

`transform-message-node` exports one native `async func`. `transform-stream-node` starts one finite session and returns an output stream plus a completion future. Each output stream element is either one owned envelope or one typed `process-error`; the completion future carries session-level success or failure.

This shape is not described as zero-copy. Canonical ABI lowering and lifting may copy payload bytes between host and guest memories. The measured PoC must count those copies rather than infer their absence from `stream<T>`.

## Ownership

Passing an envelope or stream transfers its contained values to the callee. The receiver owns strings, metadata, lineage strings, and payload bytes until it returns them or drops them. A stream writer owns each element until a successful write transfers it to the reader. Dropping either stream endpoint closes that direction; closing the input marks the finite input sequence complete. The returned completion future is owned by the host and must be resolved or dropped with the session.

No `borrow<T>` occurs in the selected package. Although the parser accepts a borrowed resource as an async input, that handle would remain live across suspension and needs lifecycle evidence that this task does not have; the selected shape therefore excludes it. P3 Filter and Router interfaces are deliberately not designed here: using this owned shape for them would require payload ownership transfer or a host-side preservation copy, so the existing P2 borrowed-buffer advantage is not assumed.

## Rejected probes

`ownership-probes/owned-buffer-resource` compiles, but ownership of a host-created resource moves into the guest. Returning it requires a guest-to-host owned handle transfer; dropping it releases the last handle. Fan-out requires one resource per branch or shared backing hidden behind host resource handles. Filter/Router forwarding also becomes resource-lifetime protocol, not the current call-scoped borrow.

`ownership-probes/separate-payload-stream` compiles, but each message couples an owned header to an independently owned byte stream. It can avoid materializing the entire payload at once, not prove zero-copy. It adds stream endpoints, chunk allocation/copy behavior, early-close handling, and header/payload pairing to every message; fan-out needs payload buffering or stream teeing. Filter/Router forwarding must retain or duplicate the unread stream.

Owned `list<u8>` is selected because it is the smallest interface whose host and Rust guest bindings compile for both one-message and finite-stream calls. Its known copy cost is preferable to introducing resource or nested-stream lifecycle policy before the vertical slice measures a benefit.

## Reproduce

Use only the P1-T1 pinned tools:

```sh
export DEVELOPER_DIR=/Library/Developer/CommandLineTools
export WASM_TOOLS=/path/to/pinned/wasm-tools-1.259.0
export WIT_BINDGEN=/path/to/pinned/wit-bindgen-0.62.0

out="$(mktemp -d)/generated"
experiments/p3/wit/probe.sh "$out"
cargo +1.98.1 check --manifest-path experiments/p3/wit/probes/host-bindings/Cargo.toml --locked
for feature in message stream owned-buffer payload-stream; do
  cargo +1.98.1 build \
    --manifest-path experiments/p3/wit/probes/guest-bindings/Cargo.toml \
    --target wasm32-wasip2 --no-default-features --features "$feature" --locked
done
```

The committed evidence under `experiments/p3/evidence/p1-t2/` records parsed WIT, generated binding shapes, host/guest compilation, component validation, production WIT hashes, and the terminology audit. The complete copy/allocation/drop decision is in the plan scratch directory's `ownership-decision.md`.
