---
type: architecture
title: Compiler pipeline
updated_round: 5
---

# Compiler pipeline (`karac`)

`karac` is implemented in Rust. The pipeline stages (each a module or module tree under
`src/`), in dependency order:

1. **Lexer** (`lexer`) — tokenizer. Supports raw identifiers (`r#NAME`), `c"..."`
   C-string literals, **`b'A'` byte-char literals** (round 3), codepoint-aware recovery for
   non-ASCII bytes, and a reserved `expr_<NNNN>` fragment-specifier identifier namespace.
2. **Parser + AST** (`parser`, `ast`) — recursive-descent with Pratt expression parsing;
   spans on every node; error recovery continues after errors and reports multiple
   diagnostics. The AST is split into `ast/{exprs,stmts,items,patterns,types}.rs`; the
   parser into `parser/{exprs,stmts,items,patterns,types,generics,attributes,
   items_effects,items_trait,items_extern}.rs`.
3. **Resolver** (`resolver`) — name resolution in two passes (Pass 1 top-level
   collection, Pass 2 item resolution) plus block/stmt/expr resolution and
   type/pattern/effect/bound resolution.
4. **Type checker** (`typechecker`) — type inference and checking; method resolution
   (inherent priority, autoref, `E0236` with typo suggestions), UFCS dispatch, const
   generics, derive validation, closure-capture inference. Heavily split into submodules
   (`typechecker/{exprs,items,patterns,inference,env,bounds,derives,lowering,
   stdlib_*}.rs`).
5. **Effect checker** (`effectchecker`) — see [[design-effect-system]].
6. **Ownership checker** (`ownership`) — see [[design-ownership]].
7. **Interpreter** (`interpreter`) — tree-walking evaluator (Phase 4); executes programs
   directly and backs the REPL. Split into `interpreter/{eval_expr,eval_call,eval_stmt,
   method_call,iter_eval,pattern_match,value,exec,...}.rs`.
8. **Codegen** (`codegen`) — LLVM backend (Phase 7); see [[codegen]].

Supporting analyses: `concurrency` ([[design-concurrency-and-providers]]), `exhaustive`
(match exhaustiveness), `provider_escape`, `rc_predicate`, `use_classifier`, `dominator`,
`cfg`, `lowering`, `desugar`, `manifest`/`module` (project/module graph), `formatter`,
`doc`, `scaffold`, `span_visitor`, and lint passes (`must_use_lint`, `missing_must_use_lint`,
`missing_track_caller_lint`, `unsafe_lint`, `logical_lint`, `ffi_lint`, `diagnostic_attrs_lint`,
`raii_check`).

**New round-2 modules** (agent/query and diagnostics surfaces):

- `diagnostic_class` — the `DiagnosticClass` taxonomy threaded through errors ([[design-ai-first-compiler]]).
- `def_path` — stable definition paths (Phase 8 P0 foundation) feeding the query surface.
- `call_graph` + `codegen_queries` + `queries` + `monomorphization` — the `karac query`
  back end: affected-by call-graph reach, inlining/branch-hint analysis, monomorphization
  query ([[cli]]).
- `catalog` — public-API surface index (JSONL) for `karac catalog`.
- `query_attributes` + `attribute_validator` — the [[attributes|attribute]] registry and
  tool-namespaced query.
- `cost_summary` extensions — provider/RC/shared cost visibility ([[design-ownership]]).

**New round-3 modules:**

- **Package manager** ([[package-management]]): `dep_graph`, `dep_resolver`,
  `dep_diagnostic`, `lockfile`, `install_spec`, `registry_proxy`, `build_cache`,
  `karac_toolchain`, plus a much larger `manifest`.
- **Concurrency safety**: `cross_task_safe` + `typechecker/cross_task_check`
  ([[design-concurrency-and-providers]]); `ownership/concurrent_shared` +
  `ownership/par_capture_classify` (the concurrent-struct diagnostics + lock-block auto-fix,
  [[design-ownership]]).
- **Codegen**: `codegen/file`, `codegen/tcp`, `codegen/tls`, `codegen/task_group`,
  `codegen/module_bindings`, `codegen/json` ([[codegen]], [[networking]]).
