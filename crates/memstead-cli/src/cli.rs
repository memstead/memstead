//! Clap derive for the `memstead` binary, lifted out of `main.rs` so
//! the xtask doc generator can call `Cli::command()` against the same
//! tree the binary exposes — no duplicated declarations, no drift.
//!
//! One command set, including the multi-mem / mem-repo lifecycle
//! subcommands; the shape-dependent ones refuse at runtime on a
//! folder-only workspace rather than being compiled out.

use clap::{Parser, Subcommand};

use crate::commands;

/// Top-level `--help` epilog describing the exit-code posture. The
/// taxonomy is intentionally coarse — success vs failure — because
/// agents read JSON, not exit codes, and shell scripts can lift the
/// granular `code` from `--json | jq .code`.
///
/// Code 6 breaks that success/failure symmetry on purpose: it means the
/// measurement completed and the caller asked to be gated on what it
/// found. A CI job needs three outcomes, not two, and it cannot get the
/// third from a code that also means "the engine failed to boot". Keep
/// it exclusive to explicit opt-in gate modes — the moment a run that
/// FAILED returns 6, the distinction stops being worth anything.
///
/// The line is "did the measurement complete", not "was everything
/// well". An artifact the pass could not read is a finding: it was
/// observed and could not be adjudicated, which is an answer. An
/// unreadable anchors sidecar is not: nothing could be observed at all,
/// so verify refuses with `ANCHORS_SIDECAR_UNREADABLE` rather than
/// reporting every artifact uncovered — that was a live defect, found
/// 2026-08-21, where a corrupt file produced a red build blaming the
/// mem.
///
/// This string is the source the published reference renders from
/// (`docs-site/.../reference/cli/cli.md`, xtask-generated and
/// drift-gated). Editing the table here and not regenerating leaves the
/// published page asserting an exit-code space the binary no longer has.
pub const EXIT_CODES_HELP: &str = "\
Exit codes:
  0  success
  1  generic failure (catch-all for non-classified errors)
  2  usage error (clap argument-parse failure — unknown flag, bad value)
  3  not found (entity / mem / resource missing)
  4  hash mismatch (optimistic-locking failure on a mutation)
  5  validation / schema / policy refusal
  6  findings present — the measurement COMPLETED and recorded
     something you asked to be gated on
     (`projection verify --fail-on-findings`). A run that could not
     complete returns its own code above, so a CI job can tell \"the
     mem and its source disagree\" from \"the engine could not run\".
     An artifact the pass could not read is a finding, not an error:
     it was observed, and not being able to adjudicate it is the
     measurement's answer.

  For programmatic branching, prefer `--json` over the exit code:
    memstead <subcommand> ... --json | jq -r .code
  One caveat, and it bites exactly where code 6 matters: a gate-mode run
  that exits 6 emits TWO documents on stdout — the report, then the typed
  error. The recipe above reads only the first and prints `null`. Read the
  stream instead:
    memstead ... --fail-on-findings --json | jq -s -r '.[-1].code'
  The JSON envelope's `code` field carries the typed token
  (e.g. INVALID_TITLE, HAS_INCOMING_REFS, CROSS_MEM_LINK_NOT_ALLOWED)
  with structured recovery details under `.details`.";

/// Query and mutate Memstead knowledge graphs from the shell.
#[derive(Parser, Debug)]
// `--version` prints the full build version (engine semver plus the
// git build sha for dev builds) so two builds between releases stay
// distinguishable in the field.
#[command(name = "memstead", version = memstead_base::build_info::full_version(), about, long_about = None, after_long_help = EXIT_CODES_HELP)]
pub struct Cli {
    /// Emit JSON instead of markdown. Matches MCP `structured_content` shape.
    #[arg(long, global = true)]
    pub json: bool,

    /// Suppress engine startup logs on stderr.
    #[arg(long, global = true)]
    pub quiet: bool,

    /// Operate on the workspace at PATH instead of walking up from the
    /// current directory (like `git -C`: the process runs as if
    /// invoked from PATH, so relative path arguments resolve against
    /// it). Also settable via the `MEMSTEAD_WORKSPACE` environment
    /// variable; the flag wins when both are present. A PATH that is
    /// not an initialised workspace refuses with
    /// `WORKSPACE_NOT_INITIALISED` naming the path — it never falls
    /// back to the directory walk.
    #[arg(long, global = true, value_name = "PATH")]
    pub workspace: Option<std::path::PathBuf>,

