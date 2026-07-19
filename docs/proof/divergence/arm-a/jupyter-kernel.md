---
type: architecture
title: Jupyter kernel and notebook surface
updated_round: 3
---

# Jupyter kernel and notebook surface

New in round 2: Kāra ships a **Jupyter kernel** so `.kara` code can run in notebooks. It
is tracked as **Phase 5 line 719** (Jupyter kernel, 6 slices) with a cluster of
notebook-experience follow-ups (lines 681, 689, 671, 679, 761, 747). This is a major new
front for the [[design-ai-first-compiler|AI-first / interactive]] positioning.

## The kernel (`kernel/` crate)

A dedicated `kernel/` Rust crate implements the Jupyter wire protocol:

- `kernel/src/wire.rs`, `transport.rs`, `zmq_transport.rs`, `connection.rs` — the ZeroMQ
  transport and message framing.
- `kernel/src/runtime.rs` — the execution engine bridging notebook cells to a persistent
  Kāra **`Session`**.
- Built across **line 719 slices 1–6**: skeleton + wire + sockets (1–3),
  `execute_request → Session` integration (4), `complete` / `is_complete` / `interrupt`
  handlers (5), Python shim + kernelspec installer (6).

## Python shim + installer (`kernel/python/`)

A `karac_kernel` Python package provides the launcher and the `jupyter` kernelspec
installer (`install.py`, `launcher.py`, `kernel.json`, `pyproject.toml`), so the kernel
registers with an existing Jupyter install.

## Notebook execution model

- **Persistent `Session`.** Cells execute against one long-lived session so state carries
  across cells.
- **Persistent-let / value snapshots** (line 671) — a **value-snapshot persistent-let
  model** lets a `let` re-defined in a later cell take a fresh snapshot; this closed the
  line 669 parent item.
- **Cross-cell providers** (line 681, slices 1–5) — [[design-concurrency-and-providers|
  providers]] established in one cell remain available to later cells; a
  **notebook-aware closed-scope diagnostic** reports when a provider's scope has closed,
  with **cell-byte-range tracking**. Closing line 681 also fed a line 689 follow-up.
- **`:save` session export** (line 679) — export a session with **compile + behavior
  fidelity** (the exported program compiles and behaves like the interactive session).

## Notebook magics (`%magic`)

The **`%magic` surface** (Phase 5 line 747) graduated `[~] → [x]` in round 2. A `%magic`
dispatcher was wired for the notebook (line 689). Magics include **`%show`** (line 761),
which pairs with the rich display path below.

Round 3 added two more magics and **un-deferred `%rc`** (which round 2 had pushed to
v1.1.x — see [[history-reversals-and-deprecations]]):

- **`%timeline`** (line 773, shipped 2026-05-20) — a **cross-cell dependency analyzer**
  surfacing an **effect-conflict timeline**, built on a **per-cell structured effect
  snapshot** stored in the `Session`. Routes through `display_data`.
- **`%rc`** (line 785, shipped 2026-05-20) — surfaces RC-fallback decisions; routes through
  `display_data`.

## Rich display (`DisplayBundle`, line 761)

**Line 761 shipped 2026-05-19** — a rich display pipeline delivered in 4 slices:

1. `src/repl/display.rs` — a **`Value → DisplayBundle`** renderer.
2. **`%show` magic + `MagicOutput.rich`** field.
3. Route the `DisplayBundle` through Jupyter's **`display_data`** message.

This lets notebook output carry richer representations than plain text, routed through the
kernel's `display_data` channel.

## Relationship to the REPL

The kernel reuses REPL machinery (`src/repl/`, including `repl/display.rs` and
`repl/util.rs`) and the same `Session`/notebook-aware diagnostics used by the interactive
[[cli|REPL]]. The playground ([[playground]]) is a sibling interactive surface on wasm32.

Related: [[implementation-phases]], [[design-ai-first-compiler]], [[cli]], [[playground]].
