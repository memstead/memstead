---
type: spec
created_date: 2026-07-15T18:46:18Z
last_modified: 2026-07-15T18:46:18Z
level: M1
stability: experimental
tags: interop, ffi, producer, repr-c, abi, library, phase-8
---

# Additive Interop and Producer Mode

## Identity
Kāra's producer-side interop surface: compiling Kāra as a linkable library that a C or Rust host calls INTO — `pub extern "C"` exports, per-target `#[repr(C)]` struct-by-value calling conventions, cross-boundary ownership handoff, and producer-mode build artifacts — the inverse of the consumer FFI in [[unsafe-and-ffi-surface]].

## Purpose
To let a team add a Kāra module to an existing codebase and call it as a normal native library, lowering adoption to an additive step instead of a whole-program rewrite.

## Relationships
- **PART_OF**: [[kara-compiler]]
- **DEPENDS_ON**: [[llvm-codegen-backend]]
- **MOTIVATED_BY**: [[ship-producer-and-library-mode-as-a-v1-direction]]
- **CONTRASTS_WITH**: [[unsafe-and-ffi-surface]]
- **REFERENCES**: [[unsafe-and-ffi-surface]]
- **REFERENCES**: [[ship-producer-and-library-mode-as-a-v1-direction]]
- **REFERENCES**: [[llvm-codegen-backend]]
- **REFERENCES**: [[wasm-target-backend]]

## Realization

- Export ABI + C header: src/cheader.rs (C header generation), src/codegen/param_own.rs (ownership at the boundary)
- Per-target repr(C) ABI: src/codegen/functions.rs + types_lowering.rs (AAPCS / SysV / Windows-x64 classifiers), tests/abi_repr_c_struct.rs (forced-arch signature-match gate)
- Producer-mode link + artifacts: src/codegen/driver.rs, src/manifest.rs ([lib] table)
- FFI ownership primitives: runtime/stdlib/mem.kara (forget), runtime/stdlib/nul_error.kara, owning CString
- Docs + demo: docs/book/src/ch18-interop.md ("Kāra as a Library"), docs/spikes/additive-interop-adoption.md, examples/interop/ (host.c, host.rs, kernel.kara)

## Specifies

- Export ABI: `pub extern "C"` function exports with category-specific export-ABI rejection diagnostics; a generated C header for the exported surface.
- `#[repr(C)]` struct-by-value calling conventions per target: AArch64 AAPCS (HFA / <=16B register / >16B indirect + sret), x86-64 SysV (eightbyte INTEGER/SSE classification, >16B byval/sret), and Windows x64 (1/2/4/8-byte register, else by-reference/sret). Validated identical to clang via a Linux forced-arch signature-match test (identical IR => identical ABI); AArch64 confirmed on real hardware. The Windows native-execution CI leg is deferred on an upstream `llvm-config.exe` gap.
- Cross-boundary ownership handoff: `forget[T]` (relinquish a Kāra-owned value to the host), auto-boxing + auto-destructors (Slice-4 Path B), owning `CString` + `String.to_cstring` + `NulError`, and raw-pointer instance methods (`offset`/`read`/`write`) for manual buffer handoff (Path A).
- Producer-mode build artifacts: static/dynamic library builds via a `[lib]` manifest table (project mode) and single-file mode, Windows library artifacts, and a smoothed Rust-host static-link std collision.
- WebAssembly producer bindings: reproducible embedded-component (WIT) output — the wasm arm of the same producer direction.

## Constraints



## Rationale

Realizes the owner decision to make producer/library mode a v1 direction ([[ship-producer-and-library-mode-as-a-v1-direction]]); built on the [[llvm-codegen-backend]] emission. The WebAssembly component/WIT bindings are the wasm arm of the same producer story (see [[wasm-target-backend]]).
