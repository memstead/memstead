# Changelog

All notable changes to Memstead are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

**The doorway release.** A first-time schema author, modelling a domain the
project had never seen, hit a wall before writing a single entity. This release
takes the wall down, and it is the first release cut on the rule that made it
overdue: **a schema-language change means the next release is due within days**
— no released binary should refuse a key this repository's own examples,
built-ins, and `schema new` scaffold emit.

**The doorway itself** is the headline. Titles are display text — `Bösenberg
Grundstücks GmbH & Co. KG`, `Wohnung 2.OG rechts`, `RFC 2119 §5` all create and
render verbatim, deriving their slug exactly as the pipeline always did, with a
lossy derivation now *warning* rather than refusing. And one polarity rule
covers both declaration kinds: **a section or metadata field is optional unless
it declares `required: true`** — no more silent default on one side and an
explicit key on the other. Around them, the refusals themselves grew up: schema
validation reports every violation in one pass instead of the first, two
refusals now name the fact that would let the caller recover, a binding's scope
excludes the engine's own files, and the friction ledger records *why* a
refusal happened.

**Second beat: a deadline becomes a first-class axis.** A type declares
`due: {date_field, status_field, open_values}` and `memstead due` renders one
deterministic brief across every mounted mem that declares it — the engine
learns one declaration, not one domain. `obligation@0.1.0`, the first
non-software built-in, is that axis's first declarer: the
document-and-deadline pattern (a dated duty with a stated consequence, the
parties, the agreement, the decision trail) with nothing in it naming a domain
or jurisdiction. `export --format html` completes the beat from the other end —
one self-contained file, zero network requests, handed to a person who has
installed nothing. The docs now state the operating model these three assume:
the agent writes, the engine enforces, and a periodically-invoked agent run
measures and advances — the engine does not calculate, and recurrence is the
loop's property, not a missing engine feature.

**Third: the field's friction, closed.** The agent-trust bundle that preceded
the doorway — repair below boot, boot failures that name their cause, anchors
that resolve per medium, rehearsal as the default for batch writes, atomic
`memstead_relate`, read-mems as ordinary mounts, mem rename, dev builds that
identify themselves — plus the fixes the dogfood loop surfaced along the way.

**Breaking changes** (pre-1.0, but an upgrade needs to see them): the metadata
polarity flip retires `optional:` in favour of `required:` — authored schemas
refuse it with a one-sentence inversion, while sealed packages keep their
written meaning through a format marker, so nothing flips silently;
`memstead_relate` takes a `relations: [...]` list instead of a single triple;
and legacy `readMems` config entries migrate one-way to mounts at boot. Every
built-in that changed shipped a *new* version — every prior version stays
byte-sealed and loadable, so out-of-repo pins keep working unchanged.

### Added
- **`export --format html` — a read surface for non-operators.** One
  self-contained HTML file per mem: every entity as a section (title,
  metadata table, sections rendered from markdown), a type-grouped
  navigation index, and the mem's identity block (title, description,
  subject, schema, export date, and — for read-only mounts — the
  third-party trust class). Wiki-links and typed relationships
  resolve to in-document anchors; cross-mem references render
  labelled, never as anchors; stubs render marked. Self-containment
  is a hard line: zero network requests on open — raw HTML in user
  markdown is escaped as text, and external images degrade to plain
  labelled links (an `<a href>` stays clickable; nothing fetches).
  Byte-deterministic given store and export date. Backend-uniform,
  CLI-only (the export family's surface policy) — a file you hand to
  a person: no server, no account, no installed anything.
- **The docs state the operating model — the agent loop is the
  runtime.** README gains "How a Memstead system runs" (the agent
  writes, the engine enforces, and a periodically-invoked agent run
  measures, maintains, and advances what needs advancing — curated
  by agents, enforced by schema, run by the agent loop) and the
  honesty list gains the computation boundary: **the engine does not
  calculate** — it can know a statement is due, hold every input,
  and name what is missing, and producing the output is the agent's
  work. VISION states the same model beside the freshness problem.
  Scheduling, notifications, and recurrence are the loop's
  properties, not missing engine features.
- **`obligation@0.1.0` — the first non-software built-in schema.**
  The document-and-deadline pattern: `obligation` (a dated duty with
  a stated consequence — a liability that forfeits, not a target
  that slips), `party`, `commitment` (the modelled agreement,
  distinct from the artifact evidencing it), and `decision` (the why,
  with its rejected alternatives). Nine typed relationships
  (CONCERNS, OBLIGES, EVIDENCED_BY, ARISES_FROM, REPLACES, BLOCKS,
  PARTY_TO, PART_OF, plus alias REFERENCES with the wildcard
  cross-mem grant, so an obligation mem cites into a mem of any
  schema). The obligation type declares the `due:` axis — the
  due-brief's first declarer — plus block-tier `requires_when`
  (completed_on when done; responsible when criticality is high) and
  a block-severity `required_outgoing` to its subject. Recurrence is
  a label for the maintaining agent, never engine automation. Nothing
  in the schema names a domain or jurisdiction — it is deliberately
  the pattern many domains fork. Doubles as the doorway bundle's
  integration fixture: exemplar titles exercise the widened title
  grammar and fields the flipped `required:` polarity.
- **The due-brief: a schema declares its deadline axis, the engine
  renders it.** A type can declare `due: {date_field, status_field,
  open_values, lead_section?}` — validated at schema load with the
  loader's usual recovery quality (and report-all accumulation) — and
  `memstead due [--within <window>] [--mem <name>]` renders one
  deterministic brief over every mounted mem whose schema declares
  the axis, writable and read-only mounts alike: open entities whose
  date falls inside the window or is already past, **overdue first
  and marked**, then ascending by date (ids as tiebreaker), each
  entry carrying id, title, date, status, and the lead section's
  content. Third-party (read-only) entries carry their origin label
  and quote their content — a stranger's mem states a deadline, it
  does not instruct. Windows are relative (`90d`, `6m`, `2y`;
  **default 90d**); everything overdue is always included. The
  renderer is one shared engine entry point, so the CLI and UniFFI
  (`due_brief`) serve byte-identical content; there is deliberately
  no MCP tool (briefs are the CLI/app family — the MCP instructions'
  CLI-companion note names the verb). The engine never advances a
  date: recurrence is the agent loop's job, by design.

### Changed
- **Metadata fields become opt-in required — absence means optional,
  everywhere.** One rule now covers both declaration kinds: **a
  section or metadata field is optional unless it declares
  `required: true`.** `MetadataFieldDef` gains `required` (defaulted,
  documented in the generated meta-schema); the retired `optional:`
  key refuses on every authoring/install path with a one-sentence
  inversion fix (delete `optional: true`; replace `optional: false`
  with `required: true`), while **sealed** schemas carrying it keep
  loading with inverted-but-equivalent semantics. Because an absent
  key used to mean *required* and now means *optional*, sealed
  packages gain a format marker (`schema-format.json`) written by the
  install/seal path from this change on — **an unmarked sealed
  package keeps its legacy written meaning (absence = required), so
  nothing silently flips in either direction.** Built-ins migrate by
  version bump per the append-only retention manifest:
  `default@1.3.0`, `project@0.4.0`, `software@0.4.0` ship in the new
  language; **every prior version stays byte-sealed and loadable, so
  out-of-repo pins on earlier versions keep working unchanged.**
  `quickstart` pins `default@1.3.0`; the `schema new` scaffold now
  teaches the polarity with one required and one optional field.
  A required field with a `default_value` is auto-filled and never
  refused — required-with-default means "always present", not
  "caller must type it".
- **Titles are display text: the grammar widens, the slug stays, the
  divergence warns.** A title is now any single-line text —
  `Bösenberg Grundstücks GmbH & Co. KG`, `Wohnung 2.OG rechts`,
  `Anlage 4a – Leistungsbeschreibung`, `Acme Inc.`, `RFC 2119 §5` all
  create, render verbatim as the H1 and on every read surface, and
  derive their slug exactly as the permissive pipeline always did
  (characters outside Unicode alphanumerics/whitespace/hyphen
  dropped, whitespace to hyphens, lowercased). When the derivation
  drops characters, the response carries the new typed warning
  `TITLE_CHARS_DROPPED_FROM_SLUG` naming the dropped characters and
  the derived slug — the title↔id divergence stays visible, it just
  stops being fatal. `INVALID_TITLE` refusals remain for control
  characters, titles whose slug derives empty (`§§§`), and over-long
  composed ids. Every title the old grammar admitted produces a
  byte-identical entity; the slug and wiki-link grammars are
  untouched (`[[Natural Title]]` still refuses — link by slug). The
  `TITLE_GRAMMAR_RULE` sentence moved at every embedding site (CLI
  help, MCP descriptions, conformance tests, API reference).

### Fixed
- **Two source comments no longer describe retired behaviour.** The
  `schema new` scaffold's annotation on `no_self_loop_relationships`
  claimed community-signal propagation — the exact promise the
  `propagating_relationships` rename retired; it now states the
  field's single effect (self-loop refusal on relate) in agreement
  with the `no_self_loop_relationships_effect` note the schema
  response serves. The slug pipeline's doc comment claimed
  Obsidian-style `[[<title>]]` authoring "round-trips without lookup"
  generally; it now states the property's real scope — it holds for
  titles already in slug form (case-less scripts, lowercase
  single-token Latin), while any other title derives a different slug
  and the strict wiki-link decoder refuses the natural form. No
  behaviour changed.

### Changed
- **The friction ledger records why a refusal happened.** For refusals
  whose typed envelope carries a closed, engine-owned reason
  discriminator (today `INVALID_TITLE` — `invalid_chars` /
  `control_chars` / `id_too_long` / `empty` — and
  `MEM_PATH_NOT_ALLOWED` — `no_allowlist_configured` / `no_match` /
  `outside_workspace`), the ledger entry now carries that reason and
  `health --include friction` (CLI and both MCP flavours, one shared
  summary) breaks the code's count down by reason under `by_reason`.
  Refusals without a discriminator record exactly as before — no
  field, no placeholder — and pre-change ledger lines keep parsing
  and counting. The privacy hard line is enforced by construction:
  reasons enter the writer only as `&'static str` selected from a
  per-code vocabulary table (`closed_reason`), so a caller-influenced
  value can select from but never extend the closed set, and the
  module contract now states that vocabulary rule instead of
  enumerating the record's fields.
- **A binding's scope excludes the engine and states its own reach.**
  The engine's own state — `.memstead/`, `.memstead.cache/` (by name,
  wherever they appear), and every mount's *resolved* storage
  location (mem-repo working copy, folder-mem root, archive file) —
  is excluded from every strategy's input set unconditionally: the
  exclusion is not a configurable deny entry, an explicit allow glob
  does not admit it, and it applies identically to the mtime
  enumeration and the git diff pathspecs, so the coverage denominator
  is strategy-invariant and `projection verify`'s own findings store
  never enters its next run's input. Freshly scaffolded
  `codebase`/`filesystem` bindings additionally carry default
  `deny_paths` for platform/tooling debris (`**/.DS_Store`,
  `**/.git/**`, `**/node_modules/**`, `**/Thumbs.db`) —
  **materialised into the record at scaffold time, not injected at
  load**, so the author can see and delete them, and bindings created
  before this change keep behaving exactly as recorded. `projection
  init` whose resolved medium base falls outside the workspace root
  now warns, naming the consequence (workspace-relative `../…`
  artifact ids; source-relative anchors resolving as orphaned) while
  still succeeding.
