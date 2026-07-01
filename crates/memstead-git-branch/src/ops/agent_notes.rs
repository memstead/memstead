//! `agent_notes_since` — walk a vault's branch from a caller-provided
//! cursor to the current tip and return one [`CommitNote`] per commit
//! along the way, with the body parsed into structured fields.
//!
//! This is the symmetric read-side of [`crate::vcs::format_commit_message`]:
//! the same engine layer that writes the trailer block (`Tool:`, `Actor:`,
//! `Client:`) parses it back out. Plugin and outer-repo cursor consumers
//! that want agent-note bullets read this surface instead of
//! shelling out to `git log` and re-implementing the trailer parser.
//!
//! Subject shapes the engine emits (see callers of `format_commit_message`):
//! - `memstead: <verb> <id>` (entity CRUD: `create`, `update`, `delete`)
//! - `memstead: <verb> <from> → <to>` (`rename`, `relate`, `unrelate`)
//! - `memstead: vault_<verb> <name>` (lifecycle, with optional ` (config)` /
//!   ` (seal)` qualifier)
//!
//! The parser captures the verb token and the remainder of the subject
//! verbatim into `entity_id` — callers that need to split rename/relate
//! arrows do so themselves; the engine does not over-structure here.
//!
//! Empty-tree sentinel (`EMPTY_TREE_SHA`) is accepted as `since` and is
//! treated as "walk every commit reachable from head" — mirrors
//! [`crate::ops::changes::changes_since`]'s convention.

use std::path::Path;

use crate::ops::changes::EMPTY_TREE_SHA;
use crate::vcs::VcsError;

// Data shapes live in `memstead-base`. Re-export here so downstream
// callers that still import `memstead_git_branch::ops::agent_notes::*`
// (and `memstead_git_branch::{CommitNote, ...}` via lib.rs) keep working.
pub use memstead_base::ops::agent_notes::{AgentNotesReport, CommitNote};

/// Resolve `refs/heads/__MEMSTEAD` (unified schemas + per-vault configs)
/// in the vault-repo gitdir shared by every writable vault. Returns
/// `None` when the ref does not exist — pre-migration workspaces and
/// fresh repos legitimately have no `__MEMSTEAD` yet.
pub fn read_memstead_ref(git_dir: &Path) -> Result<Option<String>, VcsError> {
    let repo = gix::open(git_dir)?;
    Ok(repo
        .rev_parse_single("refs/heads/__MEMSTEAD")
        .ok()
        .map(|id| id.to_hex().to_string()))
}

/// Parsed shape of one commit body. Mirrors the layout produced by
/// [`crate::vcs::format_commit_message`]:
///
/// ```text
/// <subject>
///
/// <optional note paragraph>
///
/// Tool: <verb>
/// Actor: <agent|cli|external|unknown>
/// Client: <name>@<version>
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCommit {
    pub subject: String,
    pub tool_verb: Option<String>,
    pub entity_id: Option<String>,
    pub note: Option<String>,
    pub actor: Option<String>,
    pub tool: Option<String>,
    pub client: Option<String>,
    /// Value of the `Logical-Op:` trailer when present. Round-trips
    /// with `memstead_base::vcs::format_commit_message`'s emission.
    pub logical_operation_id: Option<String>,
    /// Ids from the `Entities:` trailer (multi-entity commits, e.g.
    /// `batch_update`). Empty when the trailer is absent — single-entity
    /// commits carry their id in `entity_id` instead. Round-trips with
    /// `format_commit_message`'s `Entities: id1, id2, …` emission.
    pub entity_ids: Vec<String>,
}

