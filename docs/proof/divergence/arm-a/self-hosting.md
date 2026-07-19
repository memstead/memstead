---
type: implementation-plan
title: Self-hosting (Phase 12) — the Kāra-in-Kāra port
updated_round: 10
---

# Self-hosting — Phase 12

**New in round 5, the front of the roadmap in round 6.** Phase 12 is a new implementation
phase: **porting the compiler to Kāra itself**, starting with the lexer. It was made the
**v1 pivot** — the roadmap was **resequenced to `8 → 9 → 10 → 12 → 11`** (self-hosting jumps
ahead of the Phase 11 [[numerical-stdlib-and-tensors|stdlib longtail]]), because a
self-hosted front end is the sharpest real-world exercise of the codegen surface and drives
the whole v1 dogfood. See [[implementation-phases]], [[history-reversals-and-deprecations]].

Tracked in `docs/implementation_checklist/phase-12-self-hosting.md` (+156 lines this round)
and the `selfhost/` project. **Round 6 grew `selfhost/` from a single-file lexer into a
modular lexer + parser + AST**: `span.kara`, `token.kara` (+214), `lexer.kara` (+1596),
`ast.kara` (+764), `ast_render.kara` (+1115), `parser.kara` (+2408), and `main.kara` (~1630).
The self-host binary (`nbe_main`) is gitignored (`6486e118`).

## Why the lexer first — and why it drives codegen

The self-hosted lexer is deliberately the first port: it is self-contained, exercises the
[[stdlib-and-traits|string/byte stdlib]] hard, and surfaces codegen bugs at scale. A
**profiling spike** (`docs/spikes/selfhost-lexer-profile.md`) resolved that the **`match`
on string literals** was the **#1 real-world codegen lever** — which drove the
**[[codegen|string-literal `match` → switch-tree dispatch]]** landing (was a `memcmp`
cascade). See [[codegen]].

The lexer is **byte-indexed** (a recorded design decision) rather than char-indexed, for
codegen tractability and performance.

## Lexer slices (A–E) and follow-ons

The Kāra lexer was built in slices, each expanding the differential oracle:

- **Slice A** — operators + keyword table.
- **Slice B** — comments (line/block skip + doc tokens).
- **Slice C** — number forms (radix, float, separators, suffixes).
- **Slice D** — string / multi-string / char / byte literals; **D-cont** adds f-string
  interpolation + c-strings.
- **Slice E** — raw idents, reserved forms, non-ASCII recovery.
- **L1** — `\u{…}` Unicode escapes.
- **L3** — multi-line span coverage in the lexer oracle.
- Remaining follow-ons carved into trackable **L1–L4** items.

## Differential lexer oracle

A **differential lexer oracle** (`tests/selfhost_lexer.rs`, +602 lines) runs the
self-hosted lexer against the Rust lexer and diffs the token streams. It is a first-class
correctness gate — it **caught auto-par bug #8** (the auto-par dependency analysis was not
tracking `self` reads/writes; see [[bug-tracker]], [[design-concurrency-and-providers]]).

## Round 6 — modularization and the parser port

Round 6 split the self-hosted front end into a clean **module DAG** and began porting the
**parser**.

- **Module DAG** — the lexer was split into a `span ← token ← lexer ← main` dependency graph
  (`e0468c7a`), then `ast ← ast_render` and `parser` were added on top. This immediately
  surfaced a cluster of **cross-module name-resolution bugs** (`B-2026-06-15-4…-7`): imported
  enums must register their **variants** (not just the type name) so unqualified variant
  patterns resolve; an imported user type must **shadow** a same-named baked-prelude type;
  qualified `module.Type { … }` construction must parse; and an imported struct's **field
  types** must be re-resolved transitively in the defining module. All four fixed
  (`2b2c9acf`, `5867dfe6`) — this is where multi-file Kāra programs got real cross-module
  semantics. See [[compiler-pipeline]].
- **AST model** — the parser AST was rewritten to the **direct `shared enum` model**
  (`1f226847`): AST nodes are reference-counted shared enums, the recursive-heap shape that
  drove much of the round's [[codegen|codegen drop/ownership]] work. A **recursive-heap gate**
  was deliberately flushed with a focused kata burst **before** writing the parser
  (`B-2026-06-14-28`), and an **AST-port wrapping convention** was decided up front
  (`e093999b`, gate 1).

