---
type: spec
created_date: 2026-07-15T07:25:51Z
last_modified: 2026-07-15T19:09:13Z
level: M1
stability: evolving
tags: effects, core, semantics
---

# Effect System

## Identity
Kāra's effect system: a static, inferred, per-function record of the capabilities a function exercises — six built-in verbs plus user-defined resources — verified across all call sites.

## Purpose
To make side effects first-class, checkable data so the compiler can enforce effect discipline and, crucially, derive concurrency from it without async/await.

## Relationships
- **REFERENCES**: [[auto-concurrency]]
- **REFERENCES**: [[colored-functions]]
- **REFERENCES**: [[effect-verb]]
- **DEFINES**: [[effect-verb]]
- **REFERENCES**: [[wasm-target-backend]]
- **REFERENCES**: [[fallible-allocation-profile]]
- **REFERENCES**: [[embedded-and-mmio-surface]]

## Realization

- src/effectchecker.rs and src/effectchecker/ (inference.rs, verify.rs, subtyping.rs, with_e.rs, bounds.rs, extern_ffi.rs)
- Effect syntax parsing: src/parser/items_effects.rs
- Built-in verbs: reads, writes, sends, receives, allocates, panics
- runtime/stdlib effect annotations (e.g. env.set with writes(Env))

## Specifies

- Six built-in effect verbs and user-declared resources, with parameterized effects (e.g. `writes(Db)`).
- Effect groups, `with E` / `with _` handler regions, and transparent effects.
- Bottom-up effect inference (Phase B), then verification of declared-vs-inferred.
- Call-site effect subtyping so a low-effect function is usable where a higher-effect one is expected.
- Per-method allocator effects for Map/Set; extern-fn effect annotation with an FFI linter and profile gate.
- Ambient resources: lowercase aliases `clock` / `rand` / `stdin` / `stdout` / `stderr` / `fs` with `with_provider` override, lowered to their ambient methods (env.args/var, rand.next_u64, Stdin.read_line, Stdout/Stderr print, FileSystem.write).
- `#[target(...)]` attribute + target-gate pass: each compile target provides a fixed resource set, and a function using a resource the target does not provide is rejected at resolution (E0411). See [[wasm-target-backend]].
- E0412 resource-receiver contradiction: a resource used as a method receiver against its declared effect clause is rejected, with a machine-applicable `ref self` rewrite.
- A `[profile]`-table knob substrate (ProfileConfig threading) gates profile-dependent effects — e.g. the [[fallible-allocation-profile]].

- A `Hardware` effect verb carried by the memory-mapped I/O intrinsics (volatile_read / volatile_write), flowing through the call graph so bare-metal register access is tracked — see the [[embedded-and-mmio-surface]].

## Constraints

- Effects must unify at `with E` boundaries or the program is rejected.
- Extern/FFI functions must declare their effects; block-level `@noblock` propagates.

## Rationale

The effect record is the input to [[auto-concurrency]]; it is what lets Kāra avoid [[colored-functions]]. See [[effect-verb]] for the concept.
