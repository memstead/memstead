---
type: spec
created_date: 2026-07-15T07:28:50Z
last_modified: 2026-07-15T19:08:26Z
level: M1
stability: evolving
tags: stdlib, prelude, phase-8
---

# Standard Library

## Identity
The Kāra standard library (Phases 8/11): the prelude and stdlib modules, defined as baked `.kara` source under runtime/stdlib/ via [[compiler-builtin-baking]], covering core types, traits, collections, and I/O modules.

## Purpose
To provide the language's out-of-the-box vocabulary — Option/Result, the operator and comparison traits, collections, iterators, and the std.* modules — as a self-documenting, dogfooded surface.

## Relationships
- **REFERENCES**: [[compiler-builtin-baking]]
- **REFERENCES**: [[bake-standard-library-in-kara-source]]
- **PART_OF**: [[kara-compiler]]
- **MOTIVATED_BY**: [[bake-standard-library-in-kara-source]]
- **REFERENCES**: [[network-runtime-and-cooperative-scheduling]]
- **REFERENCES**: [[tensor-and-numerical-stdlib]]
- **REFERENCES**: [[portable-simd]]
- **REFERENCES**: [[fallible-allocation-profile]]
- **REFERENCES**: [[wasm-target-backend]]
- **REFERENCES**: [[secret-handling-and-constant-time-comparison]]
- **REFERENCES**: [[embedded-and-mmio-surface]]

## Realization

