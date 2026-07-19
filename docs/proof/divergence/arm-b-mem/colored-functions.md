---
type: concept
created_date: 2026-07-15T07:24:22Z
last_modified: 2026-07-15T07:34:36Z
maturity: established
abstraction_level: abstract
tags: concurrency, anti-pattern
---

# Colored Functions

## Definition
Function coloring is the property of async/await systems whereby functions are partitioned into two incompatible kinds — synchronous and asynchronous — such that async functions can only be called from async contexts, forcing the color to propagate through every caller.

## Explanation
In languages with async/await, marking a function `async` changes its calling convention: callers must `await` it and themselves become async. The 'color' spreads virally up the call graph, splitting libraries into sync and async variants and forcing manual bridging. Kāra cites this as the central defect it avoids: because concurrency is derived from effect analysis rather than an async keyword, no function carries a color and any function composes with any other.

## Relationships
- **REFERENCES**: [[effect-verb]]
- **REFERENCES**: [[auto-concurrency]]
- **REFERENCES**: [[auto-concurrency-instead-of-async-await]]
- **CONTRASTS_WITH**: [[auto-concurrency]]

## Boundaries

- Not the same as effects: an [[effect-verb]] signature is data the compiler reads, not a calling-convention split.
- Contrast with [[auto-concurrency]], which achieves parallelism with zero coloring.

## Significance

Naming this anti-pattern precisely is what motivates [[auto-concurrency-instead-of-async-await]]; the whole concurrency design is defined by its avoidance.