### Parser slices (1 → 3c)

The parser was built in slices, each extending a **differential parser oracle**
(`tests/selfhost_parser.rs` +800, `selfhost_parser_items.rs` +737, `selfhost_parser_types.rs`
+316) that diffs the Kāra parser's AST against the Rust parser's:

- **Slice 1 / 1.5** — expression parser + **postfix expressions** (`b2b0f45c`).
- **Slice 2a** — control-flow expressions: **blocks / `if` / `return`** (`6bef0c85`).
- **Slice 2b** — **loops, `break`, `continue`** (`4f59bb1f`), then **labeled** loops +
  labeled break/continue (`b6df2692`).
- **Slice 3a** — the **type-expression parser** (`7fb425d6`).
- **Slice 3b** — **patterns + `match`** (`6296b9ae`, after a codegen-regression stopgap).
- **Slice 3c** — item grammar: **leaf items** (`use`/`const`/`type`, `1d75ed67`),
  **struct + enum** items (`67d368d3`), and **function** items (`d15c36a7`).
- **Slice 3c-iv / 3c-v** (round 7) — **trait + impl item grammar** (`066c4f69`) and
  **generic type params across items** (`4b8501c2`), extending the parser AST
  (`selfhost/src/ast.kara`, `parser.kara`) to the last item kinds and generics.
- **Slice 3d-i** (round 8) — **`#[...]` attributes + doc comments** (`5c5d4326`). The
  self-hosted parser now handles attributes and doc comments, with churn in
  `selfhost/src/parser.kara` (+304), `selfhost/src/ast.kara` (+56), and a grown
  `selfhost/src/ast_render.kara` (+119, AST rendering); tests in
  `tests/selfhost_parser_items.rs` (+119).

### Round 8 — attributes + doc comments, and an ownership false-positive

The port continues on the direct `shared enum` AST model. This surfaced the ownership
false-positive `B-2026-07-03-26` (fixed `b4dd3ba8`): matching a **non-Copy field of a
BORROWED `mut ref self` receiver** was wrongly treated as **consuming `self`**, which had
reddened all three selfhost parser oracles. See [[bug-tracker]].

### Round 8.5 — self-hosting paused for LLJIT, then un-paused

Self-hosting was briefly **paused for the [[codegen|LLJIT productionization]] work**
(`fbbcfc9c`, which also corrected a false-DONE tracker), and the roadmap was **resequenced to
insert LLJIT-productionization before Phase 12** (`44169c73`, owner-confirmed). Once the JIT
gate cleared, Phase 12 self-hosting was **un-paused** (`2141da23`). See [[implementation-phases]].

### Round 9 — the parser drop/ownership saga

The port drove a long **multi-session drop/ownership investigation** on the `shared enum` AST.
It began by *fixing the oracle harness itself*: the three parser oracles had been **vacuously
skipping** — a Rust panic matched no error pattern, so `!bin.exists()` fell through to the
"no LLVM / missing archive" skip and reported a false "ok". They now treat a compiler panic /
signal-kill as a **hard failure**, and the whole parser port had been **differentially
unverified for weeks** while CI stayed green (`B-2026-07-09-11`, `706a71e4`).

With the harness honest, the parser *compiled but crashed at runtime*:

- **`B-2026-07-09-11`** — a **niche-optimized `Option[shared T]`** stored into a *conventional*
  4-word field slot crashed codegen (`parse_else() -> Option[Expr]` into `IfExpr.else_branch`).
- **`B-2026-07-09-12`** (control-flow expressions SEGV) — root-caused, after a long false
  lead that chased an "inline shared-enum packing offset", to **auto-par falsely parallelizing
  sequential `mut ref self` cursor calls** in `parse_if` (they share `self.pos`). Closed by
  teaching the write-dependency analysis that a `mut ref self` receiver and a `let` RHS's
  inner-writes constitute a write. See [[design-concurrency-and-providers]].