- **Two refusals now name the fact their caller needs to recover.**
  `MEM_PATH_NOT_ALLOWED` with reason `no_allowlist_configured` names
  the concrete grant command (`memstead workspace allow-create
  '<pattern>' --schema <name@version>` / MCP
  `memstead_workspace_allow_create`; the delete table names
  `allow-delete`) in both the prose and a structured
  `details.remedy` — the cold-start dead-end where the second
  command of a fresh workspace refused without naming the way
  forward is gone, while the default-deny posture stays. `no_match`
  gets its own sentence (rules exist, none matched — with the
  configured patterns and the covering-rule remedy);
  `outside_workspace` gains no remedy, since adding a rule would not
  fix it. `ENTITY_ALREADY_EXISTS` names the title of the entity
  occupying the id (`details.existing_title` /
  `details.existing_is_stub`, mirrored in the message) on every
  producer — create, batch create, rename — so colliding titles that
  derive the same slug are diagnosable from the refusal alone; a
  stub occupant says it is a stub instead of rendering an empty
  title.
- **Schema validation reports every violation at once.** The schema
  loader (shared by `schema validate`, `schema install`, workspace
  boot, and every other loader entry point) now accumulates all
  semantic violations found on successfully parsed structure and
  refuses once, each violation keeping its full recovery material
  (offending object, declared/allowed set, nearest-match suggestion).
  A seven-type schema with violations spread across its files is
  fixed in one edit instead of one validate round per violation. A
  single violation reports exactly as before — never as a one-element
  list — and structural failures (unparseable manifest,
  declared-vs-found type-file mismatch) still short-circuit, since
  everything downstream of them would be noise. Accumulated order is
  deterministic (declaration order; type files sorted by name).
- **The batch family's no-dry-run contract is reversed: rehearsal now
  covers the whole write surface.** `batch-create` / `batch-update` /
  `batch-relate` accept `--dry-run` (engine: a batch-level `dry_run`
  parameter): the FULL validation pass runs — intra-batch reference
  resolution, in-order relate semantics, report-all refusals — then
  the batch stops before any write and reports the would-be per-entry
  receipt with the marker form's empty `commit_sha`. An illegal batch
  refuses with the identical per-entry envelope the real call
  returns. The old contract ("the batch family has no dry-run") was
  set when the family was pure bulk-ingest tooling; pre-validating a
  whole multi-entity build before entity one lands is precisely
  batch-shaped, so the contract is deliberately reversed rather than
  honoured. `memstead_relate` (MCP) and `memstead relate` (CLI) gain
  `dry_run` the same way — a rehearsed relate reports the would-be
  edge and would-be auto-stub (reported, never created), and a
  rehearsed illegal relate refuses exactly as the real call would.
  The filesystem MCP flavour keeps its typed `UNSUPPORTED_PARAM`
  refusal (now also on relate) so no surface silently ignores the
  flag. The rehearsal contract — identical validation, observable
  zero side effects (git refs, working tree, `.memstead/`
  byte-identical), marker form on every rehearsed response — is now
  asserted by tests across create, update, relate, and the batch
  family, including against quarantined and read-only mems.
- **`propagating_relationships` renamed to
  `no_self_loop_relationships` — the name stops lying.** The field's
  only functional effect was always the self-loop refusal on
  `memstead_relate`; its name promised propagation the engine never
  performed, and two external schemas were designed around the
  misreading. The new key says exactly what it does and is now
  optional (empty lists can simply be deleted). The old key refuses
  at authoring/install load with a typed error naming the new key —
  one mechanical rename per schema — while SEALED content (compiled
  built-ins, installed refs) keeps loading with the old key
  translated, so shipped versions never break (install-time strict,
  sealed-tolerant). On a workspace where one mem pins an old-key
  authored schema, that mem quarantines with the rename error as its
  reason (`SCHEMA_LOAD_FAILED`) while every healthy mem serves —
  broken authored schema packages in general no longer take the
  workspace down (the schema-dir walk skips-and-records instead of
  failing boot). Every built-in that carried the old key ships a new
  version with the new spelling (default@1.1.0, engineering@0.2.0,
  ingest@0.3.0, planning@0.3.0, project@0.2.0, software@0.2.0 —
  ingest@0.3.0 additionally declares its three entry types `leaf`);
  the old versions stay sealed and loadable per the retention
  manifest. `memstead quickstart` now pins the current default
  generation.

### Added
- **Dev builds are distinguishable: every version surface carries the
  full build version.** Between releases every dev build reported the
  same crate semver, so the "engine version changed → re-read the
  tool roster" hint and the `ENGINE_VERSION_SKEW` stamp comparison
  could never fire in dogfood or field use. A build script in
  `memstead-base` now best-effort captures the git commit at build
  time (short sha, `-dirty` suffix on a modified tree; empty — and
  harmless — outside a git checkout, e.g. crates.io builds), and
  `build_info::full_version()` renders `<semver>+g<sha>[-dirty]`.
  Consumers: CLI `--version`, both MCP flavours' `serverInfo.version`
  (the full server additionally appends a runtime `Build: …` sentence
  to its instructions when a sha exists; the compile-time semver line
  is unchanged), the overview's `_engine_version`, and the per-mem
  mutation stamp — whose skew comparison now compares full strings,
  so a rebuild between mutations fires the existing warn-tier,
  non-blocking hint. Old plain-semver stamps compare against the full
  value and may fire once — desired, no migration.
- **A pinned built-in with newer generations says so:
  `SCHEMA_GENERATIONS_BEHIND`.** A new default (ungated, warn-tier,
  never blocking) health signal: a mem whose pinned schema resolves
  from the built-in catalogue while the catalogue registers at least
  one strictly-higher version (real semver ordering) surfaces a
  warning naming the pinned ref, the newest available version, and
  the migration verb (`memstead mem set-schema`). The pin keeps
  working — retention seals every shipped version. Locally-installed
  (workspace-storage) pins are silent: the engine only knows
  generations for built-ins. Rides the health JSON `warnings` and
  both markdown renderers, like the skew hint.
- **The check operation: "checked and sound" vs "never checked" vs
  "checked, but changed since" is derived, never declared.** A new
  verb — `memstead_check` (MCP, both flavours; the deliberate single
  tool addition of the agent-trust bundle) and `memstead check`
  (CLI) — records "entity E checked, verdict ok | failed, via method
  M" as an engine-recorded act carrying mutation-provenance identity
  (actor, client, caller-declared role) and the entity's
  `content_hash` at check time. Checking mutates nothing: entity
  markdown, `_hash`, and mem commits are untouched — that
  non-mutation is what makes staleness computable. Records append to
  the workspace's check ledger (`.memstead/state/checks/`,
  append-only, no rotation; persistence failure refuses
  `CHECK_NOT_RECORDED` — never best-effort); a newer check
  supersedes older ones for state derivation but never erases them.
  Derived check state — `never_checked` | `checked_ok` |
  `check_failed` | `check_stale` (entity changed after its last
  check, computed by hash comparison) — serves in the entity read's
  opt-in `mutation_provenance` block alongside the newest check
  record. The verdict vocabulary is closed (`ok` | `failed`;
  `INVALID_VERDICT` names it); unknown entities, read-only and
  quarantined mems refuse typed. A check with an unspecified role
  records honestly but cannot confirm independence downstream.
  The `checks` health axis aggregates per mem: counts of the four
  derived states plus the author≠checker independence gate over
  ok-checked entities — the entity's created-by identity compared
  against its newest check's recorded identity (`self_checked` when
  equal: twice-asserted, not verified; `confirmed_independent` when
  different; `unconfirmable` for an unspecified-role check or an
  unknowable author) — the first real gate built on recorded
  mutation provenance, computed from records, never from fields.
  And binding-less verification no longer observes-and-forgets:
  `memstead verify-anchors` persists its flagged findings (drifted /
  unresolvable) under a mem-scoped `standalone` key in the findings
  store — a keyspace that can never collide with a binding's
  `hash(D)` and never shares its file — and the next pass re-serves
  them as `already_seen` (a repaired source closes its finding).
  Binding-backed findings flows are byte-untouched. And process-mem
  pairing is declarative: a destination mem's config can declare its
  process mem (`processMem` in the mem config) — the declaration wins
  over the binding-name convention through one shared resolution
  function serving both the brief renderer and the open-questions
  health axis, which now pairs even with no binding at all; a
  declaration naming an unmounted mem surfaces as the typed
  `DECLARED_PROCESS_MEM_MISSING` finding, never a silent fallback.
  Without a declaration, name-derivation behaves exactly as before.
  Gate-hardening from the plan's own grading: the CLI's folder-backend
  mutation paths now record their client identity (previously only
  mem-repo commits carried `Client:`), and the independence gate
  treats a record with an unrecorded client half as `unconfirmable` —
  a same-binary author+check can never read as independent.