    /// Declare the role this invocation's mutations are performed in
    /// (agent-trust plan 13): `author` | `checker` | `verifier`.
    /// Recorded immutably alongside each mutation (commit trailer /
    /// ledger). Omit to record mutations as unspecified — legal
    /// forever, never refused.
    #[arg(long = "role", global = true)]
    pub role: Option<String>,

    /// Declare WHO is acting in this invocation (agent-trust plan
    /// 15): an opaque identity string of your choosing — an agent
    /// name, a session handle, a person's tag. Recorded immutably
    /// alongside each mutation and check (commit trailer / ledger);
    /// the author≠checker independence gate compares identities and
    /// nothing else. Also settable via the `MEMSTEAD_IDENTITY`
    /// environment variable; the flag wins when both are present.
    /// Caller-declared and unverified, but tamper-evident in
    /// append-only history. Omit to record operations without an
    /// identity — legal forever, never refused; identity-less
    /// records read `unconfirmable` at the gate.
    #[arg(long = "identity", global = true)]
    pub identity: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Node / edge counts, schema distribution, and per-binding projection state.
    Status,

    /// Read one entity as markdown.
    Entity(commands::entity::Args),

    /// List typed edges for an entity.
    Relations(commands::relations::Args),

    /// Find entities by text or graph proximity.
    Search(commands::search::Args),

    /// Filter entities by metadata (no text match — use `search` for that).
    List(commands::list::Args),

    /// Read an entity's community cluster.
    Context(commands::context::Args),

    /// All clusters with summaries and member lists. Renders the same
    /// rich content the MCP `memstead_overview` tool emits — both
    /// surfaces share the engine composer in `memstead-base`.
    Overview(commands::overview::Args),

    /// Describe one type, or list all types when no name given.
    Type(commands::type_cmd::Args),

    /// Health summary (orphans, stubs, stale entities, missing fields).
    ///
    /// Every report carries a verdict-coverage line with three buckets:
    /// `examined` names the axes the defect verdict answers for (a
    /// finding there fails `--strict`); `advisory` names the axes the
    /// report renders, always or on `--include`, beside the verdict
    /// without folding them in (stale entities, conformance findings,
    /// anchor drift, check states: the figures are shown, the verdict
    /// says nothing about them); `not_examined` names the axes this
    /// surface never looks at, which another surface answers for.
    Health(commands::health::Args),

    /// Render the due-brief: open entities whose schema-declared due
    /// date falls inside the window (default 90d), overdue first.
    Due(commands::due::Args),

    /// Render the gates brief: the standing of every schema-declared gated transition — closed and open entities per gate, related-check coverage, open entities in dependency order.
    Gates(commands::gates::Args),

    /// Export a mem: markdown in place, a portable `.mem` archive, JSON, one self-contained HTML page, or one agent-readable Markdown document (`llms-txt`).
    Export(commands::export::Args),

    /// Initialise a filesystem mem in the current (or named) folder.
    /// Strict: errors out when the target is not empty.
    Init(commands::init::InitArgs),

    /// One-command cold start: workspace + default-schema mem + seed
    /// entity + MCP wiring for your agent(s), in the current (or named)
    /// folder. Tolerates dotfiles and README-grade files; derives the
    /// mem name from the folder. For the strict, script-safe variant
    /// use `memstead init`. Restart the agent session afterwards: a
    /// session that is already running does not attach an MCP server
    /// added while it runs.
    Quickstart(commands::quickstart::Args),

    /// Install a sealed `.mem` mem — either a local file, or `<scope>/<name>`
    /// from the memstead.io registry. Registers it as a workspace-level
    /// read-only mount; `memstead uninstall` is the symmetric removal.
    /// Works on every workspace shape: a read-mem attaches to the workspace,
    /// not to one of your mems.
    Install(commands::install::Args),

    /// Remove an installed read-mem's workspace-level mount. The global
    /// cache copy survives by default; re-`install` re-registers it.
    /// Works on every workspace shape, symmetric with `install`: a
    /// workspace that can attach a read-mem can detach one.
    Uninstall(commands::uninstall::Args),

    /// Verify every anchor in a mem against its declared source — the
    /// standalone drift statement, no binding required. Mutates no entity,
    /// but records its findings store like any verify run.
    #[command(name = "verify-anchors")]
    VerifyAnchors(commands::verify_anchors::Args),

