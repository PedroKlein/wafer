# Contributing to WAFER

WAFER is the software artefact for an undergraduate thesis at UFRGS.
It is maintained primarily by the author, so review and merge times
will vary with academic deadlines. External contributions are still
welcome, especially bug reports, portability fixes, and clarifications
to the documentation.

## Before you open an issue or PR

Read enough of `docs/architecture/` to know whether your change fits
the design that the thesis defends. In particular:

- The runtime targets edge gateways, not cloud servers. Proposals that
  trade footprint for peak throughput will usually be declined.
- The plugin sandbox uses the Wasm Component Model. Changes to the
  WIT contracts under `wit/` need to keep the four package boundaries
  intact.
- The DAG topology is validated before execution. Runtime shapes that
  cannot be statically checked are out of scope.

## Local development

Install `mise` and let it fetch the pinned toolchains:

```sh
mise install
mise run setup
```

Then the standard loop is:

```sh
mise run fmt
mise run clippy
mise run test
```

If you touch the runtime hot path, run the criterion benches under
`crates/wafer-core/benches/` (e.g. `cargo bench -p wafer-core`) before
opening a PR. If you touch a Wasm plugin, run `mise run //plugins:build-*` for
that plugin and confirm the produced `.wasm` still loads.

## Pull request checklist

- The workspace builds clean: `mise run build` exits 0.
- Tests pass: `mise run test` exits 0.
- Clippy is clean at the project's configured level: `mise run clippy`.
- New behaviour has at least one test. Bug fixes have a regression test
  that fails before the fix and passes after.
- No `/Users/...` paths, no personal API tokens, no `.env` values.

## Agent-assisted contributions

Agent-assisted PRs are fine when they follow the same checklist as
human PRs. `.agents/AGENTS.md` describes the conventions this
repository uses when agents run against it. Please strip
`Co-authored-by:` trailers for AI agents before submitting; keep them
only for human co-authors.

## License

By contributing you agree that your contributions are licensed under
the MIT License (see `LICENSE`).
