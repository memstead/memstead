---
type: spec
created_date: 2026-07-15T10:51:22Z
last_modified: 2026-07-15T19:04:43Z
level: M1
stability: experimental
tags: self-hosting, lexer, phase-12, dogfooding
---

# Self-Hosting the Kara Compiler

## Identity
Kāra's Phase-12 self-hosting effort: a Kāra-language reimplementation of the compiler, beginning with a byte-indexed lexer written in Kāra (selfhost/src/main.kara), validated token-for-token against the Rust lexer by a differential oracle, and gated by an LLVM-C FFI proof that Kāra can drive LLVM directly.

## Purpose
To make the compiler compile itself — the strongest correctness and completeness proof for the native backend and stdlib, exercising them on a large real program rather than synthetic katas.

## Relationships
- **PART_OF**: [[kara-compiler]]
- **MOTIVATED_BY**: [[resequence-self-hosting-as-the-v1-pivot]]
- **DEPENDS_ON**: [[llvm-codegen-backend]]
- **DEPENDS_ON**: [[standard-library]]
- **REFERENCES**: [[resequence-self-hosting-as-the-v1-pivot]]
- **REFERENCES**: [[llvm-codegen-backend]]
- **REFERENCES**: [[standard-library]]
- **REFERENCES**: [[jit-execution-path]]
- **REFERENCES**: [[self-hosted-llvm-ir-emitter-backend]]

## Realization

- Self-host project: selfhost/src/main.kara (~1,800-line byte-indexed lexer), selfhost/kara.toml
- Differential oracle: tests/selfhost_lexer.rs (compares the Kāra lexer to the Rust lexer)
- LLVM-C FFI spike: docs/spikes/self-hosting-llvm-c-{ffi,proof,surface}.md, selfhost-lexer-profile.md
- Tracker: docs/implementation_checklist/phase-12-self-hosting.md


- The self-host source has grown from the single-file lexer into a module DAG: selfhost/src/{span,token,lexer,ast,ast_render,parser,main}.kara (parser ~2,400 lines, ast ~760, ast_render ~1,115, lexer ~1,600). The lexer was split span←token←lexer←main and later the parser layered on top.
- Differential parser oracles: tests/selfhost_parser.rs, selfhost_parser_types.rs, selfhost_parser_items.rs (Kāra parser vs Rust parser), joining the lexer oracle. scripts/oracle-sync-guard.sh guards oracle provenance.

## Specifies

- Byte-indexed lexer skeleton, then slices A–E: operators + keyword table (A), comments (line/block skip + doc tokens) (B), number forms — radix/float/separators/suffixes (C), string / multi-string / char / byte literals + f-string interpolation + c-strings (D), raw idents / reserved forms / non-ASCII recovery (E); `\u{…}` Unicode escapes; multi-line span coverage.
- A differential lexer oracle as a CI gate (Tier 1) — it caught auto-par bug #8 among others.
- LLVM-C FFI proof: a minimal Kāra program that links and calls LLVM-C, exit=42, meeting the spike DoD (all six sub-questions resolved).
- A 3-way bug triage classifying each lexer-port failure by which flow carries the Rust fix into the Kāra port.
- Real-world codegen levers surfaced by the self-hosted-lexer profile — chiefly string-literal `match` dispatch (a switch tree instead of a memcmp cascade), identified as the #1 codegen lever.


- Phase-12 has advanced from the lexer to a Kāra-language **parser**. Landed parser slices: 1 (+1.5 postfix expressions), 2a (control-flow expressions: blocks / if / return), 2b (loops, break/continue + labeled forms), 3a (type-expression parser), 3b (patterns + match), 3c (item grammar: 3c-i leaf items use/const/type, 3c-ii struct+enum, 3c-iii function items, 3c-iv trait + impl item grammar, 3c-v generic type params across items), and 3d-i (attributes + doc comments). The parser AST was rewritten to the direct shared-enum model, with an ast_render module (selfhost/src/ast_render.kara) round-tripping it.
- Splitting the lexer single-file → multi-file surfaced a cluster of cross-module name-resolution bugs (imported-enum variant registration, import-shadows-prelude, qualified `module.Type {}` construction, transitive imported field types) — all fixed (B-2026-06-15-4..7), after which main.kara dropped its `import span.Span` workaround. The self-host lexer also removed its #47 span_str_v ownership workaround (no longer reproduces).

- Sequencing: Phase-12 self-hosting was PAUSED to productionize the LLJIT (an owner-confirmed resequence — see [[jit-execution-path]]) and RESUMED once the LLJIT gate cleared.
- Oracle status: **all three parser oracles now pass and are un-ignored** (expression, items, types). The expression oracle's residual crash was the auto-parallelizer racing sequential `mut ref self` parser-cursor calls (fixed by recording receiver + let-RHS inner writes so they serialize, B-2026-07-09-12). The item/type oracles' render-time drop double-free on the wider item AST (FnDefNode/EnumDefNode with nested `Vec[struct]` + `Option[shared]`/bare-shared fields) was resolved through a multi-commit codegen slice — entry-copy fresh-vs-retained rc-inc plus clone-on-extract-from-view completeness — closing B-2026-07-10-4.
- Parser surface widened: `union` items, fn effect clauses, struct patterns + destructuring `let`, braced/multi-item imports, uppercase-rooted path expressions, loop-label resolution, and reserved match-arm binding suppression all parse.

- **Compiler stages beyond the parser have begun their Kāra port.** TypeChecker port: slices 1–21 landed (primitive inference + binding/param env, operator/call-return/method-call typing, struct-literal + field checks, argument-count / missing-field / extra-field checks, NonExhaustiveMatch exhaustiveness, enum-vs-X mismatch, StringNotIndexable, NotCallable, logical/arithmetic/bitwise operand validity, InvalidUnary/BinaryOp, if/else + match-arm + while-condition type consistency) — differential-oracle'd against the Rust typechecker (tests/selfhost_typechecker.rs). Resolver port: name-resolution core with a differential oracle (Slice 1), item-form surface (2a struct/enum/type-alias/const, 2b trait/impl, 2c prelude seeding), and a program-level two-pass resolver (parse_program + resolve_program). Codegen port Slice 1: a self-hosted **LLVM-IR text emitter** (println of strings) — the first proof the Kāra compiler can emit its own IR directly.
- Backend direction settled: the self-hosted-backend feasibility spike returned **GO** — see [[self-hosted-llvm-ir-emitter-backend]].

## Constraints

- The Kāra lexer must produce byte-identical tokens to the Rust lexer or the oracle fails.
- Codegen gaps block the port and are fixed against the oracle before the slice proceeds.

## Rationale

Realizes [[resequence-self-hosting-as-the-v1-pivot]]. Depends on the [[llvm-codegen-backend]] and [[standard-library]] being complete enough to express a compiler; each gap is filed as a numbered self-hosting blocker and fixed against the oracle.
