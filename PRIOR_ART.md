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
- **[Logseq](https://logseq.com)**, **[Foam](https://foambubble.github.io)**, **[Quartz](https://quartz.jzhao.xyz)** — all use git as the persistence layer for personal Markdown knowledge bases. Memstead extends this with first-class git-branch storage (the pro mem-repo) for mem-level versioning beyond outer-repo commits.
- **[git-annex](https://git-annex.branchable.com)** — the precedent for using git's content-addressed model for non-source data. Memstead's optimistic-locking model (the `_hash` field on entities) borrows the spirit of content addressing.

## Schema validation

- **[JSON Schema](https://json-schema.org)** ([RFC 8259](https://datatracker.ietf.org/doc/html/rfc8259) for the JSON substrate, draft 2020-12 for the schema dialect) — Memstead uses [schemars](https://github.com/GREsau/schemars) to derive JSON Schema from Rust types for MCP tool parameter schemas. The schema-validates-the-data pattern itself is decades old.
- **[OpenAPI](https://www.openapis.org)** / **[Swagger](https://swagger.io)** — model for declaratively describing HTTP APIs so machine readers can consume them. Memstead uses [utoipa](https://github.com/juhaku/utoipa) for the registry HTTP surface's OpenAPI document.
- **[Protocol Buffers](https://protobuf.dev)** / **[Cap'n Proto](https://capnproto.org)** — schema-first system design. Memstead's schema-first stance for entities echoes this.

## AI-agent integration

- **[Model Context Protocol (MCP)](https://modelcontextprotocol.io)** — Anthropic's protocol for AI agents to call structured tools over stdio (and HTTP). The MCP tool surface, the `read_only_hint` / `idempotent_hint` annotation vocabulary, the JSON-RPC envelope, and the tool-discovery pattern in Memstead are direct uses of the MCP specification — no invention by Memstead beyond which tools to expose. The official Rust SDK [rmcp](https://github.com/modelcontextprotocol/rust-sdk) is the implementation foundation.
- **[Function Calling](https://platform.openai.com/docs/guides/function-calling)** (OpenAI), tool use (Anthropic), built-in tool support (Gemini) — pre-MCP attempts at the same problem. Memstead's typed-tool-surface design fits the family.
- **[LangChain](https://www.langchain.com)** / **[CrewAI](https://www.crewai.com)** / **[AutoGen](https://github.com/microsoft/autogen)** — agent frameworks. Memstead is not bound to any of them; the MCP surface is the integration point.

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
- The basis/pro architecture split and the engine-as-only-git-consumer principle
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
