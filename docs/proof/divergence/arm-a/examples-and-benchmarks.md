---
type: reference
title: Examples and benchmarks
updated_round: 10
---

# Examples and benchmarks

## Parallax — the flagship concurrency demo + benchmark

**Parallax** (`examples/parallax/`) is the canonical concurrency workload (typed-resource
providers). It has a benchmark harness comparing **Kāra vs Rust vs Go vs Node** (Slice E),
with an apples-to-apples bench kernel, connection sweep, multi-run stats, percentile
distribution, and cold-start baselines. **Round 4** wired a **fifth comparator**,
**Phoenix / Elixir** (a `Task.async` fan-out server, `examples/parallax/bench/phoenix/`),
into `bench.sh`, and folded a Bandit-tuning entry into the Parallax close-out. **Round 5**
added a **Java/Netty** comparator, planned **EC2 throughput runs** (x86 + Graviton + Mac,
`docs/investigations/parallax_ec2_bench_plan.md`), and **trimmed the cohort to K/R/J/G**
(Kāra / Rust / Java / Go) with the headline **flipped to Graviton-led** (see
[[history-reversals-and-deprecations]]). The v7 five-impl close-out landed regression-clean.

- **`parallax_lite`** (`examples/parallax_lite/`) — a smaller microbenchmark workload;
  ground-truth multicore scaling measured (N=3..500, 18 cores).
- **Perf investigations** (`docs/investigations/`):
  - **parallax_perf** — H1: an earlier **thread-per-call fan-out** was the bottleneck,
    fixed with a **long-lived worker pool**; then codegen **IR-opts (`default<O2>`)** were
    found to be the remaining bottleneck (probe sweep ruled out the runtime).
  - **http_layer_perf** — H1 `KARAC_HTTP_BLOCK_IN_PLACE` A/B probe; H2 killed intermediate
    `String` allocs in the trampoline.
  - **bench_robustness** — bench-harness robustness (multi-run stats, percentiles).

## Mend — "AI writes Kāra"

**Mend** (`examples/mend/`) is an end-to-end demo where an AI fixes buggy Python by
writing Kāra solutions, with a harness (`mend.py`), a system prompt, canned responses,
and asciinema casts (order_status, user_lookup, welcome_emails tasks). Ties to the
[[design-ai-first-compiler|AI-first]] positioning. **Round 5** added a **`concurrent_emails`**
pair (buggy Go + Python references, canned responses, a live-run finding) exercising the
**effect / par-conflict** axis.

**Round 8/9 turned Mend into a scored, batch-runnable corpus** — the loop that "develops Kāra
through the loop": a codified **task + oracle format** (`examples/mend/TASK_FORMAT.md`), a
`mend_batch.py` that runs + scores the whole corpus in one command, and a `mend_score.py` that
aggregates run transcripts into a **measured score**. New corpus tasks (`grade_histogram`,
`render_status`, `two_source_totals`) each ship a `task.md` + `solution.kara` +
`canned_responses.json`. Several round-9 stdlib/typechecker gaps (e.g. the widening-coercion
rule `B-2026-07-09-7`) were surfaced authoring these. Round 9 also wired a **flagship-demo
benchmark regression gate** into CI (Phase-6 Slice 7).

## New round-5 examples

- **Tangle** (`examples/tangle/`) — a **dogfooding project** modelling graph shapes in Kāra:
  a **parent-pointer tree** (`parent_tree.kara`) and a **cross-edge graph** (`cross_graph.kara`)
  that surfaces the [[rc-elision|RC fallback]] **with its trigger line** reported. It drove
  the `karac query ownership/effects/concurrency` fix that lets those queries **target impl
  methods** (see [[cli]]).
- **`ssr_counter`** (`examples/ssr_counter/`) — a **dual-target SSR** example: the same Kāra
  renders on the server and in the browser (wasm) via a **provider-injection** pattern
  (`run_browser.mjs`, `index.html`). Documented in the book (`ch17-ssr.md`). See
  [[wasm-targets]].
- **`std_net`** (`examples/std_net/`) — minimal `http_hello` / `https_hello` / `ws_echo`
  surfacing the v1 stdlib net stack in the README.