    /// Publish a `.mem` archive to the registry. Triggers GitHub
    /// Device Flow on first use; subsequent runs are silent.
    Publish(commands::publish::Args),

    /// Unpublish (hard-delete) `<scope>/<name>` from the registry.
    /// Permitted to the original uploader and to admins. The same
    /// `<scope>/<name>` becomes immediately re-publishable.
    Unpublish(commands::unpublish::Args),

    /// Domain-authority publishing: generate the signing key for a domain you
    /// control and print the `.well-known` manifest to host. `publish --scope
    /// <domain>:<handle>` then signs with that key — no GitHub account needed.
    Domain {
        #[command(subcommand)]
        action: commands::domain::DomainAction,
    },

    /// Admin-only registry moderation: take a mem down or deny-list
    /// bytes. Gated server-side by the `MEMSTEAD_ADMINS` allowlist; every
    /// action is recorded in the registry's append-only audit log.
    Admin {
        #[command(subcommand)]
        action: commands::admin::AdminAction,
    },

    /// Authenticate with a registry via GitHub Device Flow. Optional —
    /// `publish` auto-triggers the same flow on first use.
    Login(commands::login::Args),

    /// Remove stored credentials for a registry.
    Logout(commands::logout::Args),

    /// Create a new entity. Provide `--title`, `--type`, and the required
    /// section fields, or pass `--from <file.json>` with the full payload.
    Create(commands::create::Args),

    /// Modify an existing entity. `--expected-hash` is required for an update
    /// that changes content, unless `--auto-hash` (refetch before write) or
    /// `--force` (skip check) is given; an anchors-only update needs none,
    /// since anchors sit outside the content hash.
    Update(commands::update::Args),

    /// Add or remove a typed relationship between two entities.
    Relate(commands::relate::Args),

    /// Delete an entity. Use `--dry-run` to preview impact first.
    /// Delete is hashless by design (no post-state to race on); race
    /// protection comes from `HAS_INCOMING_REFS` — and
    /// `RESIDUAL_STUB_FOR_READONLY_REFERRERS` for read-only-referrer cases.
    Delete(commands::delete::Args),

    /// Rename an entity (changes ID, file path, and every incoming wiki-link).
    Rename(commands::rename::Args),

    /// Change an entity's type in place (id, path and incoming edges stay;
    /// sections, metadata and every edge are validated against the target type).
    Retype(commands::retype::Args),

    /// Update many entities in one atomic call. Input is a JSON file
    /// with a top-level `updates: [...]` array (one entry per entity,
    /// each with its own hash mode and mutation fields). All-or-nothing:
    /// if any entry fails (validation, hash mismatch, missing entity)
    /// the whole batch is refused and NOTHING is committed — fix the
    /// named entry and resubmit. On success the batch lands as one
    /// commit. Mirrors `memstead update` per entry.
    /// MEM-REPO WORKSPACES ONLY — refuses with
    /// `UNSUPPORTED_WORKSPACE_SHAPE` on the filesystem-mem workspace
    /// `memstead quickstart` produces; fall back to one `memstead
    /// update` per entity there.
    #[command(name = "batch-update")]
    BatchUpdate(commands::batch_update::Args),

    /// Create many entities in one atomic call. Input is a JSON file
    /// with a top-level `creates: [...]` array — each entry the same
    /// shape as `create --from`, with its own provenance `note`.
    /// Intra-batch references resolve as real targets (cycles included
    /// where the schema permits), so a mutually-referencing set lands
    /// in a single pass with no stubs. All-or-nothing: any invalid
    /// entry refuses the whole batch and names EVERY failing entry.
    /// One commit per touched mem.
    /// MEM-REPO WORKSPACES ONLY — refuses with
    /// `UNSUPPORTED_WORKSPACE_SHAPE` on the filesystem-mem workspace
    /// `memstead quickstart` produces; fall back to one `memstead
    /// create` per entity there (losing atomicity and intra-batch
    /// reference resolution).
    #[command(name = "batch-create")]
    BatchCreate(commands::batch_create::Args),