- runtime/stdlib/*.kara (~60 files: option, result, vec, vec_deque, map, set, sorted_set, iterator, into_iterator, ordering, entry, hash, eq/ord/partial_*, add/sub/mul/div/rem/neg/bitand/bitor/bitxor/shl/shr, from/into/try_from/try_into, index, not, display/debug, atomic, f32/f64, encoding, regex, stats, http, json, cli, process, pool, tracing, runtime, io, io_error, var_error, intrinsics, memory_ordering, peekable, channel/sender/receiver, semaphore, rate_limiter, bounded_channel, mutex)
- src/prelude.rs (baking + registration)

- runtime/stdlib/io.kara + runtime/src/file.rs (File), tcp.kara, tls.kara, ws.kara, task_group.kara, drop.kara, utf8_error.kara

- New primitive modules: runtime/stdlib/column.kara, dataframe.kara, arena.kara, interner.kara, once.kara; surface traits runtime/stdlib/reduce.kara, elementwise_map.kara, elementwise_ord.kara; scalar float intrinsics in src/float_math.rs

## Specifies

- Core enums: Option, Result, Ordering (split into comparison + memory-ordering variants), Entry (mut-ref payload), IoError, VarError.
- Trait floor: PartialEq/Eq, PartialOrd/Ord, Hash, Display/Debug, the arithmetic/bitwise operator traits, Index/IndexMut, From/Into, TryFrom/TryInto, Iterator/IntoIterator, Not.
- std.* modules: std.http (server + client), std.json, std.cli (subcommands + auto --help/--version), std.process (Command/Child/ProcessTable), std.tracing (Span/LogEvent/Exporter), Pool[T] connection pool, std.runtime introspection, env (env.set with writes(Env), From[VarError] for IoError).
- Primitive-type associated constants; impl Option[Ordering] partial-comparison helpers.

- std.io File handle: open/read/write + FileSystem.read_to_string, with scope-exit close (FreeFileHandle); std.tcp (TcpStream/TcpListener), std.tls (TlsListener/TlsStream), std.ws (WebSocket, RFC 6455) — see [[network-runtime-and-cooperative-scheduling]].
- String methods: push(char) / push_str, bytes() -> Slice[u8], substring(start), starts_with(prefix), String.from_utf8 (Utf8Error); i64.parse(s) -> Option[i64].
- Structured-concurrency + atomic types: TaskGroup / TaskHandle[T] / spawn declarations, Atomic[T].

- Backpressure & synchronization: `Semaphore` (new/acquire/release), a token-bucket `RateLimiter` (per-key try_acquire), `BoundedChannel[T]` (capacity-bounded send/recv), and `Mutex` — the explicit backpressure surface for [[network-runtime-and-cooperative-scheduling]] servers.
- std.http client: chained `RequestBuilder`, `Client.get`/`Client.post`, `Response.text()`/`.bytes()`/`.header(name)`/`.headers()`; HTTP/2 via hyper `auto::Builder` (ALPN h2 + h2c). std.process `Command` adds stdin/stdout/stderr redirection (Stdio.Inherit/Null). std.tcp `TcpStream.connect` and std.tls `TlsStream.connect` add plain-TCP and TLS client sockets.
- std.tracing: active-span propagation (with_span + auto-stamp + `par` inherit), ambient `Log.*` emission, and a StdoutExporter with full codegen.


- Buffered I/O: `BufReader[R]` (fill_buf/consume, `.lines()` -> LinesIter) and `BufWriter[W]` (write / write_all, cancel-safety annotation).
- `Pool[T]` connection pool: auto-release `PooledConnection` on Drop, plus `Pool.with_health_check` (opt-in validation hook + evict-on-error).
- std.process: `Stdio.Piped` + `Child` stdout/stderr/stdin capture handles; `blocks` execution verb on synchronous `Child.wait()`.
- Numeric surface: `wrapping_add/sub/mul` on 64-bit ints, saturating/other numeric conversion method families, `char.try_from(n)` -> `Result[char, i64]`, integer `parse`/`from_str_radix` typed as `Option[<int>]`, `f64.parse` -> `Option[f64]`, u8 ASCII predicates (`is_ascii_digit`/`_alphabetic`/`_hexdigit`).
- String: two-arg `substring(start, end)` byte-range slice and `s[a..b]` slicing; NUL-safe/interior-NUL handling; collection `Display` in f-string / to_string.
- New long-tail surfaces: the [[tensor-and-numerical-stdlib]], [[portable-simd]], and the [[fallible-allocation-profile]] `try_*` companions; gated std.wasi / std.web modules for the [[wasm-target-backend]]; `CStr` borrowed surface (from_ptr / to_string -> Result[String, Utf8Error]) — the first safe pointer-producer.


- **SortedMap[K: Ord, V]**: an ordered key→value map (the sibling of SortedSet), interpreter-complete and (this round) codegen-complete for integer/String keys — see the codegen-completions note below. Core map surface plus ordered queries min/max/floor/ceiling/range (inclusive), ascending-key iteration. runtime/stdlib/sorted_map.kara.
- **protobuf** (runtime/stdlib/protobuf.kara): proto3 wire format (varint/length-delimited), `#[derive(Message)]` comptime codegen, and a `.proto` schema → message-type generator — a schema-to-types pipeline built on the comptime substrate.
- Expanded String surface: `trim` / `to_lowercase` / `to_uppercase` (full Unicode, matching Rust's str, all three backends) / `replace(from,to)`; `char_at(i) -> Option[char]` and `char_count()` (O(n) Unicode-scalar); `ends_with(suffix)`; `s.chars()` (+ `.collect()` to Vec[char], bindable to a variable) with ASCII fast-paths for push/decode; `from_utf8` wired through codegen.
- Expanded char surface: `is_uppercase`/`is_lowercase`, Unicode classifiers, `char.to_digit(radix) -> Option[u32]`.
- Expanded integer/scalar surface: `pow`, bit intrinsics (`count_ones`/`leading_zeros`/`trailing_zeros` -> u32), `{checked,saturating,overflowing}_{add,sub,mul}` (width-correct, all backends), and `Vec.binary_search`/`Slice.binary_search -> Option[i64]` (int + String).
- Browser/host event surface: std.web.events.* channel producers (keydown/keyup, clicks, dblclick, contextmenu, wheel, pointer_moves + PointerEvent.buttons, touchstart/touchmove/touchend, focus/blur, resize) and std.web.time.every (recurring setInterval) — see [[wasm-target-backend]].


- **Column[T]**: a nullable, Arrow-backed columnar type — Arrow-buffer layout, `fillna` (with a `treat_nan_as_null` flag) / `dropna` / `from_iter_nullable`, `iter` / `iter_valid`, and SQL three-valued-logic (3VL) arithmetic/comparison; interpreter + native backend. runtime/stdlib/column.kara.
- **Arena[T] / ArenaRef[T]**: a bulk-allocation primitive (arena-allocate many values, hand back lightweight `ArenaRef` handles). runtime/stdlib/arena.kara.
- **Symbol + Interner**: a dedup string-handle primitive (intern a string once, compare/store cheap `Symbol` handles). runtime/stdlib/interner.kara.
- **OnceLock[T] / OnceCell[T]**: write-once lazy-init cells; `OnceCell` carries single-task structural enforcement. runtime/stdlib/once.kara.
- **Scalar float math**: transcendental (exp/log/trig-family) + rounding methods on `f32`/`f64` across typecheck / interpreter / codegen (src/float_math.rs).
- Expanded **protobuf** (runtime/stdlib/protobuf.kara): proto3 field coverage — nested-message, repeated, enum, map, and oneof fields; `float`/`double` (incl. in repeated / map-value / oneof positions); `sint`/`fixed`/`sfixed` via field-attribute reflection; and per-field number overrides for sparse/non-contiguous schemas.

- **DataFrame**: a multi-column tabular type over Column — `new` / `insert` / `column` / accessors / `column_names` / `select(cols)` / `describe()`, value-copy column semantics, and DataFrame–String integration; interpreter MVP plus Arrow-buffer codegen. runtime/stdlib/dataframe.kara, src/codegen/dataframe.rs.
- **Reduce / ElementwiseMap / ElementwiseOrd surface traits** (runtime/stdlib/reduce.kara, elementwise_map.kara, elementwise_ord.kara): stdlib traits that Column and [[tensor-and-numerical-stdlib]] Tensor implement, exposing `fold` / `map` / `zip_with` / `sorted` / `argsort` / `argmin` / `argmax` / `prod` / `sum` / `min` / `max` / `range` for bound-generic dispatch; `Reduce.range` is a default method (`max − min`) inherited by implementors and by generic user impls over the builtin containers.
- **Stats.*** free-function statistics (runtime/stdlib/stats.kara): sum / mean / prod / min / max / median / quantile / percentile / argmin / argmax / sort / argsort over `ref Slice[f64]` and i64 slices (element-typed, checked folds), sharing the reduce-kernel emitters with Column/Tensor; a `DataFrame.describe()` companion.


- Codegen completions this round: **SortedSet[T] and SortedMap[K,V] now lower under `karac build`** (KaracMap-backed storage, ascending order materialized only at iteration/min-max/keys/values/entries via a codegen-emitted comparator) — no longer interpreter-only. `Map.try_insert` / `Set.try_insert` fallible-allocation codegen (a `karac_map_try_insert` runtime path over a null-checked resize) completing the [[fallible-allocation-profile]] `try_*` split; `Vec.sorted()` / `String.sorted()` and general-element `Vec.sort()` (recursive `karac_cmp_<T>` comparators for structs/enums/tuples/nested-Vec). `String.cmp` and derived-Ord struct/enum comparison operators lower.
- New stdlib functions: **std.cmp** `min`/`max`/`clamp`; **std.mem** `swap`/`replace`/`take` + a generic `T.default()` (`#[derive(Default)]` synthesizes a concrete inherent `default()` monomorphized through the bound). protobuf `#[derive(Message)]` bodies compile + round-trip under codegen/JIT (comptime derive-under-codegen: reorder fold→re-resolve→re-typecheck, span reanchor, skip comptime-fn bodies, compile the pure-Kāra ProtoBuf namespace).


- New stdlib surfaces this round: **[[secret-handling-and-constant-time-comparison]]** (Secret[T] / std.secret) and the **[[embedded-and-mmio-surface]]** (volatile MMIO intrinsics, VolatileCell, critical_section, fences, Atomic ordering).
- **Phase-8 long-tail scalar methods** (typecheck + interpreter + codegen): float `hypot` / `copysign` / `fract` / `recip` / `to_degrees` / `to_radians` / `signum` / inverse-trig / hyperbolics / `exp_m1` / `ln_1p` / `exp2` / `log10` / `trunc`; integer `abs_diff` (→ unsigned sibling), `div_euclid` / `rem_euclid`, `rotate_left` / `rotate_right`, `count_zeros` / `reverse_bits` / `swap_bytes`, unsigned `is_power_of_two` / `next_power_of_two`; scalar `min` / `max` / `clamp` method forms; `ref_eq(a, b)` reference-identity comparison for shared types.
- **String / char long-tail**: `split_whitespace()` → Vec[String], `lines()` → Vec[String], `trim_start()` / `trim_end()`, `strip_prefix` / `strip_suffix` → Option[String]; char `to_ascii_uppercase` / `to_ascii_lowercase` / `is_ascii`; `chars().count()` / `.len()` ergonomics.
- **Conversions**: From[char] for String (String.from(c) / c.into()), From[T] for Option[T] / Result[T,E] via `.into()`, built-in numeric-narrowing TryFrom (`iN.try_from` / `.try_into`).
- **f-string format specifiers**: `f"{expr:spec}"` — width, zero-pad, align, radix, precision.
- **Collections**: Vec[T] `.insert(idx, value)`, `.clear()`, `.extend(other)`.
- **Option/Result** `.map(f)` implemented across typecheck / interpreter / codegen.
- **File I/O**: `fs.read_lines(path)` → Result[Vec[String], IoError] and `stdin.lines()` streaming line iterator, with run/build parity.
- **OnceLock/OnceCell** `set`/`get`/`get_or_init` now lower under `karac build` for heap-owning and wide element types (leak- and double-free-clean), not just scalars.

## Constraints

- Baked declarations are the source of truth for Option/Result/Vec (CR-202 swap); dispatch stays programmatic.
- `#[compiler_builtin]` restricted to these stdlib files.

## Rationale

Phase 8 = stdlib floor; Phase 11 = long-tail. Structure follows [[bake-standard-library-in-kara-source]].
