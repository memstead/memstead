---
type: memo
created_date: 2026-07-15T10:56:40Z
last_modified: 2026-07-15T19:08:55Z
status: active
tags: plan, observation, ci, testing, quality
---

# CI Test-Coverage Tiers and the Leak Gate

## Claim
Kāra's CI is organized into test-coverage tiers: Tier 1 (llvm codegen E2E + the self-host lexer oracle) and Tier 2 (a memory-sanitizer job — ASAN + Linux LeakSanitizer) have landed as gates; Tier 3 is open.

## Context
- Codegen correctness and memory safety need machine gates, not manual review, as the codegen surface grows.
- The differential self-host lexer oracle of [[self-hosting-the-kara-compiler]] (Kāra lexer vs Rust lexer) doubles as a Tier-1 CI gate — it caught auto-par bug #8; Tier 1 also exercises the [[llvm-codegen-backend]] E2E.
- Spike: docs/spikes/ci-test-coverage.md.

## Relationships
- **REFERENCES**: [[self-hosting-the-kara-compiler]]
- **REFERENCES**: [[llvm-codegen-backend]]

## Substance

- Tier 1: runs the `--features llvm` codegen E2E and the self-host oracle; LLVM 18 installed via apt (not install-llvm-action) to fix codegen-e2e. A dedicated CI job now also gates the `--features llvm` codegen-backend clippy surface (which the no-llvm clippy job never compiles).
- Tier 2: a memory-sanitizer job (ASAN + Linux LeakSanitizer) is the leak gate; its first full run surfaced 11 leaks, most since fixed against numbered bug entries.
- Tier 3: open.
- Cross-platform reds fixed alongside (Windows fat-stack CLI thread, unix-gated ws-helper tests, separator normalization).


- A local Linux ASAN+LeakSanitizer harness now backs the Tier-2 leak gate off-CI: docker/lsan.Dockerfile + scripts/lsan-local.sh (colima), the AUTHORITATIVE leak gate — macOS ASAN (Apple clang, no LeakSanitizer) misses reachable leaks, so a whole class of ownership leaks was caught here rather than post-landing.
- A machine-countable bug ledger landed: docs/bug-ledger.jsonl (one JSON record per bug: id/date/source/surface/class/severity/status/fix/title) with a generated readable view (docs/bug-ledger.md), plus tooling scripts/bug-curve.py (bug-discovery curve) and scripts/bug-lint.sh; the older docs/bugs.md was retired in favor of the single ledger. scripts/oracle-sync-guard.sh guards the self-host oracle's provenance.


- The memory-sanitizer job gained an **arm64 (aarch64) LeakSanitizer leg**, motivated by an arm64-only index-assign leak whose x86 balancing hid it (B-2026-07-12-29) — the leak class is genuinely architecture-dependent, so the arm64 leg is the authoritative gate for it (and later caught B-2026-07-14-3). This retires the earlier 'leaks are architecture-independent' assumption for the RC/ABI surface.
- A **book-snippets harness** (tests/book_snippets.rs) compiles every Kāra Book code block, and a flagship-demo benchmark regression gate + the ws_idle_holder connection-density bench gate wire performance into CI.
- The bug ledger (docs/bug-ledger.jsonl) now runs through B-2026-07-14, with the dogfood-driven discovery curve dominated by codegen-correctness and ownership/leak classes surfaced by the example programs and self-hosting port.

## Alternatives



## Outcome

- Establishes leak-freedom and codegen-E2E as CI-enforced invariants rather than spot checks — a machine gate that lets human review step back.
