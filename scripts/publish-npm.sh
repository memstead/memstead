#!/usr/bin/env bash
# Assemble and publish the Memstead npm package:
#
#   @memstead/wasm — the browser engine bundle, built from crates/memstead-wasm
#
# Usage:
#   scripts/publish-npm.sh --dry-run   # assemble + `npm publish --dry-run`, upload nothing
#   scripts/publish-npm.sh             # real publish (requires `npm login` as a
#                                      # member of the npm `memstead` org)
#
# The package is scoped, so publishes need `--access public`; that is
# also pinned via `publishConfig.access` in package.json, and passed
# explicitly below for good measure.
#
# Assembly: wasm-pack (preferred) or cargo+wasm-bindgen (fallback —
# wasm-pack's installer has no aarch64-apple-darwin asset) emits the
# web-target bundle into target/npm/wasm, then the checked-in
# crates/memstead-wasm/package.json is copied over the generated one —
# the checked-in manifest is authoritative (scoped name, files list,
# metadata). The crate README ships as the package README.
#
# Toolchain: rustup target wasm32-unknown-unknown, plus wasm-pack or a
# wasm-bindgen-cli matching the wasm-bindgen version in Cargo.lock.
#
# After the real publish, flip the memstead.io site to the published
# package (the site's activate-published-wasm script, maintained with the site).

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CRATE_DIR="$ROOT/crates/memstead-wasm"
OUT_DIR="$ROOT/target/npm/wasm"

DRY_RUN=0
for arg in "$@"; do
    case "$arg" in
        --dry-run) DRY_RUN=1 ;;
        *) echo "unknown argument: $arg (only --dry-run is supported)" >&2; exit 2 ;;
    esac
done

PUBLISH_ARGS=(--access public)
if [[ "$DRY_RUN" == 1 ]]; then
    PUBLISH_ARGS+=(--dry-run)
fi

rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR"

if command -v wasm-pack >/dev/null 2>&1; then
    echo "publish-npm: wasm-pack build --target web --release -> $OUT_DIR"
    wasm-pack build --target web --release --out-dir "$OUT_DIR" "$CRATE_DIR"
elif command -v wasm-bindgen >/dev/null 2>&1; then
    echo "publish-npm: cargo build --target wasm32-unknown-unknown --release -p memstead-wasm"
    cargo build --release --target wasm32-unknown-unknown -p memstead-wasm \
        --manifest-path "$ROOT/Cargo.toml"
    WASM="$ROOT/target/wasm32-unknown-unknown/release/memstead_wasm.wasm"
    echo "publish-npm: wasm-bindgen --target web --out-dir $OUT_DIR"
    wasm-bindgen --target web --out-dir "$OUT_DIR" "$WASM"
    if command -v wasm-opt >/dev/null 2>&1; then
        echo "publish-npm: wasm-opt -Os"
        wasm-opt -Os -o "$OUT_DIR/memstead_wasm_bg.wasm.opt" "$OUT_DIR/memstead_wasm_bg.wasm"
        mv "$OUT_DIR/memstead_wasm_bg.wasm.opt" "$OUT_DIR/memstead_wasm_bg.wasm"
    fi
else
    echo "publish-npm: neither wasm-pack nor wasm-bindgen found." >&2
    echo "  install hint: cargo install wasm-bindgen-cli --version <see Cargo.lock>" >&2
    exit 1
fi

# The checked-in manifest is authoritative; wasm-pack's generated one is not.
cp "$CRATE_DIR/package.json" "$OUT_DIR/package.json"
cp "$CRATE_DIR/README.md" "$OUT_DIR/README.md"
rm -f "$OUT_DIR/.gitignore"

# Fail fast if the bundle is missing anything the manifest promises.
for f in memstead_wasm.js memstead_wasm.d.ts memstead_wasm_bg.wasm memstead_wasm_bg.wasm.d.ts README.md; do
    [[ -f "$OUT_DIR/$f" ]] || { echo "publish-npm: missing $f in bundle" >&2; exit 1; }
done

echo "publish-npm: npm publish ${PUBLISH_ARGS[*]} (@memstead/wasm)"
(cd "$OUT_DIR" && npm publish "${PUBLISH_ARGS[@]}")

if [[ "$DRY_RUN" == 1 ]]; then
    echo "publish-npm: dry-run OK — @memstead/wasm assembles and passes npm publish --dry-run"
else
    echo "publish-npm: done — @memstead/wasm published"
    echo "publish-npm: next, flip the .io site to the published package (its activate-published-wasm script)"
fi
