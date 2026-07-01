#!/usr/bin/env python3
"""Pro-build smoke: vault-repo round-trip.

Mirrors `basis_smoke.py` for the pro flavour. Bootstraps a vault-repo
workspace (`memstead vault-repo init` + `memstead vault init`), boots the pro
`memstead-mcp`, exercises the full mutation surface end-to-end:

* `memstead_create` a source + a target
* `memstead_update` round-trip with `expected_hash` (success path)
* `memstead_relate` an `USES` edge
* `memstead_entity` confirms the edge surfaces with `include_relations`
* `memstead_search` finds the entity
* `memstead_changes_since` returns the entities since the workspace's
  initial empty-tree commit (the canonical fresh-client first sync).

The vault-repo path's surface differs from filesystem in subtle ways
(14-tool surface, gitdir-backed commits, different changelog
mechanism), so this probe runs the same shape as basis-mutation
without the JSONL changelog inspection.

Invocation::

    python3 ci/pro_smoke.py \\
        --memstead path/to/target/release/memstead \\
        --memstead-mcp path/to/target/release/memstead-mcp
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from mcp_client import (  # noqa: E402
    McpServer,
    assert_eq,
    assert_true,
    cleanup_workspace,
    fresh_workspace,
    init_vault_repo_workspace,
)


# Canonical empty-tree git hash. `memstead_changes_since since=<this>`
# returns every entity in the vault as `added` — the "fresh client
# first sync" semantic.
EMPTY_TREE_SHA = "4b825dc642cb6eb9a060e54bf8d69288fbee4904"


def run(memstead: Path, memstead_mcp: Path) -> int:
    workspace = fresh_workspace()
    try:
        init_vault_repo_workspace(memstead, memstead_mcp, workspace)

        with McpServer(memstead_mcp, workspace) as server:
            tools = server.list_tools()
            # Pro surface is 14 tools — read-only + mutation +
            # vault-lifecycle (`memstead_vault_create`, `memstead_vault_delete`)
            # + `memstead_reload`. The probe exercises the entity-surface
            # subset; the lifecycle tools are covered in
            # `memstead-workspace`'s in-process tests already.
            for required in (
                "memstead_create",
                "memstead_search",
                "memstead_entity",
                "memstead_update",
                "memstead_relate",
                "memstead_changes_since",
                "memstead_vault_create",
                "memstead_vault_delete",
                "memstead_reload",
            ):
                assert_true(required in tools, f"pro tool surface missing {required}")

            # ---- create source + target ------------------------------------
            source_create = server.call(
                "memstead_create",
                {
                    "title": "Source",
                    "entity_type": "spec",
                    "vault": "myvault",
                    "sections": {
                        "identity": "Source entity for pro smoke.",
                        "purpose": "Drives update / relate / search checks.",
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
                    "vault": "myvault",
                    "sections": {
                        "identity": "Target entity for pro smoke.",
                        "purpose": "Lives at the receiving end of the USES edge.",
                    },
                },
            )
            assert_eq(target_create.is_error, False, "create target")
            target_id = (target_create.structured_content or {}).get("id")
            assert_true(bool(target_id), "target id present")

            # ---- update with hash ------------------------------------------
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

            # ---- relate + verify in entity ---------------------------------
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

            # ---- search round-trip -----------------------------------------
            # The pro `memstead_search` returns markdown only (no
            # structuredContent on hits); inspect the body for the
            # source id rather than walking a JSON envelope.
            search = server.call("memstead_search", {"query": {"any": ["source"]}})
            assert_eq(search.is_error, False, "search")
            assert_true(source_id in search.text, "search did not surface the created entity")

            # ---- changes_since first-sync probe -----------------------------
            # Same posture as search — the response is markdown.
            # `since=<empty-tree>` should list both source and target
            # as added.
            changes = server.call(
                "memstead_changes_since",
                {"vault": "myvault", "since": EMPTY_TREE_SHA},
            )
            assert_eq(changes.is_error, False, "changes_since")
            for required_id in (source_id, target_id):
                assert_true(
                    required_id in changes.text,
                    f"changes_since missing {required_id}",
                )

        sys.stderr.write("pro_smoke: OK\n")
        return 0
    finally:
        cleanup_workspace(workspace)


def main() -> None:
    parser = argparse.ArgumentParser(description="Pro MCP smoke probe.")
    parser.add_argument("--memstead", required=True, type=Path, help="Path to the pro `memstead` binary.")
    parser.add_argument("--memstead-mcp", required=True, type=Path, help="Path to the pro `memstead-mcp` binary.")
    args = parser.parse_args()
    sys.exit(run(args.memstead, args.memstead_mcp))


if __name__ == "__main__":
    main()
