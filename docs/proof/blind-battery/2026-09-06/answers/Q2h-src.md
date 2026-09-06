A published mem installs as a read-only mount in your workspace. The engine enforces strict boundaries.

The mem can be read but not written to. Attempts to update, create, or delete entities within a published mem fail with a `ReadOnlyMount` error. (`memstead_base/src/engine/mutation/update.rs:378`; `memstead_base/src/engine/mutation/delete.rs:125`) The data is visible to your agent, but immutable.

The mem cannot directly modify your own data. Cross-mem relationships are blocked by default: `memstead_relate` rejects cross-mem edges unless your workspace configuration explicitly grants them via `[cross_mem_links]` policies. (`memstead_mcp/src/server.rs:2714`; `workspace_config_commands.rs` tests) The default configuration is empty, denying all cross-mem pairings. So a published mem cannot create relationships that reach into your writable mems.

Its content cannot act as instructions to your agent. When an agent requests schema information about a third-party schema, prose fields like `write_rules`, `when_to_use`, and `writing_guidance` are stripped before sending. These fields reach the agent only for schemas authored in your workspace. (`memstead_mcp/src/server.rs:2712-2715`) Data retrieved from a published mem is marked with an `origin: "third-party"` annotation so your agent knows to treat it as quoted, untrusted data rather than actionable guidance. (`memstead_mcp/src/server.rs:2387-2391`)

The engine contains it through three mechanisms. First, archive-based storage: installed mems are stored as compressed archives in a global cache, not as writable directories. (`memstead_mcp/tests/read_mem_install.rs:40-80`) Second, mount capability enforcement: the router tracks each mount as `ReadOnly` and blocks mutations. (`read_mem_install.rs:130-137`) Third, schema-origin awareness: the engine knows which schema came from outside and serves only structural information to agents, never prose that could guide behavior. (`server.rs:2712`)

Your agent can read, search, and link from published mems into your own data—they remain visible. But it cannot modify them, write instructions through them, or be directed by their embedded guidance.
