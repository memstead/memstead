# Glossary

**Status:** in progress (2026-05-10) — definitions land as they get discussed and agreed.

This glossary uses Memstead's **technical register** — the vocabulary that appears in code, MCP tools, schemas, and engine documentation. For the **conceptual register** that frames what Memstead does for end users ("knowledge graph", "planning graph", and similar modal slices), see [VISION.md](VISION.md).

Definitions are normative. Where existing code or docs use different words, they should converge on this file, not the other way around.

Each entry has two parts:

- **Definition** — what the term is, with key properties.
- **Rationale** — why these exact words, what previous misuse this corrects. Includes a *status* token (open / in-progress / done) for any convergence work the entry implies.

**Terms remaining.** None at present — the conceptual surface this glossary covers is closed. New terms will land as new architectural questions surface; today's set is the reframing target.

**Doc convergence.** Definitions here are normative. VISION.md, CLAUDE.md, and README.md converged at the 2026-06 rename; some companion docs still pre-date this glossary and use different vocabulary in places; they converge to this file at their next revision. Where a current concept doc contradicts a glossary entry directly, the entry calls it out under *status* in its rationale.

---

## Vault

### Definition

> A named, schema-pinned markdown entity graph about exactly one chosen subject — a **typed model of that subject**.

The model's modal flavour (knowledge / planning / inquiry / spec / hybrid) follows from the pinned schema. Mutations are typed and pass exclusively through the engine, leaving append-only structured provenance.

**Lifeforms.**

