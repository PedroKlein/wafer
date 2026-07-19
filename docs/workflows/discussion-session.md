# Discussion Session Workflow

> Recipe for conducting architecture discussion sessions in the WAFER Phase 0 refactor.
> Each session produces **decisions only** — no implementation code.
>
> **Status:** Phase 0 is COMPLETE (8 sessions done). This workflow is retained for
> reference and for any future design sessions that may be needed. The decision
> documents produced by these sessions now live under [`../rfcs/`](../rfcs/) as
> `RFC-NNN-<slug>.md` files.

---

## Session Structure (5 Phases)

### Phase 0: Deep Research (Silent — before any output)

**This is the most critical phase. Take ALL the time needed. No rushing.**

Read EVERYTHING relevant before producing any output:

```
1. Previous session decision docs (what's LOCKED — read completely)
   └── docs/rfcs/*.md  (formerly docs/decisions/*.md)

2. Current WAFER implementation (what exists TODAY)
   └── Read every file listed in the session prompt

3. Comparator source code (pi-repos references — read ACTUAL code)
   └── github.com/torvyn/torvyn (Wasm streaming runtime)
   └── github.com/tremor-rs/tremor-runtime (Rust DAG event processing)
   └── github.com/Rheosoph/flow-like (Wasm DAG workflow engine)
   └── github.com/infinyon/fluvio (SmartModules streaming)
   └── github.com/fermyon/spin (Component Model lifecycle)
   └── github.com/Azure-Samples/azure-edge-extensions-aio-dataflow-graphs
   └── github.com/lf-edge/ekuiper (IoT edge rules engine)
   └── github.com/candlecorp/wick (abandoned — lessons from failure)
   └── github.com/microsoft/wassette (industry validation)
   └── github.com/bytecodealliance/wasmtime (runtime internals)
   └── github.com/petgraph/petgraph (graph library patterns)

4. tcc-doc findings/ (deep source-code analysis already done)
   └── findings/source-code-comparators.md (9 systems analyzed)
   └── findings/torvyn-analysis.md
   └── Any other relevant findings files

5. Obsidian vault — academic papers and literature notes
   └── <home>/Dev/github.com/PedroKlein/tcc-doc/main/TCC/papers/
   └── <home>/Dev/github.com/PedroKlein/tcc-doc/main/TCC/software/
   └── <home>/Dev/github.com/PedroKlein/tcc-doc/main/TCC/systems/
   └── Search for keywords relevant to the session topic

6. tcc-doc research synthesis
   └── research/synthesis/ (citation leads, counter-arguments, patterns)
   └── research/analysis/ (evaluation plan, thesis statement)

7. Web research (current state of the art, 2024-2025 patterns)
   └── Search 3-4 varied queries per decision area
   └── Look for benchmarks, best practices, Rust ecosystem patterns

8. Example configs/code from 3+ external systems
   └── Read actual TOML/YAML/config files, not just docs
```

**Completion criteria for Phase 0:** You can articulate what 5+ systems do differently for each decision area, cite at least one academic source, and identify the key tensions.

---

### Phase 1: Landscape Presentation

**Announce:** *"Phase 1 — Here's what the landscape looks like for [topic area]."*

Present organized by concern area (NOT by decision number):
- **Academic foundations** — what theory/literature says (cite specific papers)
- **Industry practice** — comparison table across 5+ systems showing different approaches
- **Current WAFER state** — what we have today and why it needs to change
- **Key tensions** — the trade-offs that make this non-trivial

Format: Use tables for cross-system comparison. Use concise paragraphs for theory. Show actual code/config snippets from comparators when relevant.

**End with:** "Ready to discuss? Any area you want me to go deeper on before I propose decisions?"

**Wait for user response before proceeding.**

---

### Phase 2: Propose & Discuss (Iterative Loop)

**Announce:** *"Phase 2 — Here are my proposed decisions. All open for discussion and challenge."*

For each decision:
- Frame as: "**I propose:** [decision]. **Because:** [1-2 sentence rationale]. **Evidence:** [which systems validate this]."
- Include a rough code/config sketch
- Explicitly mark confidence: "high confidence" vs "open question — could go either way"

**Expect and welcome pushback.** When the user challenges:
1. Do NOT defend immediately — consider whether they're right
2. Do targeted research on their specific question
3. Present what you find (may support their direction or yours)
4. Iterate until alignment

