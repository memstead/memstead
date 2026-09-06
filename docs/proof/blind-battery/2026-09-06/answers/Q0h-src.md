No, one agent cannot silently overwrite the other's change. The engine prevents this through optimistic locking using a 16-character SHA-256 hash of the markdown content.

When an agent calls `memstead_update`, it passes an `expected_hash` parameter containing the entity's hash at the time it was read (engine/mutation/update.rs, line 410). The engine verifies that the current stored hash matches this expected value. If another agent has written to the entity in the meantime, the on-disk hash will differ, and the update fails immediately.

The mechanism: the engine computes `content_hash` as the first 16 characters of SHA-256(raw markdown content) whenever the entity is loaded or stored (entity/parser.rs, line 1076). On an update attempt with `expected_hash` set, the code compares `entity.content_hash != expected` (update.rs, line 412). A mismatch returns `EngineError::HashMismatch` without writing anything to disk.

The losing agent sees a `HASH_MISMATCH` error with code 4 (exit status on the CLI). The error payload includes three fields: the entity id, the current hash on disk, and whether the entity is a stub (engine/error.rs). Armed with the current hash, the losing agent can recover by re-reading the entity (to fetch its new content and updated hash), applying their intended change on top of that new base, and then retrying the update with the refreshed expected hash.

The system is atomic per-entity: a single entity update either fully commits or fails completely. Batches of updates are atomic at the batch level as well—if any item in the batch has a hash mismatch, the entire batch is refused (write_commands.rs test `batch_update_refuses_atomically_on_hash_mismatch`).
