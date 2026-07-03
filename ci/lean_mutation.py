#!/usr/bin/env python3
"""Lean-build smoke: mutation surface beyond `memstead_create`.

The manual smoke test in `dev/smoke-test-2026-05-09.md` deliberately
did not exercise `memstead_update` (hash round-trip), `memstead_delete`
(tombstone changelog row + entity removal), or `memstead_relate` (typed
edge surfacing through `memstead_entity`). This probe pins all three so
the lean ships with verified mutation tools.

Steps:

1. Create a source entity and a target entity.
2. `memstead_update` round-trip:
   * success path with the freshly-returned `expected_hash`
   * failure path with a stale hash → `HASH_MISMATCH` + `details.current`
3. `memstead_relate` an edge from source to target, then verify the edge
   surfaces in `memstead_entity(include_relations=true)`.
4. `memstead_delete` the source entity, then verify
   * `memstead_search` no longer returns it
   * `.memstead/changes.jsonl` carries a tombstone-shaped row.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from mcp_client import (  # noqa: E402
    McpServer,
    assert_eq,
    assert_true,
    cleanup_workspace,
    fresh_workspace,
    init_workspace,
)


def run(memstead: Path, memstead_mcp: Path) -> int:
    workspace = fresh_workspace()
    try:
        init_workspace(memstead, workspace)

        with McpServer(memstead_mcp, workspace) as server:
            # ---- step 1: seed source + target ------------------------------
            source_create = server.call(
                "memstead_create",
                {
                    "title": "Source",
                    "entity_type": "spec",
                    "sections": {
                        "identity": "Source entity for mutation probe.",
                        "purpose": "Drives update / relate / delete checks.",
                    },
                },
            )
            assert_eq(source_create.is_error, False, "create source")
            source_id = (source_create.structured_content or {}).get("id")
            source_hash = (source_create.structured_content or {}).get("_hash")
            assert_true(bool(source_id and source_hash), "source id/hash present")

            target_create = server.call(
                "memstead_create",
                {
                    "title": "Target",
                    "entity_type": "spec",
                    "sections": {
                        "identity": "Target entity for mutation probe.",
                        "purpose": "Lives at the receiving end of the USES edge.",
                    },
                },
            )
            assert_eq(target_create.is_error, False, "create target")
            target_id = (target_create.structured_content or {}).get("id")
            assert_true(bool(target_id), "target id present")

            # ---- step 2: update round-trip ----------------------------------
            ok_update = server.call(
                "memstead_update",
                {
                    "id": source_id,
                    "expected_hash": source_hash,
                    "sections": {"identity": "Updated source body."},
                },
            )
            assert_eq(ok_update.is_error, False, "update with fresh hash")
            new_hash = (ok_update.structured_content or {}).get("_hash")
            assert_true(bool(new_hash) and new_hash != source_hash, "update produced new hash")

            stale = server.call(
                "memstead_update",
                {
                    "id": source_id,
                    "expected_hash": source_hash,  # deliberately stale
                    "sections": {"identity": "Should be rejected."},
                },
            )
            assert_eq(stale.is_error, True, "stale-hash update should error")
            envelope = stale.structured_content or {}
            assert_eq(envelope.get("code"), "HASH_MISMATCH", "HASH_MISMATCH code")
            current = (envelope.get("details") or {}).get("current")
            assert_eq(current, new_hash, "HASH_MISMATCH details.current")

            # ---- step 3: relate + verify ------------------------------------
            relate = server.call(
                "memstead_relate",
                {"from": source_id, "to": target_id, "type": "USES"},
            )
            assert_eq(relate.is_error, False, "relate")

            with_rel = server.call(
                "memstead_entity",
                {"id": source_id, "include_relations": True},
            )
            assert_eq(with_rel.is_error, False, "entity with relations")
            assert_true("USES" in with_rel.text, "USES edge missing from entity body")
            assert_true(target_id in with_rel.text, "target id missing from entity body")

            # ---- step 4: delete + verify -------------------------------------
            # Refetch the source's hash post-relate (relate mutates the
            # entity content, so the cached new_hash is stale again).
            entity_for_delete = server.call("memstead_entity", {"id": source_id})
            entity_hash = (entity_for_delete.structured_content or {}).get("_hash") or ""
            if not entity_hash:
                # Fall back to scraping the markdown body for `_hash:`.
                for line in entity_for_delete.text.splitlines():
                    if line.startswith("_hash: "):
                        entity_hash = line[len("_hash: "):].strip().strip("`")
                        break
            assert_true(bool(entity_hash), "could not resolve current hash for delete")

            delete = server.call(
                "memstead_delete",
                {"id": source_id, "expected_hash": entity_hash},
            )
            assert_eq(delete.is_error, False, "delete")

            search = server.call("memstead_search", {"query": {"any": ["Source"]}})
            assert_eq(search.is_error, False, "search after delete")
            # The entity should be gone — no hit carries the deleted id.
            structured = search.structured_content or {}
            hits = structured.get("hits") or []
            assert_true(
                all(h.get("id") != source_id for h in hits),
                "deleted entity still surfaces in search",
            )

        # Final changelog probe: 4 mutations expected (create source,
        # create target, update, relate, delete) — five rows in total.
        # The delete row's `kind` is `delete`; that's the tombstone shape.
        changelog = (workspace / ".memstead" / "changes.jsonl").read_text(encoding="utf-8")
        rows = [json.loads(line) for line in changelog.splitlines() if line.strip()]
        kinds = [r.get("kind") for r in rows]
        assert_true("delete" in kinds, "changelog missing delete row")
        delete_row = next(r for r in rows if r.get("kind") == "delete")
        assert_eq(delete_row.get("entity"), source_id, "delete row entity")

        sys.stderr.write("lean_mutation: OK\n")
        return 0
    finally:
        cleanup_workspace(workspace)


def main() -> None:
    parser = argparse.ArgumentParser(description="Lean MCP mutation probe.")
    parser.add_argument("--memstead", required=True, type=Path, help="Path to the lean `memstead` binary.")
    parser.add_argument("--memstead-mcp", required=True, type=Path, help="Path to the lean `memstead-mcp` binary.")
    args = parser.parse_args()
    sys.exit(run(args.memstead, args.memstead_mcp))


if __name__ == "__main__":
    main()
