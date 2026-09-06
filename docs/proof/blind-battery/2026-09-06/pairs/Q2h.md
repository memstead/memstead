# Question

If I install a mem that someone else published, what can it do to my AI agent, and what can it not do? Can its content act as instructions to my agent, can it change my own data, and what does the engine do to contain it?

# Answer A

A published mem installs as a read-only mount in your workspace. The engine enforces strict boundaries.

The mem can be read but not written to. Attempts to update, create, or delete entities within a published mem fail with a `ReadOnlyMount` error. The data is visible to your agent, but immutable.

The mem cannot directly modify your own data. Cross-mem relationships are blocked by default: `memstead_relate` rejects cross-mem edges unless your workspace configuration explicitly grants them via `[cross_mem_links]` policies. The default configuration is empty, denying all cross-mem pairings. So a published mem cannot create relationships that reach into your writable mems.

Its content cannot act as instructions to your agent. When an agent requests schema information about a third-party schema, prose fields like `write_rules`, `when_to_use`, and `writing_guidance` are stripped before sending. These fields reach the agent only for schemas authored in your workspace. Data retrieved from a published mem is marked with an `origin: "third-party"` annotation so your agent knows to treat it as quoted, untrusted data rather than actionable guidance.

The engine contains it through three mechanisms. First, archive-based storage: installed mems are stored as compressed archives in a global cache, not as writable directories. Second, mount capability enforcement: the router tracks each mount as `ReadOnly` and blocks mutations. Third, schema-origin awareness: the engine knows which schema came from outside and serves only structural information to agents, never prose that could guide behavior.

Your agent can read, search, and link from published mems into your own data—they remain visible. But it cannot modify them, write instructions through them, or be directed by their embedded guidance.

# Answer B

A published mem you install becomes a read-only mounted entity graph that your agent can read but not modify. It can contain links that reference your own mems, but cannot alter your data or act as executable instructions to your agent.

What it can do: A published mem mounts as immutable data — an indexed collection of typed entities, relationships, and markdown content. Your agent can query it through search or entity-read operations and follow wikilinks into it from your writable mems, provided you grant cross-mem permission. It appears in workspace scopes and overviews just as a writable mem does, and its content is immediately available for reasoning or federation with your own knowledge graph.

What it cannot do: The published mem has zero write capability. It cannot create, modify, or delete entities in itself or in any of your writable mems. The engine enforces this at the mount level — the archive is installed with read-only capability and every mutation verb (create, update, delete, relate, rename) rejects a read-only mem as the target. Its content is not instructions to your agent; mems are data models, not prompts or executable code. If the published mem's name collides with a writable mem you own, the install rejects it with a shadow gate — a read-only mount cannot obscure a writable one.

How the engine contains it: The `.mem` archive passes through a strict ingress validator at install time before any byte reaches your cache. This validator checks entity structure, id uniqueness, schema compliance, and graph integrity, refusing the archive if any check fails. The validated bytes are cached under a content-addressed key, so two distinct archives cannot collide by name and an existing cache file is bit-identical by construction. Once installed, the workspace's permission table governs whether edges from your mems to the published one are allowed; a cross-mem edge requires explicit grant. The sealed schema the archive carries is staged read-only if needed, and the published mem's mount record persists only in your workspace configuration — neither the installed mem nor any entity in it can alter your writable mems' configs, schemas, or data without an agent's explicit, workspace-authorized operation.
