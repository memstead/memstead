#!/usr/bin/env python3
"""Lean-build smoke: happy-path round-trip.

Automates the original manual smoke probe.
Boots the lean `memstead-mcp` against a fresh `memstead init`'d tempdir,
creates one entity, searches for it, and verifies the JSONL changelog
has one row. Fails the build on any divergence.

Invocation::

    python3 ci/lean_smoke.py \\
        --memstead path/to/target/release/memstead \\
        --memstead-mcp path/to/target/release/memstead-mcp
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
            tools = server.list_tools()
            # The lean surface is filesystem-shape: 12 tools today,
            # named `memstead_*`. Don't pin the exact count — surface
            # changes are out of scope for this probe — but every tool
            # this probe calls must be present.
            for required in ("memstead_create", "memstead_search", "memstead_entity"):
                assert_true(required in tools, f"tool surface missing {required}")

            create = server.call(
                "memstead_create",
                {
                    "title": "Lean Smoke",
                    "entity_type": "spec",
                    "sections": {
                        "identity": "An entity to verify the lean happy path.",
                        "purpose": "Lets the smoke probe round-trip without the mem-repo path.",
                    },
                },
            )
            assert_eq(create.is_error, False, "create.is_error")
            structured = create.structured_content or {}
            entity_id = structured.get("id")
            assert_true(bool(entity_id), "create returned no id")
            content_hash = structured.get("_hash")
            assert_true(bool(content_hash), "create returned no content_hash")
            sys.stderr.write(f"created entity {entity_id} (hash={content_hash})\n")

            search = server.call("memstead_search", {"query": {"any": ["smoke"]}})
            assert_eq(search.is_error, False, "search.is_error")
            assert_true(
                entity_id in search.text or entity_id in (search.structured_content or {}).get("hits", [{}])[0].get("id", ""),
                "search did not surface the created entity",
            )

            entity = server.call("memstead_entity", {"id": entity_id})
            assert_eq(entity.is_error, False, "entity.is_error")
            # The lean path emits markdown body in `text`.
            assert_true("Lean Smoke" in entity.text, "entity body missing title")

        # Changelog probe — `.memstead/changes.jsonl` must carry exactly one
        # row for the single mutation, with `kind=create` and the
        # `client` provenance threaded from the MCP `clientInfo` we
        # passed during initialize.
        changelog = (workspace / ".memstead" / "changes.jsonl").read_text(encoding="utf-8")
        rows = [json.loads(line) for line in changelog.splitlines() if line.strip()]
        assert_eq(len(rows), 1, "changelog row count")
        first = rows[0]
        assert_eq(first.get("kind"), "create", "changelog row kind")
        assert_eq(first.get("entity"), entity_id, "changelog row entity")
        # `client` is the canonical `name@version` form stamped from
        # the MCP `initialize` handshake. The probe sent `lean-smoke`
        # with version `0`, so the changelog must echo `lean-smoke@0`.
        assert_eq(first.get("client"), "lean-smoke@0", "changelog row client")
        assert_eq(first.get("actor"), "agent", "changelog row actor")

        sys.stderr.write("lean_smoke: OK\n")
        return 0
    finally:
        cleanup_workspace(workspace)


def main() -> None:
    parser = argparse.ArgumentParser(description="Lean MCP smoke probe (happy path).")
    parser.add_argument("--memstead", required=True, type=Path, help="Path to the lean `memstead` binary.")
    parser.add_argument("--memstead-mcp", required=True, type=Path, help="Path to the lean `memstead-mcp` binary.")
    args = parser.parse_args()
    sys.exit(run(args.memstead, args.memstead_mcp))


if __name__ == "__main__":
    main()
