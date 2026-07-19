---
type: spec
created_date: 2026-07-15T09:01:22Z
last_modified: 2026-07-15T18:48:09Z
level: M1
stability: evolving
tags: package-manager, dependency-resolution, lockfile, phase-5, cli
---

# Package Manager and Dependency Resolution

## Identity
The Kāra package-manager and dependency-resolution subsystem (Phase 5): a Cargo-like manifest / resolver / lockfile stack — kara.toml manifests, a PubGrub version-solving resolver, a kara.lock lockfile, offline and vendored builds, a build cache, a registry-fetch stack (tarball extraction, git dependencies, yank awareness) reached through a registry proxy over the [[registry-proxy-fetch-protocol]], and a toolchain (MSRV) gate — all driven through [[karac-cli]].

## Purpose
To give Kāra projects reproducible, resolvable dependency graphs and offline-capable builds — the packaging floor a systems language needs before it can consume third-party code.

## Relationships
- **PART_OF**: [[kara-compiler]]
- **REFERENCES**: [[karac-cli]]
- **REFERENCES**: [[registry-proxy-fetch-protocol]]

## Realization

- Manifest: src/manifest.rs (structured [dependencies]/[dev-dependencies] capture, target-overlay merge, dev-deps test-mode split, run-script discovery), src/karac_toolchain.rs (toolchain.toml reader), src/install_spec.rs (install-spec parser)
- Resolver: src/dep_graph.rs (workspace deref + path-dep walk + cycle detection + registry candidate-set), src/dep_resolver.rs (topological resolution + version-conflict chains + MSRV enforcement), src/pubgrub_solve.rs (PubGrub version-solving primitive + backtracking over the widened candidate set), src/dep_diagnostic.rs (rustc-style conflict renderer)
- Lockfile: src/lockfile.rs (kara.lock schema + TOML reader/writer, BLAKE3 hashing of path-dep manifest content, git-dep resolved-commit pin, Resolution→Lockfile conversion)
- Registry fetch: src/registry_proxy.rs (fetch orchestration, retrying + caching clients, live HttpProxyClient + reference server), src/registry_extract.rs (tarball extraction + candidate-set fetch); build cache: src/build_cache.rs
- Git dependencies: src/git_fetch.rs (clone/checkout, wired through dep-graph/resolver/CLI)
- Tests: tests/cli.rs

## Specifies

- PubGrub-style resolver (line 813): dep-string → semver::VersionReq lifting; workspace deref + path-dep walking + cycle detection; topological resolution with a full version-conflict constraint chain; rustc-style conflict diagnostics; MSRV enforcement of a kara-version constraint against the active toolchain (closes line 842).
- Lockfile (line 831): kara.lock schema + TOML reader/writer, BLAKE3 content hashing of path-dep manifests, Resolution→Lockfile conversion, CLI wiring.
- CLI legs: `karac update` bare + surgical forms (line 843), `karac vendor` copies path-deps into ./vendor/ (line 859), `karac install` builds + installs path-source binaries (line 874), `karac build --offline` consults vendor/ for path-deps (line 880), `karac cache info` + key inspection (line 861).
- Registry proxy (line 851): registry-proxy client, `--no-proxy` flag plumbed through build/update/vendor, lockfile proxy-mirror reference.
- Manifest surface: target-overlay merge, dev-deps test-mode split, toolchain.toml reader, run-script manifest discovery.

- PubGrub version-solving (src/pubgrub_solve.rs): resolve routes through PubGrub selection with backtracking over the registry candidate set recorded in the dep graph; client-side yank awareness in the catalog + version selection.
- Registry fetch: proxy fetch orchestration + version selection over the [[registry-proxy-fetch-protocol]], tarball extraction, resolve-and-recurse of registry dependencies, direct-from-source fetch under `--no-proxy`, `[build].registry-proxy` manifest pin + resolution precedence, `KARAC_REGISTRY_TOKEN` proxy auth, transport-failure retry with backoff, and a client-side tarball cache.
- Git dependencies: clone/checkout primitive, threaded through the dep-graph / resolver / CLI, pinned to their resolved commit in kara.lock.
- `karac resolve`: read-only dependency-graph inspection.

- Resolution refinements this round: lockfile-pin-preference (a pinned `kara.lock` version wins over the catalog) with a `W_DEPENDENCY_YANKED` warning; `[target.<triple>.dependencies]` consumed in test/resolve/update; workspace-root discovery walking upward from member packages; and dependency-resolution diagnostics surfaced in `karac check` / `karac run` (not only build), with an `--output=json` resolver-diagnostic shape.

## Constraints



## Rationale

Phase 5 packaging floor. Driven through [[karac-cli]]; distinct from the compiler front-end passes.
