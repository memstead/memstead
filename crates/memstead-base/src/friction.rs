//! Friction ledger — the engine's record of its own surface's
//! learnability (agent-trust plan 08).
//!
//! Every typed refusal a surface returns is appended as one JSONL line
//! to a workspace-local, gitignored, size-bounded ledger under
//! `.memstead/state/friction/`. The ledger answers "which refusal
//! codes do agents actually hit, on which verbs, how often" as a query
//! instead of an anecdote — the evidence substrate for surface-design
//! changes.
//!
//! ## Hard lines (recorded contract)
//!
//! - **Privacy**: every recorded field's value space is a closed,
//!   engine-defined vocabulary — values that exist as literals in
//!   engine source (`"cli"`/`"mcp"`, the tool/subcommand name, the
//!   UPPER_SNAKE_CASE refusal code, the per-code reason
//!   discriminators in [`closed_reason`]) plus the epoch-seconds
//!   timestamp. Never parameters, entity ids, message text, free-form
//!   strings, or any payload content — a candidate field whose values
//!   a caller can influence is out of bounds no matter how useful.
//!   The write path enforces this by type: reasons enter only as
//!   `&'static str` drawn from the vocabulary table.
//! - **Local only, forever**: the ledger never leaves the machine —
//!   no transmission, no registry involvement. The read surface is
//!   `memstead health --include friction` (and the MCP counterpart).
//! - **Refusals only**: successful operations are never recorded —
//!   this is a friction ledger, not telemetry.
//! - **Best-effort, never perturbing**: recording must not affect the
//!   refusal path. Every ledger I/O error is swallowed; the refusal
//!   returns unchanged whether or not the append landed (an unwritable
//!   state dir degrades to not-recording).
//!
//! ## Concurrency and bound
//!
//! Appends are one `write` syscall of one complete line on an
//! `O_APPEND` handle — concurrent writers (a CLI invocation beside a
//! running MCP server, the normal state of a live workspace) interleave
//! whole lines, never tear them. The bound is rotation: when the
//! current file reaches the cap it is renamed to `<name>.1` (replacing
//! the previous generation), so at most ~2× cap bytes exist on disk. A
//! concurrent rotation race loses at worst the rename (swallowed) —
//! entries keep landing in whichever generation the writer's handle
//! points at, every line still complete.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Rotation threshold for the current generation. At ~80 bytes per
/// entry this holds >6k refusals per generation — months of normal
/// friction — while bounding the ledger to ~1 MiB across both
/// generations.
pub const DEFAULT_CAP_BYTES: u64 = 512 * 1024;

/// Seconds in the "recent" summary window (24 hours).
const RECENT_WINDOW_SECS: u64 = 24 * 60 * 60;

/// One ledger line. Every field's value space follows the module's
/// closed-vocabulary rule (see the privacy hard line above) — that
/// rule, not this struct's current shape, is the contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrictionEntry {
    /// Unix epoch seconds at record time.
    pub ts: u64,
    /// Which surface returned the refusal: `"cli"` or `"mcp"`.
    pub surface: String,
    /// The verb the caller invoked: MCP tool name (`memstead_create`)
    /// or CLI subcommand (`create`).
    pub verb: String,
    /// The typed refusal code (`UNKNOWN_SECTION`, `HASH_MISMATCH`, …).
    pub code: String,
    /// The refusal's closed engine-owned reason discriminator, for
    /// the codes that compute one (see [`closed_reason`]) — absent
    /// otherwise, never an empty string or placeholder. Entries
    /// written before the field existed deserialize as `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The per-code closed reason vocabularies, and the only gate through
/// which a reason reaches the ledger. Given a refusal's `code` and its
/// structured `details`, returns the matching engine-source literal
/// when — and only when — the code has a declared vocabulary AND the
/// details' `reason` value is a member. The return is the vocabulary's
/// own `&'static str`, never the input string, so a caller-influenced
/// value can only ever select from (not extend) the closed set; any
/// other value records nothing.
///
/// Adding a code here requires its `details.reason` to be a closed,
/// engine-defined discriminator (computed at refusal time from engine
/// state, e.g. `SlugError::reason()`) — an open-ended or
/// caller-derived `details.reason` must NOT be listed.
pub fn closed_reason(code: &str, details: Option<&serde_json::Value>) -> Option<&'static str> {
    let vocab: &[&'static str] = match code {
        "INVALID_TITLE" => &["invalid_chars", "control_chars", "id_too_long", "empty"],
        "MEM_PATH_NOT_ALLOWED" => &["no_allowlist_configured", "no_match", "outside_workspace"],
        _ => return None,
    };
    let candidate = details?.get("reason")?.as_str()?;
    vocab.iter().find(|v| **v == candidate).copied()
}

