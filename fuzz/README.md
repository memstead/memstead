# Fuzzing the trust boundaries — the long tier

Coverage-guided fuzzing (cargo-fuzz/libFuzzer) over the three
trust-boundary parsers. This crate is workspace-excluded: its
dependencies and the nightly toolchain it runs under never enter the
PR-blocking path. The bounded CI smoke tier lives inside the normal
suite as seeded adversarial tests:

| Target | Parser | Smoke-tier twin |
|--------|--------|-----------------|
| `frontmatter` | frontmatter/markdown family (public entry points) | `crates/memstead-base/src/entity/adversarial.rs` |
| `archive` | archive validator, nested parsers transitively | `crates/memstead-base/src/validator/adversarial.rs` |
| `content_expr` | content-expression parse + match | `#[cfg(test)]` module in `crates/memstead-schema/src/content_expr.rs` |

Both tiers share the committed seed corpus under `corpus/<target>/`
(`seed-*` files must stay valid — the smoke tier replays and asserts
them; `harvest-*.mem` are real tracked archives joining as-is). A
crash or invariant break found here is fixed at the parser — never by
widening acceptance — and pinned as a fixture regression test in the
normal suite before the finding is closed; the fix is re-fuzzed.

## Running

```sh
cargo install cargo-fuzz          # once; needs a nightly toolchain
cd fuzz
cargo +nightly fuzz run archive corpus/archive -- -max_total_time=900
cargo +nightly fuzz run frontmatter corpus/frontmatter -- -max_total_time=900
cargo +nightly fuzz run content_expr corpus/content_expr -- -max_total_time=900
```

Or dispatch the `Fuzz (long tier)` GitHub workflow, which runs the same
commands and uploads any crash artifacts. It is never a required check.

Formatting and lints: this crate is covered by rustfmt and clippy, but
the gate runs inside the fuzz workflow, not `run-tests.sh` (the one-gate
script stays offline-clean and never builds this crate).
