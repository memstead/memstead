# memstead-projection

Install expertise into your agent. `memstead-projection` is the maintenance
loop of [Memstead](https://github.com/memstead/memstead): the deterministic
half of keeping a bound mem current with its source — run briefs, change
detection, findings and verify reports, the advance gate, prune proposals,
and the one health assembly every surface composes through.

`memstead-projection` is a dependency of the two Memstead products, the
[`memstead-cli`](../memstead-cli/) binary and the
[`memstead-mcp`](../memstead-mcp/) server, and not a supported library.
It is on crates.io because cargo publishes a crate only when every path
dependency is on the registry at the same version; nothing else about it
is a product.

> **Stability:** none promised. The Rust API is pre-1.0 and changes
> without deprecation cycles whenever the products need it. For a stable
> contract, consume the binaries or the MCP surface instead.

The crate sits above the kernel, [`memstead-base`](../memstead-base/), and
below the binaries: it reads the kernel's binding declarations, anchors and
source-scope enumeration, and never depends on a storage backend. It left
the kernel crate on 2026-09-10 so that a change to the loop no longer
rebuilds the kernel and the boundary between the two is the compiler's.

## Use

Install the [`memstead-cli`](../memstead-cli/) binary or the
[`memstead-mcp`](../memstead-mcp/) server. A direct dependency on this
crate is unsupported: it may build today and break at the next release.

## License

MIT OR Apache-2.0, at your option.
