---
type: concept
created_date: 2026-07-15T07:24:52Z
last_modified: 2026-07-15T19:09:44Z
maturity: stable
abstraction_level: concrete
tags: concurrency, codegen, auto-par
---

# Auto-Parallelization

## Definition
Auto-parallelization (auto-par) is the compiler transformation that takes ordinary sequential Kāra code, uses the concurrency analysis derived from effect signatures to prove which regions are independent, and lowers those regions to run concurrently on a runtime worker pool — without any user-written concurrency construct.

## Explanation
A `ConcurrencyAnalysis` pass computes, from each statement's effects, a dependency graph over a block. Regions with disjoint read/write effects are marked parallelizable. Codegen threads this analysis into its state and emits calls into `karac_par_run`, which dispatches work to a long-lived worker pool. A captured-mutation safety net and move-aware container handling guard correctness. The transformation is gated behind the `KARAC_AUTO_PAR` environment flag while it stabilizes, and users can inspect the result via `karac --concurrency-report`.


Auto-par extends to reductions: the compiler recognizes reduction shapes (including while-loop reductions), generalized over an (op, type) matrix, and lowers them to `karac_par_reduce` — a fan-out + serial-combine primitive whose workers share the same pool as `karac_par_run`. A cost-model gate decides when parallelizing a reduction is worth it. Measured up to 9.87× vs Rust on kata-7 and ~4.1× wall-clock on the initial narrow shape. Realized in src/codegen/reduce.rs.


Reduction shapes now recognized also include collect-style (`acc.push()` gated on `#[par_unordered]`), Min/Max, and conditional acc-update (`if cond { acc = acc + delta }` / two-arm conditional), guarded by a memory-bound rejection gate and an inlining-aware, per-statement cost gate.


Correctness hardening this round: console-output statements are never parallelized (they carry no resource effect yet race on stdout — B-2026-06-13-18); ordered-output support lets logging-bearing work be parallelized while preserving output order via an OutputCapture; and the dependency analysis now sees writes through a deref / method-chain assign target (`*m.entry(k).or_insert(0) += 1`), which previously recorded no write and let a mutating loop race a later read of the same map (B-2026-06-20-16). A fork-threshold query (P1.6) exposes the auto-concurrency cost-model decision.


Shallow-depth parallel reduction: a reduction whose per-iteration delta recurses into the enclosing function (a backtracking counter) is now recognized and lowered again. An earlier conservative compile-time decline of recursive-delta reductions was replaced by a runtime fork-depth cap (`KARAC_PAR_MAX_FORK_DEPTH`, default 1): only the outermost level fans out, deeper levels run inline, bounding nesting by a constant instead of exhausting the stack (docs/spikes/shallow-depth-parallel-reduction.md). Measured ~9.5× on an N-Queens counter. Correctness seeds also grew: `sort`/`sort_by`/`reverse`/`pop`/`remove` are seeded as receiver-mutating so auto-par serializes them against later reads, and owned Column/DataFrame/Tensor handles produced in a par branch now transfer across the slot boundary instead of being freed early.


Further correctness + cost-model hardening: the dependency scan now records receiver mutation for `mut ref self` method calls and reads inner-writes from `let` RHS, and recurses into f-string interpolations and match/if/while scrutinee+condition sub-expressions — so a mutating call nested in those positions serializes instead of racing (fixes the self-hosted parser's raced parser-cursor calls B-2026-07-09-12 and a lost mut-ref-self mutation inside an f-string/match B-2026-07-12-5). The cost model treats an empty-collection constructor (`Vec.new()` / `String.new()`) as zero-work constant-init so a hot function's prologue no longer fans out a spurious ~70µs par group (B-2026-07-09-14). A `KARAC_NO_AUTOPAR` escape hatch builds every loop sequentially.

## Relationships
- **REFERENCES**: [[effect-verb]]
- **REFERENCES**: [[auto-concurrency]]

## Boundaries

- Distinct from explicit `par` blocks / `Sender.send(closure)`, which the user writes by hand; auto-par inserts parallelism into non-par regions automatically.
- Depends on but is not the same as the [[effect-verb]] system — effects are the input, parallel execution is the output.

## Significance

Auto-par is the headline punchline of the [[auto-concurrency]] design and the thing the Parallax benchmark exists to demonstrate. It is what makes 'no async/await' a feature rather than a limitation.
 Reduction auto-parallelization is the second major auto-par payoff after independent-region parallelism, and its cost-model gate is the template for deciding when derived parallelism actually pays.
