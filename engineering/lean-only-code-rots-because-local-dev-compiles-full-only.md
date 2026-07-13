---
type: memo
created_date: 2026-07-13T16:43:04Z
last_modified: 2026-07-13T16:43:04Z
status: closed
tags: observation, lesson, flavours, feature-gating, ci, engine
---

# Lean-only code rots because local dev compiles full only

## Claim
The lean flavour (`--no-default-features`) of the [[engine--memstead-engine-cargo-workspace]] went red at the workspace-store rebuild and stayed uncompilable until 2026-06-10 (33 build errors) without anyone noticing — because every local dev flow compiles the full flavour, so code reachable only under `cfg(not(feature = "mem-repo"))` is dead code that nothing type-checks between explicit lean runs.

## Context
- Observed and fixed in commit a371c668, which restored the CLI's lean-flavour compile.
- Three independent rot sites had accumulated in [[engine--memstead-cli-crate]]: the `output` module was feature-gated behind `mem-repo` although it has no full-only dependencies (25 of the 33 errors); the `vcs` imports `setup.rs` uses on both flavours were gated too; and the lean-only `UNSUPPORTED_WORKSPACE_SHAPE` arm constructed `CliError` with a stale `Option`-typed `code` field — the field had since become non-optional, but under the full cfg the arm is not compiled, so no local build ever saw the mismatch.
- The breakage window opened at the workspace-store rebuild and closed only when a lean build was explicitly run — the [[engine--modal-flavour]] split means each CI flavour job compiles a different subset of cfg arms, and the window shows the lean gate was not blocking merges during that period.

## Relationships
- **REFERENCES**: [[engine:memstead-engine-cargo-workspace]]
- **REFERENCES**: [[engine:memstead-cli-crate]]
- **REFERENCES**: [[engine:modal-flavour]]
- **REFERENCES**: [[split-engine-into-lean-and-full-flavours]]

## Substance

Two distinct rot mechanisms, both inherent to [[engineering--split-engine-into-lean-and-full-flavours]]:

1. **Over-gating** — a module or import gets gated behind `mem-repo` even though both flavours need it. The full build (the only one local dev runs — helper scripts wire `--features mem-repo`) compiles fine, so the error surfaces only on a lean build.
2. **Stale lean-only arms** — `cfg(not(feature = "mem-repo"))` code is invisible to the full type-checker, so workspace-wide refactors (here: `CliError.code` going from `Option<&str>` to `&str`) silently skip it. The arm rots in place until the next lean compile.

The shared exposure: any cfg-gated arm is compiled by exactly one of the two canonical CI runs (`--features mem-repo` / `--no-default-features`). Code that exists for flavour parity is therefore only as healthy as the *less-frequently-run* flavour build — and local convention makes that the lean one.

## Alternatives



## Outcome

Fixed 2026-06-10 in commit a371c668: the over-gates were removed (`output` module and shared `vcs` imports un-gated) and the stale arm updated to the non-optional `code` field. Both canonical suites green afterwards (`cargo nextest run --workspace --features mem-repo` and `--no-default-features`, 2017 tests each). The structural exposure remains: lean-only arms stay unchecked by local dev, so keeping the lean CI job blocking is what stands between a refactor and a repeat.


2026-07-02 sharpening (7d225bb): the exposure was worse than "lean runs are rare" — the canonical workspace-wide lean run itself never exercised the CLI's lean flavour. `cargo nextest run --workspace --no-default-features` still compiles `memstead-cli` WITH `mem-repo`, because `xtask` depends on the crate with that feature on and cargo unifies features across one build graph. Only a targeted `-p memstead-cli --no-default-features` build compiles the CLI's real `cfg(not(mem-repo))` arms (e.g. the schema-new follow-up that routes through a fresh init). `run-tests.sh` now carries that targeted true-lean CLI leg, closing the unification blind spot.
