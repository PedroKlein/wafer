# Cross-compile for aarch64-linux (Pi / Jetson / ARM64 servers)

> **Status: shipped.** Path chosen: `docker run --platform linux/arm64` with
> a native `rust:<toolchain>-slim-bookworm` image. Reproduced on macOS aarch64
> (M-series) host. Verified 2026-08-02.

The canonical Pi runs described in RFC-008 require WAFER binaries built
for `aarch64-unknown-linux-gnu`. This document records the three paths
we tried (per canonical-runs C1) and pins the one we ship.

## Ship path — `mise run cross-build-pi`

```bash
mise run cross-build-pi         # builds wafer, wafer-loadgen, waferctl
mise run cross-build-pi-check   # confirms `file` reports ARM aarch64
```

Under the hood the task runs a `linux/arm64` `rust:1.98.1-slim-bookworm`
container (the tag is read from `rust-toolchain.toml`, so the image always
matches the pinned toolchain) with the workspace bind-mounted at `/work` and an isolated
target directory at `target/docker-aarch64-linux/`. Because macOS
aarch64 hosts (M-series) support the `arm64` architecture natively,
Docker Desktop does NOT use qemu emulation — the container executes on
the same silicon as the host, so build times are comparable to a
bare-metal Pi.

The container installs `pkg-config`, `libssl-dev`, and `g++` (required
for `rustls_platform_verifier` and `-lstdc++` at link time), then runs
`cargo build --locked --release -p wafer-runtime -p wafer-loadgen -p waferctl`
against the workspace as-is (no `Cross.toml`, no `rustup target add`).

Binary outputs land at:

- `target/docker-aarch64-linux/release/wafer`         (~80 MB)
- `target/docker-aarch64-linux/release/wafer-loadgen` (~8.4 MB)
- `target/docker-aarch64-linux/release/waferctl`      (~6.7 MB)

## Which glibc a binary needs

Every release binary links `libc.so.6` dynamically, and the linker records
the newest glibc symbol version it used. That version is set by the glibc of
the build environment, not by the target: a binary from the bookworm
container needs glibc 2.36, one from the `ubuntu-24.04-arm` CI runner needs
2.39. It fails to start with `version GLIBC_2.xx not found` on any host
whose glibc is older.

| Host | OS | glibc |
|------|----|-------|
| Raspberry Pi 5 | Raspberry Pi OS (Debian 13) | 2.41 |
| Jetson Orin Nano | L4T R36 (Ubuntu 22.04) | 2.35 |
| x86_64 workstation | Ubuntu 22.04 / 24.04 | 2.35 / 2.39 |

`scripts/glibc-floor.sh` prints the version each binary requires and, with
`--max`, fails when a binary needs a newer glibc than the host has:

```bash
mise run glibc-floor -- target/docker-aarch64-linux/release/wafer
mise run glibc-floor -- --max 2.35 target/docker-aarch64-linux/release/wafer
```

`cross-build-pi` and both CI jobs print the report after every build. The
first CI report (2026-09-28, `ubuntu-24.04-arm`) was `wafer: glibc 2.39`,
`wafer-loadgen: glibc 2.34`, `waferctl: glibc 2.34`: the Rust code itself
needs only 2.34, and the extra requirement in `wafer` comes from the C and
C++ objects linked into it (the prebuilt ONNX Runtime and the crates that
compile C on the build host). So the CI aarch64 `wafer` does not start on a
Jetson. For a Jetson, build natively on the device (install Rust with
rustup; the toolchain comes from `rust-toolchain.toml`) so those objects link
against the device's own glibc 2.35, then run the check with `--max 2.35`.
If `wafer` still needs a newer glibc after a native build, the prebuilt ONNX
Runtime is the remaining source and the build needs `ORT_LIB_LOCATION`
pointing at an ONNX Runtime built on the device (see
[ONNX Runtime](../operations/dependencies.md#onnx-runtime)).

## x86_64 Linux

```bash
mise run build-release-x86    # native build on an x86_64 Linux host
```

The task runs `cargo build --locked --release` for the same three binaries,
checks that `file` reports `x86-64`, and prints the glibc report. Outputs land
under `target/release/`. CI builds the same set on `ubuntu-latest` in the
`build-x86` job of `cross-arch.yml` and uploads them as
`wafer-x86_64-linux-<sha>`.

## Reproducing on a fresh clone

```bash
git clone <this-repo> && cd wafer
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
  the host are not consulted; the container image carries the toolchain
  pinned in `rust-toolchain.toml`. (Until 2026-09 the task used the floating
  `rust:1-slim-bookworm` tag, which resolved to rustc 1.97.1 on 2026-08-02.)
- **Native perf on aarch64 hosts.** No qemu tax.
- **CI portability.** The same command works on GitHub Actions
  `ubuntu-latest` runners via `docker buildx` (linux/amd64 → linux/arm64
  emulated) or `ubuntu-24.04-arm` runners (native).

## CI integration

`.github/workflows/cross-arch.yml` builds the same three binaries with
`cargo build --locked --release` on a native `ubuntu-24.04-arm` runner and on
`ubuntu-latest` (x86_64), each with a cached cargo target, on every push to
`main` and on pull requests that touch Rust code. It does not use the Docker recipe: emulating arm64 with
QEMU made a cold build take most of the job's time budget. The job is not
a required check.

## Known non-issues

- **`ort` crate downloads onnxruntime binaries into
  `/root/.cache/ort.pyke.io/`.** Happens once at build time; the archive
  is pinned and hash-checked (see
  [ONNX Runtime](../operations/dependencies.md#onnx-runtime)) and linked
  statically into `wafer`. The cache lives inside the Docker container and
  does not pollute the host cache. The download (and the container's
  `apt-get`) need network access; `mise run cross-build-pi` does not forward
  `ORT_LIB_LOCATION` into the container, so offline builds use a native
  aarch64 build with `ORT_LIB_LOCATION` set instead (same section).
- **First build is slow (~11 minutes).** Subsequent builds reuse
  `target/docker-aarch64-linux/` and complete in under a minute for
  code-only changes.
- **`libstdc++` link step.** Solved by `apt-get install g++` inside
  the container. This drags in the C++ standard library needed for
  a subset of wasmtime's cranelift + zstd assembly optimizations.

## Files

- `mise.toml` — `[tasks.cross-build-pi]`, `[tasks.cross-build-pi-check]`,
  `[tasks.build-release-x86]`, `[tasks.glibc-floor]`
- `scripts/glibc-floor.sh` — glibc requirement report and floor check
- `docs/eval/jetson-host-setup.md`, `docs/eval/x86-host-setup.md` — host
  expectations checked by `eval/scripts/preflight-jetson.sh` and
  `eval/scripts/preflight-x86.sh`
- `.github/workflows/cross-arch.yml` — CI integration
- `target/docker-aarch64-linux/` — build output directory (gitignored)
