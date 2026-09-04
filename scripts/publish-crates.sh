#!/usr/bin/env bash
# Publish the open Memstead engine crates to crates.io.
#
# Usage:
#   scripts/publish-crates.sh --dry-run   # package + verify everything, upload nothing
#   scripts/publish-crates.sh             # real publish (requires `cargo login` first)
#
# Publication order — topological over the internal path-dependency graph:
#
#   memstead-schema       (leaf — no internal deps)
#   memstead-base         -> schema
#   memstead-git-branch   -> base, schema
#   memstead-mcp          -> base, git-branch, schema
#   memstead-cli          -> base, git-branch, schema
#
# Exactly these five crates are the public Rust surface. Deliberately NOT
# published (see the `publish = false` comments in their manifests):
# memstead-wasm
# (distribution channel is npm — scripts/publish-npm.sh), xtask (internal
# tooling).
#
# Dry-run note: a *per-crate* `cargo publish --dry-run -p <crate>` cannot
# succeed before that crate's internal deps are live on crates.io — the
# packaged manifest keeps only the version requirement, and resolution
# against the registry fails. The multi-package form below (stable since
# cargo 1.90) packages all requested crates together and verifies each
# build against a local overlay registry, so the whole set is verifiable
# before anything is published.
#
# Real-publish note: crates are published one at a time in the order above;
# cargo blocks until each published crate is visible in the index before
# the script moves on, so the next crate's dependency resolution succeeds.
# A crate whose current version is already live is skipped, making the
# script safe to re-run after a partial failure.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

CRATES=(
    memstead-schema
    memstead-base
    memstead-git-branch
    memstead-mcp
    memstead-cli
)

DRY_RUN=0
for arg in "$@"; do
    case "$arg" in
        --dry-run) DRY_RUN=1 ;;
        *) echo "unknown argument: $arg (only --dry-run is supported)" >&2; exit 2 ;;
    esac
done

if [[ "$DRY_RUN" == 1 ]]; then
    PKG_ARGS=()
    for c in "${CRATES[@]}"; do PKG_ARGS+=(-p "$c"); done
    echo "publish-crates: dry-run — packaging + verifying ${#CRATES[@]} crates against a local overlay registry"
    cargo publish --dry-run "${PKG_ARGS[@]}"
    echo "publish-crates: dry-run OK — all ${#CRATES[@]} crates package and build in publication order"
    exit 0
fi

WORKSPACE_VERSION="$(cargo metadata --format-version 1 --no-deps \
    | python3 -c 'import json,sys; m=json.load(sys.stdin); print(next(p["version"] for p in m["packages"] if p["name"]=="memstead-schema"))')"

is_live() {
    curl -sf -A "memstead-publish-script (ci@memstead.com)" \
        "https://crates.io/api/v1/crates/$1/$WORKSPACE_VERSION" >/dev/null 2>&1
}

for c in "${CRATES[@]}"; do
    if is_live "$c"; then
        echo "publish-crates: $c@$WORKSPACE_VERSION already on crates.io — skipping"
        continue
    fi
    # Retry the upload: the 0.13.0 release lost exactly one crate to a
    # transient "Connection reset by peer" from crates.io and failed the
    # whole job over it. Between attempts, re-check liveness FIRST — an
    # upload can land server-side while the client sees the reset, and a
    # blind re-publish of a live version reads as failure. Persistent
    # errors (bad manifest, auth) still fail after the attempts.
    published=0
    for attempt in 1 2 3; do
        echo "publish-crates: publishing $c@$WORKSPACE_VERSION (attempt $attempt)"
        if cargo publish -p "$c"; then
            published=1
            break
        fi
        if is_live "$c"; then
            echo "publish-crates: $c@$WORKSPACE_VERSION is live despite the failed attempt — the upload landed"
            published=1
            break
        fi
        [[ "$attempt" == 3 ]] || { echo "publish-crates: attempt $attempt failed, retrying in $((attempt * 20))s"; sleep $((attempt * 20)); }
    done
    if [[ "$published" != 1 ]]; then
        echo "publish-crates: $c@$WORKSPACE_VERSION failed after 3 attempts" >&2
        exit 1
    fi
done

echo "publish-crates: done — all ${#CRATES[@]} crates published at $WORKSPACE_VERSION"