/// Append-side handle. Cheap to construct per refusal — no state
/// beyond the target path and the cap.
#[derive(Debug, Clone)]
pub struct FrictionLedger {
    path: PathBuf,
    cap_bytes: u64,
}

/// The ledger's directory under the workspace store:
/// `<root>/.memstead/state/friction/`.
fn friction_dir(workspace_root: &Path) -> PathBuf {
    workspace_root
        .join(crate::workspace_store::WORKSPACE_STORE_DIR)
        .join("state")
        .join("friction")
}

/// The current-generation ledger file path for a workspace.
pub fn friction_ledger_path(workspace_root: &Path) -> PathBuf {
    friction_dir(workspace_root).join("refusals.jsonl")
}

impl FrictionLedger {
    /// The workspace's ledger with the default cap.
    pub fn for_workspace(workspace_root: &Path) -> Self {
        Self {
            path: friction_ledger_path(workspace_root),
            cap_bytes: DEFAULT_CAP_BYTES,
        }
    }

    /// A ledger at an explicit path with an explicit cap — the test
    /// constructor (the bound assertion drives a tiny cap).
    pub fn at_path(path: PathBuf, cap_bytes: u64) -> Self {
        Self { path, cap_bytes }
    }

    /// Append one refusal. Best-effort by contract: every failure —
    /// unwritable dir, full disk, rename race — is swallowed, and the
    /// caller's refusal path proceeds unchanged. Records only values
    /// from closed engine-defined vocabularies (module hard line);
    /// `reason` is `&'static str` by design — the only way to pass one
    /// is an engine-source literal, normally [`closed_reason`]'s
    /// return. A refusal whose reason cannot be determined records
    /// with `None` rather than not recording.
    pub fn record(&self, surface: &str, verb: &str, code: &str, reason: Option<&'static str>) {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let entry = FrictionEntry {
            ts,
            surface: surface.to_string(),
            verb: verb.to_string(),
            code: code.to_string(),
            reason: reason.map(str::to_string),
        };
        let Ok(mut line) = serde_json::to_vec(&entry) else {
            return;
        };
        line.push(b'\n');

        let Some(dir) = self.path.parent() else {
            return;
        };
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
        // Self-ignoring subtree, same convention as the findings /
        // advance stores: per-checkout engine residue inside a
        // possibly-tracked workspace never surfaces as git noise.
        let gitignore = dir.join(".gitignore");
        if !gitignore.exists() {
            let _ = std::fs::write(&gitignore, "*\n");
        }

        // Size bound: rotate the full current generation aside
        // (replacing the previous one) before appending. A concurrent
        // rotation race loses the rename, which is swallowed — every
        // already-written line survives in one generation or the other.
        if let Ok(meta) = std::fs::metadata(&self.path)
            && meta.len() >= self.cap_bytes
        {
            let _ = std::fs::rename(&self.path, self.rotated_path());
        }

