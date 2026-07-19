---
type: design-decision
title: Unsafe, FFI, raw pointers, and strict provenance
updated_round: 9
---

# Unsafe, FFI, raw pointers, and strict provenance

Round 2 built out Kāra's low-level / `unsafe` surface substantially — a group of Phase 5
diagnostics lines plus an `unsafe extern { }` block model. These are the escape hatches
for systems-level and FFI code, gated by `unsafe` and by targeted `E_*` errors.

## The `unsafe` surface

- **Module-scope `unsafe`** (v2 unsafe-track slice 1) — an `unsafe` surface at module
  scope.
- **`unsafe_op_in_unsafe_fn`** — an operation lint (slices 1–5) that requires `unsafe`
  operations inside an `unsafe fn` to still be wrapped; the rule epic is complete. The four
  lint-level attributes are **rejected** on it (see [[attributes]]).
- **`unsafe extern { }` blocks** — block-level `# Safety` doc-comment checks (slice 5a) with
  inline block-level prose per child page (5b), and **block-level `@noblock` propagation**
  through the effect checker (slice 4; see [[design-effect-system]]).

## FFI unions (Phase 5 lines 549–569)

A full **FFI union** type, delivered as slices 1–4 with a family of forbidding errors:

- Slice 1 — parse + resolve + **decl-time validation** (line 549).
- **`E_UNION_READ_REQUIRES_UNSAFE`** (2a, 549) — reading a union field needs `unsafe`.
- **`E_UNION_BORROW_REQUIRES_UNSAFE`** (2b, 561).
- **`E_UNION_LITERAL_REQUIRES_ONE_FIELD`** (2c, 563) — a union literal sets exactly one field.
- **`E_UNION_DROP_FORBIDDEN`** (3a, 565) — unions have no drop glue.
- **`E_UNION_NON_EXHAUSTIVE_FORBIDDEN`** (3b, 567).
- Slice 4 — **codegen lowering** (line 569).

## Opaque foreign types

- **`ExternItem::OpaqueType`** (slice 1a) — opaque foreign types declared in extern blocks,
  with **use-site precision** for opaque foreign type uses (slice 1b) and per-child doc
  pages for `unsafe extern { }` block items.
- **`E_OPAQUE_TYPE_NO_METHODS`** (line 523) — an opaque foreign type exposes no methods.

## Raw pointers and strict provenance

- **Raw pointer construction** (line 573) — **`ptr.const` / `ptr.mut`** plus stdlib surface.
- **Strict provenance** (slices 1–3) — **`ptr↔int` cast rejection** (int-to-pointer and
  pointer-to-int casts are rejected in favor of provenance-preserving APIs), a `ptr` stdlib
  API surface, and codegen lowering for the `ptr.*` APIs.
- **`ptr.offset_of`** shipped; **`ptr.container_of` / `ptr.container_of_mut`** (line 509)
  landed after `container_of` was briefly **soft-blocked on line 511** (see [[deferred-work]]).
- A **layout intrinsic family** (from the unsafe-extern slices) supports these.

## C-string literals

- **`c"..."` c-string literals** (line 587) — parser + typechecker support with a
  **`ref CStr`** type, for null-terminated FFI strings. (The lexer already recognized
  `c"..."`.)

## Round-5 FFI additions

Round 5 built out the **outbound** and **inbound** FFI surface (driven by the
[[self-hosting|self-hosting]] LLVM-C FFI need and the [[wasm-targets|host-fn]] work):

- **Exported `extern "C" fn` definitions** — Kāra functions callable from C, with a
  **C-unwind panics gate** (phase-6 line 47).
- **`#[link_name]`** — honored on `unsafe extern fn` imports (rename the linked symbol).
- **`kara.toml [link]` directive** — declares a foreign library to link against
  (manifest-level linking).
- **Raw pointers are `Copy`** (`*const T` / `*mut T`) — a correctness fix, so passing a raw
  pointer does not move it.
- **CStr borrowed surface** — **`CStr.from_ptr(*const u8) -> ref CStr`** (an unsafe
  constructor) and **`CStr.to_string() -> Result[String, Utf8Error]`**. The `CStr` borrowed
  surface is **the first safe pointer-producer**, unblocking the `(ptr, len)` **host-fn**
  E2E. See [[wasm-targets]].
- **`process.exit(code)`** lowers to libc `exit`.

## Round-9 — additive interop, producer mode, and the `#[repr(C)]` ABI

