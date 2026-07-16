# Changelog

All notable changes to Memstead are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- `/sync --all` under a recurring loop now **ends the loop on quiescence**:
  a second consecutive nothing-due rotation means the catch-up job is done —
  the skill cancels the schedule driving it and reports quiescence, instead
  of ticking no-ops forever. A standing watch is a deliberate restart at a
  slower cadence. Matches the operator mental model "run until the graph is
  back in sync, then stop".

### Added
- **Folder mems join cross-process drift detection.** The filesystem
  backend's `current_head` now derives a drift cursor from its
  append-only changelog (the last line's RFC3339-millis `ts` — the same
  dialect `folder_changes_since` accepts), so a sibling process's commit
  to a folder mem triggers the same reload-before-operation /
  `MEM_RELOADED` / `MemChangedEvent` machinery git-branch mems always
  had. Self-write bookkeeping records the backend's own probe answer
  (`record_self_write` probes once post-commit), so an engine's own
  writes never masquerade as sibling drift on any backend. Folder mems
  with no changelog keep the historical no-drift-signal behavior.
- **Bulk per-mem topology projection: `Engine::mem_topology`.** One call
  returns `{nodes, edges, communities}` for a mem — every entity (id,
  title, type, global Louvain cluster id, stub flag), every relationship
  edge sourced in the mem with cross-mem targets marked
  (`target_in_mem: false`, reported at the source mem only, so composing
  all mems yields each edge exactly once), and the mem's community roster
  from the workspace-global partition. Coordinate-free and unpaged by
  contract. Unknown mems refuse with `UNKNOWN_MEM`. Hoists the projection
  UI consumers previously re-derived per surface (serve's private variant,
  the macOS app's paged N+1 assembly).
- **`Actor::App` provenance category (`Actor: app` trailer / changelog
  value).** Human-driven application embedders — the macOS app, the node
  app's HTTP surface, any future UI consumer — get their own caller
  category, distinct from `agent` (LLM over MCP) and `cli`. The paired
  `ClientId` names which software spoke and derives the commit author
  (`<client>@memstead.io`), exactly as agent/cli identities do; `external`
  keeps meaning out-of-band writes the engine discovered rather than
  performed. Additive: existing trailers, readers, and wire values are
  unchanged.
- **`create_mem` seeds commit with the caller's own provenance.**
  `MemCreateParams` gains `actor` + `client`; each transport passes its
  category (MCP `agent`, CLI `cli`, UniFFI/HTTP embedders `app`). The
  previous hardcoded `Actor::Agent` misattributed every non-MCP mem
  creation — including the macOS app's — as an agent write.
- **Schema-level `system_context` in the full `memstead_schema` payload.**
  A schema manifest's `system_message` — the author's voice/posture prose —
  was previously unreachable from the agent surface (its only consumer was
  the `memstead type` CLI markdown). `verbosity: "full"` now serves it as
  top-level `system_context`, wire-named to match the per-type key; schemas
  without the field render unchanged (key omitted). Third-party schemas
  remain structural-only.
- **Explicit `--storage folder|git-branch` override on `memstead mem init` /
  `create_mem`**, enabling mixed-backend workspaces (folder mems beside
  git-branch mems). Omitted, the workspace-shape heuristic is unchanged;
  `folder` forces a plain-markdown folder mem at the mem's location even
  inside a mem-repo workspace — its files sit visibly in the outer tree,
  and the outer-repo `.gitignore` append is skipped; `git-branch` refuses
  with a typed `INVALID_INPUT` in a workspace without `mem-repo/.git/`.
  The mount loader and runtime already dispatched per-mount — only the
  creation surface was missing. MCP and UniFFI wire shapes are unchanged.
- **Folder mems skip `README.md` at load.** A folder mem living visibly
  in a repository tree carries a human-facing README beside its entity
  files; the loader no longer parses it as an entity (quickstart already
  tolerated README-grade files at init — the load side now matches).
- **`memstead export` skips `README.md` too.** The export walker still
  collected it, so exporting a folder mem that carries a README failed
  strict validation with `missing frontmatter at README.md` — what load
  skips, export now skips as well.
- **`WorkspaceConfig` preserves unknown fields instead of refusing.** The
  engine's own runtime machinery writes fields the workspace-shape config
  struct did not model (`syncState` from the projection sync baseline,
  `writeGuidance`), so exporting any projection-maintained folder mem
  failed with `workspace config malformed: unknown field syncState` — and
  a rewrite would have dropped the fields. Unknown fields now flow through
  a flattened extra map and survive read-modify-write round-trips.
- **`--detach-incoming` on `memstead mem delete` — the mem-replacement
  affordance.** Deleting a mem that other Write-Mems still link into
  normally refuses `MEM_HAS_INCOMING_REFS`; with the flag, the delete
  proceeds, the referrers' files stay untouched, their edges degrade to
  unresolved stubs, and a later same-name `memstead mem init` re-adopts
  them — the intended flow when re-homing a mem (backend or location
  change) under a stable name. The response lists every detached referrer
  (`detached_referrers`) so re-adoption can be verified. CLI-only; MCP and
  UniFFI wire shapes are unchanged.
- **`software@0.1.0` declares its outbound knowledge-side cross-mem
  vocabulary, additively.** Two new `cross_mem_relationships` blocks let a
  software mem's entities anchor into their knowledge-side companions:
  `engineering` (REFERENCES / MOTIVATED_BY / DERIVED_FROM / VALIDATES) and
  `project` (REFERENCES / MOTIVATED_BY / DEPENDS_ON / IMPLEMENTS /
  SUPERSEDES / OWNS, OWNS staying actor-sourced) — census-driven from live
  paired-mem content. Intra-mem vocabulary, types, and every existing
  definition are untouched.
- **`project@0.1.0` gains the knowledge cluster — `decision` and `memo`,
  additively.** Field shapes are structurally identical to the `software@` /
  `engineering@` namesakes (decisions and memos migrate between the three
  schemas with metadata verbatim). `principle` additively gains an optional
  `justification` section and optional `authority`/`universality`
  engineering-lineage fields — no existing type, section, or field changes
  shape. The relationship vocabulary gains DERIVED_FROM / SPECIALIZES /
  GENERALIZES / DEFINES; the cross-mem vocabulary widens REFERENCES sources
  to the knowledge types and adds GOVERNS / MOTIVATED_BY / MOTIVATES /
  CONSTRAINS / DEFINES toward software mems plus a new `engineering` block.
  The `engineering@0.1.0` builtin gains its own outbound cross-mem block
  toward software mems (REFERENCES / GOVERNS / MOTIVATED_BY / MOTIVATES /
  IMPLEMENTS / CONSTRAINS) — census-driven from live standing-knowledge
  content.
- **New builtin schema `engineering@0.1.0` — standing engineering
  knowledge.** The knowledge-only counterpart of `software@0.1.0`: three
  types (`decision`, `principle`, `memo`) answering WHY the system is the
  way it is, with field shapes identical to their `software@0.1.0`
  namesakes so entities migrate between the two schemas with metadata
  intact. Current-state types are deliberately absent — a `spec` in a mem
  pinned to this schema refuses `UNKNOWN_ENTITY_TYPE`, making the
  knowledge/system-model class boundary a write-time gate. Census-driven
  strict relationship vocabulary (structural, reasoning, lifecycle, rule,
  abstraction, evidence groups); body wiki-links alias-emit `REFERENCES`.
  `software@0.1.0` is untouched.
- **Out-of-root folder mounts with portable anchoring: `--location` on
  `memstead mem init`.** A folder mem can now live at any path a config can
  express — including outside the workspace root (`--location
  ../public/engineering`, the monorepo/submodule case). The mount record
  keeps the caller's *expressed* form: a relative location serialises into
  `mounts.json` as that relative path, so a clone of the whole tree to a
  different absolute prefix still resolves the mount; an absolute location
  stays absolute (machine-pinned by expression). The location's basename
  must match the mem name's last segment (existing invariant); agent-mode
  creates outside the workspace root still refuse with
  `MEM_PATH_NOT_ALLOWED` / `outside_workspace` — out-of-root placement is
  operator-mode only. MCP and UniFFI wire shapes are unchanged.
- **Prepared-content hashing, hash backfill, and deterministic drift
  adjudication.** Anchor observation on a `path`-medium mem now computes
  the **prepared-content hash** of each present hash-bearing (`anchored` /
  `derived`) `file`/`span` artifact — SHA-256 (house 16-hex form) over a
  minimal canonical form (BOM stripped, CRLF/CR → LF, trailing newlines
  trimmed; binary bytes hash raw) — so a recorded hash adjudicates
  `resolves` / `drifted` (stable medium) / `recheck` (unstable medium)
  deterministically, with no LLM sampling on the hash leg. On first
  observation of a **hash-less** hash-bearing anchor whose artifact
  resolves, `projection verify` records the computed hash onto the anchor
  in the engine-owned anchors sidecar (a completed-run bookkeeping write,
  like the `#verified` baseline — never entity content), reported as
  `hash_backfilled` in the CLI output; the backfill is idempotent and the
  tier-3 recheck queue for such anchors drains instead of re-queueing
  forever. Class semantics hold: `authored` / `informed-by` anchors never
  gain hashes and never adjudicate `drifted`; a `tree`-grain anchor has no
  prepared form this cycle and still resolves `recheck`. Anchor-less mems
  are unaffected (no hashes are computed where no hash-bearing anchor
  exists).
- **`projection verify --full` — the complete measurement.** Walks the
  entire enumerable source `S(D)` (the rotating sample scheduler is
  bypassed and its state untouched), treats the per-run adjudication cap
  as unlimited, and performs the prepared-hash backfill, so the tier-1
  report's coverage and accuracy figures are computed over everything —
  the output leads with the full-measurement statement and carries no
  sampling or truncation caveat, and the JSON `full_resync` decision is
  `forced`. A facet over a non-enumerable medium refuses the whole run
  with the typed `PROJECTION_CAPABILITY_UNSUPPORTED` error instead of
  rendering a fabricated-complete report. Without the flag, the
  capped/sampled loop economics are byte-compatible with before.
- **`/sync --inventory <binding>` — the full stock-take as a sync mode.**
  The plugin's sync skill gains the inventory operation's skill leg: run
  the complete measurement (`projection verify --full`), then repair in
  passes off the rendered sync brief (mutations via the normal MCP
  surface, dispositions via `projection advance`) with a re-verify after
  each, until the brief reports nothing to sync and the re-verify is
  clean or every remaining finding carries a disposition — closing with
  the fidelity report, verdict first. Termination is a hard skill rule:
  the open work (open findings plus artifacts awaiting disposition) must
  strictly shrink every pass; a pass that shrinks nothing ends the run
  with an honest "did not converge" report naming the stuck items, never
  a silent loop. The skill keeps no state of its own — the engine's
  recorded dispositions are the resume point — and engine refusals (sync
  not enabled, non-enumerable medium) are relayed with their remedies,
  never pre-checked or worked around. The default (non-inventory) loop
  path is asserted untouched: renderer-level tests lock the sync brief's
  block sequence and sweep every build/verify/sync brief shape for
  inventory machinery.

### Fixed
- **Verify findings survive source-head movement.** The findings store now
  keys on the binding's `hash(D)` alone; the `source_head` a finding was
  observed at rides as metadata on the finding. Sync briefs present all
  open findings regardless of recorded head, so an open finding keeps
  appearing after the source advances — previously the `(hash(D),
  source_head)` key made every head move hide the open findings from all
  subsequent briefs (a campaign-confirmed leak: an orphan finding hid from
  4+ consecutive briefs). Verify merges each pass: re-observed targets take
  the pass's outcome (clean closes, a cap deferral never downgrades a
  prior `drifted`/`wrong` verdict), unobserved-but-still-open findings
  carry forward, and findings whose artifacts left `S(D)`, gained
  coverage, or whose anchors vanished are closed — resolved findings never
  re-present and the store cannot grow unboundedly. A binding-declaration
  edit still supersedes. The on-disk format is unchanged: existing stores
  in live workspaces load without loss (legacy same-hash per-head batches
  collapse to the latest on the next verify).
