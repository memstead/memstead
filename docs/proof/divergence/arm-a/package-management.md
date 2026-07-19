---
type: architecture
title: Package management and dependency resolution
updated_round: 9
---

# Package management and dependency resolution

**New in round 3.** A large slice of Phase 5 built `karac` into a real package manager:
manifest parsing, a **PubGrub-style dependency resolver**, a lockfile, vendoring, a
registry proxy, a build cache, an install-spec surface, and a toolchain reader. This is
the round's single biggest new subsystem (thousands of new lines across `src/dep_graph.rs`,
`dep_resolver.rs`, `dep_diagnostic.rs`, `lockfile.rs`, `install_spec.rs`,
`registry_proxy.rs`, `build_cache.rs`, `karac_toolchain.rs`, and a much larger
`manifest.rs`).

## Manifest and dependencies

- **Structured `[dependencies]` / `[dev-dependencies]`** capture in the manifest (slice 1
  of the resolver, line 813).
- Dependency version strings lift into **`semver::VersionReq`** (slice 2).
- **Target-overlay merge**, a **dev-deps test-mode split**, a **`toolchain.toml` reader**
  (`karac_toolchain.rs`), and **run-script manifest discovery** (lines 882/884/892/898).

## The resolver (PubGrub, line 813)

Delivered as 7 slices:

- **`dep_graph`** — workspace deref + path-dep walking + cycle detection (slice 3).
- **`dep_resolver`** — topological resolution with a version-conflict chain (slice 4).
- **`dep_diagnostic`** — a **rustc-style conflict renderer** with the full constraint chain
  (slice 5).
- **MSRV enforcement** — a `kara-version` constraint checked against the active toolchain
  (slice 6, also closes line 842).
- **CLI wiring** closes line 813 (slice 7).

## Lockfile (`kara.lock`, line 831)

Four slices:

- **Lockfile schema + TOML reader/writer** (`lockfile.rs`, slice 1).
- **BLAKE3 hashing** for path-dep manifest content (slice 2).
- **`Resolution → Lockfile`** conversion (slice 3).
- **CLI integration** closes line 831 (slice 4).

The lockfile schema was later **extended with a proxy-mirror reference** (registry-proxy
slice 3, line 851).

## CLI package commands

See [[cli]] for the command surface. Round-3 additions:

- **`karac update`** — bare-form (update all, line 843 slice 1) and **surgical-form
  validation** (update a named package, slice 2, closes line 843).
- **`karac vendor`** — copy path-deps into `./vendor/` (ships line 859).
- **`karac install`** — consumes an **install-spec** parser (`install_spec.rs`, line 871)
  and **builds + installs path-source binaries** (line 874).
- **`karac cache`** — `info` + key inspection over the **build cache** (`build_cache.rs`,
  line 861); the build-cache typed surface landed first (slice 1).
- **`karac build --offline`** — consults `vendor/` for path-deps (line 880).
- **Registry proxy** (line 851) — a registry-proxy client (`registry_proxy.rs`), a
  **`--no-proxy`** flag plumbed through build/update/vendor, and the lockfile proxy-mirror
  extension.

Several v1.1.x carve-outs were recorded against these lines (deferred sub-scope). See
[[deferred-work]].

## Round 8 — dependency fetching

Round 8 turned the resolver into a real fetcher: git dependencies, a registry-proxy
subsystem, and PubGrub version-solving.

### Git dependencies

`karac` can now fetch git dependencies through a clone/checkout primitive (slice 1),
wired through the dep-graph, resolver, and CLI (slice 2), with git deps **pinned to their
resolved commit in `kara.lock`** (slice 3). Compiler code lives in `src/git_fetch.rs`,
with e2e coverage in `tests/git_fetch_e2e.rs`.

### Registry proxy (new subsystem)

A registry-proxy fetch layer with a documented wire protocol
(`docs/registry-proxy-protocol.md`). It ships as a **new `registry-proxy/` crate**
(`src/lib.rs`, `src/main.rs`, README) plus compiler-side `src/registry_proxy.rs` and
`src/registry_extract.rs` (tarball extraction):

- **fetch orchestration + version selection** (slice 1), **tarball extraction** (slice 2),
  **resolve + recurse registry dependencies** (slice 3), and **CLI activation** (slice 4).
- transport layering — a live **`HttpProxyClient`** + reference server, a
  **`CachingProxyClient`** client-side tarball cache, and a **`RetryingProxyClient`** that
  retries transport failures with backoff.
- a **`build` subcommand** to assemble a store in one command (see [[cli]]).
- manifest **`[build].registry-proxy` pin** + resolution precedence; proxy auth via the
  **`KARAC_REGISTRY_TOKEN`** env var.
- **`--no-proxy`** direct-from-source registry fetch.
- client-side **yank awareness** in the catalog + version selection.

### PubGrub version resolution

The resolver now routes through **PubGrub** version-solving (`src/pubgrub_solve.rs`): a
pubgrub primitive (slice 1), routing resolve through pubgrub version selection (slice 2),
a **`RegistryProvider`** candidate-set fetch (methods + recording the candidate set in the
dep-graph, slices 3a–3c), and **backtracking over the widened candidate set** (slice 3d).
This drove large churn in `src/dep_resolver.rs` and `src/dep_graph.rs`.

### wasm playground

Native-only registry/migration deps are **gated off `wasm32`** so the browser playground
still builds. See [[wasm-targets]].

## Round 9 — lockfile pinning, target deps, and workspace discovery

Round 9 tightened the resolver's determinism and reach (dependency-resolution follow-ups):

- **Pin-preference version selection** (`79abb44b`, slice 1) — the resolver prefers a version
  already **pinned in `kara.lock`** over freely re-solving.
- **Lockfile-pin-over-catalog + `W_DEPENDENCY_YANKED`** (`94d9ee5e`, follow-up (h)) — a pin
  wins over a newer catalog candidate; using a **yanked** version emits a warning rather than
  silently resolving it.
- **`[target.<triple>.dependencies]`** (`630ee1e1`, follow-up (e)) — per-target dependency
  tables are consumed in `test` / `resolve` / `update`.
- **Upward workspace-root discovery** (`117b1fd0`, follow-up (g)) — the workspace root is
  discovered by walking **up** from a member package.
- **Dependency-resolution diagnostics** surfaced in `karac run` (`1e0b9d4e`) and `karac check`
  (`d48c2653`), with a `--output=json` resolver-diagnostic shape pinned (`6c33f931`). See
  [[cli]].

Related: [[cli]], [[implementation-phases]], [[compiler-pipeline]], [[design-ai-first-compiler]],
[[design-unsafe-ffi-and-pointers]].
