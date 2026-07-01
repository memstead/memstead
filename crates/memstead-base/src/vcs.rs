//! Backend-agnostic VCS provenance types and trailer-block helpers.
//!
//! The engine's git-bound bits — the [`Vcs`] trait, gix-using
//! repository helpers, [`VcsError`] and its `From<gix::*>` conversions
//! — live in `memstead_git_branch::vcs`. What stays
//! here is the data model that travels through every commit (caller
//! actor, client identity, optional tool name and provenance note) and
//! the deterministic helpers that turn that data into the author
//! signature and trailer block. Both adapters (legacy disk + git-tree)
//! call the helpers so two paths produce byte-identical commit
//! messages for the same logical input.
//!
//! [`Vcs`]: ../../memstead_git_branch/vcs/trait.Vcs.html
//! [`VcsError`]: ../../memstead_git_branch/vcs/enum.VcsError.html

/// Generic email domain for derived author addresses. No PII: the
/// local-part is a sanitised client name (or `external`), never a user.
const PROVENANCE_EMAIL_DOMAIN: &str = "memstead.io";

/// Caller categories for the `Actor:` trailer and for picking an author
/// signature. `Agent` and `Cli` get their author from the paired
/// `ClientId` when one is present; `External` always uses the synthetic
/// `external <external@memstead.io>` identity (no client is known); `Unknown`
/// falls back to the committer identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    Agent,
    Cli,
    External,
    Unknown,
}

impl Actor {
    /// String form used for the `Actor:` trailer. Stable; downstream LLMs
    /// grep on these values.
    pub fn as_trailer(&self) -> &'static str {
        match self {
            Actor::Agent => "agent",
            Actor::Cli => "cli",
            Actor::External => "external",
            Actor::Unknown => "unknown",
        }
    }

    /// Inverse of [`Self::as_trailer`]. Returns `None` for any string
    /// outside the four canonical wire forms — readers that may
    /// encounter older or malformed values choose how to handle the
    /// absence (default to [`Actor::Unknown`], surface a warning, …).
    pub fn from_trailer(s: &str) -> Option<Self> {
        match s {
            "agent" => Some(Actor::Agent),
            "cli" => Some(Actor::Cli),
            "external" => Some(Actor::External),
            "unknown" => Some(Actor::Unknown),
            _ => None,
        }
    }
}

/// Identity of the process speaking to the engine. For MCP, this is the
/// `clientInfo` from the initialize handshake (e.g.
/// `ClientId { name: "claude-code", version: "2.1.0" }`). For CLI-direct
/// mutations, the crate populates it with its own name and version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientId {
    pub name: String,
    pub version: String,
}

/// Provenance bundle for a single commit. Produced at the caller boundary
/// (`memstead-mcp` tool handler, `memstead-cli` subcommand, engine-internal drift
/// flush) and threaded through to the VCS commit path.
#[derive(Debug, Clone)]
pub struct CommitContext<'a> {
    pub actor: Actor,
    pub client: Option<ClientId>,
    /// Name of the MCP tool that initiated the commit (e.g.
    /// `"memstead_update"`). Present for MCP-sourced commits; CLI-direct and
    /// external-drift commits leave this `None`.
    pub tool: Option<&'a str>,
    /// Agent-authored one-sentence provenance note. When present and
    /// non-empty it lands in the commit body between the caller's prose
    /// and the `Tool:/Actor:/Client:` trailer block. Whitespace-only
    /// values are treated as absent. The MCP layer validates length
    /// (`NOTE_MAX_LEN`, 280 chars) before the mutation touches disk;
    /// callers must not feed unbounded input to this field.
    pub note: Option<String>,
    /// Correlation id linking every commit produced by a single
    /// logical operation (notably multi-vault `memstead_rename`). When
    /// `Some`, [`format_commit_message`] emits a `Logical-Op: <id>`
    /// trailer alongside `Tool:` / `Actor:` / `Client:`. The git-
    /// branch backend's `parse_commit_message` recovers the value
    /// from the trailer block so `read_provenance` reconstructs
    /// `Provenance::logical_operation_id` round-trip-clean. `None`
    /// for legacy or single-call mutations that don't participate
    /// in correlation; consumers branch on whether the id recurs to
    /// identify a multi-commit logical operation.
    pub logical_operation_id: Option<&'a str>,
    /// Entity ids this commit touched, when one commit covers more than
    /// one entity (notably `batch_update`, whose subject collapses to
    /// `(N entities)`). When `Some` and non-empty, [`format_commit_message`]
    /// emits an `Entities: id1, id2, …` trailer that `parse_commit_message`
    /// recovers into `CommitNote::entity_ids`, so an `--include-notes`
    /// consumer can name every entity a batch changed from the note record
    /// alone. `None`/empty for single-entity commits — those name their
    /// one id in the subject (and thus `entity_id`), so no list is needed.
    pub entity_ids: Option<Vec<String>>,
}

impl<'a> CommitContext<'a> {
    /// Author-neutral context: no actor, no client, no tool. The author
    /// signature falls back to the committer identity — preserving the
    /// pre-provenance behaviour. Used by engine tests and by call sites
    /// that have not yet been taught to build a real context.
    pub fn internal() -> Self {
        Self {
            actor: Actor::Unknown,
            client: None,
            tool: None,
            note: None,
            logical_operation_id: None,
            entity_ids: None,
        }
    }
}