    /// Apply many edge changes in one atomic call. Input is a JSON
    /// file with a top-level `relates: [...]` array mixing additions
    /// and removals, applied in order — each entry mirrors `relate`
    /// (`from` / `rel_type` / `to`, optional `remove`, `description`,
    /// per-entry `note`). All-or-nothing: any invalid entry refuses
    /// the whole batch and names EVERY failing entry. One commit per
    /// touched mem.
    /// MEM-REPO WORKSPACES ONLY — refuses with
    /// `UNSUPPORTED_WORKSPACE_SHAPE` on the filesystem-mem workspace
    /// `memstead quickstart` produces; fall back to one `memstead
    /// relate` per edge there.
    #[command(name = "batch-relate")]
    BatchRelate(commands::batch_relate::Args),

    /// Apply parse-time-drift recovery across writable mems. Walks
    /// `PARSED_RELATION_INVALID` warnings, re-renders affected
    /// source entities to drop the stale rows, and reports per-entry
    /// outcomes. Read-only-origin drops surface as skipped.
    Recover(commands::recover::Args),

    /// Read provenance anchors (E3a): `memstead anchors <id>` lists an
    /// entity's anchors + composition; `memstead anchors --artifact <path>`
    /// reverse-looks-up every entity whose anchor references that path
    /// (the query the check-realization hook consumes).
    Anchors(commands::anchors::Args),

    /// List and resolve git merge conflicts in folder-backed mems —
    /// the one sanctioned repair when a merge in the user's repo
    /// writes conflict markers into entity files. `conflicts list`
    /// shows conflicted entities; `conflicts resolve <id> --side
    /// ours|theirs` keeps one side, validated before it lands and
    /// committed as an attributed mutation.
    Conflicts(commands::conflicts::Args),

    /// Report a mem's changes since a cursor. The cursor is
    /// backend-specific and is never a mutation's `write_id`: on a
    /// git-branch mem pass a commit SHA (the `head` a prior call
    /// returned, or the canonical empty-tree hash
    /// `4b825dc642cb6eb9a060e54bf8d69288fbee4904` for a first sync);
    /// on a folder mem pass an RFC3339 timestamp (the `ts` of the last
    /// ledger entry you read, or empty for a first sync).
    Changes(commands::changes::Args),

    /// Record a check: "entity E checked, verdict ok | failed, via
    /// method M" — an engine-recorded act carrying the session's
    /// `--role`, never a mutation (entity markdown, hash, and mem
    /// commits untouched). Derived check state serves via
    /// `memstead entity <id> --provenance`.
    Check(commands::check::Args),

    /// Read and move the per-mem review mark — the engine's one
    /// pointer per mem to the last human-approved state. `list` shows
    /// every mem's mark and head; `set`/`clear` move it (explicit
    /// target only); `diff` reports the unreviewed delta. Marks never
    /// gate writes.
    #[command(name = "review-mark")]
    ReviewMark(commands::review_mark::Args),

    /// Reload one writable mem's slice of the in-memory store from
    /// its on-disk branch tip — or every writable mem when
    /// `--mem` is omitted. CLI parity with the MCP `memstead_reload`
    /// tool.
    Reload(commands::reload::Args),

    /// Fetch a mem's branch refs from a git remote into the mem-repo
    /// (no local branch moves — inspect first, then `pull`). Requires a
    /// git-branch-backed mem (`INVALID_INPUT` on folder mounts);
    /// refuses `UNKNOWN_REMOTE` when the remote is not configured.
    Fetch(commands::transport::FetchArgs),

    /// Fast-forward a mem's branch to its fetched remote counterpart
    /// and reload the in-memory store. Refuses `LOCAL_DIVERGENCE` when
    /// the local branch is not an ancestor of the remote — reconcile
    /// via `branch-reset`, or resolve on another clone and push.
    Pull(commands::transport::PullArgs),

    /// Push a mem's branch to a git remote. `--force` uses
    /// force-with-lease semantics; without it, non-fast-forward pushes
    /// refuse (`NON_FAST_FORWARD`). Refuses `UNKNOWN_REMOTE` when the
    /// remote is not configured. `--all` pushes every mounted
    /// git-branch mem's branch plus the workspace's schema-and-config
    /// ref, fast-forward only: silent for refs already in sync, one line
    /// per ref moved, a refused ref named while the others still go,
    /// non-zero exit at the end.
    Push(commands::transport::PushArgs),

    /// Reset a mem's branch pointer to a target ref/SHA. Refuses to
    /// discard commits reachable from any remote ref
    /// (`PUSHED_COMMITS_PROTECTED`).
    #[command(name = "branch-reset")]
    BranchReset(commands::branch_reset::BranchResetArgs),