/// Parse one commit body — subject on the first line, optional note
/// paragraph, then a trailer block. Trailers are recognised as lines
/// matching `<Capitalized>: <value>`. The note is everything between the
/// subject (after one blank line) and the first trailer (or end of body
/// if none).
///
/// Body input is the raw commit message including subject — the same
/// shape `git log --format=%B` returns. Trailing newlines are tolerated.
///
/// Empty / whitespace-only bodies produce a `ParsedCommit` with an empty
/// subject and every other field `None` — callers decide whether that
/// counts as skippable.
pub fn parse_commit_message(body: &str) -> ParsedCommit {
    let trimmed = body.trim_matches('\n');
    let mut lines = trimmed.split('\n');
    let subject = lines.next().unwrap_or("").trim_end().to_string();

    // Subject parser: `memstead: <verb> <rest>`. The remainder may itself
    // contain spaces (rename arrow, vault qualifier) — we capture it
    // verbatim into `entity_id`. Non-engine subjects (e.g. external drift
    // commits a developer typed by hand) leave both fields `None`.
    let (tool_verb, entity_id) = parse_subject(&subject);

    // Walk the body looking for the first trailer line; everything before
    // it (after stripping leading/trailing blank lines) is the note.
    let body_lines: Vec<&str> = lines.collect();
    let first_trailer_idx = body_lines.iter().position(|l| is_trailer_line(l));

    let note_slice = match first_trailer_idx {
        Some(idx) => &body_lines[..idx],
        None => &body_lines[..],
    };
    let note = collect_note(note_slice);

    let mut tool: Option<String> = None;
    let mut actor: Option<String> = None;
    let mut client: Option<String> = None;
    let mut logical_operation_id: Option<String> = None;
    let mut entity_ids: Vec<String> = Vec::new();
    if let Some(start) = first_trailer_idx {
        for line in &body_lines[start..] {
            if let Some((key, value)) = split_trailer(line) {
                match key {
                    "Tool" if tool.is_none() => tool = Some(value.to_string()),
                    "Actor" if actor.is_none() => actor = Some(value.to_string()),
                    "Client" if client.is_none() => client = Some(value.to_string()),
                    "Logical-Op" if logical_operation_id.is_none() => {
                        logical_operation_id = Some(value.to_string());
                    }
                    "Entities" if entity_ids.is_empty() => {
                        entity_ids = value
                            .split(',')
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                            .collect();
                    }
                    _ => {}
                }
            }
        }
    }

    ParsedCommit {
        subject,
        tool_verb,
        entity_id,
        note,
        actor,
        tool,
        client,
        logical_operation_id,
        entity_ids,
    }
}

fn parse_subject(subject: &str) -> (Option<String>, Option<String>) {
    // Promotion pattern: the engine writes `memstead:` subjects, but old
    // history carries the legacy `mdgv:` spelling — both stay parseable
    // forever so provenance over pre-rename commits keeps working.
    let rest = match subject
        .strip_prefix("memstead:")
        .or_else(|| subject.strip_prefix("mdgv:"))
    {
        Some(r) => r.trim_start(),
        None => return (None, None),
    };
    let mut parts = rest.splitn(2, char::is_whitespace);
    let verb = match parts.next() {
        Some(v) if !v.is_empty() => v.to_string(),
        _ => return (None, None),
    };
    let remainder = parts.next().unwrap_or("").trim().to_string();
    let entity_id = if remainder.is_empty() {
        None
    } else {
        Some(remainder)
    };
    (Some(verb), entity_id)
}

fn is_trailer_line(line: &str) -> bool {
    split_trailer(line).is_some()
}

/// Trailer lines match `^[A-Z][A-Za-z-]+:\s.+$`. Returns the key (without
/// colon) and the value (trimmed) when matched. Mirrors the pattern the
/// plugin uses today (`/^[A-Z][A-Za-z-]+:\s/`).
fn split_trailer(line: &str) -> Option<(&str, &str)> {
    let colon = line.find(':')?;
    let (key, rest) = line.split_at(colon);
    if key.is_empty() {
        return None;
    }
    let mut chars = key.chars();
    let first = chars.next()?;
    if !first.is_ascii_uppercase() {
        return None;
    }
    if !chars.all(|c| c.is_ascii_alphabetic() || c == '-') {
        return None;
    }
    let value_with_colon = &rest[1..];
    let value = value_with_colon.strip_prefix(' ')?.trim_end();
    if value.is_empty() {
        return None;
    }
    Some((key, value))
}

