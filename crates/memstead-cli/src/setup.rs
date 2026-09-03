//! Engine setup from global CLI flags. Produces an `Engine`
//! synchronously (no tokio) for the CLI to call into directly.
//!
//! Post-rebuild there is one workspace marker: `.memstead/workspace.toml`
//! at the workspace root. The `mem-repo` Cargo feature decides
//! which engine factory consumes it — full routes through
//! [`memstead_git_branch::workspace_store::engine_from_workspace_root`]
//! (git-branch backends plus folder + archive), lean routes through
//! [`memstead_base::Engine::from_workspace_root`] (folder + archive
//! only).
//!
//! [`CliEngine`] wraps either flavour; subcommands match-dispatch on
//! it. The `WorkspaceShape` variant is retained so the lean build
//! can still surface an actionable "this is the lean binary, your
//! workspace has git-branch mounts" error when the operator points a
//! lean binary at a full workspace — the shape tag is derived from
//! `mem-repo/.git` co-existing with the marker rather than the
//! marker itself.

use std::path::{Path, PathBuf};

#[cfg(feature = "mem-repo")]
use anyhow::Context;

use memstead_base::Engine as BaseEngine;
use memstead_base::vcs::ClientId;
#[cfg(feature = "mem-repo")]
use memstead_base::vcs::{Actor, CommitContext};
#[cfg(feature = "mem-repo")]
use memstead_git_branch::workspace_store::engine_from_workspace_root;

use crate::CliError;
use crate::output::ExitKind;

/// Structured-code constant for the missing-workspace exit envelope.
/// Surfaced on both `--json` output (under the `code` key in
/// `details`) and as the `Display` body of the underlying `CliError`.
/// Scripts and agents branch on this stable token; the human prose
/// (which mentions the recovery command) is the message and can be
/// adjusted without breaking the contract.
pub const WORKSPACE_NOT_INITIALISED_CODE: &str = "WORKSPACE_NOT_INITIALISED";

/// Recovery command suggested when no `.memstead/workspace.toml` is
/// reachable from cwd. `memstead mem-repo init` in the full build (this
/// binary speaks mem-repo); `memstead init` in the lean build. The
/// structured `hint.recovery_command` field carries this token
/// verbatim so an agent can re-exec it.
#[cfg(feature = "mem-repo")]
pub const WORKSPACE_RECOVERY_COMMAND: &str = "memstead mem-repo init";
#[cfg(not(feature = "mem-repo"))]
pub const WORKSPACE_RECOVERY_COMMAND: &str = "memstead init";

/// Build the typed `WORKSPACE_NOT_INITIALISED` exit envelope. Goes
/// through `CliError` so the top-level `main` downcast lifts the
/// `code` + `hint` fields into the JSON output.
pub fn workspace_not_initialised_error(message: &str) -> CliError {
    CliError {
        kind: ExitKind::Generic,
        code: WORKSPACE_NOT_INITIALISED_CODE,
        message: message.to_string(),
        details: Some(serde_json::json!({
            "hint": { "recovery_command": WORKSPACE_RECOVERY_COMMAND },
        })),
    }
}

/// Lift a [`memstead_base::BootError`] into the typed CLI envelope.
/// The boot seam previously flattened these through `anyhow`, so the
/// `main` downcast missed them and every boot failure surfaced as
/// `code: INTERNAL` with no next step (plenum 2026-08-06/07, expertise
/// 2026-08-07). The typed material lives on
/// [`memstead_base::BootError::code`]; this function only wraps it in
/// the CLI's exit shape. The message is
/// [`memstead_base::BootError::surface_message`] verbatim — identical
/// on the MCP server's boot diagnostics for the same broken workspace.
pub fn boot_error_to_cli(workspace_root: &Path, e: memstead_base::BootError) -> CliError {
    let details = e.details();
    let details = match &details {
        serde_json::Value::Object(map) if map.is_empty() => None,
        _ => Some(details),
    };
    CliError {
        kind: ExitKind::Generic,
        code: e.code(),
        message: e.surface_message(workspace_root),
        details,
    }
}

