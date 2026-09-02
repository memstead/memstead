//! Check records — the engine-recorded act of verification
//! (agent-trust plan 14).
//!
//! A check is an agent recording "entity E checked, verdict ok |
//! failed, via method M". It is engine state, never entity content:
//! absent from markdown and `content_hash`, and it produces no mem
//! commit — checking-touches-nothing is what makes check-staleness
//! computable. Records are append-only JSONL under the workspace
//! store (`.memstead/state/checks/checks.jsonl`); a newer check of
//! the same kind supersedes older ones for state derivation but
//! never erases them (kinds derive independently, see [`CheckKind`]).
//!
//! Unlike the friction ledger next door, recording here is NOT
//! best-effort: a check the ledger failed to persist must refuse —
//! the caller believes the act was recorded, and a silently dropped
//! check is exactly the self-report dishonesty this tier exists to
//! end. For the same reason there is no rotation cap: check history
//! is the substrate process state derives from, not disposable
//! telemetry.
//!
//! Each record carries plan-13 provenance (actor, client, declared
//! role) plus the entity's `content_hash` at check time. State
//! derivation compares that hash against the current one:
//!
//! - no record            → `never_checked`
//! - hash matches, ok     → `checked_ok`
//! - hash matches, failed → `check_failed`
//! - hash differs         → `check_stale` (whatever the verdict was,
//!   it no longer speaks to the current content — stated, never
//!   silently carried forward)
//!
//! A `conformance` record additionally carries the mem's schema pin
//! and goes stale when the pin moves ([`derive_state_pinned`]): the
//! prose it judged against is no longer the prose in force.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The closed verdict vocabulary. Nuance goes in the method note or
/// in process-mem entities — never in new verdict values.
pub const VERDICTS: [&str; 2] = ["ok", "failed"];

/// The closed kind vocabulary. `verification` is the default and
/// today's behaviour: "I checked this entity's content". `conformance`
/// is the semantic judgment "this entity satisfies its type's
/// schema prose (`write_rules` / `writing_guidance`)" — recorded with
/// the mem's schema pin, stamped by the engine at record time, so the
/// verdict's freshness against both the content AND the prose version
/// stays computable. A third kind is a separate decision; closed
/// kinds keep health aggregation well-defined, matching the closed
/// verdict vocabulary.
pub const CHECK_KINDS: [&str; 2] = ["verification", "conformance"];

/// Prefix of a caller-declared check kind the engine records verbatim
/// and never interprets: `x-<name>`, `name` lowercase letters, digits
/// and hyphens. The prefix makes the declaration deliberate — a typo of
/// an engine kind cannot silently become a new kind — mirroring the
/// rule that a third ENGINE kind is a separate decision. Foreign kinds
/// never influence `check_state`; health lists them by count.
pub const FOREIGN_KIND_PREFIX: &str = "x-";

/// The typed code for a malformed finding.
pub const INVALID_CHECK_FINDING_CODE: &str = "INVALID_CHECK_FINDING";

/// A check kind from the closed vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckKind {
    Verification,
    Conformance,
}

/// What a caller may declare as a check's kind: one of the engine's
/// two kinds, or a foreign `x-<name>` kind recorded verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordKind {
    Engine(CheckKind),
    Foreign(String),
}

impl RecordKind {
    /// Parse a wire kind: an engine kind, an `x-` kind (name non-empty,
    /// lowercase letters, digits and hyphens), or `None` for anything
    /// else — the vocabulary the refusal names is [`CHECK_KINDS`] plus
    /// the `x-<name>` form.
    pub fn from_wire(s: &str) -> Option<Self> {
        if let Some(k) = CheckKind::from_wire(s) {
            return Some(Self::Engine(k));
        }
        let name = s.strip_prefix(FOREIGN_KIND_PREFIX)?;
        let well_formed = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && !name.starts_with('-')
            && !name.ends_with('-');
        well_formed.then(|| Self::Foreign(s.to_string()))
    }

