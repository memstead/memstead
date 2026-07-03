//! Clap derive for the `memstead` binary, lifted out of `main.rs` so
//! the xtask doc generator can call `Cli::command()` against the same
//! tree the binary exposes — no duplicated declarations, no drift.
//!
//! One crate, two build configs: the default (`mem-repo`) build
//! exposes the full command set including the multi-mem / mem-repo
//! lifecycle subcommands; `--no-default-features` drops those, leaving
//! the engine-agnostic surface.

use clap::{Parser, Subcommand};

use crate::commands;

/// Top-level `--help` epilog describing the exit-code posture. The
/// taxonomy is intentionally coarse — success vs failure — because
/// agents read JSON, not exit codes, and shell scripts can lift the
/// granular `code` from `--json | jq .code`.
pub const EXIT_CODES_HELP: &str = "\
Exit codes:
  0  success
  1  generic failure (catch-all for non-classified errors)
  2  usage error (clap argument-parse failure — unknown flag, bad value)
  3  not found (entity / mem / resource missing)
  4  hash mismatch (optimistic-locking failure on a mutation)
  5  validation / schema / policy refusal

  For programmatic branching, prefer `--json` over the exit code:
    memstead <subcommand> ... --json | jq -r .code
  The JSON envelope's `code` field carries the typed token
  (e.g. INVALID_TITLE, HAS_INCOMING_REFS, CROSS_MEM_LINK_NOT_ALLOWED)
  with structured recovery details under `.details`.";

/// Query and mutate Memstead knowledge graphs from the shell.
#[derive(Parser, Debug)]
#[command(name = "memstead", version, about, long_about = None, after_long_help = EXIT_CODES_HELP)]
pub struct Cli {
    /// Emit JSON instead of markdown. Matches MCP `structured_content` shape.
    #[arg(long, global = true)]
    pub json: bool,

    /// Suppress engine startup logs on stderr.
    #[arg(long, global = true)]
    pub quiet: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Node / edge counts and schema distribution.
    Stats,

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

    /// All clusters with summaries and member lists. The full build
    /// renders the same rich content the MCP `memstead_overview` tool
    /// emits — both surfaces share the engine composer in `memstead-engine`.
    Overview(commands::overview::Args),

    /// Describe one type, or list all types when no name given.
    Type(commands::type_cmd::Args),

    /// Health summary (orphans, stubs, stale entities, missing fields).
    Health(commands::health::Args),

    /// Export the write mem as markdown (in place) or as a portable `.mem` archive.
    Export(commands::export::Args),

    /// Initialise a filesystem mem in the current (or named) folder.
    /// Strict: errors out when the target is not empty.
    Init(commands::init::InitArgs),

    /// One-command cold start: workspace + default-schema mem + seed
    /// entity + MCP wiring for your agent(s), in the current (or named)
    /// folder. Tolerates dotfiles and README-grade files; derives the
    /// mem name from the folder. For the strict, script-safe variant
    /// use `memstead init`.
    Quickstart(commands::quickstart::Args),

    /// Install a sealed `.mem` mem — either a local file, or `<scope>/<name>`
    /// from the memstead.io registry.
    #[cfg(feature = "mem-repo")]
    Install(commands::install::Args),

    /// Link a filesystem mem to a registry-published dependency.
    /// `memstead link <scope/name>` fetches the archive into
    /// `.memstead/memstead-io/` and records the dep in `.memstead/config.json`.
    Link(commands::link::LinkArgs),

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

    /// Modify an existing entity. `--expected-hash` is required unless
    /// `--auto-hash` (refetch before write) or `--force` (skip check) is given.
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

    /// Update many entities in one atomic call. Input is a JSON file
    /// with a top-level `updates: [...]` array (one entry per entity,
    /// each with its own hash mode and mutation fields). All-or-nothing:
    /// if any entry fails (validation, hash mismatch, missing entity)
    /// the whole batch is refused and NOTHING is committed — fix the
    /// named entry and resubmit. On success the batch lands as one
    /// commit. Mirrors `memstead update` per entry.
    #[cfg(feature = "mem-repo")]
    #[command(name = "batch-update")]
    BatchUpdate(commands::batch_update::Args),

    /// Apply parse-time-drift recovery across writable mems. Walks
    /// `PARSED_RELATION_INVALID` warnings, re-renders affected
    /// source entities to drop the stale rows, and reports per-entry
    /// outcomes. Read-only-origin drops surface as skipped.
    #[cfg(feature = "mem-repo")]
    Recover(commands::recover::Args),

    /// Diff a mem's HEAD against a commit SHA. Pass `--since` = a
    /// prior `commit_sha` from a mutation, or the canonical empty-tree
    /// hash `4b825dc642cb6eb9a060e54bf8d69288fbee4904` for a first sync.
    Changes(commands::changes::Args),

    /// Reload one writable mem's slice of the in-memory store from
    /// its on-disk branch tip — or every writable mem when
    /// `--mem` is omitted. CLI parity with the MCP `memstead_reload`
    /// tool.
    Reload(commands::reload::Args),

    /// Fetch a mem's branch refs from a git remote into the mem-repo
    /// (no local branch moves — inspect first, then `pull`). Requires a
    /// git-branch-backed mem (`INVALID_INPUT` on folder mounts);
    /// refuses `UNKNOWN_REMOTE` when the remote is not configured.
    #[cfg(feature = "mem-repo")]
    Fetch(commands::transport::FetchArgs),

    /// Fast-forward a mem's branch to its fetched remote counterpart
    /// and reload the in-memory store. Refuses `LOCAL_DIVERGENCE` when
    /// the local branch is not an ancestor of the remote — reconcile
    /// via `branch-reset`, or resolve on another clone and push.
    #[cfg(feature = "mem-repo")]
    Pull(commands::transport::PullArgs),

    /// Push a mem's branch to a git remote. `--force` uses
    /// force-with-lease semantics; without it, non-fast-forward pushes
    /// refuse (`NON_FAST_FORWARD`). Refuses `UNKNOWN_REMOTE` when the
    /// remote is not configured.
    #[cfg(feature = "mem-repo")]
    Push(commands::transport::PushArgs),

    /// Reset a mem's branch pointer to a target ref/SHA. Refuses to
    /// discard commits reachable from any remote ref
    /// (`PUSHED_COMMITS_PROTECTED`).
    #[cfg(feature = "mem-repo")]
    #[command(name = "branch-reset")]
    BranchReset(commands::branch_reset::BranchResetArgs),

    /// Mem lifecycle commands.
    #[cfg(feature = "mem-repo")]
    Mem {
        #[command(subcommand)]
        action: commands::mem::MemAction,
    },

    /// Mem-repo-git lifecycle commands.
    #[cfg(feature = "mem-repo")]
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
    #[cfg(feature = "mem-repo")]
    Workspace {
        #[command(subcommand)]
        action: commands::workspace::WorkspaceAction,
    },

    /// Author-time schema tooling. `memstead schema validate <path>`
    /// checks a schema package directory against the engine's loader
    /// without touching a workspace.
    Schema(commands::schema::Args),

    /// Pipeline-config tooling. `memstead pipeline migrate` converts the
    /// legacy `scopes|projections|ingests/` JSON folders into the
    /// `.memstead/` workspace store's four-primitive shape.
    Pipeline(commands::pipeline::Args),
}