/// Global CLI state: shared flags + a lazily-initialized `Engine`.
pub struct CliContext {
    pub json: bool,
    /// User asked for quiet stderr (`--quiet`). The CLI runs the
    /// engine in-process and never installs a `tracing_subscriber`,
    /// so the flag is informational.
    pub quiet: bool,
    /// The invocation-level declared role (`--role`, agent-trust
    /// plan 13), already validated at parse time. Stamped onto every
    /// engine this context constructs so mutations record it.
    pub role: memstead_base::vcs::Role,
    /// The invocation-level declared identity (`--identity` /
    /// `MEMSTEAD_IDENTITY`, agent-trust plan 15), already normalised
    /// and length-checked at parse time. Stamped onto every engine
    /// this context constructs so mutations and checks record it.
    pub identity: Option<String>,
}

/// Workspace flavour resolved from cwd. Subcommands dispatch on this
/// to pick the right engine accessor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceShape {
    /// Mem-repo workspace — multi-mem, git-backed.
    /// The `.memstead/workspace.toml` root also carries `mem-repo/.git/`.
    MemRepo,
    /// Filesystem-mem workspace — single-mem, history-free.
    /// The `.memstead/workspace.toml` root has no `mem-repo/.git/`.
    Filesystem,
}

/// Render a string as one POSIX shell word. Bare when every character
/// is safe unquoted; otherwise single-quoted, with embedded `'` closed
/// and re-opened the POSIX way (`'\''`).
///
/// A leading `-` forces quoting even though `-` is otherwise safe: an
/// argument that starts with a dash is read as an option by whatever
/// receives it. (Quoting alone does not save `cd`, which parses its
/// argument after the shell strips quotes — callers printing a `cd`
/// emit `cd --`.)
///
/// Lives here rather than beside its first caller because every message
/// that interpolates a filesystem path into a command the reader is
/// expected to run needs it, and the one that did not — the shape
/// disclosure's other-shape command — was unrunnable for anyone whose
/// binary path contained a space.
pub fn shell_quote(value: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "._-/@:+,=".contains(c);
    if !value.is_empty() && !value.starts_with('-') && value.chars().all(safe) {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// The running binary, resolved and shell-quoted — the form to
/// interpolate into any command a message tells the reader to run.
fn memstead_word() -> String {
    shell_quote(&memstead_program())
}

/// The `UNSUPPORTED_WORKSPACE_SHAPE` refusal, in one place because both
/// mem-repo-only gates mint it and they must not drift. Names the
/// recovering command and the verbs that do work here, both resolved to
/// this binary — the refusal is read by someone who is about to type
/// what it says.
///
/// Both gates that mint it are mem-repo-only, so the lean build never
/// reaches this refusal (it has no mem-repo-only subcommand to refuse).
#[cfg(feature = "mem-repo")]
fn unsupported_workspace_shape_message() -> String {
    let m = memstead_word();
    format!(
        "this subcommand is mem-repo-only and not yet supported on filesystem-mem workspaces — \
         bootstrap one with `{m} mem-repo init` in a fresh folder, or use `{m} status` / \
         `{m} list` / `{m} search` / `{m} entity` / `{m} health` / \
         `{m} create|update|delete|relate|rename` here instead."
    )
}

/// Resolve the running `memstead` binary to something the reader can
/// actually type. Bare `memstead` when that name on `PATH` resolves to
/// this very binary; otherwise the path we were invoked as.
///
/// A reader who ran `./target/debug/memstead`, or an unpacked download,
/// or a binary under a versioned directory, has no `memstead` on
/// `PATH` — and every printed command naming a bare `memstead` fails
/// for them with `command not found`. Every message that tells someone
/// to run this binary goes through here.
pub fn memstead_program() -> String {
    let Ok(exe) = std::env::current_exe() else {
        return "memstead".to_string();
    };
    let canonical_exe = exe.canonicalize().unwrap_or_else(|_| exe.clone());
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join("memstead");
            if candidate.is_file() && candidate.canonicalize().is_ok_and(|c| c == canonical_exe) {
                return "memstead".to_string();
            }
        }
    }
    exe.display().to_string()
}

