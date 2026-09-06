`HASH_MISMATCH` is the optimistic lock doing its job: the `expected_hash` your agent sent (the `_hash` from its earlier read) no longer matches the entity's current content hash, so the entity changed between that read and the update (engine--update-mutation, engine--mcp-tool-surface). Typical causes: another agent, a CLI session, or a sibling engine process wrote the entity, or your own agent updated it earlier and kept the old hash instead of the `content_hash` the successful response returned. Before checking the hash the engine reloads the mem if its on-disk head moved, so the mismatch reflects real current content, not a stale cache; if a reload happened, the error also carries a `MEM_RELOADED` warning and a `mem_changed` notice (engine--reload-before-operation-coherence).

Your data is safe in both directions. The refusal happens in the validation phase, before anything is staged or committed, so nothing of yours landed and nothing of the other writer's was overwritten (engine--update-mutation). The error's `details` carry `{id, current, is_stub}`, where `current` is the live hash (engine--typed-error-and-warning-envelope).

Correct recovery sequence:

1. `memstead_entity <id>` to read the current content and its `_hash`.
2. Reconcile your edit against what is actually there now.
3. `memstead_update` again with `expected_hash` set to that fresh `_hash`.
4. Keep the `content_hash` from the successful response for any follow-up edit to the same entity (engine--update-mutation).

Shortcuts:

- Retry immediately with `details.current` as `expected_hash`, skipping the re-read. Fine for additive edits (`append_sections`, metadata set, `declare_relations`) or `patch_sections`, which refuses `PATCH_OLD_NOT_FOUND` if the text it targets has moved. A bad idea with `sections` (full replace) or `sections_unset`: you would overwrite the other writer's change without ever seeing it, which is exactly the clobber the lock exists to prevent (engine--update-mutation, engine--typed-error-and-warning-envelope).
- `dry_run: true` skips the hash check, writes nothing, and returns the unchanged on-disk `content_hash` (reuse that) plus a `prospective_hash` (do not reuse it; the auto-stamped `last_modified` shifts the real result) (engine--update-mutation).
- Omitting `expected_hash` is not an escape hatch over MCP: a content-changing update without it refuses `EXPECTED_HASH_REQUIRED`; only anchors-only writes may omit it (engine--mcp-tool-surface).

If `is_stub` is true, no update will succeed: promote the stub with `memstead_create` instead (engine--update-mutation).
