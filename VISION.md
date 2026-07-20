# Memstead — Vision

Memstead is a schema-agnostic graph engine. A mem keeps a typed model of a chosen subject — its facts, plans, decisions, open questions, or any mix the schema allows. Knowledge graphs are one well-known modal slice; Memstead generalises across all of them.

AI agents are the primary consumers and structured markdown is the storage layer. Every entity is a typed file in git; every mutation goes through the engine's MCP tool surface; every schema and every mem travels as a portable, versioned package.

This document describes why the engine exists, what it bets on, and where it is going.

## Why this exists

In an AI-coding world, specifications capture the lion's share of the value. Code becomes commodity; the ability to *specify what should exist, why, and how it relates* becomes the differentiator. Spec-first development is the natural reaction.

It has so far failed in practice — not because specs are wrong, but because they rot the moment code starts evolving. Three parts to the problem:

1. **Format** — how should specs look so an LLM can actually use them?
2. **Integrity** — how do we stop them from going silently malformed or self-contradictory as many actors write to them?
3. **Freshness** — how do they stay synchronised with the reality they describe?

Memstead is the bet that all three are solvable with the same architecture.

## Core value proposition

Memstead does **not** claim that an agent reads, understands, or answers better because the knowledge is typed. The project's own controlled evaluation ([docs/proof/substrate/](docs/proof/substrate/)) put a schema-forced typed mem against equally-curated free-form notes and measured a signed answer-quality delta of ≈ −0.010 ± 0.006 — no measurable read-side advantage. The token saving that does show up traces to *curation*, which flat notes share, not to typing. That axis is closed on the project's own evidence, and nothing here should be read as reopening it.

What a typed graph gives that a folder of good markdown notes cannot:

- **Enforcement on write** — the schema rejects a malformed or off-vocabulary entity at mutation time, so the knowledge cannot go quietly wrong. A convention in a style guide is advisory; a schema is a gate.
- **Determinism** — traversal, community detection, and drift queries run over typed edges with no model in the query path. The same question returns the same answer, at no token cost, whether or not an LLM is present.
- **Accountability** — every mutation is a native git commit with structured provenance. Who wrote a claim, by which tool, in which session, is answerable from `git log` without parsing prose.
- **Ownership** — plain markdown in the user's own repository. No database to run, no vendor to leave, no export step.
- **Packaging** — a mem travels as a sealed, semver-versioned `.mem` that embeds its schema and its provenance, so it stays self-describing and auditable offline.

Three observations have repeated across early use:

- Knowledge gaps in coding agents close when web research is projected into specs the agent already has access to.
- Planning sessions that would otherwise drift between context windows preserve their structure when the planning artifact is a graph instead of a transcript.
- Teams that specify their workflows once produce specs their colleagues use rather than abandon.

The plain-`CLAUDE.md` approach worked but did not scale. Memstead is the scalable form — schema-validated, typed, portable, queryable.

## What Memstead is

A schema-agnostic, markdown-based graph engine. Specs and any other typed knowledge live as structured markdown in git, queryable and mutable through the engine's MCP tool surface.

It is not one product — it is an engine that powers different products:

- **Reimplementation tooling** — old codebase → specs (via projection) → new codebase, with explicit divergences. Specs as the migration bridge.
- **Company specification tools** — guided specification of workflows, processes, and institutional knowledge with AI assistance.
- **Smart notebooks** — structured graphs underneath, not flat notes.
- **Vibe-coding apps** — specs as the layer between intent and code.

The engine stays generic. Specific products go to market separately.

## Design bets

### Markdown + git as the foundation

Human-readable, git-diffable, no migration tooling, no vendor lock-in, no database to run. Knowledge lives in files the user owns. Workspaces are directories; mems are typed sub-directories; everything is a regular file behind the API.

### Three dimensions: state, structure, narrative

Information in a mem lives along three orthogonal dimensions, each with its own storage shape, mutation semantics, and read pattern.

**State** — entity content. The current, timeless facts about each thing: a decision's choice and consequences, a spec's purpose, a contract's wire shape. Read via `memstead_entity`; mutated via section-level operations.

**Structure** — graph edges. How entities relate: PART_OF, IMPLEMENTS, SUPERSEDES, CHOSEN, REJECTED, and the rest of the typed vocabulary. Read via the relations payload on entity reads and via traversal; mutated via `memstead_relate`.