/// The command that produces the *other* shape than the one a
/// disclosure is describing. Feature-gated because every command a
/// message names must exist in the binary that prints it: the lean
/// build has no `mem-repo` subcommand group, so it points at the full
/// build rather than at a verb it would reject. The program name is
/// resolved rather than hardcoded, for the same reason the verify
/// commands resolve it — this is an instruction, not a mention.
#[cfg(feature = "mem-repo")]
fn mem_repo_init_hint() -> String {
    format!("`{} mem-repo init` in a fresh folder", memstead_word())
}
#[cfg(not(feature = "mem-repo"))]
fn mem_repo_init_hint() -> String {
    "the full build of memstead (this lean build has no `mem-repo` subcommand), then \
     `memstead mem-repo init` in a fresh folder"
        .to_string()
}

/// What a filesystem-mem workspace cannot do — stated with the same
/// feature gate as the hint above, and for the same reason. The full
/// build names `memstead install`, which exists there and refuses by
/// shape; the lean build has no `install` subcommand at all, so naming
/// it would send the reader to a verb that does not parse. The lean
/// wording states the limit without borrowing a command it lacks.
#[cfg(feature = "mem-repo")]
const FILESYSTEM_CANNOT: &str = "**It cannot install mems from the registry.** `memstead install \
     <scope>/<name>` (and the other mem-repo-only subcommands) refuse here with \
     `UNSUPPORTED_WORKSPACE_SHAPE`.";
#[cfg(not(feature = "mem-repo"))]
const FILESYSTEM_CANNOT: &str = "**It cannot install mems from the registry, and holds exactly \
     one mem.** The subcommands that do either are mem-repo-only, and this lean build does not \
     carry them at all.";

impl WorkspaceShape {
    /// Resolve the shape of an existing workspace root. Routes through
    /// the engine's shared probe so the CLI, the refusals, and the MCP
    /// boot line can never disagree about the same directory.
    pub fn at(workspace_root: &Path) -> Self {
        if memstead_base::is_mem_repo_shaped(workspace_root) {
            WorkspaceShape::MemRepo
        } else {
            WorkspaceShape::Filesystem
        }
    }

    /// The one spelling of this shape, shared with the engine.
    pub fn label(self) -> &'static str {
        match self {
            WorkspaceShape::MemRepo => "mem-repo",
            WorkspaceShape::Filesystem => "filesystem-mem",
        }
    }
}

/// The three-part disclosure a workspace-creating command owes its
/// caller: which shape was just made, one concrete thing that shape
/// cannot do, and the exact command that produces the other one.
///
/// Held as parts rather than pre-rendered prose because both receipts
/// carry it: the markdown block a human reads, and the `--json`
/// envelope an agent reads. A label alone on the machine surface would
/// name the fork without disclosing it, which is the failure this whole
/// disclosure exists to end — so both renderings come from one value.
pub struct ShapeDisclosure {
    /// The shape just created.
    pub shape: WorkspaceShape,
    /// One sentence on what this shape is.
    pub summary: String,
    /// One concrete thing this shape cannot do, in markdown.
    pub cannot: &'static str,
    /// The shape a caller would get instead.
    pub other_shape: WorkspaceShape,
    /// The exact command producing [`Self::other_shape`], in markdown.
    pub other_shape_command: String,
}

/// The disclosure for a shape.
///
/// `quickstart`, `init`, and `mem-repo init` all print this — the
/// disclosure is symmetric, not a warning bolted onto one branch. It
/// belongs in the creating command's own receipt because that is the
/// moment the fork is decided and the output the newcomer is already
/// reading; a sentence elsewhere (the `install --help` clause)
/// demonstrably arrives after the workspace exists.
pub fn shape_disclosure(shape: WorkspaceShape) -> ShapeDisclosure {
    shape_disclosure_in(shape, None)
}