/// Inverse of the `name@version` rendering used in both the commit
/// trailer block (`Client: <name>@<version>`) and the folder-backend
/// JSONL changelog (`"client": "<name>@<version>"`). Splits on the
/// **last** `@` because client names may legitimately contain `.`
/// and `-`; versions never contain `@`. Returns `None` for malformed
/// input (no `@`, empty name, empty version) so tolerant readers
/// drop the field rather than constructing a half-record.
pub fn parse_client_id(s: &str) -> Option<ClientId> {
    let (name, version) = s.rsplit_once('@')?;
    if name.is_empty() || version.is_empty() {
        return None;
    }
    Some(ClientId {
        name: name.to_string(),
        version: version.to_string(),
    })
}

/// Sanitise a raw client name to a git-safe local-part matching
/// `[a-z0-9._-]+`. Empty/whitespace-only input falls back to `"unknown"`.
///
/// - Lowercase ASCII.
/// - Anything outside `[a-z0-9._-]` becomes `-` (spaces, `/`, `@`, …).
/// - Non-ASCII bytes also collapse to `-` rather than being dropped, so
///   the output length still tracks the input coarsely (useful for
///   debugging a garbled clientInfo).
pub fn sanitise_client_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() || matches!(lower, '.' | '_' | '-') {
            out.push(lower);
        } else {
            out.push('-');
        }
    }
    if out.chars().all(|c| c == '-' || c.is_whitespace()) {
        return "unknown".to_string();
    }
    out
}

/// Build the per-commit author `(name, email)` pair from the context.
/// `None` means "fall back to the committer identity" — adapters then
/// reuse the committer signature for the author slot.
///
/// Public so both the legacy disk adapter and the git-tree adapter can
/// build byte-identical commit objects without re-implementing the
/// trailer + author convention.
pub fn author_identity(ctx: &CommitContext<'_>) -> Option<(String, String)> {
    match (ctx.actor, ctx.client.as_ref()) {
        (Actor::Agent | Actor::Cli, Some(c)) => {
            let local = sanitise_client_name(&c.name);
            let email = format!("{local}@{PROVENANCE_EMAIL_DOMAIN}");
            Some((local, email))
        }
        (Actor::External, _) => Some((
            "external".to_string(),
            format!("external@{PROVENANCE_EMAIL_DOMAIN}"),
        )),
        // Agent/Cli without a ClientId, or Unknown: no derived identity;
        // caller falls back to the committer signature.
        _ => None,
    }
}

/// Append the trailer block to the caller's prose, separated by exactly
/// one blank line. Normalises trailing newlines so `"subject"` and
/// `"subject\n"` both produce `"subject\n\nActor: …\n…"`.
///
/// When `ctx.note` carries a non-blank string, it is inserted between the
/// prose and the trailer block — with exactly one blank line on each
/// side. Whitespace-only notes are treated as absent (callers that want
/// an empty note must pass `None`). The final layout is:
///
/// ```text
/// <prose>
///
/// <note, if present>
///
/// <trailer block>
/// ```
///
/// `Actor:` is always emitted. `Tool:` is emitted when `ctx.tool` is set.
/// `Client:` is emitted when `ctx.client` is set. Order: `Tool`, `Actor`,
/// `Client`.
///
/// Public so both adapters share the same trailer block — the two paths
/// must produce byte-identical commit messages for the same logical
/// input.
pub fn format_commit_message(prose: &str, ctx: &CommitContext<'_>) -> String {
    let trimmed = prose.trim_end_matches('\n');
    let mut trailers: Vec<String> = Vec::with_capacity(4);
    if let Some(tool) = ctx.tool {
        trailers.push(format!("Tool: {tool}"));
    }
    trailers.push(format!("Actor: {}", ctx.actor.as_trailer()));
    if let Some(c) = ctx.client.as_ref() {
        trailers.push(format!("Client: {}@{}", c.name, c.version));
    }
    // `Logical-Op:` is the wire-stable trailer key. Recognised by
    // `parse_commit_message` and threaded back into
    // `Provenance::logical_operation_id` so the multi-vault rename
    // correlation survives a commit-log round-trip through the
    // git-branch backend.
    if let Some(id) = ctx.logical_operation_id {
        trailers.push(format!("Logical-Op: {id}"));
    }
    // `Entities:` lists every id a multi-entity commit touched (batch
    // update), comma-separated. Recovered by `parse_commit_message` into
    // `CommitNote::entity_ids` so a note read in isolation names the
    // entities even though the subject only says `(N entities)`. Omitted
    // when absent or empty — single-entity commits carry their id in the
    // subject. Ids never contain `, ` so the join is unambiguous.
    if let Some(ids) = ctx.entity_ids.as_ref().filter(|v| !v.is_empty()) {
        trailers.push(format!("Entities: {}", ids.join(", ")));
    }
    let note_body = ctx
        .note
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty());
    match note_body {
        Some(note) => format!("{trimmed}\n\n{note}\n\n{}", trailers.join("\n")),
        None => format!("{trimmed}\n\n{}", trailers.join("\n")),
    }
}