fn collect_note(slice: &[&str]) -> Option<String> {
    // Strip leading blank lines (the `\n\n` separator after subject) and
    // trailing blank lines (the `\n\n` separator before trailers).
    let mut start = 0;
    while start < slice.len() && slice[start].trim().is_empty() {
        start += 1;
    }
    let mut end = slice.len();
    while end > start && slice[end - 1].trim().is_empty() {
        end -= 1;
    }
    if start == end {
        return None;
    }
    let joined = slice[start..end].join("\n");
    let trimmed = joined.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Walk the per-vault branch from `since` (exclusive) to the current
/// branch tip (inclusive) and return one [`CommitNote`] per commit on
/// the path, parsed via [`parse_commit_message`]. Order: newest first
/// (matches `git log` default).
///
/// `since` may be the canonical empty-tree SHA — in which case every
/// reachable commit is returned. Empty repos / empty refs return an
/// empty report with `head` echoing the empty-tree sentinel.
///
/// `head_ref` follows the same convention as
/// [`crate::ops::changes::changes_since`]: pass `Some("refs/heads/<vault>")`
/// for vault-repo-backed vaults, `None` to fall back to the gix HEAD.
pub fn agent_notes_since(
    vault_name: &str,
    git_dir: &Path,
    since: &str,
    head_ref: Option<&str>,
) -> Result<AgentNotesReport, VcsError> {
    let repo = gix::open(git_dir)?;

    let head_lookup: Result<gix::Commit<'_>, ()> = match head_ref {
        Some(ref_name) => repo
            .rev_parse_single(ref_name)
            .ok()
            .and_then(|id| id.object().ok())
            .and_then(|obj| obj.try_into_commit().ok())
            .ok_or(()),
        None => repo.head_commit().map_err(|_| ()),
    };
    let head_commit = match head_lookup {
        Ok(c) => c,
        Err(()) => {
            let memstead_ref = read_memstead_ref(git_dir)?;
            return Ok(AgentNotesReport {
                vault: vault_name.to_string(),
                since: since.to_string(),
                head: EMPTY_TREE_SHA.to_string(),
                notes: Vec::new(),
                memstead_ref,
            });
        }
    };
    let head_sha = head_commit.id.to_hex().to_string();

    // Resolve `since` to an ObjectId for the walker's `with_hidden`
    // boundary. The empty-tree sentinel means "no boundary" — walk every
    // reachable commit. Unknown / unreachable since refs surface as
    // `ObjectNotFound` exactly like `changes_since`.
    let hidden: Vec<gix::ObjectId> = if since == EMPTY_TREE_SHA {
        Vec::new()
    } else {
        let id = repo
            .rev_parse_single(since)
            .map_err(|e| VcsError::ObjectNotFound(format!("{since}: {e}")))?;
        // Resolve through to a commit so unreachable refs surface here
        // rather than at walk time.
        let object = id
            .object()
            .map_err(|e| VcsError::ObjectNotFound(format!("{since}: {e}")))?;
        object
            .try_into_commit()
            .map_err(|_| VcsError::ObjectNotFound(format!("{since} is not a commit")))?;
        vec![id.detach()]
    };

    let walk = repo
        .rev_walk([head_commit.id])
        .with_hidden(hidden)
        .all()
        .map_err(|e| VcsError::Git(format!("rev-walk: {e}")))?;

    let mut notes: Vec<CommitNote> = Vec::new();
    for info in walk {
        let info = info.map_err(|e| VcsError::Git(format!("rev-walk-step: {e}")))?;
        let commit = info
            .object()
            .map_err(|e| VcsError::Git(format!("commit-load: {e}")))?;
        let sha = commit.id.to_hex().to_string();
        let timestamp = commit
            .time()
            .map(|t| t.seconds)
            .unwrap_or(0);

        // gix exposes the message via `decode()`. The body is the raw
        // bytes including subject + body — exactly what `format_commit_message`
        // wrote.
        let body_string = match commit.message_raw() {
            Ok(bstr) => std::str::from_utf8(bstr.as_ref())
                .map(|s| s.to_string())
                .unwrap_or_default(),
            Err(_) => String::new(),
        };

        let parsed = parse_commit_message(&body_string);
        notes.push(CommitNote {
            vault: vault_name.to_string(),
            sha,
            subject: parsed.subject,
            tool_verb: parsed.tool_verb,
            entity_id: parsed.entity_id,
            note: parsed.note,
            actor: parsed.actor,
            tool: parsed.tool,
            client: parsed.client,
            logical_operation_id: parsed.logical_operation_id,
            entity_ids: parsed.entity_ids,
            timestamp,
        });
    }

    let memstead_ref = read_memstead_ref(git_dir)?;

    Ok(AgentNotesReport {
        vault: vault_name.to_string(),
        since: since.to_string(),
        head: head_sha,
        notes,
        memstead_ref,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> crate::vcs::CommitContext<'static> {
        crate::vcs::CommitContext {
            actor: crate::vcs::Actor::Agent,
            client: Some(crate::vcs::ClientId {
                name: "claude-code".into(),
                version: "2.1.0".into(),
            }),
            tool: Some("memstead_create"),
            note: Some("Demoting drift hook to engine surface.".into()),
            logical_operation_id: None,
            entity_ids: None,
        }
    }

    #[test]
    fn parser_round_trips_logical_operation_id_trailer() {
        // Multi-vault rename commits carry a `Logical-Op:` trailer so
        // consumers reading the commit log can correlate every
        // per-vault commit a single rename produced. The folder
        // backend's JSONL writer carries the same id under its
        // `"logical_op"` field — both paths must round-trip back to
        // `Provenance.logical_operation_id`.
        let ctx = crate::vcs::CommitContext {
            actor: crate::vcs::Actor::Agent,
            client: Some(crate::vcs::ClientId {
                name: "claude-code".into(),
                version: "2.1.0".into(),
            }),
            tool: Some("rename_entity"),
            note: None,
            logical_operation_id: Some("logop-abc123def456"),
            entity_ids: None,
        };
        let raw = crate::vcs::format_commit_message("memstead: rename a → b", &ctx);
        assert!(
            raw.contains("Logical-Op: logop-abc123def456"),
            "format_commit_message must emit the Logical-Op trailer; got:\n{raw}"
        );
        let parsed = parse_commit_message(&raw);
        assert_eq!(
            parsed.logical_operation_id.as_deref(),
            Some("logop-abc123def456"),
            "parser must reconstruct the Logical-Op trailer; got: {:?}",
            parsed.logical_operation_id
        );
    }

    #[test]
    fn parser_round_trips_entities_trailer() {
        // batch_update collapses its subject to `(N entities)`; the
        // `Entities:` trailer carries the real ids so an --include-notes
        // reader can name them. format → parse must round-trip.
        let ctx = crate::vcs::CommitContext {
            actor: crate::vcs::Actor::Cli,
            client: None,
            tool: Some("batch_update"),
            note: None,
            logical_operation_id: None,
            entity_ids: Some(vec![
                "specs--alpha".to_string(),
                "specs--beta".to_string(),
                "memos--gamma".to_string(),
            ]),
        };
        let raw = crate::vcs::format_commit_message("memstead: batch-update (3 entities)", &ctx);
        assert!(
            raw.contains("Entities: specs--alpha, specs--beta, memos--gamma"),
            "format_commit_message must emit the Entities trailer; got:\n{raw}"
        );
        let parsed = parse_commit_message(&raw);
        // The subject (and thus entity_id) keeps its count-string shape.
        assert_eq!(parsed.entity_id.as_deref(), Some("(3 entities)"));
        // The ids are recovered additively.
        assert_eq!(
            parsed.entity_ids,
            vec!["specs--alpha", "specs--beta", "memos--gamma"],
            "parser must reconstruct the Entities trailer; got: {:?}",
            parsed.entity_ids
        );
    }

    #[test]
    fn parser_leaves_entity_ids_empty_without_trailer() {
        // A single-entity commit names its id in the subject — no
        // Entities trailer, so entity_ids stays empty.
        let raw = crate::vcs::format_commit_message("memstead: update specs--solo", &ctx());
        let parsed = parse_commit_message(&raw);
        assert!(parsed.entity_ids.is_empty(), "no trailer → empty: {:?}", parsed.entity_ids);
        assert_eq!(parsed.entity_id.as_deref(), Some("specs--solo"));
    }

    #[test]
    fn parser_round_trips_format_commit_message_with_note() {
        let ctx = ctx();
        let raw = crate::vcs::format_commit_message("memstead: create specs--demo", &ctx);
        let parsed = parse_commit_message(&raw);
        assert_eq!(parsed.subject, "memstead: create specs--demo");
        assert_eq!(parsed.tool_verb.as_deref(), Some("create"));
        assert_eq!(parsed.entity_id.as_deref(), Some("specs--demo"));
        assert_eq!(parsed.tool.as_deref(), Some("memstead_create"));
        assert_eq!(parsed.actor.as_deref(), Some("agent"));
        assert_eq!(parsed.client.as_deref(), Some("claude-code@2.1.0"));
        assert_eq!(
            parsed.note.as_deref(),
            Some("Demoting drift hook to engine surface.")
        );
    }

    #[test]
    fn parser_round_trips_format_commit_message_without_note() {
        let ctx = crate::vcs::CommitContext {
            actor: crate::vcs::Actor::External,
            client: None,
            tool: None,
            note: None,
            logical_operation_id: None,
            entity_ids: None,
        };
        let raw = crate::vcs::format_commit_message("memstead: rename a → b", &ctx);
        let parsed = parse_commit_message(&raw);
        assert_eq!(parsed.subject, "memstead: rename a → b");
        assert_eq!(parsed.tool_verb.as_deref(), Some("rename"));
        assert_eq!(parsed.entity_id.as_deref(), Some("a → b"));
        assert_eq!(parsed.actor.as_deref(), Some("external"));
        assert!(parsed.note.is_none());
        assert!(parsed.tool.is_none());
        assert!(parsed.client.is_none());
    }

    #[test]
    fn parser_accepts_legacy_memstead_subject_prefix() {
        // Pre-rename history carries `mdgv:` subjects — the parser must
        // keep accepting them forever (promotion pattern, read-tolerated).
        let parsed = parse_commit_message("mdgv: update specs--legacy\n");
        assert_eq!(parsed.subject, "mdgv: update specs--legacy");
        assert_eq!(parsed.tool_verb.as_deref(), Some("update"));
        assert_eq!(parsed.entity_id.as_deref(), Some("specs--legacy"));
    }

    #[test]
    fn parser_handles_unrecognized_subject() {
        let parsed = parse_commit_message("hand-typed external drift\n");
        assert_eq!(parsed.subject, "hand-typed external drift");
        assert!(parsed.tool_verb.is_none());
        assert!(parsed.entity_id.is_none());
        assert!(parsed.actor.is_none());
    }

    #[test]
    fn parser_recognises_multiline_note() {
        let body = "\
memstead: update specs--alpha

First line of the note.
Second line of the note.

Tool: memstead_update
Actor: agent
Client: claude-code@2.1.0
";
        let parsed = parse_commit_message(body);
        assert_eq!(
            parsed.note.as_deref(),
            Some("First line of the note.\nSecond line of the note.")
        );
        assert_eq!(parsed.actor.as_deref(), Some("agent"));
    }

    #[test]
    fn parser_tolerates_subject_only_body() {
        let parsed = parse_commit_message("memstead: update specs--alpha\n");
        assert_eq!(parsed.subject, "memstead: update specs--alpha");
        assert_eq!(parsed.tool_verb.as_deref(), Some("update"));
        assert_eq!(parsed.entity_id.as_deref(), Some("specs--alpha"));
        assert!(parsed.note.is_none());
        assert!(parsed.actor.is_none());
    }

    #[test]
    fn parser_treats_lowercase_keys_as_body() {
        // Plugin convention writes `Mdgv-cursor:` (passes; capital M).
        // A line starting `tool:` (lowercase) must not be confused for a
        // trailer — keeps the parser robust against prose that happens to
        // contain colon-bearing lines.
        let body = "\
memstead: update specs--alpha

note line one
tool: this is prose, not a trailer

Actor: agent
";
        let parsed = parse_commit_message(body);
        assert_eq!(
            parsed.note.as_deref(),
            Some("note line one\ntool: this is prose, not a trailer")
        );
        assert_eq!(parsed.actor.as_deref(), Some("agent"));
        assert!(parsed.tool.is_none());
    }

    #[test]
    fn parser_skips_trailer_without_value() {
        // `Foo:` with nothing after it is not a valid trailer — keeps
        // `Foo:` from accidentally splitting a note.
        assert!(!is_trailer_line("Foo:"));
        assert!(!is_trailer_line("Foo: "));
        assert!(is_trailer_line("Foo: bar"));
    }

    // The gix-walking integration is exercised through the engine-level
    // tests once the Engine wrapper lands. Walking-without-a-repo here
    // would just re-test gix; the parser tests above cover the trailer
    // contract that is the engine's actual ownership boundary.
}