**Narrative** — change rationale and git history. Why each change happened, with `[[id]]` hypertext linking to other entities. Read via per-entity history exposure and across-session history queries; recorded as commit metadata accompanying every mutation.

Each piece of information belongs to exactly one dimension. An option's intrinsic cons live in state; the rationale for rejecting that option lives in the narrative of the relate-call that established the REJECTED edge; the rejection itself is structure. Authoring discipline follows directly: temporal narrative does not leak into entity content because there is a designated dimension for it. The graph stays navigable by agents because a query in one dimension does not need to interpret content from another.

The engine's MCP surface and schema rules align with this separation. State is what schemas constrain; structure is what edge vocabularies enforce; narrative is what commit provenance carries. Adding capability to the system is a question of which dimension it strengthens — and a feature that touches more than one dimension is usually two features.

### Commit provenance for history reconstruction

Every mem mutation lands as a native git commit with structured provenance. Author, actor, tool, and client are recorded as message trailers; external disk edits get their own provenance category. A future agent reading `git log` inside a mem can filter reliably — *"the last five edits on this entity all came from the ingest skill"*, *"these bytes appeared outside any agent path on this date"* — without parsing free prose. No PII enters the mem.

### Schema-agnostic engine, schemas as packages

The engine has no hardcoded entity types. Schemas — a `schema.yaml` manifest plus `types/*.yaml` definitions — are first-class, semver-versioned packages. A mem pins its schema by name; a workspace can share schemas across many mems; published mem archives embed the pinned schema so they remain self-describing on import. New schemas — for product requirements, customer research, legal compliance, anything — are author-only work, never engine work.

This is the foundation the memstead.io registry is being built toward — domain-specific schemas published, browsed, and installed the way packages are: an invitation to the first publishers, not yet an achieved marketplace.

### LLM-first engine, human-first app

The engine, the schema system, the relationship vocabulary, and the MCP interface are designed for LLM agents as the primary author and consumer. The macOS app and human-facing projections (specs to documentation, mem summaries to overviews) are designed for humans. Two layers, two consumer profiles, one shared substrate.

### Two modes of truth

- **Code-bound mems:** code is the source of truth, specs are an abstraction layer that helps LLMs reason about the codebase. Verify measures where the specs have diverged from the code they describe, and sync writes the corrections that keep the spec layer current.
- **Knowledge-only mems:** the spec *is* the source of truth. There is no other hard reality. Direct authoring; freshness is measured against authorial commitment, not external code.

The engine handles both with the same primitives — what changes is which features carry weight.

### Mem scaling: many small, federated

A mem is sized for one coherent subject — by design on the order of 1,000–5,000 entities. Beyond ~10,000, the subject discipline usually breaks: the "subject" has become two or three subjects in one bucket. The architectural answer at higher scale is not a bigger mem but **more mems connected by cross-mem edges**.

Two tiers fall out of this:

- **Working Mem** — folder or git-branch backed, sized for 1k–5k entities, full read/write, agents traverse the whole graph, communities, mutations through MCP. This is what the engine ships today.
- **Indexed Mem** *(planned, not built)* — read-only at million-entity scale. Agents don't traverse; they query an index. The index is a **derived projection over finished mems, never a parallel source of truth**: markdown+git stays authoritative, the index is rebuilt from it, and drift is one-directional — rebuild forward, never sync back. It answers questions; it never originates state. Because it is derived, its backing engine (an embedded graph store, Neo4j, a search index) is an interchangeable implementation detail — no lock-in, since truth never lives there. Cross-mem edges from working mems point into it; full-graph operations (community-detect, full-traverse) are not offered. The use case is "the FDA's structured drug database" or "every paper in PubMed" — knowledge an agent needs to query, not navigate.

The federation pattern follows: a workspace mounts dozens of small working mems plus a handful of indexed mems; the memstead.io registry indexes published mems across authorities. The engine's `MemBackend` trait makes new backend kinds (indexed-archive, remote-fetch, etc.) additive — no engine surgery to add a new tier.

This is what "engine is generic, apps come later" means at scale — as a **bet, not an achieved capability**. The engine ships the Working Mem tier today; the Indexed Mem tier the large end needs is planned and unbuilt, and no federated graph at that size has been run. The wager is that one engine can drive a personal planning mem (50 entities) and, once the indexed tier exists, a federated research-knowledge graph in the millions of entities across hundreds of mems — with the backend behind each mount changing, not the engine's shape. Knowledge-at-scale is an open frontier for this project, not a solved one.

### What Memstead refuses to become

