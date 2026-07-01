#!/usr/bin/env bash
# Validate plugin-format files in the live workspace against the
# memstead-plugin/v0 schemas. Walks the four-primitive layout under
# `.memstead/`: `mediums/<vault>/*.json`, `facets/<vault>/*.json`,
# `projections/<vault>/*.json`, and `ingests/*.json`.
#
# Also validates each schema document against the JSON Schema 2020-12
# metaschema (sanity check on the schema files themselves) and the
# example fixtures against their schemas.
#
# Uses Node built-ins only. No npm dependencies.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
SCHEMAS_DIR="$REPO_ROOT/plugins/claude-code/schemas/memstead-plugin/v0"
WORKSPACE_DIR="$REPO_ROOT/graph"

cd "$REPO_ROOT"
exec node "$SCHEMAS_DIR/../../validate-live-workspace.mjs" \
  --schemas-dir "$SCHEMAS_DIR" \
  --workspace-dir "$WORKSPACE_DIR"
