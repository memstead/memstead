# Changelog

All notable changes to Memstead are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- **A short entity id names its missing prefix.** Every id-taking
  mutation verb (`update`, `delete`, `rename`, `retype`, `relate`, the
  batch forms, and their MCP tools) given a bare slug without the `mem--`
  prefix now resolves it when exactly one mounted mem carries an entity
  of that slug, announcing the resolution as `SHORT_ID_RESOLVED` on the
  response, and otherwise refuses `ENTITY_ID_MISSING_MEM` naming every
  full id that carries the slug (`details.candidates`, empty when none
  does). Before, a bare slug reached the verbs as an id whose mem was
  the empty string, and the caller was told a mem called "" did not
  exist. One resolver in the engine carries the rule; a full id takes
  the path it always took, byte for byte.
- **An ambiguous artifact id is refused with both names.** On a binding with
  several primary sources, a source-relative id carried by more than one of
  them (`docs/a.md` under two pointers) now refuses the whole
  `projection exclude` call with `PROJECTION_EXCLUDE_AMBIGUOUS_ARTIFACT`,
  naming every canonical id it could denote in the message and under
  `details.ambiguous`, and recording nothing. Before, the fold took the first
  source that matched, so the source listed earliest in the binding silently
  won and the exclusion landed on an artifact the caller never named. Either
  canonical (workspace-relative) id is the unambiguous recovery. A binding
  with one source is unaffected in every response. The cross-source rule now
  sits beside the within-source resolver it is often confused with: the
  latter settles which reading wins under ONE pointer and has no opinion on
  two sources carrying the same relative path.
- **Coverage drops the artifacts you excluded on purpose.** After a
  `projection exclude`, the fidelity report no longer lists the excluded
  artifact under `coverage.uncovered` or in the "Uncovered artifacts"
  section: it leaves that set and is counted beside the figures as
  `coverage.excluded` (rendered as `uncovered (no anchor): N; excluded on
  purpose (not owed): M`), so the number a reader or a gate sees is the
  number still owed. Before, an artifact ruled out on purpose kept showing
  up as uncovered and kept being counted. Dropped rather than marked,
  because a reader counts the array. The reasoning stays visible in the
  "Excluded on purpose" block and in `disposed_excluded_rationales`, the
  findings axis is unchanged, and a binding with no exclusions renders
  byte-identically to before: both new clauses are gated on there being an
  exclusion. The subtraction that used to happen in the renderer is gone,
  since the array now arrives net. The fidelity-contract reference states why
  coverage drops what the anchor population names in place.
- **A bare `projection verify` no longer advances the `#verified` baseline.**
  The freshness token the selection loop reads now moves only under a new
  `projection verify --advance` flag, so a gate or a grader that verifies in
  order to READ leaves the destination mem's config byte-identical. Before,
  every completed run bumped it, and the surface's read-only claim was false.
  A run without the flag says so on the report ("Baseline not advanced") and
  carries `advanced: false` in the JSON envelope, so an empty
  `verified_baseline` is never mistaken for a run that had nothing to record.
  Two writes deliberately stay ungated, because neither is a freshness claim:
  the findings store, which is the verify surface's own state outside the
  mem, and the prepared-hash backfill onto hash-less anchors, which is
  measurement machinery. Gating that backfill too was tried and reverted on
  the evidence: withheld, an anchor never leaves `recheck`, and seven
  projection tests went from reporting drift to reporting clean. The `/sync`
  skill's `--verify` and `--inventory` recipes pass the flag, so the token
  keeps a producer now that a bare run no longer writes it; the verify-in-CI
  guide, which documents the gate case, states the opposite and says why.
- **The short-id rule reaches `retype` and the batch renderer.** `retype`
  with `--auto-hash` or `--force` read the id the caller typed in its hash
  preflight and refused a bare slug with `ENTITY_NOT_FOUND` before the
  engine resolver ran; it now routes that preflight through the same seam
  `update`, `delete` and `rename` use. The batch commands' markdown render
  gained the warnings line their `--json` envelope already carried, so a
  `SHORT_ID_RESOLVED` announcement in a batch is no longer JSON-only.
- **`rename` and `delete` print their warnings on the human surface.**
  On a mem-repo workspace both verbs rendered their markdown outcome
  without the `- Warnings:` line every sibling verb carries, so a
  `NOTE_MISSING` hint (and, since the short-id rule landed,
  `SHORT_ID_RESOLVED`) reached only `--json` callers. Both now render the
  same line as `update`, `retype` and `relate`; the JSON envelope is
  unchanged.
- **The lite schema skeleton carries a field's `value_pattern` and a
  type's `last_resort` flag.** The `memstead_schema` lite reply (and the
  CLI `type` markdown that shares the renderer) now renders a metadata
  field's declared pattern under `pattern`, the key the full reply already
  used, and a type's `last_resort: true` beside `leaf`, so an agent that
  plans a write from the skeleton sees every constraint the engine
  enforces (`INVALID_FIELD_VALUE` on a pattern miss) and which type the
  schema names as its fallback. Both keys appear only where a schema
  declares them; a schema declaring neither renders byte-identical to
  before. software@0.5.0 is the first built-in to carry both.
- **A completed schema migration re-stamps the mem's mutation stamp.**
  `memstead mem set-schema` / `memstead_mem_set_schema` now re-stamps the
  engine-owned mutation stamp (the marker the `ENGINE_VERSION_SKEW` hint
  reads) with the target generation when the switch completes, so a
  reader of the marker after a migration sees the generation the mem now
  sits on instead of the one the last entity write validated against. A
  dual-pin entry leaves the marker on the old generation, and the
  response's new `stamped_schema` field names what the marker carries
  after every call. The stamp is now readable without opening the config:
  `memstead mem list --json` carries `mutation_stamp` per mem, and the
  overview's mem entry renders a `Last mutation` line when a stamp exists.
- **The verdict-coverage line files advisory axes under a name that
  says so.** Every verdict surface (`health`, `overview`, `status`,
  `verify-anchors`, `projection verify`, `workspace dump`, and the MCP
  `verdict_coverage` / `_verdict_coverage` stamps) now renders three
  buckets: `examined` (the verdict answers for the axis), `advisory`
  (the surface renders the axis, always or on request, beside the verdict
  and never folds it in), and `not_examined` (the surface never looks at
  the axis; another surface answers for it). Health's stale, conformance,
  check-state and the other rendered-but-advisory axes read `advisory`
  where they read `not_examined`, which every independent reader had
  taken as "not looked at"; the examined set, the promotion of `anchors`
  on include, and every verdict are unchanged. The coverage registry
  refuses an axis filed in two buckets, and the health and MCP references
  state the three buckets. The CLI markdown now prints the line the
  composer stamped instead of re-rendering the static declaration, so
  `--include anchors` promotes `anchors` into `examined` on the markdown
  exactly as the JSON and the MCP payload of the same run do.
- **The parse-generate fixpoint holds when a merged section ends inside
  an open HTML block.** A non-schema section whose content ends inside an
  HTML block of the kinds no blank line ends (`<!X`, `<!--`, `<?`,
  `<![CDATA[`, a `<script>`-family tag) is merged into the catch-all in
  front of later pieces; the open block hid every fence in those pieces
  on the next parse, so a `## ` line a fence had masked in situ surfaced
  as an empty non-schema heading and was dropped a round later, and a
  re-save of an unchanged file produced a diff. The merge's incremental
  context close and the generator's part close now terminate such a
  block with its own end line (`>`, `-->`, `?>`, `]]>`, `</script>`),
  verified against the CommonMark referee the way fence closers are;
  balanced content is untouched. Found by the long-tier fuzzer on the
  0.17.0 release readiness run; the artifact is pinned in the shared
  corpus (`crash-1233c134…`) and a reduced fixture in the parser tests.
- **The wasm health surface no longer traps once the stale axis defers to
  anchor state.** The engine's default mutation clock was
  `SystemTime::now`, which is unimplemented on `wasm32-unknown-unknown`
  and traps the instance; every stamping path and, since the stale axis
  reads anchor state, every `health()` call reached it. The default clock
  is now `wall_clock_now()`, which derives the instant from the JS-backed
  clock on wasm and from `SystemTime::now` everywhere else, so the same
  ISO stamps and the same anchor adjudication come out on every target.
  The wasm test suite's health test (F11) is the regression guard.
- **`projection exclude` resolves the artifact id and refuses an unknown
  one.** An exclusion id written in the source-relative form (relative to
  the source's pointer) is resolved through the binding's source join at
  exclude time, the way the anchor write gate resolves an artifact path,
  so it names the same artifact as the workspace-relative form the report
  uses; the ledger holds the one canonical id (a ledger written before
  keeps working), and the response lists each requested id beside the
  canonical id it was recorded as. An id that resolves to no artifact of
  the binding's source refuses the whole call
  (`PROJECTION_EXCLUDE_NOT_SOURCE_MEMBER`) naming the nearest known ids in
  the message and under `details.nearest`, never recording a no-op.

- **An anchors-only update replaces the row on every mount kind, and
  says so.** `memstead update --anchor` (and `memstead_update` with
  `anchors`) naming a stored (artifact, grain, class) triple replaces the
  row on folder and git-branch mems alike: rewritten hash-less for the
  next verify to backfill when any supplied field differs or the update
  also changed the entity's content (the sync brief's one-update repair,
  which used to keep the old baseline and read `drifted` against the
  content it had just repaired), and a truthful no-op when the row
  restates what is stored (nothing written, no commit, `UPDATE_NOOP`).
  Every update response that carried anchors now says `anchors_changed`.

- **The health mem filter applies to every section and warning.** Under
  `health --mem <m>` and `memstead_health` with `mem`, one rule scopes the
  whole report: the anchors section, the folder-ledger map and the per-mem
  config entries now carry only the named mem (they used to list every
  mem), and every warning is kept only when it concerns that mem, the
  folder-mem out-of-band notice included (a warning attributed to no mem,
  a request notice or a workspace-level condition, concerns every mem and
  stays). Only the mem rosters and `default_writable_mem`, workspace
  facts, stay global. Without a filter the report is
  byte-identical to before. The CLI markdown also files consistency-axis
  rows (the `integrity` include) under their own "Consistency findings"
  heading instead of "Conformance findings".

- **The integrity axis blames no grant after an unmount.** A cross-mem
  edge whose target mem is not mounted is reported once, as the dangling
  finding (`DANGLING_RELATION_TARGET_MISSING`, emitted even when the
  target lingers as a load-time stub of the vanished mem, which the
  dangling-link collector alone kept silent about), and never as
  `CROSS_MEM_EDGE_UNGRANTED` while the grant table still names the pair:
  the grant check now distinguishes "no grant declared" from "target not
  mounted" and only the first carries the re-grant repair. A pair with no
  grant and a mounted target reads exactly as before.

### Added

- **The stale axis defers to anchor state.** An entity carrying at least
  one adjudicated hash-bearing anchor reads by its anchors instead of the
  wall clock: `resolves` keeps it off the stale list (and lists it under
  `anchor_fresh` when the day threshold would have named it), `drifted`
  and `recheck` list it as their own condition whatever its age, each
  such row carrying `clock: anchors` and its `anchor_state`; entities with
  no adjudicated anchor keep the `staleness_threshold_days` reading, their
  rows byte-identical to before, and an anchor-less workspace renders
  unchanged. Derived at read time from the same verification the anchors
  axis runs; nothing stored. `MEMSTEAD_TODAY=YYYY-MM-DD` pins the health
  clock for fixtures, honoured by the CLI and the MCP server alike.

- **`software@0.5.0`: a code mem says only true things.** A new built-in
  generation (0.4.0 stays byte-identical): `contract` gains the `protocol`
  value `engine_state` and an optional `version_axes` field (a csv list of
  `name=constant` pairs) for the durable state files an engine owns and
  later engines must read back; `spec` gains an optional `notes` catch-all
  section for standing remarks (a frozen spec's historical-record marker,
  what superseded it) so Identity stays one sentence of current state, and
  declares `last_resort: true` so the vital-signs axis reads the type-share
  signal on every software mem. The guidance now says what a code-projected
  mem can hold: `spec` is the home type for a surface there and dominates
  by design (the signal worth watching is a cluster with no non-spec entity
  beside its specs), `requirement` belongs to mems with a normative source
  and is absent from a code mem by design, and the failure-mode list names
  only relationships the vocabulary carries.
- **A metadata field declares the shape of its values.** `value_pattern`
  on a metadata field is a regular expression every written value must
  match in full, member by member on a `csv_array` field; a malformed
  value refuses `INVALID_FIELD_VALUE` naming the member and the pattern,
  and a pattern that does not compile refuses at install
  (`InvalidFieldPattern`). The schema render and the MCP skeleton show it
  as `pattern`.

- **A type declares what would resolve its open entities.** The schema
  language gains one optional key on a type, `resolution:` (`condition_section`,
  optional `status_field` with `open_values`, optional `check_kind`), in the
  mould of `due:`. With it the `open_questions` health axis lists, per mem,
  the open entities whose condition section is empty (`resolution_missing`)
  and the open entities whose condition nobody has checked under the
  declared kind (`resolution_unchecked`, from the check ledger at the
  entity's current content, `x-` kinds admitted by name); a type without a
  status field is open in every entity, so a criterion's assertion can be
  its own condition. A reading only: the write path refuses nothing new and
  the engine never judges a condition. The loader validates the declaration
  at install (`InvalidResolutionAxis` naming the offender), and a schema
  package carrying the key refuses at parse on engines before this one, as
  every new key does (format addition; `deny_unknown_fields` intact).

## [0.17.0] - 2026-09-02

### Added

- **The due brief reads `overdue`, and `project@0.5.0` puts milestones on
  the due axis.** `memstead due` names, beside the entities due inside the
  window (`due_soon`, with the days until), every open entity whose
  declared due date has gone by (`overdue`, with the days past); the JSON
  envelope carries both lists as data (`overdue`, `due_soon`, `mems`,
  `through`) beside the prose, each row the entity, its date and status
  and the quoted lead section, no severity and no recommendation. The
  reading rides a type's `due` declaration: the `obligation` built-in has
  it already, and the new `project@0.5.0` generation declares it on
  `milestone` (`target_date` over `status`, open on `planned` and `active`,
  quoting the blockers), so an overdue milestone that was edited yesterday
  finally reads as what it is; the stale axis keeps measuring edit
  recency alone.

- **`engineering@0.4.0`: the decision type sanctions the dated in-place
  amendment.** A later change that keeps a decision's meaning (a corrected
  count, a moved path, a later confirmation, a figure re-measured at a newer
  release) is recorded in place as a dated sentence or paragraph opening
  `Corrected <date>:` or `<date> amendment:`; a change of meaning still
  means a new decision with `SUPERSEDES` and the old body left as history.
  The 0.3.0 rule forbade every in-place edit while the house practice was
  dozens of dated amendments, so the rule was one nobody could enforce. A
  new generation under the append-only rule (0.3.0 is byte-identical in the
  binary); the migration is a pin move, entities that already practise the
  form validate unchanged.

- **`memstead export --format mem` redacts private patterns in the
  archive's authoring provenance.** Every mutation rationale that ships in
  `.memstead/provenance.json` passes through the engine's redaction
  vocabulary: each matched span becomes `[redacted:<class>]` and the
  record keeps its shape and its other fields (redact, never strip). The
  classes are the leak scan's seven `scan` lines, label and pattern
  verbatim, held equal by a test (`ops::redaction`) so a class added to
  one without the other fails naming it. The export report and its JSON
  count redactions per class (`redactions`); the bytes export carries the
  same report (`Engine::export_mem_bytes_report`). Entity bodies are not
  rewritten: the leak scan keeps guarding them, and an archive whose
  bodies carry a private string still refuses at the seal gate.

- **`memstead health` builds its report through the engine's shared
  composer, and takes `--mem`.** The CLI no longer assembles the health
  axes itself: it calls the same `compose_health` the MCP `memstead_health`
  tool runs (now in `memstead-base`, so the lean CLI build shares it too),
  so `memstead health --json` is byte-identical to the tool's
  `structured_content` for every include key and under a mem filter
  (`--mem <name>`, new on the CLI; an unknown name refuses with
  `UNKNOWN_MEM` naming the writable roster). The markdown rendering is
  unchanged and pinned by recorded fixtures; `--strict` reads its Tier-2
  violations off the composed report. The mem-scoped `_mem_schema` anchor
  is set by the composer, so both surfaces carry it.

- **`memstead push --all` publishes the whole mem-repo.** Every mounted
  git-branch mem's declared branch plus the mem-repo's `__MEMSTEAD` ref
  (schemas and mem configs, which the single-mem verb had no route for),
  fast-forward only: one `ls-remote` decides which refs lag, a ref already
  at the remote's SHA is skipped silently, one line per ref moved
  (`<ref> <previous> -> <new>`), so a run with nothing to publish prints
  nothing and exits 0. A ref that cannot fast-forward is refused by name
  (`NON_FAST_FORWARD`, the mem named) while the other lagging refs still
  go, and the run exits non-zero at the end with every refused and pushed
  ref under `details`. `--force` stays on the single-mem verb and is
  refused beside `--all`; folder and archive mounts have no branch and are
  skipped. Engine: `Engine::push_all` over two new transport primitives
  (`ls_remote`, `resolve_ref`) on the git-branch ops table; the MCP
  surface is unchanged.
- **An unreadable anchors sidecar is a typed condition on every read
  surface, never zero rows.** A sidecar the engine cannot read (an unknown
  version, a truncated file, an IO fault, a retired state name) used to
  degrade to "no anchors" everywhere but the binding-scoped fidelity
  report, and a mem then verified clean over rows nobody read. Now one
  condition, one code, `ANCHORS_SIDECAR_UNREADABLE`, rendered by every
  surface from the engine's one sidecar check: `memstead anchors <id>` and
  `memstead verify-anchors` refuse typed (the mem and the parse reason
  named, `fully_adjudicated: false` and an unknown population in the
  details, nothing recorded), `memstead anchors --artifact` carries the
  affected mems under `sidecar_unreadable`, `health --include anchors`
  carries a per-mem `condition` with an unknown population, `health
  --include integrity` lists the finding (id: the mem) and `--strict`
  refuses under either include, and the entity read (CLI `--json` and
  markdown, MCP `memstead_entity`) carries `anchors_sidecar_error`.

### Changed

- **The stub integrity finding is `UNRESOLVED_STUB`** (was `ORPHAN_STUB`:
  a stub is by construction referenced, never orphaned; the name said the
  opposite of the condition). The code changes on every surface that
  emits or names it (health findings, the MCP tool descriptions, the error
  index, the `--strict` summary label `unresolved_stubs`); nothing accepts
  finding codes as input, so there is no retired-name refusal to add.