    /// The engine kind, when this is one.
    pub fn engine_kind(&self) -> Option<CheckKind> {
        match self {
            Self::Engine(k) => Some(*k),
            Self::Foreign(_) => None,
        }
    }

    /// Stable wire form.
    pub fn as_wire(&self) -> &str {
        match self {
            Self::Engine(k) => k.as_str(),
            Self::Foreign(s) => s.as_str(),
        }
    }

    /// The vocabulary sentence a refusal carries.
    pub fn vocabulary_hint() -> String {
        format!(
            "{}, or a caller-declared `{FOREIGN_KIND_PREFIX}<name>` kind (lowercase letters, digits, hyphens) the engine records verbatim and never interprets",
            CHECK_KINDS.join(", ")
        )
    }
}

/// A structured finding riding a check record: WHAT failed (or what was
/// observed) in a locatable form, so a `failed` verdict never forces
/// the author to re-derive the failure from a free-text method note.
/// `code` is the checker's own vocabulary (free for callers); the
/// wrapper shape is fixed and refuses unknown keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckFinding {
    /// The checker's finding code (`hidden-premise`, `stale-source`, …).
    pub code: String,
    /// The section key the finding concerns, when it concerns one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    /// One or two sentences a reader can act on.
    pub message: String,
    /// What the finding rests on: a quote, a coordinate, a reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
}

impl CheckFinding {
    /// The shape sentence every refusal names.
    pub const SHAPE: &'static str =
        "{code: <non-empty>, message: <non-empty>, section?: <key>, evidence?: <text>}";

    /// `code` and `message` are required and non-empty; `section` and
    /// `evidence`, when present, are non-empty too.
    pub fn validate(&self) -> Result<(), String> {
        if self.code.trim().is_empty() {
            return Err(format!(
                "finding.code is required and must be non-empty — shape {}",
                Self::SHAPE
            ));
        }
        if self.message.trim().is_empty() {
            return Err(format!(
                "finding.message is required and must be non-empty — shape {}",
                Self::SHAPE
            ));
        }
        if self.section.as_deref().is_some_and(|s| s.trim().is_empty()) {
            return Err(format!(
                "finding.section, when given, must be non-empty — shape {}",
                Self::SHAPE
            ));
        }
        if self
            .evidence
            .as_deref()
            .is_some_and(|s| s.trim().is_empty())
        {
            return Err(format!(
                "finding.evidence, when given, must be non-empty — shape {}",
                Self::SHAPE
            ));
        }
        Ok(())
    }

    /// Parse and validate a finding from JSON (the CLI's `--finding` and
    /// batch entries, the MCP param). An unknown key, a missing required
    /// key, or an empty value is one typed refusal naming the shape.
    pub fn from_json(value: serde_json::Value) -> Result<Self, String> {
        let finding: CheckFinding = serde_json::from_value(value)
            .map_err(|e| format!("finding does not match the shape {} ({e})", Self::SHAPE))?;
        finding.validate()?;
        Ok(finding)
    }
}

impl CheckKind {
    /// Parse a wire value; `None` for anything outside the vocabulary.
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "verification" => Some(Self::Verification),
            "conformance" => Some(Self::Conformance),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Verification => "verification",
            Self::Conformance => "conformance",
        }
    }
}

/// A check verdict from the closed vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Failed,
}

impl Verdict {
    /// Parse a wire value; `None` for anything outside the vocabulary.
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "ok" => Some(Self::Ok),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
        }
    }
}

