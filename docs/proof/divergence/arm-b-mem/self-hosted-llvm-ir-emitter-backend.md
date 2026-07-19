---
type: decision
created_date: 2026-07-15T19:05:24Z
last_modified: 2026-07-15T19:05:24Z
status: accepted
decided_on: 2026-07-14
deciders: kara-maintainers
scope: system
tags: self-hosting, backend, codegen, phase-12, spike
---

# Self-Hosted LLVM-IR Emitter Backend

## Decision
We chose to build the self-hosted Kāra compiler's code generator as a Kāra-native **LLVM-IR text emitter** — the Kāra compiler prints LLVM IR directly and hands it to the LLVM toolchain — rather than driving LLVM's C API through FFI from the self-hosted compiler. The self-hosted-backend feasibility spike returned GO, and Codegen port Slice 1 (println of strings) landed as the first proof.

## Context
Self-hosting (see [[self-hosting-the-kara-compiler]]) had reached the parser and typechecker/resolver ports; the next question was how the Kāra-in-Kāra compiler would generate machine code. An earlier LLVM-C FFI spike proved Kāra can call LLVM directly (the gate for starting the lexer port), but binding the full LLVM-C surface from Kāra is a large, brittle dependency. A backend-feasibility spike scoped emitting LLVM IR as text instead — the same textual IR the Rust [[llvm-codegen-backend]] already dumps — and found it feasible.

## Consequences
- The self-hosted compiler emits textual LLVM IR and shells out to the LLVM toolchain, avoiding a large hand-bound LLVM-C FFI surface in Kāra.
- The port advances stage-by-stage: Codegen Slice 1 emits `println` of string literals; later slices grow the emitter.
- A real type representation for the self-hosted TypeChecker was scoped as a companion spike (docs/spikes/selfhost-typechecker-real-types.md) so the emitter has typed input.
- Trackers: docs/spikes/selfhost-backend-feasibility.md (the GO finding), phase-12-self-hosting.md.

## Relationships
- **MOTIVATED_BY**: [[self-hosting-the-kara-compiler]]
- **REFERENCES**: [[self-hosting-the-kara-compiler]]
- **REFERENCES**: [[llvm-codegen-backend]]

## Options

- Drive the LLVM-C API via FFI from the self-hosted compiler — rejected for this round: a large, brittle hand-bound surface, though the FFI proof spike showed it is possible.
- Emit LLVM IR as text and hand it to the toolchain — chosen: reuses the textual-IR contract the Rust backend already exercises, with a far smaller Kāra-side surface.
- Target a simpler existing backend (e.g. a bytecode VM) for the self-hosted compiler — rejected: would diverge from the production native backend the project is proving out.

## Notes