- **Mutation provenance: who acted, in what role — recorded, not
  declared.** Every mutation records a caller-declared role from the
  closed vocabulary `author` | `checker` | `verifier` (or
  unspecified) immutably alongside the mutation, extending the
  existing trailer/ledger mechanism: a `Role:` commit trailer on
  mem-repo backends, a `role` field in the folder JSONL ledger — one
  shape across backends, absence recorded as absence. Declared per
  call (a `role` parameter on all five MCP mutation verbs) or per
  session (`--role` on both binaries; per-call wins); unknown values
  refuse `INVALID_ROLE` naming the vocabulary; omitting the role is
  legal forever and records unspecified — never refused, never
  defaulted to a real role. The entity read serves the derived
  record opt-in (`include_provenance` on `memstead_entity`,
  `--provenance` on `memstead entity`): a `mutation_provenance`
  block with `created_by` and `last_modified_by` — actor, client,
  role, timestamp, backend reference — derived from append-only
  history; when the recorded story does not start at creation,
  `created_by` is absent and `story_truncated` is true. Default
  responses stay byte-unchanged, and provenance never participates
  in entity markdown or `_hash`. Trust model: roles are
  caller-declared but tamper-evident — bound to specific operations
  in history no verb can edit afterwards, so process gates compare
  recorded identities across operations instead of trusting
  self-written metadata fields.
- **Derivation staleness: "my source changed" is computed, never
  stamped.** A schema can declare a rel-type `derivation: true` —
  the source derives from the target. Explicitly writing such an edge
  (create relations, update declare_relations, relate, and every
  batch sibling) records the target's current content hash as the
  edge's baseline in an engine-owned sidecar
  (`.memstead/derivations.json`, the anchors precedent: same-commit
  staging, invisible in the markdown, excluded from `_hash`). The
  include-gated `stale_derivations` health axis reports every such
  edge whose target's current hash differs from its baseline —
  "source S is stale against target T" — and edges with no baseline
  as `unbaselined`, distinctly, never fabricated as fresh or stale.
  Re-asserting the edge via `memstead_relate` refreshes the baseline
  as the duplicate-add's one effect — the agent's explicit "reviewed,
  still holds" — with the refresh STATED on the response
  (`DERIVATION_BASELINE_REFRESHED` + the sidecar commit's sha, `_hash`
  and markdown untouched); undeclared rel-types keep today's exact
  no-op. Warn-tier forever: staleness is a review signal, never a
  write-block. This delivers, properly declared, the behaviour the
  retired `propagating_relationships` name falsely promised.
- **Open-questions axis: a mem can enumerate what it does not know.**
  `memstead health --include open_questions` (and the MCP counterpart
  on both flavours) serves, per mem, a composed worklist of the
  holding's own holes: its stubs, its never-confirmed (`recheck`) and
  `unresolvable` anchors, its unsatisfied constraints, its dangling
  links — and, when a paired process mem is resolvable for the
  destination, that process mem's open entries, with negative
  findings under a DISTINCT `already_searched` heading ("done, keep
  off" — never flattened into the todo pile). Each item carries its
  kind and the id it hangs on. The axis is a composition of existing
  signals — it computes nothing new and reads each signal from the
  same source its own axis serves, so it can never disagree with
  them. Include-gated (health output is byte-unchanged without it),
  per-kind item cap with an explicit `more` remainder, and an
  unresolvable process pairing is stated per mem rather than silent.
- **Negative findings: "searched, nothing found" is a result, not a
  gap.** ingest@0.5.0 adds a fourth type, `negative_finding` — what
  was sought, the search directions actually walked (`search_path`,
  the load-bearing section: git carries when, only the entry can
  carry where the search looked), and the empty result. It is the
  operational opposite of `coverage_gap`: a gap is work to do (the
  source has material the destination lacks), a negative finding is
  work that is done and must not be silently redone — both types'
  prose names the other and the rule for choosing. Leaf-declared
  (edge-less findings are never orphans), with the same optional
  wiki-link reach into the destination claim the absence bears on,
  and an engine-validated exemplar demonstrating a real absence
  finding. Prior ingest versions stay sealed per the retention
  manifest; the process-mem pairing pins the new version.
- **Schemas teach by example: a type can carry one engine-validated
  exemplar.** A schema type may declare `exemplar:` — one canonical
  entity in the mem markdown shape (title, metadata, sections,
  relations with bare placeholder-slug targets). The engine validates
  every exemplar against its own type through the REAL create path
  (`dry_run` on an in-memory engine) at `memstead schema validate` and
  at install/seal on both backends: a package whose exemplar does not
  conform refuses with a typed error naming the type and the defect —
  no warn-and-carry mode exists, so an exemplar can never drift into
  teaching the wrong shape. `memstead_schema` at `verbosity: full`
  serves each type's exemplar; the lite skeleton is byte-unchanged.
  `memstead type <name>` renders it in the full-depth CLI view. The
  never-validated, never-served `examples:` list is retired: authoring
  loads refuse it with a pointer at `exemplar:`; sealed content keeps
  loading with the key dropped. The worked-example teaching package
  (`memstead-schema/examples/minimal`) models the practice, gated by
  the same validator in CI. Every built-in type carries an exemplar:
  six new manifest-sealed generations ship them (default@1.2.0,
  engineering@0.3.0, ingest@0.4.0, planning@0.4.0, project@0.3.0,
  software@0.3.0 — 44 exemplars, all engine-validated in CI, with
  completeness asserted on the newest version of every name);
  `memstead quickstart` and the ingest process-mem pairing pin the new
  generations; older versions stay sealed and loadable per the
  retention manifest.
- **Friction ledger: the engine measures its own surface's
  learnability.** Every typed refusal the CLI or MCP surface returns
  appends one entry — every value drawn from a closed engine-defined
  vocabulary; never parameters, ids, or message text — to a
  workspace-local, gitignored, size-bounded ledger under
  `.memstead/state/friction/`.
  `memstead health --include friction` (and the MCP counterpart on
  both flavours) summarizes it: counts per refusal code and per verb,
  whole-ledger plus a recent 24h window; without the include, health
  output is unchanged. The ledger is local-only forever — no
  transmission, no registry involvement — and recording is
  best-effort: a ledger write failure is swallowed and the refusal
  returns unchanged, so the instrument never perturbs what it
  measures. Appends are single-write whole lines on an append-mode
  handle, so a CLI invocation beside a running MCP server interleaves
  entries without corruption; the bound is two-generation rotation.
- **Leaf declaration: a type can say its entities are terminal by
  construction.** A schema type may declare `leaf: true`; health's
  orphan axis then exempts that type's edge-less entities (they are
  edge-less by design — counting them as orphans was noise masking
  real orphans, e.g. 7 false ingest-mem orphans per report) and
  instead reports the population as `leaf_entities_by_type`
  (`<schema_ref>:<type>` → count) on `memstead health` and the MCP
  counterpart — visible, never vanished. Leaf means "no edges
  required", not "edges forbidden": leaf entities with edges stay
  legal, and every other health axis, search, and traversal treats
  them like any other entity. The flag is served at both schema
  verbosity levels; output is byte-unchanged for schemas that declare
  nothing.
- **Surface honesty: the MCP instructions tell the whole truth about
  the surface.** Session start now answers "what can this engine do,
  on which surface, at which version" with no out-of-band knowledge:
  both server flavours' instructions carry the engine version, the
  complete grouped tool roster (24 tools full / 12 lean — previously
  13 of 24 were named), and a CLI-companion note naming the verb
  families that deliberately live on the CLI (batch mutation,
  export/install, distribution, bootstrap/repair) and when to reach
  for them — ending the capability-blindness class where an agent
  hand-rolled 175 single-entity calls beside an unannounced
  `batch-create`. The MCP serverInfo version now equals the crate
  version on both flavours (the hardcoded `"0.1.0"` is gone), and
  `memstead_overview` frontmatter gains `_engine_version`. The
  instructions are registry-tested: a bidirectional test fails when
  the text lags or leads the tool registry, the version is pinned to
  the crate by test, and a byte-budget tripwire guards against
  unbounded instruction growth.
- **Quarantine boot: a broken mem disables itself, never the
  workspace.** Mem-level boot failures (unresolvable or missing schema
  pin, backend instantiation or read failure) no longer take the
  workspace down — the mem quarantines with its typed reason and
  repair command while every healthy sibling loads and serves
  normally, ending the outage class where one bad pin held thirteen
  mems hostage. Quarantine is not tolerance: the mem serves nothing;
  reads and writes naming it refuse with the new `MEM_QUARANTINED`
  code carrying the underlying reason (never masquerading as
  `UNKNOWN_MEM` or `ENTITY_NOT_FOUND`), and cross-mem links into it
  degrade like dangling links. The roster rides the existing
  dashboards — overview (`## Quarantined Mems`) and health
  (`quarantined[]`), ungated, byte-unchanged when healthy. Repair
  returns the mem in-process: `mem set-schema` on a quarantined mem
  repins the retained mount and re-attaches; `memstead_reload`
  re-attempts the attach after any external repair. Binding-level
  failures (legacy pre-v2 or corrupt projection configs) likewise
  quarantine only that binding: the workspace boots, healthy bindings
  serve, and the affected binding's projection verbs refuse
  `PROJECTION_QUARANTINED` with the reason naming `memstead projection
  migrate`. The MCP server now starts whenever the process starts: a
  partially broken workspace serves normally, and a wholly unbootable
  one (corrupt store) serves a mem-less diagnostic shell whose
  overview/health answer with the typed boot diagnosis
  (`boot_diagnosis`) instead of the historical silent
  `-32000 Connection closed` exit. The wholesale-abort regression
  tests are replaced by quarantine-behaviour tests as a deliberate
  act.
