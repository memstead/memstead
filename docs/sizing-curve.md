# Sizing curve — measured operating limits

**Measured:** 2026-08-06 · engine at commit `c48cd07` (post-0.4.0, pre-0.5) ·
Apple M5 Max (macOS, aarch64), release binary. **All numbers are
hardware-relative** — treat the shape of the curve as portable and the
absolute milliseconds as this machine's. The field deployment that
motivated this measurement ran on different hardware and saw ~0.5 ms/entity;
this machine sits in the same band.

The engine's MCP instructions describe a mem as "designed for 1,000–5,000
entities". Until this document, that span was advertised, not measured
(plenum channel, finding 10). This page states what the four everyday
operations actually cost across workspace sizes, so the deferred redesigns
that wait for numbers (incremental derived-structure maintenance — and
lazy mounts plus deferred cross-mem targets, since landed and recorded
below) can be argued from data.

## Reproducing

One command generates the graded synthetic workspaces, measures, and writes
machine-readable results:

```
cargo run -p xtask -- sizing-curve
```

Defaults: sizes `500,2500,5000,7500`, 3 iterations per operation, results
to `target/sizing-curve.json` (`format: "sizing-curve/v1"`, per-run
samples included). Override with `--sizes`, `--iterations`, `--output`.
The harness builds the release binary itself, drives everything through
the product surface (`memstead mem-repo init` → `mem init` →
`batch-create`), and runs each workspace in a temp directory that is
deleted afterwards — no residue in the repo, no effect on the test suite.

To diff after an engine change: rerun, then compare the JSON
(`diff <(jq . old.json) <(jq . target/sizing-curve.json)` or any JSON
diff) — medians per operation per size are the contract.

## Method

Synthetic corpora, self-contained: `spec` entities under the builtin
`default` schema — the harness pins no version, so a run takes whatever
the binary's default is (`default@1.3.0` today) — three prose sections each, rotating `level`
metadata, two explicit edges (USES / DEPENDS_ON) to earlier entities plus
one body wiki-link (alias-emitting REFERENCES) — edge density ~3/entity,
following the flavour of the largest real deployment without depending on
it. Backend: git-branch (mem-repo), the backend the field pain was
measured on. Each operation is a fresh `memstead` process — the **cold
CLI path**, where cost was reported; a warm MCP server pays boot once at
startup and is out of scope here.

The four operations, timed spawn-to-exit, median of 3:

| Operation | Command | What it pays |
|---|---|---|
| boot | `memstead list --limit 1` | process + engine boot + full workspace load |
| update | `memstead update <id> --auto-hash --append …` | boot + read + write commit + index invalidation |
| search | `memstead search <term>` (right after the update) | boot + search-index rebuild + query |
| overview | `memstead overview` | boot + community/summary path |

## The curve

Median milliseconds per operation (full per-run samples in the JSON):

| Entities | Generation | Boot | Update | Search | Overview | Boot ms/entity |
|---:|---:|---:|---:|---:|---:|---:|
| 500 | 233 | 181 | 182 | 185 | 181 | 0.36 |
| 2,500 | 1,048 | 1,162 | 1,166 | 1,179 | 1,163 | 0.46 |
| 5,000 | 2,918 | 3,043 | 3,071 | 3,044 | 3,040 | 0.61 |
| 7,500 | 4,739 | 5,647 | 5,656 | 5,639 | 5,640 | 0.75 |

Two findings carry the whole page:

1. **Every cold operation costs the same as boot.** At every size, update,
   search-after-mutation, and overview sit within noise of the plain boot
   (±10 ms at 7,500 against a 5,600 ms baseline). Index rebuild, the
   mutation commit, and community detection are invisible next to loading
   the workspace. On the cold CLI path there is exactly one cost: **load**.
2. **Per-entity load cost grows super-linearly.** 15× the entities costs
   31× the time (0.36 → 0.75 ms/entity from 500 → 7,500). The advertised
   span's ceiling (5,000) costs ~3 s per cold command on this hardware;
   the largest real deployment's size (7,414) costs ~5.6 s — matching the
   ~4 s the field measured on slower hardware at 7.4k.

Calibration anchors from the field (different hardware, real content):
boot ~0.5 ms/entity at a 7,414-entity workspace; a 6,900-entity ingest at
~4 s/CLI-call. Both sit on this curve's shape.

Generation context: one `batch-create` call lands 7,500 entities in
~4.7 s — the batch path exists precisely because per-call cold boots made
per-entity creation scale to hours (plenum finding 1).

## What the numbers imply for the deferred redesigns

Data, not decisions — each paragraph states what the curve says, the
backlog items decide.

**Real lazy mounts (plenum 7) — landed 2026-08-21, measured.** The curve
says load is the only cold-path cost and it grows super-linearly with
loaded entities. Every mounted mem used to add its full entity count to
every cold command, needed or not. `"lifecycle": "lazy"` now defers a
mem's entity load to first read, cutting cold-path cost proportionally to
the unread share. Measured on the dogfood workspace (9 mounts, 687
entities, release build, median of 10 cold runs): a single-mem read
(`memstead entity`, target mem eager, the other 8 mounts lazy) dropped
from 237 ms to 106 ms — the remaining cost is the process spawn plus the
one mem actually read — while a deliberately workspace-scoped command
(`search --mem` under the CLI's full-load default) stayed at ~300 ms,
unchanged by design. The original projection stands for larger
workspaces: at 0.6–0.75 ms/entity above 5k, splitting a 7.5k workspace
into five mems and touching one turns a ~5.6 s command into roughly a
~0.8 s one.

**Incremental maintenance of derived structures (plenum 9) — landed
2026-08-22.** The cold path cannot see this cost: search-after-mutation
equals boot within noise at every size, because the full index rebuild
is dwarfed by the full workspace load that precedes it. The decision
therefore ran on the **warm path** (a long-lived engine absorbing
mutations, boot excluded), measured release-build with the same binary
in maintained vs simulated whole-drop modes: search-share speedup 1.8x
at 500 entities, 1.4x at 2000, 1.3x at 5000. Single mutations now
maintain the index in place under a generation-keyed memo; the honest
finding alongside: the per-query cost grows with store size in BOTH
modes, so query-side work (not the rebuild) is the warm path's next
lever.

**Deferred cross-mem target resolution (plenum 8) — landed 2026-08-22.**
The curve priced the forced mount this redesign removes: each
additionally mounted mem added its entities × 0.6–0.75 ms to **every**
cold command, permanently — a dossier citing 20 small mems of 350
entities each paid ~7k entities of load (~5+ s per command on this
hardware) for edges that needed only target-existence checks. Write-time
verification now asks storage directly (branch-tree existence check plus
one blob read for the type; the SPEC rejected the deferred-stub
direction), so the same dossier pays 20 tree lookups, zero mounts, zero
loads, and no change to any subsequent cold command.

## Relation to the advertised range

Inside 1,000–5,000, a cold CLI command costs roughly 0.5–3 s on this
hardware; the warm MCP path pays load once per server start. The span
remains a design statement about model granularity — the curve attaches
its measured price and shows the price keeps rising past the ceiling
(super-linearly, not catastrophically: 7.5k works, at ~5.6 s per cold
command). The MCP instructions now cite this document as the measured
grounding.
