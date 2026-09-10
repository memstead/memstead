# Sizing curve — measured operating limits

**Measured:** 2026-09-10 (cold path) and 2026-09-11 (warm path, same
corpus, same hardware, engine at 0.20.0 plus the harness's warm leg) ·
engine at 0.20.0 (the release that carries the
linear-boot fix and this page) · Apple M5 Max (macOS, aarch64),
release binaries. **All numbers are hardware-relative** — treat the shape of
the curve as portable and the absolute milliseconds as this machine's.
The first measurement (2026-08-06, commit `c48cd07`) is kept below as a
dated record: it found the cost super-linear, a profile on 2026-09-10
named the cause, and the fix made the curve linear the same day. The
field deployment that motivated the first measurement ran on different
hardware and saw ~0.5 ms/entity on the pre-fix engine.

The engine's MCP instructions describe a mem as "designed for 1,000–5,000
entities". Until this document, that span was advertised, not measured.
This page states what the four everyday cold
operations and the three warm reads actually cost across workspace sizes, so the deferred redesigns
that wait for numbers (incremental derived-structure maintenance — and
lazy mounts plus deferred cross-mem targets, since landed and recorded
below) can be argued from data.

## Reproducing

One command generates the graded synthetic workspaces, measures, and writes
machine-readable results:

```
cargo run -p xtask -- sizing-curve
```

Defaults: sizes `500,2500,5000,7500`, 3 iterations per cold operation
and 20 calls per warm operation, results to `target/sizing-curve.json`
(`format: "sizing-curve/v2"`, per-run samples included; v1 carried the
cold leg alone). Override with `--sizes`, `--iterations`,
`--warm-iterations`, `--output`; `--memstead` and `--memstead-mcp` point
at pre-built release binaries. The harness builds both release binaries
itself otherwise, drives everything through the product surface
(`memstead mem-repo init` → `mem init` → `batch-create` for the cold
leg, one `memstead-mcp` server over stdio JSON-RPC for the warm leg),
and runs each workspace in a temp directory that is deleted afterwards
— no residue in the repo, no effect on the test suite.

To diff after an engine change: rerun, then compare the JSON
(`diff <(jq . old.json) <(jq . target/sizing-curve.json)` or any JSON
diff) — medians per operation per size are the contract. Run-to-run
tolerance on this hardware: cold medians within about 5 %, warm medians
within about 20 % at the small end (a sub-millisecond call is dominated
by scheduling noise) and about 10 % above 2,500; the per-entity slopes
are the stable reading.

## Method

Synthetic corpora, self-contained: `spec` entities under the builtin
`default` schema — the harness pins no version, so a run takes whatever
the binary's default is (`default@1.3.0` today) — three prose sections each, rotating `level`
metadata, two explicit edges (USES / DEPENDS_ON) to earlier entities plus
one body wiki-link (alias-emitting REFERENCES) — edge density ~3/entity,
following the shape of the largest real deployment without depending on
it. Backend: git-branch (mem-repo), the backend the field pain was
measured on. Each cold operation is a fresh `memstead` process — the
**cold CLI path**, where cost was first reported; the **warm path**, one
MCP server paying boot once and answering calls after it, is measured
separately below on the same workspace.

The four cold operations, timed spawn-to-exit, median of 3:

| Operation | Command | What it pays |
|---|---|---|
| boot | `memstead list --limit 1` | process + engine boot + full workspace load |
| update | `memstead update <id> --auto-hash --append …` | boot + read + write commit + index invalidation |
| search | `memstead search <term>` (right after the update) | boot + search-index rebuild + query |
| overview | `memstead overview` | boot + community/summary path |

The three warm operations, timed request-to-reply over one long-lived
`memstead-mcp` server on the same workspace (boot paid once by a
warm-up call and excluded), median of 20:

| Operation | Tool call | What it pays |
|---|---|---|
| warm search | `memstead_search` (`query.any`, `limit` 10) | index query + hit rendering |
| warm entity | `memstead_entity` (`include_relations: true`) | one read + the incoming-edge scan over the store |
| warm overview | `memstead_overview` | the workspace-global partition summary, roster and budget fill |

## The curve

Median milliseconds per operation (full per-run samples in the JSON):

| Entities | Generation | Boot | Update | Search | Overview | Boot ms/entity |
|---:|---:|---:|---:|---:|---:|---:|
| 500 | 324 | 59 | 60 | 62 | 60 | 0.12 |
| 2,500 | 1,223 | 145 | 142 | 148 | 144 | 0.06 |
| 5,000 | 2,801 | 255 | 247 | 255 | 252 | 0.05 |
| 7,500 | 4,976 | 371 | 356 | 369 | 365 | 0.05 |

Two findings carry the whole page:

1. **Every cold operation costs the same as boot.** At every size, update,
   search-after-mutation, and overview sit within noise of the plain boot
   (±15 ms at 7,500 against a 371 ms baseline). Index rebuild, the
   mutation commit, and community detection are invisible next to loading
   the workspace. On the cold CLI path there is exactly one cost: **load**.
2. **Load is linear in the entity count.** 15× the entities costs 6× the
   time; the per-entity figure falls from 0.12 to 0.05 ms because the
   small end is dominated by process spawn and repository open (~40 ms
   flat), not by entities. The advertised span's ceiling (5,000) costs a
   quarter of a second per cold command on this hardware; the largest
   real deployment's size (7,414) sits under 0.4 s.

### Dated record: the 2026-08-06 curve and its cause

The first measurement (commit `c48cd07`, same hardware, same corpus):

| Entities | Boot | Boot ms/entity |
|---:|---:|---:|
| 500 | 181 | 0.36 |
| 2,500 | 1,162 | 0.46 |
| 5,000 | 3,043 | 0.61 |
| 7,500 | 5,647 | 0.75 |

15× the entities cost 31× the time, and the page read that as the
price of loading, with lazy mounts and mem splitting as the levers. A
CPU profile of the 7,500-entity boot on 2026-09-10 (Instruments, Time
Profiler, 6.2 s sampled) put 96 % of the time in the per-entity read of
the git-branch backend: the boot listed every entity (a walk that
already read every blob and dropped the bytes), then read each one
through `read_entity`, which opened the repository, peeled the ref and
inflated the root tree afresh on every call. A 7,500-entry tree
inflated 7,500 times is quadratic; the markdown parser and the
validators were under one percent. The fix is one backend primitive,
`read_all_entities`: the whole mem in one tree walk, taken by boot and
reload, under the same source-selection rules as the per-path read.
Field calibration from that era (boot ~0.5 ms/entity at 7,414 entities,
a 6,900-entity ingest at ~4 s per CLI call) measured the quadratic path
and no longer predicts the engine.

Generation context: one `batch-create` call lands 7,500 entities in
~4.7 s — the batch path exists precisely because per-call cold boots made
per-entity creation scale to hours.

## The warm path

Median microseconds per call after boot (full per-call samples in the
JSON), measured 2026-09-11 on the same corpus and hardware:

| Entities | Warm search | Warm entity (with relations) | Warm overview |
|---:|---:|---:|---:|
| 500 | 427 | 822 | 1,563 |
| 2,500 | 487 | 1,079 | 6,270 |
| 5,000 | 624 | 1,393 | 12,578 |
| 7,500 | 693 | 1,711 | 19,156 |

Per-entity slopes over the 500 to 7,500 span: search **0.04 µs/entity**
(a fixed ~0.4 ms floor, the query itself is nearly free of the store
size), entity read with relations **0.13 µs/entity** (linear: the
incoming-edge scan walks the store), overview **2.5 µs/entity** (linear
and sixty times the search slope: the partition summary, roster and
budget fill touch every entity). Two findings:

1. **After boot, a session pays under two milliseconds per read at the
   advertised ceiling.** At 5,000 entities a search returns in about
   0.6 ms and an entity with its relations in about 1.4 ms; the cold
   path's quarter of a second per command is boot, and boot alone.
2. **The overview is the warm path's next lever, not search.** It is
   the one warm read that scales like a load (2.5 µs/entity, 19 ms at
   7,500), and it is the cold-start call every session makes first. The
   incremental-maintenance work of 2026-08-22 predicted "query-side
   work" as the next lever without a number; the number says which
   query: the workspace-global overview assembly, and after it the
   incoming-edge scan behind `include_relations`.

## What the numbers imply for the deferred redesigns

Data, not decisions — each paragraph states what the curve says, the
backlog items decide. The three paragraphs below were argued from the
2026-08-06 curve and carry its per-entity multipliers (0.36–0.75 ms);
the mechanisms they decided stand, the multipliers are a twentieth of
that on the linear engine, and the cold-path share each lever removes
is proportionally smaller.

**Real lazy mounts (landed 2026-08-21, measured).** The curve
says load is the only cold-path cost and it grows super-linearly with
loaded entities. Every mounted mem used to add its full entity count to
every cold command, needed or not. `"lifecycle": "lazy"` now defers a
mem's entity load to first read, cutting cold-path cost proportionally to
the unread share. Measured on the project's own workspace (9 mounts, 687
entities, release build, median of 10 cold runs): a single-mem read
(`memstead entity`, target mem eager, the other 8 mounts lazy) dropped
from 237 ms to 106 ms — the remaining cost is the process spawn plus the
one mem actually read — while a deliberately workspace-scoped command
(`search --mem` under the CLI's full-load default) stayed at ~300 ms,
unchanged by design. The original projection stands for larger
workspaces: at 0.6–0.75 ms/entity above 5k, splitting a 7.5k workspace
into five mems and touching one turns a ~5.6 s command into roughly a
~0.8 s one.

**Incremental maintenance of derived structures (landed 2026-08-22).** The cold path cannot see this cost: search-after-mutation
equals boot within noise at every size, because the full index rebuild
is dwarfed by the full workspace load that precedes it. The decision
therefore ran on the **warm path** (a long-lived engine absorbing
mutations, boot excluded), measured release-build with the same binary
in maintained vs simulated whole-drop modes: search-share speedup 1.8x
at 500 entities, 1.4x at 2000, 1.3x at 5000. Single mutations now
maintain the index in place under a generation-keyed memo; the honest
finding alongside: the per-query cost grows with store size in BOTH
modes. The warm-path table above (2026-09-11) prices that growth per
call: search is nearly flat, the entity read is linear in the store
through its incoming-edge scan, and the overview is the warm read that
scales like a load.

**Deferred cross-mem target resolution (landed 2026-08-22).**
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

Inside 1,000–5,000, a cold CLI command costs roughly 0.1–0.25 s on this
hardware; the warm MCP path pays load once per server start and then
under two milliseconds per search or entity read, and up to about
thirteen milliseconds per overview at the ceiling. The span
remains a design statement about model granularity — the curve attaches
its measured price and shows the price rising linearly past the ceiling
(7.5k works, at ~0.4 s per cold command). The MCP instructions cite this
document as the measured grounding.
