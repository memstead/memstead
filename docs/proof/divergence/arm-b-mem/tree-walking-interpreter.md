---
type: spec
created_date: 2026-07-15T07:28:10Z
last_modified: 2026-07-15T18:48:46Z
level: M1
stability: evolving
tags: compiler, interpreter, phase-4
---

# Tree-Walking Interpreter

## Identity
The Kāra interpreter (Phase 4): a tree-walking evaluator over the checked AST that runs programs directly and serves as the reference semantics (oracle) for the codegen backend. After `karac run`, `karac test`, and the REPL flipped to JIT-default, it is the `--interp` / `KARAC_RUN_JIT=0` dev-and-debug backend rather than the default run engine.

## Purpose
To be the executable specification of language semantics that the JIT and native backends must match, and the fallback dev/debug run backend.

## Relationships
- **REFERENCES**: [[llvm-codegen-backend]]
- **REFERENCES**: [[provider-system]]
- **PART_OF**: [[kara-compiler]]
- **DEPENDS_ON**: [[effect-checker]]
- **DEPENDS_ON**: [[ownership-checker]]
- **IMPLEMENTS**: [[algebraic-data-types-and-pattern-matching]]
- **USES**: [[provider-system]]

## Realization

- src/interpreter.rs and src/interpreter/ (value.rs, exec.rs, eval_expr.rs, eval_call.rs, eval_stmt.rs, eval_ops.rs, method_call*.rs, pattern_match.rs, resource_method.rs, iter_eval.rs, builtin.rs, helpers.rs)
- src/repl.rs; tests/interpreter.rs, tests/repl.rs

## Specifies

- Full expression/statement/pattern evaluation; `Value` model and runtime error types.
- Resource-method dispatch through the [[provider-system]]; println/print/eprintln via Stdout/Stderr providers.
- Collections (Vec/VecDeque/Map/Set), iterators, Option/Result/Ordering, closures with comparator honoring, String.chars() per-char iteration.
- NLL drop placement, unified drop+defer cleanup stack, weak-reference behavior, `dbg()` task-id tagging.
- REPL: `--auto-clone` opt-in mode, notebook-aware use-after-move diagnostics, cell-byte-range tracking.
- Runs on a fat-stack scoped thread (8 MB) to survive deep recursion on Windows debug builds.

## Constraints

- Interpreter and [[llvm-codegen-backend]] must agree on observable semantics; divergence is a bug.

## Rationale

Phase 4; the reference implementation. Front-end guarantees are shared with codegen via one checked AST.
