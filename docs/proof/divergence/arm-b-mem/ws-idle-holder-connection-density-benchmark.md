---
type: memo
created_date: 2026-07-15T10:19:27Z
last_modified: 2026-07-15T10:55:30Z
status: active
tags: observation, benchmark, performance, websocket, phase-6
---

# ws-idle-holder Connection-Density Benchmark

## Claim
The ws_idle_holder benchmark measures how many idle WebSocket connections a single Kāra event-loop server holds and at what per-connection memory density, validating the Phase-6 event-loop runtime against a full commercial comparator ladder (Rust, Java/Netty, Go, .NET/ASP.NET Core, Node.js ws, Phoenix Channels) up to 2M connections.

## Context
- Demo 1: the flagship idle-connection-density demo for the [[network-runtime-and-cooperative-scheduling]] event loop, distinct from the Parallax throughput benchmark.
- Exercises the cooperative scheduler holding many parked connections rather than request throughput; the event loop is what makes high connection counts affordable (see [[phased-runtime-model]]).
- Run on EC2 rigs (M1 local baseline, M3, and x86_64-Linux cross-ISA) with canonical run_1m.sh / run_2m.sh scripts and an ec2_setup.sh file-max patch.
- Comparators: a Rust reference impl (examples/ws_idle_holder/rust/) and a Phoenix/Elixir server.

## Relationships
- **REFERENCES**: [[network-runtime-and-cooperative-scheduling]]
- **REFERENCES**: [[phased-runtime-model]]

## Substance

- Per-connection density measured at 12.1 KB/conn (2.30× a Rust baseline) on a 1M working-handler re-measure; earlier figures were flagged provisional pending the handler-execution (line-17) fix.
- Scale-invariance validated through 2M connections; a 250K-connection production cost model landed; Rust is treated as the 2M credibility comparator, and the head-to-head ceiling flipped after the Rust 2M EC2 comparator landed.
- Harness: active-traffic + handshake-QPS mode, `--stagger-arrival` for realistic arrival, restored handshake-pool stats; the handler-execution blocker (line-17) was fixed to unblock density re-measure.
- Cross-ISA x86_64 1M confirmation landed alongside the arm64 numbers.

- Commercial comparator ladder landed, per-connection density head-to-head: Kāra 12.1 KB/conn; Java/Netty 14.4 KB/conn (1.19×, second-densest); Go (gorilla/websocket) 44.4 KB/conn (3.66×); .NET/ASP.NET Core (Linux) 52.9 KB/conn; Node.js (ws) added at 250K+50K with a heap-cap sidebar; Phoenix Channels+Presence 102.8 KB/conn (8.69×, heaviest). Commercial-scale runners run_250k.sh / run_50k.sh.
- p50 latency: a TCP_NODELAY/Nagle fix on all WS/TLS handshake + accepted-socket paths cut p50 from 45.01 → 1.63 ms at 250K, making Kāra field-leading on tail latency.

## Alternatives



## Outcome

- Post-fix density and 2M numbers back-filled into examples/ws_idle_holder/bench/REPORT.md, split per comparator.
- Establishes idle-connection density (not just throughput) as a first-class credibility axis for the event-loop runtime.
