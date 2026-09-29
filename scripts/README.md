# scripts

Repository-level checks. The evaluation harness lives in `eval/scripts/`.

| File | Role | Run by |
|---|---|---|
| `check-docs.sh` | Dependency versions and `crates/` paths in docs match the tree (`mise run check-docs`). | CI lint job |
| `check-commit-attribution.sh` | Rejects commits with machine identities or attribution trailers. | CI commits workflow |
| `load-test-metrics.sh` | Load-tests the `/metrics` endpoint of the observability example. | manual (`examples/observability/`) |
