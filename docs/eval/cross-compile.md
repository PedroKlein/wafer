# Cross-compile for aarch64-linux (Pi / Jetson / ARM64 servers)

> **Status: shipped.** Path chosen: `docker run --platform linux/arm64` with
> a native `rust:1-slim-bookworm` image. Reproduced on macOS aarch64
> (M-series) host. Verified 2026-08-02.

The canonical Pi runs described in RFC-008 require WAFER binaries built
for `aarch64-unknown-linux-gnu`. This document records the three paths
we tried (per canonical-runs C1) and pins the one we ship.

## Ship path — `mise run cross-build-pi`

```bash
mise run cross-build-pi         # builds wafer, wafer-loadgen, waferctl
mise run cross-build-pi-check   # confirms `file` reports ARM aarch64
```

Under the hood the task runs a `linux/arm64` `rust:1-slim-bookworm`
container with the workspace bind-mounted at `/work` and an isolated
target directory at `target/docker-aarch64-linux/`. Because macOS
aarch64 hosts (M-series) support the `arm64` architecture natively,
Docker Desktop does NOT use qemu emulation — the container executes on
the same silicon as the host, so build times are comparable to a
bare-metal Pi.

The container installs `pkg-config`, `libssl-dev`, and `g++` (required
for `rustls_platform_verifier` and `-lstdc++` at link time), then runs
`cargo build --release -p wafer-runtime -p wafer-loadgen -p waferctl`
against the workspace as-is (no `Cross.toml`, no `rustup target add`).

Binary outputs land at:

- `target/docker-aarch64-linux/release/wafer`         (~80 MB)
- `target/docker-aarch64-linux/release/wafer-loadgen` (~8.4 MB)
- `target/docker-aarch64-linux/release/waferctl`      (~6.7 MB)

## Reproducing on a fresh clone

```bash
git clone <this-repo> && cd wafer-poc
mise install                # rust/uv/wasm-tools/etc.
mise run cross-build-pi     # cold-cache first run: ~11 minutes
mise run cross-build-pi-check
```

Confirm the outputs:

```bash
$ file target/docker-aarch64-linux/release/wafer
target/docker-aarch64-linux/release/wafer: ELF 64-bit LSB pie executable,
ARM aarch64, version 1 (GNU/Linux), dynamically linked, ...
```

## Paths we tried

### Path 1 — `rustup target add aarch64-unknown-linux-gnu` + host toolchain (REJECTED)

`rustup target add aarch64-unknown-linux-gnu` on macOS installs the
Rust std lib for the target but does NOT provide a Linux linker.
Attempting `cargo build --target aarch64-unknown-linux-gnu` fails with
`error: linking with 'cc' failed` because the macOS system linker
cannot produce ELF binaries.

Fixing this requires manually installing `aarch64-linux-gnu-gcc`
(homebrew) or an equivalent cross-linker AND pointing `cargo` at it
via `.cargo/config.toml` `[target.aarch64-unknown-linux-gnu] linker =`.
This is fragile (host toolchain drift) and rejected by the plan
constraints: *"prefer `cross` over homebrew linker (reproducibility)"*.

### Path 2 — `cross` crate (Docker; REJECTED here)

`cargo install cross` and then
`cross build --release --target aarch64-unknown-linux-gnu -p wafer-runtime`.

Result: fails inside the container with
`error: toolchain 'stable-x86_64-unknown-linux-gnu' may not be able to
run on this system`. The default `cross` container is `linux/amd64`;
it mounts `~/.rustup` from the macOS aarch64 host, and rustup refuses
to install `stable-x86_64-unknown-linux-gnu` because the mounted
rustup profile declares the host as `aarch64-apple-darwin`.

Working around this needs a `Cross.toml` that pins a preinstalled
image and passes `RUSTUP_HOME`/`CARGO_HOME` to isolate the container
rustup from the host mount. Achievable but adds a config file that
doesn't materially improve on Path 3.

### Path 3 — `docker run --platform linux/arm64 rust:slim-bookworm` (SHIPPED)

Directly invoke a `linux/arm64` Rust container. Docker Desktop on
macOS aarch64 uses the native arm64 architecture (no qemu emulation),
so this is essentially "compile inside a Pi-like environment on the
Mac". Binary produced links against `libc.so.6` (glibc) with
`interpreter /lib/ld-linux-aarch64.so.1`, matching a stock Debian /
Ubuntu / Raspbian Pi target.

Why we ship this over `cross`:

- **Reproducibility.** No `Cross.toml`; the `mise run cross-build-pi`
  recipe is entirely inline in `mise.toml` and reads self-explanatory
  in a fresh clone.
- **Zero host toolchain state.** The rustup + rust-toolchain.toml on
  the host are not consulted; the container has its own copy of
  `rust:1-slim-bookworm` (rustc 1.97.1 as of 2026-08-02).
- **Native perf on aarch64 hosts.** No qemu tax.
- **CI portability.** The same command works on GitHub Actions
  `ubuntu-latest` runners via `docker buildx` (linux/amd64 → linux/arm64
  emulated) or `ubuntu-24.04-arm` runners (native).

## CI integration (T10, thesis-hardening)

`.github/workflows/cross-arch.yml` runs `mise run cross-build-pi` on
every push to `main` and on pull requests. See the workflow file's
header for the runner selection rationale and the non-gating policy
(canonical-runs T10 depends on this recipe existing).

## Known non-issues

- **`ort` crate downloads onnxruntime binaries into
  `/root/.cache/ort.pyke.io/`.** Happens once at build time; the
  cached blobs are large (~50 MB) but not embedded in the final
  binary. Isolated inside the Docker container, does not pollute
  the host cache.
- **First build is slow (~11 minutes).** Subsequent builds reuse
  `target/docker-aarch64-linux/` and complete in under a minute for
  code-only changes.
- **`libstdc++` link step.** Solved by `apt-get install g++` inside
  the container. This drags in the C++ standard library needed for
  a subset of wasmtime's cranelift + zstd assembly optimizations.

## Files

- `mise.toml` — `[tasks.cross-build-pi]` + `[tasks.cross-build-pi-check]`
- `.github/workflows/cross-arch.yml` — CI integration
- `target/docker-aarch64-linux/` — build output directory (gitignored)
