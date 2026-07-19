---
type: spec
created_date: 2026-07-15T08:40:58Z
last_modified: 2026-07-15T10:24:06Z
level: M1
stability: experimental
tags: repl, jupyter, playground, wasm, tooling
---

# Interactive Surfaces REPL Jupyter Playground

## Identity
Kāra's interactive execution front-ends: the terminal REPL with rich value display and `%magic` commands, an out-of-process Jupyter kernel, and a wasm32 web playground — sharing one cell/session model with cross-cell providers. The kernel and playground run the tree-walking interpreter; the REPL defaults to the JIT execution path.

## Purpose
To give humans and agents a fast, stateful way to run Kāra without a full compile — an interactive notebook and browser sandbox for exploration, teaching, and demos.

## Relationships
- **PART_OF**: [[kara-compiler]]
- **DEPENDS_ON**: [[tree-walking-interpreter]]
- **REFERENCES**: [[tree-walking-interpreter]]
- **REFERENCES**: [[jit-execution-path]]

## Realization

- REPL: src/repl.rs, src/repl/display.rs (Value → DisplayBundle renderer), src/repl/util.rs
- Jupyter kernel: kernel/ crate (main.rs, connection.rs, transport.rs, wire.rs, zmq_transport.rs, runtime.rs) — execute_request→Session, complete / is_complete / interrupt handlers; kernel/python/ shim + kernelspec installer (karac_kernel)
- Playground: playground/ (wasm32 target, wasm-bindgen wrapper, src/lib.rs, web/ HTML+JS shell, build.sh)

## Specifies

- REPL rich display: Value → DisplayBundle renderer routed through the kernel's display_data; `%show` magic backed by MagicOutput.rich; the `%magic` surface dispatcher; `%rc` magic surfaces RC-fallback decisions (shipped, line 785, routed through display_data) — reversing the earlier v1.1.x deferral; `%timeline` magic + cross-cell effect-conflict dependency analyzer (line 773), backed by a per-cell structured effect snapshot in Session.
- Session model: value-snapshot persistent-let semantics; `:save` session export with compile + behavior fidelity; cross-cell providers with a notebook-aware closed-scope diagnostic that closes cross-cell providers at cell boundaries.
- Jupyter kernel: skeleton + socket wiring, execute_request routed to a Session, complete / is_complete / interrupt handlers, Python launcher shim + kernelspec installer.
- Web playground: wasm32 compile target, static HTML/JS shell, URL share via compressed fragment.

- REPL JIT integration: a `karac_jit_runner --repl-mode` persistent-engine subprocess (KARAC_REPL_JIT dispatch) executes cells via JIT, with value-snapshot persistent-let semantics ported to JIT for primitives, String, and Vec / Map / Set of primitives, plus cross-cell symbol amortization and impl-method dedup across cells (see [[jit-execution-path]]).

## Constraints

- The Jupyter kernel and wasm playground run the [[tree-walking-interpreter]] on a fat-stack scoped thread; the terminal REPL defaults to the JIT execution path (see [[jit-execution-path]]), with the interpreter as reference semantics.
- REPL/`karac explain` prose is human-facing and optimizes for readability, not machine parsing.

## Rationale

Human- and agent-facing surfaces over the [[tree-walking-interpreter]]. Rich display and `%magic` shipped as Phase-5 line 761/689; the kernel (line 719) and playground (line 703) are new out-of-process front-ends.
