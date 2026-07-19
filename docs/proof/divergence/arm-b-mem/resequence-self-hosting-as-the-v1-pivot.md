---
type: decision
created_date: 2026-07-15T10:51:03Z
last_modified: 2026-07-15T10:51:03Z
status: accepted
decided_on: 2026-06-05
deciders: kara-maintainers
scope: system
tags: roadmap, self-hosting, strategy, phase-12
---

# Resequence Self-Hosting as the v1 Pivot

## Decision
We chose to pull self-hosting forward as Kāra's v1 pivot, resequencing the roadmap from 8 → 9 → 10 → 11 to 8 → 9 → 10 → 12 → 11 — i.e. Phase 12 (rewriting the Kāra compiler in Kāra, starting with the lexer) is now scheduled ahead of Phase 11 (the stdlib long-tail). The compiler compiling itself becomes the primary v1 credibility proof, gated first by an LLVM-C FFI spike proving Kāra can drive LLVM directly.

## Context
By this point the native backend, effect/ownership checkers, and stdlib floor were mature enough that the language could plausibly express its own compiler. Self-hosting is the strongest possible dogfooding signal: it forces the codegen and stdlib surface to be complete and correct on a large, real program (the lexer alone is ~1,800 lines of Kāra), and it surfaces codegen bugs that synthetic katas never reach. Deferring the stdlib long-tail (Phase 11) until after the self-host lexer keeps the effort pointed at the levers self-hosting actually needs.

## Consequences
- Phase 12 (self-hosting) is scheduled before Phase 11 (stdlib long-tail); Phase 11 items are pulled in on demand rather than up front.
- Every codegen bug the lexer port hits is filed as a numbered self-hosting blocker (#1, #2, …) and fixed against a differential oracle that compares the Kāra lexer to the Rust lexer token-for-token.
- Self-hosting was gated on an LLVM-C FFI proof spike (call LLVM from Kāra, minimal exit=42 proof) before the lexer port began.
- Real-world codegen levers (e.g. string-literal match dispatch) get prioritized because the self-hosted lexer profile exposes them as the #1 cost.
- Realized incrementally in [[self-hosting-the-kara-compiler]].

## Relationships
- **MOTIVATED_BY**: [[backend-first-v1-positioning]]
- **REFERENCES**: [[self-hosting-the-kara-compiler]]

## Options

- Keep 8 → 9 → 10 → 11 and self-host after the full stdlib — rejected: delays the strongest correctness proof and leaves codegen gaps latent longer.
- Skip self-hosting for v1 — rejected: it is the clearest evidence the language is production-real, not a toy.
- Self-host, lexer first, ahead of the stdlib long-tail (12 before 11) — chosen.

## Notes

The lexer is the first slice; later compiler stages follow. Three flows carry Rust-side fixes into the Kāra port. Tracked in docs/implementation_checklist/phase-12-self-hosting.md.
