//! `memstead check` — record a check of one entity (agent-trust
//! plan 14), or a batch of checks from a file.
//!
//! Mirrors the MCP `memstead_check` tool 1:1. A check is the
//! engine-recorded act of verification: verdict from the closed
//! vocabulary (`ok` | `failed`), optional method note, plan-13
//! provenance (actor, client, the session's `--role`), and the
//! entity's `content_hash` at check time — appended to the
//! workspace's append-only check ledger. Checking mutates nothing:
//! no entity write, no mem commit. Derived check state is served by
//! `memstead entity <id> --provenance`.
//!
//! `--from <file>` records many checks in ONE engine boot — the batch
//! family's contract applies: every entry is validated up front
//! (verdict and kind vocabulary, entity existence) and any invalid
//! entry refuses the WHOLE batch naming every failing entry; nothing
//! is recorded on a refusal. The need is measured, not hypothetical:
//! one campaign run paid 242 engine boots for 242 verdicts.

use clap::Parser;
use memstead_base::EntityId;
use memstead_base::check::{CheckFinding, CheckKind, RecordKind, VERDICTS, Verdict};
use memstead_base::vcs::Actor;
use serde::Deserialize;

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::CliContext;

#[derive(Parser, Debug)]
pub struct Args {
    /// Full entity id (`mem--slug`) of the entity that was checked.
    #[arg(required_unless_present = "from", conflicts_with = "from")]
    pub id: Option<String>,

    /// The verdict: `ok` | `failed`. The vocabulary is closed —
    /// nuance goes in `--method` or in process-mem entities.
    #[arg(long, required_unless_present = "from", conflicts_with = "from")]
    pub verdict: Option<String>,

    /// Free-text method note — how the check was performed. For a
    /// conformance check, name the judging model here.
    #[arg(long, conflicts_with = "from")]
    pub method: Option<String>,

    /// The check kind: `verification` (default — "I checked this
    /// entity's content") | `conformance` (a semantic judgment
    /// against the type's schema prose; the engine stamps the mem's
    /// schema pin into the record, and the verdict goes stale when
    /// the content hash moves OR the pin changes) | `x-<name>` (a
    /// caller-declared kind the engine records verbatim and never
    /// interprets: it stamps no pin, moves no state, and health lists
    /// it by count). Anything else refuses `INVALID_CHECK_KIND`.
    #[arg(long, conflicts_with = "from")]
    pub kind: Option<String>,

    /// A structured finding as JSON: `{"code": "...", "message": "...",
    /// "section"?: "<key>", "evidence"?: "..."}`. Persisted on the
    /// ledger line, echoed on the output, rendered by `health --include
    /// checks` under the entity's latest verdict. `code` is your own
    /// vocabulary; the wrapper shape is fixed and refuses unknown keys
    /// (`INVALID_CHECK_FINDING`).
    #[arg(long, value_name = "JSON", conflicts_with = "from")]
    pub finding: Option<String>,

    /// Record a batch of checks from a JSON file in one engine boot:
    /// `{"checks": [{"id": "...", "verdict": "ok", "method": "...",
    /// "kind": "...", "finding": {...}}, ...]}` — `method`, `kind` and
    /// `finding` optional per entry, mirroring the single form. All-or-nothing: any invalid entry
    /// (unknown verdict or kind, missing entity) refuses the whole
    /// batch and names EVERY failing entry; nothing is recorded.
    #[arg(long, value_name = "PATH")]
    pub from: Option<std::path::PathBuf>,
}

/// The `--from` file payload. `deny_unknown_fields` on both levels so a
/// typo'd key refuses loudly instead of silently dropping data — the
/// batch family's posture.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct BatchPayload {
    checks: Vec<BatchEntry>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct BatchEntry {
    id: String,
    verdict: String,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    /// Validated as a whole, so an unknown key or a missing `code` /
    /// `message` refuses the entry rather than dropping data.
    #[serde(default)]
    finding: Option<serde_json::Value>,
}

/// Parse a wire kind string, `None` input meaning the default
/// verification kind; an `x-<name>` kind is recorded verbatim. Shared by
/// the single and batch forms so the two cannot drift.
fn parse_kind(kind: Option<&str>) -> Result<RecordKind, CliError> {
    match kind {
        None => Ok(RecordKind::Engine(CheckKind::Verification)),
        Some(s) => RecordKind::from_wire(s).ok_or_else(|| {
            CliError::new(
                ExitKind::Validation,
                "INVALID_CHECK_KIND",
                format!(
                    "unknown check kind {s:?} — the vocabulary is: {}",
                    RecordKind::vocabulary_hint()
                ),
            )
        }),
    }
}