Round 9 built out **"Kāra as a library"** — the ability to compile a Kāra crate into a static
or dynamic library that a **C or Rust host** links against — decided via an *additive-interop*
spike (`d610f3c7`, "an addable component, not a rewrite") and a **verified-independence** Slice-4
decision (`d7ce7754`). A new book chapter documents it (`ec623cb3`, **"Kāra as a Library"**, v1.x).

### Producer-mode library artifacts

- **`[lib]` manifest table** (`db05bbb8`) — a `kara.toml` project-mode library build.
- **Producer-mode artifacts** (`8dcbc155`, Slices 2/3/3½/5) — static + dynamic library outputs,
  plus **Windows library artifacts** (`4ce20a08`).
- **Rust-host static-link `std` collision smoothed** (`06452c44`) — linking a Kāra staticlib
  into a Rust host no longer collides on the two runtimes' `std`.
- **`forget[T]`** (`1ba35317`) — an **FFI ownership-handoff primitive** (Slice 4, part 1): move
  a value's ownership out of Kāra's drop tracking so a C host can take it.

### The C boundary — auto-boxing, raw-pointer methods, `#[repr(C)]` enums

- **Raw-pointer instance methods** (`cfbf0e72`, Slice 4 Path A) — `p.offset(i)` / `p.read()` /
  `p.write(v)`, plus **method return types + `is_null` method-form + unsafe enforcement**
  (`172061db`). A focused **`E_RAW_POINTER_UNRESOLVED_POINTEE`** diagnostic fires at the read
  site when a `*const T`'s pointee `T` can't be resolved (`5b358be3`). (A **turbofish-inferred**
  pointer binding — `let p = ptr.null[u8]()` — still loses `T` in codegen, `B-2026-07-11-1`,
  open low; the annotated form works.)
- **Auto-boxing + auto-destructors** (`9b0cdc16`, Slice 4 Path B) — a Kāra value returned across
  the C ABI is auto-boxed with an auto-generated destructor; **nested boxed returns + an
  ABI-honesty gate** followed (`9a233aa5`).
- **`#[repr(C)]` enums cross the C ABI** — **all-unit** enums cross **transparently**
  (`1d023959`), and a **scalar-payload** `#[repr(C)]` enum crosses as a **boxed tagged union**
  (`a143237b`). **Category-specific export-ABI rejection diagnostics** (`ad671699`) and a
  sharpened **`E_EXPORT_ABI`** that suggests `#[repr(C)]` on struct returns (`220a9cc7`).

### `#[repr(C)]` struct-by-value ABI (`B-2026-07-09-2`) — a silent miscompile CI caught

A `#[repr(C)]` struct passed by value across the export boundary was **mislowered on AArch64**
(Apple silicon): codegen emitted a raw LLVM struct-by-value type and relied on the LLVM backend
default, whereas a real C frontend does explicit **per-target ABI classification** in the IR.
`stats_mean(Stats{ f64 sum=30.0; i64 count=4 })` returned `0.00` instead of `7.50` — a **silent
wrong-data miscompile**, caught by the new **macOS-arm64 CI leg**. A real classifier now emits:

- **AArch64 AAPCS** — HFA → float v-regs; non-HFA ≤16B → `[N x i64]`; >16B → **indirect** ptr
  param (`6a6294fc`) / **sret (`x8`)** return (`c3a68206`). Params `991d3e2c`, returns `fa180294`.
- **x86-64 SysV** — eightbyte INTEGER/SSE classification; >16B → `byval`/`sret` (`bc6a78cc`).
- **Microsoft x64** — 1/2/4/8-byte → a single integer register; every other size → by
  reference (params) / `sret` (returns) (`4c90993d`, `B-2026-07-09-8`).

Verified by a **`KARAC_FORCE_TARGET_ARCH` signature-match test** on Linux (identical IR ⟹
identical ABI) plus the **5-target CI matrix**; the Windows *execution* leg was deferred
(upstream `llvm-config.exe` missing on the Windows LLVM installer). See [[codegen]], [[cli]].

### C strings

- **Owning `CString`** + **`String.to_cstring`** + a **`NulError`** (`c8189cea`, phase-8
  C-string literals) — an interior-NUL-checked owning null-terminated FFI string.
- **`CStr.to_string_slice`** (`050fec14`) — a **zero-copy UTF-8 view** of a borrowed `CStr`,
  the round-5 `CStr.to_string` (copying) sibling.

Related: [[design-effect-system]], [[codegen]], [[stdlib-and-traits]], [[attributes]],
[[deferred-work]], [[self-hosting]], [[wasm-targets]], [[package-management]].
