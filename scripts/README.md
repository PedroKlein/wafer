# scripts

Repository-level checks. The evaluation harness lives in `eval/scripts/`.

| File | Role | Run by |
|---|---|---|
| `check-docs.sh` | Dependency versions and `crates/` paths in docs match the tree (`mise run check-docs`). | CI lint job |
| `check-commit-attribution.sh` | Rejects commits with machine identities or attribution trailers. | CI commits workflow |
| `glibc-floor.sh` | Prints the glibc version each release binary needs; `--max VERSION` fails on a newer one (`mise run glibc-floor`). | cross-build tasks, cross-arch CI |
| `load-test-metrics.sh` | Load-tests a running `/metrics` endpoint; defaults to `http://localhost:9091/metrics`, for example a runtime started with `--metrics-bind 127.0.0.1:9091`. | manual |
