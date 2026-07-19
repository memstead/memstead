---
type: decision
created_date: 2026-07-15T07:23:48Z
last_modified: 2026-07-15T17:52:16Z
status: accepted
decided_on: 2026-06-10
deciders: kara-maintainers
scope: subsystem
tags: runtime, binary-size, performance
---

# Binary Size Reduction Strategy

## Decision
We chose a multi-lever strategy to keep Kāra-compiled binaries small: `panic=abort`, symbol stripping (`strip -x`) with an audited keep-list, cross-archive LTO (including fat LTO), and dead-code elimination flags. Applied in phases (phase 1 strip + panic=abort + symbol audit; phase 2 cross-archive LTO + DCE).

## Context
A naive native binary links the whole Kāra runtime plus LLVM support, producing large executables. Systems-language credibility depends on lean output. The runtime maintains a `SYMBOL_KEEP_LIST.md` so stripping does not remove intrinsics the generated code needs.

## Consequences
- Smaller shipped binaries; `panic=abort` also removes unwinding machinery (matching the runtime's panic model).
- LTO/DCE increase link time.
- A symbol keep-list must be maintained so runtime intrinsics survive stripping.

## Options

- No size work — rejected: large binaries hurt the systems-language pitch.
- panic=abort + strip + LTO + DCE — chosen, staged across two phases.

## Notes

Additional size levers landed under this same strategy: lean fatal paths that stop std-IO from anchoring ~250 KB onto Vec/String binaries; a panic-free stable merge sort dropping the ~262 KiB large-N sort floor; outlined panic bodies + a folded contract-free fault prefix (restores kata-5 inlining, +16 KiB lean floor); and DWARF stripped from emitted .wasm by default (482→30 KiB).
 A `wasm_size` benchmark track (bench/wasm_size/) now measures emitted WebAssembly module size for Kāra vs Rust vs TinyGo (hello + a filter_core kernel), giving the size story a cross-language receipt.