- **Repair below boot: a fix for a boot failure no longer requires the
  boot.** The verbs boot-failure messages name as remedies now run on
  exactly the workspace whose boot they repair. `memstead mem
  set-schema` falls back to a below-boot repair path when the
  workspace does not boot: it repins through the same target-ref
  resolver and the same value-level config-pin writer the booted path
  uses (no second validation regime), skips only the entity-conformance
  gate (entities are unreadable before boot — the output says so, and
  the next boot's health carries any findings), and never force-writes
  a pin that resolves nowhere. `memstead schema install` never boots at
  all on the mem-repo flavour — it validates through the same
  `validate_schema_package` gate and seals onto the
  `__MEMSTEAD:schemas/` ref directly, so the full plenum recovery path
  (install the missing package, repin, boot green) works end to end on
  an unbootable workspace. `memstead projection migrate` no longer
  deadlocks when its reconcile-cursor seeding meets a workspace that
  still doesn't boot: seeding is explicitly deferred with a
  `RECONCILE_CURSORS_DEFERRED` notice naming the follow-up, and the
  cursor file is kept. The rule itself — repair verbs operate below
  boot — is recorded in the handbook's engine chapter.
- **Built-in retention and version stamps: shipped versions never
  vanish.** The `ingest@0.1.0` built-in — deleted from the catalogue
  by the 2026-08-06 in-place version bump, stranding a workspace
  pinned to it — is restored as a side-by-side directory, and the
  whole failure class is now impossible to reintroduce:
  `builtins/MANIFEST.toml` is the append-only ledger of every
  ever-shipped built-in `(name, version)` with a sealed content hash,
  and a retention test fails CI on removal, in-place edit, or an
  unlisted new version (appending the printed `[[shipped]]` block is
  the whole ceremony). Separately, every mutation now stamps the mem's
  engine-owned config (`mutationStamp`: engine version + resolved
  schema ref, written only when the value changes, riding the
  `__MEMSTEAD` ref so mem-branch cursors never move); boot compares
  the stamp against the running binary and surfaces a divergence as
  the warn-tier `ENGINE_VERSION_SKEW` hint on boot output and
  `memstead health` — informative, never fatal, and silent for
  stamp-less mems and read-only sessions.
- **Typed boot errors: every boot failure names its code and its
  fix.** Boot failures no longer collapse to `ERROR [INTERNAL]` with
  no next step. `BootError` now carries `code()` / `details()` /
  `surface_message()`; the CLI's engine-setup seam lifts them into the
  standard `{code, message, details}` envelope, and the MCP server
  prints the same code and message on stderr before exiting (its
  transport never comes up on a failed boot, so stderr is the
  diagnostic surface). Store-layer failure classes each carry their
  own token (`WORKSPACE_STORE_PARSE`, `WORKSPACE_STORE_IO`,
  `WORKSPACE_STORE_FORMAT_MISMATCH`, `LEGACY_WORKSPACE_LAYOUT`,
  `PROJECTION_STORE_LEGACY` — now real, previously a phantom doc
  comment — `UNKNOWN_BINDING_VERSION`); classes with no mechanical
  remedy say so plainly instead of inventing a command. The
  schema-pin failure message now ends in the repair command the
  source trail calls for: right-name/wrong-version pins name the
  concrete `memstead mem set-schema <mem> <name>@<installed-version>`
  repin (the previously suppressed disappeared-built-in case), and
  name-unknown-everywhere pins name the `memstead schema install`
  path even when no authoring package is present to point at. A
  boot-failure class sweep (`memstead-cli/tests/boot_typed_errors.rs`)
  plus a CLI/MCP parity test pin the contract.
- **The overview names its workspace: `_workspace_root` frontmatter.**
  `memstead_overview` (both server flavours, and the CLI `overview`)
  renders the absolute path of the workspace the serving engine booted
  from as a `_workspace_root` frontmatter slot — the one authoritative
  place a session can learn where `memstead` CLI invocations must point
  (`--workspace <root>`) without inheriting a cwd or an env var from
  the caller. Omitted for engines built straight from a mount list
  (in-memory sketches have no root). The plugin's `/sync` skill now
  resolves the root once (`binary-version.mjs root <dir>` — the
  existing walk-up + `.mcp.json` cd-target probe, new `root`
  subcommand) and passes `--workspace` explicitly on every CLI call it
  issues, so the sync loop works from any directory with zero
  configuration — no more `MEMSTEAD_WORKSPACE` export.
- **Section content format: schemas declare a section's markdown
  shape.** A schema section can carry `content` — a flat expression
  over the mdast block vocabulary (`"(heading(3) list(bullet))+"`,
  operators: sequence, name-alternation, `+ * ?`) — plus
  `item_pattern` (a regex over list items with lazy continuations
  joined, or over paragraph source lines; named groups name the parts
  in refusals), `table` (`columns` pins header names and order,
  `column_patterns` per-cell regexes — judged on the REAL source cell
  count that GFM would silently pad or truncate), `example` (echoed
  verbatim in every format refusal), and `format_severity`
  (`block` default / `warn`). Enforcement uses a real CommonMark
  parser (`pulldown-cmark`, no default features), so the validator
  agrees with the renderer on lazy continuations, mixed bullet
  markers, code blocks containing `- ` lines, and malformed GFM
  delimiter rows. Create judges every written section; update judges
  each touched section on its COMPOSED body; block-tier refuses
  pre-commit with `SECTION_CONTENT_MISMATCH` (found sequence,
  `failed_at` line, `expected_next`, the example) /
  `SECTION_ITEM_PATTERN_MISMATCH` / `INVALID_TABLE_COLUMNS`; standing
  violations of any tier ride `health --include constraints`. Reserved
  headings: `^# ` joins `^## ` as a write-time refusal in every
  section; setext h1/h2 refuse inside format-checked sections; a
  `heading(1)`/`heading(2)` declaration refuses at load. Loader
  honesty with sealed leniency: install and strict validation refuse a
  malformed declaration naming EVERY problem; an already-sealed schema
  carrying one keeps loading, the defect surfaces under
  `schema_format_defects` in health, and the declaration is never
  enforced. The `memstead_schema` response renders every declaration
  at both verbosity levels. First consumer: the auto-managed
  Relationships section's line shape is now enforced through this
  mechanism (the hand-rolled strict-validator check is gone), and the
  built-in `planning` schema adopts `content: "list(bullet)"` on its
  bullet-prescribing sections as version 0.2.0 (0.1.0 unchanged).
- **The constraint vocabulary: schemas declare what is unhealthy to
  keep.** A type can now declare `constraints` in its schema YAML —
  five forms: `requires_when` ("`status: checked` requires
  `checked_by`": a metadata field or section becomes required whenever
  another metadata field holds a declared value), `unique` (a tuple of
  metadata fields unique among the type's entities within one mem —
  defaults to `block`, its whole point is bouncing the duplicate),
  `enum_from_neighbour` (a field's legal values are the bullet entries
  of a named section on the entity reached via a named edge; an
  unbacked value is a finding), `status_propagation` (a terminal
  status value taints every entity reaching it — transitively — via a
  named rel-type and direction; tainted entities are health findings
  naming their tainting ancestor; always warn-tier, and a `block`
  declaration refuses at load rather than becoming a promise the
  engine won't keep), and a `severity` on every `required_outgoing`
  block. One uniform severity model: `warn` (the default — health
  finding, plus a write-time `CONSTRAINT_UNSATISFIED` warning) or
  `block` (write-time refusal on every surface — create, update, and
  the relate-remove that would break a `required_outgoing` block or
  un-back an `enum_from_neighbour` value — plus the same health
  finding for pre-existing violations). `memstead health --include
  constraints` lists standing violations (participates in `--strict`),
  the `memstead_schema` response renders every declaration with its
  severity at both verbosity levels, and malformed declarations
  (unknown field, unknown trigger field or rel-type, a value outside
  the field's enum, a section no type declares, an unevaluated `kind`)
  refuse at schema load with a typed error naming the offender — no
  declaration can load and be silently ignored. Schemas declaring no
  constraints keep byte-identical behavior.
- **`memstead verify-anchors` — a drift statement without a binding.**
  Verifies every anchor in a mem against its declared source and
  reports, per anchor: `resolved`, `drifted` (hash differs under
  `stable` stability), `recheck` (differs under `unstable`, or a hash
  is missing), or `unresolvable` (source absent, or a grain the
  mechanism does not reach) — honestly, never fabricating a state.
  Works on hand-authored mems with no binding at all; binding-backed
  mems report the same states the binding verify sees, because both
  now share one per-anchor resolution mechanism: the old
  workspace-wide single-source gate is gone, so a mem whose bindings
  span multiple media no longer collapses to "unobserved". Read-only —
  no entity change, no commit. `memstead health` gains an
  include-gated `anchors` axis (per-mem counts of the four states) on
  the CLI and both MCP server flavours; without the include, health
  output is unchanged.
- **`memstead_relate` is a list of relation operations, applied
  atomically.** Breaking parameter-shape change: the tool now takes
  `relations: [{from, to, type, remove?, description?}, ...]` plus an
  optional shared `note` — the whole list is all-or-nothing in one
  commit per touched mem, with per-entry validation identical to a
  single operation and in-order semantics (later entries validate
  against the state earlier entries produced). One failing entry
  refuses the whole list and the refusal reports every failing entry:
  a list of one surfaces its entry's own typed code top-level; larger
  lists wrap under `BATCH_REFUSED` with per-entry envelopes. The
  response is a plural `results[]` envelope (per entry: from, to,
  rel_type, action, source, `_hash`) with top-level `commit_sha`,
  `warnings`, and `orphan_stubs_removed`; a list of one routes through
  the single-op engine path, so it behaves exactly like the historical
  single call modulo envelope plurality. Both server flavours. The
  batch-relate CLI command and the pinned MCP batch-tool abstentions
  are unchanged — atomicity arrives as the list form, never a second
  tool. `BatchResult` additionally reports `orphan_stubs_removed`
  (also visible on the CLI batch envelope).
- **Mem curation reaches MCP.** `memstead_mem_create` gains optional
  `title`, `description`, and `subject` parameters (applied at
  creation through the same setters the CLI verbs use), and the new
  `memstead_mem_configure` tool updates the same three fields on an
  existing mem — set what is present: absent fields untouched, empty
  string clears, `clear_subject` clears the subject block as a unit.
  Gate-free like the sibling setters; unknown mems and read-only
  mounts refuse typed. The MCP consumer profile — what the tool
  surface is complete for, and what deliberately stays off it — is
  now written down in `dev/handbook/agent-surfaces.md`, and the batch
  abstention test's rationale argues against that profile (boot cost,
  agent-context cost, atomicity) rather than boot cost alone.
- **Read-mems are ordinary read-only mounts, and `memstead uninstall`
  exists.** Installed read-mems no longer attach to a host writable
  mem's config — `memstead install` (and the MCP server's `--read-mem`
  boot flag) registers the archive as a workspace-level mount with
  `capability: read_only` and its content-addressed cache path, ending
  the one structural special case in the mount model. The `--mem` host
  flag is gone. `memstead uninstall <name>` removes the registration
  symmetrically — the global cache copy survives by default, and a
  re-install re-registers without a download; it refuses while writable
  entities still hold edges into the read-mem
  (`MEM_HAS_INCOMING_REFS`), and refuses writable mems
  (`MEM_NOT_READ_ONLY`). **Breaking config migration:** workspaces
  whose configs still carry legacy `readMems` entries migrate one-way
  at boot — the entries become mounts, the legacy key is removed
  through the engine's own config writers, and one
  `READ_MEMS_MIGRATED_TO_MOUNTS` warning names what moved; a second
  boot is silent. Search and cross-mem references into read-mems
  behave identically before and after.
- **A mem can be renamed.** `memstead mem rename <old> <new>` performs
  a complete rename across every surface that carries the name: entity
  ids re-prefix (ids derive from the mount name), every cross-mem edge
  and wiki-link in every writable mem is rewritten (one commit per
  affected mem, all sharing one logical-operation id), anchors-sidecar
  keys re-key, workspace `[cross_mem_links]` grants naming the mem are
  rewritten on either side, sync-state keys and the
  binding/findings-store paths follow, and the mem's commit history is
  preserved — the branch moves at its tip, never a fresh seed. Agent
  mode requires the old name to pass `[[mem_management.delete]]` and
  the new name `[[mem_management.create]]` (schema pin unchanged);
  operator-mode bypasses both, as with init/delete. Every refusal
  (unknown mem, collision, grammar, read-only mount, allowlists) fires
  before the first write and leaves the workspace byte-identical. An
  interrupted rename is detectable (health reports the dangling
  references as stubs) and completable by re-issuing the same command.
  Works on git-branch and folder-backed mounts; read-only mounts
  refuse. Previously the only path was a hand-built migration
  (measured at ~40 minutes for a 73-entity mem in the field).
- **The sizing curve is measured, not advertised.** A new on-demand
  harness (`cargo run -p xtask -- sizing-curve`) generates graded
  synthetic mem-repo workspaces through the product surface and times
  the four everyday cold-CLI operations (boot, update, search,
  overview) across 500–7,500 entities, writing machine-readable
  results (`sizing-curve/v1`). The committed curve
  (`docs/sizing-curve.md`) records the measured shape: on the cold
  path every operation costs what boot costs — load is the only
  visible cost and grows super-linearly (~0.36 ms/entity at 500 →
  ~0.75 at 7,500) — plus what that implies for the deferred lazy-mount
  / incremental-index / deferred-cross-mem redesigns. The MCP server
  instructions' "designed for 1,000–5,000 entities" statement now
  cites the measured document. The harness runs in temp directories,
  leaves no residue, and is not part of the default test suite.
- **`memstead export --format json` — the bulk read.** One CLI
  invocation, one engine boot, emits the complete non-stub entity set
  as a single JSON document on stdout (`format: "memstead-export/v1"`),
  grouped per mem: per entity the same structured envelope
  `memstead entity --json` produces (id, type, title, metadata,
  sections, relationships with edge source, `_hash`), per group the
  mem's schema pin, `read_only` marker, and entity count.
  Backend-uniform (git-branch and folder mems), observably read-only.
  `--mem <name>` selects one mem (read-only mounts included when named);
  omitting it exports every writable mem, read-mems excluded. External
  projections and check scripts consume this instead of per-entity CLI
  calls (which pay the engine boot each) or raw git against the
  mem-repo. `--output` combined with `--format json` refuses
  (`INVALID_INPUT`) — the document goes to stdout. Deliberately
  CLI-only: the MCP abstention for `memstead_export` stands (a bulk
  dump into an agent context is an anti-feature; agents use
  `memstead_search`/`memstead_entity`).
- **The structured entity envelope carries `title`.** `memstead entity
  --json` and MCP `memstead_entity` now surface the `# H1` display
  title as a top-level field next to `id`/`mem`/`type` — previously
  consumers had to parse the rendered markdown to recover it. Additive.
- **Health includes carry what health knows.** Three closures on the
  health surface. (1) `HealthIssue` gains a structured `code`
  (`MISSING`, `SECTION_HEADING_MISMATCH`, `UNDECLARED_RELATIONSHIP`,
  `INVALID_REL_SHAPE`) — the code stops being a message-string prefix,
  and both `missing_fields` projections (MCP composer and CLI) carry
  per-issue `issues[] {field, code, message}` additively beside the
  byte-identical legacy `missing` field-name array. A heading-mismatch
  finding can no longer surface under a bare "missing" label through a
  projection that drops messages. The Swift/UniFFI `HealthIssue`
  record carries the code too. (2) `config` joins the health include
  catalogue: `memstead health --include config` on the CLI and
  `include: ["config"]` over MCP render the same workspace-config
  projection `include_config: true` always served (one shared
  renderer in `memstead-base`; the boolean stays as a documented
  alias; passing both renders once). (3) The sealed-violator write
  guarantee — a mem pinned to a heading-round-trip-violating sealed
  schema serves writes, with the divergence warning and the persisting
  health finding — is now locked by a test, not by review.
- **The warm server picks up out-of-band installs.** `memstead_reload`
  (and CLI `memstead reload`) gain an additive full-refresh mode:
  `full: true` re-scans the schema sources and the mount manifest on
  top of the workspace-wide content reload, so a schema installed and
  a mem registered out of band become usable in the running process —
  the reported end-to-end blockage (install schema → `memstead_mem_create`
  pinned to it → write an entity) now closes with no restart and no
  operator action. Additive only, deliberately: removals (an
  unregistered/deleted mem, a schema version gone from a source) are
  SKIPPED and reported — they take effect on restart — because
  removing live state can strand entities and cached hashes the
  process is still serving. The response's `refresh` block says what
  changed (`schemas_added`, `mems_mounted`), what was skipped
  (`schema_removals_skipped`, `mem_removals_skipped`), per-item
  `failures` (a failed source or mount never surfaces as newly
  available and never aborts the rest), and `elapsed_ms`. Newly
  mounted mems load cold like any boot-time mount, with the
  workspace-global validation passes batched once per refresh. The
  default reload is byte-for-byte unchanged.
- **Cross-mem wildcard destination, bound to the alias rel-type.** A
  schema's `cross_mem_relationships` may now declare `to_schema: "*"`
  — restricted at load to the rel-type the schema names as its
  `alias_target_rel_type` (a wildcard for any other rel-type refuses,
  naming both; a schema without an alias target cannot use one). The
  binding is the safety argument: the author already permitted soft,
  auto-emitted references of that type, and the wildcard only extends
  that decision across the mem boundary — hand-authored structural
  edges keep requiring a per-destination-schema declaration, and the
  workspace `cross_mem_links` policy, target existence, and
  source-type gates all stay in force. One matcher serves edge
  validation, the load-path edge filter, and the per-edge-description
  posture lookup, so the wildcard is honoured identically at write
  and at reload (previously the store builder silently dropped such
  edges at boot). The built-in `ingest` schema moves to 0.2.0 and
  replaces its two hardcoded destination entries (`software`,
  `project`) with the wildcard, so a process mem links into its paired
  destination whatever schema that destination pins.
- **`SCHEMA_NOT_FOUND` tells the truth in its message.** An unresolved
  schema pin now summarises the resolution trail in the message
  itself — which sources were searched and what each held (e.g.
  `searched local_storage (nothing for "x"), builtin (holds 1.0.0),
  remote (not_configured)`) — so a right-name/wrong-version pin and a
  never-installed package are distinguishable without opening
  `details` (where the structured `sources` trail remains unchanged).
  When the pin fails while a loadable authoring package of that name
  sits in the workspace root uninstalled, the error names the fix:
  `memstead schema install <path>` (message and
  `details.install_hint`). A reported autonomous loop burned five
  rounds and a server restart on the old one-sentence message.
- **Health reports authoring drift for installed schemas.** `memstead
  schema install` from a package directory now stamps the sealed copy
  with the authoring path it was installed from
  (`install-provenance.json`, workspace-local — never exported into
  `.mem` archives). A health run then reports, per stamped pinned
  schema, when the authoring package is MISSING from the working tree
  (`SCHEMA_AUTHORING_SOURCE_MISSING`) and separately when it is
  present but no longer parses equivalent to the sealed copy
  (`SCHEMA_AUTHORING_SOURCE_DIVERGED`) — parsed-schema comparison,
  never raw bytes, so editor-header comment lines and serialisation
  cosmetics never trip it. Both findings participate in `health
  --strict` with no `--include` opt-in. Unstamped schemas — sealed
  before this change, built-ins, archive installs — produce no
  finding (no backfill: a guessed provenance is worse than an absent
  one). The CLI `health` command now also renders engine-level
  warnings (previously MCP-only), which is the surface this axis
  ships on.
- **Folder mems say what provenance means, at creation.** Creating a
  mem on storage without version control (a folder mount — via
  `memstead_mem_create`, `memstead mem init` with folder storage, or
  `memstead init`) now returns a `FOLDER_MEM_PROVENANCE` warning
  stating plainly: mutations ARE recorded in the changelog ledger
  (`.memstead/changelog.jsonl`) with their notes, but there are no
  commits, the returned `commit_sha` is a synthetic placeholder, and
  the content is not durable until the surrounding repository commits
  it. A warning, never a refusal; git-backed mems get no notice.
- **Batch parity: `memstead batch-create` and `memstead batch-relate`.**
  Creation and edge changes gain the atomic batch form updates already
  had, completing the CLI batch family. `batch-create --from
  <file.json>` takes a `creates: [...]` array of single-`create
  --from`-shaped entries (per-entry provenance `note`, no batch-level
  note flag) and creates all of them with one workspace load and one
  commit per touched mem. Intra-batch references resolve as REAL typed
  targets: every entity is staged before per-entry validation, so
  sibling edges get full target-type shape validation, duplicates
  within the batch are refused, and no transient stub or stub warning
  is created for a batch-supplied target — cycles included where the
  schema permits, turning the two-pass bulk-ingest workaround into one
  pass. `batch-relate --from <file.json>` takes a `relates: [...]`
  array mixing additions and removals, applied IN ORDER (a later entry
  sees an earlier entry's edge) with the same one-commit semantics.
  Engine-side, `create_entity` and `relate_entity` are split into the
  prepare/commit halves `batch_update` established, so the batch paths
  share every single-item validation gate by construction. The batch
  commands stay CLI-only (no MCP tool, no UniFFI surface — the
  existing surface test pins the MCP abstention).
- **Batch refusals now report EVERY failing entry.** The whole family
  (`batch-update` upgraded, the two new commands from birth) refuses
  all-or-nothing and names every failing entry with its index and
  typed code — bounded at 50 detailed envelopes with an
  `errors_suppressed` count beyond that (never silent truncation) —
  so one repair cycle fixes the file. Previously `batch-update`
  stopped at the first invalid entry.
- **Metadata values are findable.** The per-mem search index gains one
  tokenized `metadata` field per entity carrying that entity's
  metadata KEYS and VALUES — built from the entity, not the schema, so
  it needs no `filterable` declaration and works for undeclared
  fields. It joins the free-text query at a fixed weight below
  title/section prose, so identifier-shaped values (the motivating
  case: a search for `20/54/033` silently returned zero while an
  entity carried exactly that value) are found without enum/date
  tokens swamping prose ranking. A hit matched through metadata
  reports `field: "metadata"` in its matched-terms breakdown. The
  untokenized `meta_<key>` filter fields, the equality/range filters,
  and their four outcome codes are untouched; prose-only queries
  return the same entities in the same order.
- **A published mem carries its identity and its subject.** `MemConfig`
  gains an optional human-readable `title` (display text, not identity
  — the slug name stays the sole handle everywhere) and an optional
  three-member `subject` block: `scope` (what the mem covers),
  `method` (how its content was arrived at, optional), and
  `exclusions` (what was considered and deliberately left out — prose,
  order preserved, may be empty; exactly three members by design, so
  the block stays reviewable). Both publish verbatim in
  `PublishedMemConfig`; `write_guidance` and `rules` stay author-only.
  `PUBLISHED_MEM_FORMAT` moves 3 → 4; readers accept both via the
  shared `published_format_accepted` predicate (the two mems already
  on the live registry stay installable), writers emit 4, formats 1/2
  keep refusing. New `memstead mem set-title` / `mem set-subject`
  setters mirror `set-description` (empty clears; the subject clears
  as a unit). The mem roster, the workspace overview, and the health
  config projection prefer the title with the name kept visible as
  the addressable slug.
- **An anchor names the source that produced it.** Anchor records gain
  an optional `source` — the NAME of the producing binding's source
  entry — so a discovery run over a multi-source binding is measurable
  per entry point. Present-but-empty refuses `INVALID_ANCHOR`; a name
  the anchor's own (still-resolvable) producing binding does not
  declare refuses with the declared names in the recovery payload; an
  absent or unresolvable binding accepts any non-empty name
  (validation never requires the binding to resolve). The field rides
  every anchor-accepting surface through the shared `AnchorInput`
  (MCP `anchors[]`, CLI `--anchor` on create/update/batch-update);
  the build/one-shot and sync briefs' provenance block now lists the
  binding's declared source names and instructs setting `source`, the
  verify brief notes per-source measurability, and the sync skill's
  capability-gated anchor line names the entry point. Pre-existing
  anchors are never backfilled — an absent value is honest — and the
  sidecar loads unchanged (additive, no version bump).
- **Directional graph traversal.** `related_to` and `expand_via` gain a
  `direction` selector — `out` (edges pointing away from the seed:
  what does this rest on), `in` (edges pointing at it: what rests on
  this), `both` (default — the historical undirected walk, so every
  existing query returns exactly what it always did). The choice
  applies at every hop: depth > 1 is a pure transitive closure in the
  chosen direction, never a mixed walk. Present on the MCP
  `memstead_search` params, the CLI (`--direction`, which also gains
  the previously missing `--expand-via` / `--expand-depth` pair), and
  the UniFFI `SearchScope` record (defaulted — existing Swift call
  sites compile unchanged). Expanded hits report the reaching edge's
  traversal direction (`expansion.via_direction`, and `[out]`/`[in]`
  beside the label in the markdown channel). The dead undirected
  `reachable` walker was deleted; the two live walkers carry the
  selector.
- **Coverage semantics resolve per medium.** A binding can no longer
  assert exhaustive coverage over a medium whose ground set the engine
  cannot enumerate. `coverage_semantics` is now optional on the binding
  record — absent means "not stated", a different fact from "stated as
  exhaustive" — and the effective value resolves per binding (all
  sources on enumerable media → `exhaustive`; any non-enumerable
  source → `curated`, the weaker claim a mixed binding can honestly
  make). A declared `exhaustive` with a non-enumerable source is
  refused by `validate_binding` (typed, names the source, the medium,
  and `curated` as the remedy, reported alongside other refusals).
  `hash_binding` serialises the resolved value, never the `Option`, so
  enumerable-media bindings keep their hash byte-for-byte (findings
  survive) and a web-source binding that declared nothing rehashes
  exactly once. The fidelity report and status gates read the
  effective value; the report marks a resolved (undeclared) value so a
  reader never mistakes a resolution for an author's assertion.
  Legacy migrations now carry coverage through as unstated instead of
  baking in "exhaustive by silence"; the `projection init` scaffold
  asserts nothing.
- **CLI parity and surface honesty (four gaps closed).** (1) One JSON
  template feeds both `create --from` and `update --from`:
  `UpdatePayload` gains `note` (flag wins), `CreatePayload` gains
  `dry_run` (ORs with the flag), and each command *tolerates* the
  other's identity fields with consistency checks instead of silent
  drops (a template `id` must match create's derived slug; a template
  `title`/`entity_type`/`mem` must match the entity on update). The
  optimistic-locking selectors (`auto_hash`, `force`) stay flag-only
  by design — a stored payload must never disable locking. (2)
  `memstead search` gains a repeatable `--range-filter KEY=VALUE`
  using the MCP `range_filters` key grammar and the same engine path,
  so the four typed outcome codes reach the CLI with identical
  meaning. (3) A global `--workspace <path>` flag plus
  `MEMSTEAD_WORKSPACE` (flag wins, then env, then the upward walk)
  points the CLI at a workspace from any working directory; a
  marker-less override refuses naming the tried path and never falls
  back to the walk. The per-subcommand `--workspace` flags on
  `publish`/`link` were folded into the global one. (4) The title
  grammar is stated as a rule derived from the validator
  (`memstead_base::TITLE_GRAMMAR_RULE`): CLI create/rename help embeds
  it at build time, the MCP `memstead_create`/`memstead_rename`
  descriptions carry it verbatim (surface-test-enforced), and a
  conformance test binds the sentence to `validate_and_derive_slug`
  behaviour — covering the characters outside projects found by
  collision (`.`, `(`, `)`, `/`, `:`, em dash).
- **The schema response carries every legality condition on outgoing
  edges.** Both the full and lite `memstead_schema` projections now
  report each type's `required_outgoing` blocks (relationship
  alternatives + cardinality, declaration order; a type with no blocks
  reports an empty list, never a missing key), a top-level
  `propagating_relationships_effect` note states that field's single
  real effect (the self-loop relate refusal — outside schema authors
  had read impact-propagation or evidence obligations into the name),
  and the server-instructions legality-flag enumeration names
  `required_outgoing`. The long-advertised `MISSING_REQUIRED_OUTGOING`
  mutation warning is now real: create and update evaluate the entity's
  blocks once the mutation's edges are known — through the same
  evaluation the health sweep uses, so the two surfaces cannot
  disagree — and warn (never refuse) with the unsatisfied blocks and
  cardinality.