**This phase loops.** It may go through 2-5 rounds of:
```
You propose → User challenges → You research deeper → You present findings → Discussion
```

The session's best decisions typically emerge from this loop, NOT from the initial proposal.

---

### Phase 3: Converge on Decisions

**Announce:** *"Phase 3 — Let me present the key decision forks for confirmation."*

Use `ask_user` tool for genuinely contested decisions where:
- There are 2+ viable options with real trade-offs
- The user's values/priorities determine the choice (not just technical merit)
- The decision has significant downstream impact

For obvious decisions (where research clearly points one way), just state them — don't force unnecessary interaction.

After user answers, summarize what's now locked.

---

### Phase 4: Deepen & Optimize

**Announce:** *"Phase 4 — With [key decisions] locked, here's what they enable."*

Explore:
- **Optimizations** unlocked by the strict choices made
- **Simplifications** that cascade (things we can remove, merge, or skip)
- **Edge cases** that need addressing given the decisions
- **Interactions** between decisions that create new opportunities

This phase often produces the most valuable insights because constraints enable creativity.

**User may redirect:** "What about X?" → research and present. Loop until exhausted.

---

### Phase 5: Impact Trace & Close

**Announce:** *"Phase 5 — Tracing impacts and writing up."*

Steps:
1. **Check previous sessions** — do any decisions here amend/contradict prior session docs?
2. **Check future sessions** — how do these decisions change the context for upcoming sessions?
3. **Update ROADMAP / task plan context** — mark completed decisions, adjust future-session context, and record cross-session dependencies in `ROADMAP.md` or the active `plan_tasks` plan
4. **Write RFC** — `docs/rfcs/RFC-NNN-<topic>.md` with full rationale, code sketches, sources consulted (see the RFC template in [`../rfcs/README.md`](../rfcs/README.md))
5. **Generate next session prompt** — incorporate all new context, copy to clipboard

---

## Anti-Patterns to Avoid

| Anti-Pattern | Instead Do |
|---|---|
| Presenting 8 "final decisions" before discussion | Present proposals as brainstorm-style, expect revision |
| Defaulting to "keep existing flexibility" | Challenge yourself: is the simpler/stricter option better? |
| Researching broadly but shallowly | Go deep on 5 key sources rather than skimming 20 |
| Wall of text before any interaction | Announce phases, pause between them, ask "ready to continue?" |
| Anchoring on first proposal | When user pushes back, genuinely consider their direction is better |
| Only citing implementations | Academic papers provide the WHY; implementations show the HOW |
| Skipping pi-repos comparators | These are locally cloned — read actual source code, not summaries |
| Forgetting to trace impacts | Always check: does this break or change anything from prior sessions? |

---

## What Makes a Good Decision Document

The output `docs/rfcs/RFC-NNN-*.md` should contain:
- **Header block** — status, original session date, `Amends:` / `Amended by:` cross-links
- **Abstract** — one paragraph, ≤ 200 words
- **Context** — why this session exists, what feeds into it
- **Decisions** — the choice, rationale (brief), code sketch, interactions
- **Alternatives Considered** — what was weighed and rejected
- **Related RFCs** — cross-links to sibling decisions
- **Implementation Notes** — how the code matches or diverges, cited to concrete files
- **(optional) Summary** — type hierarchy, data flow, or whatever the core mental model is

---

## Session Prompt Template

When generating the next session prompt, include:

```markdown
# Discussion N: [Topic]

## Context
[What previous sessions decided. What this session covers.]

## Before Starting the Discussion
[The "take all the time" instruction + full reading list]

## Decisions to Make
[Numbered list of open questions]

## Documents to Read
[Organized by: this repo, tcc-doc, pi-repos, obsidian vault, web research]

## Key Constraints
[Hard constraints from prior sessions that bound the design space]

## Output Format
[What the decisions should look like + any summary visualizations expected]
```

---

## How the User Drives the Session

The user's role:
- **Challenge proposals** — "should it really work this way?"
- **Redirect research** — "do more research on X specifically"
- **Simplify** — "do we even need this?"
- **Ask for depth** — "what optimizations does this unlock?"
- **Confirm direction** — via ask_user responses or explicit "that's good"
- **Signal phase transitions** — "anything else to explore?" = ready to close

The AI should watch for these signals and adjust pace accordingly.