/// One recorded check — the full ledger line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckRecord {
    /// Unix epoch seconds at record time.
    pub ts: u64,
    /// Full entity id (`mem--slug`).
    pub entity: String,
    /// `ok` | `failed`.
    pub verdict: String,
    /// Optional free-text method note ("diffed against source spec",
    /// "re-ran the derivation").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// The entity's `content_hash` at check time — the staleness
    /// baseline.
    pub entity_hash: String,
    /// Recorded actor identity (plan-13 provenance).
    pub actor: String,
    /// Recorded client identity (`name@version`), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    /// The caller-declared role, or `"unspecified"` — recorded
    /// honestly; downstream gates treat unspecified as
    /// cannot-confirm, never as any real role.
    pub role: String,
    /// The caller-declared identity (agent-trust plan 15): an opaque
    /// caller-chosen string, the ONLY comparator the independence
    /// gate uses. Absent on ledger lines written before identities
    /// existed and on identity-less callers — both downgrade every
    /// comparison to `unconfirmable`, never to a guessed category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    /// The check kind, from [`CHECK_KINDS`]. Absent on ledger lines
    /// written before kinds existed AND on freshly recorded
    /// `verification` checks — both read as `verification`, so an
    /// existing ledger upgrades with no migration and a kind-omitted
    /// caller's lines stay byte-identical to before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// For `conformance` records: the mem's schema pin
    /// (`name@x.y.z`) as stamped by the engine at record time — never
    /// caller-supplied, so a verdict cannot claim a prose version the
    /// caller never read. Absent on `verification` records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_ref: Option<String>,
    /// The structured finding the checker attached, when one was.
    /// Absent on every line written before findings existed and on
    /// finding-less checks — serde-default, so every existing line
    /// still parses and finding-less lines stay byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finding: Option<CheckFinding>,
}

impl CheckRecord {
    /// The record's ENGINE kind, legacy lines included: an absent or
    /// unrecognised kind reads as `verification`, which is exactly what
    /// every pre-kind line was; a foreign `x-<name>` kind is `None` —
    /// recorded, listed, never aggregated into a state.
    pub fn resolved_kind(&self) -> Option<CheckKind> {
        match self.kind.as_deref() {
            None => Some(CheckKind::Verification),
            Some(k) if k.starts_with(FOREIGN_KIND_PREFIX) => None,
            Some(k) => Some(CheckKind::from_wire(k).unwrap_or(CheckKind::Verification)),
        }
    }

    /// The foreign kind, when the record carries one.
    pub fn foreign_kind(&self) -> Option<&str> {
        self.kind
            .as_deref()
            .filter(|k| k.starts_with(FOREIGN_KIND_PREFIX))
    }
}

/// Derived per-entity check state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    NeverChecked,
    CheckedOk,
    CheckFailed,
    CheckStale,
}

impl CheckState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NeverChecked => "never_checked",
            Self::CheckedOk => "checked_ok",
            Self::CheckFailed => "check_failed",
            Self::CheckStale => "check_stale",
        }
    }
}

/// Derive the state from the newest record (if any) and the entity's
/// current `content_hash`. Hash-only: this is the `verification`
/// derivation, and stays the whole story for that kind — a schema
/// re-pin never stales a verification verdict.
pub fn derive_state(latest: Option<&CheckRecord>, current_hash: &str) -> CheckState {
    derive_state_pinned(latest, current_hash, None)
}

/// Derive the state with schema-pin awareness: beyond the hash
/// comparison, a record that carries a `schema_ref` (a `conformance`
/// record) is stale when the mem's current pin differs from the
/// recorded one — the prose the verdict judged against is no longer
/// the prose in force. A mem that has since lost its pin entirely
/// stales the verdict the same way. Records without a `schema_ref`
/// (every `verification` record) are unaffected by the pin argument.
pub fn derive_state_pinned(
    latest: Option<&CheckRecord>,
    current_hash: &str,
    current_schema_ref: Option<&str>,
) -> CheckState {
    match latest {
        None => CheckState::NeverChecked,
        Some(rec) if rec.entity_hash != current_hash => CheckState::CheckStale,
        Some(rec)
            if rec.schema_ref.is_some() && rec.schema_ref.as_deref() != current_schema_ref =>
        {
            CheckState::CheckStale
        }
        Some(rec) if rec.verdict == "failed" => CheckState::CheckFailed,
        Some(_) => CheckState::CheckedOk,
    }
}

/// The ledger's directory under the workspace store:
/// `<root>/.memstead/state/checks/`.
fn checks_dir(workspace_root: &Path) -> PathBuf {
    workspace_root
        .join(crate::workspace_store::WORKSPACE_STORE_DIR)
        .join("state")
        .join("checks")
}

