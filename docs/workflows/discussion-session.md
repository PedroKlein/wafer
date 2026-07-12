# Discussion Session Workflow

> Recipe for conducting architecture discussion sessions in the WAFER Phase 0 refactor.
> Each session produces **decisions only** — no implementation code.
> 
> **Status:** Phase 0 is COMPLETE (8 sessions done). This workflow is retained for reference
> and for any future design sessions that may be needed.

---

## Session Structure (5 Phases)

### Phase 0: Deep Research (Silent — before any output)

**This is the most critical phase. Take ALL the time needed. No rushing.**

Read EVERYTHING relevant before producing any output:

```
1. Previous session decision docs (what's LOCKED — read completely)
   └── docs/decisions/*.md

2. Current WAFER implementation (what exists TODAY)
   └── Read every file listed in the session prompt

3. Comparator source code (pi-repos references — read ACTUAL code)
   └── <home>/Dev/pi-repos/repos/github.com/torvyn/torvyn/main
   └── <home>/Dev/pi-repos/repos/github.com/tremor-rs/tremor-runtime/main
   └── <home>/Dev/pi-repos/repos/github.com/Rheosoph/flow-like/dev
   └── <home>/Dev/pi-repos/repos/github.com/infinyon/fluvio/master
   └── <home>/Dev/pi-repos/repos/github.com/fermyon/spin/main
   └── <home>/Dev/pi-repos/repos/github.com/Azure-Samples/azure-edge-extensions-aio-dataflow-graphs/main
   └── <home>/Dev/pi-repos/repos/github.com/lf-edge/ekuiper/master
   └── <home>/Dev/pi-repos/repos/github.com/candlecorp/wick/main
   └── <home>/Dev/pi-repos/repos/github.com/microsoft/wassette/main
   └── <home>/Dev/pi-repos/repos/github.com/bytecodealliance/wasmtime/main
   └── <home>/Dev/pi-repos/repos/github.com/petgraph/petgraph/master

4. tcc-doc (thesis research + findings)
   └── <home>/Dev/github.com/PedroKlein/tcc-doc/main/findings/
   └── <home>/Dev/github.com/PedroKlein/tcc-doc/main/research/

5. Obsidian vault — academic papers and literature notes
   └── <home>/Dev/github.com/PedroKlein/tcc-doc/main/TCC/papers/
   └── <home>/Dev/github.com/PedroKlein/tcc-doc/main/TCC/software/
   └── <home>/Dev/github.com/PedroKlein/tcc-doc/main/TCC/systems/
```

**Completion criteria:** Can articulate what 5+ systems do differently for each decision area.

---

### Phase 1: Landscape Presentation
### Phase 2: Propose & Discuss (Iterative Loop)
### Phase 3: Converge on Decisions
### Phase 4: Deepen & Optimize
### Phase 5: Impact Trace & Close

(See full details in the original workflow — abbreviated here since Phase 0 design is complete.)

---

## Anti-Patterns

| Anti-Pattern | Instead Do |
|---|---|
| Presenting "final decisions" before discussion | Present proposals as brainstorm-style, expect revision |
| Defaulting to "keep existing flexibility" | Challenge: is the simpler/stricter option better? |
| Researching broadly but shallowly | Go deep on 5 key sources rather than skimming 20 |
| Wall of text before any interaction | Announce phases, pause between them |
| Skipping pi-repos comparators | Read actual source code, not just summaries |