    /// Mem lifecycle commands.
    Mem {
        #[command(subcommand)]
        action: commands::mem::MemAction,
    },

    /// Mem-repo-git lifecycle commands.
    #[command(name = "mem-repo")]
    MemRepo {
        #[command(subcommand)]
        action: commands::mem_repo::MemRepoAction,
    },

    /// Introspect and configure workspace policy — `dump` reads the
    /// effective config; `allow-create`/`revoke-create`/`allow-delete`/
    /// `revoke-delete`/`grant-cross-link`/`revoke-cross-link`/`set-mutations`
    /// write the mem-lifecycle allowlist, cross-mem link grants, and
    /// mutation policy.
    Workspace {
        #[command(subcommand)]
        action: commands::workspace::WorkspaceAction,
    },

    /// Author-time schema tooling. `memstead schema validate <path>`
    /// checks a schema package directory against the engine's loader
    /// without touching a workspace.
    Schema(commands::schema::Args),

    /// Pipeline tooling — one versioned v2 binding per pipeline, sources
    /// inline. Nine verbs: `brief` renders a binding's run-brief (the
    /// Markdown prompt an agent consumes); `init` scaffolds a fresh v2
    /// record non-interactively; `migrate` converts every prior on-disk
    /// generation (gen-1 root folders, the four-primitive store, the v1
    /// three-file store) into v2 records in place; `enable
    /// <build|sync|verify> <binding>` adds a missing operation block;
    /// `edit` patches a binding's author-editable fields; `advance`
    /// records disposition-gated sync-baseline advances; `exclude`
    /// records authored exclusions for in-scope artifacts; `verify`
    /// measures a binding's fidelity and records findings; `check-path`
    /// answers deny verdicts for paths and patterns.
    Projection(commands::projection::Args),
}

impl Command {
    /// The subcommand's user-facing verb name, as typed on the command
    /// line — the `verb` field the friction ledger records on a typed
    /// refusal. Nested action groups report their top-level noun
    /// (`mem`, `mem-repo`, `workspace`, `domain`, `admin`): per-verb
    /// counts at that granularity already answer the design questions,
    /// and nothing payload-shaped can leak through a static name.
    pub fn verb(&self) -> &'static str {
        match self {
            Command::Status => "status",
            Command::Entity(_) => "entity",
            Command::Relations(_) => "relations",
            Command::Search(_) => "search",
            Command::List(_) => "list",
            Command::Context(_) => "context",
            Command::Overview(_) => "overview",
            Command::Type(_) => "type",
            Command::Health(_) => "health",
            Command::Due(_) => "due",
            Command::Gates(_) => "gates",
            Command::Export(_) => "export",
            Command::Init(_) => "init",
            Command::Quickstart(_) => "quickstart",
            Command::Install(_) => "install",
            Command::Uninstall(_) => "uninstall",
            Command::VerifyAnchors(_) => "verify-anchors",
            Command::Publish(_) => "publish",
            Command::Unpublish(_) => "unpublish",
            Command::Domain { .. } => "domain",
            Command::Admin { .. } => "admin",
            Command::Login(_) => "login",
            Command::Logout(_) => "logout",
            Command::Create(_) => "create",
            Command::Update(_) => "update",
            Command::Relate(_) => "relate",
            Command::Delete(_) => "delete",
            Command::Rename(_) => "rename",
            Command::Retype(_) => "retype",
            Command::BatchUpdate(_) => "batch-update",
            Command::BatchCreate(_) => "batch-create",
            Command::BatchRelate(_) => "batch-relate",
            Command::Recover(_) => "recover",
            Command::Anchors(_) => "anchors",
            Command::Conflicts(_) => "conflicts",
            Command::Changes(_) => "changes",
            Command::Check(_) => "check",
            Command::ReviewMark(_) => "review-mark",
            Command::Reload(_) => "reload",
            Command::Fetch(_) => "fetch",
            Command::Pull(_) => "pull",
            Command::Push(_) => "push",
            Command::BranchReset(_) => "branch-reset",
            Command::Mem { .. } => "mem",
            Command::MemRepo { .. } => "mem-repo",
            Command::Workspace { .. } => "workspace",
            Command::Schema(_) => "schema",
            Command::Projection(_) => "projection",
        }
    }
}

