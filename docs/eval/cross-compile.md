# Build release binaries for the evaluation hosts

The evaluation hosts run `wafer`, `wafer-loadgen` and `waferctl` release
binaries built for `aarch64-unknown-linux-gnu` (Raspberry Pi 5, Jetson) or
`x86_64-unknown-linux-gnu`. This page shows how to build them and how to check
that a binary will start on its target.

## aarch64: `mise run cross-build-pi`

```bash
mise run cross-build-pi         # builds wafer, wafer-loadgen, waferctl
mise run cross-build-pi-check   # confirms `file` reports ARM aarch64
```

The task needs Docker. It runs a `linux/arm64`
`rust:<toolchain>-slim-bookworm` container, with the tag read from
`rust-toolchain.toml` so the image matches the pinned toolchain. The workspace
is mounted at `/work` and the target directory at
`target/docker-aarch64-linux/`. The container installs `pkg-config`,
`libssl-dev` and `g++` (the link step needs `libstdc++`), then runs
`cargo build --locked --release -p wafer-runtime -p wafer-loadgen -p waferctl`.
The binaries land in `target/docker-aarch64-linux/release/`.

On an arm64 machine (such as an Apple silicon Mac) the container runs
natively. On x86_64 Docker emulates arm64 with QEMU, which is much slower. The
first build takes several minutes even natively; later builds reuse
`target/docker-aarch64-linux/`.

The build needs network access: the container installs packages with
`apt-get`, and the default `ort-download` feature downloads the pinned ONNX
Runtime archive (cached inside the container, not in the host's
`~/.cache`). The task does not pass `ORT_LIB_LOCATION` into the container, so
for an offline build compile natively on an aarch64 host with
`ORT_LIB_LOCATION` set (see
[ONNX Runtime](../operations/dependencies.md#onnx-runtime)).

## Which glibc a binary needs

Every release binary links `libc.so.6` dynamically, and the linker records
the newest glibc symbol version it used. That version comes from the build
environment, not the target: a binary from the bookworm container needs
glibc 2.36, one from the `ubuntu-24.04-arm` CI runner needs 2.39. A binary
fails to start with `version GLIBC_2.xx not found` on any host whose glibc is
older.

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

`cross-build-pi`, `build-release-x86` and both CI jobs print this report after
every build. The Rust code alone needs glibc 2.34; the C and C++ objects
linked into `wafer` (the prebuilt ONNX Runtime and crates that compile C on
the build host) raise its floor to the build environment's glibc. So neither
the container build nor the CI build of `wafer` starts on a Jetson. Build on
the Jetson itself (install Rust with rustup; the toolchain comes from
`rust-toolchain.toml`) and check with `--max 2.35`. If `wafer` still needs a
newer glibc, the prebuilt ONNX Runtime is the cause: point `ORT_LIB_LOCATION`
at an ONNX Runtime built on the device.

## x86_64 Linux

```bash
mise run build-release-x86    # native build on an x86_64 Linux host
```

The task builds the same three binaries with `cargo build --locked --release`,
checks that `file` reports `x86-64`, and prints the glibc report. The
binaries land in `target/release/`.

## CI builds

`.github/workflows/cross-arch.yml` builds the three binaries natively on an
`ubuntu-24.04-arm` runner and on `ubuntu-latest`, on pushes and pull requests
to `main` that change the crates, Cargo files, `rust-toolchain.toml`,
`mise.toml` or the glibc script. It uploads them as
`wafer-aarch64-linux-<sha>` and `wafer-x86_64-linux-<sha>`.