/// The disclosure for a shape whose mem folder is `mem_folder` — a
/// workspace-relative folder name when the mem does not own the
/// workspace root (the guided `quickstart --repo` layout), `None` for
/// the collapsed shape every other front door creates.
///
/// The parameter exists because the filesystem shape's summary makes a
/// claim about *where the files are*, and that claim is the reader's
/// first check: pointing them at "this folder" when their entities live
/// one folder down would be untrue in exactly the receipt that has to
/// be trusted.
pub fn shape_disclosure_in(shape: WorkspaceShape, mem_folder: Option<&str>) -> ShapeDisclosure {
    match shape {
        WorkspaceShape::Filesystem => ShapeDisclosure {
            shape,
            summary: match mem_folder {
                None => "One mem, plain `.md` files in this folder, no git history — nothing \
                         else to set up."
                    .to_string(),
                Some(folder) => format!(
                    "One mem, plain `.md` files in `{folder}/` — that folder is the whole \
                     graph, and Memstead keeps no history of its own for it."
                ),
            },
            cannot: FILESYSTEM_CANNOT,
            other_shape: WorkspaceShape::MemRepo,
            other_shape_command: format!(
                "**The other shape** — mem-repo: many mems, git-backed, registry-capable — \
                 comes from {hint}. Switching later means starting a second \
                 workspace, so decide now if you intend to install mems.",
                hint = mem_repo_init_hint(),
            ),
        },
        WorkspaceShape::MemRepo => ShapeDisclosure {
            shape,
            summary: "Many mems on git branches, full history — every subcommand works here, \
                      including `memstead install <scope>/<name>`."
                .to_string(),
            cannot: "**It costs a git repository.** The mems live in `mem-repo/.git/` and \
                     every mutation is a commit — not a folder of files you can hand-edit.",
            other_shape: WorkspaceShape::Filesystem,
            other_shape_command: format!(
                "**The other shape** — filesystem-mem: one mem, plain `.md` files, no git — \
                 comes from `{} quickstart` in a fresh folder.",
                memstead_word(),
            ),
        },
    }
}

impl ShapeDisclosure {
    /// The markdown block for a human-facing receipt.
    pub fn lines(&self) -> Vec<String> {
        vec![
            format!("## Workspace shape: {}", self.shape.label()),
            String::new(),
            self.summary.clone(),
            String::new(),
            format!("- {}", self.cannot),
            format!("- {}", self.other_shape_command),
        ]
    }

    /// The same three parts for a `--json` receipt. The agent surface
    /// gets the limit and the recovering command, not just the label.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "shape": self.shape.label(),
            "summary": self.summary.clone(),
            "cannot": self.cannot,
            "other_shape": self.other_shape.label(),
            "other_shape_command": self.other_shape_command,
        })
    }
}

/// Convenience for callers that only render markdown.
pub fn shape_disclosure_lines(shape: WorkspaceShape) -> Vec<String> {
    shape_disclosure(shape).lines()
}

/// [`shape_disclosure_lines`] for a mem that lives in its own folder.
pub fn shape_disclosure_lines_in(shape: WorkspaceShape, mem_folder: Option<&str>) -> Vec<String> {
    shape_disclosure_in(shape, mem_folder).lines()
}

/// Engine instance + the workspace flavour it serves. Subcommands
/// match on the variant to call the right engine API; the read-side
/// store accessor (`engine.store()`) lives on both flavours so simple
/// read commands can share most of their bodies.
///
/// The `MemRepo` variant is only present under the `mem-repo`
/// feature. In the lean build (`--no-default-features`) the enum
/// collapses to a single `Filesystem` arm — every subcommand's
/// dispatch elides the missing arm via `cfg`.
pub enum CliEngine {
    #[cfg(feature = "mem-repo")]
    MemRepo(BaseEngine),
    /// Filesystem-mem flavour, served by the unified [`memstead_base::Engine`].
    Filesystem(BaseEngine),
}

impl CliEngine {
    /// The unified base engine behind whichever flavour booted. Both
    /// variants wrap [`BaseEngine`]; commands that treat the flavours
    /// identically destructure here instead of carrying a per-site
    /// match (which, in the lean build's single-variant enum, is the
    /// `infallible_destructuring_match` shape the isolated lean clippy
    /// leg flags).
    pub fn base(&self) -> &BaseEngine {
        #[cfg(feature = "mem-repo")]
        {
            match self {
                CliEngine::MemRepo(e) => e,
                CliEngine::Filesystem(e) => e,
            }
        }
        #[cfg(not(feature = "mem-repo"))]
        {
            let CliEngine::Filesystem(e) = self;
            e
        }
    }

    /// Mutable twin of [`Self::base`].
    pub fn base_mut(&mut self) -> &mut BaseEngine {
        #[cfg(feature = "mem-repo")]
        {
            match self {
                CliEngine::MemRepo(e) => e,
                CliEngine::Filesystem(e) => e,
            }
        }
        #[cfg(not(feature = "mem-repo"))]
        {
            let CliEngine::Filesystem(e) = self;
            e
        }
    }