- **`B-2026-07-10-1`** — a **let-bound `Vec[shared]` element moved into an enum ctor**
  (`stmts.push(Stmt.Exp(ExprStmt{ expr })); Expr.Blk(block)`) read the statement expr back as a
  garbage `Error` node: the whole-struct move suppressor zeroed only the Vec `cap`, not `len`,
  so the source's drop still ran the len-driven per-element rc-dec walk over shared children.
- Along the way, an intra-function **clone-on-extract** family (six increments) handled moving
  a heap child out of a shared-enum-payload *view* — but the framing shifted repeatedly (the
  "deep-clone-on-bind" strategy leaked shared incs against move-out drop-suppression; the sound
  strategy is **clone/inc-on-extract**, scoped to the extract site).

The **expression parser oracle (`selfhost_parser`) is now green** (150/150 corpus). The
**item/type oracles stay `#[ignore]`'d** against the round's headline open bug **`B-2026-07-10-4`
(high)** — the item/type parser still crashes (heap corruption) on ~5 **attribute-arg** inputs;
the ledger entry carries a detailed next-agent handover (a parse-side premature-free of an
`Option[String]` nested in a `Vec[AttrNode]`). See [[bug-tracker]].

### Round 10 — the front end lands: parser green, resolver + typechecker ported, codegen begun

Round 10 was the biggest self-hosting round yet: the self-hosted front end advanced from a
partly-verified parser to a full **lexer → parser → resolver → typechecker** pipeline, with a
codegen backend begun.

- **Parser port fully verified — all three oracles green.** The round-9 headline open bug
  **`B-2026-07-10-4`** was CLOSED (`1b5f543`): a **caller-retains defensive-copy gap** for a
  by-value `Vec[struct]` argument carrying `Option[String]` / `Option[shared]` fields. **All
  three parser oracles — `selfhost_parser` (expressions), `selfhost_parser_items`,
  `selfhost_parser_types` — are now UN-IGNORED and GREEN**, running the full differential diff
  against the Rust seed parser. (The control-flow-expression SEGV that had blocked the
  expression oracle was root-caused not to construction/drop but to the **auto-parallelizer
  falsely racing three sequential `mut ref self` cursor calls in `parse_if`** — see round 9,
  [[design-concurrency-and-providers]].) New parser grammar this round: the **`union` item**
  (`e02f37d6`), braced / multi-item imports, struct patterns + destructuring `let`,
  struct-literal parsing, uppercase-rooted paths (`Vec.new`, `Token.Error`), loop-label
  resolution, `fn` effect clauses, explicit-discriminant enums, and a program-level two-pass
  resolver hook.
- **Resolver ported (new this round).** A full **Resolver port** landed in slices: Slice 1
  (name-resolution core + differential oracle, `06946960`), Slice 2a
  (struct/enum/type-alias/const items), 2b (trait/impl items), 2c (full prelude seeding), plus
  a program-level two-pass **`parse_program + resolve_program`**, use-import collection, and
  `selfhost/src/resolver.kara` (+1525 lines). New oracle `tests/selfhost_resolver.rs` (+512).
