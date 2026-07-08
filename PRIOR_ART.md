# Prior Art

Memstead stands on a long lineage of tools and concepts. This document catalogues the ideas, projects, and standards that shaped its design, so readers can see what is genuinely new in Memstead and what is inheritance from the broader ecosystem.

Listing prior art is also a defensive measure: similarities to other tools in this space are *expected and intentional*, not unique-to-Memstead inventions. If you see a pattern in Memstead that resembles something else, this document is where to start looking for the genealogy.

## Knowledge-graph patterns

- **Personal Knowledge Management tools** — **[Obsidian](https://obsidian.md)** (closed source), **[Logseq](https://logseq.com)** (AGPL), **[Foam](https://foambubble.github.io)** (MIT), **[Roam Research](https://roamresearch.com)** (closed). All popularised plain Markdown files plus `[[wikilink]]` syntax plus graph visualisation. Memstead's body wiki-link semantics, two-direction relation graph, and the "mem = folder of markdown files" model are direct descendants. The schema-driven typed-entities layer, the relationship vocabulary, and the engine's MCP-first design are Memstead's additions.
- **[Org-mode](https://orgmode.org)** (Emacs) — the canonical prior art for structured, semantic markup in plain text. Heavy structure, expressive metadata blocks, file-as-database. Memstead's per-entity metadata frontmatter and the section-based body model owe Org-mode a debt.
- **[Notion](https://notion.so)** — popularised the "database of typed structured records" UX for non-technical users. Memstead inherits the typed-entity model conceptually while rejecting Notion's closed, SaaS-bound storage.
- **Triple Stores** ([RDF](https://www.w3.org/RDF/), Wikidata, Neo4j, Datomic) — long-established model for typed subject-predicate-object knowledge graphs. Memstead's entities-as-subjects + relationships-as-predicates structure descends from this tradition; the difference is Memstead's storage being human-editable Markdown rather than triples or property graphs.

## Markdown + Git as substrate

- **[Jekyll](https://jekyllrb.com)** / **[Hugo](https://gohugo.io)** / **[Astro](https://astro.build)** — static-site generators popularised YAML frontmatter for per-page metadata in Markdown. Memstead's frontmatter shape (key-value metadata, type discriminator at the top of each file) follows the same convention.
- **[Pollen](https://docs.racket-lang.org/pollen/)** — structured documents as code, but in Racket. Showed the value of treating document files as compileable artefacts.
- **[Logseq](https://logseq.com)**, **[Foam](https://foambubble.github.io)**, **[Quartz](https://quartz.jzhao.xyz)** — all use git as the persistence layer for personal Markdown knowledge bases. Memstead extends this with first-class git-branch storage (the full mem-repo) for mem-level versioning beyond outer-repo commits.
- **[git-annex](https://git-annex.branchable.com)** — the precedent for using git's content-addressed model for non-source data. Memstead's optimistic-locking model (the `_hash` field on entities) borrows the spirit of content addressing.

## Schema validation

- **[JSON Schema](https://json-schema.org)** ([RFC 8259](https://datatracker.ietf.org/doc/html/rfc8259) for the JSON substrate, draft 2020-12 for the schema dialect) — Memstead uses [schemars](https://github.com/GREsau/schemars) to derive JSON Schema from Rust types for MCP tool parameter schemas. The schema-validates-the-data pattern itself is decades old.
- **[OpenAPI](https://www.openapis.org)** / **[Swagger](https://swagger.io)** — model for declaratively describing HTTP APIs so machine readers can consume them. Memstead uses [utoipa](https://github.com/juhaku/utoipa) for the registry HTTP surface's OpenAPI document.
- **[Protocol Buffers](https://protobuf.dev)** / **[Cap'n Proto](https://capnproto.org)** — schema-first system design. Memstead's schema-first stance for entities echoes this.

## AI-agent integration

- **[Model Context Protocol (MCP)](https://modelcontextprotocol.io)** — Anthropic's protocol for AI agents to call structured tools over stdio (and HTTP). The MCP tool surface, the `read_only_hint` / `idempotent_hint` annotation vocabulary, the JSON-RPC envelope, and the tool-discovery pattern in Memstead are direct uses of the MCP specification — no invention by Memstead beyond which tools to expose. The official Rust SDK [rmcp](https://github.com/modelcontextprotocol/rust-sdk) is the implementation foundation.
- **[Function Calling](https://platform.openai.com/docs/guides/function-calling)** (OpenAI), tool use (Anthropic), built-in tool support (Gemini) — pre-MCP attempts at the same problem. Memstead's typed-tool-surface design fits the family.
- **[LangChain](https://www.langchain.com)** / **[CrewAI](https://www.crewai.com)** / **[AutoGen](https://github.com/microsoft/autogen)** — agent frameworks. Memstead is not bound to any of them; the MCP surface is the integration point.

## The 2026 agent-memory category

The sections above list ancestors. This one lists contemporaries: the systems that, as of 2026, give AI agents durable state. Memstead is part of this category, so honesty demands naming it and placing itself in it.

The category splits along one axis. Most of it **derives** memory: an extraction pipeline distils conversations or documents into a retrieval store, and the agent recalls from it. Memstead sits at the **authored** end: agents and humans deliberately write typed entities into a schema-validated graph, stored as markdown in a git repository the user owns. Both approaches are legitimate; they optimise different things. Derived memory optimises recall per token. An authored mem is a model of a subject — reviewable, diffable, and portable as a versioned package. Notably, the substrate bet is no longer unique: two of the five systems below store memory as markdown files, and one of those keeps them in git. What stays distinctive is the layer above the substrate — schema-validated writes, a controlled relationship vocabulary, and structured commit provenance that travels inside the published package (each entity's authoring rationale ships in the sealed `.mem`, readable offline).

### mem0

[mem0](https://mem0.ai) describes itself as a "universal, self-improving memory layer for LLM applications". An LLM pipeline extracts discrete memories from conversations; since the v3 algorithm (April 2026) the pipeline is add-only — one extraction call, no update/delete pass, memories accumulate rather than being rewritten. Retrieval is multi-signal: vector similarity, BM25 keyword search, entity linking, and temporal reasoning. Storage is a vector store (local Qdrant by default in library mode, Postgres + pgvector in the self-hosted server) plus a SQLite history log; external graph-database support was removed from the open-source SDK in favour of built-in entity linking held in a parallel vector collection. It ships three ways — pip/npm library, Docker self-hosted server, managed platform — under Apache-2.0.

What it is genuinely good at: drop-in cross-session personalisation at scale, with strong (self-reported) benchmark numbers, sub-second retrieval, and a fully self-hostable stack — "you own the stack, the data, and every component" is a fair description of the open-source tier.

How Memstead differs: mem0's memories are derived — LLM-extracted statements ranked for recall. You can self-host every component and query memories through the API, but the substrate is a vector database: there is no human-readable file per memory, no meaningful diff between two states of the store, no schema constraining what a memory may contain, and no typed relationships between memories (entity linking connects them statistically, not semantically). Memstead makes the opposite trade: every entity is a markdown file an agent deliberately authored against a pinned schema, every mutation a git commit — slower to fill, but auditable and portable as a versioned mem.

<!-- Sources: https://github.com/mem0ai/mem0 (README, v3 pipeline, benchmarks, licence), https://docs.mem0.ai and https://docs.mem0.ai/open-source/overview (storage defaults, deployment tiers), https://docs.mem0.ai/open-source/graph_memory/overview (graph-store removal, entity linking). Retrieved 2026-07-03. -->

### Zep (Graphiti)

[Graphiti](https://github.com/getzep/graphiti) is Zep's open-source engine (Apache-2.0): a temporal knowledge graph built by LLM extraction. Nodes are entities, edges are facts with bi-temporal validity windows; when reality changes, superseded facts are invalidated with a timestamp rather than deleted, and every derived fact traces back to its source "episode" (the raw ingested data). Developers define custom entity and edge types as Pydantic models — Graphiti genuinely has typed entities and typed edges, and it would be a strawman to pretend otherwise. [Zep](https://www.getzep.com) is the hosted platform on top: thread ingestion, context blocks optimised for LLM consumption, sub-200ms retrieval (vendor figure). The graph lives in a graph database — Neo4j, FalkorDB, or Amazon Neptune.

What it is genuinely good at: temporal reasoning over changing facts, automatic graph construction from message streams and business data without authoring effort, and provenance from every fact back to its source episode. If the question is "what did we believe about this customer in March, and when did that change?", this is the strongest tool in the category.

How Memstead differs: the direction of writes and the ownership of storage. Graphiti's types guide what the extraction pipeline produces; they are not validation of a deliberate authored write — the graph is what the LLM read into your data, reviewable through queries but not through files. Memstead's graph is markdown in a git repository the user owns: every entity a file you can open, every mutation a commit with structured provenance, the whole mem exportable as one archive with its pinned schema embedded, so it stays self-describing outside any running service. Graphiti answers "what was true when"; a mem is an artefact you curate, review in diffs, and ship.

<!-- Sources: https://github.com/getzep/graphiti (README: bi-temporal model, episodes, Pydantic types, backends, licence), https://help.getzep.com/concepts (platform surface, fact invalidation, custom types, retrieval latency). Retrieved 2026-07-03. -->

### Letta (MemGPT)

[Letta](https://docs.letta.com) grew out of the MemGPT paper: agents that manage their own context window. The platform's memory model has three tiers — memory blocks (labelled, character-limited sections of the context window, shareable between agents, editable via API), archival memory (a semantically searchable store queried on demand), and sleep-time agents that reorganise memory asynchronously while the primary agent is idle. Its newer Agent SDK goes further with **MemFS**: a git-backed markdown memory filesystem (Letta's term: "context repository") — YAML-frontmatter markdown files, a `system/` directory always loaded into the prompt, everything else pulled in when relevant, the agent autonomously creating and reorganising files, every memory edit committed to a git repository (local repos when run against local backends).

What it is genuinely good at: context-window engineering as a first-class discipline — self-editing memory, block sharing across agents, asynchronous consolidation. MemFS also deserves plain acknowledgement: it is markdown in git, the same substrate bet Memstead makes, and evidence the category is converging on it.

How Memstead differs: the layer above the substrate, and who the memory belongs to. MemFS files are freeform prose an agent organises for its own recall — no schema, no typed entities, no relationship vocabulary, no validation on writes; the unit of memory is an agent. A mem is a typed model of a *subject*, independent of any one agent: the schema is pinned, non-conforming writes are refused with typed errors, graph edges come from a controlled vocabulary, and the package travels across tools through MCP rather than living inside one agent runtime.

<!-- Sources: https://docs.letta.com/letta-agent/memory (MemFS, git-backed, system/ directory, local repos), https://docs.letta.com/concepts/letta (V1→Agent SDK transition, export/import), https://docs.letta.com/guides/agents/archival-memory/ and https://docs.letta.com/guides/agents/architectures/sleeptime/ (memory tiers, sleep-time agents). Retrieved 2026-07-03. -->

### basic-memory

[basic-memory](https://github.com/basicmachines-co/basic-memory) (Basic Machines) is the closest neighbour in the category, and any comparison that pretends otherwise is dishonest: it is local-first plain markdown on disk, exposed to agents through an MCP server, with two-way Obsidian compatibility and an optional paid cloud sync. Notes carry YAML frontmatter (title, type, tags), "observations" (categorised facts in a bracket notation), and "relations" (wikilinks with relation labels such as `relates_to [[Target]]`) that together form a traversable knowledge graph, indexed in a local database that a `doctor` command keeps consistent with the files. It even ships schema tooling — `schema_infer`, `schema_validate`, `schema_diff` — that infers the implicit structure of a knowledge base and audits notes against it.

What it is genuinely good at: exactly its tagline — persistent, human-readable memory across any MCP client with near-zero infrastructure. The files are yours, readable in any editor, and the observation/relation grammar gives an LLM real graph structure to build context from.

How Memstead differs: the direction of enforcement, and then git. basic-memory's structure is conventional — the grammar is a convention the model is asked to follow, relation types are freeform labels, `write_note` accepts whatever the model produces, and the schema tools audit drift after the fact. Memstead validates at write time: a mem pins one schema, and unknown sections, metadata fields, enum values, and relation types are refused with typed errors before they enter the graph — conformance by construction rather than by later audit. Second, versioning: basic-memory leaves git to the user, while Memstead's engine commits every mutation itself with structured provenance trailers, so history reconstruction is a query rather than an archaeology project. Third, packaging: a mem exports as a versioned archive with its schema embedded, installable elsewhere as a self-describing unit.

<!-- Sources: https://github.com/basicmachines-co/basic-memory (README: storage model, note grammar, Obsidian, cloud tier), https://docs.basicmemory.com/reference/mcp-tools-reference (tool list, write_note/edit_note semantics, schema tools advisory-only, no git integration documented). Retrieved 2026-07-03. -->

### Claude Code built-in memory (CLAUDE.md / auto memory)

[Claude Code](https://code.claude.com/docs/en/memory) carries two memory mechanisms. CLAUDE.md files are human-written instructions at managed, user, project, and local scope, concatenated into context at the start of every session, with `@path` imports and path-scoped rules under `.claude/rules/`. Auto memory is the inverse: notes Claude writes for itself, per repository, in a plain-markdown directory — a `MEMORY.md` index loaded up to its first 200 lines or 25KB, plus topic files read on demand. Both are files the user can open, edit, and delete, and the docs are explicit that both are "context, not enforced configuration".

What it is genuinely good at: zero infrastructure and honest scope. The instructions/learnings split is clean, everything is auditable markdown, and for per-repository working knowledge it is very hard to beat on simplicity. Its docs are also refreshingly frank about the limits — files past ~200 lines reduce adherence, so the guidance is to keep memory small.

How Memstead differs: this is the approach Memstead's own VISION names as its origin — plain CLAUDE.md worked, and did not scale. The memory is flat prose: no graph, no types, no validation, no relationships, and it is machine-local rather than portable. The scaling strategy is to stay small. Memstead is the structured continuation: when working knowledge outgrows a flat file, it becomes a typed, schema-validated graph that an agent queries piece by piece instead of front-loading into every session — and that graph travels, as a mem, to other machines and other tools.

<!-- Source: https://code.claude.com/docs/en/memory (full page: CLAUDE.md scopes and loading, rules, auto memory storage and load limits, "context, not enforced configuration"). Retrieved 2026-07-03. -->

## Versioned, portable artefacts

- **OCI / [Docker images](https://opencontainers.org)** — the contemporary standard for portable, content-addressed software artefacts. Memstead's `.mem` mem export-and-publish model conceptually parallels OCI's image lifecycle, though `.mem` is far simpler.
- **[Nix](https://nixos.org)** / **[Guix](https://guix.gnu.org)** — content-addressed reproducible-build systems. Memstead mem hashes use a similar idea at a much smaller scale.
- **[Helm](https://helm.sh)** charts, **[crates.io](https://crates.io)**, **[npm](https://www.npmjs.com)**, **[PyPI](https://pypi.org)** — versioned package registries. memstead.io follows this pattern for mem publishing.

## Rust ecosystem patterns

- **Open-core projects in Rust** — **[swc](https://swc.rs)**, **[Polars](https://www.pola.rs)**, **[Pydantic-core](https://github.com/pydantic/pydantic-core)**, **[Lightning CSS](https://lightningcss.dev)**, **[Tantivy](https://github.com/quickwit-oss/tantivy)**, **[Tauri](https://tauri.app)**. All apply the "Rust engine + language-specific FFI wrappers" architecture Memstead uses. None invented the pattern; they each refined it for their domain.
- **[UniFFI](https://mozilla.github.io/uniffi-rs/)** (Mozilla) — generates language bindings (Swift, Kotlin, Python, Ruby, Go) from a Rust UDL contract. memstead-swift uses this verbatim.
- **[axum](https://github.com/tokio-rs/axum)** + **[tower](https://github.com/tower-rs/tower)** — the standard Rust web-server pattern. memstead-registry follows it conventionally.
- **[gix](https://github.com/Byron/gitoxide)** — pure-Rust git implementation. Replaces libgit2/cgo dependency chains. Memstead's mem-repo backend uses it.
- **[clap](https://github.com/clap-rs/clap)** + **[clap-markdown](https://github.com/ConnorGray/clap-markdown)** — the standard derive-based CLI parser plus its Markdown docs renderer. memstead-cli is conventional usage; the docs site's CLI reference is mechanically generated.

## Cleanroom claim

The following components were originally written for Memstead without reference to source code from any other project, only to the prior art listed above:

- The schema vocabulary (`software`, `default`, plus the `alias_target_rel_type` mechanism for body wiki-link auto-relations)
- The MCP tool naming convention (`memstead_*`)
- The lean/full architecture split and the engine-as-only-git-consumer principle
- The probe skill (exploratory engine testing via agent reasoning, with protocol files)
- The Surface Parity Matrix as a first-class documentation artefact
- The mem-repo branch-per-mem layout
- The `.mem` archive format
- The xtask deterministic documentation generator

If any of these turn out to substantially resemble a prior work the author was unaware of, a credit-where-credit-is-due correction in this file is welcome. Open an issue or send a pull request with the reference.

## Why this document exists

Two reasons.

**Honesty.** Software builds on software. Pretending otherwise would misrepresent the work and disrespect the projects Memstead leans on. The list above acknowledges that lineage explicitly.

**Defense.** If a future dispute alleges that Memstead copied a specific design from a specific source, this document is the starting record showing what Memstead knowingly built on, when it did so, and what was original work. The git history of this file (and the git history of the repository as a whole) provides timestamps; the references above provide context. See [`SECURITY.md`](SECURITY.md) for the contact channel.

Updates to this file are welcomed via pull request — both to add prior art the author missed, and to correct attributions that are unclear.