- **Schemas whose section headings cannot round-trip to their keys are
  refused at install.** The heading→key derivation (lowercase, spaces to
  underscores) now lives in one function (`derive_section_key`) shared by
  the entity parser and the schema loader, and a new installation-path
  check (`check_section_heading_roundtrip`) refuses any schema declaring
  a section whose heading does not derive back to its key — the defect
  class where a create writes one heading, a later update writes another,
  and the same file silently carries both while health reports the
  section *missing*. The refusal is typed
  (`SchemaLoadError::SectionHeadingMismatch`, surfacing as
  `SCHEMA_VALIDATION_FAILED` on the wire), names **every** offending
  `(type, key, heading, derived_key)` tuple, and states the fix. It fires on the authoring/installation surfaces only —
  CLI `schema validate`, CLI `schema install`, and the engine's
  `install_schema` primitive (which now validates packages before
  sealing, including a manifest-identity-vs-install-ref check) — never
  on boot, so a schema already sealed into a mem-repo keeps loading.
- **Health distinguishes "section absent" from "content under a
  non-deriving heading".** A sealed schema that violates the round-trip
  rule surfaces per mem as a `SCHEMA_HEADING_ROUNDTRIP_VIOLATION` load
  warning (boot succeeds; the finding lists every offending tuple), and
  an entity whose declared section content sits under the non-deriving
  declared heading gets a distinct `SECTION_HEADING_MISMATCH` health
  issue naming both the found heading and the catch-all the content
  landed in — instead of the misleading "required section is empty"
  report that sent the plenum operator hunting for content that was
  present all along. The parser now retains the file's literal `## `
  heading list per entity (a derived, never-persisted parse artefact)
  to power the distinction.