    /// Owning twin of [`Self::base`].
    pub fn into_base(self) -> BaseEngine {
        #[cfg(feature = "mem-repo")]
        {
            match self {
                CliEngine::MemRepo(e) => e,
                CliEngine::Filesystem(e) => e,
            }
        }
        #[cfg(not(feature = "mem-repo"))]
        {
            let CliEngine::Filesystem(e) = self;
            e
        }
    }
}

impl CliContext {
    /// Resolve the workspace flavour by walking up from cwd. Returns
    /// `None` when no `.memstead/workspace.toml` is found in any ancestor.
    ///
    /// Post-rebuild the marker is shape-neutral — the same
    /// `.memstead/workspace.toml` carries both folder-only workspaces and
    /// mem-repo workspaces. The flavour tag comes from whether the
    /// workspace root also carries `mem-repo/.git/` (mem-repo
    /// flavour) or not (folder-only flavour). The lean CLI uses this
    /// distinction to surface "this is the lean binary" when the
    /// operator points it at a workspace with git-branch mounts.
    pub fn workspace_shape(&self) -> Option<(WorkspaceShape, PathBuf)> {
        let cwd = std::env::current_dir().ok()?;
        let root = find_workspace_root(&cwd)?;
        Some((WorkspaceShape::at(&root), root))
    }

    /// Build a [`CliEngine`] from the current cwd. The workspace
    /// marker `.memstead/workspace.toml` resolves either flavour; the
    /// presence of `mem-repo/.git/` switches the engine factory.
    ///
    /// On the lean build (`--no-default-features`) the mem-repo
    /// branch surfaces a clear "not built into this binary" error so
    /// a user pointing the lean build at a mem-repo workspace
    /// gets an actionable signal rather than a confusing "no
    /// workspace" bail.
    pub fn cli_engine(&self) -> anyhow::Result<CliEngine> {
        match self.workspace_shape() {
            Some((_, root)) => self.cli_engine_at(&root),
            None => Err(workspace_not_initialised_error(
                "No workspace found. Run from a directory containing `.memstead/workspace.toml` (run `memstead init` for a folder-mount workspace, or `memstead mem-repo init` for a mem-repo workspace).",
            )
            .into()),
        }
    }

    /// [`Self::cli_engine`] with the lazy-mount load scoped to ONE mem:
    /// deferred (lazy, not-yet-loaded) mems other than `mem` stay
    /// unloaded, so a cold command that touches only this mem pays only
    /// its load — the cold-path cut the sizing curve names. Only for
    /// commands whose ENTIRE answer is computable from the named mem's
    /// slice of the store (plus mount metadata): anything that renders
    /// cross-mem state — incoming edges, workspace-wide counts, search
    /// without a mem filter — must use [`Self::cli_engine`], whose
    /// full load keeps every answer computed over a complete store.
    /// Engine mutations need no caller-side scoping either way: each
    /// runs the `reload_if_stale` funnel for its target mem itself, and
    /// the ones whose guards read cross-mem state take the full load
    /// themselves (delete's incoming-refs guards, relate's two
    /// endpoints).
    pub fn cli_engine_scoped(&self, mem: &str) -> anyhow::Result<CliEngine> {
        match self.workspace_shape() {
            Some((_, root)) => {
                let mut engine = self.cli_engine_at_unloaded(&root)?;
                match &mut engine {
                    #[cfg(feature = "mem-repo")]
                    CliEngine::MemRepo(e) => e.ensure_mems_loaded(Some(mem)),
                    CliEngine::Filesystem(e) => e.ensure_mems_loaded(Some(mem)),
                }
                Ok(engine)
            }
            None => Err(workspace_not_initialised_error(
                "No workspace found. Run from a directory containing `.memstead/workspace.toml` (run `memstead init` for a folder-mount workspace, or `memstead mem-repo init` for a mem-repo workspace).",
            )
            .into()),
        }
    }