#[cfg(test)]
mod write_id_gloss_tests {
    use clap::CommandFactory;

    /// The CLI twin of `memstead-mcp`'s
    /// `no_mutation_description_glosses_write_id_as_git_or_cursor`.
    ///
    /// That guard walks the five MCP tool descriptions and nothing
    /// else, so it was blind to the clap tree — and the clap tree is
    /// exactly where the defect survived a sweep: `changes` kept an
    /// about-text reading "Pass `--since` = a prior `write_id` from a
    /// mutation" while its own `--since` help said the cursor is never
    /// a `write_id`. One help screen, the wrong instruction and its
    /// correction, both on screen at once. A rename that only replaces
    /// the identifier and never re-reads the sentence around it
    /// produces precisely that, so the check belongs where the
    /// sentences are.
    ///
    /// Walks every help string in the tree: each command's about and
    /// long-about, and every argument's help and long-help.
    #[test]
    fn no_cli_help_text_glosses_write_id_as_git_or_cursor() {
        // Each phrase would reintroduce one half of the defect: a git
        // identity claim, or cursor advice.
        // Structural, matching the MCP guard. This was a list of eight
        // literals until 2026-08-27, which its own name already
        // contradicted: "the `write_id` is a per-mem commit identifier"
        // passes a list built for "per-mem git", and that is the exact
        // evasion `ops/mod.rs` was rewritten to close. A sentence naming
        // the token and calling it a commit must also name WHICH backend
        // produces one; no sentence naming it may invite polling.
        const CURSOR_INVITES: &[&str] = &[
            "polling",
            "poll via",
            "since cursor",
            "as the `since`",
            "prior `write_id`",
            "`write_id` from a mutation",
        ];

        fn texts(cmd: &clap::Command, path: &str, out: &mut Vec<(String, String)>) {
            let mut push = |s: Option<&clap::builder::StyledStr>| {
                if let Some(v) = s {
                    out.push((path.to_string(), v.to_string()));
                }
            };
            push(cmd.get_about());
            push(cmd.get_long_about());
            for arg in cmd.get_arguments() {
                if let Some(h) = arg.get_help() {
                    out.push((format!("{path} --{}", arg.get_id()), h.to_string()));
                }
                if let Some(h) = arg.get_long_help() {
                    out.push((format!("{path} --{}", arg.get_id()), h.to_string()));
                }
            }
            for sub in cmd.get_subcommands() {
                if sub.get_name() == "help" {
                    continue;
                }
                let child = if path.is_empty() {
                    sub.get_name().to_string()
                } else {
                    format!("{path} {}", sub.get_name())
                };
                texts(sub, &child, out);
            }
        }

        let cmd = super::Cli::command();
        let mut all = Vec::new();
        texts(&cmd, "", &mut all);

        let mut violations = Vec::new();
        for (where_, text) in &all {
            if !text.contains("write_id") {
                continue;
            }
            // Judge EACH sentence naming the token on its own. Joining
            // them first was the flaw in the first cut: a correct
            // sentence later in the same help text excused a wrong one
            // earlier, so "The `write_id` is a per-mem commit
            // identifier" passed as long as some other sentence said
            // "git-branch". Per-sentence also keeps a legitimate gitdir
            // mention about something else out of scope without an
            // allowlist, and allowlists are where the next drift hides.
            for sentence in text.split(". ").filter(|s| s.contains("write_id")) {
                let lower = sentence.to_lowercase();
                if (lower.contains("commit") || lower.contains("sha"))
                    && !lower.contains("git-branch")
                {
                    violations.push(format!(
                        "`memstead {where_}` help calls `write_id` a commit without naming \
                         which backend produces one — {sentence}"
                    ));
                }
                if lower.contains("gitdir") || lower.contains("include_config") {
                    violations.push(format!(
                        "`memstead {where_}` help points at a gitdir in a sentence about \
                         `write_id` — the lookup errors on a backend without one"
                    ));
                }
                for phrase in CURSOR_INVITES {
                    if lower.contains(phrase) {
                        violations.push(format!(
                            "`memstead {where_}` help invites polling with `write_id` \
                             (\"{phrase}\") — it is an identity, not a change cursor"
                        ));
                    }
                }
            }
        }
        assert!(
            violations.is_empty(),
            "write_id gloss violations in CLI help:\n  {}",
            violations.join("\n  ")
        );
        // Guard the guard: if the token ever stops appearing in CLI
        // help at all, the loop above passes vacuously.
        assert!(
            all.iter().any(|(_, t)| t.contains("write_id")),
            "no CLI help text mentions `write_id` — this check has gone vacuous"
        );

        // Second half: the edge spelling. The loop above only inspects
        // text that names `write_id`, so it was blind to help that
        // documents a relation entry with the retired bare `type` —
        // which `batch-relate`'s about-text did, describing a shape its
        // own `deny_unknown_fields` parser refuses. A door documenting
        // what it rejects is worse than one saying nothing.
        const RETIRED_EDGE_SHAPES: &[&str] = &[
            "`from` / `type` / `to`",
            "`from`/`type`/`to`",
            "{from, to, type}",
            "{to, type}",
        ];
        let mut edge_violations = Vec::new();
        for (where_, text) in &all {
            for shape in RETIRED_EDGE_SHAPES {
                if text.contains(shape) {
                    edge_violations.push(format!(
                        "`memstead {where_}` help documents a relation entry as {shape} — \
                         the type is `rel_type` on every surface and the parser refuses \
                         the retired spelling"
                    ));
                }
            }
        }
        // Vacuity floor for THIS half. The token half above asserts the
        // token is mentioned somewhere; nothing asserted that any help
        // text documents a relation entry at all, so if `--relation`
        // stopped naming a shape this check would pass in silence.
        assert!(
            all.iter()
                .any(|(_, t)| t.contains("REL_TYPE:") || t.contains("rel_type")),
            "no CLI help documents a relation entry shape — this check has gone vacuous"
        );
        assert!(
            edge_violations.is_empty(),
            "retired edge spelling in CLI help:\n  {}",
            edge_violations.join("\n  ")
        );
    }

