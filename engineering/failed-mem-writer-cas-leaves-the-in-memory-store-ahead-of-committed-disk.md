---
type: memo
created_date: 2026-07-13T16:43:04Z
last_modified: 2026-07-13T16:43:04Z
status: closed
tags: observation, lesson, coherence, cas, reload, concurrency, engine
---

# Failed mem-writer CAS leaves the in-memory store ahead of committed disk

## Claim
A mem mutation whose git commit loses the ref compare-and-swap race leaves its staged write orphaned in the git-branch backend's in-memory `pending` buffer (the failure path skipped the buffer-clear), and because `read_entity` serves `pending` ahead of the committed tip, the never-committed write reads back as truth — a divergence a head-gated `memstead_reload` does not repair but actively spreads into the store.

## Context
- Observed live: two engines shared one git-branch mem-repo — a long-lived MCP engine plus a sibling ingest loop writing the same branch writing the same branch.
- `memstead_update` calls returned `MEM_ERROR: mem writer cas conflict` (the commit was rejected), yet subsequent `memstead_entity` reads from that engine returned the would-be-written content while the git branch tip still held the originals.
- `memstead_reload` reported `head_before == head_after` and a no-op `changed_entity_ids`, but reads kept serving the phantom content — the in-memory slice stayed divergent from the committed blob at the same head.
- Cleared only by restarting the MCP server (fresh load from disk). This is the [[engine--reload-before-operation-coherence]] floor failing in the one case it cannot see.

## Relationships
- **REFERENCES**: [[engine:reload-before-operation-coherence]]
- **REFERENCES**: [[engine:explicit-mem-reload-surface]]

## Substance

Two pieces combine, each individually reasonable.

1. **Root cause — `commit()` did not clear `pending` on failure** (`crates/memstead-git-branch/src/storage/git_tree.rs`, `GitTreeMemWriter::commit`). The backend stages writes into an in-memory `Pending { parent, ops }` buffer via `write_entity`, then `commit_as` does a compare-and-swap on the branch tip. On a CAS conflict the conflict arm `return`ed early with `HashMismatch` and never reached `pending.clear()`, which sat on the success tail. The rejected write stayed in `ops`.
2. **Amplifier — `read_entity` prefers `pending` over the committed tip** (same file: "Pending ops win over the branch tip"). That precedence is correct *during* a live transaction, but defect 1 left a *dead* transaction's bytes in the buffer, so the orphaned write was served as truth.

Why it hid, then spread: `memstead_entity` reads the engine's parsed `self.store`, not the backend, and `apply_prepared_to_store` runs only after a *successful* commit — so right after the failed commit the store still held truth and reads looked fine. Then `memstead_reload` ([[engine--explicit-mem-reload-surface]]) → `collect_source_entries` → `read_entity` per path (`engine/boot.rs`, `engine/lifecycle.rs::reload_one_mem_inner`) ingested the phantom into the store. The automatic `reload_if_stale` never fired because the committed tip never moved. Worst case: a *later* successful mutation builds on the phantom store entity and bakes it onto disk.

## Alternatives

- Roll the in-memory entity back to its pre-mutation state when the ref-CAS commit fails — the targeted fix for defect 1.
- Make reload reconcile on a content/tree comparison, not only the head SHA, so an in-memory/disk divergence at an unchanged head is detectable — defends against defect 2 even if 1 regresses.
- Surface the CAS failure as a typed retryable error that also invalidates the affected entity's in-memory copy, rather than a generic `MEM_ERROR`.

## Outcome

Fixed 2026-06-09. `commit()` now calls `pending.clear()` immediately after `commit_as` returns — covering success, CAS conflict, and any other commit error — so a failed transaction is always aborted and `read_entity` falls back to the committed tip. This is the targeted defect-1 fix from Alternatives; it restores the invariant `read_entity` assumed (the buffer is non-empty only during a genuinely in-flight commit). Regression test `cas_conflict_clears_pending_so_reads_fall_back_to_committed_truth` stages an update that loses the CAS race and asserts the read returns committed truth, not the phantom; the full `memstead-git-branch` git_tree suite (36 tests) passes and the full workspace shows no new failures. The defect-2 hardening (content-aware reload rather than head-SHA-gated) remains optional follow-up — no longer required for correctness now that no failed commit can leave the buffer dirty.
