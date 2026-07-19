---
type: memo
created_date: 2026-07-15T07:25:16Z
last_modified: 2026-07-15T10:55:42Z
status: closed
tags: observation, lesson, performance, benchmark
---

# Parallax Performance Investigation

## Claim
Kāra's large deficit against Rust/Go/Node on the Parallax HTTP benchmark had two distinct root causes — a thread-per-call fan-out in the parallel runtime and unoptimized emitted LLVM IR — both now fixed.

## Context
- Parallax is the flagship multi-language HTTP benchmark that exercises effect-driven [[auto-parallelization]] under load (wrk); the cohort was later trimmed to Kāra/Rust/Java(Netty)/Go and the EC2 throughput runs flipped Graviton-led (x86 + Graviton + Mac).
- Initial numbers showed Kāra far behind; an investigation was opened (docs/investigations/parallax_perf.md).
- The work also produced parallax_lite, a multicore-scaling microbenchmark (N=3..500 on 18 cores).

## Relationships
- **REFERENCES**: [[auto-parallelization]]
- **REFERENCES**: [[run-llvm-o2-mid-end-passes-on-emitted-ir]]
- **REFERENCES**: [[http-layer-performance-investigation]]

## Substance

- H1 (runtime): `karac_par_run` spawned a fresh thread per call, so per-request fan-out paid thread-creation cost every time. Confirmed under wrk profiling; fixed by a long-lived worker pool.
- H2 (codegen): after the runtime fix, a probe sweep ruled out the runtime and interpreter and pinned the remaining gap on emitted IR that was never optimized. Fixed by running LLVM `default<O2>` mid-end passes — see [[run-llvm-o2-mid-end-passes-on-emitted-ir]].
- Secondary HTTP-trampoline allocation cuts (kill intermediate String allocs) came from the paired [[http-layer-performance-investigation]].

## Alternatives



## Outcome

- Both root causes fixed; post-fix Parallax numbers back-filled into the benchmark table.
- Established the discipline: probe/measure to localize the bottleneck before optimizing.
- Residual Kāra-vs-Rust gap-closure path documented as conditional future work.