    /// Build a [`CliEngine`] rooted at an explicit workspace directory,
    /// skipping the cwd walk-up. The flavour is still derived from
    /// whether `<root>/mem-repo/.git/` is present, so callers that
    /// already know the root (e.g. `memstead publish --workspace`) get
    /// the same factory selection as [`Self::cli_engine`]. The split
    /// also gives subcommands a chdir-free, unit-testable engine seam.
    pub fn cli_engine_at(&self, root: &Path) -> anyhow::Result<CliEngine> {
        let mut engine = self.cli_engine_at_unloaded(root)?;
        // Default lazy-mount posture (flywheel W7/01): the CLI loads
        // every deferred mem up front, so a one-shot command behaves
        // byte-identically to the all-eager world — no answer computes
        // over a partial store. Commands whose whole answer lives in one
        // mem opt into [`Self::cli_engine_scoped`] instead.
        match &mut engine {
            #[cfg(feature = "mem-repo")]
            CliEngine::MemRepo(e) => e.ensure_mems_loaded(None),
            CliEngine::Filesystem(e) => e.ensure_mems_loaded(None),
        }
        Ok(engine)
    }

    /// The boot half of [`Self::cli_engine_at`]: flavour detection and
    /// engine construction, with NO deferred-mem load — every caller
    /// decides the load scope explicitly (full for the correct-by-
    /// default path, one mem for the scoped path).
    fn cli_engine_at_unloaded(&self, root: &Path) -> anyhow::Result<CliEngine> {
        if memstead_base::is_mem_repo_shaped(root) {
            #[cfg(feature = "mem-repo")]
            {
                let mut engine =
                    engine_from_workspace_root(root).map_err(|e| boot_error_to_cli(root, e))?;
                engine.set_role(self.role);
                engine.set_identity(self.identity.clone());
                return Ok(CliEngine::MemRepo(engine));
            }
            #[cfg(not(feature = "mem-repo"))]
            {
                return Err(CliError {
                    kind: ExitKind::Generic,
                    code: "UNSUPPORTED_WORKSPACE_SHAPE",
                    message:
                        "this is the lean build of memstead (folder-mount only); the workspace is mem-repo-shaped (`mem-repo/.git/` present). Install the full build (`cargo build --features mem-repo`) or run from a workspace whose mounts are all folder-backed."
                            .to_string(),
                    details: None,
                }
                .into());
            }
        }
        let mut engine =
            BaseEngine::from_workspace_root(root).map_err(|e| boot_error_to_cli(root, e))?;
        engine.set_role(self.role);
        engine.set_identity(self.identity.clone());
        Ok(CliEngine::Filesystem(engine))
    }

    /// Build the unified [`memstead_base::Engine`] for a mem-repo-shaped
    /// workspace. Delegates to `engine_from_workspace_root` which
    /// handles layout detection, mount enumeration, schema resolution,
    /// and readMems hydration in one pass.
    ///
    /// Only compiled into the full build — the lean build never sees a
    /// mem-repo workspace because `cli_engine()` rejects it before
    /// reaching here.
    #[cfg(feature = "mem-repo")]
    pub fn engine(&self) -> anyhow::Result<BaseEngine> {
        let cwd = std::env::current_dir().context("Could not determine current directory")?;

        let Some(root) = find_workspace_root(&cwd) else {
            return Err(workspace_not_initialised_error(
                "No workspace found. Run from a directory containing `.memstead/workspace.toml` (run `memstead mem-repo init` to bootstrap).",
            )
            .into());
        };

        // Subcommands routed through `engine()` (rather than
        // `cli_engine()`) require mem-repo shape — they read /
        // write commit-shaped artefacts (`workspace dump` snapshots,
        // `batch-update` commit envelopes) that have no analogue on a
        // folder-mount-only workspace. Surface the mem-repo-only
        // tag here so callers print an actionable message instead of
        // booting into a foldery engine and erroring later.
        if !memstead_base::is_mem_repo_shaped(&root) {
            return Err(CliError {
                kind: ExitKind::Generic,
                code: "UNSUPPORTED_WORKSPACE_SHAPE",
                message: unsupported_workspace_shape_message(),
                details: None,
            }
            .into());
        }

        let mut engine =
            engine_from_workspace_root(&root).map_err(|e| boot_error_to_cli(&root, e))?;
        engine.set_role(self.role);
        engine.set_identity(self.identity.clone());
        // Same interim lazy-mount posture as `cli_engine_at`.
        engine.ensure_mems_loaded(None);
        Ok(engine)
    }
}

