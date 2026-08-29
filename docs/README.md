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
- **Preparing the Raspberry Pi 5 evaluation host?** →
  [`eval/pi5-host-setup.md`](eval/pi5-host-setup.md), then
  [`eval/pi5-experiment-runbook.md`](eval/pi5-experiment-runbook.md).
- **Investigating a design decision?** →
  [`rfcs/`](rfcs/) for long-form and [`adr/`](adr/) for short summaries.
- **Checking what is implemented today?** →
  [`status/implementation-status.md`](status/implementation-status.md).

## Directory tree

```
docs/
├── architecture/         arc42 §1–8 + comparators (Explanation)
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
│   ├── functional.md
│   └── non-functional.md
├── interfaces/           WIT / HTTP / TOML / SDK (Reference)
│   ├── wit-contracts.md
│   ├── http-api.md
│   ├── config-schema.md
│   └── plugin-sdk.md
├── adr/                  Nygard-format decision records
├── rfcs/                 Long-form design records
├── operations/           Task-oriented how-tos (How-to)
│   ├── getting-started.md          (Tutorial)
│   ├── configuration.md
│   ├── mqtt-setup.md
│   ├── registry.md
│   ├── observability.md
│   └── dependencies.md
├── status/               Current state of the world (Reference)
│   ├── implementation-status.md
│   └── evaluation-progress.md
├── benchmarks/           Measurement reports
│   └── hot-swap.md
├── workflows/            Session recipes (Explanation)
│   ├── discussion-session.md
│   ├── implementation-session.md
│   └── planning-session.md
├── api/                  Machine-readable API artefacts
│   ├── openapi.yaml
│   └── bruno-collection/
└── history/plans/        Archived planning scratchpads (Historical)
```

## Reader profiles

- **Thesis reviewer** — read `architecture/00-vision.md`,
  `architecture/07-quality-requirements.md`, `architecture/09-comparators.md`,
  and skim `rfcs/README.md` for the amendments graph.
- **Edge-gateway operator** — start with
  `operations/getting-started.md`, then work through
  `operations/configuration.md` for your pipeline shape, then
  `operations/observability.md` for scraping metrics.
- **Plugin author** — `interfaces/wit-contracts.md`,
  `interfaces/plugin-sdk.md`, and the concrete example in
  `plugins/pass-through/src/lib.rs`.
- **Runtime contributor** — walk `architecture/03-building-blocks.md`,
  `architecture/06-crosscutting-concepts.md`, then the relevant
  RFC(s) under `rfcs/`. Load the domain skills under
  `.agents/skills/` when editing code.

## Historical note (2026-07-18 refactor)

The previous monolithic layout (`docs/SPEC.md`, `docs/MVP.md`,
`docs/api.md`, `docs/REGISTRY.md`, `docs/decisions/`) has been split
into the arc42-lite tree above. Every legacy topic maps to a section
in the new tree:

| Legacy location | New home |
|-----------------|----------|
| `SPEC.md` §Architecture / §WIT contracts / §Node categories | `architecture/`, `interfaces/wit-contracts.md`, `interfaces/config-schema.md`. |
| `SPEC.md` §Hot-swap mechanism | `architecture/04-runtime-view.md`, `adr/0003-hot-swap-mechanism.md`, `adr/0012-watch-channel-hot-swap.md`, `rfcs/RFC-005-orchestrator.md`. |
| `MVP.md` | `status/implementation-status.md`. |
| `api.md` | `interfaces/http-api.md`. |
| `REGISTRY.md` | `operations/registry.md`. |
| `decisions/` | `rfcs/` (renamed and harmonised into `RFC-NNN-<slug>.md`). |

Legacy files are removed; git history preserves them if needed.

## Cross-repository pointers

- `github.com/PedroKlein/tcc-doc` — thesis writing, evaluation plan
  and RQ definitions.
- `github.com/PedroKlein/obsidian-personal` — literature notes under
  `TCC/`.
- `.agents/AGENTS.md` — how humans and agents collaborate in this
  repository.