/// The ledger file path for a workspace.
pub fn check_ledger_path(workspace_root: &Path) -> PathBuf {
    checks_dir(workspace_root).join("checks.jsonl")
}

/// Append/read handle for a workspace's check ledger.
#[derive(Debug, Clone)]
pub struct CheckLedger {
    path: PathBuf,
}

impl CheckLedger {
    pub fn for_workspace(workspace_root: &Path) -> Self {
        Self {
            path: check_ledger_path(workspace_root),
        }
    }

    /// Append one record. One `write` syscall of one complete line on
    /// an `O_APPEND` handle — concurrent writers interleave whole
    /// lines, never tear them. Errors propagate: a check that did not
    /// persist must refuse at the surface.
    pub fn record(&self, rec: &CheckRecord) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut line = serde_json::to_string(rec).map_err(std::io::Error::other)?;
        line.push('\n');
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        f.write_all(line.as_bytes())
    }

    /// All records, oldest first. A missing ledger is an empty one;
    /// unparseable lines are skipped (a torn tail must not poison the
    /// readable history).
    pub fn all(&self) -> Vec<CheckRecord> {
        let Ok(content) = std::fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        content
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// The newest record for one entity, of any kind. State
    /// derivation is per (entity, kind) — use [`Self::latest_for_kind`]
    /// there; this remains the "what happened last" accessor.
    pub fn latest_for(&self, entity: &str) -> Option<CheckRecord> {
        self.all().into_iter().rev().find(|r| r.entity == entity)
    }

    /// The newest record for one entity of one kind. A later check of
    /// the OTHER kind never supersedes it: the two derivations answer
    /// different questions.
    pub fn latest_for_kind(&self, entity: &str, kind: CheckKind) -> Option<CheckRecord> {
        self.all()
            .into_iter()
            .rev()
            .find(|r| r.entity == entity && r.resolved_kind() == Some(kind))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn rec(entity: &str, verdict: &str, hash: &str) -> CheckRecord {
        CheckRecord {
            ts: 1,
            entity: entity.to_string(),
            verdict: verdict.to_string(),
            method: None,
            entity_hash: hash.to_string(),
            actor: "cli".to_string(),
            client: None,
            role: "checker".to_string(),
            identity: None,
            kind: None,
            schema_ref: None,
            finding: None,
        }
    }

    #[test]
    fn state_derivation_covers_all_four_states() {
        assert_eq!(derive_state(None, "h1"), CheckState::NeverChecked);
        let ok = rec("m--e", "ok", "h1");
        assert_eq!(derive_state(Some(&ok), "h1"), CheckState::CheckedOk);
        assert_eq!(derive_state(Some(&ok), "h2"), CheckState::CheckStale);
        let failed = rec("m--e", "failed", "h1");
        assert_eq!(derive_state(Some(&failed), "h1"), CheckState::CheckFailed);
        // A failed check on changed content is stale too — the verdict
        // no longer speaks to current content either way.
        assert_eq!(derive_state(Some(&failed), "h2"), CheckState::CheckStale);
    }

    #[test]
    fn ledger_appends_and_serves_newest_per_entity() {
        let tmp = TempDir::new().unwrap();
        let ledger = CheckLedger::for_workspace(tmp.path());
        assert!(ledger.latest_for("m--a").is_none());
        ledger.record(&rec("m--a", "failed", "h1")).unwrap();
        ledger.record(&rec("m--b", "ok", "h9")).unwrap();
        ledger.record(&rec("m--a", "ok", "h2")).unwrap();
        let latest = ledger.latest_for("m--a").unwrap();
        assert_eq!(latest.verdict, "ok");
        assert_eq!(latest.entity_hash, "h2");
        // Supersession never erases: all three records remain.
        assert_eq!(ledger.all().len(), 3);
    }

    #[test]
    fn verdict_vocabulary_is_closed() {
        assert!(Verdict::from_wire("ok").is_some());
        assert!(Verdict::from_wire("failed").is_some());
        assert!(Verdict::from_wire("passed").is_none());
        assert!(Verdict::from_wire("OK").is_none());
    }

    fn conf(entity: &str, verdict: &str, hash: &str, pin: &str) -> CheckRecord {
        CheckRecord {
            kind: Some("conformance".to_string()),
            schema_ref: Some(pin.to_string()),
            ..rec(entity, verdict, hash)
        }
    }

    #[test]
    fn kind_vocabulary_is_closed() {
        assert!(CheckKind::from_wire("verification").is_some());
        assert!(CheckKind::from_wire("conformance").is_some());
        assert!(CheckKind::from_wire("semantic").is_none());
        assert!(CheckKind::from_wire("Conformance").is_none());
    }

    /// Criterion 5: a pre-kind ledger line (no `kind` field) parses
    /// and derives as a `verification` record, byte-for-byte the old
    /// shape on the write side too.
    #[test]
    fn legacy_lines_read_as_verification() {
        let legacy = r#"{"ts":1,"entity":"m--e","verdict":"ok","entity_hash":"h1","actor":"cli","role":"checker"}"#;
        let parsed: CheckRecord = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.resolved_kind(), Some(CheckKind::Verification));
        // A freshly built verification record serialises with no kind
        // and no schema_ref key at all.
        let fresh = rec("m--e", "ok", "h1");
        let line = serde_json::to_string(&fresh).unwrap();
        assert!(!line.contains("kind"));
        assert!(!line.contains("schema_ref"));
        // An identity-less record carries no identity key either —
        // pre-plan-15 lines and identity-less callers stay
        // byte-identical (agent-trust plan 15, criterion 3).
        assert!(!line.contains("identity"));
    }

    /// Criterion 3: state derives per (entity, kind) — a later check
    /// of the other kind does not supersede.
    #[test]
    fn latest_is_per_kind() {
        let tmp = TempDir::new().unwrap();
        let ledger = CheckLedger::for_workspace(tmp.path());
        ledger.record(&rec("m--a", "ok", "h1")).unwrap();
        ledger
            .record(&conf("m--a", "failed", "h1", "planning@1.0.0"))
            .unwrap();
        let v = ledger
            .latest_for_kind("m--a", CheckKind::Verification)
            .unwrap();
        assert_eq!(v.verdict, "ok");
        let c = ledger
            .latest_for_kind("m--a", CheckKind::Conformance)
            .unwrap();
        assert_eq!(c.verdict, "failed");
        assert_eq!(c.schema_ref.as_deref(), Some("planning@1.0.0"));
    }

    /// Criterion 4: a conformance verdict is stale on a content move
    /// AND on a pin move; a verification verdict ignores pin moves.
    #[test]
    fn conformance_stales_on_pin_move_verification_does_not() {
        let c = conf("m--e", "ok", "h1", "planning@1.0.0");
        assert_eq!(
            derive_state_pinned(Some(&c), "h1", Some("planning@1.0.0")),
            CheckState::CheckedOk
        );
        assert_eq!(
            derive_state_pinned(Some(&c), "h2", Some("planning@1.0.0")),
            CheckState::CheckStale
        );
        assert_eq!(
            derive_state_pinned(Some(&c), "h1", Some("planning@2.0.0")),
            CheckState::CheckStale
        );
        // The mem losing its pin stales the verdict too.
        assert_eq!(
            derive_state_pinned(Some(&c), "h1", None),
            CheckState::CheckStale
        );
        // Verification: unaffected by any pin argument.
        let v = rec("m--e", "ok", "h1");
        assert_eq!(
            derive_state_pinned(Some(&v), "h1", Some("planning@9.0.0")),
            CheckState::CheckedOk
        );
        assert_eq!(derive_state(Some(&v), "h1"), CheckState::CheckedOk);
    }

    // --- findings and open kinds ---

    #[test]
    fn finding_shape_is_fixed_and_validated_whole() {
        let ok = CheckFinding::from_json(serde_json::json!({
            "code": "hidden-premise", "message": "The step assumes X.", "section": "step"
        }))
        .unwrap();
        assert_eq!(ok.code, "hidden-premise");
        assert_eq!(ok.section.as_deref(), Some("step"));
        for bad in [
            serde_json::json!({ "message": "no code" }),
            serde_json::json!({ "code": "x" }),
            serde_json::json!({ "code": "", "message": "empty code" }),
            serde_json::json!({ "code": "x", "message": "   " }),
            serde_json::json!({ "code": "x", "message": "m", "severity": "high" }),
            serde_json::json!({ "code": "x", "message": "m", "section": "" }),
        ] {
            let err = CheckFinding::from_json(bad.clone()).unwrap_err();
            assert!(err.contains("shape"), "{bad}: {err}");
        }
    }

    #[test]
    fn open_kinds_parse_only_with_the_prefix_and_never_resolve_to_an_engine_kind() {
        assert_eq!(
            RecordKind::from_wire("verification"),
            Some(RecordKind::Engine(CheckKind::Verification))
        );
        assert_eq!(
            RecordKind::from_wire("x-step-walk"),
            Some(RecordKind::Foreign("x-step-walk".to_string()))
        );
        for bad in ["step-walk", "x-", "x-Step", "x--a", "x-a-", "X-a"] {
            assert!(RecordKind::from_wire(bad).is_none(), "{bad}");
        }
        assert!(RecordKind::vocabulary_hint().contains("x-<name>"));
        let mut r = rec("m--e", "ok", "h");
        r.kind = Some("x-step-walk".to_string());
        assert_eq!(r.resolved_kind(), None);
        assert_eq!(r.foreign_kind(), Some("x-step-walk"));
        r.kind = None;
        assert_eq!(r.resolved_kind(), Some(CheckKind::Verification));
        r.kind = Some("conformance".to_string());
        assert_eq!(r.resolved_kind(), Some(CheckKind::Conformance));
        // A pre-`x-` unrecognised kind keeps its legacy reading.
        r.kind = Some("mystery".to_string());
        assert_eq!(r.resolved_kind(), Some(CheckKind::Verification));
    }

    #[test]
    fn pre_finding_ledger_lines_parse_and_derive_unchanged_and_findings_round_trip() {
        let tmp = TempDir::new().unwrap();
        let ledger = CheckLedger::for_workspace(tmp.path());
        let dir = check_ledger_path(tmp.path());
        std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
        // A line written before findings existed: no `finding`, no `kind`.
        std::fs::write(
            &dir,
            "{\"ts\":1,\"entity\":\"m--e\",\"verdict\":\"failed\",\"entity_hash\":\"h\",\"actor\":\"cli\",\"role\":\"unspecified\"}\n",
        )
        .unwrap();
        let old = ledger
            .latest_for_kind("m--e", CheckKind::Verification)
            .unwrap();
        assert!(old.finding.is_none());
        assert_eq!(derive_state(Some(&old), "h"), CheckState::CheckFailed);

        let mut with = rec("m--e", "failed", "h");
        with.ts = 2;
        with.finding = Some(CheckFinding {
            code: "hidden-premise".into(),
            section: Some("step".into()),
            message: "The step assumes X.".into(),
            evidence: None,
        });
        ledger.record(&with).unwrap();
        // A foreign-kind line beside it moves no verification state.
        let mut foreign = rec("m--e", "ok", "h");
        foreign.ts = 3;
        foreign.kind = Some("x-step-walk".into());
        ledger.record(&foreign).unwrap();
        let latest = ledger
            .latest_for_kind("m--e", CheckKind::Verification)
            .unwrap();
        assert_eq!(
            latest.ts, 2,
            "the foreign record is not the latest verification record"
        );
        assert_eq!(latest.finding.as_ref().unwrap().code, "hidden-premise");
        assert_eq!(derive_state(Some(&latest), "h"), CheckState::CheckFailed);
        let text = std::fs::read_to_string(&dir).unwrap();
        assert!(text.contains("\"finding\":{\"code\":\"hidden-premise\",\"section\":\"step\",\"message\":\"The step assumes X.\"}"), "{text}");
        assert!(text.contains("\"kind\":\"x-step-walk\""));
        assert_eq!(text.lines().count(), 3, "append-only");
    }
}