/// Walk upward from `start` looking for the first ancestor that
/// contains `.memstead/workspace.toml` (the post-rebuild workspace
/// marker). Returns the first ancestor directory carrying the marker,
/// or `None` if the walk reaches filesystem root without finding one.
///
/// Both files and directories are accepted as `start`. A plain file's
/// parent is used as the first candidate; for a directory, the
/// directory itself is the first candidate.
///
/// Deeper-marker semantics: because the walk is upward and stops at
/// the first match, an inner workspace nested inside an outer one
/// resolves to the inner.
///
/// Mirrors `memstead-mcp/src/main.rs::find_workspace_root` and the
/// per-command walkers in `memstead-cli/src/commands/link.rs` /
/// `memstead-cli/src/commands/publish.rs`. Keep the resolution rules in
/// sync if any of these change.
pub fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    let mut cursor: PathBuf = if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent()?.to_path_buf()
    };
    loop {
        if memstead_base::is_workspace_root(&cursor) {
            return Some(cursor);
        }
        let parent = cursor.parent()?;
        if parent == cursor {
            return None;
        }
        cursor = parent.to_path_buf();
    }
}

/// Compatibility alias for `find_workspace_root` — kept so existing
/// CLI subcommands (export, changes, …) that historically routed
/// through the lean-flavour walker continue to compile. Both walkers
/// now find the same marker; the alias is intentional for
/// call-site clarity (`find_workspace_root` reads as the canonical
/// surface; `find_filesystem_workspace_root` documents the
/// folder-mount-only intent of its caller).
pub fn find_filesystem_workspace_root(start: &Path) -> Option<PathBuf> {
    find_workspace_root(start)
}

/// Provenance bundle for every CLI-initiated mutation. `Actor::Cli` +
/// `memstead-cli@<CARGO_PKG_VERSION>`. The `Tool:` trailer stays `None`: CLI
/// subcommands aren't MCP tools and the commit subject (`memstead: create …`)
/// already carries the action verb — a second taxonomy would drift.
///
/// Only used by mem-repo write paths today; filesystem-mem write
/// paths assemble their own provenance directly. The function therefore
/// only compiles when `mem-repo` is enabled.
#[cfg(feature = "mem-repo")]
pub fn cli_ctx() -> CommitContext<'static> {
    cli_ctx_with_note(None)
}