- **Health and status lists are byte-stable across runs.** `edge_types`
  and `type_distribution` (`memstead status --json`, the health
  composer's summary, the ui-api status endpoint) are ordered by name
  (`Status.edge_types` is a `BTreeMap`); the health `orphans`,
  `missing_fields` and `stale` lists are ordered by entity id. They used
  to follow hash-map iteration order, so two runs, or the CLI and the MCP
  server, could disagree on the order of identical content.

- **One anchor-state name: `resolves`.** The verify surfaces, the health
  `anchors` axis and their text renderings spelled the matching state
  `resolved` while the entity read and the sidecar's `last_observed.state`
  spelled it `resolves` (the `AnchorState` wire form). `resolves` wins
  everywhere: the `resolved` count on `verify-anchors` and `health
  --include anchors` is now `resolves`, and a row state reads `resolves`.
  `resolved` as input (a sidecar row's `last_observed.state`) refuses with
  the vocabulary named, as `ANCHORS_SIDECAR_UNREADABLE`.
- **`health --include anchors` examines anchors.** The `verdict_coverage`
  line lists `anchors` under examined when the axis was rendered, and an
  unreadable sidecar is a strict violation on that run.
- **A sampled `projection verify` honours the binding as declared now.** The
  rotation scheduler used to walk its cached order to the end of a rotation,
  so after `projection edit` added a `deny_paths` entry every sampled run
  kept recording `uncovered` findings for files the binding denied. The
  order is now reconciled against the current item set on every window: a
  departed item leaves it, an arriving item joins the rotation in flight,
  and the recorded window is held to `S(D)` as a second wall. `--full` is
  unchanged.
- **`projection_status` and `projection_rollup` share one scan.**
  `memstead status` and the ui-api status endpoint compute the per-binding
  resolution once (`projection_overview`) and derive the rollup from it; the
  two standalone functions remain and agree, and the rollup's shape is
  unchanged.
- **Authored exclusions key on the artifact and its source.** An exclusion
  records the facet it was declared under, survives any `projection edit`
  that keeps that source, and is dropped when the source leaves the
  declaration; the sync brief now lists the exclusions in force (artifact,
  source, rationale) and reports a dropped one once with its source named.
  Entries recorded before the field existed are attributed on the next
  brief or verify.
- **Mem membership follows reload-before-operation.** Before each operation
  the engine compares a fingerprint of the mount roster
  (`.memstead/state/mounts.json`; a stat, plus a hash only when size or
  mtime moved) with the one it last reconciled on. On a change it mounts
  new entries cold under the boot quarantine rules, unmounts gone entries
  atomically (store slice, schema entry, router slot, search index,
  community partition, pending change notices), re-scans the schema
  sources, and marks the response `MEM_ROSTER_CHANGED` with `added`,
  `removed`, `quarantined` and `failures`. An operation naming a mem that
  left refuses `MEM_UNMOUNTED`. Cross-mem edges into an unmounted mem read
  dangling on the integrity axis. `memstead_reload` `full: true`, `memstead
  reload --full` and the ui-api reload run the same reconciliation forced
  and report removals as applied: the `refresh` block's
  `mem_removals_skipped` is replaced by `mems_unmounted` and
  `mems_quarantined`. The ui-api emits `mem-roster-changed` on its SSE
  channel (`added`, `removed`, `quarantined`) and the web app invalidates
  its mem list and every per-mem read on it. With the `file-watcher`
  feature, `watch_roster` reports writes to the roster file alongside the
  mem-repo refs. Engine: `Engine::reconcile_roster`, `subscribe_roster_changes`.
- **The independence gate compares the executor, not the criterion's
  author.** A check on an acceptance criterion reads
  `confirmed_independent` only when its identity differs from every
  identity that mutated the verified plan, its criteria or its session-log
  notes since the criterion was written; a check under one of those
  identities reads `self_checked`; a check or a record without an identity
  stays `unconfirmable`. Until now the reading compared against the
  criterion entity's creator, so the executing session's own checks read
  independent (found by the evidence-engine bundle, once on a wrong check).
  The `transition_requires_checks` gate consumes the same reading: a plan
  cannot complete on the executor's own checks, and the gate names
  `self_checked` or `unconfirmable` on the entity that holds it open. The
  reading is derived at read time from the append-only provenance record
  (commit trailers on git-branch mems, the ledger on folder mems); no field
  is stamped and every existing check ledger parses unchanged. `health
  --include checks` renders `comparator`, `executors` (the identities each
  ok-checked criterion was compared against) and `readings` (every
  verification record's own reading, so a superseded self-check stays
  visible beside the grader's). Engine: `Engine::executors_of`,
  `independence_of`, `check_standing_provider`.
- **The `vital_signs` health axis.** `health --include vital_signs` (CLI,
  full and lean MCP, one composer) reports per mem the cheap, countable
  model-truth signals the remodel campaign specified: per community, how
  many entities sit on the schema's declared last-resort type
  (`type_share_by_community`; `not_declared` when the schema declares
  none, never a guess from names); bound-source files no entity's anchor
  claims, largest first with sizes (`unclaimed_source_files`; the size
  threshold stays in the skill); files two or more entities claim while
  none owns them (`contested_unowned_files`); entities with no outgoing
  edge, folded into the community of their subject rather than ranked as
  singletons (`zero_outgoing_entities`); declared sections an entity
  carries empty (`empty_declared_sections`). Each signal is a count plus a
  capped list with an explicit `more` remainder; the payload carries no
  verdict, threshold or recommendation. The axis reuses the community
  partition, the anchor sidecar reads and the source enumeration the other
  axes use. The schema language gains `last_resort: true` on a type
  definition, at most one per schema (`MultipleLastResortTypes` at load);
  the built-in schemas do not declare it yet, so their share signal reads
  `not_declared` until a version that does. The `/remodel` skill's scan
  step reads the axis and keeps the thresholds in its own text.
- **Coverage counts describing entities per artifact.** The fidelity report
  states its unit: an artifact counts once when at least one entity anchors
  it, however many anchor rows it carries; `covered_artifacts`,
  `describing_entities` and the `unit` line ride the report's coverage
  block. Historic reports are not rewritten.

## [0.16.0] - 2026-09-02

### Added

- **`/sync --sweep` takes a list of mems.** `--sweep <mem> [<mem>...]`
  walks the named mems in order, one finished before the next starts, a
  mem the workspace does not mount named and skipped; the closing
  per-mem counts name any mem the session did not reach, so the next
  invocation lists exactly those. One mem still works as before.
- **Check records carry a structured finding and accept an open `x-`
  kind.** `memstead check` (single and `--from`) and `memstead_check` take
  an optional `finding {code, message, section?, evidence?}`: persisted on
  the ledger line (serde-default, every existing line still parses),
  echoed on the output, rendered by `health --include checks` under the
  entity's latest verdict (`findings`); a missing `code` or `message`, an
  empty value or an unknown key refuses `INVALID_CHECK_FINDING` naming the
  shape, and nothing is appended. A kind of the form `x-<name>` is
  accepted and recorded verbatim: the engine aggregates only its own two
  kinds, never stamps a pin or moves `check_state` for a foreign kind, and
  lists foreign kinds by count (`foreign_kinds`); any other unknown kind
  keeps refusing `INVALID_CHECK_KIND` with the vocabulary named. The
  independence derivation is unchanged.
- **Chain export: `memstead export --root <id> --via REL[,REL]
  [--direction out|in|both] [--depth N]`** renders, for `--format json`,
  `html` and `llms-txt`, only the subgraph reachable from the root along
  the named rel-types in the given direction — direction applied at every
  hop, the same transitive-closure contract `memstead search` uses — each
  entity with its metadata, sections, relationships and (json) its anchors
  with live state, stubs in the chain marked, references to entities
  outside the chain left unresolved rather than rendered as broken links.
  The json export carries a `chain` block (root, via, direction, depth,
  the nodes and the induced edges with cross-mem targets marked) that
  matches the ui-api topology endpoint for the same scope, whose
  `GET /mems/{mem}/topology` now accepts `root`, `via`, `direction` and
  `depth`. Without `--root` every export is byte-identical to before. An
  unknown rel-type refuses `INVALID_REL_TYPE` naming the vocabulary, an
  unknown root `ENTITY_NOT_FOUND`, `--root` without `--via` (and the
  reverse) `INVALID_INPUT`. No MCP tool: bulk export stays CLI-only.
- **`memstead retype <id> --type <target>` and `memstead_retype`: change
  an entity's type in place.** The id, file path and every incoming edge
  stay; nothing is deleted or re-created. The existing sections and
  metadata are validated against the target type with a report-all
  refusal (every unknown section, missing required section, unknown or
  invalid metadata value and block-tier constraint together, with the
  target's declared sections, its catch-all and a proposed `section_map`
  in the details), `--section-map old=new` renames section keys on the
  way (nothing is moved into the catch-all silently), and every incoming
  and outgoing edge, cross-mem included, is re-checked against the target
  type's relationship pins — a violation refuses `INVALID_REL_SHAPE`
  listing each edge, so the loader never has to drop a shape-invalid edge
  at the next boot. Referrers in a lazy (unloaded) mem are probed through
  storage; a mem that cannot be probed refuses
  `RETYPE_REFERRER_UNPROBEABLE`. One commit lands with the new `retype`
  provenance kind, and the response states that check records and
  derivation baselines on the entity are stale because its content hash
  moved. `dry_run` previews without a hash. The MCP roster grows to 20
  tools (14 lean); `update --from` and the `memstead_update` description
  point at the new verb.
- **Url anchors are observable.** `memstead verify-anchors --mem <m>
  --observations <file>` takes observer-supplied rows `{artifact, hash |
  content | absent: true, observed_at?}` for the one grain the engine never
  observes itself (it never fetches): a url row with a supplied observation
  adjudicates through the same funnel a file anchor does (equal hash
  `resolved`, differing hash `drifted` under `stable` and `recheck` under
  `unstable`, `absent` → `recheck`), `content` is hashed under the write
  path's rule, and a url row without an observation stays `unobserved`. The
  matched observations are recorded on the sidecar rows as `last_observed
  {at, hash, state}` (anchors sidecar version 2; version 1 files load
  unchanged and are rewritten as version 2 on the next anchor write, an
  unknown higher version still refuses), so the per-entity read, `health
  --include anchors` (an `aging` list) and `--include open_questions`
  (`anchors_aging`), `verify-anchors` and the fidelity report all show
  `unobserved for N days` for a row resting on an older observation. A
  malformed observation row refuses the whole run with `INVALID_OBSERVATION`
  before any state changes; rows naming no url anchor are reported as
  unmatched. The `url` grain is now admitted beside every medium (a URL
  never enters a path namespace), so a mem with one filesystem binding
  accepts url anchors; a path-shaped grain whose artifact is a URL refuses
  `INVALID_ANCHOR` naming that rule. The run brief's anchor instruction and
  the docs tell authors to set `hash_stability: stable` on immutable
  documents.
- **`memstead schema migrate <dir>` rewrites an authoring package's
  retired keys into the current schema language.** The keys `schema
  validate` refuses (`propagating_relationships`, the metadata-field
  `optional:`, the retired `examples:` list, the exemplar-relation
  `to:`/`type:` spelling) are rewritten by exactly the translations the
  loader applies to sealed content: one table (`memstead_schema::migrate::
  LEGACY_KEYS`) pinned by the suite against the loader's serde sentinels,
  and a run-time faithfulness check that loads the original through the
  sealed-style read and the rewrite through the authoring read and
  refuses to write unless both resolve the same schema. Dry run by
  default (one line per rewrite, nothing written); `--write` edits the
  files in place, comments and key order preserved. A package that
  carries `optional:` was written when an absent key meant required, so
  its fields declaring neither key get `required: true` — the meaning
  their sealed copies already have, shown in the dry run for the author
  to keep or delete. Never bumps `version`, never touches a sealed copy;
  the report closes with the `validate` → `install` → `mem set-schema`
  steps. Refuses with `SCHEMA_MIGRATE_FAILED` (a `reason` in details)
  when a value cannot be rewritten mechanically or the package fails to
  load for a reason no retired key explains.
- **`memstead schema <pin>` renders workspace-installed packages, not
  only built-ins.** A pinned reference that is no built-in now falls
  through to the workspace's installed stores (the filesystem
  `.memstead/schemas/` layout and the mem-repo's `__MEMSTEAD:schemas/`
  ref), so a workspace-local schema's sealed README — a contract
  carrier, not only documentation — is readable through the same
  sanctioned verb (`origin: "workspace"` on the JSON envelope). Bare
  names stay built-in-only; a pin found nowhere refuses naming both
  stores.
- **`leak-scan.sh` accepts caller-supplied allowlist extensions.** The
  env var `LEAK_SCAN_EXTRA_ALLOW_FILE` names a file of extended-regex
  lines OR'd into the allowlist, for callers scanning a tree with its
  own legitimate self-matches (e.g. a seal gate scanning exported
  archives). Blank lines and `#` comments are skipped; the default
  pattern classes are never weakened.

### Fixed

- **Schema install stamps its authoring provenance portably.** The
  install-provenance stamp recorded the authoring directory as an
  absolute machine-local path, so on every other clone (CI included)
  the authoring-drift health axis reported the package's source as
  missing instead of checking it. A path inside the workspace is now
  stamped workspace-relative and the axis resolves it against the
  current workspace root; out-of-workspace authoring dirs stay
  honestly machine-pinned absolute.
- **A folder mem whose schema was installed onto the mem-repo's
  `__MEMSTEAD:schemas/` ref can now export.** The archive assembler
  resolved schemas only from the filesystem `.memstead/schemas/`
  chain, while the loader resolves pins from the ref — two readers,
  two answers: the mem mounted and wrote but `export --format mem`
  refused with `schema not found`. The folder-mount export paths now
  consult the ref store through a new `GitBranchOps` hook with the
  same precedence the branch export applies, so the archive seals the
  package the loader resolved.
- **Projection status stopped stat-walking the world.** Two costs on
  the status path multiplied into minute-long `memstead status` runs
  (and a hanging UI status endpoint): facet-file enumeration walked
  every directory under a source pointer — build trees and
  node_modules included — even when no scope pattern could match
  there, and `mem_predates_binding` answered "does this mem have
  anchors" by observing (hashing) every anchor against its live
  source. Enumeration now prunes directories outside every allow
  pattern's literal prefix (unanchored `**` scopes keep the full walk),
  and the emptiness question is answered by a sidecar parse
  (`Engine::mem_has_anchors`) that observes nothing. Measured on the
  dogfood workspace: `memstead status` 53s → 0.9s.

- **Plain `tree` anchors adjudicate deterministically instead of resting
  in `recheck` forever.** A `tree` anchor under no code-map preparation
  had no prepared form: observation never yielded a hash, backfill never
  fired, and every verify pass re-queued the same
  `queued-for-adjudication` finding while the report's own remedy
  (`verify --full`) could not drain it. The tree's prepared form is now a
  digest over every scoped file under it — the code map under a code-map
  preparation, the plain per-file prepared-content map otherwise — so a
  hash-less tree anchor backfills on first observation and thereafter
  resolves or drifts from the hash comparison alone, exactly like a file
  anchor. Any scoped-file byte change, and any file joining or leaving
  the tree, moves the digest; a tree with no resolvable source-join or a
  partial enumeration still observes no hash and stays honest `recheck`.
- **An authored exclusion now supersedes a standing `uncovered` finding
  in the store, not only in the presentation filter.** The head-durable
  merge carried an unsampled `uncovered` finding forward even after its
  artifact gained a ledger exclusion, so the verdict line kept counting a
  finding the coverage section already reported as accounted for. The
  merge's accounting closure folds the exclusion ledger in; the stale
  finding closes on the next verify pass.
- **A stored `_`-prefixed metadata key can no longer shadow a computed
  frontmatter field.** The underscore namespace belongs to the read
  channel's computed slots (`_hash`, `_tokens`, `_signals`, ...), but the
  write gate accepted `_hash` as ordinary metadata and the markdown
  renderer emitted the stored copy beside the computed line — an external
  agent test surfaced the duplicate `_hash` after pasting read-response
  frontmatter into a write. Both halves of the shared-gate rule now hold:
  every set path refuses `_`-prefixed metadata keys as `READ_ONLY_FIELD`
  (unset stays permissive, the sanctioned repair for an already-smuggled
  key), and the markdown frontmatter applies the same computed-and-reserved
  filter the JSON envelope always had.

## [0.15.0] - 2026-09-01

### Added

- **New `/remodel` skill — model truth as its own maintenance cadence.**
  Where `/sync` repairs what entities SAY, `/remodel` repairs what a mem
  IS: whether every obligation of the subject has exactly one home
  entity of the right type, substance sits in its declared sections,
  and the graph is wired. A cheap signal scan (`--all` walks every mem
  and descends only where type collapse, unowned source mass, missing
  edges, or empty definition-test sections justify it; `--scan` reports
  without writing) selects the cluster; the round then derives a target
  inventory blind from contract plus source, has it adversarially
  checked by an independent subagent, diffs it against the live
  entities, rebuilds conservatively under a two-gate creation
  discipline (neighbours read first; every symbol claim traced to a
  source line, mechanically verified), and brackets big rebuilds with a
  before/after reconstruction probe. Grown and measured in the
  model-truth benchmark campaign: the shipped loop closed 0 of 51
  frozen model-truth items, the grown round closed 14 per pass and 24
  over all seeds at zero collateral across 300+ control evaluations
  and zero fabricated claims in its final 35 creations.

- **`memstead check --from <file>` records a batch of checks in one
  engine boot.** Payload `{"checks": [{"id", "verdict", "method"?,
  "kind"?}, …]}`; the batch family's contract applies — every entry
  validated up front, any invalid entry refuses the whole batch naming
  EVERY failing entry, nothing recorded on refusal. The need is
  measured: one campaign run paid 242 engine boots for 242 verdicts.

- **`memstead export --format json --include anchors`.** Each entity
  envelope gains its stored provenance anchors, so the file-to-entity
  map a carving or sync pass starts from is one export instead of one
  `memstead anchors <id>` call per entity (139 in the live run).

- **`sections_unset` — a section can be closed.** `memstead_update`
  (MCP, both servers), `memstead update --section-unset KEY`, and
  `batch-update` entries gain the fourth section mode: remove a
  section's heading and body outright. No-op on an absent key
  (symmetric with `metadata_unset`); refused for a schema-REQUIRED
  section (`MISSING_REQUIRED_SECTION` — the right repair there is
  filling, not removing), for `relationships`, and for a key also
  written in the same call (`CONFLICTING_SECTION_MODES`). The
  mutation response reports removals under `modified_sections.unset`.

- **A section takes several patches in one call, applied in order.**
  `patch_sections` accepts a LIST of patches per section on every
  surface — MCP `memstead_update` (a single object stays valid
  unchanged), `update --from`, `batch-update` entries, and repeated
  `--patch`/`--patch-all` flags for one section (which used to refuse
  `duplicate patch`, costing one call per extra edit; two campaigns hit
  it). Patches apply in order against the section's evolving body.

### Changed

- **A declared-but-unwritten optional section is absent, not an empty
  scaffold heading.** The generator emitted every declared optional
  heading whether or not the entity carried the section, and the parser
  materialised every declared key back as present-with-empty — so "no
  heading" and "empty heading" were the same entity, every fresh entity
  sprouted scaffold headings no content ever reached (one campaign
  counted 82 across a mem), and no gesture could close one. Now an
  optional section renders only when the entity carries its key, and
  parsing a document without a heading yields no key for it. Existing
  files are byte-stable (their headings parse as present-with-empty and
  keep rendering); newly created entities no longer carry scaffold
  headings; `sections_unset` is the close gesture and survives the
  round-trip.

- **The sync skill adopts the model-truth campaign's maintenance
  disciplines.** Corrections are applied silently — no dated stamps or
  was-wrong narration in normative sections (the measured sediment
  mechanism: one repair pass had planted 39 dated markers across 27
  entities; git and the check record carry the archaeology, and the
  schema's designated history forms stay exempt). The sweep may cite
  git history as a legitimate source, and cut defects (a fused entity,
  a missing owner) route to `/remodel` — the sweep reports them, never
  re-cuts.
- **The sync skill repairs claim by claim and gains the standing-claim
  sweep.** Three disciplines proven by the drift-benchmark run series
  (closure 0-5/42 before, 13-17/42 per pass and 25/42 over three seeds
  after, zero collateral across 200 control evaluations) now bind the
  repair path: a drifted entity is worked claim by claim instead of a
  gestalt materiality call, every edited entity is re-read once against
  the source, and an entity that contradicts itself gets its normative
  section reconciled with its own dated corrections. New `--sweep <mem>`
  mode: the standing-claim walk over any mem, bound or not, verifying
  what entities assert even where no change signal points, leaving check
  records as its machine-readable trace.

### Fixed

- **An inline `--patch` whose text carries a second `=>` refuses instead
  of corrupting the section.** The splitter took the first occurrence,
  silently mis-splitting OLD from NEW; the ambiguity now refuses with a
  pointer to `--from`, which carries arbitrary text unambiguously.

- **`verify-anchors` backfills first-observed hashes.** Only the
  binding-backed verify backfilled, so a manually re-pinned anchor on a
  binding-less mem read `recheck` forever (`hash_source: backfill` on
  every manual re-pin, live melt). The standalone pass now records
  observed hashes onto hash-less anchors exactly as the binding pass
  does — idempotent, and the recheck queue drains on the next pass.

- **`batch-update` entries tolerate the template `mem` key.**
  `batch-create` accepted it and `update --from` tolerated it while
  `batch-update` refused the whole batch on the unknown field; it now
  validates against the entry id's mem exactly as `update --from` does.

- **`memstead type` marks rel-types the alias machinery owns.** The
  manual-authoring posture now renders beside each relationship
  (`manual authoring FORBIDDEN — emitted from body wiki-links only`),
  so the refusal is visible BEFORE a batch is composed instead of
  arriving after an all-or-nothing `batch-relate` was built around it.


- **Non-JSON CLI errors carry the recovery payload.** The human error path
  printed `code: message` alone and serialized the structured `details`
  only under `--json`, so the recovery data the engine already computes
  (`INVALID_REL_SHAPE`'s allowed endpoint types, `PATCH_OLD_NOT_FOUND`'s
  current content) was invisible exactly where an agent on the text
  channel needed it — one run probed five rel-types in sequence for an
  answer the payload held. The header line keeps its documented shape;
  `details` follows as an indented pretty-printed block on stderr.

- **`PATCH_OLD_NOT_FOUND` names the sections that DO contain the
  substring.** The refusal said only where the patch's `old` was not
  found; when it lives in a different section (the common mistargeting),
  the new `details.found_in_sections` turns three attempts into one.

- **A sub-tree-pointed binding's changed slice honours the pointer join.**
  A `**`-prefixed scope glob was pushed to git verbatim as a repo-wide
  pathspec, so a binding pointing at a subtree (`plugin/graph` points at
  `../public/plugins/claude-code`) was steered by its sync brief at
  changed artifacts across the whole repository — artifacts its own
  enumeration correctly kept out of `S(D)` and its `exclude` gate refused
  as out-of-scope; the two axes provably disagreed (reproduced twice in
  the drift-benchmark, re-reproduced live before the fix). Scope globs
  now anchor at the medium base in the git diff exactly as the
  enumeration anchors them; a deny whose namespace root lies outside the
  repo keeps its conservative repo-wide reading.

- **An accepted exclusion takes effect on the very next sync brief.** The
  brief served the last recorded findings batch as-is, so an artifact
  `projection exclude` had just accepted kept presenting as uncovered
  until a verify pass rewrote the batch; three independent runs read that
  as a repair that did not take, and one live run was steered at ~300
  findings a fresh measurement no longer carried. The brief's findings
  read now consults the durable exclusion ledger and drops excluded
  uncovered findings; superseded batches were already structurally
  excluded and are now pinned by test. The brief's uncovered guidance
  also gained the missing routing: `projection exclude` is the verb whose
  gate accepts a stable artifact (`advance` gates on the changed slice),
  and the brief now prints the buildable command instead of never naming
  the verb.

- **The plugin's entity-bash guard classifies per target instead of
  command-wide.** The hook blocked any command that mentioned an entity
  file AND matched any write pattern anywhere, so a read piped to a
  scratch path (`grep ... entity.md > /tmp/out`), a compound command
  containing an `echo`, and read-only git plumbing on entities whose
  kebab-case names contain words like `install` or `patch` were all
  refused as mutations (five agents rerouted around the guard in one
  campaign day). Now a redirect is a block reason only when its target
  IS an entity file, output-producing verbs (`echo`, `printf`, heredocs)
  are not write patterns by themselves, and entity paths are blanked
  before verb matching so a verb inside a filename never counts. Real
  writes (redirects into entities, `sed -i`, `mv`/`rm`/`cp`, `dd of=`,
  git `checkout`/`restore`/`reset`) block exactly as before.

- **Published command help catches up with the shipped surface.** The
  `projection` command tree's own docs said "five leaves" while nine ship;
  the top-level help now lists all nine verbs. `memstead status` printed a
  Do-next line recommending `projection sync`, a verb the binary does not
  have (now: `projection brief <binding> --sync`). `projection verify`'s
  help and the fidelity report still described findings as keyed
  `(hash(D), source_head)` although the store keys on `hash(D)` alone with
  the head carried as metadata. `uninstall` and `recover` claimed
  "MEM-REPO WORKSPACES ONLY" although both are shape-agnostic. `projection
  init --name`'s help named the retired three-file scaffold. The ingest
  module header no longer frames the subsystem as a port in progress. The
  generated CLI reference picks these up on the next docs build.

## [0.14.0] - 2026-08-29

### Fixed

- **An undeclared cross-schema body link no longer vanishes silently.** A
  wiki-link into a mem whose schema the source schema declares no
  `cross_mem_relationships` entry for (a default-schema scratch mem citing
  a planning mem, found by the graph-plans lifecycle grading) used to
  EMIT the alias edge at write time and then lose it silently at the next
  load — the write showed a relation the graph would not keep. The alias
  pass now respects the declaration at write time: no edge is emitted, the
  write still succeeds, and a typed `CROSS_SCHEMA_LINK_UNDECLARED` warning
  names the target and the declaration gap with the schema-side remedy.

- **A batch-created entity keeps its author.** The per-entity history
  walk treated a `batch-create` commit as truncation, so every
  batch-authored entity served no `created_by` record and the checks
  independence gate degraded to `unconfirmable` for it. A batch-create
  that lists the entity is now recognized as its creation: `created_by`
  carries the batch commit's role and identity trailers, and the walk
  stops there like any single create.

- **A hierarchical mem exports under its leaf name.** `export --format mem`
  on a git-branch mem named with a path (`planning/plan-x`) stamped the
  full path as the archive identity, which the archive slug grammar
  refuses — so such mems could never produce a `.mem`. The archive
  identity is now the leaf segment (`plan-x`), matching the publish
  contract's documented rule; flat names are unchanged. The export
  warning for dangling cross-mem edges now also points at
  `--self-contained` as the remedy.


- **The README and the crates' own descriptions speak one register.** The
  README pitched the published crates as what you program against while every
  library crate's crates.io description called itself an internal surface.
  Both now say the same true thing: a surface you can program against,
  pre-1.0, experimental, no API stability promise (operator decision,
  2026-08-28).

- **A partial enumeration can no longer pose as a population — three
  surfaces, one rule.** The scheduled full walk branched on enumerability
  alone, so a facet whose enumeration was known-incomplete (a malformed or
  retired-dialect scope pattern) was walked and announced as full where an
  explicit `--full` already refused; it now lands in the typed refusal list
  beside the non-enumerable facets. `projection exclude` enumerated the
  membership set unreported, so it refused genuinely in-scope artifacts and
  printed the short count as if it were `S(D)`; it now refuses outright
  under a partial enumeration (`PROJECTION_EXCLUDE_PARTIAL_ENUMERATION`).
  And a code-map tree anchor observed over a partial enumeration would have
  silently changed its stored digest; it now observes no hash and resolves
  `recheck`, the same posture as a failed read.
- **One artifact-resolution rule, one implementation.** The ratified
  candidate priority (source-join first, workspace-relative fallback —
  decision 26+29) was implemented separately by anchor resolution, the
  write-time gate, the reverse anchor lookup, and the population scope
  matcher, and the copies disagreed on a self-nested layout and on climbing
  `../…` artifacts (where the fabricated `<ptr>/../…` join could resolve
  into a sibling tree). All four now construct their candidates through one
  shared function; a climbing artifact never joins, and a `.` pointer reads
  exactly as an empty one on every surface.
- **`rename` rewrites prose-only referrers.** Body wiki-links are not edge
  sources, so a hand-authored file carrying `[[old-slug]]` with no
  `## Relationships` row had no incoming edge and the referrer walk left its
  link stale. The rename now also scans section bodies for links resolving
  to the renamed id — the hand-commit folder-mem model is rewritten like
  every engine-written referrer.
- **The sync brief warns about retired-dialect scope patterns.** The
  migration notes reached only the verify report and the `--full` refusal,
  so a binding running only build and sync was never told its scope selects
  nothing. The operative-data block now names each such pattern and offers
  the mechanical rewrite where one exists.
- **`projection advance` no longer demands `--dispositions` for the
  empty-slice first call.** The flag defaults to `{}` — the shape of a first
  advance over an empty presented slice, which exists only to write the
  baseline.

- **A ledger-excluded artifact is no longer recorded as an uncovered
  finding.** The authored-exclusion ledger gated only the report's
  decoration: the rationales rendered right beside a verdict line that still
  counted the same artifacts as uncovered in the findings store (observed as
  "uncovered: 3" over three excluded artifacts). The exclusion now gates the
  recording itself — an excluded artifact raises no finding, a prior open one
  closes on the next verify, and the rationale keeps rendering. Pinned by an
  integration test that fails on the pre-fix recording.
- **The chunker reads frontmatter through the consolidated core.** Its
  hand-rolled scanner recognised only the newline-terminated opening fence,
  so a `---\r\n` document was treated as having no frontmatter: the chunk
  view prepended a second frontmatter block over the first and the entity
  keys vanished. `frontmatter_parts` now wraps `split_frontmatter_core`, both
  delimiter flavours resolve in one place, and a carriage-return regression
  test pins the repair.
- **CLI/MCP parity: `update --from` accepts `relations_unset`.** The
  repair-shaped relation removal MCP's `memstead_update` offers was refused
  at payload parse on the CLI ("unknown field"), so the repair had no CLI
  equivalent. The key now routes to the same engine semantics (refused
  `REPAIR_NOT_NEEDED` on a conformant entity, pointing at
  `memstead relate --remove` for everyday detachment).

- **The sync brief's drifted-anchor recipe now works as written.** The brief
  told the repairing agent to re-declare a drifted anchor without a hash and
  let the next verify backfill it — but a hashless re-declare deliberately
  KEEPS the stored baseline (incremental anchoring depends on that), so the
  advice cleared nothing: on the 0.13.0 release-readiness pass all 18 drifted
  flagship anchors stayed drifted until the real reset was found. The brief
  now states that reset: `anchors_unset` the row and write it fresh in the
  same update call.
- **`publish-crates.sh` retries a failed upload and re-checks liveness between
  attempts.** The 0.13.0 release lost exactly one crate to a transient
  connection reset from crates.io and failed the whole publish job over it.
  Each crate now gets three attempts, and before a retry the script asks the
  registry whether the upload landed server-side despite the client-side
  error — a live version is success, not a duplicate-publish failure.

### Added

- **`projection verify --fail-on-inconclusive`.** A completed run whose
  rollup verdict is `inconclusive` — no readable change signal, an empty
  enumerated scope — can now fail the job itself: exit 6 with the typed
  `PROJECTION_VERIFY_INCONCLUSIVE`, report rendered first, evaluated
  after `--fail-on-findings` so a substantive result outranks a
  blindness report. Opt-in and additive: without the flag an
  inconclusive run keeps its long-standing exit 0, and the exit contract
  stays three-valued — CI branches on 6 and reads `.code` to tell which
  gate fired. Supersedes the verify-in-CI guide's two-step verdict read
  for opted-in callers (operator decision, 2026-08-28, option c of the
  exit-code entry; shipped as the graph-plans pilot).

- **The gates brief: `memstead gates`.** The engine renders the standing
  of every schema-declared `transition_requires_checks` gate: per gated
  type, the closed entities, and the open ones in dependency order
  (topological over the schema's acyclic edges, prerequisites first),
  each with its related-check coverage and the exact unconfirmed
  entities. The related-set enumeration is the same code the write-time
  refusal runs, so the brief can never disagree with the gate. Brief
  family rules apply: shared engine entry point, CLI verb, deliberately
  no MCP tool.

- **Gated transitions: the `transition_requires_checks` constraint.** A
  schema can now declare that a write landing a metadata field at a named
  value requires every entity related via named relationships (incoming or
  outgoing) to carry a fresh confirming check record — derived
  verification state `checked_ok`; stale and failed checks do not confirm.
  Block-tier by default: the unverified transition refuses at write time
  listing each unconfirmed entity with its derived state; a check that
  goes stale after the transition surfaces as a standing violation on the
  health `constraints` axis. Generic by construction (any schema, any enum
  field, any relation set) — the first consumer is the workspace-local
  planning schema's completion rule ("`complete` requires a check record
  on every VERIFIES-linked criterion"). Evaluated in the single shared
  declared-constraints pass; verdicts ride the check ledger, never an
  entity field, so the gate cannot be satisfied by editing the entity.

- **`memstead projection edit` — the general binding-field patch.** The shared
  `pipeline_edit` layer has carried the full author-editable record (patch
  semantics, validate-before-write, refusals-introduced-only) since the
  projection-promotion work, but no CLI command exposed it: `init` and
  `enable` existed, editing a source list or any other binding field did not,
  a gap the projection-pipeline pilots hit and the backlog carried since
  June. `projection edit <binding> --patch '<json>'` closes it: absent fields
  preserved, `null` clears where clearing is legal, a present block replaces
  whole, `version` stays engine-managed, and a patch that would introduce a
  validation refusal writes nothing and names the refusals
  (`PROJECTION_EDIT_REFUSED`). First consumer: giving a binding additional
  sources over sibling trees.

- **Caller-declared identity in provenance and checks (agent-trust plan 15,
  engine core).** Every mutation and check can now record WHO acted — an
  opaque caller-chosen string (an agent name, a session handle), declared per
  session on the engine and recorded immutably beside actor/client/role: as
  an `Identity:` commit trailer on the git-branch backend, an `identity`
  field on the folder changelog and the check ledger. The entity read's
  provenance block and entity history serve it back. The author≠checker
  independence gate now compares identities and nothing else: equal
  identities read `self_checked`, differing ones `confirmed_independent`,
  a missing identity on either side stays `unconfirmable` — the (actor,
  client) transport pair is recorded context and never again a comparator.
  Same trust model as roles: caller-declared, unverified, tamper-evident in
  append-only history. Absence stays legal forever; records predating the
  field stay `unconfirmable`, never backfilled. Declaration on every
  surface: a per-call `identity` parameter on the MCP mutation and check
  tools (both flavours), `--identity` on both binaries and the
  `MEMSTEAD_IDENTITY` environment variable as session defaults (per-call
  wins over flag wins over environment). Over-length values refuse typed
  (`INVALID_IDENTITY`, cap 128 chars); a missing identity is never refused.

### Changed

- **Dependency refresh across the workspace.** The MCP SDK moves to
  rmcp 3.1 (from 2.2): the server now models the 2026-07-28 protocol
  revision, so `tools/list` replies carry the additive
  `resultType`/`ttlMs`/`cacheScope` fields and tool dispatch returns
  the MRTR `CallToolResponse` wrapper internally; the served tool
  surface and every tool's reply body are unchanged. Alongside: gix
  0.87 (the `tree-editor` feature folded into core; `tree-error`
  replaces it in the manifest), reqwest 0.13 (`rustls-tls` split into
  `rustls` + `webpki-roots` + explicit `form`), dirs 6, base64 0.23,
  and a full `cargo update` of the lockfile. CI actions ride along
  (setup-node v7, checkout v6 on the npm publish job, install-action
  current pin).

- **BREAKING (wire): the entity's type is `entity_type` on every read
  surface.** The MCP entity envelope (and with it the CLI's JSON entity,
  export, and changes payloads, which share the renderer) served the field
  as `type` while the wasm `getEntity` has always served the serialized
  Entity's `entity_type` — the same conceptual read under two spellings,
  found the hard way by a reader following the published README example.
  One spelling now: the envelope carries `entity_type`, `memstead status`'s
  per-type counts carry `entity_type`, and the retired `type` key is gone,
  not aliased (a wire-shape test pins its absence). Out of scope by design:
  the frontmatter `type:` key (on-disk storage format, still refused as a
  read-only metadata key under that name), binding-config `type` (the
  source's medium — a different concept), and the playground's compact
  graph dialect (a foreign consumer's contract, retained per its own
  record). Operator decision 2026-08-28; rides this batch with the
  `format` gate below.

- **BREAKING (config): the workspace mem config's `format` is modeled and
  gated.** `<mem>/.memstead/config.json` could carry any `format` value and
  serde silently dropped it — `"format": 99` verified clean and wrote a
  `#verified` baseline. The field is now modeled: absent means version 1
  (the healthy common case), the known published-config versions stay
  readable (a mounted read-mem's cached config parses through the same
  struct), and an unknown value refuses loudly at parse and in
  `check_config`, matching the binding record, `WorkspaceConfig`, and the
  anchors sidecar. Breaking only for a config carrying a value no engine
  ever wrote.

- **Exemplar relations are authored in the mutation vocabulary.** A type's
  `exemplar.relations` entries now take `target:` / `rel_type:` — the same
  keys `memstead_create` accepts — so the served exemplar and the authoring
  YAML finally speak one spelling. The retired `to:` / `type:` keys refuse
  at authoring load (`memstead schema validate`, install) with a rename
  pointer; sealed content — shipped built-ins, installed workspace schemas —
  keeps loading with the old keys translated, so no existing package
  breaks. Executes the convergence rider from the 2026-08-28 ruling
  "a built-in schema version is minted for meaning, never for spelling".

## [0.13.0] - 2026-08-28

### Fixed

- **A quarantined mem can no longer disappear from `status`, and `type --mem`
  names the quarantine.** Two residuals a closing grade filed against the
  quarantine-fallback work. `memstead type --mem <quarantined>` refused
  `UNKNOWN_MEM` with "no mems loaded", phrasing identical to an empty
  workspace, because the loaded roster was tested before any quarantine
  lookup; the arm now consults the engine's own adjudicator and refuses
  `MEM_QUARANTINED` carrying the typed reason and the repair command. And
  `memstead status`, the roster surface the filesystem-workspace `mem list`
  refusal points at, rendered no quarantine at all, so a quarantined mem read
  as a small healthy workspace; both output forms now carry the quarantine
  roster under the same never-behind-an-opt-in rule `health` follows. Both
  pinned by integration tests that fail on the pre-fix binary.

- **`memstead health` markdown form now serves conformance data it gathers.**
  `--include conformance` was accepted, documented at length in `--help`, and
  had no effect on the default markdown rendering: the `--json` form returned a
  populated `findings` array while the human form printed only the six-line
  summary, so an operator diagnosing a mem by eye was told nothing about
  content the engine was holding and reporting. The markdown rendering now
  carries `## Conformance findings` (with an explicit zero when requested and
  clean, so silence never reads as not-served), `## Body observations`,
  `## Constraint violations` (the same gap one include over), and schema
  format defects. Pinned by an integration test on the folder-workspace shape
  the defect was found on.

### Added

- **A gate: an advertised response field must exist in the emitting source.**
  The tool-description lint checked backticked references against a
  hand-maintained allowlist, so adding a token to that list was
  indistinguishable from shipping the field: a description once advertised
  `body_observations` on `memstead_health`, the generated reference carried it,
  and the suite stayed green while no server emitted the key. A new meta-test
  (`every_response_shape_ref_exists_in_emitting_source`, in
  `memstead-mcp/tests/tool_surface.rs`) holds every allowlisted token to
  existence as a bounded identifier or literal in the source of the crates that
  compose responses; its first run caught and removed one stale entry
  (`details.missing_targets`, referenced by nothing and emitted by nothing). It
  is an existence check, not a per-tool emission proof: that stronger gate
  needs a response-coverage harness and stays tracked.

- **A gate: no release path rests on a quota, and no gate reports a failed read
  as a failure.** One exhausted anonymous GitHub quota on one shared address
  produced two findings an hour apart — the first command a stranger runs, and
  the script that verifies the release those strangers are about to get — with a
  third instance recorded in a workflow comment. The two instances were repaired
  earlier in this sweep; a declared-set gate stops the class returning. The
  gate itself (`scripts/check-release-external-deps.py` and its declaration
  `scripts/release-external-deps.json`) lives in the private workspace repo
  that carries this one as a submodule, not here, and every path it names below
  is relative to that repo — so a reader of this repo alone will not find those
  two files, and the scripts under `public/scripts/` are the subjects it covers
  inside this one. Every
  release-path script's external hosts are declared, a script that contacts a
  host it has not declared fails (in every shape twenty-nine grades have found;
  the docstring lists what is known to escape and says plainly that the list is
  not a proof) — through a URL, a network verb, a classified
  command, a process substitution, or a `/dev/tcp` redirect target — a declaration naming a host the script no longer
  contacts fails, and a host marked quota-bound with no declared fallback fails.
  Subjects are DISCOVERED, not read off the declaration, and discovery is a
  CLOSURE: it starts at the release-path roots and adds every script a
  discovered script names, so a helper the adoption leg invokes is a subject
  whether or not anyone listed it. That is how `scripts/pin-wasm.sh` (reaches
  npm) and `scripts/sync-private-locks.sh` entered scope. A `.mjs` or `.py`
  subject, which the gate cannot read statically at all, is in scope
  unconditionally and declares its hosts by hand, because silence about a file
  the check cannot read is the same failure as silence about an undeclared host.
  One predicate decides whether a subject is read as shell (a shell suffix, or
  a shell shebang) and whether it counts as unreadable, so a subject can never
  be discovered and then analyzed for nothing: an unreadable one still has the
  hosts named in its text compared against its declaration, so declaring a file
  does not stop the gate looking at it.
  The second half checks classification: a verification script whose
  read-failure branch renders a disagreement fails, and so does one that renders
  it agreement; only unmeasured passes — in every region shape twenty grades
  have found, which is not the same as every shape shell accepts. A declared fallback may name that third
  state rather than credentials — the anonymous read is the measurement, so
  authenticating it would change what is measured instead of bounding it. Runs
  as its own job in the repo-hygiene lane beside its sibling declared-set gates,
  with a `--self-test` whose fixtures run through the same walk the tree does,
  each pinned to the message it expects, so a rule that stops working fails the
  self-test rather than going red for some other reason.
  **What it does not do:** it reads shell statically, so it cannot prove what a
  script reaches at runtime. A command or a classifier reached through a
  variable, `eval`, or a builtin taking shell source as a string (`trap 'ssh h'
  EXIT`) is outside its reach — the gate reports nothing there rather than
  guessing — and a `.mjs`/`.py` subject's declaration is a human's
  word rather than a reading. A program handed inline to an interpreter is
  still not read, but it is now REPORTED rather than passed: the call must be
  acknowledged in the declaration, with the host it reaches or with none. Its docstring lists what is known to escape, and says
  plainly that the list is what grades have found rather than a proof that
  nothing else does, and that several entries on it are false positives rather
  than misses — the same caveat applies to this entry.
- **Semantic conformance is now a recordable, schema-bound check.** Every schema
  carries two halves: the structural half the write gate validates, and the
  semantic half (each type's `write_rules` / `writing_guidance` prose) that no
  validator reads. A conformance judgment ("does this entity satisfy its type's
  prose") can now be recorded and its freshness computed without another LLM
  call: `memstead_check` and `memstead check` accept `kind`
  (`verification` | `conformance`; omitted stays exactly today's behaviour, and
  pre-kind ledger lines read as `verification` with no migration). A
  `conformance` record carries the mem's schema pin, stamped by the engine at
  record time and never caller-supplied, and derives stale when the entity's
  content hash moves OR the pin changes; `verification` staleness stays
  hash-only. State derives per (entity, kind) — the kinds never supersede each
  other. The entity provenance block serves `conformance_state` /
  `last_conformance_check` beside the existing fields, the health `checks`
  include serves per-kind counts (JSON and markdown), an unknown kind refuses
  `INVALID_CHECK_KIND` naming the closed vocabulary, and the `/tidy` skill
  documents the conformance pass (worklist from health, judgment against the
  type's schema prose, verdict recorded with the judging model in the method
  note). Verdicts are advisory: nothing gates a write or a read on conformance
  state.

- **Every clean verdict now declares the axes it answers for, and a gate keeps
  it that way.** A sweep found eight instances of one shape: a surface reporting
  clean over state it never examined. The fixes landed one by one; this closes
  the class. A workspace axis vocabulary (the health include keys plus
  `projection` and `mounts`) and per-consumer registries now declare, for every
  CLI subcommand and every MCP tool, either the axes its verdict examined, with
  a stated reason per excluded axis, or why the surface emits no verdict at all
  (`memstead check` stands declared outside the rule: its verdict belongs to the
  caller). Gate tests walk the live clap tree and both MCP tool routers, so a
  new surface fails until it declares and a new axis fails every declaration
  that has not met it. The declaration also rides the output: `health`,
  `status`, `overview`, `workspace dump`, `verify-anchors`, and
  `projection verify` carry a compact `verdict_coverage` line
  (`examined=...; not_examined=...`) in their JSON, markdown, or frontmatter, so
  a reader sees which axes a verdict covers without reading the source.

- **A cross-mem edge whose grant was revoked is now reported, and fails a strict
  run.** Cross-mem links are default-deny and gated on write. Revoke the grant
  afterwards and every edge written under it stayed exactly where it was, loaded
  without comment and exited zero under `--strict`: the workspace's own policy
  file had stopped describing its graph and no surface noticed. Grants were
  policed against the mem roster (delete refuses, rename re-keys) and never
  against the edge set.

  `health --include integrity` now reports `CROSS_MEM_EDGE_UNGRANTED` on the
  consistency axis, naming the referrer, the target, and that the cause is an
  absent grant rather than a missing target, and `--strict` refuses while any
  remain. `memstead workspace revoke-cross-link` names the edges the revocation
  orphans at the moment it happens, rather than leaving them to be discovered on
  a later gate run.

  Never a load refusal and never a quarantine: a policy edit must not take a mem
  offline, because the recovery would need the very links the refusal blocks.
  Revocation is not refused and needs no force flag, it deletes no edges, and
  removing an orphaned edge still needs no grant — so the reported condition is
  always resolvable. The check calls the same resolver the write gate calls,
  including the create-rule default union, so an edge permitted only through
  that union is not reported.

### Changed

- **BREAKING (wire): one spelling for an edge on every input, matching the
  outputs.** The same edge was spelled four ways depending on which door you
  came through, and one surface spoke two of them. `memstead_create`'s
  `relations` and `memstead_update`'s `declare_relations` took `{to, type}`;
  `memstead_relate` entries took `{from, to, type}`; `memstead_update`'s
  `relations_unset` already took `{rel_type, target}`; the CLI batch payloads
  and the private HTTP layer each added their own variant. Outputs were already
  consistent under a rule, so the inputs adopt it rather than the reverse: the
  type is `rel_type` everywhere, both ends are `from`/`to` where both are
  given, and the far end is `target` where the near end is implied by the call.
  So `memstead_create` / `declare_relations` now take `{target, rel_type}`,
  `memstead_relate` takes `{from, to, rel_type}`, and the CLI's
  `--relation REL_TYPE:target-id` joins the same pair. **No alias:** the old
  names are refused, not accepted as synonyms — an alias is the cheap migration
  that makes the defect permanent by keeping both spellings alive in examples,
  transcripts and agent memory. The wire-shape tests that pinned the old
  asymmetry are updated, and a new pin asserts the retired spellings refuse.

- **BREAKING (wire): every mutation's `commit_sha` is now `write_id`, and it is
  no longer documented as a change cursor.** The field was named for git on
  backends that have none: a folder or in-memory mem returns a synthetic opaque
  token, and every tool description glossed it as "per-mem git; gitdir via
  `memstead_health include_config=true`" while the gitdir lookup errors for that
  storage kind and the health projection omits the field, so the pointer
  resolved to nothing on exactly the workspace shape `memstead quickstart`
  produces. The field is renamed rather than removed: it must exist on
  git-branch responses, and omitting it elsewhere would make the response shape
  depend on the backend. `memstead_mem_create`'s `seed_commit_sha` becomes
  `seed_write_id` on the same grounds. The git-branch backend's value is
  unchanged — still the commit SHA of the commit that write produced.
  Consumers reading the old key must rename; there is no alias.
- **The mutation token is no longer advertised as a polling cursor, on any
  backend.** `memstead_changes_since` takes a backend-specific cursor and a
  `write_id` was never one of them on a folder mem: the folder cursor is an
  RFC3339 timestamp and the token is minted from a nanosecond clock as
  fixed-width hex, so it sorts below every ledger timestamp and passing it back
  silently replayed the entire history instead of a delta, with no error. The
  tool description, the CLI `--since` help, the MCP parameter description, the
  server instructions, the `INVALID_CURSOR` message and the folder-mem
  provenance warning now each name the value that IS a cursor: the `head` a
  prior call returned on a git-branch mem, the `ts` of the last ledger entry on
  a folder mem (empty for a first sync; a folder mem returns no `head`).
- **`memstead type` says when the schema it prints is not the workspace's own.**
  Its schema resolution had three silent fallbacks to the engine built-in
  default: no workspace at all, no writable mem loaded, and a resolved mem
  carrying no schema entry. The second is what a **quarantined** mem produces,
  so a workspace whose only mem the engine had correctly refused to load got
  the default schema's name, version and entire type catalogue printed over it
  with no mention of the quarantine — a loud, correct refusal turned into a
  quiet wrong answer three surfaces later. Each fallback now states its own
  condition above the catalogue, and the quarantine case carries the engine's
  own typed reason and repair command rather than restating them. `--json`
  gains a `fallback` field (`code` plus `detail`) on both the success and the
  refusal envelopes, `null` whenever the answer is genuinely the mem's own — a
  stable shape, so branch on the value rather than on the key's presence. The
  refusal arm carries it too: without that, a user whose own schema declares the
  type is told it does not exist, attributed to a schema that is not theirs.
  The cold-start probe outside a workspace is
  deliberately unchanged and silent: there the built-in default IS the answer,
  and a warning on the healthy path teaches readers to ignore warnings.
- **`memstead link` is retired; `memstead install` is the one verb.** Both
  fetched the same archive from the same registry URL and, since the pointer-join
  fix, both landed it in the same mount roster — two names for one act, which is
  what this campaign exists to remove. `install` keeps the name because it is the
  one a newcomer types, and because `install` / `uninstall` is the honest verb
  pair. **`memstead link <scope>/<name>` is gone; use `memstead install
  <scope>/<name>`.**

  The rename is the smaller half. `install` booted the mem-repo-only engine and
  refused a folder-shaped workspace with `UNSUPPORTED_WORKSPACE_SHAPE`, so the
  shape `memstead quickstart` produces had **no working way to attach a
  published mem at all**: `install` refused it and `link` wrote into a void.
  `install` now boots the shape-agnostic engine, which is correct on its own
  terms — a read-mem attaches to the workspace mount roster, and every workspace
  shape carries one. `memstead init`'s next-steps block points at `install`
  accordingly.
- **A binding's scope patterns resolve against its source's pointer, and a
  partial enumeration stops being reported as a percentage.** The facet walk
  honoured the source pointer for the walk and then matched every candidate by
  its *workspace*-relative path, with no join. The two readings coincide only
  for an empty pointer — the shape the scaffolder writes and the only shape the
  enumeration test covered — so under a real pointer a prefix-anchored pattern
  or a bare literal matched nothing, a `**`-prefixed one matched regardless,
  and a scope mixing the two produced a silently truncated denominator that
  coverage, verify sampling, refinement, the exclusion gate and the change
  cursor all computed over. Scope is now source-relative in both the mtime walk
  and the git strategy, which is the convention the anchor artifact decision
  already ratified and the one the rendered brief teaches. Ingest-level
  `deny_paths` stay workspace-relative, deliberately: an ingest deny spans every
  source in a binding, so it has no single pointer to be relative to.

  Three silent degradations went with it. A malformed scope pattern used to
  take the whole glob set with it — one bad allow emptied the enumeration, one
  bad deny disabled every deny — so `memstead projection init` and every other
  binding write now refuse a scope pattern that will not compile, naming it,
  and the enumerator reports rather than drops what it skipped. A partial
  enumeration now reports counts with no percentage and names the patterns that
  were skipped, instead of reducing a truncated set to a ratio. And the deny
  oracle behind `memstead projection check-path` shared a glob library with the
  enumeration but not the two rules that extend it (a literal-base
  directory-prefix block and a malformed-entry fallback), so a path could be
  denied at the hook and still counted in the denominator; one resolver now
  answers both.

  `projection verify --full` refuses a facet whose enumeration is
  known-incomplete rather than reporting complete coverage over it, and its
  empty-walk remedy text now names the retired dialect as the cause where that
  is the cause, instead of telling an author to check patterns that do select
  artifacts. **Existing bindings with prefixed scope patterns must be rewritten
  relative to their source pointer**; the fidelity report names every pattern
  still in the old dialect, whether or not the walk came up empty.
- **`memstead link` attaches a registry mem through the engine, on any workspace
  layout.** It used to walk for the workspace root itself and then read
  `.memstead/config.json` from that root, which only exists in the collapsed
  single-mem layout `memstead init` produces; every repo-overlapping, multi-mem
  and mem-repo workspace refused with `WORKSPACE_CONFIG_READ_FAILED`. The
  command now owns no layout knowledge at all: it boots the engine, which
  resolves whatever shape it is standing in, and hands the fetched archive to
  the same cache-plus-mount path `memstead install <scope>/<name>` uses. The
  attachment lands in the mount roster (`.memstead/state/mounts.json`), so what
  `link` records is what the next boot mounts, and a re-link refreshes it. The
  old success was also inert: the reference went into a `deps` list in
  `.memstead/config.json` that no engine path read, and the archive into a
  workspace-local cache whose only resolver had no callers. Both are gone.
  `deps` is now a hard tombstone in config validation (`LEGACY_FIELD_PRESENT`),
  pointing at `memstead link` / `memstead install`, and the dead Tier 3
  cache resolver is removed. `link` is a mem-repo-feature command now, like
  `install` and `uninstall`; the lean `--no-default-features` build no longer
  carries it.
- **A workspace state write no longer republishes a stale roster over a sibling
  process's registration.** `Engine::persist_state` serialized its cached mount
  list wholesale, so a long-lived process (the MCP server) silently dropped
  every mount another process had registered since that cache was taken: run
  `memstead link` or `memstead install` beside a running server and the
  attachment vanished at the server's next state write, with no warning. The
  write is now a three-way merge against the roster this engine last read or
  wrote, committed through a compare-and-set on `state/mounts.json` (the same
  lockfile shape the folder backend's config compare-and-set uses) with a
  bounded retry, so a mount the writer never touched keeps whatever the file
  says and a mount the writer removed still goes. This is the condition the
  mem-config writers already close by re-reading, reaching the one writer that
  fix did not cover. Quarantined mems are now part of what a state write
  publishes, so a quarantined mount's retained record survives instead of being
  dropped from the file.
- **`DANGLING_LINK` split into the three conditions it was fusing.** One code
  covered a body link whose target has no file, a body link to a written
  entity that the referrer does not relate to, and a relationship row naming
  an entity absent from the store. The three have three different repairs, and
  two of them were not separable from the payload at all: a reader had to
  notice whether `section` was null and guess. `dangling_links` entries now
  carry `kind`, and the consistency axis emits
  `DANGLING_LINK_TARGET_MISSING`, `DANGLING_LINK_NOT_RELATED` or
  `DANGLING_RELATION_TARGET_MISSING` with a `repair` detail. The health strict
  gate refuses on all three, so the split does not widen what passes.

  Breaking for anything that pinned the literal `DANGLING_LINK`: no surface
  emits it any more.

- **Every configured mount appears on every surface that lists mounts.** A
  mount that could not serve was either missing from a roster or present on it
  looking healthy, depending on how each surface built its list. Nine call
  sites reached for a query whose contract is "mems WITH config" while wanting
  to enumerate mounts, so a folder mount whose directory is gone was invisible
  to all of them. `workspace dump`, `mem list`, the changes path, the publish
  path and the export paths now ask for every mount, with config-derived
  fields absent rather than the mount absent.

  `workspace dump` carries a per-mem `serving` block naming why a mount serves
  nothing, and its wire version moves `workspace-dump/v0` → `v1`. The change is
  additive by the letter of the versioning rule, but the MEMBERSHIP of the
  `mems` array changed, and a consumer that assumed every row carries a schema
  pin is broken by that just as surely.

  A folder or archive mount whose storage is gone now quarantines rather than
  serving an empty graph, which ends an incoherence: the same broken mount used
  to quarantine or serve empty depending on whether the mounts file happened to
  carry a schema assertion, a detail unrelated to the breakage. `mem list`
  renders the quarantine roster in both output forms, so quarantine does not
  become a second way to disappear. A present-but-empty mem still only warns.

  Git-branch mounts are deliberately not included: a missing ref is also the
  normal state of a mem never pushed or never cloned, and quarantining it
  strands push, fetch and pull behind the condition they repair.

- **`status` and the change record answer for what they actually examined.**
  Three surfaces stated facts they had not established, and each is now
  either established or declared unestablished.

  A folder mem's drift cursor is its own change ledger, which only the engine
  writes, so an edit made to its files by anything else advanced nothing: the
  engine kept serving pre-edit content and `changes_since` reported the edit
  as never having happened. The cursor stays cheap (a directory walk before
  every operation would change the cost profile of the whole backend), and
  the engine now SAYS it cannot detect such edits, as the health warning
  `OUT_OF_BAND_EDITS_UNDETECTED`. `health --include ledger` reconciles the
  ledger against the files on demand, naming a ledger line with no file
  separately from a file with no ledger line, and writes nothing: tidying the
  ledger would fabricate provenance for a change the engine cannot attribute.
  Git-branch mems gain neither, because their change set is a real two-tree
  diff and the divergence cannot arise there.

  `memstead status` names the subject its verdict answers for, and a
  workspace with no projection bindings reports `nothing-declared` instead of
  `clean` — the old verdict read as a general all-clear over a workspace it
  never looked at. It also names every mem whose durability it could not
  establish, without claiming debt it did not observe.

  Every mutation response carries `durability_basis` beside `durable`, so a
  caller can tell an answer established from a real commit from one read off
  the mount kind. The marker itself is unchanged: removing it would move the
  same guess into every caller.

  `ENGINE_VERSION_SKEW` is now a semver comparison carrying a direction
  (whether the mem was last written by a newer or an older binary). The old
  rule was raw string inequality, so every rebuild between releases read as
  skew, which on any workspace built from source was the common case and
  drowned the real signal. It also fires at write time against the stored
  stamp, not at boot only: boot-only detection meant the first mutation both
  revealed the skew and, by restamping, hid it. Never fatal and never a
  refusal, so a deliberate downgrade still works.

- **A long-lived server can no longer write its boot-time config over a
  sibling's work.** Each mem's config was read once at boot and held for the
  life of the process, which for an MCP server is days. Eight operations cloned
  that cached struct, set one field, and wrote the whole thing back, so anything
  another process had changed in between was gone. The reload-before-operation
  invariant did not help: the staleness probe watches the entity branch on a
  git-branch mem and the change log on a folder mem, and a config-only write
  advances neither.

  All eight now go through one writer that reads the config the backend HAS,
  changes the one field on it, and writes that back. A field a call did not set
  cannot be reverted by it, because the call never had an opinion about it. The
  folder backend's write, which had no compare-and-set and no history to recover
  from, now re-reads before writing and re-applies onto whatever is there.

  When the stored config had moved on since the engine last read it, the
  operation says so on its own response as `CONFIG_WRITE_INTERVENED`, naming the
  fields the other writer had changed. Nothing is refused: the write lands on
  top of theirs. A single-writer workspace never sees the warning.

  The eighth writer is why the damage looked spontaneous: the mutation version
  stamp rides ordinary create, update, relate, rename and delete, so an operator
  saw a config field vanish during an innocuous entity write with no lifecycle
  call in sight. It is covered by the same guarantee and stays exactly as
  dormant as before.

- **An unterminated code fence can no longer swallow the rest of an entity in
  silence.** A fence opened and never closed runs to end of text in CommonMark,
  so every section heading after it was masked and absorbed into the opening
  section's body. The sections then read as empty, the entity read as healthy,
  and the next write appended the closing fence AFTER the absorbed bytes,
  sealing them inside a legitimately fenced block where nothing could tell them
  from prose the author meant to fence.

  Three changes close it. `UNTERMINATED_FENCE` refuses caller-supplied section
  content that would hide a following delimiter. The health conformance axis
  reports an entity already in that state, naming the absorbing section and the
  declared sections buried inside it, and every entity read carries
  `_unread_sections` beside them, so a section that reads as empty because a
  fence swallowed it is never mistaken for one the author left blank.
  `UNTERMINATED_FENCE_IN_STORED_BODY` refuses a write that would freeze the
  absorption; replacing the absorbing section in the same call is the way out,
  so the condition stays recoverable through the engine. Export and archive
  packaging decline the affected entity rather than baking the freeze into a
  `.mem` that would read as clean wherever it is installed.

  Heading recognition is unchanged and nothing is auto-repaired: guessing where
  an author meant to close a fence would rewrite their content. The oracle is
  the markdown referee's existing sentinel probe, not a second fence model.

- **An entity body that carries content its type does not declare is now
  visible as exactly that.** Write an entity by hand with a heading the schema
  does not declare, or a frontmatter key the type does not carry, and the engine
  said nothing. Sometimes it was right and the content survived; sometimes the
  content was gone on the next write. The reader could not tell which case they
  were in, and the engine did not distinguish them either.

  The health conformance axis gains BODY OBSERVATIONS, beside its findings and
  never among them: an undeclared heading absorbed into the catch-all, a heading
  repeated so that later bodies were not kept, and a frontmatter key the next
  write drops. Each states whether the content SURVIVES, which is the
  distinction that was missing. None of them marks an entity unconformant:
  absorbing an undeclared heading is the catch-all working as designed, and
  reporting it as a violation would fail every mem that uses the feature.

  What the axis could see before was a tautology. It linted the parsed section
  keys, which come out of the parser and are declared by construction, so every
  heading a file actually carried was invisible to it. The observations read
  what the FILE carried instead.

  **The engine also stopped handing out a value it would not accept back.** The
  catch-all re-emits absorbed content under its original heading line, so an
  agent that read an entity and wrote that section back in replace mode was
  refused its own value with `SECTION_CONTENT_INVALID`. Inside the catch-all
  only, an undeclared heading is now accepted, because the reparse absorbs it
  straight back rather than forking the entity. A DECLARED heading there still
  refuses, because that one really would fork, and every other section is
  unchanged.

  Its exact complement is a new refusal, `EMPTY_UNDECLARED_HEADING`: an
  undeclared heading with NO body under it is rejected at the write, because the
  catch-all skips empty content and the write would drop it silently. Content
  under the heading survives and is accepted; nothing under it does not and is
  refused. A write against an entity whose stored body already carries absorbed
  content, where the caller introduces none, is not affected.

- **An anchor resolution figure never renders without the population it
  covers, and two checks keep it that way.** A resolution percentage is read by
  gates and by people as health. Every finding this campaign's anchor bundle
  fixed made that percentage mean less than a reader assumes, and not one made
  it wrong in a way anyone could see: the figure was correct over whatever
  happened to be in the sidecar.

  Every surface reporting a figure now states what it was computed over and how
  much of it could not be adjudicated, and the figure and its statement render
  as ONE unit, so a compact or budget-reduced rendering cannot carry the number
  and drop the caveat. Rows the axis could not adjudicate (unobserved, spans
  never checked against their artifact, an entity end nobody reconciled) make
  the verdict inconclusive rather than clean, through the blind-spot mechanism
  that already existed. Exclusions deliberately do not: an out-of-scope or
  other-binding anchor is a complete, correct answer about a row this binding
  does not answer for, and treating it as an unknown would be the same collapse
  this release repairs elsewhere.

  The standalone anchor surface and the health axis stop collapsing two
  conditions into one bucket. An artifact that is GONE is a measured failure; an
  anchor the pass could not observe at all is the absence of a measurement, and
  the repairs differ. They now have their own counts, on the surface a reader
  reaches without a binding in hand.

  Two permanent checks in the repo-hygiene lane, running without anyone choosing
  to run them: one fails on a rendering that shows a resolution count without
  saying what it covered, or on a rendering site nobody declared, so a NEW
  surface starts red rather than starting the cycle again; the other derives the
  anchor-state vocabulary from the enum and fails until a written-down list is
  edited, because adding a state compiles once its match arms are supplied and
  reaches a fully green suite otherwise.

- **An anchors-only update no longer demands a compare-and-swap token for a
  value it cannot move.** Anchors live in a sidecar outside the entity's
  content hash, by deliberate and documented design, so an update that touches
  only anchors cannot change the hash it was being asked to present. Both
  surfaces required one anyway, which cost a read or dry-run roundtrip per
  entity and fell on exactly the backfill flows the anchor dialect exists to
  make attractive.

  `expected_hash` is now optional on `memstead_update` and `--expected-hash`
  optional on `memstead update`, for an anchors-only payload. Any update that
  changes content still requires it and still refuses a mismatch; a payload
  naming neither anchors nor content is still an empty update, with the
  recognised-key list that refusal carries now naming `anchors` and
  `anchors_unset`, which the guard had always checked.

  The requirement is enforced at the surfaces from ONE engine-side predicate,
  so a caller cannot find a write accepted on MCP and refused on the CLI. The
  engine core is unchanged: it has always checked the token only when a caller
  supplies one, and this restores that posture at the edges rather than
  inventing a new rule.

- **A span anchor could be born pointing at lines a file does not have, and a
  re-pin could lose its drift baseline without saying so.** The read path
  handled spans; the write path accepted almost anything. It checked the
  artifact's BASE path, truncating the reference at the first locator
  separator, so the span itself was never looked at.

  A `span` locator that can never address anything is now refused at write
  (`INVALID_ANCHOR`): an empty locator, and a line range that contradicts
  itself (`L0`, `L9-L2`, half a range). Where the caller supplied the content,
  a range beyond the artifact's end refuses too. Where the write path holds no
  content it does not read the source to find out, and the row records that its
  span is unverified instead, so no later surface reports it as adjudicated. A
  reference with no locator addresses the whole file and stays legal, which is
  what a span's hash covers anyway.

  A re-pin that omits `hash` now keeps the baseline already stored. It used to
  drop it, and the next verify's backfill then re-established one silently, so
  drift became unfalsifiable with nothing recording that the history had been
  lost. Supplying a hash still replaces it, and unsetting the row before
  writing it fresh is the explicit way to clear it. Every hash now records
  whether an author pinned it or the backfill inferred it, and the fidelity
  report counts both the unverified spans and the inferred baselines.

  A payload naming one `(artifact, grain, class)` triple twice is refused
  rather than collapsed to its last occurrence: that triple is the sidecar's
  merge identity, so the earlier rows used to disappear and the caller was
  never told. The merge identity itself is unchanged, so the same artifact at
  file grain and at span grain is still two rows.

- **An anchor is an edge with two ends, and the engine checked one of them.** A
  mem whose entity files were gone, but whose sidecar still named them, verified
  clean at a hundred percent: every row resolved against a source that was fine,
  and nothing asked whether the entity end still existed. Coverage counted the
  artifact as covered for the vanished entity, findings named nonexistent ids,
  and the prune pass could propose deleting an entity that was already gone.

  A row whose entity the mem no longer holds is now reported as dangling, on the
  binding report, the standalone anchor surface and health alike, named rather
  than counted. It is its own condition, not a fifth anchor state: the four
  describe the artifact end, and a vanished entity says nothing about the
  source, so folding it into `orphaned` would name the opposite repair. Nothing
  is deleted or rewritten, because the row is the only remaining trace that
  something wrote the mem from outside the engine.

  The check reads an absence from the loaded graph, which is evidence only when
  the graph holds everything the mem has. A mem that is not mounted, is
  quarantined, has not run its lazy load, or carries a file that failed to parse
  is reported as unreconciled instead, with the reason. Every one of those would
  otherwise have turned the mem's whole sidecar into false dangling rows, and a
  zero over an entity end nobody examined reads as health.

- **A binding's fidelity report answers for that binding's anchors.** The anchor
  axis had no notion of the population it covered, so it covered everything in
  the destination mem: a binding's report scored anchors another binding wrote,
  and anchors pointing at artifacts its own scope excludes, and narrowing a
  binding's scope until it matched nothing left its anchors still scoring. Three
  separately reported problems were this one defect. The report, the findings
  pass and the prune proposal now answer for one binding's population;
  provenance decides membership where an anchor records it, the binding's
  declared scope decides where it does not.

  Excluded anchors are named in the report with the reason, never deleted,
  rewritten or silently dropped, and an excluded anchor can no longer raise a
  finding against a binding that did not write it. The report states what its
  denominator counted, giving the distinct-artifact count beside the row count
  so several rows on one artifact cannot read as several artifacts, and says so
  when anchors recording no producing binding are included by the
  pre-provenance fallback. That fallback is deliberate: filtering on the field
  strictly would empty the axis for every mem written before it existed, and an
  empty report reads as success.

- **memstead.ai's playground derives its live session graph from the engine's
  shared topology.** The server behind the playground carried its own
  coordinate-free projection, written when reaching the engine's types meant
  dragging a git library in; `Engine::mem_topology` now lives in the engine
  core that server already depends on, so the re-derivation is gone and what
  remains adapts the shared projection to the wire shape the browser already
  consumes. Nothing in the shipped binary changes. Two behaviours do: an entity
  the community partition never assigned is no longer presented as a member of
  the first cluster, and the per-response community index now ranks the
  clusters present in the projected mem rather than every cluster in the
  workspace.

- **The frontmatter delimiter contract has one implementation.** The tolerant
  split, the borrowing peek and the strict validator each carried their own
  copy of the same ten lines of offset arithmetic, held in step only by a
  differential property test, and the byte-order-mark was stripped in four
  places under three different conventions (one of which had already lost a
  marked file's entire frontmatter). All three are now thin wrappers over
  `split_frontmatter_core`, which strips the mark once and returns borrowed
  slices; the wrappers differ only in what they do with a missing or unclosed
  block, which is the only thing that ever differed. Behaviour is unchanged and
  was proved so: the pre-consolidation implementations were kept alongside the
  core and compared over 12,017 cases (the generated adversarial space plus the
  committed corpus) before being removed. The differential property retired
  with the duplication it existed to hold in step; the corpus replay stays, now
  exercising the core.

  One behaviour did move, and it is a latent inconsistency the consolidation
  exposed rather than a regression: archive validation used to strip the mark
  before handing bytes to the parser, so a marked file inside an archive was
  content-hashed without its mark while the same file read locally was hashed
  with it. Both paths now hash the bytes as they arrived, so the two agree.
- **The MCP tool descriptions are text files, not string literals.** Every
  agent that touches Memstead reads them before it does anything else, which
  makes them the product's most-read prose; as single-line literals inside a
  17,000-line source file they were diffable only as multi-thousand-character
  one-line changes, and no prose checker reached them, because the vocabulary
  lint walks `.md` and `.mdx`. All 32 descriptions across both servers, plus
  both servers' session-start instruction strings, now live under
  `crates/memstead-mcp/descriptions/` and are compiled in with `include_str!`.
  The served bytes are unchanged. rmcp's `#[tool(description = ...)]` accepts
  a string literal and nothing else, so the attribute carries no description
  and a router wrapper stamps each one in; every consumer reaches the router
  through that wrapper.

### Fixed
- **Folder-workspace CLI mutations carry `write_id` in `--json`.** The
  standing reason the token exists on every mutation response is that
  omitting it anywhere would make the response shape depend on the backend —
  and the CLI's folder arms of `create`, `update`, `rename` and `delete` did
  exactly that while the MCP filesystem flavour and the CLI's own `relate`
  and `conflicts` returned it. All four now carry it; a parity test pins the
  field non-empty across the four operations on a quickstart workspace.
- **The commit-less flavour stops promising commits.** The lean
  `memstead_rename` description claimed referrers are rewritten "in one
  per-mem commit" on a substrate that keeps a change ledger and no commits;
  the `memstead_mem_delete` description and the CLI's `mem delete` help said
  "no per-mem commit" without saying for which backend. A new guard walks
  every lean description and refuses any sentence asserting a commit that
  neither negates it nor names the mem-repo flavour as its subject — the
  four `write_id` guards key on the token and were blind to a bare commit
  claim.
- **The lean entity read injects `_hash` only into a document that starts
  with frontmatter.** It previously inserted at the first `---` found
  anywhere, which on a frontmatter-less rendering would have landed the
  field inside body text at the first thematic break or fence; the full
  flavour's starts-with shape is now used on both.
- **A folder mem's change feed refuses a non-timestamp cursor instead of
  silently replaying the whole history.** The folder ledger's cursor is an
  RFC3339 timestamp compared lexically, and a mutation's `write_id` —
  fixed-width hex minted from a nanosecond clock — sorts below every
  timestamp, so passing it back as `since` returned the entire history with
  no error. Now any `since` that is neither empty, nor the empty-tree
  sentinel, nor RFC3339-parseable refuses with the existing `INVALID_CURSOR`
  code, on both the unified engine (MCP full flavour and CLI) and the lean
  filesystem server; the message names the value that IS a cursor. The lean
  server additionally honours the empty-tree sentinel as "from the
  beginning" — before, its hex sorted above every timestamp and silently
  returned an empty window. Callers passing a real timestamp, an empty
  string, or the sentinel are unaffected; a previously-accepted garbage
  cursor now refuses instead of answering wrongly.
- **`INVALID_FIELD_VALUE` and `SECTION_CONTENT_INVALID` are named where the
  schema-conformance vocabulary is listed.** Both are codes the engine can
  produce, and both were missing from the recovery-code lists agents read: an
  agent hitting either had no entry telling it the refusal carries a recovery
  payload. `INVALID_FIELD_VALUE` joins its siblings in the server
  instructions, the `create` and `update` descriptions and both dry-run
  descriptions; `SECTION_CONTENT_INVALID` joins the instructions and the same
  two descriptions. The `create` and `update` descriptions sit against a hard 2048-byte
  cap, so each dropped two repetitions of a cross-reference it made three
  times rather than raising the cap, and each still points at the server
  instructions once.
- **The server instructions no longer advertise five error codes nothing can
  produce.** `MEM_NOT_WRITABLE`, `MEM_BRANCH_MISSING`, `VCS_ERROR`,
  `EXPORT_ERROR` and `WORKSPACE_SCHEMAS_ERROR` were listed in the instruction
  string's error-code enumeration with no construction site anywhere in the
  workspace and no entry in the generated Error Code Index, so an agent was
  told to expect refusals that cannot arrive. `READ_ONLY_FIELD` and
  `SECTION_NOT_UPDATABLE` were the mirror defect: both carry a `details`
  recovery payload and neither was in the recovery-payload list. All of it is
  now gated mechanically rather than by a hand-maintained list. Five checks,
  all derived from the same scan the Error Code Index is built from: the
  instructions may name no code the workspace cannot construct; the canonical
  recovery-payload list may omit no schema-conformance code the engine can
  return; no such code may go unnamed in the MCP prose entirely; no per-tool
  excerpt may name an impossible code; and every per-tool excerpt carries the
  elision marker that declares it an excerpt rather than a complete set. The
  hand-maintained list is what missed `INVALID_FIELD_VALUE` in the first
  place, and it is now held to the first of those rules too.
- **Transport and diff act on the branch the mount declares.** `memstead
  push` and `memstead pull` used to reconstruct their refs from the mem's
  name, so a mem whose declared branch sits under a namespace
  (`refs/heads/team/engine` for mem `engine`) could not be pushed or
  pulled at all — the error named a ref the engine invented, and pull
  blamed the remote for not carrying the mem. Both now derive every ref
  (local, remote-tracking, and the pre-transfer validation ref) from the
  mount's declared branch; the bare `HEAD` shorthand in `memstead diff`
  and a `HEAD`-based `since` in `memstead changes` re-anchor on it too.
  Flat workspaces, where branch and mem name coincide, behave exactly as
  before. The pre-push schema gate additionally stops passing when it
  could not run: validating a ref that does not resolve now refuses
  naming the declared branch instead of silently skipping validation and
  pushing anyway.
- **`release-verify.sh` tells "the channel is wrong" from "I could not
  look".** On the 0.12.0 release an exhausted anonymous REST quota made the
  script render two correctly-serving channels as red, because every one of
  its eight HTTP channel reads classified any failed read as a
  disagreement. Each read now has a third state: a channel whose read
  failed reports as UNMEASURED, naming the cause drawn from the response
  itself (HTTP status, or the rate limit with its reset time), rides the
  report-only exit code, and the verdict line stops claiming "every channel
  serves" when channels went unread. The pre-flight probe now fails on the
  response shapes the channel reads fail on (a rate-limited 403 used to
  pass it), an unresolvable target is a named skip instead of a red run,
  and a mistyped flag is fatal instead of sharing the exit code the CI
  wrapper renders as a green "everything serves" notice. The reads stay
  anonymous by decision: the script measures what a stranger receives.
- **The one-line installer no longer depends on GitHub's anonymous REST
  quota (cold-start finding, major).** `install.sh` resolved "latest" via
  `api.github.com`, so on an address that had spent the 60-per-hour
  anonymous budget (an office, a campus, a CI runner) the entry page's
  first command failed with a bare 403. Resolution now follows the
  release host's own redirect (`/releases/latest` → `/releases/tag/<tag>`),
  which is not quota-bound; no code path contacts the REST API and no
  request is authenticated. A genuinely failed resolution now names the
  probable cause and the `--version <tag>` / `MEMSTEAD_VERSION` escape and
  exits non-zero. Riding along: a failed child-installer download is no
  longer masked by the `curl | sh` pipe (the script downloads first, runs
  second), and a fixture suite with a faked rate-limited API pins all of
  it in the ordinary test run.

## [0.12.0] - 2026-08-24

### Changed
- **Every surface that teaches an install now states the restart wall
  (sealed-gate finding F7).** A running agent session does not attach an
  MCP server added while it runs, and picks up a freshly installed
  plugin's skills only after `/reload-plugins` or a restart. Neither is a
  Memstead defect and neither was stated where the install is taught, so
  the documented next step (`/setup` right after installing) answered
  `Unknown skill`. The README, the plugin README and marketplace entry,
  `install.sh`, both published crate readmes, the setup skill,
  `quickstart --help`, the docs-site getting-started guide and the skills
  page each now carry the disclosure that applies to what they teach, in
  one shared phrasing, and `scripts/check-restart-disclosure.sh` (wired
  into `run-tests.sh`) fails the suite when a surface it holds loses its
  sentence. It holds every file that quotes an install command, plus a
  named few that teach without quoting one. The README and
  getting-started additionally name the non-interactive path for a
  session that cannot restart: wire it before launch (`quickstart` writes
  `.mcp.json` first; `--mcp-config` and `--plugin-dir` load both at
  startup).

### Fixed
- **`export --format mem` and bare `publish` work on every workspace
  `quickstart` and `init` produce (sealed-gate finding F6).** The
  archive assembly resolved the mem's `.memstead/config.json` against
  the WORKSPACE root, which only coincides with the mem folder in the
  legacy single-mem layout, so the README's own sharing example failed
  on the front door's output with `ARCHIVE_ASSEMBLY_FAILED`. Both
  callers now export through the engine, which reads whatever layout it
  booted (the mount roster locates the mem folder and its config), so
  the typed refusals stay backend-symmetric; the legacy folder assembly
  remains only as bare publish's fallback where no engine boots. The
  full quickstart → export → install round trip is pinned in the suite.
- **A relationship row's target never crosses a line (fuzz finding,
  long tier, CI dispatch).** The row pattern captured `[[…]]` across
  newlines, but a generated multi-line token re-enters the mask with
  different structure (a following dash-plus-tab line reads as
  list-item indented code and swallows the closing `]]`), so such a row
  silently vanished one round later. A row is a line: targets spanning
  newlines, which can never exist as entity files, are consistently not
  relationships in any round. Triggering input pinned in the shared
  corpus.
- **Lenient wiki-link decoration stripping runs to a fixpoint (fuzz
  finding, long tier, CI dispatch).** Each strip pass can expose work
  for another: an anchor or alias cut exposes trailing whitespace, and
  trimming that whitespace can expose a `.md` suffix the same pass
  could not see (`x.md` followed by a CR), so the decoded id changed
  between rounds. The lenient decoder now strips and trims until
  nothing changes; the strict decoder still runs one pass and refuses
  the leftover shapes. Triggering input pinned in the shared corpus.
- **The catch-all merge closes fences in context, never per piece in
  isolation (fuzz finding, long tier, local runs).** The same bytes can
  be a fence in one context and prose in another (CR line endings, lazy
  continuation), so a per-piece close judged in isolation injected a
  spurious closer that the generator's part-level close paired into an
  empty fence block, growing the document by two fence lines per round;
  skipping closes entirely instead let a dangling piece fence swallow
  the next piece's heading. The merge now appends pieces raw and makes
  every close decision over the running string after each append, so a
  dangling fence still closes before the next piece and no closer is
  added for a construct the document does not read as a fence. The
  whole ten-crash corpus passes under this design; four earlier
  variants are disproven in the plan's log. Triggering input pinned in
  the shared corpus.
- **A relationship row whose target decodes to an empty path is not a
  relationship (fuzz finding, long tier, local run).** A degenerate raw
  target like `[[specs--]]` survived the row pattern but decoded to an
  empty-path id on the tolerant path; the generator then rendered
  `[[]]`, which the row pattern cannot re-capture, so the row silently
  vanished one round later. Such rows are now skipped at parse, exactly
  as rows that never match the pattern are; both strict gates already
  refuse such targets. Triggering input pinned in the shared corpus.
- **The catch-all re-emits each unknown section under its original
  heading line, byte-verbatim (fuzz finding, long tier, local run).**
  It previously rebuilt the heading from the derived section key, and
  the rebuilt form can mean something else to the CommonMark referee: a
  CR inside a heading is a line ending of its own, so its tail can be a
  live fence opener that the derived key lost; the re-parse then read
  formerly-fenced content as structure and dropped a promoted empty
  heading entirely. Verbatim re-emission makes the reconstruction
  byte-faithful; ordinary single-word unknown headings render exactly
  as before. Triggering input pinned in the shared corpus.
- **Section values are trimmed exactly once (fuzz finding, long tier,
  local run).** `parse_markdown` re-trimmed every section value after
  the splitter had already normalised it, silently promoting a
  whitespace-prefixed first line (a vertical tab before backticks) to
  column 0, where the CommonMark referee saw a fence opener the stored
  form did not have; the section structure then shifted between
  parse-generate rounds. The splitter's trim (leading blank lines
  dropped, first visible line byte-exact, trailing trimmed) is now the
  only content trim. Triggering input pinned in the shared corpus.
- **Every generated part is fence-checked, the title and relationships
  block included (fuzz finding, long tier, local run).** The generator
  fence-terminated only section content, but a TITLE can itself carry a
  live fence opener (CR characters inside it are CommonMark line
  endings of their own, so its tail after a CR is a line that can open
  a fence); the opener then masked every following section heading on
  the next parse and the document collapsed to empty required sections.
  `close_open_fence` now runs over exactly the bytes of every emitted
  part. Triggering input pinned in the shared corpus.
- **The catch-all merge keeps every merged piece fence-balanced (fuzz
  finding, long tier, first local run).** The catch-all reconstruction
  concatenates section contents; a piece ending inside an open code
  fence inverted the mask parity of every piece after it, so a
  `## Specifies` that sat safely inside a fence in one parse surfaced
  as a real duplicate heading in the next, and the duplicate rule
  dropped the tail of the section. Each merged piece now closes its own
  unterminated fence (same oracle helper the generator's section closer
  uses), judged over exactly the bytes emitted, the re-emitted heading
  line included: a heading can itself carry a live fence opener, since
  CR characters inside it are CommonMark line endings of their own, and
  judging the content alone read an in-piece closer as a fresh opener
  and grew the document by one fence line per round. Both triggering
  inputs pinned in the shared corpus.
- **A same-mem relationship target whose path matches the cross-mem
  dash form self-qualifies (fuzz finding, long tier, third dispatch).**
  A same-mem target with a path like `nttype--ospity` rendered as a
  bare wiki-link, which the decoder's tier 0 reads back as the
  cross-mem `nttype:ospity` — a different id, so the edge drifted and
  parse-generate was not a fixpoint. The generator now asks the decoder
  itself whether the bare path re-decodes to the same id and renders
  the colon-qualified form when it does not; unambiguous targets stay
  bare and canonical bytes are unchanged for them. Triggering input
  pinned in the shared corpus.
- **An indented heading-lookalike on a section's first content line no
  longer becomes structure (fuzz finding, long tier, second dispatch).**
  The section splitter's full content trim promoted a first line like
  ` ## Specifies` to column 0 inside stored content; after the
  catch-all re-emit, the next parse read it as a real duplicate section
  heading and the duplicate rule dropped the content, breaking the
  parse-generate fixpoint and losing body text on the tolerant path.
  Leading blank lines still drop, but the first visible line now keeps
  its indentation, so the lookalike stays content forever. The mutation
  path's embedded-heading refusal keeps its own full trim and refuses
  exactly what it refused. Triggering input pinned in the shared corpus.
- **Lenient wiki-link ids are stable under parse-generate (fuzz finding,
  long tier, first dispatch).** The alias and anchor cuts inside the
  decoration strip run after its whitespace trim, so `[[foo |label]]` or
  an anchor following a line break left trailing whitespace inside the
  lenient id; the generated relationship row then re-parsed to a
  different id on the next round, breaking the one-round fixpoint. The
  lenient decoder now trims what the cuts expose. The strict gate is
  untouched and still refuses those shapes; the triggering input is
  pinned in the shared fuzz corpus and replayed by the normal suite.
- **The fuzz workflow builds for the runner's real host triple.** Its first
  dispatch failed before fuzzing: the prebuilt cargo-fuzz binary is
  musl-linked and defaults to its own compile-time target, where
  AddressSanitizer cannot link against static libc. The run step now passes
  `--target` resolved from `rustc -vV`.
- **The sync brief names the drift-clearing move.** Its drifted-findings
  guidance said "update the entity, then re-verify to advance the baseline",
  which leaves a drifted anchor drifted: neither an entity update nor the
  baseline advance re-baselines the anchor hash. The missing two-step
  (re-declare the anchor on the entity without a hash; the next
  binding-backed verify backfills the freshly observed hash) cost a live
  sync session the whole discovery loop on 2026-08-24. The brief states it
  now, in the drifted group's own instruction.

## [0.11.0] - 2026-08-24

### Added
- **Code-map preparation: anchors on code drift on interface changes
  only.** The registry's third flavour, `code-map`, on path-shaped
  sources: a scoped file's prepared form is its interface digest
  (imports, exports, declarations with their signatures; comments,
  formatting and bodies invisible), heuristic and language-family aware
  by extension (JS/TS and the C-like families, Python, a Vue or Svelte
  component's script block, canonical JSON; everything else whole). A `tree` anchor
  under it hashes the code map of every scoped file under the tree (file
  hash and path, path order), which closes the tree grain's
  recorded-but-unhashed residue for code sources; a tree anchor on a
  source without a code map stays unhashed and resolves `recheck`, the
  stated remainder. Write-time `content` on a prepared source is hashed
  through the source's preparation, so the recorded hash is the one
  observation computes, and the build brief tells the agent when a
  source's anchors hash a prepared form. Measured on a 214-file
  JS/Vue/Python corpus: the digest is 4.6% of the raw bytes, and over 300
  commits 252 of 424 scoped file changes (59.4%) were body-only, so a
  code-map anchor stays quiet for them. `PREPARATION_IMPL_VERSION` is 3.
- **Delivery preparation: one file, many units, one order.** The
  preparation registry's second touchpoint is live: a source declaring a
  delivery preparation is delivered as units, addressed `<path>#<key>`,
  in a total order derived from the units' own keys (never discovery or
  directory order), identical on every pass. First flavour,
  `dated-entries`, for path-shaped sources: a unit begins at every line
  opening with an ISO date or date-time (after markdown markers), its key
  is the normalized stamp (an ordinal disambiguates same-stamp entries in
  one file), and a source's units deliver in stamp order across files, so
  a chronological corpus (logs, transcripts, journals, mail threads) is
  never shuffled and unit N assumes only the units before it. A first run
  delivers every unit; a change run delivers only the added, changed and
  removed units at their ordered positions, diffed against the git
  baseline content (without one, every unit of a changed file, flagged as
  coarser). The brief presents the sequence numbered by position, capped
  at the build operation's `batch_size` with the remainder counted and
  re-presented in order as units are disposed; `projection advance`
  accepts unit ids, and an anchor over exactly a unit auto-disposes it (a
  file-level anchor never disposes units). A span anchor `<path>#<key>`
  on such a source hashes the unit, not the file: an unchanged unit in a
  changed file still resolves, a removed unit orphans, an edited one
  drifts. Sources declaring no delivery preparation keep file-granularity
  delivery byte-for-byte. `PREPARATION_IMPL_VERSION` is 2.
- **The preparation slot has a registry, and its first flavour.** A
  source's `preparation` names a preparation the engine registers
  (`memstead-base::preparation`); the engine refuses only an identifier
  the registry does not know (the same `PreparationUnsupported` shape,
  now naming the registered set) on the edit paths and, for a hand-edited
  record, on the brief-render path, whose message no longer speaks of
  facets. A registered identifier over a medium whose anchor namespace
  admits none of its grains refuses too (`PreparationGrainMismatch`). The
  registry is consulted at anchor observation (the prepared form an
  artifact hashes as; the standalone `verify-anchors` and the
  binding-backed verify share that one site and inherit every entry) and
  is reserved at ingest delivery. First flavour, `entity-load-bearing`
  for graph sources: an entity-grain anchor's prepared form is the stable
  serialization of the type's load-bearing sections (the new optional
  `load_bearing` flag on a schema section; else the required sections;
  else every section), so a notes-only edit keeps a dependent's anchor
  resolving while a load-bearing edit drifts it. A source declaring no
  preparation observes byte-for-byte as before. The `url` grain acquires
  its prepared form the way path grains do, over the content its observer
  supplies: anchors accept `content` beside `hash` (the engine computes the
  prepared hash; mutually exclusive with `hash`, refused for the
  `entity`/`tree` grains and non-hash classes), and a url anchor defaults
  to `hash_stability: unstable`. `PREPARATION_IMPL_VERSION` is 1: every
  binding's `hash(D)` changed, so every prior finding is superseded by
  construction and re-derived by the next verify (findings are
  measurements, never content).
- **An untagged release is now a machine-visible state.** 0.9.0 was cut,
  committed and pushed and never tagged; every channel kept serving 0.8.1
  for four days while the tree said otherwise, and nothing noticed.
  `scripts/untagged-release.sh` compares the workspace version against the
  newest tag the remote actually carries (`git ls-remote`, SemVer
  precedence, so `v0.9.0` never outranks `v0.10.0`) and refuses once the
  release commit has sat on `origin/main` for more than a day, naming the
  commit and its date; `scripts/ci-status.sh` runs it before its CI
  readout, derives the repository from `origin` instead of a hard-coded
  name, and counts only the checks of workflows this repository defines
  (GitHub's own Dependabot updater run had painted a green `main` red).
  The `untagged-release` workflow runs the same check daily and on
  dispatch, keeping exactly one issue in step with it (filed when it trips,
  updated while it holds, closed when it clears, nothing on a tagged
  state) through `scripts/untagged-release-issue.sh`.
- **`release-verify.sh` runs inside the release itself.** Declared as
  cargo-dist's post-announce job (`custom-release-verify`, rendered into
  `release.yml` by `dist generate`), it asks every channel from outside
  after `announce` and fails the run when one disagrees with the tag; the
  same workflow runs on dispatch with a tag input, so any past release can
  be re-verified from CI. It also reads the run's own publish jobs and
  fails on one that concluded `skipped` on a non-prerelease (dist's
  `announce` accepts that by design; it is how a channel stays unfed
  behind a green release). The script now has four exit codes: 0 green, 1
  fatal, 2 green with report-only findings (today: the local tree standing
  ahead of the verified tag), 3 skipped for lack of network
  (`MEMSTEAD_VERIFY_OFFLINE=1` simulates it), plus explicit option
  parsing (`--run-id`, `--repo`) beside the positional version.
- **`xtask release` refuses two more states that shipped.** A missing
  flagship directory is a refusal, not a warning, unless
  `--allow-missing-flagship` names the skip (the docs-vs-binary guard then
  runs nowhere, and the cut says so). An `[Unreleased]` section above 64
  KB is refused naming its size unless `--allow-large-body` is passed:
  cargo-dist lifts the section into the GitHub Release body and the
  Homebrew publish job died on 0.6.0's 81 KB of it.

- **`MOUNT_UNBACKED`: a mount that resolves to nothing says so.** A
  git-branch mount whose branch was never created (or was deleted), a
  folder or archive mount whose path is gone, and a mount whose storage
  holds no entity all listed as "zero entities" and sat in the writable
  roster without a word; the dogfood workspace carried two such mounts
  for weeks. Boot and reload now raise `MOUNT_UNBACKED` with
  `details.reason` in `missing_ref | missing_path | empty` and the
  location named; a mount serving at least one entity is silent. The
  backend trait gained `storage_present()` (the branch ref, the folder,
  the archive file) so the probe can tell "never created" from "empty".
  The overview, the cold-start surface, carries it too: an unbacked mem's
  roster entry names the reason and location under `Unbacked:` (the
  structured `mems[].unbacked` field) and the warning rides
  `## Warnings`, so `Entities: 0` is never mistaken for an empty mem.
- **`health --strict` refuses configuration defects.** Always on, no
  include needed: `SCHEMA_PIN_MISMATCH`, `SCHEMA_UNSTAMPED_SOURCE_ROT` and
  `MOUNT_UNBACKED`. With `integrity` included: `DANGLING_LINK` and
  `ORPHAN_STUB`. Stale entities, drifted anchors and
  `SCHEMA_GENERATIONS_BEHIND` stay advisory. A strict run used to exit 0
  on a workspace with three pin mismatches, two rotted schema packages,
  two unbacked mounts, seven stubs and fourteen dangling-link findings.

- **`memstead export --format mem --self-contained`.** A mem that
  references its sibling mems exported with `DANGLING_CROSS_MEM_EDGE_IN_EXPORT`
  warnings and then could not be installed anywhere, not even back into
  the workspace it came from: `install` refuses cross-mem relationship
  rows by design. The flag drops every `## Relationships` row whose
  target lives in another mem, re-packs canonically and proves the
  result with the strict validator `install` runs; each dropped edge is
  reported as `CROSS_MEM_EDGE_DROPPED`. Section text, body wiki-links
  included, is never touched, so an alias row synthesised from a body
  link loses nothing the body does not still say. The dogfood
  workspace's retired `features` mem (fifty such rows, all alias rows)
  is the first archive to round-trip this way.
- **The prose is checked against the binary it describes.**
  `ci/check_prose.py` (built-ins only) runs every `memstead` invocation
  in fenced `bash`/`sh`/`console` blocks and in the `run:` lines of fenced
  `yaml`, every flag attached to one, and every relative link against a
  given binary's `--help` tree; a `--scope whole-file` switch extends the
  sweep to inline code and prose for the documents that want it; a
  docs-site content tree named with `--routes-root` resolves its links
  by route (`../../glossary/`, `/reference/cli/cli/`); placeholders and
  prose phrases are allowlisted by the `xtask/docs-guard-allow.txt`
  format, now with `flag:` and `re:` entries; a directory argument
  stands for every Markdown file below it. `run-tests.sh` gained the leg
  "the public prose describes the binary" over the README, CONTRIBUTING,
  GLOSSARY, VISION, the examples README, `docs/**` (minus the divergence
  corpus), the docs-site guides and concepts and the plugin's Markdown,
  after the checker's own fixture self-test. `xtask release`'s
  docs-vs-binary guard is the same checker at whole-file scope over the
  flagship (the Rust extractor is gone); it still refuses an unknown
  command and still honours `--allow-missing-flagship` and
  `--allow-large-body`. First catch: `docs/proof/reconstruction/README.md`
  documented a `memstead stats` that does not exist.
- **`release-verify.sh --prose` reports the gap between the prose and the
  published tag.** It resolves the highest tag on `origin`
  (`untagged-release.sh --highest-tag`), downloads that release's CLI
  archive once into a cache (`MEMSTEAD_VERIFY_CACHE`), runs the checker's
  user-facing subset (README, guides, plugin; `--prose-set` overrides)
  against the published binary, and prints file and flag per gap as a
  report-only finding (exit 2), never trusting a local binary's version
  string; exit 3 `SKIPPED: no network` when the archive cannot be
  fetched. The same script now checks the changelog: every `## [X.Y.Z]`
  header other than `[Unreleased]` must have a tag on `origin` or a
  "never published" note, and every compare link must name refs that
  exist (`MEMSTEAD_VERIFY_TAGS` seeds the tag list for tests).
- **The plugin gates `--repo` and `--consume` on the recorded binary.**
  Beside the anchors gate, `binary-version.mjs` carries `REPO_MIN` and
  `CONSUME_MIN` (both 0.10.0) in a `CAPABILITIES` table and a
  `capabilityGate(root, name)` (CLI: `gate <dir> [capability]`). Setup
  drops `--repo` from `quickstart` and the ingest router drops
  `--consume` from `projection brief --all` when the recorded binary
  predates the flag, each saying which version it found and which it
  needs; at or above the minimum the flag passes silently; a missing or
  unparseable record degrades the same way. `--fail-on-findings` and
  `--redact-anchors` sit on no skill path and stay ungated.
- **`memstead schema <ref>` renders a built-in package's README for the
  generation it ships in.** Sibling generations of a built-in carry the
  README of their first generation verbatim (the bytes are sealed by the
  retention guard, so no in-place edit may fix them), and 17 of 25 packages
  stated a version that was not theirs. The render substitutes every
  `<name>@<x.y.z>` reference to the package's own name with the resolved
  pin and leaves everything else alone; `<ref>` is a pin (`planning@0.4.0`)
  or a bare name, resolved to its newest generation for a read (`install`
  keeps refusing bare names, so a pin stays explicit). `--json` carries
  `schema`, `name`, `version`, `origin` and the rendered `readme`. The
  `new` / `validate` / `install` subcommands are unchanged. Exercised for
  every built-in in the binary, so a new generation is covered the day it
  ships; `MANIFEST.toml` and the sealed bytes are untouched.

### Fixed
- **`mem set-schema` repairs a `SCHEMA_PIN_MISMATCH` instead of
  declaring a noop.** The noop check compared the target with the mount's
  expectation in `mounts.json`; with the mount ahead of the mem's own
  config (the mismatch the warning names) a target equal to the mount
  answered "noop" and the authoritative config stayed on the old
  generation forever. The check now asks the pin the engine actually
  serves; when the served pin already equals the target and only the
  mount expectation lags, the expectation is aligned and reported as
  switched. An in-flight dual-pin migration still completes through the
  conformance gate.

### Changed
- **`SUSPICIOUS_NESTED_PREFIX` says what it can tell.** When the link's
  prefix is itself a mounted mem the message reads "target missing in
  mem X" (a well-formed cross-mem reference whose target is absent; every
  one of the eight hits on the dogfood graph); when the prefix only
  matches a mem name's last segment it reads "prefix 'X' is not a mounted
  mem" (the rename-drift pattern). `details` gains `target_mem` and
  `prefix_mounted`. The old wording called every case "almost certainly
  mem-rename drift", which was wrong eight times out of eight.
- **`memstead-mcp --version` prints the stamped build version**
  (`<semver>+g<sha>[-dirty]` inside a git checkout), the same string the
  server hands out as `serverInfo.version` and the CLI already printed.
  The bare crate version could not tell a day-stale release binary from a
  fresh build of the same version line, which is exactly what a
  binary-staleness check needs to know. The stamp's `-dirty` suffix now
  reflects modified build inputs only (`crates/`, `Cargo.toml`,
  `Cargo.lock`): a modified doc, workflow or folder-mem file elsewhere in
  the repository changes no byte of the binary. The build script now
  re-runs when those inputs change, not only when a ref moves; before, an
  edit-then-build kept the clean stamp computed at the last commit and the
  `-dirty` suffix could not appear in the ordinary flow.

## [0.10.0] - 2026-08-23

Forward compatibility: this release is one schema-language generation. Schema
packages declaring any of the wave's new keys (`when_field` / `when_value` on
`required_outgoing` blocks, `must_reach`, `relationships.acyclic_sets`,
`status_propagation.rel_types`, `signals`, `relationships.labelling`) need
engine 0.10.0 or later; older engines refuse them at parse
(`deny_unknown_fields`), never load-and-ignore.

### Fixed
- **Three parser defects found by the new adversarial harness, each fixed
  at the parser.** A BOM-prefixed local file silently parsed as all-body
  and lost its entire frontmatter, while the strict validator and the
  archive path strip the BOM; the tolerant parser family (`parse_markdown`,
  `body_after_frontmatter`, `peek_type_from_frontmatter`) now lands on the
  same boundary. A document carrying multiple non-schema sections
  reconstructed its catch-all in hash-random order, so canonical bytes
  differed from parse to parse of the same input; non-schema sections now
  re-emit in document order. A section whose content ends inside an open
  code fence absorbed every section the generator wrote after it on the
  next parse: content shifted between sections and the document grew on
  every parse-generate round. The generator now terminates the open fence
  (balanced content is byte-identical to before), making parse-generate a
  fixpoint after one normalising round. Each fix is pinned by a fixture
  regression test carrying its triggering input.
- **Four README/SECURITY doc-vs-code drifts, reported by an external review of
  the public repository.** The repository table no longer claims the serve and
  bridge crates live here (they are in the private commercial repository; the
  wasm crate stays listed); `memstead_search` is described as ranked lexical
  search (BM25) rather than "exact"; the CI claim now states the actual
  posture (developed on macOS, CI test gate on Linux only); and the trust
  posture's `SECURITY.md` pointer lands on real content — the third-party mem
  trust model (structural-only schema serving, `origin` tagging, the host-side
  residual) is now documented there, with a bypass declared in scope for
  security reports.

### Added
- **Grounded labelling over a declared attack set:
  `relationships.labelling`, plus chain-shape statistics.** A schema can
  name which of its rel-types constitute `attack`; the engine serves the
  grounded labelling (the one argumentation-semantics computation that is
  parameter-free, unique, polynomial, and explainable by construction) as a
  reported observation with its evidence: `accepted` / `defeated` /
  `undecided` per non-stub entity of a declaring mem, a defeated label
  always naming its accepted direct attackers, an undecided one the open
  attacker set that keeps it open. Served on entity reads (`_labelling` on
  the structured envelope; `_label` in the rendered frontmatter plus a
  `## Labelling` evidence section) and on the include-gated `labelling`
  health axis (counts per label, defeated/undecided lists with evidence,
  and `cross_mem_edges_excluded` — a cross-mem attack edge is excluded and
  counted, never guessed). The labelling is deliberately support-blind: a
  defeated supporter never flips what it supports. An optional `support`
  walk (the `must_reach` grammar: `relationships`, `direction`,
  `terminal_types`) adds chain-shape statistics to the read (`depth`,
  `branching`, `terminal_share`, `defeated_in_support`,
  `undecided_in_support`); the engine serves numbers, the reader judges.
  Labels are never stored, never gate writes, and the memo invalidates
  wherever the community memo does (every mutation, drift reload,
  quarantine transitions, apply-commit). The loader refuses empty or
  undeclared attack sets and malformed support blocks; schemas without the
  declaration keep byte-identical responses everywhere.

- **Aggregate signals: `signals` on type definitions, served with their
  evidence.** A type can declare exact, parameter-free counts with declared
  thresholds: this wave ships one `kind`, `edge_load` (count the edges of an
  inline relation set in a named `direction`, optionally restricted via
  `neighbour_field` / `neighbour_value` to edges whose counterpart holds a
  named enum value). `thresholds` map counts to levels of the new two-member
  `SignalLevel` enum (`notice` / `warn`, deliberately not
  `ConstraintSeverity`); below the first threshold the served level is
  `none`. Values are computed at read time, never stored, never part of
  `_hash`, and nothing multiplies, averages, or decays. Served on every
  entity read of a declaring type (`_signals` on the structured envelope;
  headline in the rendered frontmatter plus a `## Signals` contributors
  section), on the new include-gated `signals` health axis (above-`none`
  entities with per-level counts; `warn` participates in `--strict`,
  `notice` never does), and as the new out-of-band warning
  `SIGNAL_THRESHOLD_CROSSED` on mutations that move a signal across a
  threshold in either direction (entity, signal, value, old and new level;
  never error-shaped). The loader refuses bad names, duplicate names,
  undeclared rel-types, empty or non-increasing thresholds, and
  half-declared or undeclared neighbour pairs. Declarations render in the
  `memstead_schema` response at both verbosity levels; schemas without
  signals keep byte-identical responses everywhere.

- **Relation sets for acyclicity and propagation: `relationships.acyclic_sets`
  and `status_propagation.rel_types`.** A schema manifest can now declare
  acyclicity over a SET of rel-types: a write that closes a cycle in the
  union subgraph refuses with the existing `RELATIONSHIP_CYCLE` error, whose
  payload additively gains `acyclic_set` (the declared set) and
  `existing_path_rel_types` (one rel-type per hop, so the path may mix
  rel-types); single-rel-type refusals and the per-definition `acyclic` flag
  stay byte-identical. The boot-time sweep drops on-disk cycle-closing edges
  in a set's union subgraph exactly as it does per rel-type. The
  `status_propagation` constraint accepts `rel_types` (an inline relation
  set) where it accepts `rel_type` today; the single-name key keeps parsing,
  exactly one of the two per declaration, and taint walks the union so it
  crosses rel-type boundaries. The loader refuses undeclared names, a
  rel-type in two sets, sets with fewer than two members, `rel_type` and
  `rel_types` together, and an empty `rel_types`. Both shapes are visible in
  the `memstead_schema` response at both verbosity levels; schemas without
  the declarations keep byte-identical responses.

- **Reachability obligations: `must_reach` on type definitions.** A type can
  now declare that its entities must reach at least one non-stub entity of a
  named set of terminal types (`terminal_types`), following edges of an
  inline relation set (`relationships`) in a named `direction` (`out` / `in`,
  the vocabulary search already speaks), within an optional `max_depth`.
  Evaluated on the health sweep only (the `constraints` axis), never on the
  write path: a transitive gap is created by writes on other entities, so
  the loader refuses `severity: block` (same posture as
  `status_propagation`). Findings echo the whole declaration; a satisfied
  obligation is silent; cycles terminate via visited-set discipline;
  cross-mem edges are followed like any edge; the incoming direction with
  `max_depth: 1` covers the required-incoming-edge case. The loader refuses
  undeclared rel-types, unknown terminal types, unknown directions, empty
  sets, and a zero depth. The `memstead_schema` response shows the
  obligation at both verbosity levels; schemas without the form keep
  byte-identical responses and health output.

- **Conditional edge requirements: `when_field` / `when_value` on
  `required_outgoing` blocks.** A type definition can now declare that an
  outgoing-edge obligation applies only while a named metadata field of the
  entity holds a named enum value (the same two keys `requires_when` already
  uses; absent pair = unconditional block = unchanged behaviour). Semantics
  are identical to unconditional blocks in every other respect: same
  cardinality vocabulary, same warn/block severity model, evaluated on
  create, on update (a metadata flip that arms a block on an entity lacking
  the edge is caught), and on the relate remove path, and surfaced on the
  `missing_required_outgoing` health axis. Wherever the engine names an
  unsatisfied block (the `MISSING_REQUIRED_OUTGOING` refusal payload, the
  write-time warning, the health finding, and the `memstead_schema`
  response at both verbosity levels), a conditional block carries its
  `when_field` and `when_value`, so the reader sees which trigger armed it.
  The loader refuses a condition whose field is undeclared or lacks
  `enum_values`, whose value is outside the enum, or that carries one key
  without the other, with a typed error naming the offender.

- **A coverage-guided fuzzing tier and a committed shared seed corpus for
  the three trust-boundary parsers.** A workspace-excluded `fuzz/` crate
  (cargo-fuzz/libFuzzer) carries three targets: raw bytes through the
  archive validator (nested parsers covered transitively, canonical
  fixpoint asserted), the frontmatter/markdown family through its public
  entry points (mask and idempotence invariants asserted), and the
  content-expression parser and matcher (parse-source-parse stability,
  deterministic matching, coherent failure payloads). A manual-dispatch
  workflow runs the targets on nightly with a stated per-target budget
  and uploads crash artifacts; it is never a required check, and no
  nightly toolchain or fuzz dependency enters the PR-blocking path. The
  seed corpus is materialized to committed files under `fuzz/corpus/`
  (the seeded smoke harnesses' documents, expressions, and archive
  shapes, plus five real tracked `.mem` artifacts harvested as-is) and
  is shared with the smoke tier: the normal suite replays every corpus
  member and asserts the materialized seeds stay valid.
- **A seeded adversarial smoke over the content-expression parser and
  matcher.** Foreign expression strings reach this parser through the
  schema tree published archives embed. The harness generates
  adversarial expressions (fragment assemblies, splices and truncations
  of valid seeds) and asserts: parsing never panics (typed refusals
  only); an accepted expression's verbatim source re-parses to a
  structurally identical expression; the compiled NFA stays linear in
  the expression's terminal count (no adversarial state blowup); and
  matching arbitrary block sequences never panics, is deterministic,
  and reports coherent failure payloads. 9000 cases, under a second; no
  defect found.
- **A seeded adversarial smoke over the archive trust boundary.** Foreign
  bytes through the validating entry point, covering the nested parsers
  (config, strict entity checks, schema loader, id and graph validation,
  the canonical re-pack) transitively: zip-level bit flips, truncations,
  splices, inner-content mutations, and hostile extra entries (traversal
  paths, meta-dir payloads, duplicates) over a seed corpus of valid
  archives. Asserts no input panics the validator, every accepted
  archive's canonical bytes re-validate to the same canonical bytes, and
  the deliberate forward-compat tolerance (unrecognised `.memstead/`
  members) never influences canonical output. Deterministic and bounded
  (4500 cases, well under a second).
- **A seeded adversarial smoke over the frontmatter/markdown parser
  family.** A deterministic, bounded harness (hand-rolled xorshift64, no
  fuzz dependency, about 1s) assembles adversarial inputs from a fragment
  alphabet and mutated realistic documents, asserting on every case: no
  entry point panics; the three frontmatter implementations (tolerant,
  peek, strict) agree on where frontmatter ends and the body begins;
  masking preserves byte length and newline positions; and parse-generate
  is idempotent. Failures reproduce from the seed and case index in the
  panic message.
- **Derived structures are maintained, not discarded.** The search
  index and the community-partition memo key on a rollback-aware
  `DerivedKey` (store generation + schemas epoch): repeated reads
  serve the memo, a refused batch stays recompute-free, a rolled-back
  interim state can never be served as fresh, and a schema switch
  correctly invalidates both (closing a previously masked staleness:
  the index field set and community weights derive from the pinned
  schema). Single mutations maintain the search index in place —
  exactly the touched documents are replaced or removed — with named,
  scoped fallbacks (schema-shape change, index error) that rebuild
  rather than serve stale results; batch and reload paths keep the
  amortized whole-map rebuild. A seeded property test pins identity
  with a from-scratch rebuild across arbitrary mutation sequences,
  refused batches, schema switches, and reloads. Embedders gain
  `Engine::drop_search_indexes` as an explicit memory-release /
  forced-rebuild hook.
- **Cross-mem targets verify against storage — no mount, no load.** A
  write referencing an entity in a mounted-but-unloaded (lazy) mem, or
  in a mem with no mount record at all whose content branch lives in
  the mem-repo, is verified against real storage at write time: a
  tree-lookup-class existence probe (`MemBackend::entity_exists` —
  metadata-class on folder backends, stop-at-entry tree lookup on
  git-branch, never the blob-reading listing walk) plus one resolved
  blob read for the target's entity type when the cross-schema shape
  check needs it. A verified target admits as an ordinary reference
  (its in-store stub carries the load-time kind and no
  `AUTO_STUB_CREATED` / mem-uncreated warning); an absent target keeps
  the exact semantics it had — the typed read-only refusal (now
  answerable without the mem loaded, and never firing for an entity
  storage actually contains), or the forward-reference auto-stub with
  its warnings. Verification never loads a lazy mem and never adds a
  mount — a dossier citing twenty unmounted topic mems pays twenty
  tree lookups, not twenty permanent eager loads. Unmounted-mem
  discovery (branch resolution plus the stored schema pin, so
  cross-schema edge routing keeps its authority) is installed by the
  full workspace boot; lean and embedded engines keep the
  forward-reference mechanic unchanged. No new trust class exists:
  every admitted cross-mem edge is either storage-verified at write
  time or a forward-reference stub with today's semantics, and stub
  kinds remain annotation, not state.
- **Lazy mounts are real: `"lifecycle": "lazy"` defers a mem's entity
  load to first read.** The slot existed since V1, persisted but inert;
  a mount that declares it now resolves only its metadata half at boot
  (config, provenance, schema pin — a broken pin quarantines exactly as
  an eager mount's would) and loads its entities when the first
  operation touches the mem, through the same per-operation funnel every
  MCP call already passes. A scoped operation loads exactly its mem; a
  workspace-scoped or cross-mem one loads every deferred mem first —
  search's graph-walking forms (`related_to`, `expand_via`), overview
  (whose community partition is workspace-global even under a mem
  filter), health (always, mem filter included), and `memstead_entity`'s
  `include_relations` / `include_context` forms (incoming edges and
  community context can originate anywhere) all take the full load; the
  destructive-delete guards, the write-time acyclicity guard (single
  and batch mutation paths alike), the mem-rename reference sweep, and
  parse recovery load fully before adjudicating — so
  no answer is computed over a partial store — and a
  lazy mem is never silently absent: the roster carries it with its pin,
  and load state is observable. The lazy load runs the same validation
  gauntlet an eager boot runs, and a failed deferred load quarantines at
  first read with the same typed reporting. Opt-in per mount; a
  workspace with no lazy mounts boots and serves byte-identically to
  before. On the CLI, `memstead entity` pays only the target mem's load
  (its `--include-relations` form, whose incoming edges can originate
  anywhere, still loads fully); other commands load fully for now.
- **`memstead publish --redact-anchors` — trust metadata without the
  source's identity.** Every artifact reference in the packaged anchors
  sidecar — the `artifact` field and each `derived_from` entry — becomes the
  fixed sentinel `[redacted]`, while the provenance class, `at_version`,
  grain, hash, hash stability, and source name survive: a consumer still
  reads how strongly each entity claims fidelity to a source without
  learning which source. Redact, not strip — and publish-time only: the
  workspace's own sidecar is never touched, and without the flag published
  anchors ride byte-identical to before. The pre-built archive shape
  refuses the flag (its anchors are already baked in), same precedent as
  `--version`. Redaction removes identity, not existence: the kept fields
  still reveal the medium shape, possibly a commit SHA, the author's source
  name, and a hash that permits confirming guessed content — the publish
  guide states this plainly.

  Two hardenings landed with it: the engine-agnostic folder assembler now
  embeds the mem's anchors sidecar (a bare `memstead publish` of a folder
  mem used to ship silently without the anchors its engine-exported sibling
  carries — the publish-strip failure the anchors contract exists to
  close), and archive validation refuses an anchors member carrying an
  empty artifact reference, so a botched redaction is caught rather than
  shipped.
- **`memstead projection check-path` — one deny dialect, engine-answered.**
  Is a path (or a Glob/Grep pattern) hidden by a binding's `deny_paths`? The
  engine now answers directly — single-path and `--batch` stdin forms, the
  verdict naming the matched deny entry — evaluated with the same `globset`
  machinery the enumeration path uses, plus the directory-prefix rule the
  plugin hook carried (`dev/**` also blocks a read of `dev` itself). With
  `--binding` omitted it answers for the *active* binding: the one whose
  brief was last consumed, published as a pointer by consuming renders.
  Generic by design — any consumer gets the same authoritative answer; the
  plugin's PreToolUse deny hook is the first, now a thin subprocess caller.

  Retired with it: the hook's 167-line JavaScript re-implementation of the
  engine's glob semantics (parity was guaranteed only for `*`, `**`, `?`
  and literals — character classes and brace alternates now follow engine
  semantics everywhere), the shared dialect fixture pinning the two
  implementations together, its Rust consumer (which reached from the engine
  crate into the plugin tree by relative path — that cross-boundary
  dependency is gone), and the engine-written deny-list cache. Enforcement
  now reads the active binding's record fresh on every check, so a stale
  deny list can no longer be enforced by construction — the pointer file
  (`.memstead.cache/projection/active-binding.json`) carries only the
  binding id, never a list.
- **`memstead export --format llms-txt` — any mem as one document an agent
  reads in a single pass.** Every non-stub entity once, in stable id order,
  with its type visible and its `[[wiki-links]]` resolved to working Markdown
  links; empty sections are kept, because an explicitly empty slot tells an
  agent the schema asked and nobody answered. `--base-url` renders absolute
  links exactly as a Memstead deployment serves them; without it they are
  document-relative, and there is no third form. Links inside code are left
  alone — resolution scans the engine's masked view and slices from the
  original, so a fenced sample documenting wiki-link syntax survives verbatim.
  A reference that cannot be resolved — a slug two foreign mems both own, or a
  target that does not exist — is named in plain text: never guessed, and never
  left as wiki-link syntax in a document promised as self-contained. This
  is the shape memstead.ai serves at `/llms-full.txt`, and it is now literally
  the same code: the renderer moved into the engine and the served endpoint
  consumes it, so the exported and served documents cannot drift.

  One deliberate change to what a Memstead deployment serves: an
  unresolvable reference on the served pages used to reach the reader as
  literal `[[slug]]` and is now plain text, because the shared renderer fixed
  that leak for both surfaces at once. Everything else the served document
  emits is unchanged.

  The resolver reads the full wiki-link grammar, the canonical colon
  cross-mem form (`[[mem:slug]]`) included — target normalisation is the
  same routine the parser and validators use, not a hand-rolled subset, so a
  fully-qualified reference a live mem legitimately carries resolves instead
  of degrading to plain text. A qualified miss stays a miss: the author
  named a mem, and rebinding the slug elsewhere would be a guess.
- **Graph-medium bindings now measure fidelity instead of performing it.** A
  binding whose source is another mem enumerates that mem's in-scope entities
  as a real `S(D)` denominator, so coverage is computed rather than vacuously
  `0/0` and an unprojected source entity surfaces as a gap. `entity`-grain
  anchors resolve against the live graph: an anchor over a changed source
  entity reports `drifted`, over an unchanged one `resolves`, over a deleted
  one `orphaned`. Previously every graph anchor read "unobserved this pass",
  which meant drift was structurally undetectable while the capability matrix
  claimed full parity — a stale-pinned anchor over a changed entity went
  unflagged. `git`-medium sources enumerate too; they were excluded from the
  same walk for the same reason.
- **Graph scope is an entity selector.** A graph source's `scope` patterns are
  `*`, `type:<entity_type>`, or `id:<glob>` over the full entity id, enforced
  at run time and refused at binding validation if they are anything else.
  `projection init` scaffolds `*` for a graph source instead of the path glob
  `**/*`, which nothing interpreted — a facet could carry scope that looked
  like selection and reached nothing. An unscoped graph facet now refuses like
  every other medium's, and a graph source's brief names its entities and how
  to read the source baseline rather than printing path globs at an agent with
  no glob tool over a mem.
- **`verify --full` refuses an empty enumeration.** A medium the matrix marks
  enumerable whose walk yields no artifacts refuses rather than reporting
  complete coverage of nothing. This is the standing guard: a medium cannot be
  added to the matrix as enumerable without an enumeration arm and still
  return green.
- **`memstead projection verify --fail-on-findings` — a verify a CI job can
  gate on.** Three outcomes a workflow can branch on without parsing output:
  exit `0` the run completed and found nothing, exit `6` the run completed
  and recorded findings, any other nonzero the measurement itself failed.
  Code 6 is dedicated: a run that cannot complete returns its own code
  instead, which is the whole point — a job can tell "the mem and its source
  disagree" from "the engine could not boot". The line is *did the
  measurement complete*, not *was everything well*: an artifact the pass
  could not read is a finding (it was observed), while an input it could not
  read at all refuses — an unreadable anchors sidecar now returns
  `ANCHORS_SIDECAR_UNREADABLE` rather than reporting every artifact
  uncovered. The full report is rendered before the findings exit
  fires, so a red build still carries the report that explains it. Opt-in:
  without the flag verify's exit behaviour on a *measurable* binding is
  byte-for-byte unchanged — clean and drifted both still exit 0. One ungated
  exit did change, deliberately: an unreadable anchors sidecar now refuses
  instead of completing (see Fixed).
- **A rollup verdict on the fidelity report — `clean` / `drifted` /
  `inconclusive`.** The report now opens with the answer instead of with
  denominator provenance, plus the ranked concrete actions. The third value
  is the one that matters: a measurement can complete without being able to
  support a green claim, so a medium with no change signal, an empty
  enumerated scope (the graph-medium `0/0` case), or a pass that adjudicated
  no anchor all resolve to `inconclusive` with the blindness named, never to
  `clean`. Derived from the report's own figures, so the headline cannot
  disagree with the body. A mem that predates its binding is never rendered
  red for pre-binding history alone.
- **A verify-in-CI guide, exercised rather than printed.** The docs site
  gains `guides/verify-in-ci` — the three-outcome table, a copyable
  GitHub Actions job, how to read a red build, the JSON contract, and
  every cap that bounds the gate's honesty. A committed fixture
  workspace (`ci/fixtures/verify-gate/`) and `ci/verify_gate.py` run the
  guide's own command on every `run-tests.sh`, in both polarities plus an
  operational failure, and assert the guide still prints the command the
  harness runs — so the published example and the exercised one cannot
  drift apart silently.
- **`projection verify --json` is versioned.** The payload carries
  `"format": "memstead-verify/v1"` in the house style
  (`memstead-export/v1`, `workspace-dump/v0`) and ships the `rollup` block
  alongside the report — consumers assert the marker, so a future shape
  change fails loudly instead of misparsing.

- **`memstead quickstart --repo <PATH>` — the guided point-at-your-repo
  first session.** One invocation from an existing repository leaves a
  workspace, a mem, a scaffolded `codebase` binding over that tree, agent
  wiring, and a printed brief stating what the starter mem holds (one seed
  entity), what it does not (anything from the repository), and the exact
  command that starts the ingest loop. Nothing is ingested and no
  repository file becomes an entity: the mem takes a folder of its own
  inside the repo, and the engine already excludes every mount's storage
  location from binding enumeration. Without a target path the repository
  *is* the workspace root (clean repo-relative artifact ids, `.memstead/`
  and the agent config where an agent working in the repo finds them);
  with one, the workspace lands there and the out-of-root layout caveat is
  printed with its relocation recipe. The plain path is untouched — its
  arguments, gates, refusals, receipt, and JSON are unchanged, and the
  tolerant-emptiness gate still refuses a populated folder.
- **`memstead_base::binding::scaffold_binding`** — the engine-side
  definition of "a fresh binding" (scoped source, materialised deny
  defaults, capability-matrix-filtered operations, prune where sync
  survives), now shared by `projection init` and the guided quickstart
  path instead of living in one command. `init_filesystem_mem_at` is its
  workspace counterpart: a folder mem in a subdirectory of the workspace
  root, the uncollapsed form of `init_filesystem_mem`.

### Removed
- **The six `memstead_workspace_*` MCP tools.** Workspace policy —
  which mems may be created or deleted, which cross-mem links are
  granted — is the operator deciding what an agent is allowed to do.
  Exposing those switches on the agent's own tool surface handed the
  constrained party the keys to its constraints. The capability is
  unchanged and reachable via `memstead workspace <action>`; a
  policy-gated mutation still refuses with the typed code and now
  names the exact CLI command to report, rather than an MCP tool the
  agent cannot call. The pro server drops from 25 tools to 19; the
  lean server and the memstead.ai session endpoint never carried them.
- **`memstead_base::entity::parser::extract_wiki_links` and its
  `WikiLink` struct.** A public wiki-link extractor that scanned raw
  content with no code-block masking, with zero callers anywhere in the
  workspace. Dead pre-migration semantics standing beside the unified
  extractors is how a future caller reaches for the wrong one; the
  masked strict and lenient extractors are the surface.

### Changed
- **One CommonMark referee for code.** Section splitting, title
  extraction, heading spans, wiki-link extraction, wiki-link rewriting,
  the strict validator and the write-time section guard now share one
  definition of what a code block and an inline code span are, taken
  from `pulldown-cmark` (`memstead_base::markdown`) instead of two
  hand-rolled line scanners that disagreed. Content inside **any**
  CommonMark code block — a backtick or tilde fence at any legal
  indent, a fence inside a list item or blockquote, a fence whose
  would-be closer carries an info string, a fence longer than the one
  that tries to close it, or a plain indented code block — opens no
  section, registers no heading, never becomes an entity title, and
  yields no wikilink on any path. Inline code spans come from the
  parser too, so a multi-backtick span (```` `` ```` / ```` ``` ````) is
  one span rather than a run of mis-paired delimiters that used to
  swallow whole paragraphs of prose — or leave real code spans exposed.
  Heading recognition is unchanged: a section is still an ATX `## ` at
  column 0, setext headings and indented ATX create sections nowhere
  (the contract is now stated in `GLOSSARY.md`). Migration evidence: a
  structural re-parse diff over 766 documents — all 9 mems of the
  dogfood workspace (681 entities), the agentic test workspaces, the
  crate fixtures — changed no section, heading, title or heading span
  anywhere; the only differences are 60 wikilink visibility changes in
  8 documents, every one of them a mis-paired-backtick misparse now
  fixed.
- **The write-time section guard checks what the reparse will see.** It
  masks code blocks first, so a `## ` inside a fenced or indented code
  block is no longer refused as an embedded heading; and it trims
  first, so content whose stored (trimmed) form exposes a column-0
  `## `/`# ` — the indented-code-block-opens-a-section round-trip fork
  — is now refused at ingress instead of forking the entity on its
  next read.
- **A relationship row inside a code block is no longer an edge.** The
  `## Relationships` row parser scanned the section body unmasked, so a
  fenced or indented `- **REFERENCES**: [[target]]` — the obvious thing
  to write in an entity documenting the row syntax — became a real graph
  edge and auto-created a stub, while the strict validator, the link
  extractor, the rewriter and the dangling-link reporter all correctly
  treated that link as invisible. One path synthesising an edge from
  what every other path refuses to see is the asymmetry the one-
  definition rule exists to prevent. Real rows are untouched: the scan
  reads the masked body and takes every captured span from the original,
  so type normalisation, em-dash descriptions and ambiguous-delimiter
  warnings behave exactly as before. Inline code spans count as code
  here too: a row inside a multi-line span was invisible to the
  validator and every extractor while still building an edge.
- **Two more readers joined the one definition.**
  `filesystem::tier3::extract_tier3_refs` (the registry's Tier 3
  `[[scope/name:slug]]` scanner) and health's `enum_from_neighbour`
  bullet harvest both read raw text; a reference or a legal value
  written inside a code sample counted as real. Both now mask.
- **HTML export stops rewriting wiki-links inside code.** The
  exporter's `[[…]]` resolution was a hand-rolled string walk with no
  masking, so a fenced or indented code sample documenting wiki-link
  syntax came out as a rendered link — or marked `*(unresolved)*` —
  inside `<pre><code>`. It now scans the same masked view every other
  reader uses and slices from the original, leaving code byte-identical
  while prose links resolve exactly as before. The renderer's parser
  options are derived from the engine's one dialect rather than
  rebuilt, so a flag added to the reader reaches the renderer too.
- **Merge-conflict detection reads frontmatter too, and is no longer
  blinded by it.** The guard that refuses an entity file carrying git
  conflict markers now scans one rejoined view — raw frontmatter plus
  masked body — instead of masking the whole file. Two consequences:
  markers written into the frontmatter are caught for the first time
  (git writes them wherever the hunks fall), and a conflict triple that
  straddles the `---` terminator is caught as well. Previously a
  frontmatter value that read as a code-fence opener could hide the
  markers, and the file would load with both merge sides fused into one
  body — the outcome the guard exists to prevent. The complement is
  unchanged: a fenced code example documenting conflict markers in a
  section body still does not trip it.
- **A mem rename no longer leaves dangling cross-mem links behind.**
  The mem sweep's wiki-link rewriter takes whole entity files, and
  masked them as markdown; a frontmatter value that read as a fence
  opener blanked the body, so the scan found no links and the rewrite
  reported zero changes — indistinguishable from having nothing to
  rewrite. It now masks the body only.
- **`[[]]` is visible to every path.** The wiki-link pattern the
  extractor uses accepts an empty target, so an empty link routes to
  the same typed refusal the strict validator already emitted instead
  of being invisible to one side of the seam. The read-side lenient
  scanner sees it and decodes nothing, as it does for any target it
  cannot resolve.
- **The build brief tells the truth about its destination, its source and
  its anchor names.** A binding scaffolded before its mem exists (an
  order `projection init` deliberately allows) rendered a brief whose
  whole mandate is "mutate the destination", with nothing saying the mem
  was not there — the agent found out on its first write. The Destination
  block now names the absence and a remedy verified to work in the
  reader's own workspace shape: `mem init` alone in a mem-repo workspace
  that already admits the name, the `workspace allow-create` pair where
  it does not — both steps naming one concrete schema pin the reader can
  copy, resolved from the registry, rather than a `<name@version>`
  placeholder they would have to fetch vocabulary for — and, in a
  filesystem-mem workspace, which holds one mem,
  re-declaring the binding, because the record's folder decides which
  mem's anchors resolve and editing `destination_mem` in place leaves
  every anchored write refusing. A source pointer resolving to nothing on disk is
  named as such, and the Sources block prints the pointer it is talking
  about. And the provenance block promised that an undeclared anchor
  `source` name always refuses; that check fires only for anchors
  carrying a producing binding, and an unknown name over a path that
  resolves workspace-relative is accepted on purpose — so a legacy anchor
  whose binding was renamed keeps writing. The brief now states both
  halves of that contract.
- **`memstead projection init` warns when the declared source does not
  exist.** Declaring a not-yet-present tree stays legal; the silence was
  how a mistyped answer produced a binding that could never yield
  anything.

### Fixed
- **Half the guide's own example was neither run nor pinned.** The printed
  job has two steps; the harness exercised the first and pinned four lines
  from it. A grade ran nine mutations against the second — the step the
  guide calls "what makes the gate trustworthy" — and all nine passed,
  including deleting the step outright, inverting its comparison, and
  dropping `set -o pipefail` so exit 6 stops propagating through `tee`.
  The harness now **executes** that step's script, lifted out of the
  guide's own YAML, against a clean run (must pass) and an inconclusive
  one (must fail) — the case it exists to catch. Pinning proves a step is
  printed; running it proves it works.
- **The verify-in-CI guide's own job passed runs the same guide said were
  not gateable.** Exit `0` means "recorded no findings", which an
  `inconclusive` pass also does — a facet with no readable change signal, an
  empty scope, a graph-source binding. The printed workflow branched on the
  exit code alone, so it went green on exactly those. It now reads
  `rollup.verdict` in a second step and fails on anything but `clean`, and
  the outcomes table says plainly that exit 0 is necessary, not sufficient.
  The underlying gap — the exit-code contract has no third value — is
  recorded in the backlog as an operator call on the external contract.
- **A corrupt anchors sidecar produced a red CI build blaming the mem.** The
  anchor readers degrade a malformed sidecar to "no anchors" — right for a
  reader, wrong for anything concluding from the absence of anchors. A
  fidelity pass read every artifact as uncovered, recorded that as findings
  and exited 6, with nothing on stderr: "no anchors parsed" and "no anchors
  exist" are different facts and only one is the mem's fault. `verify` now
  asks `Engine::anchors_sidecar_error` first and refuses with
  `ANCHORS_SIDECAR_UNREADABLE`. The diagnostic is general — any caller that
  must not confuse unreadable with absent can ask.
- **"Verify is read-only" was false on eleven surfaces, including the ones
  agents read.** Verify mutates no entity, but a completed run records its
  findings store, backfills observed content hashes onto hash-less anchors,
  and writes a `#verified` baseline. The claim is corrected everywhere it was
  made — the CLI's own clap comments (which regenerate into the published
  reference twice), the fidelity-contract page's opening, the glossary, the
  binding-format generator and its JSON schema, the plugin README and `sync`
  skill, and the verify brief an agent reads at run time. Two grading rounds
  were needed to find them all: the first swept the obvious ones, and the
  page that still contradicted itself top-to-bottom survived it.
- **The rollup could render `clean` on a change-blind binding.** It read the
  medium's capability row, which says a `codebase` source *can* signal
  change — while a binding declaring `change_detection: "none"` resolves that
  medium to no strategy at all. Such a run printed a green verdict above a
  body that said "freshness unknowable" two screens down. The derivation now
  reads the resolved signal, so a binding that asked its medium not to report
  change is `inconclusive` and says which facet and why.
- **The fidelity contract named its remaining caps.** The page now also
  states that an mtime `#synced` baseline does not survive a fresh
  checkout, which bounds what a CI gate can honestly claim. (It named
  graph-medium verify as a cap too; that one was fixed rather than
  documented — see the graph-fidelity entry above.) It
  also says outright that verify **writes**: a findings store, an anchor
  hash backfill, and a `#verified` baseline.
- The fidelity-contract page has claimed since it was written that the
  report "opens with a rollup verdict and the top concrete actions". No code
  produced one. It does now — the docs stopped being wrong by the code
  catching up, not by the claim being deleted.
- **The backfill path named a subcommand that does not exist.** The
  fidelity report and three health findings told the reader to run
  `memstead projection sync <binding>`; the CLI has no `projection sync`
  verb — the sync brief is `memstead projection brief <binding> --sync`.
  Four printed sites corrected, the pinning test with them.
- **Superseded `default@1.0.0` citations** in the repo README, the CLI
  crate README, `docs/build.md`, `docs/workspace.toml.example`,
  `docs/sizing-curve.md`, the MCP and schema crate READMEs,
  `examples/README.md`, the plugin setup skill's README and the
  reconstruction proof's recipe — every one taught a schema pin the
  shipped binary no longer writes. (The same misstatement inside the
  sealed built-in schema packages is deliberately left alone: their
  content is hash-pinned by the retention gate.)
- **A refusal offered a remedy that refuses.** `projection brief <binding>
  --sync` and `projection advance` on a binding with no sync block named
  `projection enable sync <binding>` unconditionally — but over a medium
  whose capability row cannot carry sync (a `web` source), that command
  refuses too, bouncing the reader from refusal to remedy to capability gap
  with nothing that closes it. The absent-operation refusal now validates a
  candidate carrying the operation — the same question `enable` asks — and
  names the gap directly where the medium cannot carry it, keeping the
  one-command remedy everywhere it is honest. `concepts/fidelity-contract`
  said web-medium sync is refused "at declaration time, not at run time";
  `projection init` in fact scaffolds build-only with a warning and the
  refusal arrives when sync is asked for, which is what it now says.
- **`/ingest --clear` handed a first-session reader an engine internal.**
  In a filesystem-mem workspace — the shape `memstead quickstart`
  produces — the router echoed the engine's `VALIDATION_FAILED: memstead
  mem delete requires a mem-repo workspace … is filesystem-shaped`, which
  names a workspace shape the reader never chose. It now says what is
  true: such a workspace holds one mem, so there is no paired process mem
  to clear, and starting the build over means removing `.memstead/` and
  the mem's folder and re-running `quickstart`.
- **The crate READMEs and the installer offered only the empty-directory
  entry.** `memstead-cli`'s `## Start`, the `memstead-mcp` wiring
  section, and `install.sh`'s next-step line now name
  `memstead quickstart --repo .` beside it.

## [0.9.0] - 2026-08-19 (cut 2026-08-19, never published; its content ships in 0.10.0)

### Added
- **A pilot-grade GitHub-issues mirror** (`scripts/mirror-issues.mjs`):
  mirrors any repository's issues (body, comments, labels, state, dates)
  one-file-per-issue into a dedicated git repo a normal `filesystem`
  binding consumes — deterministic re-runs, updatedAt-incremental,
  `--full` refetch-and-prune. Deliberately NOT a stable `memstead` CLI
  surface: the tool marks itself pilot-grade, names the open
  forge-medium design questions, and states the freshness gap (the
  mirror is as fresh as its last run; nothing measures it against
  GitHub) at every point of use.

### Changed
- **Anchor artifact paths speak the source dialect.** An anchor's
  artifact path now resolves source-relative first — joined onto the
  pointer its `source` name declares in the mem's bindings, out-of-root
  pointers included — with the workspace-relative form as the fallback
  when the join does not resolve; a path resolving under both joins goes
  to the source-join, deterministically. Anchors without a `source` name
  observe workspace-relative exactly as before. This makes the dialect
  every other binding surface speaks (scope globs, the brief's path
  list, disposition artifact ids) the correct one for anchors too, with
  zero migration of existing sidecars.
- **Write time resolves or refuses.** A path-grain anchor whose artifact
  resolves under no candidate join refuses `INVALID_ANCHOR`
  (`ArtifactUnresolvable`) with every candidate tried in the payload —
  a mutation never stores a silently dead (orphaned-at-birth) anchor.
  The gate skips only when it cannot know: no workspace root, or an
  unreadable binding store.
- **Rendering a rotation brief is a pure read.** `projection brief
  --all` no longer advances the round-robin cursor or per-pair backoff;
  taking the rotation slot is the new explicit `--consume` flag
  (requires `--all`), which the sync skill's loop driver and the ingest
  router pass. The JSON envelope gains `not_rotated`, naming every
  (binding, operation) pair the filter admits but the binding does not
  loop-declare, with the enable remedy; the markdown form prints the
  same as stderr notes.
- **A foreign or garbage-collected sync baseline reseeds instead of
  degrading.** A git-shaped `#synced` baseline the source's repo does
  not contain now reseeds at HEAD (one honest full re-roam, then normal
  change detection) instead of reporting "git signal unavailable"
  forever; the reseed message states the foreign-baseline case
  honestly. `projection brief <id> --sync` on a sync-disabled binding
  refuses `PROJECTION_SYNC_NOT_ENABLED` with the enable remedy in
  `details`.
- **The brief names sources by their declared name** (`**main-app**
  (codebase, primary)`) in the Operative-data section, matching the
  provenance section's instruction; the zero-artifact `projection
  advance` says "No artifacts were presented this pass" instead of
  claiming disposal of nothing.

### Fixed
- **A sealed schema package carries the generation its content was
  written in.** `schema install` used to stamp every unmarked package
  with the current-language format marker — a legacy builtin sealed
  onto `__MEMSTEAD` (or exported into an archive) read back with every
  bare metadata field flipped from required to optional, the exact
  silent flip the 0.6.0 marker contract promises cannot happen. The
  seal paths now carry the source's generation as-found: a legacy
  package seals unmarked (absence IS its legacy claim), a
  current-generation builtin's marker travels with it, and only the
  authoring resolver — which just validated the package under the
  current language — mints a new marker. Workspaces that installed a
  legacy package on an affected engine may hold a mis-stamped seal:
  the affected window, the detection one-liner, and the reinstall
  repair are documented in the docs-site schema-authoring guide
  ("Repair: mis-stamped legacy seals"); `schema install --help`
  points there.
- **Hydrating an archive with an unknown `format` refuses typed.** The
  byte-hydration path (`Engine::from_archive_bytes`, the wasm
  package's `fromSnapshot` included) never consulted the format
  predicate — an archive rewritten to `format: 99` hydrated and served
  every entity. It now refuses with the accepted formats named; every
  reader gate consults the one predicate.
- **`memstead_search` with a mem filter naming no visible mem refuses
  `UNKNOWN_MEM`** instead of succeeding with 0 hits and a
  missing-index warning — an empty result now always means a valid mem
  with no matches, matching every other mem-naming surface.
- **Per-entry batch notes survive on the git-branch backend.** All
  three batch operations (create, update, relate) routed per-entry
  notes through a documented no-op; they now ride the one batch
  commit's note record as `<id>: <note>` lines, retrievable via the
  notes surface. A batch with no notes carries no note record.

### Removed
- **The `memstead-swift` UniFFI crate left the workspace.** Its sole
  consumer — the native macOS app — was retired on 2026-08-18, so the
  in-process embedding surface goes with it: the crate, the xtask UDL
  reference generator, the generated UniFFI docs page, and the parity
  matrix's UniFFI column are all removed. Every remaining engine
  consumer is subprocess (MCP/CLI), HTTP, or wasm; `git log` preserves
  the in-process embedding implementation for any future embedder.

## [0.8.1] - 2026-08-18

### Added
- **The entity envelope answers "what depends on this?".** Every
  `relationships[]` entry now declares its `direction`; a default read
  carries the entity's own outgoing edges (`direction: "out"`, endpoint
  under `target`), and `include_relations: true` adds the incoming
  edges (`direction: "in"`, endpoint under `from`) to the structured
  channel — previously the array was silently one-directional, and an
  agent branching on the envelope saw half the neighbourhood with no
  signal that half was missing (cold-start 0-8-0, F15).
- **`origin` is rendered at the shared envelope layer**, so every
  surface that composes an entity read carries the trust class — the
  CLI's `--json` read included, which previously omitted it while the
  guides promised it on "every read surface"; a script branching on
  trust silently treated third-party content as first-party there
  (cold-start 0-8-0, F9/F13).
- **Folder mems can be re-attached.** `mem init --storage folder
  --location <dir> --reattach` (recovery `reattach` over MCP) adopts an
  existing folder mem — config, entities, and sync state untouched —
  instead of refusing with `CONFIG_ERROR`, closing the gap where
  `mem unregister`'s documented re-init promise held for git-branch
  mems only. The `CONFIG_ERROR` message now names that remedy.

### Changed
- **A batch rehearsal reports itself as one.** `batch-* --dry-run`
  renders "rehearsed — N item(s) valid, nothing written" with
  per-line "would create" markers, where it previously printed the
  same "applied — N item(s) in one commit" as a real run and left the
  reader to discover the empty graph themselves (cold-start 0-8-0, F5).
- **The `UNSUPPORTED_WORKSPACE_SHAPE` remedy names published API.**
  The lean-boot error pointed at a `mem-repo` Cargo feature that does
  not exist on the published crates and named a factory without saying
  how to wire it; it now names
  `memstead_git_branch::workspace_store::engine_from_workspace_root`
  and the `set_backend_factory` alternative (cold-start 0-8-0, F6).
- **The schema scaffold marks its required keys.** Every key a type
  file must carry (`name`, `description`, `when_to_use`, `sections`,
  `metadata_fields`, `title_weight`, `text_fields`,
  `hierarchy_relationship`) now carries the same `REQUIRED` prefix the
  manifest's `community:` block already used, so deleting one is an
  informed act rather than a validate-time surprise (cold-start
  0-8-0, F4).
- **`@memstead/wasm`'s compatibility note is version-agnostic** — the
  rule ("same number as the engine") no longer hardcodes an example
  version that goes stale one release later (cold-start 0-8-0, F8).

## [0.8.0] - 2026-08-15

### Added
- **`@memstead/wasm` joins the release line at 0.7.0.** The package ran
  on its own version track, which is how it came to sit at `0.1.2`
  against a `0.7.0` CLI — unable to read any archive that CLI writes —
  with nothing on either registry page saying so. It is version-matched
  to the engine now, its README states that plainly, and
  `release-verify.sh` compares it like every other channel instead of
  printing a bare number on a track of its own.
- **`@memstead/wasm` can enumerate a snapshot's entity ids** —
  `entityIds(mem?)` returns the sorted ids in a hydrated archive.
  Without it the package could not meet its own stated purpose: a
  browser handed only a `.mem` had no way to learn which ids exist, so
  every caller paired the archive with an id list generated by the CLI
  — defeating the point of shipping something self-contained.

### Changed
- **The HTML export renders declared section headings**, not the
  engine's storage keys — `Current State` where the schema author wrote
  it, rather than `current_state`. The export is the one artifact handed
  to someone with nothing installed, and the declared heading is the
  only place an author controls how their model reads to an outsider.
  Anchors are unaffected (they derive from entity ids).
- **Docs pages name the revision they were generated from.** Every
  footer read `Generated from dev on unbuilt` — placeholders that render
  like data, on a site whose pitch is that its pages are generated from
  the live source. The stamp is now injected by the deploy or derived
  from git, and a build that can determine neither fails rather than
  publishing an unattributable page.
- **No hand-written docs page states a skill count.** The index claimed
  eight against a generated six; the roster page derives its count, and
  a build step now fails if any hand-written page states one at all.
- **Hand-written guides pin the schema the shipped binary writes.**
  Sample commands carried `default@1.0.0`, so a copied `memstead init`
  pinned a mem to an older schema than the default.
- **`memstead_create`'s description shows the relation object
  literally** (`{to, type, description?}`, no source field — the new
  entity is the source), with the full shape and the sibling surfaces'
  different spellings on the `relations` parameter itself. Nothing is
  renamed on any surface.
- **The workspace shape is stated where the choice is made.** `memstead
  quickstart`, `memstead init`, and `memstead mem-repo init` each close
  their receipt with the shape they just created, one concrete thing
  that shape cannot do, and the exact command for the other shape. The
  fork was previously silent on both branches: a newcomer learned that
  `quickstart`'s workspace cannot consume the registry only when
  `memstead install` refused — after the workspace already existed and
  had been modelled.
- **`memstead-mcp`'s boot line names the shape it opened**, not the
  build it was compiled as: `boot: filesystem-mem workspace at …` or
  `boot: mem-repo workspace at …`. The full binary serves both shapes
  and previously logged `mem-repo` for either, which made a genuine
  `UNSUPPORTED_WORKSPACE_SHAPE` refusal read as spurious to anyone
  debugging from the log.
- **`memstead create --relation` works on filesystem-mem workspaces.**
  The guard was CLI-local — the shared `prepare_create` validates and
  materialises inline relations on any backend, and the MCP surface has
  been creating entities with their edges on this shape all along.
  `--help` no longer claims a mem-repo-only limit. (`--dry-run` remains
  unimplemented on that path and still refuses.)
- **`quickstart` names a check its own session can run.** The receipt
  still names the restart for what the restart does — registering the
  MCP tools — and additionally names two verifications that need no
  restart: `<wired-binary> --version` and `memstead overview`. An agent
  session that has just run onboarding cannot restart itself, so the
  last mile of the wiring was previously unverifiable from inside it.

## [0.7.0] - 2026-08-14

### Changed
- **The surfaces a newcomer reads stop withholding what they know.**
  From the 2026-08-13 cold-start run — an agent with no prior
  knowledge of Memstead, working from public surfaces alone. Every
  item here is a sentence that was missing, not a capability:
  - The installer's closing block names `memstead quickstart` as the
    next step, and prints the two plugin commands immediately above
    the `/setup` line that depends on them. It previously ended with
    "In Claude Code, run `/setup`" — a command that does not exist
    until a plugin the installer never mentioned is installed.
  - `batch-create`, `batch-relate`, `batch-update`, `install`,
    `uninstall`, `recover`, and `create --dry-run` say in `--help`
    that they are mem-repo-only, and name the fallback. All seven
    refuse at runtime on the filesystem-mem workspace `quickstart`
    produces — the workspace every getting-started surface leads to —
    and none of them said so before the refusal. (`create
    --relation` already carried the clause; these were the missed
    instances of an established pattern.)
  - The `schema new` scaffold shows the relationship keys whose
    defaults are not the permissive ones — `per_edge_description`
    above all, whose `forbidden` default refuses every
    `relate --description` on that type — plus `source_types`,
    `target_types`, `cardinality_per_source`, and `manual_authoring`,
    as commented lines. The scaffold teaches by example, so a key it
    omits is invisible; a field author hit 38 uniform refusals for
    one absent line.
  - The scaffold marks `community:` as required rather than "the
    defaults are fine" (it is required, and the comment read as
    permission to omit it), and points at the workspace's
    meta-schema as the exhaustive key reference.
  - `memstead type`'s footer no longer tells agents to call
    `memstead_schema` with a **type** name — that tool takes a
    **schema** name, so the hint sent an agent into a typed error on
    its first schema lookup.

### Fixed
- **A published mem now installs on the strength of the schema it
  carries.** `memstead install` could only install mems pinned to a
  schema the installing engine already had — in practice, only the
  built-ins. Every archive embeds its own schema and the archive
  validator read and accepted it, but nothing ever wrote that package
  into the storage the pin resolver consults, so the mount refused
  with `SCHEMA_NOT_FOUND` and advised installing a package that was
  sitting inside the archive. Three of the four mems published on
  memstead.io failed this way, and had since the first release. The
  install now stages the archive's embedded schema into the
  workspace's own schema storage before registering the mount, so a
  third party can publish under a vocabulary of their own authorship
  and anyone can install and read it — offline, with no manual schema
  step and no republishing.
  - Sealed content is read under the generation it was sealed in, by
    the same reader that admitted it: a package the archive validator
    accepts is a package the mount can be registered against. Keys
    retired after a package was sealed (`propagating_relationships`,
    the `optional:` polarity) keep their written meaning on the way in
    — the installing user is not the author and cannot fix a third
    party's bytes. Authoring is unchanged and still strict: `memstead
    schema validate` and `memstead schema install` refuse those keys
    by name so the author can act.
  - An archive whose embedded schema genuinely will not load now
    refuses with `EMBEDDED_SCHEMA_INVALID`, quoting the loader's own
    diagnosis, instead of `SCHEMA_NOT_FOUND` pointing at a package the
    archive contains. Nothing is mounted and nothing staged after such
    a refusal.
  - No leaf of the install flow reports `code: INTERNAL` any more.

### Removed
- The `<workspace>/.memstead.cache/schemas/` layer, which the schema
  registry and the publish-side collector both read and nothing ever
  wrote, together with the dead archive-extraction helper that was to
  have filled it and the `SCHEMA_CACHE_COLLISION` code that guarded
  it. It offered the appearance of a staging mechanism while the pin
  resolver looked elsewhere; there is now one staging path.

## [0.6.0] - 2026-08-10

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
- **A machine with no configured git identity can create a mem
  again.** Ref transactions write a reflog, and the reflog entry's
  committer was sourced from the ambient git config — so on any
  environment without a global `user.name` / `user.email` (a fresh
  laptop, a container, a CI runner) the very first step of
  `memstead quickstart` refused with `MEM_ERROR: … ref transaction
  rejected: The reflog could not be created or updated`. The engine
  already signs its *commit* objects as `engine <noreply@memstead.io>`
  rather than borrowing the user's identity; the reflog now does the
  same, so mem-repo bookkeeping no longer depends on ambient config in
  either place. Nothing about user-authored content changes — the
  author identity on content commits is untouched.
- **Ref-transaction failures name their cause.** The gix errors behind
  a rejected transaction are headlines over a `source()` chain, and the
  conversion rendered only the headline — "The reflog could not be
  created or updated" with the reason deleted. Every such conversion now
  renders the full chain, so the message above arrives as "… : reflog
  messages need a committer which isn't set" and is diagnosable from one
  line.
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

[Unreleased]: https://github.com/memstead/memstead/compare/v0.17.0...HEAD
[0.17.0]: https://github.com/memstead/memstead/compare/v0.16.0...v0.17.0
[0.16.0]: https://github.com/memstead/memstead/compare/v0.15.0...v0.16.0
[0.15.0]: https://github.com/memstead/memstead/compare/v0.14.0...v0.15.0
[0.14.0]: https://github.com/memstead/memstead/compare/v0.13.0...v0.14.0
[0.13.0]: https://github.com/memstead/memstead/compare/v0.12.0...v0.13.0
[0.12.0]: https://github.com/memstead/memstead/compare/v0.11.0...v0.12.0
[0.11.0]: https://github.com/memstead/memstead/compare/v0.10.0...v0.11.0
[0.10.0]: https://github.com/memstead/memstead/compare/v0.8.1...v0.10.0
[0.8.1]: https://github.com/memstead/memstead/compare/v0.8.0...v0.8.1
[0.8.0]: https://github.com/memstead/memstead/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/memstead/memstead/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/memstead/memstead/compare/v0.4.0...v0.6.0
[0.4.0]: https://github.com/memstead/memstead/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/memstead/memstead/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/memstead/memstead/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/memstead/memstead/releases/tag/v0.1.0