- `projection advance` now answers a medium-relative artifact id (the form
  agents naturally type, e.g. `a.rs` where the slice printed `src/a.rs`)
  with a remedy-bearing refusal: the `PROJECTION_ADVANCE_UNKNOWN_ARTIFACT`
  message names the expected workspace-relative dialect and carries the
  concrete corrected id when prefixing the medium root yields a presented
  id (machine-readable as `corrected_artifacts` in the error details). The
  accepted dialect does not widen — the medium-relative form is still
  refused, keeping one id dialect across enumeration, anchors, coverage,
  and advance.
- `update --from` silently dropped `--dry-run` and `--expected-hash` while
  its help text promised the hash-mode flags were respected. Both now apply
  exactly as on the inline path — `--dry-run` forces a dry run (validated,
  nothing written), `--expected-hash` enforces CAS and overrides the file's
  `expected_hash` field — and the content flags (`--section`, `--append`,
  `--patch`/`--patch-all`, `--metadata`/`--metadata-unset`,
  `--declare-relations`, `--anchor`) now conflict with `--from` at parse
  time instead of being silently ignored. The `--from` help states exactly
  which flags apply.
- Three projection-pipeline defects found by a controlled sync campaign
  (every binding with a non-root medium pointer was affected):
  anchor observation double-prefixed workspace-relative artifact ids and
  reported every such anchor `orphaned`; the default-scaffolded `**/*`
  facet scope was lexically re-rooted onto the medium git root, fataling
  git and silently degrading all change detection to no-signal; and
  source enumeration walked `.git`/`.svn`/`.hg` internals into the
  coverage denominator while the dead-deny scan pruned them (two
  walkers, two answers).