/// The `memstead-cli@<version>` client identity stamped into the commit
/// body's `Client:` provenance trailer. Shared by every CLI mutation
/// path so the trailer is uniform across `create` / `update` / `relate`
/// / `rename`. Un-gated (unlike [`cli_ctx_with_note`]) because the
/// `relate` path passes the client to `relate_entity` directly rather
/// than through a `CommitContext`, and that path compiles on both
/// flavours.
pub fn cli_client_id() -> ClientId {
    ClientId {
        name: "memstead-cli".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// Provenance bundle carrying an optional agent-authored `--note`.
/// The note rides into the same payload slot the MCP `note` parameter
/// uses; the engine's `require_notes` policy gate fires `NOTE_MISSING`
/// symmetrically across both surfaces.
#[cfg(feature = "mem-repo")]
pub fn cli_ctx_with_note(note: Option<String>) -> CommitContext<'static> {
    CommitContext {
        actor: Actor::Cli,
        client: Some(cli_client_id()),
        tool: None,
        note,
        role: Default::default(),
        identity: None,
        logical_operation_id: None,
        entity_ids: None,
    }
}

/// Build the unified [`memstead_base::Engine`] for a mem-repo-shaped
/// workspace. Delegates to `engine_from_workspace_root` which
/// handles layout detection, mount enumeration, schema resolution,
/// and readMems hydration in one pass.
///
/// Subcommands routed through this helper require mem-repo shape —
/// they read / write commit-shaped artefacts (`workspace dump`
/// snapshots, `batch-update` commit envelopes) that have no analogue
/// on a folder-mount-only workspace.
#[cfg(feature = "mem-repo")]
pub fn full_engine(_ctx: &CliContext) -> anyhow::Result<BaseEngine> {
    // Typed, not INTERNAL: an unreadable or deleted working directory
    // is an environment condition the caller can act on (`cd` somewhere
    // that exists), and no leaf of a user-triggerable command may
    // collapse into the generic sentinel.
    let cwd = std::env::current_dir().map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            "INTERNAL_IO_ERROR",
            format!("could not determine the current directory ({e}) — run from a directory that exists and is readable"),
        )
    })?;

    let Some(root) = find_workspace_root(&cwd) else {
        return Err(workspace_not_initialised_error(
            "No workspace found. Run from a directory containing `.memstead/workspace.toml` (run `memstead mem-repo init` to bootstrap).",
        )
        .into());
    };

    if !memstead_base::is_mem_repo_shaped(&root) {
        return Err(CliError {
            code: "UNSUPPORTED_WORKSPACE_SHAPE",
            kind: ExitKind::Generic,
            message: unsupported_workspace_shape_message(),
            details: None,
        }
        .into());
    }

    let mut engine = engine_from_workspace_root(&root).map_err(|e| boot_error_to_cli(&root, e))?;
    engine.set_role(_ctx.role);
    engine.set_identity(_ctx.identity.clone());
    // Same CLI lazy-mount posture as `cli_engine_at`/`engine()`: load
    // every deferred mem up front, so no consumer of this seam (`mem
    // list` counts, `recover`, the batch commands, install/uninstall)
    // computes an answer over a partial store. `full_engine` names the
    // FullEngine flavour, not this posture — without this call an
    // unloaded lazy mem rendered as entity count 0 (fifth lazy-mount
    // grade).
    engine.ensure_mems_loaded(None);
    Ok(engine)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn touch_marker(ws: &std::path::Path) {
        std::fs::create_dir_all(ws.join(".memstead")).unwrap();
        std::fs::write(ws.join(".memstead").join("workspace.toml"), "").unwrap();
    }

    #[test]
    fn find_workspace_root_walks_up_to_marker() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        let nested = ws.join("a").join("b").join("specs");
        std::fs::create_dir_all(&nested).unwrap();
        touch_marker(&ws);
        let found =
            find_workspace_root(&nested).expect("walk should find .memstead/workspace.toml");
        assert_eq!(found.canonicalize().unwrap(), ws.canonicalize().unwrap());
    }

    #[test]
    fn find_workspace_root_returns_none_when_absent() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        assert!(find_workspace_root(&nested).is_none());
    }

    #[test]
    fn find_workspace_root_stops_at_containing_dir() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        touch_marker(&ws);
        let found = find_workspace_root(&ws).expect("ws itself carries .memstead/workspace.toml");
        assert_eq!(found, ws);
    }

    #[test]
    fn find_workspace_root_accepts_file_start() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        touch_marker(&ws);
        let file = ws.join("some-file.md");
        std::fs::write(&file, "").unwrap();
        let found = find_workspace_root(&file).expect("file start should resolve to its dir");
        assert_eq!(found, ws);
    }

    #[test]
    fn find_workspace_root_deeper_marker_wins() {
        // Outer and inner each carry `.memstead/workspace.toml`. The walk
        // starts deep inside the inner dir and must resolve to the
        // inner — deeper marker wins because the upward walk stops at
        // the first match.
        let tmp = TempDir::new().unwrap();
        let outer = tmp.path().join("outer");
        let inner = outer.join("inner");
        let deep = inner.join("a").join("b");
        std::fs::create_dir_all(&deep).unwrap();
        touch_marker(&outer);
        touch_marker(&inner);
        let found = find_workspace_root(&deep).expect("walk should find the inner marker");
        assert_eq!(found.canonicalize().unwrap(), inner.canonicalize().unwrap());
    }
}

/// The id a command looks up BEFORE it calls the engine (a hash
/// refetch, a template-identity check, a referrer preview): a bare
/// slug is resolved through the engine's one rule
/// (`Engine::resolve_entity_id`) so the preflight reads the entity the
/// verb will act on, while the verb itself still receives the id the
/// user typed and announces the resolution on its outcome. A full id
/// returns unchanged.
pub fn preflight_id(
    engine: &mut BaseEngine,
    id: &memstead_base::EntityId,
) -> anyhow::Result<memstead_base::EntityId> {
    Ok(engine
        .resolve_entity_id(id)
        .map_err(crate::CliError::from_engine_op)?
        .0)
}