- **Mutations warn on section-heading divergence.** An update writing a
  section whose declared heading differs from a heading the file
  already carried for the same key emits `SECTION_HEADING_DIVERGENCE`
  naming both headings; the write still commits (refusing would strand
  entities written before the gate existed) and the regenerated file
  carries the declared heading.

### Changed
- **A guard that exists on one write path exists on all of them.** Two
  closures of the same defect class. (1) The cycle family — the
  self-loop refusal on propagating rel-types and the
  `RELATIONSHIP_CYCLE` refusal on acyclic ones — previously ran only
  on `memstead_relate`; the same illegal edge written through
  `memstead_create.relations[]`, `memstead_update.declare_relations`, or a
  batch landed on disk and was then silently dropped by the next
  boot's cycle sweep, announced only as a boot warning the writer
  never saw. All edge-writing verbs now run one shared gate
  (`validate_edge_acyclicity` — one owner, no per-path copies) with
  identical codes and recovery detail; batch-create stages each
  item's edges so an intra-batch cycle refuses exactly like a stored
  one; relate's behaviour is byte-identical. The boot sweep stays as
  the last-resort net for pre-existing data, and its coverage comment
  now tells the truth. (2) `set_mem_schema` was the only lifecycle
  setter without the read-only-mount capability gate — a schema-pin
  change (which starts a migration) was the one lifecycle mutation a
  sealed mount could not refuse. It now refuses `READ_ONLY_MOUNT`
  exactly like its six siblings, and a family-level test enumerates
  all seven so an eighth setter cannot ship ungated.