- **TypeChecker ported (new this round — the round's biggest self-host push).** A
  **TypeChecker port** landed across ~21 incremental slices, each a self-hosted checker feature
  verified against a differential oracle: primitive inference + first checks (1); binding/param
  env + identifier inference (2); operator result types (3); annotated `let` (4); call-return
  typing (5); struct-literal + field-access typing (6); field-name checks — Extra/UndefinedField
  (7); method-call typing (8); match exhaustiveness — NonExhaustiveMatch (9); enum-vs-X mismatch
  (10); MissingField (11); argument-count check (12); InvalidUnaryOp (13); InvalidBinaryOp on
  comparisons (14); StringNotIndexable (15); NotCallable (16); logical-operator operands (17);
  arithmetic/bitwise operand validity (18); if/else branch-type consistency (19); match-arm type
  consistency (20); while-condition check (21). Lands `selfhost/src/typechecker.kara` (+1529
  lines), extends `selfhost/src/ast.kara` / `ast_render.kara`, and adds
  `tests/selfhost_typechecker.rs` (+543). A design spike scoped a **real type representation**
  for the self-hosted checker (`docs/spikes/selfhost-typechecker-real-types.md`).
- **Codegen port BEGAN (new this round).** **Codegen port Slice 1 — an LLVM-IR emitter** that
  emits `println` of strings (`7a3cbcff`), `selfhost/src/codegen.kara` (+227),
  `tests/selfhost_codegen.rs` (+179). Gated by a **backend feasibility spike that returned GO**
  (`docs/spikes/selfhost-backend-feasibility.md`, "backend feasibility spike RESULT — GO"): the
  self-hosted compiler's own backend is feasible. See [[codegen]].
- **f16/bf16 lexing reversal (self-host lexer).** Bare `f16` / `bf16` were RESERVED in the
  self-hosted lexer but lexed as **identifiers** in the Rust seed (like `f32` / `f64`) — a spec
  contradiction (`design.md` said reserved keyword, but a `Tensor[bf16, …]` example used it as a
  type name). Resolved in favor of the seed (`B-2026-07-14-2`, `b517d59b`, `42841327`): the
  self-host lexer now lexes them as identifiers; the reduced-precision-float **type** stays
  Phase 7. This turned the `selfhost_lexer_matches_rust_lexer` oracle green. See [[bug-tracker]],
  [[history-reversals-and-deprecations]].

Net effect: the self-hosted front end now spans **lexer → parser → resolver → typechecker**,
with a **codegen backend begun** — a substantial step toward the v1 self-hosting pivot.

### Enabling language features

The parser port pulled real language features into v1:

- **Parallel / destructuring assignment `a, b = b, a`** (`2436ced2`, promoted to a first-class
  AST node `c9fce676`) — every RHS is evaluated before any target is written (so it swaps).
  It exists because the recursive-descent parser's `(node, pos)` return style wants it. See
  [[design-adt-and-pattern-matching]], the CHANGELOG.
- **Same-scope `let` shadowing** allowed (`49e26f18`, resolver) — `design.md § Variables`.
- **Tuple-destructure Copy** classification fixed (`B-2026-06-14-27`), latent until the
  parser's tuple-returning recursive descent.

### Workarounds retired

As the compiler caught up, port-only workarounds were removed: the `#47` `span_str_v`
ownership workaround (`8f58682a`, no longer reproduces), `main.kara`'s `import span.Span`
workaround (`cb076f9b`, once transitive field-type resolution landed), and a stale slice-3b
WIP patch (`5e1903e6`). An **oracle-sync-guard** (`scripts/oracle-sync-guard.sh`) plus a
`#31` provenance guard catch post-port seed drifts.

## The three-way bug flow

Self-hosting created a **3-way bug triage**: a bug found via the port can be (1) a real
codegen/compiler bug fixed in the Rust compiler, (2) a Kāra-language-surface gap, or (3) a
port-only issue. Round 5 codified **the three flows for carrying a Rust-side fix into the
Kāra port** and a **self-hosting kata-selection** discipline. A long **blocker chain**
(self-hosting blockers #1–#20 and the `B-2026-06-1x` clusters) drove most of round 5's
[[codegen|codegen ownership/drop fixes]] — e.g. blocker #1 (a struct field of a user enum
collapsing to i64), #4/#5/#6, and the `Vec[(.., String)]` clone-UAF. See [[bug-tracker]].

## LLVM-C FFI spike (self-hosting groundwork)

Three spikes de-risked calling LLVM from Kāra itself, a prerequisite for a self-hosted
**back end** (after the front end):

- `docs/spikes/self-hosting-llvm-c-ffi.md` — the FFI surface.
- `docs/spikes/self-hosting-llvm-c-surface.md` — the required LLVM-C API surface.
- `docs/spikes/self-hosting-llvm-c-proof.md` — a **minimal proof that RUNS** (a Kāra
  program driving LLVM-C to build and execute IR, **exit code 42** — DoD met). All six
  spike sub-questions were resolved.

Related: [[implementation-phases]], [[codegen]], [[bug-tracker]],
[[design-unsafe-ffi-and-pointers]], [[history-reversals-and-deprecations]].
