---
type: architecture
title: Protobuf (proto3) stdlib
updated_round: 9
---

# Protobuf (proto3) stdlib

**New in round 6.** Kāra grew a real **Protocol Buffers (proto3)** standard-library surface, delivered as `runtime/stdlib/protobuf.kara` (+623) with the example `examples/protobuf_schema.kara` (+40). It is the **marquee consumer** of the round-6 [[metaprogramming|comptime/derive substrate]] — a genuine end-to-end proof that the substrate carries nontrivial, real-world codegen.

## Three-layer stack

The surface is built bottom-up in three slices, each layer standing on the one below:

1. **proto3 wire format** (runtime/stdlib) — `(ea1c527c)`. The proto3 binary wire format: **varint** and **length-delimited** encoding plus decoding, the hand-written primitive layer everything else rests on. The same commit also carried a **stdlib mut-ref-self fix** that this code depends on.
2. **`#[derive(Message)]`** (comptime codegen) — `(1c008f95)`. A derive that generates `encode`/`decode` for a user struct, implemented as **comptime codegen** over the new metaprogramming substrate rather than as a compiler builtin. This is where the substrate proves itself against a non-toy target. See [[attributes]].
3. **`.proto` schema → message types** (schema import) — `(027f4eb3)`. A path from a `.proto` schema file to Kāra message types — **schema-driven type generation** — exercised by `examples/protobuf_schema.kara`.

The layering is the point: **wire format → `#[derive(Message)]` → `.proto` import**. Each higher layer is expressed in terms of the layer beneath, so the whole path from a schema down to bytes is Kāra code, with only the varint/length-delimited primitives written by hand.

## Round-7 — the field-type matrix fills in

Round 6 shipped the three-layer skeleton with scalar fields; **round 7 built out the actual
proto3 field-type matrix**, a long slice run that grew `runtime/stdlib/protobuf.kara` by
another **+1169 lines** and `examples/protobuf_schema.kara` by +78. Each field kind was added
as a numbered slice, then the cross-cutting positions (repeated / map-value / oneof) were
filled in:

- **Nested message fields** (slice 4, `708cbdaa`).
- **Repeated fields** (slice 5, `74490863`).
- **Enum fields** (slice 6, `f84d674d`).
- **Map fields** — `map<K,V>` (slice 7, `3b2c2143`). Its un-annotated construction surfaced
  the `Map.new()` K/V-inference gap `B-2026-06-22-1` (see [[bug-tracker]]).
- **Float / double fields** (slice 8, `cba59aa3`).
- **`sint` / `fixed` / `sfixed`** — via **field-attribute reflection** (slice 9, `d3e70ec2`),
  i.e. the zig-zag / fixed-width wire encodings selected by a per-field attribute read through
  the [[metaprogramming|comptime reflection]] surface.
- **`oneof`** (slice 10, `4f1548d5`), then **message and enum `oneof` payloads** (`518f58ca`).
- **Cross-position coverage** — **float/double in repeated, map-value, and oneof positions**
  (`629c2d53`) and **enums in repeated and map-value positions** (`45cbf6d7`).
- **Per-field number overrides** (`3c563aa7`) — explicit field numbers for **sparse /
  non-contiguous** schemas (not just contiguous `1, 2, 3…`).

That the whole matrix — including `sint`/`fixed` selection and per-field numbers — is driven
by **field-attribute reflection** over the comptime substrate is the continuing proof that
[[metaprogramming|comptime]] carries real, non-toy codegen.

## Round 9 — protobuf compiles under codegen

Round 6/7 built protobuf as **interpreter-only** — a `#[derive(Message)]` round-tripped under `karac run` but **failed `karac build`/JIT**. Round 9's [[metaprogramming|derive-under-codegen 3-layer fix]] (`B-2026-07-08-15`) changed that: `std.protobuf`'s pure-Kāra encoder namespace + `ProtoReader` methods are now **compiled through codegen** (a `PROTOBUF_LOWERED_PROGRAM` joined into the compiled stdlib, gated on a `#[derive(Message)]` being present), so scalar / repeated-scalar / nested-message Message round-trips now agree **interp == JIT == AOT** (`fc6f2308`). Repeated-`Vec[String]` + `Map` fields followed once indexed-field-access method dispatch was generalized (`B-2026-07-09-1`). A `#[derive(Message)]` on a **bare enum** is now rejected with an actionable diagnostic (`95f704d4`, `B-2026-07-09-5`) — proto3 enums are field types, not standalone messages. See [[metaprogramming]], [[bug-tracker]].

## Tests

Coverage is split to match the slices:

- `tests/protobuf.rs` (+246) — the wire-format primitives.
- `tests/protobuf_derive.rs` (+352) — `#[derive(Message)]` comptime codegen.
- `tests/protobuf_proto.rs` (+223) — `.proto` schema import.

Together they lock the layering in place: a regression at any layer surfaces in its own suite, and the derive/schema suites double as executable evidence that the [[metaprogramming|comptime]] path produces correct encoders and decoders.

Related: [[metaprogramming]], [[attributes]], [[stdlib-and-traits]], [[codegen]], [[examples-and-benchmarks]], [[index]]