- **Reserved metadata keys are reserved everywhere.** The engine's
  identity/discriminator triple (`type` / `mem` / `id`) now has one
  reservation with one behaviour across every path. The loader's
  reserved set widens from `type` alone to the full triple, enforced
  on the authoring/installation path (`memstead schema install` /
  `validate`, the engine install primitive) with the typed
  `ReservedSchemaKey` refusal — and, matching the heading-round-trip
  posture, no longer refuses at boot: a schema already sealed that
  violates the rule keeps loading instead of bricking the workspace.
  The create path now refuses a caller-supplied reserved key with the
  same deliberate `READ_ONLY_FIELD` the update path uses, instead of
  the incidental `UNKNOWN_METADATA_FIELD`. And `metadata_unset` may
  now name a reserved key — the sanctioned repair for an entity that
  acquired a smuggled one before the write gates closed (previously
  only delete-and-recreate, destroying provenance and edges);
  removing a reserved key can only move an entity toward the
  invariant, and unsetting `type` never leaves an entity typeless —
  the engine re-seeds the authoritative discriminator, so on a
  healthy entity it is a no-op. Setting a reserved key stays refused
  everywhere; create's stamp-and-proceed posture for engine-managed
  timestamp fields is untouched.
- **Anchors merge; they no longer silently replace.** An update carrying
  `anchors` used to discard the entity's entire prior anchor set —
  observed live as a sync loop regressing an entity's coverage because
  each later batch displaced the earlier one. Anchor writes now
  **merge**: an incoming anchor replaces the existing anchor with the
  same `(artifact, grain, class)` triple and appends otherwise, so
  incremental anchoring works and writing never removes an anchor the
  call did not name. Removal is explicit via the new `anchors_unset`
  on the update surface (MCP `memstead_update`, CLI `--anchor-unset` /
  `--from` payload / batch-update entries): each selector names an
  `artifact` and may narrow by `grain` and/or `class`; a bare artifact
  removes every anchor on it; unset applies before the merge in the
  same mutation; unsetting a nonexistent target is an idempotent no-op
  (the `metadata_unset` / `relations_unset` conventions). Full-replace
  stays expressible — unset the artifact(s) and write the new set in
  one call. Re-sending an entity's full current set produces exactly
  the same stored state as before; an empty or absent `anchors` list
  remains a no-op, never a prune. The sidecar format, `INVALID_ANCHOR`
  validation, and the `_hash` exclusion are unchanged.
- **The lean MCP surface's tool descriptions go through the same lint suite
  as the full server's.** The `tool_surface` description lints (lead-verb
  allowlist, word/byte bounds, TODO markers, backtick-reference resolution
  against the wire schema) covered only the mem-repo `McpServer`; the lean
  `FilesystemMcpServer` descriptions were unlinted and had drifted. Both
  flavours now lint identically, and the drift the first sweep caught is
  fixed: lean leads align with their full-surface counterparts
  (`Remove`/`Connect`/`Return`/`Start`), the lean `memstead_delete`
  description no longer implies a per-call `force` parameter the surface
  doesn't have, the internal kernel symbol `compute_health` no longer leaks
  into the `memstead_health` contract, and the too-thin lean
  `memstead_entity` description now documents `sections`,
  `include_relations`, and `include_context`.
- **The `propagating_relationships` deprecation pointer names its
  successor.** The effect note (both verbosity levels) now points
  schema authors at `status_propagation` — the constraint that
  carries the real propagation semantics — alongside the honest
  single-effect (self-edge refusal) statement.

### Fixed
- **`--force-overwrite`'s documentation stops lying.** The CLI flag
  help and the MCP `memstead_mem_create` recovery-parameter
  description both claimed the destroy-and-recreate recovery path was
  "not yet implemented" — it has been implemented and tested for some
  time (residual branch + config blob pruned in one ref-edit
  transaction, then the normal create path). Both surfaces now state
  the real behaviour; the flag and recovery value are unchanged.
- **Plugin guard hooks now block with their designed message.** The
  entity-edit, entity-bash, and ingest deny hooks wrote their
  `BLOCKED: …` reason to stdout — but Claude Code's exit-2 hook
  contract feeds **stderr** back to the agent, so the agent saw an
  empty "hook error" instead of the message (the blocks themselves
  always held). The reason now lands on stderr, unchanged in wording.
  A new invocation self-test executes all four `hooks.json` command
  strings exactly as written (shell + `CLAUDE_PLUGIN_ROOT` env) with
  violating and benign inputs, so neither the command strings nor the
  message channel can rot unobserved again.
- **The engine stops losing events and moving hashes on wall-clock
  boundaries.** Three determinism defects, each fixed at the
  mechanism. (1) Folder-mem drift cursors strictly advance per
  commit: the changelog append clamps a same-millisecond or
  backwards timestamp to `last + 1ms`, so two commits inside one
  millisecond no longer share a cursor and the second `MemChangedEvent`
  is no longer swallowed by the self-write dedup. Format and the
  lexicographic cursor dialect are unchanged. (2) An anchor-only
  update no longer auto-stamps `last_modified` — the documented
  "anchors never move `_hash`" contract now holds across second
  boundaries, so refreshing anchors never invalidates a cached
  `expected_hash`. (3) Mutation timestamps read an engine-owned
  injectable clock (`Engine::set_mutation_clock`, default system
  clock; stamped format unchanged) — a testing seam that lets
  canonical-byte assertions pin time instead of loosening, closing
  the cross-surface hash-parity flake.
- **The `planning` built-in's `goal` type round-trips its scope
  sections.** `scope_in`/`In Scope` and `scope_out`/`Out of Scope`
  violated the round-trip rule — content written under those headings
  fell through to the catch-all on re-parse. The keys are now
  `in_scope`/`out_of_scope` (headings unchanged), in the built-in and
  its seed-fixture mirror.
- **The legacy `@scope/name` rejection is typed.** The CLI's
  legacy-form refusal leaked code `INTERNAL` through a bare error on
  a user-triggerable path; it now refuses `INVALID_INPUT`. And the
  `READ_MEM_SHADOWS_WRITABLE` recovery prose cited the removed
  `--mem` host flag — it now names the real recovery (mem rename /
  unregister).
- **Mem curation text is visible where mems are listed.** The
  description and subject scope now render on `memstead_overview`'s
  mem roster (Description/Subject lines, only when set) and
  `memstead mem list` appends the description to each row —
  previously two of the three curation fields were writable over MCP
  but invisible on the roster, readable only via a configure
  no-field read-back.
