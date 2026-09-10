---
type: decision
created_date: 2026-09-10T17:57:25Z
last_modified: 2026-09-10T17:57:25Z
status: accepted
decided_on: 2026-09-10
deciders: operator (go on the 2026-09-10 architecture review), implementing agent sessions of planning/plan-projection-crate
scope: system
tags: crate-layout, kernel, projection, boundary, health
---

# The maintenance loop is its own crate and the kernel keeps what the write gate consults

## Decision
We chose to cut the maintenance loop out of `memstead-base` into its own workspace crate, `memstead-projection` (briefs, change detection, cursors, findings and verify reports, the advance gate, prune proposals, refinement, selection, status, the health assembly), above the kernel and `memstead-schema` and below the binaries, depending on no storage crate. The boundary is drawn by use, not by location: the kernel keeps every module the write gate or the kernel's own read axes consult, which is the anchor primitive and sidecar, the preparation registry, the binding declarations and their store (`binding`, `pipeline`, `pipeline_edit`, `pipeline_store`), and four modules that were loop code by location and declaration work by use (`source_scope`, the scope enumeration the anchors health axis consumes; `check_path`, the deny oracle; `binding_run`, once `ingest::resolve`; `binding_intent`, once `ingest::intent`). The kernel composes health once and takes the loop's contribution (the unanchored-mention warnings) as a data parameter it never interprets; `memstead_projection::health::compose_health` is the one assembly every surface calls (and `health_summary` its summary-shaped form for an embedder rendering the kernel's `HealthSummary`), so no surface can carry the kernel's axes and forget the loop's. The crate is published as a dependency of the two products under the sibling crates' posture, sixth on the version line.

## Context
The 2026-09-10 architecture review measured `memstead-base` at 165,000 lines in 134 files, a fifth of it the maintenance loop that had arrived in the kernel with the 2026-09-05 fold of `memstead-engine`; the loop had its own lifecycle and vocabulary and no wasm relevance, and the boundary between kernel and loop was convention only, so every loop change rebuilt the kernel and the next subsystem would land in the kernel by default. Before cutting, the first execution session re-measured the seam and corrected the plan's assumption: the mutation path validates an anchor's binding hash and source name against the declared bindings and hashes anchored content through the preparation registry, so those are kernel; the kernel then reached into the loop from six read sites (the anchors axis enumerating source artifacts, the projection axis resolving process mems, the entity read computing findings, a facet read enumerating scope), each of which was either declaration work that moved down or the health seam that became data. The cut landed in five commits on public main after 0.20.0, each green on the scoped suites, the last on the full suite and CI, then the consumers, publication order and describing surfaces followed.

## Consequences
- A change to the loop no longer rebuilds the kernel, and the compiler, not a convention, refuses a kernel path into the loop; the wasm job stays the kernel's portability proof.
- `Engine::health()` no longer carries the unanchored-mention axis; an embedder that renders health composes through `memstead_projection::health` (the JSON assembly or the summary-shaped one), which the UI API does since the same day.
- Four kernel items are public across the crate boundary (`engine::query::{anchor_base_path, join_pointer, artifact_candidates, resolve_across_sources}`, `Engine::validate_anchor_inputs`); `memstead-base` carries a `test-support` feature exposing one test-only accessor to the loop's suite, never on in a product.
- Six crates ride the version line and `publish-crates.sh`, the publish workflow and `release-verify.sh` name the sixth; the release leg's pin count derives from the manifest and needed no change.
- The kernel's declaration layer is nine modules by name, and a future module is placed by the same test: consulted by the write gate or a kernel axis, it is kernel; reading and reasoning over anchors, it is loop.

## Options

Rejected: enforcing the boundary inside one crate with visibility rules and a lint (still one compilation unit, still the default landing place). Rejected: cutting the anchor primitive out with the loop (the kernel reaches anchors from every mutation, backend read and health axis; cutting them inverts the dependency). Rejected: moving the loop into `memstead-git-branch` (the loop serves folder mems and must not carry gix). Rejected: keeping binding configurations on the `Engine` for the loop to reach (the write gate reads them, so they are kernel state either way). Rejected: a trait the kernel declares for the loop's health axis (a second abstraction for one implementor; data is enough). Rejected: several crates at once (one measured boundary, no coupling to argue a finer split from).

## Notes

Revisit if a second consumer of the loop appears that needs a subset (findings without briefs, say), or a kernel axis starts needing loop computation again; either is the signal to re-measure the seam before adding a dependency edge. Decision trail: the bundle `planning/plan-projection-crate` (archived to the attic on close) and public commits 7088ef90 through 73a096fe.