- **Open** — live in a workspace, writable. Realised via two [storage backends](#storage-backend): **folder** (a directory on disk — either a subfolder under a multi-vault workspace, or the workspace root itself in the collapsed single-vault form) or **git-branch** (a named branch inside a vault-repo gitdir).
- **Sealed** — `.mem` archive, immutable, content-addressed, publishable and transportable via [memstead.io](https://memstead.io).

**Role.** Atomic unit for [mount](#mount), schema-pin, cross-vault permissions, and registry distribution.

**Subject-discipline.** Editorial — ad-hoc scratch vaults like `exec-*` are a documented exception whose subject is the session itself.

**Sizing.** A vault is sized for one coherent subject — typically **1,000–5,000 entities**. Beyond ~10,000, the subject discipline usually means: split into sub-vaults connected by cross-vault edges. The engine does not hard-cap; operators are nudged toward sub-division. Reasons the cap exists in practice:

- **Subject coherence** — a "subject" with 50k entities is usually three subjects in one bucket.
- **Agent navigation** — agents generate workspace-overviews, walk communities, and reason about structure. At 1k–5k they hold the whole picture; at 100k+ every operation degrades to "subset selection first".
- **Algorithm scaling** — Louvain community detection, schema validation at boot, search index build all degrade non-linearly above this range.
- **MCP response shapes** — list-returning tools (`memstead_search`, `memstead_overview`) become hostile to agents without aggressive pagination.

For larger corpora the model is **many small vaults, federated** rather than one giant vault — see [VISION.md](VISION.md#vault-scaling-many-small-federated)'s tier model (Working Vault for active read/write; Indexed Vault for million-scale read-only query, planned).

### Rationale

The definition separates four things the codebase historically conflated: the *logical collection* (vault), the *storage form* (folder / branch / archive), the *modal flavour* (knowledge / planning / spec / …), and the *mount granularity* (a pro-flavour vault-repo gitdir contains N vaults).

Three constraints earn their place in the definition by code-verification:

- **"schema-pinned"** (not "-bound") because `VaultConfig.schema: SchemaRef` is a versioned reference lock, not a generic binding. Same verb the next sentence uses ("from the pinned schema") — consistency.
- **"entity graph"** (not "corpus") because typed relationships are first-class: schema-validated vocabulary, traversal, community detection. "Corpus" understates the graph layer.
- **"Mutations are typed and pass exclusively through the engine, with append-only structured provenance"** — without this clause, a vault is indistinguishable from a raw markdown folder. The CLAUDE.md rule against direct `.md` edits is not a stylistic convention; it is a definitional property.

Pinning the metaphor to the sealed form (`.mem` registry archive) is what makes "vault" the right word: a sealed archive is exactly a content-addressed subject capsule. Open vaults are the same kind of thing in a writable state.

---

## Workspace

### Definition

> A named runtime context that lists a set of vault mounts and the policy that governs them collectively.

The workspace is persisted in a single configuration file at the workspace root. The engine boots one workspace per process; every Claude Code session, MCP server invocation, or CLI command operates against that workspace's mount set.

**Contents.**

- **Workspace metadata** — name, description, format version.
- **Vault mounts** — which vaults the workspace mounts, where each is sourced from (folder path, branch reference inside a vault-repo, `.mem` archive), and how each is attached (read / write, eager / lazy, cross-linkable / isolated).
- **Cross-vault permissions** — the directed allowlist for wikilinks between mounted vaults.
- **Workspace-level policy** — mutation requirements (mandatory notes, expected-hash discipline), drift behaviour, vault lifecycle allowlists, plugin hooks.
- **Pipeline configuration** — scopes, projections, ingests; persisted centrally.

Schema definitions and per-vault configs are **not** workspace-level — they live with each mount's [storage backend](#storage-backend). The workspace just mounts backends and dispatches schema resolution through them in a fixed order (local → built-in → registry). See [Schema](#schema).

**Role.** Atomic unit for engine boot. Defines the visible vault universe for any agent or CLI invocation operating against this workspace.

### Rationale

The definition separates "workspace" from three concepts historical code conflated it with:

- **Workspace ≠ folder.** A workspace may carry zero, one, or many vault mounts. The single-vault collapsed-folder layout is one configuration of the workspace store, not its definition.
- **Workspace ≠ git repository.** A workspace mounts vaults; each mount references a storage backend (folder, git-branch, or archive). A vault-repo is one possible target for git-branch mounts — the same workspace may carry folder-backed mounts alongside it.
- **Workspace ≠ vault.** A vault exists independent of any workspace; any workspace may mount it under its own capability and policy. The single-vault case where workspace and vault collapse to the same folder is a degenerate configuration, not the definition.

**Status:** largely realized. The single canonical marker (`.memstead/workspace.toml`) plus the engine-managed `.memstead/state/mounts.json` replaced the historical `.mdgv.toml` / `.mdgv/config.json` split, and `memstead-base` carries `Workspace`, `Mount`, and `FileWorkspaceStore` as first-class types. The residual rename of `memstead-git-branch` (which now hosts the git-branch storage backend, not the workspace concept) is tracked separately.

---

## Graph

### Definition

> The live, mutable form of typed models in a workspace, at any compositional level.

At its smallest, one vault's content (a vault-graph — homogeneous: one subject, one schema). At its largest, a workspace's full mosaic — every mounted vault-graph united by their cross-vault edges (a workspace-graph — heterogeneous: multi-subject, multi-schema). A graph at one compositional level is built from graphs at smaller levels; the union is itself a graph in the mathematical sense.

**Compositional levels.**

- **Vault-graph** — the content of one [vault](#vault). Homogeneous (one schema, one subject); bounded; no cross-vault edges of its own — cross-vault edges live at the workspace level.
- **Workspace-graph** — every mounted vault-graph plus the cross-vault edges among them. Heterogeneous (multi-schema, multi-subject); the structure a user sees when working in a [workspace](#workspace).

**Role.** The user-facing prose word for the live form. Distinguishes the working state from the **sealed** distribution form, which is a vault (`.mem` archive). When a user says *"graph"*, the level of composition is settled by context — solo subject means vault-graph; multi-subject project means workspace-graph. Both are correct uses of "graph".

### Rationale

The friction that motivates this entry: in natural prose, *"vault for my project"* breaks. *Vault* doesn't say what's inside, and the *for* preposition wrongly implies the project is what's being stored. *"Graph for my project"* flows because *graph* names a structure and the project is the subject that structure is *of*.

Three things this entry keeps clear:

- **Recursion is a feature, not a problem.** A union of typed sub-graphs with cross-edges is itself a graph — that is the mathematical definition, not a fudge. *"Graph"* therefore works at the vault-level and the workspace-level without needing different words.
- **Graph is user-facing; vault is technical.** Both refer to the live, mutable structure, but *vault* carries CLI / MCP / code semantics (the unit of mount, schema-pin, distribution). *Graph* is the prose word for onboarding, marketing, talks. *Vault* appears when the conversation is technical or about the sealed / published form.
- **"Sub-graph", "area", or "part" disambiguate when needed.** *"My project graph has three sub-graphs: engine, macOS, plugin"* works in technical writing; *"three areas"* often reads more naturally in casual speech. Both refer to vault-graphs from the user's prose perspective.

**Status:** open. The convergence work this implies: docs, onboarding, and marketing prose adopt **graph** as the live-form word and reserve **vault** for technical and distribution contexts.

---

## Mount

### Definition

> The act of making one vault — together with its schema and capabilities — available to a running engine. Also the resulting record in the engine's mount registry.

One mount = one vault. If five vaults live in the same vault-repo gitdir, that is five mount operations producing five mount records — the shared gitdir is an implementation detail of how the engine reuses a backend handle, not part of the mount concept.

**Properties of a mount.**

- **Vault** — the named subject corpus being made available.
- **Storage** — where the vault's bytes physically live: folder path, branch reference inside a git repository, or archive file.
- **Capability** — read or write.
- **Lifecycle** — eager (loaded at engine start) or lazy (loaded on first access).
- **Cross-linkable** — whether other mounts in the same workspace may reference entities in this one via wikilinks.

**Interfaces that issue mounts.**

- **Rust API** — the most primitive form. The caller passes a list of mount records to the engine constructor directly. The macOS app uses this via UniFFI bindings.
- **CLI** (`memstead`) — reads a [workspace store](#workspace-store) on startup, recovers the mount list, and issues the mount calls against the engine.
- **MCP server** (`memstead-mcp`) — same pattern as the CLI; reads a workspace store, issues mounts, then exposes the running engine over the MCP protocol.

### Rationale

The definition is operations-first. Mount is the verb of making a vault available; the mount record is the noun of the resulting registry entry. Both are per-vault.

Two pitfalls this avoids:

- **"Mount = storage container" is structurally wrong.** Today's `memstead-git-branch::vault_repo_mounts` defines `Mount` as a Rust type that wraps a gitdir handle — one such type per vault-repo, surfacing N vaults. That is an implementation helper for sharing a gitdir connection across multiple per-vault mounts; it is not the conceptual mount. The user-facing operation is *"mount this vault with these capabilities"*, not *"mount this gitdir and hope each branch inherits sensible defaults"*.
- **Per-vault capabilities don't need a nested override layer.** If five vaults sit in one vault-repo and one of them is read-only while the rest are write, the per-vault mount record is the natural place. A storage-container-level mount would need a sub-property *vault-overrides* — which is just per-vault mounts pretending to be subordinate config.

**Status:** realized. `memstead-base::workspace::Mount` is the per-vault conceptual mount, carrying `storage: MountStorage::{Folder, GitBranch, Archive}` uniformly. The engine accepts `Vec<Mount>` directly via the workspace store; the legacy `vault_repo_mounts` shared-gitdir-handle helper has been deleted, with shared gitdir reuse now handled inside the git-branch backend itself.

---

## Workspace store

### Definition

> The persisted form of a workspace's configuration — a logical data structure containing the mount list, the cross-vault permission table, the workspace-level policy, and the workspace-level pipeline configuration. How it reaches durable storage is the responsibility of a replaceable persistence adapter.

The workspace store is logical, not physical. Its content is fixed; the form on disk is an adapter's choice. The store does **not** carry schema definitions or per-vault configs — those live with the [storage backend](#storage-backend) that holds each vault's content, and travel with it (a cloned vault-repo carries its schemas in the same gitdir; a folder workspace carries them in its own `.memstead/`).

**Contents.**

- **Workspace metadata** — name, description, format version.
- **Mount list** — one entry per mounted vault, each carrying the vault's storage reference and attachment properties (capability, lifecycle, cross-linkable). The schema pin lives in per-vault config in the storage backend, not in the mount entry.
- **Cross-vault permissions** — directed allowlist for wikilinks between mounted vaults.
- **Workspace-level policy** — mutation requirements (mandatory notes, expected-hash discipline), drift behaviour, vault lifecycle allowlists, plugin hooks.
- **Pipeline configuration** — scopes, projections, ingests. Per-vault primitives, persisted centrally because they change with workspace lifecycle, not with vault content.

**Role.**

- **Required** by the CLI (`memstead`) and MCP server (`memstead-mcp`) — they consult the workspace store through their configured persistence adapter on every invocation to recover the workspace configuration before issuing mount calls against the engine.
- **Not required** by direct engine API consumers (e.g. the macOS app via UniFFI) — they construct workspace configuration in memory and pass it to the engine constructor without persisting anything. No adapter involved.

**Persistence adapters.**

- **File adapter** (target default) — distributes the store across files under a single `.memstead/` umbrella at the workspace root:

  ```
  <workspace>/
  ├── .memstead/
  │   ├── workspace.toml        ← operator config (rules, permissions, policy, plugin hooks)
  │   ├── state/mounts.json     ← engine-managed mount records
  │   ├── scopes/               ← pipeline configs (workspace-level)
  │   ├── projections/
  │   └── ingests/
  ├── <vault folders or storage containers like vault-repo/>
  └── ...
  ```

  Two halves of the store live in separate files: `workspace.toml` carries operator-curated config (vault management rules, cross-vault permissions, mutation policy, plugin hooks); `state/mounts.json` carries engine-managed mount records. The split mirrors two different update frequencies and two different authors — operator edits rules rarely (via `memstead workspace allow-create / grant-cross-link / set-mutations / show`; hand-editing is the fallback for batch edits and `[plugin.*]` sections the CLI does not own); engine writes mount records on every `mount add` / `mount remove`. Keeping them separate avoids merge-conflicts between operator intent and agent state mutations.

  Pipeline configs sit alongside as separate directories under `.memstead/`.

- **Alternative adapters** (potential, not implemented) — single-file (TOML or JSON; conflates operator-config and engine-state and is therefore discouraged for shared workspaces), SQLite-backed (separate tables for config and state, atomic mount mutations), remote-service-backed, encrypted store, in-memory test fixture. The engine API is adapter-agnostic; new adapters do not change engine behaviour, only the source from which configuration is materialized.

### Rationale

Three reasons to define the workspace store as logical content with a swappable persistence adapter, rather than as one specific file format:

- **Storage form is an implementation choice, not a definitional one.** What the workspace store conceptually contains is fixed. How those contents reach disk is a per-deployment concern. Pinning the definition to "a TOML file" or "a SQLite file" would conflate the two.
- **Configuration is one coherent whole.** The workspace's operator-edited `.memstead/workspace.toml`, the engine-managed `.memstead/state/mounts.json`, the per-vault `.memstead/config.json` (or `.memstead/vaults/<vault>/config.json` in multi-vault layouts), and the pipeline directories under `.memstead/` are all expressions of the same logical entity, behind a single persistence adapter.
- **"Store" not "database".** Database implies SQL, indexes, joins, transactions — none of which the concept needs. Store carries the same logical-vs-physical split without that semantic baggage.

**Schemas and per-vault configs are not in the workspace store.** They live with the storage backend that holds each vault's content. This is what keeps a vault-repo (or a folder vault, or an archive) self-contained — clone or copy it, get the vaults *and* their schemas and configs. The workspace store is the layer that mounts storage backends; the storage backends own their own per-vault metadata.

**Status:** partially realized. The store schema and the file adapter are landed: `WorkspaceStoreAdapter` lives in `memstead-base`, `FileWorkspaceStore` is the default implementation, and the `.memstead/workspace.toml` + `.memstead/state/mounts.json` split that the file adapter writes is the canonical on-disk shape. The remaining work is alternative adapters (SQLite, in-memory, remote-service) — none implemented today; only the file adapter exists — and the pipeline-folder migration into the unified `.memstead/` layout. **Doc convergence:** a refined vocabulary (Medium / Facet / Projection) is planned for what today's code calls scope / projection / ingest; reconciling it with the glossary is future work.

---

## Storage backend

### Definition

> The mechanism that holds one vault's bytes — folder of files, branch of a git repository, or `.mem` archive.

A storage backend is referenced per [mount](#mount), not per workspace. Multiple mounts may reference the same underlying resource (e.g. several mounts each pointing at one branch inside a shared vault-repo gitdir); the engine pools shared handles internally, but the conceptual storage backend reference is per-mount.

**Three sibling kinds.**

| Kind | Where bytes live | Writable | History | Vault lifeform |
|---|---|---|---|---|
| **Folder** | A directory on disk containing `.md` entity files plus a per-vault provenance log (today: `.memstead/changes.jsonl`) | yes | no | open |
| **Git-branch** | A named branch inside a `vault-repo/.git/`; entity bytes are git blobs, provenance lives in commit-message trailers | yes | yes (full git log per vault) | open |
| **Archive** | A `.mem` zip file — immutable, content-addressed. Source: locally exported, registry-downloaded, or otherwise materialized | no (sealed) | no | sealed |

**Per-backend schema and per-vault config storage.**

Each backend carries its own [schemas](#schema) and per-vault configs in a parallel layout — schemas live with the storage that holds the vaults pinning them.

| Backend | Vault content | Per-vault config | Schemas |
|---|---|---|---|
| **Folder** (multi-vault) | `<workspace>/<vault>/*.md` | `<workspace>/.memstead/vaults/<vault>/config.json` | `<workspace>/.memstead/schemas/<name>@<version>/` |
| **Folder** (collapsed single-vault) | `<workspace>/*.md` (workspace root IS the vault) | `<workspace>/.memstead/config.json` | `<workspace>/.memstead/schemas/<name>@<version>/` |
| **Git-branch** | `refs/heads/<vault>` tree | `__MEMSTEAD:vaults/<vault>/config.json` | `__MEMSTEAD:schemas/<name>@<version>/` |
| **Archive** | inside `.mem` zip | inside `.mem` zip | inside `.mem` zip |

The git-branch backend's `__MEMSTEAD` ref unifies what today's pro flavour splits into two orphan refs (`__SCHEMAS` for schema YAMLs, `__SYSTEM` for per-vault configs). One ref, parallel structure with the folder backend's `.memstead/` directory.

The folder backend supports two operator layouts: **multi-vault** (the workspace root is a container; each vault is a subfolder) and **collapsed single-vault** (the workspace IS the one vault; config at root, no `vaults/` subfolder). The collapsed form is detected by `.memstead/config.json` at workspace root instead of `.memstead/vaults/<name>/config.json` entries.

**Per-mount git-repo (git-branch backend only).**

A git-branch mount's `repo` field is per-mount. Multiple mounts in the same workspace may target the same git-repo (default convention: one `vault-repo/` per workspace, all branches inside) or different repos (e.g. private `planning-repo/` separate from public `vault-repo/`). The workspace store binds them; cross-vault edges resolve through the workspace, not through git. Trade-offs of the multi-repo variant:

- **Cross-mount mutations are not atomic across separate gitdirs.** A refactor that touches two vaults in different repos produces two commits that can fail independently. Best-effort semantics with repair-on-next-read replaces single-commit atomicity.
- **Operator overhead multiplies.** Clone / pull / push runs once per repo; sync responsibilities scale with repo count.
- **Workspace portability requires path discipline.** Absolute or external `repo` paths in `state/mounts.json` break on workspace-clone; relative-to-workspace paths or a known shared parent directory keep portability.

Default convention: one git-repo per workspace, multi-repo only when the operator has a concrete reason (different sharing model, different visibility, different sync target).

**Capabilities follow from kind.**

- **Folder** and **git-branch** are open backends; mounts using them may carry either `read` or `write` capability.
- **Archive** is sealed; mounts using it always carry `read` capability — write semantics are not defined for content-addressed archives.

**Backend differentiators — what folder deliberately does NOT do.**

The folder backend is intentionally simple: it carries entity files and per-vault config, nothing more. Two capabilities that the git-branch backend offers and the folder backend deliberately does not:

- **Drift detection.** Git-branch tracks each vault's HEAD SHA; when a sibling writer advances it, the engine emits `VAULT_RELOADED` and auto-reloads. Folder has no equivalent signal — `VaultBackend::current_head` returns `None`. Multi-process workflows (two CLIs on the same workspace, macOS app + Claude Code plugin, iCloud/Dropbox sync) need git-branch.
- *(Open: change history / mutation provenance — see [Provenance](#provenance--mutation-log) for the current shape and the trade-off.)*

The product position: **folder = simple, single-context notes. Git-branch = multi-actor, history-bearing knowledge.** Anyone who needs drift detection, audit trails, or multi-process safety chooses git-branch. The folder backend is not "git-branch lite" — it is a distinct affordance for users who want their vault to be a plain directory of markdown.

### Rationale

The three kinds map directly to a [vault's](#vault) two lifeforms: folder and git-branch are the open lifeform's two realizations; archive is the sealed lifeform. There are no other lifeforms — if a fourth backend kind appears later (e.g. registry-auto-refreshed archive), it composes existing kinds rather than introducing a new lifeform.

Two pitfalls this avoids:

- **Conflating "storage backend" with "the workspace itself".** Today's `memstead-git-branch` Rust crate hosts the git-branch backend; the crate name suggests it owns the workspace concept, but it does not. After convergence, that crate is renamed for what it actually implements: a git-branch backend, sibling to a folder backend and an archive backend.
- **Conflating capability-by-mount with capability-by-backend.** A folder backend can be mounted read-only (a workspace's choice); an archive can only be read (an intrinsic property of content-addressed sealed bytes). Splitting *kind* from *capability* keeps error envelopes and mount semantics consistent — a write attempt against an archive fails at a different layer than a write attempt against a read-only-mounted folder.

**Status:** largely realized. `VaultBackend` lives in `memstead-base::backend` and the engine talks to every storage kind through it: the folder backend (`memstead-base::storage::filesystem`) and the archive read path (`memstead-base::storage::archive`) are linked unconditionally, the git-branch backend (`memstead-git-branch`) is added by the pro flavour via a registered backend factory. The residual cleanup is the `memstead-git-branch` crate rename and the migration of a handful of ops-level methods (`read_agent_notes`, `export_to_archive`, `changes_since` with rename detection) off the trait into backend-specific helpers, both tracked separately.

---

## Schema

### Definition

> The type vocabulary that constrains a vault's content — what entity types exist, what sections each type has, which sections are required, what relationship types are allowed, what metadata fields are valid.

A schema is the contract that makes a vault a *typed* model rather than a raw markdown collection. Entities in a schema-pinned vault must conform to one of the schema's declared types; mutations are validated against the schema's section, metadata, and relationship rules at the engine boundary.

**Three roles the term plays.**

- **Schema definition** — the actual type vocabulary, expressed as YAML files (today: `manifest.yaml` plus per-type files). Declares types, their sections, metadata fields, relationship vocabulary, write rules. Versioned. The distributable folder form is a **schema package**: the YAML files plus README, optionally carrying a `vault-template.json` (a per-vault config starter consumed client-side at vault creation).
- **Schema pin** — a [vault's](#vault) reference to one specific schema definition: a `name@version` string (e.g. `software@0.1.0`). Stored in the vault's per-vault config inside its [storage backend](#storage-backend).
- **Schema registry** — the resolution mechanism that turns a pin into a definition at engine startup. Consults three sources in order — local storage, built-in, registry — and returns the first match.

**Sources of schema definitions, in resolution order.**

1. **Local storage** — schemas carried by the same storage backend as the vault that pins them:
   - **Git-branch backend** — `__MEMSTEAD:schemas/<name>@<version>/` on the `__MEMSTEAD` ref in the same gitdir.
   - **Folder backend** — `<workspace>/.memstead/schemas/<name>@<version>/` (or `<vault>/.memstead/schemas/<name>@<version>/` for the collapsed single-vault form).
   - **Archive backend** — `schemas/<name>@<version>/` inside the `.mem` zip.
2. **Built-in** — schemas compiled into the engine binary, available on every install (today: `default@1.0.0` plus the bundled schema packages `software`, `planning`, `project`, `ingest`). Used when no local match. Works offline by definition.
3. **Registry** — schemas served by memstead.io, fetched on demand and cached locally. Reserved slot; not implemented in the current rebuild.

**Authoring.**

Schemas are authored where they live — in the storage backend that holds the vaults pinning them. A schema added to a git-branch backend's `__MEMSTEAD:schemas/` is authored by committing to that ref. A schema added to a folder backend's `.memstead/schemas/` is authored by writing to that directory. Sealed archives embed their schema at seal time, frozen for transport.

Multiple vaults sharing a storage backend (e.g. five vaults as five branches in one vault-repo) share schemas at the backend's scope — one copy serves all of them.

### Rationale

Three confusions this entry resolves:

- **"Schema" without qualifier is ambiguous.** Code, docs, and conversation use the bare word for whichever role is contextually relevant — often without distinguishing definition (the YAML), pin (the reference), and registry (the resolver). Naming the roles separately fixes that.
- **Schemas live with their storage.** A vault's schema travels with the storage backend that holds the vault. A cloned vault-repo carries its schemas in the same gitdir; a folder workspace copied to a USB stick carries them under its own `.memstead/`; an archive embeds them in the zip. The workspace store does not host schemas — the workspace is the layer that mounts storage backends, not the layer that owns schema definitions.
- **Resolution-order is fixed and not arbitrary.** Local storage wins so a vault is self-resolvable from its own backend. Built-in is the fallback for shipping defaults (`default@1.0.0` works without network or local authoring). Registry is the third-source fallback for pins neither local nor built-in carries. The order is hard-coded; workspaces do not customise it.

**Status:** in-progress — substantially converged (re-baselined 2026-06-13). Done: the unified `__MEMSTEAD` ref on the git-branch backend (landed with the workspace-store rebuild, replacing `__SCHEMAS` and `__SYSTEM`); a uniform schema registry with the fixed three-source order in `memstead-base`/`memstead-schema`; `SCHEMA_NOT_FOUND` carries a `details.sources` payload naming which sources were consulted (local storage / built-in / remote-reserved) and the versions each held; authoring paths on both flavours — authored packages resolve at boot from `<workspace>/.memstead/schemas/<name>@<version>/` (folder) and the `__MEMSTEAD:schemas/` ref (git-branch); `memstead schema validate <path>` and `memstead schema install <name|path>` for both backend destinations; the folder-backend schema location is fixed at `.memstead/schemas/` and the `schemas_dir` workspace.toml key is retired (a legacy key is warned and ignored, never honoured); the schema-pin relocation is complete — `VaultConfig.schema` (the vault's per-vault backend config) is the authoritative pin and `Mount.schema` is now an optional expectation assertion (`Option<SchemaRef>`), so a copied vault resolves without consulting any workspace's `mounts.json`; built-in packages ship a `vault-template.json` consumed by `memstead vault create`/`init` (which accept an opaque `write_guidance` map persisted into the seed config); and the JSON meta-schemas are published under `.memstead/meta-schemas/` with the `# yaml-language-server` directive on bundled packages for IDE-side validation. Still open: the remote/registry resolution step — fetching a pinned schema from memstead.io is reserved (the third source slot is diagnostic-only; no download path yet).

---

## Cross-vault edge

### Definition

> A relationship between an entity in one mounted vault and an entity in another mounted vault of the same workspace.

Encoded as a wikilink in the source entity's markdown: `[[target-vault:target-slug]]`. The target vault is named by the workspace's mount; resolution is workspace-level — the source vault has no knowledge of the target vault's contents.

Within-vault wikilinks (`[[slug]]`) resolve inside the source vault and are **not** cross-vault edges. Cross-vault edges always cross the vault boundary.

**Properties.**

- **Source entity** — lives in a vault mounted in the workspace.
- **Target entity** — lives in another vault mounted in the same workspace, identified by `vault-name:slug`. The target vault's storage backend (folder, git-branch, archive) is irrelevant to the edge.
- **Permission** — every cross-vault edge must be authorised by the workspace's cross-vault permission table. Source vault → target vault must appear in the directed allowlist; otherwise the edge is rejected at validation. Cycles are valid policy.
- **Direction** — edges are directed (one source, one target). A symmetric link requires two edges and reciprocal permission.

**Cross-workspace edges (today: Tier-3 wikilinks like `[[scope/name:slug]]`)** are cross-vault edges to a [mount](#mount) whose storage backend is an archive — typically downloaded from memstead.io and mounted as read-only, cross-linkable. The wikilink form is different at the surface, but the edge itself follows the same rules: permission required, target vault must be mounted, target entity must exist.

### Rationale

Two pitfalls this avoids:

- **Cross-vault edges live at the workspace layer, not the vault layer.** A vault has no knowledge of other vaults — it carries entities and within-vault wikilinks only. The workspace is the layer that mounts multiple vaults, knows their names, and resolves edges between them. This is why the cross-vault permission table belongs in the [workspace store](#workspace-store), not in any single vault's metadata.
- **The Tier-1 / Tier-2 / Tier-3 framing was misleading.** Today's docs distinguish three tiers of wikilinks (same-vault, cross-vault-same-vault-repo, cross-repo-via-registry). In the reframed model only two distinctions exist: within-vault (resolves inside the vault) and cross-vault (resolves through the workspace). The "cross-repo-via-registry" tier collapses into "cross-vault edge to an archive-backed mount" — once the archive is mounted, it is another vault in the workspace.

**Status:** open. The convergence work this implies: collapse the three-tier wikilink framing in engine docs and code comments into the within-vault / cross-vault binary; treat the registry-published case as a mount choice (archive backend, fetched from memstead.io), not as a wikilink tier of its own; the cross-vault permission table in the [workspace store](#workspace-store) is the single authorisation point for any edge crossing a vault boundary, regardless of the target's storage backend.

---

## Entity

### Definition

> An atomic, addressable element in a vault — a single markdown document conforming to one type from the vault's pinned schema.

An entity is the smallest unit the engine reads, writes, links, or validates. It carries a YAML frontmatter (typed metadata fields) and named sections (typed content blocks). It is referenced by an ID derived from its title and vault, and may declare outgoing relationships to other entities.

**Properties.**

- **ID** — a slug-shaped, vault-prefixed identifier of the form `<vault>--<title-slug>` (e.g. `engine--commit-provenance-trailers`). Renaming an entity changes its ID; the engine rewrites incoming wikilinks atomically as part of the rename operation.
- **Type** — declared in frontmatter; must be one of the types declared by the vault's pinned [schema](#schema).
- **Sections** — named content blocks (e.g. `## Identity`, `## Purpose`, `## Definition`, `## Rationale`). The schema declares which sections each type requires, allows, or treats as catch-all.
- **Metadata fields** — typed key-value pairs in frontmatter (e.g. `created_date`, `tags`, `level`). The schema declares which fields each type requires, their types, and any enum constraints.
- **Outgoing relationships** — typed wikilinks to other entities, either within the vault (`[[slug]]`) or via [cross-vault edges](#cross-vault-edge) (`[[target-vault:target-slug]]`).

**Identity.** An entity is identified by its ID, not by its file path or storage location. The same entity may be encoded as a `.md` file (folder backend), a git blob (git-branch backend), or a zip entry (archive backend); identity is content plus ID, not encoding.

### Rationale

Two distinctions worth preserving:

- **Entity ≠ file.** Calling an entity a "file" leaks one encoding (folder backend) into the conceptual model. The same entity has different physical forms across the three storage backends; the entity itself is its content plus identity.
- **Entity ≠ raw markdown.** An entity is markdown *constrained by a schema* — its sections, metadata, and relationships all conform to its vault's schema pin. Without that constraint, the markdown is just text; with it, it is a typed entry in a typed model.

**Status:** n/a — no convergence work. The term has been stable throughout the codebase; the entry exists to document the boundary, not to redirect implementation.

---

## Subject

### Definition

> The topical focus a [vault](#vault) is *about* — what makes one vault distinct from another that pins the same [schema](#schema).

A vault has exactly one subject. The subject is editorial — not enforced by code. The vault's name and entity content together imply it; convention keeps the entities on-subject.

**Examples.** The Memstead project's own pro vault-repo carries five vaults with the `software@0.1.0` schema; each has a different subject — *the engine codebase*, *the macOS app*, *the Claude Code plugin*, *the registry server*, *the project as a whole*. Same schema, five distinct vaults, because five distinct subjects.

**Boundary.**

- **Subject ≠ schema.** Schema is type vocabulary; subject is what the vault is about.
- **Subject ≠ namespace.** Namespace is identifier-scoping; subject is editorial direction.
- **Subject ≠ code-enforced.** The engine cannot judge whether an entity is on-subject; only the operator's discipline keeps the vault coherent. Ad-hoc scratch vaults (`exec-*`) are documented exceptions.

### Rationale

Subject earns its own entry because it is the criterion that distinguishes vaults *logically* once Schema and Storage are equal. Without it, "why is this its own vault rather than entities in some other vault?" has no principled answer.

**Status:** n/a — definitional, not implementation work.

---

## Modal flavour

### Definition

> The conceptual genre a vault inhabits — knowledge, planning, inquiry, spec, or hybrid — determined by the [schema](#schema) the vault pins.

| Flavour | Schema constrains entries to … | Vault becomes a … |
|---|---|---|
| **Knowledge** | factual claims, definitions, concepts | knowledge graph |
| **Planning** | deliberation primitives (goal, option, decision, step, risk, open_question) | planning graph |
| **Inquiry** | questions and hypotheses with evidence | inquiry graph |
| **Spec** | prescriptions (specs, requirements, contracts, constraints) | spec graph |
| **Hybrid** | a mix of the above | hybrid model |

The five flavours are **not** hard-coded in the engine. They emerge from each schema's type vocabulary. Adding a new flavour means authoring a new schema with a coherent set of types — no engine change.

**Boundary.**

- A flavour is a *read-back* of the schema choice, not an independent axis. `default@1.0.0` is hybrid because of the types it declares; `planning@0.1.0` is the planning flavour for the same reason.
- The flavours appear in user-facing prose ("create a knowledge graph", "create a spec graph"); the technical glossary uses [Vault](#vault) as the umbrella with [Schema](#schema) as the determining attribute.

### Rationale

Why the entry: in user-facing language, modal flavour is the *concrete name* a person uses when describing what their vault is. The technical register has Vault + Schema; the conceptual register has the modal slice. The two are systematically connected, and naming both keeps register-translation honest.

Why it is not its own enforcement axis: every schema design implies a flavour by what types it includes. Adding a flavour-as-attribute to the engine would be redundant — the schema already encodes it.

**Status:** n/a — derived from [Schema](#schema). Convergence work for adoption in user-facing prose is tracked under [Graph](#graph).

---

## Wikilink

### Definition

> A markdown reference to another [entity](#entity), of the form `[[id]]` or `[[vault:id]]`.

Two kinds, distinguished by whether the link crosses a vault boundary:

- **Within-vault wikilink** — `[[entity-slug]]`. Resolves inside the source entity's vault.
- **Cross-vault wikilink** — `[[target-vault:target-slug]]`. Resolves through the workspace; the target vault must be mounted in the same workspace, and a [cross-vault edge](#cross-vault-edge) permission must allow the source-target direction.

Wikilinks may be *typed* (`[[REL_TYPE: target]]`, e.g. `[[DEPENDS_ON: foo]]`) or *untyped* (default `REFERENCES` edge). The schema's relationship vocabulary constrains which `REL_TYPE` values are valid.

**Cross-workspace references** — pointing at vaults published by other workspaces (today's `[[scope/name:slug]]` form for registry-published vaults) — are not a separate wikilink kind in the reframed model. They are cross-vault wikilinks targeting a [mount](#mount) whose [storage backend](#storage-backend) is an archive, downloaded from memstead.io. Once mounted, they resolve like any other cross-vault wikilink.

### Rationale

Two clarifications this entry locks in:

- **Two kinds, not three.** Today's docs distinguish three "tiers" (same-vault, cross-vault-same-vault-repo, cross-repo-via-registry). The reframed model collapses Tier-2 and Tier-3 — both are cross-vault wikilinks; the difference is which storage backend the target uses.
- **Wikilinks are entity content, not workspace state.** A wikilink lives in the source entity's markdown bytes. Resolution and permission-check happen at the workspace layer at read or write time, but the link itself travels with the entity.

**Status:** open — engine docs and code comments still use Tier-1 / Tier-2 / Tier-3 vocabulary. Convergence is folded into [Cross-vault edge](#cross-vault-edge).

---

## Provenance / Mutation log

### Definition

> An append-only structured record of every mutation an entity in a vault undergoes — who, when, what, and optionally why.

Every mutation operation (`create`, `update`, `delete`, `relate`, `rename`) produces one log entry. Entries carry:

- **Timestamp** — UTC, ISO-8601.
- **Kind** — `create` / `update` / `delete` / `relate` / `rename`.
- **Entity** — the affected entity's ID.
- **Actor** — the role that initiated the mutation: `agent` (MCP / chat subprocess), `cli` (memstead binary), `external` (out-of-band), `unknown`.
- **Client** — optional name + version (e.g. `claude-code@2.1.136`), picked up from the MCP client handshake.
- **Note** — optional free-text justification supplied by the agent.

**Realizations per storage backend.**

- **Git-branch backend** — each mutation is one git commit; provenance lives in the commit-message trailer block (`Tool:`, `Actor:`, `Client:`). Per-vault provenance is the branch's git log.
- **Folder backend** — each mutation appends one JSON line to `.memstead/changes.jsonl`. Per-vault provenance is that file.
- **Archive backend** — sealed; no provenance writing. The archive freezes state at seal time; original provenance lives in the source backend the archive was exported from.

The data shape is the same; only the persistence form differs.

### Rationale

Why it earns a glossary entry: provenance is load-bearing in the [Vault](#vault) definition ("typed-mutated markdown entity graph … with append-only structured provenance"). Without it, a vault is indistinguishable from a markdown folder. Naming the structure makes the constraint concrete and the per-backend realizations comparable.

Why "append-only" matters: it makes auditing tractable, it makes incremental sync possible (`memstead_changes_since` MCP tool), and it commits the engine to never silently rewriting history.

Why two realizations look so different on disk: git already has commit-message provenance with rich semantics; folder backend has no equivalent, so a sidecar log is the closest analog. Both are append-only; both round-trip through the same engine API.

**Status:** open. The underlying mechanisms work today (Phase 1 verified the JSONL changelog round-trips through `FilesystemMcpServer`). Convergence work: define a uniform `Provenance` type in `memstead-base` so both backends produce structurally identical records regardless of persistence form.

---

## Pipeline (medium · facet · projection · ingest)

### Definition

> The workspace-level mechanism that populates a vault's content from external bodies of information rather than from direct agent writes.

Four primitives compose the pipeline:

- **Medium** — a named reference to a body of information the vault acknowledges as part of its territory (a codebase, a filesystem, another vault, a git repo, a web resource). Passive: a medium does not fetch, transform, or filter. It only names what's out there.
- **Facet** — a specific perspective from which a projection engages with a medium: a scope (allow / deny patterns), an engagement contract (verbs, tools, discipline), and an optional preparation step (PDF→markdown, audio→transcript, codebase→code-map).
- **Projection** — a declared mapping from sources (one or more facets over mediums, plus optional reference vaults) into a destination vault. Defines *what* the projection produces.
- **Ingest** — operational configuration for running a projection: mode (`discovery` / `refinement` / `one-shot`), trigger (`loop` / `manual` / `on-event`), batch size, deny-path overrides. Defines *how and when* the projection runs.

The pipeline is **per-vault** (each vault declares its own mediums, facets, projections, ingests — different vaults have different territory) but **persisted centrally in the [workspace store](#workspace-store)** because the configuration changes with workspace lifecycle, not with vault content.

**Today's vocabulary vs the refined model.**

| Refined (Medium / Facet / Projection) | Today's code (`scopes/`, `projections/`, `ingests/`) |
|---|---|
| Medium | implicit — the source target a scope filters over |
| Facet | Scope (filter only; engagement contract not modelled) |
| Projection | Projection |
| Ingest | Ingest |

The refined model **separates** territory (medium), engagement (facet), and obligation (projection); today's `Scope` conflates engagement and selection. The convergence target is the four-primitive model.

### Rationale

Why the pipeline lives at workspace level even though its declarations are per-vault: cross-vault references inside projections and shared mediums benefit from a central store; vault content (entities) lives in the storage backend, vault configuration lives in the workspace store. Same vault, two persistence layers.

Why mediums are passive: a medium can be reused across facets without inheriting any one engagement's preparation logic. The medium is "the engine codebase here" or "the filesystem there"; how a particular projection engages with it is the facet's job.

Why ingest is separate from projection: a projection declares what feeds what; the same projection may be run in different modes (full discovery vs incremental refinement) at different times. Ingest carries the mode / trigger / batch — the operational layer over the declarative projection.

**Status:** done (2026-06-14). The four-primitive refactor landed:

- The old `Scope` JSON shape is split into Medium (territory) + Facet (engagement: selection + optional engagement contract + optional `preparation` step), with Projection and Ingest reshaped to reference facets. Engine-side types, the workspace-store persistence (`.memstead/{mediums,facets,projections,ingests}/`), a boot-time read-only loader, and the plugin ingest skill all speak the four-primitive model; no code identifier uses `Scope`/`scope` for the engagement concern.
- The pipeline configs live in the workspace store's persistence adapter (the JSON-folder layout was migrated by `memstead pipeline migrate`; the legacy `scopes|projections|ingests/` folders are retired and unreadable).
- The `Facet.preparation` slot is reserved for non-text mediums (PDF, DOCX, audio); no preparation implementation ships — an ingest whose facet declares one is reported unsupported rather than run. Each non-text medium is a follow-up plan triggered by a real corpus.
- One consolidation remains as a follow-up: moving the ingest skill's `mediums.json` engagement metadata into per-facet `engagement` records (the skill still reads `mediums.json` keyed by medium type; facets carry the *optional* slot).
