#!/usr/bin/env bash
# Validate plugin-format files in the live workspace against the current
# memstead-plugin/v1 schemas. Walks the binding layout under `.memstead/`:
# `mediums/<mem>/*.json`, `facets/<mem>/*.json`, and `projections/<mem>/*.json`
# (validated against the v1 binding schema).
#
# Also metaschema-shape-checks each schema document and validates the v1
# example fixtures against their schemas.
#
# Uses Node built-ins only. No npm dependencies.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
SCHEMAS_DIR="$REPO_ROOT/plugins/claude-code/schemas/memstead-plugin/v1"
WORKSPACE_DIR="$REPO_ROOT/graph"

cd "$REPO_ROOT"
exec node "$SCHEMAS_DIR/../../validate-live-workspace.mjs" \
  --schemas-dir "$SCHEMAS_DIR" \
  --workspace-dir "$WORKSPACE_DIR"