Three architectural lines are drawn explicitly. Convergence on neighbouring systems' implementation patterns is fine where they fit; convergence across these lines dilutes the original insight and is not reversible without losing what makes Memstead distinctive.

- **No probabilistic reasoning layer.** Confidence and belief modeling are well-trodden territory in academic knowledge graphs. Adding them shifts Memstead from "structured truth with explicit revision" toward "fuzzy inference engine." The optional `confidence` metadata field stays a tag, never a probabilistic substrate.
- **No swap-out of markdown as primary storage.** Treating markdown + git as one storage option among several — with RDF or property-graph export driving interop — dilutes the bet. Other systems can learn to read markdown; markdown does not need to read like other systems.
- **No separate database backend as a source of truth.** Git's performance limits will eventually surface. The answer is to push git further (sharding, partial-load, lazy reading) rather than introduce a parallel *authoritative* store. The line: a store that is **written to and kept coherent with the files** is the hybrid that rots — refused. A **derived index, read-only and rebuilt from the files** (see the Indexed Mem tier) is not — it holds no truth git doesn't already hold, so it can be discarded and regenerated at will. Two authoritative stores in one workspace are two systems to keep coherent; an index is downstream of one.

Each line is a decision against a pull that would otherwise feel reasonable. Naming them explicitly is what keeps the architecture coherent over time.

### Form over universal fit

The engine has a deliberate form: typed entities in markdown, agent-navigated, git-versioned, mem-scale in the thousands. Some domains fit this form well — codebase architecture, decision records, planning graphs, worldbuilding, PKM. Others fit badly: domains that need probabilistic reasoning, real-time collaboration, web-scale corpora, OLAP-style aggregation, or formal inference over graph topology.

Refused domains are not gaps. Every accommodation that loosens the form to fit a new domain dilutes the guarantees the form gives to the domains it already serves. The honest answer to "could the engine serve X?" is sometimes "no, and that is the design."

This pairs with the *constraints become capabilities* principle: that principle says every restriction inside the form must unlock a concrete capability; this one says every claim of fit against the form must be earned. Together they keep the engine narrow and sharp.

## The freshness problem (central unsolved challenge)

Keeping specs in sync with reality is the hardest problem the engine has to solve. Current approaches, all complementary:

- **Sync + verify workflows** — verify measures the specs against the source they describe; sync, the maintenance writer fed by change slices and verify findings, brings the specs back into agreement.
- **Projection loops** — rebuild specs from sources periodically.
- **Drift detection** — flag entities that reference parts of reality which have moved since the entity was last updated.
- **Section-level provenance** — preserve who authored what at section granularity so LLM-invented content stays distinguishable from human-endorsed claims, and stale assumptions surface against fresh decisions.

None of these alone solves freshness. Together they bound the rot.

In the spec-first / spec-anchored / spec-as-source taxonomy that Birgitta Böckeler and Thoughtworks named (martinfowler.com, 2025-10-15), the market adopted spec-*first* — specs as ephemeral per-feature fuel — and hit exactly this rot. Memstead targets the **spec-anchored** slot the wave left structurally empty: a persistent, typed, queryable knowledge layer whose freshness is a first-class mechanism rather than a manual chore.

The project itself is the test case. Memstead must keep its own specs synchronised while it is being developed. If it cannot, the system has not solved the problem.

## Competitive positioning

Memstead's category is agent memory: the systems that give AI agents durable state — mem0, Zep/Graphiti, Letta, basic-memory, and the memory built into coding agents themselves. Most of the category *derives* memory: an extraction pipeline distils conversations into a vector or graph store and optimises recall. Memstead sits at the *authored* end: agents and humans deliberately write typed entities into a schema-validated graph, stored as markdown in a git repository the user owns. Derived memory optimises what an agent recalls; an authored mem is a model of a subject — reviewable in diffs, portable as a versioned package, and correct by construction rather than by extraction. [PRIOR_ART.md](PRIOR_ART.md#the-2026-agent-memory-category) carries the tool-by-tool comparison.

Obsidian and Notion are adjacent tools with a different job, not competitors. Obsidian shares the substrate — markdown, wikilinks, a graph view — but is a thinking-and-writing environment for humans; the shared substrate is why an Obsidian user's mental model transfers to Memstead in minutes, not a sign of product overlap. Notion demonstrated typed structured records for human teams inside a closed SaaS; Memstead takes the typed-record idea and moves it to open files, write-time schema validation, and an agent-first MCP surface. Both are excellent at what they do; neither validates writes, types relationships, or offers agents the graph as a first-class query surface — because that was never their job.