- **The independence gate stops manufacturing identity from
  transport.** The checks axis' author≠checker gate compared the
  recorded `(actor, client)` pair — but that pair names the SURFACE a
  record arrived through, not who acted, so CLI-authored +
  CLI-checked always read `self_checked` even across sessions (false
  conviction, the norm) and a cross-surface author/check read
  `confirmed_independent` (false acquittal via transport). Until a
  caller-declared identity exists, no author/checker comparison can
  be established: every ok-checked entity with recorded provenance
  now lands in `unconfirmable`. `self_checked` and
  `confirmed_independent` stay in the wire shape as explicit empties
  — categories awaiting the caller-identity substrate.
- **Health markdown parity: the text channel says what the JSON
  says.** Both markdown renderers — the MCP text channel's and the
  CLI's — lacked sections for three JSON payloads: `checks` (state
  counts plus the independence gate), `stale_derivations`, and the
  ungated `quarantined` roster (per mem the reason code and the
  message carrying the repair command). Both renderers now render all
  three with consistent wording, following the null-is-a-statement
  pattern: a requested-but-empty axis renders its explicit zero line,
  an absent key renders nothing, and without the includes the output
  is byte-unchanged.
- **A rehearsed relate no longer claims it created a stub.** The
  dry-run relate path reused the real path's `AUTO_STUB_CREATED`
  message ("stub auto-created") although rehearsals write nothing.
  The warning code is unchanged (response-shape stability); the
  message now branches — the rehearsal says the stub "would be
  auto-created by the real call" and names the two follow-ups
  (promote via `memstead_create` first, or let the real call stub).
- **`schema install <builtin>@<version>` resolves every retained
  version.** The collect path looked up the built-in by its
  name-exact directory alone, so any version living in a suffixed
  retention sibling (`planning-0.3`, …) refused `SCHEMA_NOT_FOUND`
  even though the registry registers all retained versions. The path
  now falls back to scanning the embedded catalogue for the (name,
  version) the ref pins — identity from each package's manifest, the
  directory name stays organisational — and an unregistered version
  still refuses.

## [0.4.0] - 2026-07-20

### Changed
- **VISION states what the project's own evidence supports.** The "Core value
  proposition" led with a read-side claim — an LLM reading well-structured
  specs "understands a domain immediately … no guessing" — that the project's
  controlled substrate evaluation (`docs/proof/substrate/`) falsified: against
  equally-curated free-form notes, schema-forced typing showed a signed
  answer-quality delta of ≈ −0.010 ± 0.006, and the observed token saving
  traces to *curation*, which flat notes share. VISION now states that null
  result explicitly and sells only what the mechanism supports: enforcement on
  write, determinism (no model in the query path), git-native accountability,
  ownership, and packaging. Token efficiency is no longer named as a founding
  problem (integrity replaces it), the 5M-entity federation passage reads as an
  open bet naming the unbuilt Indexed Mem tier rather than an achieved
  capability, and the horizon publishing example claims publisher
  accountability instead of avoided scraping. Documentation only — no engine,
  schema, or MCP behaviour changed.
- **The divergence proof package is leak-free and its status is true.** The
  pre-registration package under `docs/proof/divergence/` had never passed
  `scripts/leak-scan.sh`: it embedded an absolute developer home path in a
  captured MCP config and pointed public readers at a planning path that exists
  only in a private repository. Both are repaired (the capture config is now
  repo-root-relative, so the recorded launch command is machine-independent),
  and the package's `Status` no longer claims "no campaign has run" while
  `state.json` records ten completed rounds. Recorded in-place as amendment A5
  per the package's own amendment rule; no pre-registered parameter, prompt,
  band, query, rubric, slice, or model pin was touched, and no result was
  recomputed.
- **BREAKING: one record per pipeline (binding format v2).** The pipeline
  configuration is consolidated into a single versioned record at
  `.memstead/projections/<mem>/<name>.json` (`version: 2`): the standalone
  `mediums/` and `facets/` record kinds are retired, their content folded
  into the binding's inline `sources[]` — each source carries the medium
  half (`type` / `pointer` / `change_detection`) and the facet half
  (`scope` / `engagement` / `preparation`) under the facet's name verbatim,
  so per-source sync watermarks keep resolving. The engine reads **only**
  v2: a pre-v2 store (version-less gen-2 projection or v1 three-file
  binding) refuses at load/boot with a typed error naming
  `memstead projection migrate`, which now converts every prior on-disk
  generation in place (folding medium+facet content inline, removing the
  emptied `mediums/`/`facets/` trees, refusing on orphan records rather
  than dropping them; idempotent on a migrated store). The edit surface is
  projection-only everywhere — the eight medium/facet CRUD methods are gone
  from the engine, UniFFI, and wire surfaces; the cross-record dangling-
  reference error class is gone with the references (in-record source
  validation replaces it: empty/duplicate source names refuse typed).
  `hash(D)` now derives from the record alone, so pre-consolidation verify
  findings are invalidated by construction (re-derivable measurements).
  `Engine::pipeline_configs_json` returns `{ "bindings": [...] }` only.

### Added
- **Per-binding verdict on the projection status drill-down.** Each
  `ProjectionStatus` entry now carries its own resolution — `verdict`
  (`clean` / `onboarding` / `action-needed`, kebab-case like the rollup's),
  `source_moved`, and `findings` counts by class (`unresolvable` / `drifted` /
  `uncovered` / `queued`) — computed by the SAME scan the workspace rollup
  aggregates, so status consumers (the app's Pipeline tab, agents reading the
  HTTP status picture) never re-derive verdicts client-side. Additive wire
  fields; the rollup's semantics are unchanged and now provably shared (one
  scan, two projections).

### Fixed
- **MCP contract now states the true `require_notes` semantics.** The server
  instructions and the `memstead_create` tool description claimed a missing
  note *refuses* with `NOTE_MISSING` when `[mutations].require_notes = true`;
  the engine has always warned and committed ("the policy nudges, it never
  blocks" — behavior test-asserted, and the CLI help said so correctly). The
  descriptions, the pinned instruction copy in the tool-surface suite, and the
  generated MCP reference now say **non-blocking warning**. Contract text only
  — no behavior change on any surface.

### Changed
- `/sync --all` under a recurring loop now **ends the loop on quiescence**:
  a second consecutive nothing-due rotation means the catch-up job is done —
  the skill cancels the schedule driving it and reports quiescence, instead
  of ticking no-ops forever. A standing watch is a deliberate restart at a
  slower cadence. Matches the operator mental model "run until the graph is
  back in sync, then stop".

### Added
- **Per-entity history: `Engine::entity_history`.** Given a mem and an
  entity id, one query returns the entity's recorded story — every
  touch newest-first with when, provenance (actor / client / tool
  verb), and the agent's stated note — with rename chains followed
  through the engine's own rename provenance, so the story starts at
  the entity's first appearance under any prior id, and batch commits
  visible with their batch context (the other ids they touched) without
  polluting those entities' own stories. Bounded and pageable (default
  50 / cap 200, opaque continuation cursor; pages compose without gaps
  or duplicates) and honest about its edges: `story_start` states where
  and why a story truncates (unstitchable rename, records predating the
  changelog), `limitations` names per-backend gaps (folder changelogs
  record renames under the post-rename id only and carry no batch
  attribution), and refusals are typed — `UNKNOWN_MEM`,
  `ENTITY_NOT_FOUND` (an unknown id is never an empty story),
  `INVALID_CURSOR`, and `INVALID_INPUT` on archive mounts (their seam
  records no history; refusing beats fabricating emptiness). A reused
  slug never absorbs the previous holder's story — the walk stops at
  the entity's own creation, so an id freed by rename or delete starts
  a fresh narrative. Built entirely on the existing walks (the
  git-branch commit-note feed, the folder/in-memory provenance log) —
  no new storage, no index.
- **Review marks: one per-mem pointer to the last human-approved state.**
  `MemConfig` gains `reviewMark` (mem-repo state — every sibling process
  sees the same mark; stripped from published archives by the
  `PublishedMemConfig` allowlist), carried in the backend-opaque
  `changes_since` cursor vocabulary. Three engine ops:
  `Engine::review_marks` (every mem's mark + current head),
  `Engine::set_review_mark` (explicit target only, validated per backend
  — garbage SHAs and malformed timestamps refuse `INVALID_CURSOR`;
  clearing is first-class; provenance and require-notes mirror
  `set_mem_sync_state`), and `Engine::review_mark_diff` (the accumulated
  delta since the mark; markless mems refuse with the new
  `REVIEW_MARK_NOT_SET` instead of silently equating "no mark" with "no
  changes"). Marks never gate writes — no mutation path consults them.
  Surfaces: the CLI gains `memstead review-mark list|set|clear|diff`
  (same cursor vocabulary as `memstead changes --since`; `set`/`clear`
  are note-gated warn-and-commit like every mutation), and the overview's
  `## Mems` roster carries a `Review mark` line for marked mems — the
  mark value plus a head-moved indicator, naming `changes_since` as the
  delta read, so agents see review state at cold-start without a new MCP
  tool. Markless mems stay unmarked in the roster (ordinary state, never
  flagged).
- **The mem-change event channel now carries sibling-process writes.**
  `reload_if_stale`'s drift arm emits the same `MemChangedEvent` the
  self-write path always emitted, so a broadcast subscriber (SSE
  forwarders foremost) sees every change to a mem — this engine's own
  commits and out-of-band siblings alike. Previously the channel was
  self-writes only, documented as such; no wire-shape change.
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

[Unreleased]: https://github.com/memstead/memstead/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/memstead/memstead/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/memstead/memstead/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/memstead/memstead/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/memstead/memstead/releases/tag/v0.1.0