- **`wasm_hello`** (`examples/wasm_hello/`) — a WASI hello-world (`run_wasi.mjs`). See
  [[wasm-targets]].

## Dogfooding roster (`dogfooding.md`)

`docs/demo_ideas.md` was **renamed to `dogfooding.md`** and reframed as the **V1 dogfooding
roster** (name-keyed entries + a roster table). It tracks a **front-end browser demo track**
— **Plume**, **Fathom**, and **Slipstream** (wasm edition) — plus **Weave** and **Mend**.
The **data-engineering pipeline demo was demoted to post-launch** (out of the v1 checklist).
See [[history-reversals-and-deprecations]].

## Round-10 dogfoods (new examples)

New `examples/` dogfoods surfacing round-10 fixes — the **"katas are bug-finders"** pattern
continued, with dogfooding examples remaining the dominant bug source:

- **`examples/heap.kara`** — a generic binary min-heap `Heap[T: Ord]` + heapsort. Surfaced the
  generic-container codegen fixes.
- **`examples/json.kara`** — a recursive JSON parser/serializer over a `shared enum`. Surfaced
  `?`-on-`Result`-of-enum, ref-enum payload binding, and Vec-of-struct enum-payload fixes.
- **`examples/pipeline.kara`** — a functional log-analytics pipeline. Surfaced the iterator
  `fold` / for-over-chain terminals.
- **`examples/vm.kara`** — a stack-based bytecode VM. Surfaced the `with_capacity(0)` leak.
- **`examples/semantic_search.kara`** — a `std.embeddings` end-to-end demo.
- **Slipstream** got a **SIMD collide kernel** (`Vector[f64, 2]`, now Copy) and its **GPU LBM path
  completed** (see [[gpu-compute]]).

## Round-6 dogfoods (shipped)

Round 6 **shipped** the dogfooding roster's front-end and systems demos — each a real
program that surfaced (and drove fixes for) [[codegen]] / [[wasm-targets|wasm]] /
[[design-concurrency-and-providers|concurrency]] bugs, most of them logged in the
[[bug-tracker|bug ledger]]. The roster README dropped its progress-bar header and
**grounds status in the trackers** (`91a8b291`).

- **Fathom** (`examples/fathom/`) — a **browser multi-core Mandelbrot explorer** with a
  **SIMD-128 inner kernel** (`Vector[f64, 2]`, two pixels/lane-pair) and full interaction:
  wheel zoom-to-cursor, pointer pan, keyboard controls, single-finger touch-pan,
  dblclick/contextmenu zoom, a focus/blur render gate, and a resize-responsive canvas. It
  surfaced the round's hardest [[wasm-targets|wasm-threads]] bugs — the browser
  spawn-deadlock (`B-2026-06-14-17`), the SharedArrayBuffer `fd_write`/`readString` decode
  bug (`B-2026-06-14-22`), the for-over-collection body leak (`B-2026-06-14-21`), the
  non-scalar `TaskHandle.join` (`B-2026-06-14-14`), and the wasm numeric-f-string trap
  (`B-2026-06-14-15`).
- **Plume** (`examples/plume/`) — a **pointer-steered browser flow field** (added
  `f64.sqrt`), with a clicks-pinned vortex.
- **Iris** (`examples/iris/`) — a **browser image-filter studio** with a shared filter
  kernel, a native checksum oracle, and a **native/wasm A/B verify harness**.
- **Cartographer** (`examples/cartographer/`) — a **live WASM studio rendering the
  compiler's own effect graph in the browser**, built on the new whole-program
  effect/concurrency graph (`00a4f2f5`, `src/effect_graph.rs`). It surfaced the
  generic-receiver query key-join fix (`B-2026-06-14-3`). See [[design-ai-first-compiler]].