Differentiators: schema-driven structure, typed and validated relationships, MCP-native AI access, drift detection at the data layer, community clustering for navigation, and git-native versioning with structured provenance that travels inside the published package — each entity's authoring rationale ships in the sealed `.mem`, so an installer can ask "why should I trust this entity?" offline, with no running engine.

## Open-core go-to-market

The whole engine — Rust crates, the MCP server, the `.mem` format/protocol and publish/install client — is open-source under dual MIT OR Apache-2.0. The launch posture is **adoption-first**: like npm, the win is being free developer infrastructure that gets adopted; revenue is a later layer, not a launch gate. A commercial layer — a human oversight surface and an org/IP-retention tier — sits on top:

- The **macOS app** — a human control-and-oversight surface over the engine, a *free showcase* at launch rather than a monetised flagship.
- A **private/enterprise registry** — companies managing their own mems on a hosted or self-hosted server (the npm-Enterprise model, sellable because the registry server stays private).
- **Team features** — collaboration, shared mems with edit safeguards, organisation-level authority.

Open-source serves three purposes:

1. **Distribution** — visibility in the MCP ecosystem and the Rust community.
2. **Trust** — enterprises evaluate open-source tools, not opaque proprietary engines from unknown developers.
3. **Ecosystem** — the aim is that community schema contributions compound into network effects over time; that is a bet on the future, not a network that exists today.

## Long-term vision: Memstead as a web standard

> **Horizon note:** everything from here to [Known risks](#known-risks)
> is speculative long-horizon direction, not roadmap. Nothing below is
> implemented, scheduled, or promised; it exists to show where the
> design bets *could* lead.

Today, website knowledge is trapped in unstructured HTML. A university has hundreds of pages about research projects, curricula, and faculty expertise — but no AI agent can navigate it systematically. A company documents its APIs, processes, and architecture across wikis and docs — all opaque to AI.

Memstead's authority model opens a path: any domain registers as an authority on memstead.io and publishes structured knowledge graphs under its own scope. The natural extension is that domains *host their own mems* while memstead.io serves as a federated index — the same relationship GitHub repos have with npm, or websites with search engines.

```
https://mit.edu/.well-known/memstead-authority.json     "we are a memstead authority"
https://mit.edu/mems/ml-curriculum.mem             self-hosted mem
https://memstead.io/v/mit.edu:cs-dept/ml-curriculum   index entry, links to mit.edu
```

`.well-known/memstead-authority.json` becomes a discoverability signal: *this domain has structured, machine-readable knowledge — here is the entry point.* AI agents discovering a domain check for that file and immediately access a navigable knowledge graph instead of scraping HTML.

**Example — a university.** A projection runs against the university website periodically. It extracts structural, timeless knowledge — departments, research areas, degree programmes, faculty expertise, institutional relationships — not events, news, or deadlines. Current information stays on the website; the mem is a durable understanding of what the university *is*.

The projected graph is published as one or more `.mem` files on the university's own server. The authority file lists them. memstead.io indexes them but holds no copy — downloads go directly to `mit.edu`. When mems are added or removed, the registry updates.

An AI agent researching *"machine-learning programmes in Europe"* hits memstead.io, finds `mit.edu:cs-dept/ml-curriculum`, downloads from `mit.edu`, and gets a graph the publishing authority validated and signed — a stated, attributable claim rather than whatever the agent managed to parse out of the HTML. The gain is provenance and publisher accountability, not a reading advantage.

This turns Memstead from "a registry for sharing knowledge graphs" into "an open standard for how websites make their knowledge accessible to AI." The engine stays the same; the surface area grows from "developers sharing mems" to "any organisation publishing structured knowledge."

## Known risks

- **Distraction** — a large surface area: an engine, a registry, an app, several adapter clients. The most legitimate risk; mitigated by ruthless engine/app separation.
- **Simplicity** — the problem may have a much simpler solution; mitigated by treating each architectural commitment as a hypothesis to be falsified.
- **Competition** — others may build better; mitigated by building the right thing rather than the first thing.
- **Market** — the abstraction may be ahead of demand; mitigated by validating with real use cases at each step.

## Where to look next

- Working rules for contributors and AI agents in this repository: [AGENTS.md](AGENTS.md).
- Engine source: the crates at [crates/](crates/) in this repository.
