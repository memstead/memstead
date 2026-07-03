#!/usr/bin/env python3
"""Lean-build smoke: schema-strictness probes.

Pre-strictness-axis-fix the filesystem-mem write path silently
accepted unknown sections, missing required sections, and out-of-enum
metadata values (Bug 1 in the original manual smoke test). This probe
locks the wire-format error codes the agent sees on each violation.

Three probes — each one expects `isError=true` with a specific stable
`code` on `structuredContent`:

* `UNKNOWN_SECTION`: sections payload includes a key the type's schema
  does not declare.
* `MISSING_REQUIRED_SECTION`: sections payload omits a section the
  schema marks `required: true`. Today the filesystem path surfaces
  this as a warning rather than a hard error — see the assertion's
  `note` for the contract this probe pins.
* `INVALID_ENUM_VALUE`: a metadata field with an `enum` constraint
  receives a value outside the enum.
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
    init_workspace,
)


def run(memstead: Path, memstead_mcp: Path) -> int:
    workspace = fresh_workspace()
    try:
        init_workspace(memstead, workspace)

        with McpServer(memstead_mcp, workspace) as server:
            # Probe A — UNKNOWN_SECTION. `spec` declares `identity`,
            # `purpose`, plus a handful of optional sections; `claim`
            # is a memo section that must not slip past the validator.
            unknown = server.call(
                "memstead_create",
                {
                    "title": "Probe Unknown Section",
                    "entity_type": "spec",
                    "sections": {
                        "identity": "filler",
                        "purpose": "filler",
                        "claim": "this section belongs to memo, not spec",
                    },
                },
            )
            assert_eq(unknown.is_error, True, "UNKNOWN_SECTION should error")
            code = (unknown.structured_content or {}).get("code")
            assert_eq(code, "UNKNOWN_SECTION", "UNKNOWN_SECTION code")

            # Probe B — INVALID_ENUM_VALUE. The default schema's
            # `level` field is `M0|M1|M2|M3`; `Z3` is out of range.
            bad_enum = server.call(
                "memstead_create",
                {
                    "title": "Probe Bad Enum",
                    "entity_type": "spec",
                    "sections": {
                        "identity": "filler",
                        "purpose": "filler",
                    },
                    "metadata": {"level": "Z3"},
                },
            )
            assert_eq(bad_enum.is_error, True, "INVALID_ENUM_VALUE should error")
            code = (bad_enum.structured_content or {}).get("code")
            assert_eq(code, "INVALID_ENUM_VALUE", "INVALID_ENUM_VALUE code")

            # Probe C — MISSING_REQUIRED_SECTION. A create that omits a
            # required section is refused outright with the stable code
            # on the error envelope — same shape as probes A and B.
            # (Earlier engines created the entity and attached a
            # warning; the strictness work promoted this to a hard
            # error.)
            missing = server.call(
                "memstead_create",
                {
                    "title": "Probe Missing Required",
                    "entity_type": "spec",
                    "sections": {},
                },
            )
            assert_eq(missing.is_error, True, "MISSING_REQUIRED_SECTION should error")
            code = (missing.structured_content or {}).get("code")
            assert_eq(code, "MISSING_REQUIRED_SECTION", "MISSING_REQUIRED_SECTION code")

        sys.stderr.write("lean_strictness: OK\n")
        return 0
    finally:
        cleanup_workspace(workspace)


def main() -> None:
    parser = argparse.ArgumentParser(description="Lean MCP strictness probe.")
    parser.add_argument("--memstead", required=True, type=Path, help="Path to the lean `memstead` binary.")
    parser.add_argument("--memstead-mcp", required=True, type=Path, help="Path to the lean `memstead-mcp` binary.")
    args = parser.parse_args()
    sys.exit(run(args.memstead, args.memstead_mcp))


if __name__ == "__main__":
    main()