- **Effect checker**: `effectchecker/modbind_synth` (module-binding synthetic resources,
  [[design-effect-system]]).

**New round-4 modules:**

- **JIT + test harness**: `codegen/lljit` (LLJIT/orc2), `bin/karac_jit_runner` (JIT
  subprocess + `--repl-mode`), `test_jit_dispatch`, `test_main_synth` (per-test main
  synthesizer), `repl/jit_runner_client` ([[codegen]], [[cli]]).
- **Coroutine async transform**: `codegen/coro` — the LLVM-coroutine network-boundary
  transform that supersedes the state-machine body-splitter ([[design-runtime-phases]]).
- **[[design-contracts-and-verification|Phase 9 verification]]**: `codegen/contracts`,
  `codegen/refinement`, `typechecker/refinement_elision`, plus the distinct-type paths.
- **Crash diagnostics**: `codegen/debug_info` (panic location + DWARF); `codegen/test_assert`
  (assert / assert_eq / assert_ne lowering).
- **Backpressure interpreter dispatch**: `interpreter/method_call_{bounded_channel,
  rate_limiter,semaphore,process}` ([[design-concurrency-and-providers]]).
- **Tracing**: `runtime/src/tracing`; **JIT thread-local**: `runtime/src/emutls`.
- Other codegen splits: `codegen/assoc_call`, `codegen/par_blocks`, `codegen/synth`; a new
  top-level `lints` module.

**New round-5 modules:**

- **[[rc-elision|RC elision]]**: `ownership/elision` (+3521) and `ownership/ref_return`
  (borrow-return analysis), `ownership/rc_promote`.
- **[[wasm-targets|Targets / WASM]]**: `target` (the target model), `wasm_exports`,
  `wasm_glue`, `wit`, `componentize`, `codegen/cabi` (canonical ABI), `codegen/mono`,
  `effectchecker/target_gate` ([[design-effect-system]]).
- **[[simd|SIMD]]**: `simd_report`; **[[numerical-stdlib-and-tensors|Tensor]]**:
  `codegen/tensor`, `interpreter/method_call_tensor`, `typechecker/expr_method_tensor`.
- **[[fallible-allocation|Fallible allocation]]**: `fallible_alloc`,
  `typechecker/alloc_rejection`.
- **Numerics / variance**: `numeric_conv`, `typechecker/variance`, `typechecker/const_eval`,
  `typechecker/env_build`, `typechecker/fields`.
- **Codegen splits**: `codegen/clone_drop`, `codegen/synth_drop`, `codegen/collections`,
  `codegen/channel`, `codegen/bounded_channel`, `codegen/control_flow_for`,
  `codegen/param_own`, `codegen/state`, `codegen/file`, `codegen/contracts`.
- **Buffered I/O interpreter dispatch**: `interpreter/method_call_{bufreader,bufwriter}`.
- **`span_visitor`** — a span-rebasing visitor (f-string interpolation span fixes).
- **Parser**: `parser/generics` gained `Dim`/`Shape` kinds + variadic shapes;
  `parser/items_extern` gained exported `extern "C" fn` + `#[link_name]`.

Sibling crates outside `src/`: the **`kernel/`** Jupyter kernel ([[jupyter-kernel]]), the
**`playground/`** wasm32 shell ([[playground]]), and the **`runtime/`** crate (which gained
`event_loop.rs`, **`scheduler.rs`**, **`file.rs`**, and **`tls.rs`** — the
[[design-runtime-phases|event loop]] and its [[networking|network I/O stack]] — and in round
5 `mutex.rs`, `channel.rs`, `bounded_channel.rs`, `seq_scheduler.rs`,
`wasm_threads_scheduler.rs`, `wasm_alloc.rs`, `clone.rs`, `map.rs`, and `fatal.rs`).

A structural note carried from round 1: much of the recent history is **`chore:` refactors**
splitting the monolithic files (`codegen.rs`, `typechecker.rs`, `interpreter.rs`,
`parser.rs`, `ownership.rs`, `effectchecker.rs`, `resolver.rs`, `formatter.rs`) into
submodule trees — a structural cleanup, not behavior change.

Related: [[implementation-phases]], [[stdlib-and-traits]], [[design-runtime-phases]].