- **Relay** (`examples/relay/`) — a **TCP reverse proxy** built in slices: single-upstream
  passthrough → round-robin load balancer → **full-duplex bidirectional splice**
  (`TcpStream.try_clone` + `shutdown_write` half-close) → **Layer-7 path routing** → **live
  metrics via `par struct` `Atomic` counters**. It has a **wrk-based 3-language reverse-proxy
  benchmark** (Go / Node / Kāra, `examples/relay/bench/`) with HTTP/1.1 keep-alive, pooled Go
  upstream conns, and cross-host results. Surfaced the loop-spawn heap-capture double-free
  (`B-2026-06-18-8`) and several `Vec.from_slice` / `String.from_utf8` codegen gaps. See
  [[networking]].
- **Slipstream** (`examples/slipstream/`) — a **lattice-Boltzmann (LBM) wind tunnel**:
  single-threaded → fan collide+stream across the worker pool → live angle-of-attack → stall.
  It is the **[[per-layout-monomorphization|full-SoA proof]]**: its carried grid is an SoA
  `layout` block split into cache groups, and the native oracle's framebuffer checksums are
  byte-identical AoS↔SoA.
- **Weave** (`examples/weave/`) — a **CSV ETL** dogfood exercising **refinement types +
  contracts + effects** together (see [[design-contracts-and-verification]]).
- **Tangle** (`examples/tangle/`) — round 5's graph-shapes dogfood grew a **tree-walking
  interpreter** (shared scope), a **doubly-linked list** (`Rc<RefCell>` + `Weak` shape), and
  **undo/redo over shared mutable state**.
- **`protobuf_schema.kara`** — exercises the `.proto` → message-type path. See [[protobuf]].

## Other examples

- **Multi-file projects**: `db_pipeline`, `elevator_project`, `game_of_life` (each with
  `kara.toml` and tests) — these drove the design-study gap analyses (v53–v56
  brainstorms).
- **Single-file**: `elevator.kara`, `word_count.kara`, `array_basics.kara`,
  `slice_basics.kara`.
- **LeetCode** (`examples/leetcode/`): two_sum, coin_change, course_schedule,
  group_anagrams, lru_cache, valid_palindrome, valid_parentheses, merge_sorted_lists,
  max_depth_binary_tree (several with Python reference versions).
- **`phase0`** — a parallel-vs-sequential dashboard baseline.
- **`design_studies`** (`brainstorming/design_studies/`) — cross-language studies
  (db_read, event_stream, http_api_call, json_read, money_type, parallel_fanout) in
  Kāra/Rust/Python/Java with findings.

## Round-7 benchmark — `wasm_size` (module-size comparison)

**`bench/wasm_size`** (round 7, `eaa96e32`) compares **compiled WASM module size** across
**Kāra vs Rust vs TinyGo** for two workloads — a `hello` and a `filter_core` — with each
language's source under `src/{kara,rust,tinygo}/`, a `bench.sh` runner, and `sizes.json` /
`sizes.md` receipts. It was building this size receipt that surfaced the **non-reproducible
wasm component bytes** bug (`B-2026-06-22-3`): the brotli figure jittered ~0.2% run-to-run
because a pid-stamped scratch filename leaked into the module `name` section. See
[[wasm-targets]], [[bug-tracker]].

## Round-2 benchmarks

- **Auto-par reductions** — `kata-7` measured at **9.87× vs Rust** for the while-shape
  reduction lowering; the earlier narrow-shape v1 measured a **4.1× wall-clock speedup**.
  See [[codegen]], [[design-concurrency-and-providers]].
- **`bench/hash_quality/`** — the bench that picked **FxHash** over FNV-1a for the
  `karac_hash_<T>` swap.
- **`bench/hot_swap_cost/`** — measures the cost of `--enable-hot-swap` codegen indirection
  (`tight_call.kara`, `moderate_call.kara`); write-up in
  `docs/investigations/hot_swap_indirection_cost.md`. See [[codegen]],
  [[design-runtime-phases]].

## Demo 1 — `ws_idle_holder` (new in round 3)

**`ws_idle_holder`** (`examples/ws_idle_holder/`) is "Demo 1" — a WebSocket idle-connection
holder showcasing the [[networking|network I/O stack]] (Phase 6 line 170 / line 236). It has
its own README, a **`wss://` (TLS) variant** using the generated `tests/fixtures/tls/`
cert/key, and a **Rust bench harness** (`bench/`, phase 6 line 180) for apples-to-apples
comparison.