/// Parse and validate an optional finding, refusing typed on any shape
/// defect — before anything is appended.
fn parse_finding(raw: Option<serde_json::Value>) -> Result<Option<CheckFinding>, CliError> {
    match raw {
        None => Ok(None),
        Some(v) => CheckFinding::from_json(v).map(Some).map_err(|reason| {
            CliError::new(
                ExitKind::Validation,
                memstead_base::check::INVALID_CHECK_FINDING_CODE,
                reason,
            )
            .with_details(serde_json::json!({ "shape": CheckFinding::SHAPE }))
        }),
    }
}

/// The derived state the response reports: the engine kind's own state,
/// or the verification state for a foreign kind (which moves no state).
fn state_for(
    engine: &memstead_base::Engine,
    id: &EntityId,
    kind: &RecordKind,
) -> Result<memstead_base::check::CheckState, CliError> {
    let (state, _) = match kind.engine_kind() {
        Some(CheckKind::Conformance) => engine.entity_conformance_state(id.mem(), id.as_ref()),
        _ => engine.entity_check_state(id.mem(), id.as_ref()),
    }
    .map_err(CliError::from_engine_op)?;
    Ok(state)
}

fn parse_verdict(verdict: &str) -> Result<Verdict, CliError> {
    Verdict::from_wire(verdict).ok_or_else(|| {
        CliError::new(
            ExitKind::Validation,
            "INVALID_VERDICT",
            format!(
                "unknown verdict {verdict:?} — the vocabulary is: {}",
                VERDICTS.join(", ")
            ),
        )
    })
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    if let Some(path) = &args.from {
        return run_batch(ctx, path);
    }
    // The single form: clap guarantees id + verdict are present when
    // `--from` is absent.
    let id_arg = args
        .id
        .as_deref()
        .expect("clap: id required without --from");
    let verdict_arg = args
        .verdict
        .as_deref()
        .expect("clap: verdict required without --from");
    let verdict = parse_verdict(verdict_arg)?;
    let kind = parse_kind(args.kind.as_deref())?;
    let finding_raw = match args.finding.as_deref() {
        None => None,
        Some(text) => Some(
            serde_json::from_str::<serde_json::Value>(text).map_err(|e| {
                CliError::new(
                    ExitKind::Validation,
                    memstead_base::check::INVALID_CHECK_FINDING_CODE,
                    format!(
                        "--finding is not valid JSON ({e}) — shape {}",
                        CheckFinding::SHAPE
                    ),
                )
            })?,
        ),
    };
    let finding = parse_finding(finding_raw)?;
    let id = EntityId::canonical(id_arg);
    let mut engine = ctx.cli_engine()?.into_base();
    let client = crate::setup::cli_client_id();
    let record = engine
        .record_check_with(
            id.mem(),
            id.as_ref(),
            verdict,
            &kind,
            args.method.as_deref(),
            finding,
            Actor::Cli,
            Some(&client),
        )
        .map_err(CliError::from_engine_op)?;
    let state = state_for(&engine, &id, &kind)?;
    if ctx.json {
        print_json(&serde_json::json!({
            "entity": record.entity,
            "verdict": record.verdict,
            "check_state": state.as_str(),
            "kind": record.kind.as_deref().unwrap_or("verification"),
            "schema_ref": record.schema_ref,
            "role": record.role,
            "identity": record.identity,
            "ts": record.ts,
            "method": record.method,
            "finding": record.finding,
        }))?;
        return Ok(());
    }
    print_markdown(&format!(
        "Check recorded: `{}` — kind `{}`, verdict **{}**, state `{}` (role: {})",
        record.entity,
        record.kind.as_deref().unwrap_or("verification"),
        record.verdict,
        state.as_str(),
        record.role
    ));
    Ok(())
}

/// One validated batch entry: id, verdict, kind, method, finding.
type ParsedEntry = (
    EntityId,
    Verdict,
    RecordKind,
    Option<String>,
    Option<CheckFinding>,
);