### Changed
- The sync brief for a changed slice now carries a bounded **stale-claim
  search** step: extract the changed facts from the changed artifacts,
  search the destination mem for claims about them (`memstead_search`
  variants), and judge only entities whose claims mention a changed fact —
  closing the slice-blinkering blind spot where a falsified claim stood
  because its entity's anchors never intersected the slice. The step is
  bound to the changed facts (a cosmetic change yields an empty fact set
  and instructs nothing — no whole-mem sweep, no live-verify, no rewrite
  license), renders only when the cursor carries actual changed artifacts,
  and the never-rewrite-unchanged-sections rule stays in the brief.
- Build briefs (discovery and one-shot) now carry a **provenance
  instruction**: attach `anchors[]` to every entity mutation, naming the
  source artifact(s) the entity is drawn from. Rendered engine-side so it
  appears exactly when the running binary accepts the parameter — `/ingest`
  runs stop producing unanchored entities that surface as false coverage
  gaps and defeat the advance gate's auto-`worked`.
- The sync brief's disposition window now states the **live auto-`worked`
  behavior** (anchored writes dispose themselves; agents supply
  dispositions only for the residue), replacing the stale
  "auto-derivation lands in a later cycle" note that predated its own
  implementation. The `/sync` skill's advance step aligns.
- The `/sync` skill may now call `memstead_schema` — the schema-discovery
  contract requires it before any create/update, and the absorption of
  `/reconcile`'s write recipes explicitly deferred section/rel-type
  vocabulary to schema lookup at write time.
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

- **New UniFFI read `mem_config_json(mem)`** — a mem's declared config as
  JSON in the on-disk `config.json` shape (camelCase; `syncState` carries
  the engine-recorded `#synced`/`#verified` baselines). Backend-uniform: a
  git-branch mem's config lives on the `__MEMSTEAD` ref and was previously
  unreachable from any FFI consumer by file path. Read-only; typed
  NotFound for an unknown mem.

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
