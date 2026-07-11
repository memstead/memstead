# Changelog

All notable changes to Memstead are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
