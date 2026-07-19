---
type: contract
created_date: 2026-07-15T07:30:26Z
last_modified: 2026-07-15T18:16:08Z
protocol: cli
version: 0.1.0-pre
stable_since: 2026-07-01
deprecation_status: draft
tags: cli, interface
---

# Karac CLI

## Summary
`karac`, the Kāra command-line compiler. Exposes build/run, project management (clean/install/vendor), offline builds, diagnostics reporting, and an `explain` help channel. This is the primary human- and agent-facing interface to the compiler.

## Relationships
- **REFERENCES**: [[json-diagnostics-format]]
- **REFERENCES**: [[package-manager-and-dependency-resolution]]
- **REFERENCES**: [[jit-execution-path]]
- **REFERENCES**: [[wasm-target-backend]]

## Request Shape

```
karac build [--offline] [--concurrency-report] [--cost-summary] [--monomorphization-budget N]
karac clean
karac install
karac vendor
karac resolve                          # read-only dependency-graph inspection
karac new <name>                       # scaffold a project (Backend HTTP server template)
karac run [--timeout DURATION]         # run a program; --timeout bounds wall-clock
karac test                             # JIT-default; per-test timeout (default 30s, env override)
karac migrate [--atomic | --no-atomic] # shared→par<Type>; --atomic is the default heuristic
karac explain --concept=<name>         # e.g. --concept=closures
karac <repl and per-file build modes>
```
Project mode reads kara.toml (see examples/*/kara.toml) and compiles multi-file projects via a concat-super-program.

## Response Shape

- Native binary (build) or program output (run).
- `--concurrency-report`: human-readable renderer of what auto-par parallelized.
- `--cost-summary`: where RC / providers / shared-with-mut-fields cost is paid.
- Diagnostics to stderr as human text and, where enabled, structured JSON (see [[json-diagnostics-format]]).

## Errors

- Parse/resolve/type/effect/ownership errors: reported with source spans, multiple per run, with typo suggestions where applicable.
- Late-phase (per-module) diagnostics carry per-module file context.

## Versioning

Pre-1.0; the CLI surface tracks the roadmap. Subcommands and reporting flags added as phases land (clean/install/vendor/--offline in P1; --concurrency-report as auto-par Slice D).

## Deprecation



## Notes

Realization: src/cli.rs, src/cli/ (args.rs, help.rs, explain.rs); src/manifest.rs, src/module.rs, src/scaffold.rs.

Subcommands beyond build/run: `karac new` (scaffolds a project incl. a Backend HTTP server template), `karac update <pkg>` (bare + surgical), `karac cache info` + key inspection, `karac vendor`, `karac resolve` (read-only dependency-graph inspection), and `karac migrate` (shared→par<Type> cross-task migration; defaults to the Atomic[T] heuristic, `--no-atomic` opts out; typecheck-aware binding discovery, project-mode cross-file walk). `karac test` defaults to the JIT path with a per-test timeout and the REPL runs JIT (see [[jit-execution-path]]); `karac run --timeout` and `--monomorphization-budget` bound execution and codegen. Dependency resolution, the kara.lock lockfile, MSRV, offline/vendored builds, and the registry proxy are specified in [[package-manager-and-dependency-resolution]].


Target & codegen flags (Phase 10): `karac build --target=wasm_wasi | wasm_browser` (see [[wasm-target-backend]]), `--features` (opt-in wasm-threads, accepted-but-inert until built), `--bindings` (WASM output-shape selector), `--target-cpu` / `--target-features` overrides (flag + env + `[release]` table), and `--simd-report[=verbose]`. `karac check` does multi-target verification.