    /// Third surface class: what the CLI PRINTS, as opposed to what it
    /// documents.
    ///
    /// The guard above walks the clap tree, which is help text only. It
    /// could not see `mem init`'s receipt rendering the token under the
    /// label "Seed commit" on a folder mem — three lines above a warning
    /// saying the same value is not a commit. The rename had replaced
    /// the identifier in the format argument and left the label beside
    /// it, which is this plan's recurring failure in its third costume.
    ///
    /// Walks the crate's own sources for a format string that labels a
    /// write-token value with git vocabulary. Deliberately allowlist-free:
    /// every label was made backend-neutral instead, so an exemption list
    /// would be the first place the next drift hides.
    #[test]
    fn no_rendered_cli_output_labels_a_write_id_as_a_commit() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        walk(&src, &mut files);
        assert!(
            !files.is_empty(),
            "found no sources — check has gone vacuous"
        );

        let mut violations = Vec::new();
        let mut saw_a_render = false;
        for path in &files {
            let Ok(text) = std::fs::read_to_string(path) else {
                continue;
            };
            // Skip this module's own failure messages, which necessarily
            // quote the vocabulary they forbid.
            let text = text
                .split_once("mod write_id_gloss_tests")
                .map(|(before, _)| before.to_string())
                .unwrap_or(text);
            let lines: Vec<&str> = text.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                let renders_token = line.contains("write_id");
                if renders_token && (line.contains("format!") || line.contains("push_str")) {
                    saw_a_render = true;
                }
                if !renders_token {
                    continue;
                }
                // Widen to a small window, not just this line. A label
                // sits on the line above its value whenever the
                // `format!` is wrapped, and a same-line-only rule is
                // blind to exactly the costume the defect wore here.
                let lo = i.saturating_sub(2);
                let hi = (i + 3).min(lines.len());
                let window = lines[lo..hi].join(" ").to_lowercase();
                let renders = lines[lo..hi]
                    .iter()
                    .any(|l| l.contains("format!") || l.contains("push_str"));
                if (window.contains("commit") || window.contains(" sha")) && renders {
                    violations.push(format!(
                        "{}:{}: {}",
                        path.file_name().unwrap_or_default().to_string_lossy(),
                        i + 1,
                        line.trim()
                    ));
                }
            }
        }
        assert!(
            saw_a_render,
            "no CLI source renders a write token — this check has gone vacuous"
        );
        assert!(
            violations.is_empty(),
            "rendered CLI output labels a write token with git vocabulary:\n  {}",
            violations.join("\n  ")
        );
    }
}
