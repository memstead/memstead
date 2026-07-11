# Changelog

All notable changes to Memstead are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- The binding edit layer (`memstead-base::pipeline_edit`, reached via the
  UniFFI `add_projection` / `update_projection` methods) now carries the
  **full author-editable binding record** instead of the five
  projection-level fields: the `operations` block, `deny_paths`,
  `coverage_semantics`, `rules`, and `prune` are all authorable through
  the one update seam. Payloads are patches — an absent field is
  preserved (the preserve-operations guarantee, extended to every field),
  explicit `null` clears `intent` / `rules` / `prune` (rules were
  previously set-only), a present `operations` block replaces the block,
  and `version` stays engine-managed. Candidate records are validated
  against the medium-capability matrix before anything is written —
  e.g. declaring `sync` over a `web` medium refuses with the typed
  remedy-bearing message; refusals a stored record already produces
  never block an unrelated edit. Edits that would introduce a dangling
  facet/medium reference are refused; creates refuse duplicates and a
  missing `destination_mem`.
- MCP SDK (`rmcp`) upgraded 1.4 → 2.2, aligning with the MCP 2025-11-25
  spec types. The JSON wire format is unchanged — tool responses,
  envelopes, and `structuredContent` shapes are byte-identical (the
  wire-shape suite passes unmodified); the migration is Rust-API-level
  only (`Content` → `ContentBlock`).
- Crypto dependencies upgraded across the digest-0.11 ecosystem: `sha2`
  0.10 → 0.11 and `ed25519-dalek` 2 → 3 (key generation now seeds from
  `getrandom::SysRng`). Hash strings and signature bytes are unchanged —
  entity `_hash` values, ingest change-detection digests, and publish
  signatures stay byte-identical.

- **Claude Code plugin diet (0.5.0)** — the plugin is cut to its
  adapter core. `/verify` folds into `/sync` as its `--verify <binding>`
  read-only mode (one fewer skill, same capability); `/learn` shrinks to
  its non-obvious rules (variant enumeration, token-budgeted reads,
  third-party-origin distrust); the `check-realization` hook only spawns
  the CLI when `/setup` has recorded an installed binary (one file read
  instead of a doomed subprocess per edit); the entity-edit guard's
  fail-closed branch keys on the resolved mem-dir name instead of a
  hardcoded legacy `specs`; and the `/ingest` router now points at
  `/setup` when the `memstead` binary is missing instead of handing the
  agent an empty prompt.

- **UniFFI `Status` shrunk to its consumer-backed graph counts**
  (`entity_count`, `edge_count`) — a UDL break for the macOS app only.
  The rename-preserving superset fields (`stub_count`, `edge_types`,
  `community_count`, `mem_count`, `types_in_use`, `writable_mems`,
  `read_mems`) are gone: roster facts ride `mem_roster`, health facts
  ride `get_health` (the deferred data-source rework, macos-deferred-ui).
  CLI `memstead status` and every MCP surface are untouched.

### Fixed
- **The `#verified` baseline is now written.** `projection verify` records
  `<binding>/<facet>#verified = <observed facet head>` on every completed
  run, through the engine's sync-state writer — previously nothing wrote
  the token, so `status`/report rendered "never verified" forever and a
  `trigger: loop` verify was due on every `--all` pass. A failed or
  aborted run never advances the token; the recorded keys surface in the
  verify output (`verified_baseline` in `--json`).

### Removed
- The accidental `memstead-schema` release app: Cargo auto-detected the
  repo-internal `emit_json_schemas` dev tool as a binary, so cargo-dist
  shipped it — installer and Homebrew formula included — in v0.2.0 and
  v0.3.0. The crate is now dist-opted-out; the stray tap formula is
  removed separately.