/// The `--from` batch: parse, validate EVERY entry, refuse atomically on
/// any failure, then record all entries against one booted engine.
fn run_batch(ctx: &CliContext, path: &std::path::Path) -> anyhow::Result<()> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        CliError::new(
            ExitKind::Generic,
            "INVALID_INPUT",
            format!("cannot read --from file {}: {e}", path.display()),
        )
    })?;
    let payload: BatchPayload = serde_json::from_str(&raw).map_err(|e| {
        CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            format!(
                "--from payload is not the documented shape ({e}); expected \
                 {{\"checks\": [{{\"id\", \"verdict\", \"method\"?, \"kind\"?, \"finding\"?}}, …]}}"
            ),
        )
    })?;
    if payload.checks.is_empty() {
        return Err(CliError::new(
            ExitKind::Validation,
            "INVALID_INPUT",
            "--from payload carries no checks — an empty batch records nothing",
        )
        .into());
    }

    let mut engine = ctx.cli_engine()?.into_base();

    // Validate everything before recording anything — any failure
    // refuses the whole batch, naming every failing entry (the batch
    // family contract).
    let mut parsed: Vec<ParsedEntry> = Vec::new();
    let mut failures: Vec<serde_json::Value> = Vec::new();
    for (i, entry) in payload.checks.iter().enumerate() {
        let id = EntityId::canonical(&entry.id);
        let mut entry_errors: Vec<serde_json::Value> = Vec::new();
        let verdict = match parse_verdict(&entry.verdict) {
            Ok(v) => Some(v),
            Err(e) => {
                entry_errors.push(serde_json::json!({
                    "code": "INVALID_VERDICT",
                    "message": e.to_string(),
                }));
                None
            }
        };
        let kind = match parse_kind(entry.kind.as_deref()) {
            Ok(k) => Some(k),
            Err(e) => {
                entry_errors.push(serde_json::json!({
                    "code": "INVALID_CHECK_KIND",
                    "message": e.to_string(),
                }));
                None
            }
        };
        let finding = match parse_finding(entry.finding.clone()) {
            Ok(f) => Some(f),
            Err(e) => {
                entry_errors.push(serde_json::json!({
                    "code": memstead_base::check::INVALID_CHECK_FINDING_CODE,
                    "message": e.to_string(),
                }));
                None
            }
        };
        let exists = engine
            .store()
            .all_entities()
            .any(|e| !e.stub && e.mem == id.mem() && e.id.0 == *id.as_ref());
        if !exists {
            entry_errors.push(serde_json::json!({
                "code": "ENTITY_NOT_FOUND",
                "message": format!("entity not found: {}", id.as_ref()),
            }));
        }
        if entry_errors.is_empty() {
            parsed.push((
                id,
                verdict.unwrap(),
                kind.unwrap(),
                entry.method.clone(),
                finding.unwrap(),
            ));
        } else {
            failures.push(serde_json::json!({
                "index": i,
                "id": entry.id,
                "errors": entry_errors,
            }));
        }
    }
    if !failures.is_empty() {
        return Err(CliError::new(
            ExitKind::Validation,
            "BATCH_REFUSED",
            format!(
                "batch check REFUSED — {} of {} entr(ies) failed validation, nothing recorded",
                failures.len(),
                payload.checks.len()
            ),
        )
        .with_details(serde_json::json!({ "failed_entries": failures }))
        .into());
    }

    let client = crate::setup::cli_client_id();
    let mut recorded: Vec<serde_json::Value> = Vec::new();
    for (id, verdict, kind, method, finding) in &parsed {
        let record = engine
            .record_check_with(
                id.mem(),
                id.as_ref(),
                *verdict,
                kind,
                method.as_deref(),
                finding.clone(),
                Actor::Cli,
                Some(&client),
            )
            .map_err(CliError::from_engine_op)?;
        recorded.push(serde_json::json!({
            "entity": record.entity,
            "verdict": record.verdict,
            "kind": record.kind.as_deref().unwrap_or("verification"),
            "schema_ref": record.schema_ref,
            "ts": record.ts,
            "finding": record.finding,
        }));
    }

    if ctx.json {
        print_json(&serde_json::json!({
            "recorded": recorded.len(),
            "checks": recorded,
        }))?;
        return Ok(());
    }
    let mut md = format!("# Batch check recorded — {} entr(ies)\n\n", recorded.len());
    for r in &recorded {
        md.push_str(&format!(
            "- ✓ `{}` — {} ({})\n",
            r["entity"].as_str().unwrap_or_default(),
            r["verdict"].as_str().unwrap_or_default(),
            r["kind"].as_str().unwrap_or_default(),
        ));
    }
    print_markdown(&md);
    Ok(())
}
