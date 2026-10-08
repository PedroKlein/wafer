# WAFER Documentation

Navigator for the WAFER documentation tree. The tree follows the
arc42-lite framework, split into subdirectories by reader intent.

## Where to start

- **Never used WAFER before?** →
  [`operations/getting-started.md`](operations/getting-started.md).
- **Want the mental model in one page?** →
  [`architecture/00-vision.md`](architecture/00-vision.md).
- **Configuring a real pipeline?** →
  [`operations/configuration.md`](operations/configuration.md) and
  [`interfaces/config-schema.md`](interfaces/config-schema.md).
- **Writing a plugin?** →
  [`interfaces/wit-contracts.md`](interfaces/wit-contracts.md) and
  [`interfaces/plugin-sdk.md`](interfaces/plugin-sdk.md).
- **Preparing an evaluation host?** →
  [`eval/pi5-host-setup.md`](eval/pi5-host-setup.md) for the canonical
  Raspberry Pi 5, [`eval/jetson-host-setup.md`](eval/jetson-host-setup.md)
  and [`eval/x86-host-setup.md`](eval/x86-host-setup.md) for the replication
  hosts, then [`eval/pi5-experiment-runbook.md`](eval/pi5-experiment-runbook.md).
- **Investigating a design decision?** →
  [`rfcs/`](rfcs/) for long-form and [`adr/`](adr/) for short summaries.
- **Checking what is implemented today?** →
  [`status/implementation-status.md`](status/implementation-status.md)
  and the drift ledger
  [`status/implementation-gaps.md`](status/implementation-gaps.md).
- **Learning the code from source?** →
  [`learn/README.md`](learn/README.md).

## Directory tree

```
docs/
├── README.md             This navigator
├── architecture/         arc42 §1-8 + comparators (Explanation)
│   ├── README.md
│   ├── 00-vision.md
│   ├── 01-goals-and-constraints.md
│   ├── 02-solution-strategy.md
│   ├── 03-building-blocks.md
│   ├── 04-runtime-view.md
│   ├── 05-deployment.md
│   ├── 06-crosscutting-concepts.md
│   ├── 07-quality-requirements.md
│   ├── 08-risks.md
│   └── 09-comparators.md
├── requirements/         Functional + non-functional (Reference)
│   ├── README.md
│   ├── functional.md
│   └── non-functional.md
├── interfaces/           WIT / HTTP / TOML / SDK (Reference)
│   ├── README.md
│   ├── wit-contracts.md
│   ├── http-api.md
│   ├── config-schema.md
│   └── plugin-sdk.md
├── learn/                Source-guided learning path (Tutorial / Explanation)
│   ├── README.md
│   ├── workspace-map.md
│   ├── rust-in-context.md
│   ├── tokio-in-context.md
│   ├── config-to-running-pipeline.md
│   ├── message-through-wasm.md
│   ├── wasmtime-in-context.md
│   ├── plugin-boundary.md
│   ├── shutdown-and-failure.md
│   ├── stateless-hot-swap.md
│   ├── evaluation-harness.md
│   └── reading-paths.md
├── adr/                  Nygard-format decision records (0001-0018)
├── rfcs/                 Long-form design records (RFC-001 to RFC-012)
├── operations/           Task-oriented how-tos (How-to)
│   ├── README.md
│   ├── getting-started.md          (Tutorial)
│   ├── configuration.md
│   ├── mqtt-setup.md
│   ├── registry.md
│   ├── observability.md
│   └── dependencies.md
├── eval/                 Evaluation host setup and runbooks (How-to)
│   ├── pi5-host-setup.md
│   ├── jetson-host-setup.md
│   ├── x86-host-setup.md
│   ├── pi5-experiment-runbook.md
│   └── cross-compile.md
├── status/               Current state of the world (Reference)
│   ├── README.md
│   ├── implementation-status.md
│   ├── implementation-gaps.md
│   ├── canonical-readiness.md
│   └── rpi5-canonical-transition.md
├── benchmarks/           Measurement notes; no final results yet
│   ├── README.md
│   ├── hot-swap.md
│   ├── ekuiper-comparator.md
│   └── ekuiper-profile-diagnostic.md
├── api/                  Machine-readable API artefacts
│   ├── README.md
│   ├── openapi.yaml
│   └── bruno-collection/
└── history/              Archive, not current: plans, pre-final status logs,
                          closed gap entries, shakedown benchmark pages,
                          Phase 0 session recipes
```

Files under `history/` open with an "Archived, not current" banner. They
explain how the project got here; they do not describe the runtime or the
evaluation as they are today.

## Reader profiles

- **Thesis reviewer**: read `architecture/00-vision.md`,
  `architecture/07-quality-requirements.md`, `architecture/09-comparators.md`,
  and skim `rfcs/README.md` for the amendments graph. To check a claim
  against the source, follow the numbered first-read order in
  `learn/README.md`.
- **Edge-gateway operator**: start with
  `operations/getting-started.md`, then work through
  `operations/configuration.md` for your pipeline shape, then
  `operations/observability.md` for scraping metrics.
- **Plugin author**: `interfaces/wit-contracts.md`,
  `interfaces/plugin-sdk.md`, and the concrete example in
  `plugins/pass-through/src/lib.rs`.
- **Runtime contributor**: start with `learn/workspace-map.md` for the
  crate and module map and the build, run, and test commands, and use the
  first-read order in `learn/README.md` to learn the code. Then walk
  `architecture/03-building-blocks.md`,
  `architecture/06-crosscutting-concepts.md`, and the relevant
  RFC(s) under `rfcs/`. Load the domain skills under
  `.agents/skills/` when editing code.

## Cross-repository pointers

- `github.com/PedroKlein/tcc-doc`: thesis writing, evaluation plan
  and RQ definitions.
- `github.com/PedroKlein/obsidian-personal`: literature notes under
  `TCC/`.
- `AGENTS.md`: how humans and agents collaborate in this
  repository.