- **Plugin hooks that served the dogfood topology or non-product
  concerns, not external installers**: the `mem-drift-notify` /
  `mem-drift-snapshot` pair plus their bespoke stdio MCP client (two
  engine boots per conversational turn to pre-announce an event the
  engine already handles via `MEM_RELOADED` / `HASH_MISMATCH`), and the
  `guard-secrets-read` / `guard-secrets-bash` pair (generic secrets
  hygiene with false positives — `.npmrc`, `.env.example` — that Claude
  Code's own `permissions.deny` rules cover declaratively).
- **Dev tooling out of the shipped plugin payload** (a marketplace
  install copies the whole plugin directory): the roster prose lint and
  the plugin architecture guard moved to `scripts/`; the format schemas
  moved to `docs/schemas/` with the frozen `memstead-plugin/v0` tree,
  the never-wired `versions.mjs` format-negotiation layer, and the
  `validate-live-workspace` walker deleted outright (pre-v1 migration is
  the engine's own Rust migrate path).

## [0.3.0] - 2026-07-11

The projection-pipeline release. This is a breaking pre-1.0 release: it
retires the four-primitive ingest config store in favour of a first-class,
versioned **binding**, adds **anchors** as the provenance primitive, and
replaces `memstead stats` with `memstead status`. It ships the binaries the
repo and docs already describe — the shipped Claude Code plugin's ingest
front door calls `memstead projection`, a command that did not exist in the
0.2.0 binaries.

### Added
- `memstead projection` — binding (projection-promotion) tooling. One
  versioned binding file per source→mem obligation replaces the
  `projections/` + `ingests/` store. Subcommands: `projection init`
  (scaffold a fresh v1 binding non-interactively), `projection brief` /
  `projection brief --all` (render the Markdown run-brief an agent
  consumes; `--all` selects the next due binding by round-robin + backoff),
  `projection advance` (record disposition-gated sync-baseline advances),
  `projection migrate` (promote both legacy declaration generations — the
  root-folder layout and the gen-2 four-primitive store — into v1
  bindings), and `projection enable <build|sync|verify>` (add a missing
  operation block).
- **Anchors** — the provenance primitive. `memstead create` and
  `memstead update` accept `--anchor` (and `anchors[]` via `--from`); the
  MCP `memstead_create` / `memstead_update` tools gain an optional
  `anchors[]` parameter on both server flavours. New read-only
  `memstead anchors <id>` lists an entity's anchors and composition, and
  `memstead anchors --artifact <path>` reverse-looks-up every entity whose
  anchor references a path. Anchor sidecars survive `.mem` archive export
  and canonical repack. `memstead_entity` surfaces `anchors` and
  `anchor_composition` as additive fields.
- `memstead status` — node/edge counts, schema distribution, and
  per-binding projection state.
- Typed `INVALID_ANCHOR` error with recovery details across the CLI and
  both MCP flavours.

### Changed
- `memstead status` **replaces** `memstead stats`. Health stays
  lint-focused; on the MCP surface the former stats data is folded into
  `memstead_health` (there is no MCP stats tool).
- Binding format **v1**: one versioned binding file carries `intent`,
  `source_facets`, `reference_mems`, `destination_mem`, `deny_paths`,
  `coverage_semantics`, `rules`, and `operations{build,sync,verify}`.
- The Claude Code plugin's anchors capability gate now keys on the first
  anchors-capable binary (`0.3.0`); a recorded pre-0.3.0 binary fails
  closed to the degraded (no-anchors) path rather than probing by error.

### Removed
- `memstead stats` — superseded by `memstead status`.

## [0.2.0] - 2026-07-04

This release ships the binaries the public documentation already
describes: `v0.1.0` was tagged 71 minutes before `memstead quickstart`
and `memstead schema new` landed, so the published 0.1.0 binaries were
missing the documented newcomer happy path.

### Added
- `memstead quickstart` and `memstead schema new` — the two-command cold start.
  One `quickstart` run creates the workspace, a mem pinned to the built-in
  `default` schema, a seed entity, and the MCP wiring for the agent(s) you pick
  (Claude Code, Codex, Cursor, Gemini CLI).
- CLI transport commands for git-branch workspaces: `fetch`, `pull`, `push`,
  `branch-reset`, and `remote-add`.
- `memstead mem set-description`.
- Docs site: narrative guides and the glossary page.

### Changed
- The build-flavour pair is named lean/full everywhere.
- Export resolves installed schemas on both storage backends.

### Fixed
- `branch_reset` accepts the full-ref branch form on the git-branch backend.
- The pipeline store refuses path-escaping mem/name values.
- Archive read paths enforce the validator's decompression caps.
- The entity loader survives parser panics (per-file isolation boundary).
- Folder-backend archive assembly resolves installed schemas on publish.
- Cold-start round-1 text fixes: `create --help` documents the `--relation`
  filesystem-mem limitation and the `--from` JSON `entity_type` field name;
  built-in schema texts no longer claim an open relationship vocabulary;
  `install.sh` states the `.ai`/`.io`/GitHub origin relationship.

## [0.1.0] - 2026-07-02

First tagged release, with pre-built binaries for macOS, Linux, and Windows
(shell installer at `https://memstead.io/install.sh` and the
`memstead/homebrew-memstead` Homebrew tap).

### Added
- Initial public release of the open engine: the schema layer, the in-memory
  store, the folder and git-branch storage backends, the `memstead` CLI, and the
  `memstead-mcp` MCP server.

[Unreleased]: https://github.com/memstead/memstead/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/memstead/memstead/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/memstead/memstead/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/memstead/memstead/releases/tag/v0.1.0