        // One O_APPEND handle, one write_all of one complete line —
        // the whole-line atomicity concurrent writers rely on.
        let Ok(mut file) = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.path)
        else {
            return;
        };
        let _ = file.write_all(&line);
    }

    /// The previous-generation path (`refusals.jsonl.1`).
    fn rotated_path(&self) -> PathBuf {
        let mut name = self
            .path
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_default();
        name.push(".1");
        self.path.with_file_name(name)
    }

    /// Total bytes currently on disk across both generations — the
    /// observable the bound test asserts against.
    pub fn total_bytes(&self) -> u64 {
        let len = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        len(&self.path) + len(&self.rotated_path())
    }

    /// Read every parseable entry across both generations, oldest
    /// generation first. Unparseable lines are skipped (the summary is
    /// tolerant; the concurrency test asserts none exist).
    pub fn entries(&self) -> Vec<FrictionEntry> {
        let mut out = Vec::new();
        for p in [self.rotated_path(), self.path.clone()] {
            if let Ok(content) = std::fs::read_to_string(&p) {
                for l in content.lines() {
                    if let Ok(e) = serde_json::from_str::<FrictionEntry>(l) {
                        out.push(e);
                    }
                }
            }
        }
        out
    }

    /// The `include=["friction"]` health axis: counts per code and per
    /// verb over the whole ledger, plus the same for the recent 24h
    /// window. Shared by the CLI health command and both MCP flavours
    /// so the axis cannot drift between surfaces.
    pub fn summarize(&self) -> serde_json::Value {
        let entries = self.entries();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let cutoff = now.saturating_sub(RECENT_WINDOW_SECS);

        let mut by_code: BTreeMap<String, u64> = BTreeMap::new();
        let mut by_verb: BTreeMap<String, u64> = BTreeMap::new();
        // Per-code reason breakdown — only codes with at least one
        // recorded reason appear; codes without reasons report through
        // `by_code` exactly as before.
        let mut by_reason: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
        let mut recent_by_code: BTreeMap<String, u64> = BTreeMap::new();
        let mut recent_total = 0u64;
        for e in &entries {
            *by_code.entry(e.code.clone()).or_default() += 1;
            *by_verb.entry(format!("{}:{}", e.surface, e.verb)).or_default() += 1;
            if let Some(reason) = &e.reason {
                *by_reason
                    .entry(e.code.clone())
                    .or_default()
                    .entry(reason.clone())
                    .or_default() += 1;
            }
            if e.ts >= cutoff {
                recent_total += 1;
                *recent_by_code.entry(e.code.clone()).or_default() += 1;
            }
        }
        serde_json::json!({
            "total": entries.len(),
            "by_code": by_code,
            "by_verb": by_verb,
            "by_reason": by_reason,
            "recent_24h": {
                "total": recent_total,
                "by_code": recent_by_code,
            },
            "ledger_bytes": self.total_bytes(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn record_appends_and_summarize_counts() {
        let tmp = TempDir::new().unwrap();
        let ledger = FrictionLedger::for_workspace(tmp.path());
        ledger.record("cli", "create", "UNKNOWN_SECTION", None);
        ledger.record("mcp", "memstead_create", "UNKNOWN_SECTION", None);
        ledger.record("cli", "relate", "INVALID_REL_TYPE", None);
        let s = ledger.summarize();
        assert_eq!(s["total"], 3);
        assert_eq!(s["by_code"]["UNKNOWN_SECTION"], 2);
        assert_eq!(s["by_code"]["INVALID_REL_TYPE"], 1);
        assert_eq!(s["by_verb"]["cli:create"], 1);
        assert_eq!(s["by_verb"]["mcp:memstead_create"], 1);
        assert_eq!(s["recent_24h"]["total"], 3);
    }

    /// The size bound holds under a loop of refusals: total on-disk
    /// bytes across both generations never exceed ~2× the cap plus one
    /// entry of slack.
    #[test]
    fn size_bound_holds_under_refusal_loop() {
        let tmp = TempDir::new().unwrap();
        let cap = 2048u64;
        let ledger = FrictionLedger::at_path(tmp.path().join("refusals.jsonl"), cap);
        for i in 0..2000 {
            ledger.record("cli", "create", &format!("CODE_{}", i % 7), None);
        }
        let total = ledger.total_bytes();
        assert!(
            total <= 2 * cap + 256,
            "ledger grew past its bound: {total} bytes (cap {cap})"
        );
        // Rotation kept parseable content — the summary still serves.
        let s = ledger.summarize();
        assert!(s["total"].as_u64().unwrap() > 0);
    }

    /// The self-ignoring `.gitignore` lands beside the ledger.
    #[test]
    fn ledger_dir_is_self_ignoring() {
        let tmp = TempDir::new().unwrap();
        let ledger = FrictionLedger::for_workspace(tmp.path());
        ledger.record("cli", "create", "UNKNOWN_SECTION", None);
        let gitignore = friction_dir(tmp.path()).join(".gitignore");
        assert_eq!(std::fs::read_to_string(gitignore).unwrap(), "*\n");
    }

    /// An unwritable ledger location degrades to not-recording and
    /// never panics or errors — best-effort by contract.
    #[test]
    #[cfg(unix)]
    fn unwritable_dir_degrades_to_not_recording() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let sealed = tmp.path().join("sealed");
        std::fs::create_dir_all(&sealed).unwrap();
        std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o555)).unwrap();
        let ledger = FrictionLedger::at_path(sealed.join("sub").join("refusals.jsonl"), 1024);
        ledger.record("cli", "create", "UNKNOWN_SECTION", None);
        assert_eq!(ledger.entries().len(), 0);
        std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Concurrent writers (the CLI beside a running MCP server)
    /// interleave whole lines, never torn or merged ones: after a
    /// concurrent-append burst every line parses as a complete record
    /// and no entry was lost.
    #[test]
    fn concurrent_appends_never_tear_lines() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("refusals.jsonl");
        let per_thread = 200;
        let threads: Vec<_> = (0..4)
            .map(|t| {
                let ledger = FrictionLedger::at_path(path.clone(), u64::MAX);
                std::thread::spawn(move || {
                    for i in 0..per_thread {
                        ledger.record("mcp", &format!("verb_{t}"), &format!("CODE_{i}"), None);
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let content = std::fs::read_to_string(&path).unwrap();
        let mut parsed = 0;
        for line in content.lines() {
            serde_json::from_str::<FrictionEntry>(line)
                .unwrap_or_else(|e| panic!("torn/merged ledger line: {e}: {line:?}"));
            parsed += 1;
        }
        assert_eq!(parsed, 4 * per_thread, "no entry lost or merged");
    }

    /// A refusal with a closed engine-owned discriminator records it,
    /// and the summary breaks the code's count down by reason —
    /// verified for two distinct codes.
    #[test]
    fn reasons_recorded_and_summarized_for_closed_vocab_codes() {
        let tmp = TempDir::new().unwrap();
        let ledger = FrictionLedger::for_workspace(tmp.path());
        let title_details = serde_json::json!({ "reason": "invalid_chars", "input": "x" });
        let path_details = serde_json::json!({ "reason": "no_match", "candidate": "y" });
        ledger.record(
            "cli",
            "create",
            "INVALID_TITLE",
            closed_reason("INVALID_TITLE", Some(&title_details)),
        );
        ledger.record(
            "cli",
            "create",
            "INVALID_TITLE",
            closed_reason("INVALID_TITLE", Some(&title_details)),
        );
        ledger.record(
            "mcp",
            "memstead_mem_create",
            "MEM_PATH_NOT_ALLOWED",
            closed_reason("MEM_PATH_NOT_ALLOWED", Some(&path_details)),
        );
        ledger.record("cli", "update", "UNKNOWN_SECTION", None);

        let s = ledger.summarize();
        assert_eq!(s["by_reason"]["INVALID_TITLE"]["invalid_chars"], 2);
        assert_eq!(s["by_reason"]["MEM_PATH_NOT_ALLOWED"]["no_match"], 1);
        // A code without recorded reasons reports through by_code only.
        assert!(s["by_reason"].get("UNKNOWN_SECTION").is_none());
        assert_eq!(s["by_code"]["UNKNOWN_SECTION"], 1);
    }

    /// No reason ⇒ no field: the serialized line omits `reason`
    /// entirely — not an empty string, not a placeholder.
    #[test]
    fn entry_without_reason_omits_the_field() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("refusals.jsonl");
        let ledger = FrictionLedger::at_path(path.clone(), u64::MAX);
        ledger.record("cli", "create", "UNKNOWN_SECTION", None);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            !content.contains("reason"),
            "reason key must be absent, got: {content}"
        );
    }

    /// The closed-vocabulary gate: values outside a code's declared
    /// vocabulary — including caller-influenced strings arriving via
    /// `details.reason` — and codes with no vocabulary yield `None`,
    /// so nothing outside engine-source literals can reach the ledger.
    #[test]
    fn closed_reason_rejects_unlisted_values_and_codes() {
        let attacker = serde_json::json!({ "reason": "caller-supplied /etc/passwd" });
        assert_eq!(closed_reason("INVALID_TITLE", Some(&attacker)), None);
        // A code whose details carry an open-ended `reason` string is
        // not in the table — nothing records even though the key exists.
        let open_ended = serde_json::json!({ "reason": "must not carry a version or range" });
        assert_eq!(closed_reason("CONFIG_ERROR", Some(&open_ended)), None);
        assert_eq!(closed_reason("INVALID_TITLE", None), None);
        // Members pass, and the returned value is the vocabulary's own
        // static, usable as `&'static str`.
        let ok = serde_json::json!({ "reason": "id_too_long" });
        let got: Option<&'static str> = closed_reason("INVALID_TITLE", Some(&ok));
        assert_eq!(got, Some("id_too_long"));
    }

    /// Pre-change ledger lines (no `reason` key) parse, count, and
    /// summarize mixed with new-form lines in the same file and across
    /// a rotated generation pair.
    #[test]
    fn pre_change_entries_parse_and_count_across_generations() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("refusals.jsonl");
        // Rotated older generation: pre-change shape only.
        std::fs::write(
            tmp.path().join("refusals.jsonl.1"),
            "{\"ts\":100,\"surface\":\"cli\",\"verb\":\"mem\",\"code\":\"MEM_PATH_NOT_ALLOWED\"}\n",
        )
        .unwrap();
        // Current generation: one pre-change line, then a new-form append.
        std::fs::write(
            &path,
            "{\"ts\":200,\"surface\":\"cli\",\"verb\":\"create\",\"code\":\"INVALID_TITLE\"}\n",
        )
        .unwrap();
        let ledger = FrictionLedger::at_path(path, u64::MAX);
        ledger.record("cli", "create", "INVALID_TITLE", Some("empty"));

        let entries = ledger.entries();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].reason, None);
        assert_eq!(entries[1].reason, None);
        assert_eq!(entries[2].reason.as_deref(), Some("empty"));
        let s = ledger.summarize();
        assert_eq!(s["total"], 3);
        assert_eq!(s["by_code"]["INVALID_TITLE"], 2);
        assert_eq!(s["by_code"]["MEM_PATH_NOT_ALLOWED"], 1);
        assert_eq!(s["by_reason"]["INVALID_TITLE"]["empty"], 1);
    }
}