**Round-4 scaling campaign.** Demo 1 became the project's headline idle-connection-density
benchmark, driven to large scale on EC2:

- A **Rust reference implementation** (`examples/ws_idle_holder/rust/`) is the credibility
  comparator; `run_2m.sh` treats **Rust as the 2M comparator**.
- Verified at **50K → 1M → 2M** connections (M1/M3 rigs, macOS and **x86_64-Linux**, with a
  post-fix data-layout re-read confirming cross-ISA parity). `ec2_setup.sh` / `run_1m.sh` /
  `run_2m.sh` scripts, a `file-max` patch, listen-backlog clamp, and a `--churn-batch-cap`.
- A **cost model** (250K production cost model + 2M scale-invariance) and a re-measured
  **idle-holder density of ~12.1 KB/conn (2.30× Rust)** for the working handler.
- Bench realism: **`--stagger-arrival`** (realistic active-traffic arrival) and
  **`--source-ips`** (beat the loopback ephemeral-port cap); a cross-comparator **`REPORT.md`**.

### Round-5 comparator cohort (the density benchmark headline)

Round 5 turned `ws_idle_holder` into the project's **headline benchmark** and built a full
**Phase 3 comparator cohort** — one real server per stack, each holding idle WS-over-TLS
connections at commercial scale (`run_250k.sh` / `run_50k.sh` runners, a Node heap-cap
sidebar, and reap harnesses). Kāra's **per-connection memory density** is the lead metric,
underwritten by [[rc-elision|RC elision]]:

| Stack | KB / conn | × Kāra | Notes |
|-------|-----------|--------|-------|
| **Kāra** | ~12.1 (idle-holder handler; Rust=1.0 baseline) | 1.0 | density lead |
| Java / **Netty** (#68) | 14.4 | 1.19× | second-densest stack |
| Go (gorilla) (#69) | 44.4 | 3.66× | |
| **.NET** / ASP.NET Core (Linux) | 52.9 | — | "the JVM's mirror image" |
| **Phoenix** Channels + Presence (#67) | 102.8 | 8.69× | heaviest comparator |
| Node.js (**ws**) (#73) | — | — | 250K + 50K + heap-cap sidebar |

The **p50 Nagle fix** (`TCP_NODELAY` on all WS/TLS paths) took p50 from **45.01 → 1.63 ms at
250K** — now field-leading (see [[design-runtime-phases]], [[networking]]). A
**handshake-QPS reconnect-storm** rig (loopback + cross-box) and **active-traffic** runs
round out the evidence (`docs/investigations/`). The **commercial comparator ladder** landed
in the README; stretch-tier comparators (#74–#76) and a .NET-Windows comparator (#72) were
**cut by decision** (Phase 3 wrapped).

## Kata-driven work (round 3)

Much of round 3 was surfaced by "kata" exercises:

- **kata #204** — a bench rollup reframed as **two fair lanes** (single-thread vs
  parallel), with **Kāra single-thread**, **rayon**, and **Go** rows and nuanced findings.
- **Roman-numeral kata** — `docs/investigations/roman_kata_codegen.md`: a codegen
  investigation that filed several deferred follow-ups ([[deferred-work]]).
- **todo-api kata** — surfaced the collect-style / conditional reduction and several codegen
  gaps ([[codegen]]).
- **kata-8 `atoi`** — an end-to-end ASAN regression guard ([[bug-tracker]]).
- **kata-91** — the pilot that shipped the phase-4 `test { }`-block slice 7.
- **v69 Go-parity gaps** (`brainstorming/archive/v69_go_parity_gaps.md`) graduated to the
  roadmap with bench scaffolding ([[history-reversals-and-deprecations]]).

Related: [[design-concurrency-and-providers]], [[codegen]], [[networking]],
[[history-reversals-and-deprecations]], [[per-layout-monomorphization]], [[protobuf]],
[[wasm-targets]], [[bug-tracker]], [[design-ai-first-compiler]].
