You are grading two anonymous answers, A and B, to one question about Memstead. You do not know who wrote them and must not guess; ignore any stray hint of where an answer came from and judge content only.

Verify every checkable claim in both answers against the engine's Rust source under `<source>/crates` and, where a claim is about behaviour, by running the release binary `<memstead>` in a scratch workspace of your own under `<run>/grader-N2` (create it with `<memstead> init --name probe --schema default@1.3.0 <run>/grader-N2/probe` or `<memstead> quickstart <run>/grader-N2/probe`; never touch any other workspace or repository).

Return ONLY a JSON object with exactly these keys:
- "winner": "A", "B" or "tie"
- "margin": 1, 2 or 3 (1 close, 3 decisive; 0 for a tie)
- "scores": {"A": {"correctness": 0-5, "completeness": 0-5, "usefulness": 0-5, "honesty": 0-5}, "B": {same}}
- "errors_A": a list of strings, each one false or unverifiable claim in A with what the source or the binary shows instead; empty if none
- "errors_B": the same for B
- "rationale": under 120 words

Correctness is truth against the source and the binary; completeness is whether the question's parts are all answered; usefulness is whether a developer could act on it; honesty is whether the answer hedges what it could not establish and invents nothing.

Question:

My coding agent called memstead_update on an entity and got an error with code HASH_MISMATCH. What happened, is my data safe, and what is the correct sequence of calls to recover and land the edit? Are there shortcuts, and when are they a bad idea?

# Answer A

HASH_MISMATCH is optimistic locking doing its job: the `expected_hash` your agent sent no longer equals the entity's current `content_hash`, so the write was refused before anything was staged or committed. Before that compare, the engine probes the mem's branch tip and reloads it if another writer advanced it, so the comparison runs against on-disk truth, not a stale cache; if a reload happened, the refusal also carries a `mem_changed` notice and a MEM_RELOADED warning. The usual cause is that the entity changed since your agent last read it, whether by another agent, a sync job, or your own earlier update whose new `_hash` was not carried forward. Your data is safe: the update path prepares fully, then commits, and the hash gate fires in the prepare step, so nothing on disk, in the store, or in git moved.

The error payload already hands you the fix: `details.current` is the live hash, and `details.is_stub` tells you whether the target is a stub, which cannot be updated at all and must be adopted via `memstead_create` instead.

Correct sequence: call `memstead_entity` on the id, look at what changed, re-apply your edit on top of the fresh content, then call `memstead_update` with the returned `_hash` as `expected_hash`. If the edit is a section replace, re-reading matters because your payload overwrites whatever the other writer put there.

Shortcuts. Passing `details.current` straight back as `expected_hash` without re-reading lands the write but blindly clobbers the intervening change; fine for append or patch edits you are sure are independent, bad for full-section replaces. `dry_run: true` skips only the hash check, runs full validation, and returns the current `_hash` plus a `prospective_hash`; pass that `_hash` on the real call. Over MCP you cannot omit the token on a content change; that refuses with EXPECTED_HASH_REQUIRED. The CLI offers `--auto-hash` and `--force`, which refetch the hash and write regardless; they are for one-off interactive edits and are a bad idea in scripts, loops, or anywhere concurrent writers exist.

# Answer B

`HASH_MISMATCH` is the optimistic lock doing its job: the `expected_hash` your agent sent (the `_hash` from its earlier read) no longer matches the entity's current content hash, so the entity changed between that read and the update. Typical causes: another agent, a CLI session, or a sibling engine process wrote the entity, or your own agent updated it earlier and kept the old hash instead of the `content_hash` the successful response returned. Before checking the hash the engine reloads the mem if its on-disk head moved, so the mismatch reflects real current content, not a stale cache; if a reload happened, the error also carries a `MEM_RELOADED` warning and a `mem_changed` notice.

Your data is safe in both directions. The refusal happens in the validation phase, before anything is staged or committed, so nothing of yours landed and nothing of the other writer's was overwritten. The error's `details` carry `{id, current, is_stub}`, where `current` is the live hash.

Correct recovery sequence:

1. `memstead_entity <id>` to read the current content and its `_hash`.
2. Reconcile your edit against what is actually there now.
3. `memstead_update` again with `expected_hash` set to that fresh `_hash`.
4. Keep the `content_hash` from the successful response for any follow-up edit to the same entity.

Shortcuts:

- Retry immediately with `details.current` as `expected_hash`, skipping the re-read. Fine for additive edits (`append_sections`, metadata set, `declare_relations`) or `patch_sections`, which refuses `PATCH_OLD_NOT_FOUND` if the text it targets has moved. A bad idea with `sections` (full replace) or `sections_unset`: you would overwrite the other writer's change without ever seeing it, which is exactly the clobber the lock exists to prevent.
- `dry_run: true` skips the hash check, writes nothing, and returns the unchanged on-disk `content_hash` (reuse that) plus a `prospective_hash` (do not reuse it; the auto-stamped `last_modified` shifts the real result).
- Omitting `expected_hash` is not an escape hatch over MCP: a content-changing update without it refuses `EXPECTED_HASH_REQUIRED`; only anchors-only writes may omit it.

If `is_stub` is true, no update will succeed: promote the stub with `memstead_create` instead.
