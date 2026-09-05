//! The strict axis: `memstead health --strict` as the graph's referee.
//!
//! Strict evaluates one fixed set of health includes (the referee's set,
//! [`STRICT_INCLUDES`]) and reads three things off the composed report:
//!
//! * **Entity findings**, each a health condition on one entity
//!   ([`HEALTH_CONDITIONS`]): the three dangling conditions, an
//!   unresolved stub, an ungranted cross-mem edge, a missing required
//!   outgoing edge, an unsatisfied constraint, a section-format violation,
//!   a warn-level signal. Each may be **acknowledged**: a check record on
//!   that entity whose `finding.code` names the condition, with verdict
//!   `failed` and a method naming the owner and the plan that closes it.
//!   An acknowledged finding is reported and does not fail the run.
//! * **Configuration defects**, workspace- or mem-level and never
//!   acknowledgeable: a schema pin that disagrees with its mem, a rotted
//!   or drifted schema package, a mount that resolves to nothing, an
//!   unreadable anchors sidecar, a schema format defect.
//! * **Stale acknowledgements** ([`STALE_ACKNOWLEDGEMENT_CODE`]): an
//!   acknowledgement that still stands while its finding no longer occurs.
//!   That is a finding in its own right, so the acknowledged set shrinks
//!   with the repairs instead of rotting into an allowlist.
//!
//! The run fails on any unacknowledged finding, any configuration defect
//! and any stale acknowledgement. Stale entities, drifted anchors and
//! generation hints stay advisory: they are on the report, never on the
//! verdict.
//!
//! The finding-code namespace: a code spelled `UPPER_SNAKE` claims to
//! name an engine condition, so [`crate::Engine::record_check_with`]
//! refuses one that is not in [`HEALTH_CONDITIONS`]; a checker's own
//! vocabulary keeps any other spelling (`hidden-premise`) and never
//! acknowledges anything.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::check::{CheckLedger, CheckRecord};

/// The includes strict evaluates, whatever `--include` says.
pub const STRICT_INCLUDES: &[&str] = &[
    "integrity",
    "anchors",
    "stale",
    "missing_required_outgoing",
    "constraints",
    "signals",
];

/// The code of a standing acknowledgement whose finding stopped occurring.
pub const STALE_ACKNOWLEDGEMENT_CODE: &str = "STALE_ACKNOWLEDGEMENT";

/// The condition codes an entity finding carries, and the only codes an
/// acknowledgement may name. The dangling conditions are the same three
/// [`crate::ops::DanglingLinkKind::ALL_CODES`] lists.
pub const HEALTH_CONDITIONS: &[&str] = &[
    "DANGLING_LINK_TARGET_MISSING",
    "DANGLING_LINK_NOT_RELATED",
    "DANGLING_RELATION_TARGET_MISSING",
    "UNRESOLVED_STUB",
    "CROSS_MEM_EDGE_UNGRANTED",
    "MISSING_REQUIRED_OUTGOING",
    "CONSTRAINT_UNSATISFIED",
    "SECTION_FORMAT_VIOLATION",
    "SIGNAL_WARN",
];

/// Configuration defects strict refuses on; never acknowledgeable because
/// none of them belongs to one entity.
pub const CONFIGURATION_CONDITIONS: &[&str] = &[
    "SCHEMA_AUTHORING_SOURCE_MISSING",
    "SCHEMA_AUTHORING_SOURCE_DIVERGED",
    "SCHEMA_PIN_MISMATCH",
    "SCHEMA_UNSTAMPED_SOURCE_ROT",
    "MOUNT_UNBACKED",
    "ANCHORS_SIDECAR_UNREADABLE",
    "SCHEMA_FORMAT_DEFECT",
];

/// Whether `code` is a health condition an acknowledgement may name.
pub fn is_health_condition(code: &str) -> bool {
    HEALTH_CONDITIONS.contains(&code)
}

/// Whether `code` is spelled in the engine's namespace: `UPPER_SNAKE`,
/// a letter first, three characters or more. A checker's own vocabulary
/// takes any other spelling.
pub fn is_engine_code_shape(code: &str) -> bool {
    code.len() >= 3
        && code.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && code
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// The check record that acknowledges a finding, as the report carries it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Acknowledgement {
    /// Unix epoch seconds of the record.
    pub ts: u64,
    /// The caller-declared identity, when one was declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    /// The recorded actor (the transport).
    pub actor: String,
    /// The declared role, or `unspecified`.
    pub role: String,
    /// The owner and the closing plan, as the checker wrote them.
    pub method: String,
    /// The finding message the checker attached.
    pub message: String,
}

impl Acknowledgement {
    fn from_record(r: &CheckRecord) -> Self {
        Self {
            ts: r.ts,
            identity: r.identity.clone(),
            actor: r.actor.clone(),
            role: r.role.clone(),
            method: r.method.clone().unwrap_or_default(),
            message: r
                .finding
                .as_ref()
                .map(|f| f.message.clone())
                .unwrap_or_default(),
        }
    }
}

/// One entity finding on the strict axis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StrictFinding {
    /// One of [`HEALTH_CONDITIONS`].
    pub code: String,
    /// The mem-qualified entity id.
    pub entity: String,
    /// The mem the entity lives in.
    pub mem: String,
    /// What the report said about it, verbatim from the section it came from.
    pub detail: serde_json::Value,
    /// Whether a standing acknowledgement covers it.
    pub acknowledged: bool,
    /// The acknowledgement, when one stands.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acknowledgement: Option<Acknowledgement>,
}

/// One configuration defect on the strict axis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigurationDefect {
    /// One of [`CONFIGURATION_CONDITIONS`].
    pub code: String,
    /// The mem it concerns, when one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mem: Option<String>,
    /// The report's own row for it.
    pub detail: serde_json::Value,
}

/// A standing acknowledgement whose finding no longer occurs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StaleAcknowledgement {
    /// Always [`STALE_ACKNOWLEDGEMENT_CODE`].
    pub code: &'static str,
    /// The entity the acknowledgement names.
    pub entity: String,
    /// The condition it acknowledged.
    pub condition: String,
    /// The record itself, so the reader can find and withdraw it.
    pub acknowledgement: Acknowledgement,
}

/// The strict axis, as `health --strict` serves it under `strict`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StrictAxis {
    /// The includes strict evaluated ([`STRICT_INCLUDES`]).
    pub evaluated: Vec<&'static str>,
    /// The condition vocabulary an acknowledgement may name.
    pub conditions: Vec<&'static str>,
    /// Every entity finding, acknowledged or not.
    pub findings: Vec<StrictFinding>,
    /// Configuration defects, never acknowledgeable.
    pub configuration: Vec<ConfigurationDefect>,
    /// Acknowledgements whose finding no longer occurs.
    pub stale_acknowledgements: Vec<StaleAcknowledgement>,
    /// Violations by code, in code order: unacknowledged findings,
    /// configuration defects, stale acknowledgements.
    pub violations_by_code: BTreeMap<String, usize>,
    /// The total the exit code turns on.
    pub violations: usize,
    /// Acknowledged findings, for the reader.
    pub acknowledged: usize,
}

impl StrictAxis {
    /// One line naming every violating code with its count, for the refusal.
    pub fn summary(&self) -> String {
        self.violations_by_code
            .iter()
            .map(|(code, n)| format!("{code}: {n}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// The mem half of a mem-qualified entity id (`mem--slug`, where the mem
/// may carry slashes but never a double dash).
fn mem_of(entity_id: &str) -> &str {
    entity_id.split_once("--").map_or(entity_id, |(m, _)| m)
}

/// Read the strict axis off a composed health report (every include in
/// [`STRICT_INCLUDES`] already rendered into `report`) and the workspace
/// check ledger. `mem_filter` scopes the stale-acknowledgement walk the
/// way the report itself was scoped.
pub fn compose_strict_axis(
    report: &serde_json::Map<String, serde_json::Value>,
    ledger: Option<&CheckLedger>,
    mem_filter: Option<&str>,
) -> StrictAxis {
    let mut findings: Vec<StrictFinding> = Vec::new();
    let mut configuration: Vec<ConfigurationDefect> = Vec::new();

    let arr = |key: &str| -> Vec<serde_json::Value> {
        report
            .get(key)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    };
    let s = |v: &serde_json::Value, key: &str| -> String {
        v.get(key)
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string()
    };

    // Integrity findings: the consistency codes only; the conformance
    // axis reports beside them and stays advisory here, as it always was.
    for f in arr("findings") {
        let code = s(&f, "code");
        if is_health_condition(&code) {
            let id = s(&f, "id");
            findings.push(StrictFinding {
                mem: mem_of(&id).to_string(),
                code,
                entity: id,
                detail: f.get("detail").cloned().unwrap_or(serde_json::Value::Null),
                acknowledged: false,
                acknowledgement: None,
            });
        } else if code == "ANCHORS_SIDECAR_UNREADABLE" {
            configuration.push(ConfigurationDefect {
                code,
                mem: f
                    .get("detail")
                    .and_then(|d| d.get("mem"))
                    .and_then(|m| m.as_str())
                    .map(str::to_string),
                detail: f,
            });
        }
    }
    for m in arr("missing_required_outgoing") {
        let id = s(&m, "id");
        findings.push(StrictFinding {
            code: "MISSING_REQUIRED_OUTGOING".to_string(),
            mem: s(&m, "mem"),
            entity: id,
            detail: m.get("missing").cloned().unwrap_or(serde_json::Value::Null),
            acknowledged: false,
            acknowledgement: None,
        });
    }
    for c in arr("constraints") {
        let id = s(&c, "id");
        let mem = s(&c, "mem");
        if let Some(v) = c.get("violations").and_then(|x| x.as_array())
            && !v.is_empty()
        {
            findings.push(StrictFinding {
                code: "CONSTRAINT_UNSATISFIED".to_string(),
                mem: mem.clone(),
                entity: id.clone(),
                detail: serde_json::Value::Array(v.clone()),
                acknowledged: false,
                acknowledgement: None,
            });
        }
        if let Some(fv) = c.get("format_violations").and_then(|x| x.as_array())
            && !fv.is_empty()
        {
            findings.push(StrictFinding {
                code: "SECTION_FORMAT_VIOLATION".to_string(),
                mem,
                entity: id,
                detail: serde_json::Value::Array(fv.clone()),
                acknowledged: false,
                acknowledgement: None,
            });
        }
    }
    for d in arr("schema_format_defects") {
        configuration.push(ConfigurationDefect {
            code: "SCHEMA_FORMAT_DEFECT".to_string(),
            mem: d.get("mem").and_then(|m| m.as_str()).map(str::to_string),
            detail: d,
        });
    }
    if let Some(entities) = report
        .get("signals")
        .and_then(|sg| sg.get("entities"))
        .and_then(|e| e.as_array())
    {
        for e in entities {
            // A row lists every signal above `none`; the entity is a
            // warn-level finding when any of them sits at `warn`.
            let warn: Vec<serde_json::Value> = e
                .get("signals")
                .and_then(|x| x.as_array())
                .into_iter()
                .flatten()
                .filter(|sg| s(sg, "level") == "warn")
                .cloned()
                .collect();
            if !warn.is_empty() {
                let id = s(e, "id");
                findings.push(StrictFinding {
                    code: "SIGNAL_WARN".to_string(),
                    mem: mem_of(&id).to_string(),
                    entity: id,
                    detail: serde_json::Value::Array(warn),
                    acknowledged: false,
                    acknowledgement: None,
                });
            }
        }
    }
    if let Some(mems) = report.get("anchors").and_then(|a| a.as_object()) {
        for (mem, row) in mems {
            if let Some(cond) = row.get("condition").filter(|c| !c.is_null()) {
                configuration.push(ConfigurationDefect {
                    code: "ANCHORS_SIDECAR_UNREADABLE".to_string(),
                    mem: Some(mem.clone()),
                    detail: cond.clone(),
                });
            }
        }
    }
    for w in arr("warnings") {
        let code = s(&w, "code");
        if CONFIGURATION_CONDITIONS.contains(&code.as_str()) {
            configuration.push(ConfigurationDefect {
                mem: w
                    .get("details")
                    .and_then(|d| d.get("mem"))
                    .and_then(|m| m.as_str())
                    .map(str::to_string),
                code,
                detail: w,
            });
        }
    }
    // One row per configuration defect: the anchors axis and the integrity
    // findings both carry an unreadable sidecar.
    configuration.dedup_by(|a, b| a.code == b.code && a.mem == b.mem);

    // Standing acknowledgements: the newest record per (entity, condition)
    // that carries a health-condition finding, standing while its verdict
    // is `failed` — a later `ok` on the same condition withdraws it.
    let mut standing: BTreeMap<(String, String), Acknowledgement> = BTreeMap::new();
    let mut withdrawn: BTreeMap<(String, String), ()> = BTreeMap::new();
    if let Some(ledger) = ledger {
        for rec in ledger.all() {
            let Some(f) = &rec.finding else { continue };
            if !is_health_condition(&f.code) {
                continue;
            }
            let key = (rec.entity.clone(), f.code.clone());
            if rec.verdict == "failed" {
                withdrawn.remove(&key);
                standing.insert(key, Acknowledgement::from_record(&rec));
            } else {
                standing.remove(&key);
                withdrawn.insert(key, ());
            }
        }
    }

    for f in &mut findings {
        if let Some(ack) = standing.get(&(f.entity.clone(), f.code.clone())) {
            f.acknowledged = true;
            f.acknowledgement = Some(ack.clone());
        }
    }
    let present: std::collections::BTreeSet<(String, String)> = findings
        .iter()
        .map(|f| (f.entity.clone(), f.code.clone()))
        .collect();
    let mut stale_acknowledgements: Vec<StaleAcknowledgement> = standing
        .into_iter()
        .filter(|(key, _)| !present.contains(key))
        .filter(|((entity, _), _)| mem_filter.is_none_or(|m| mem_of(entity) == m))
        .map(|((entity, condition), ack)| StaleAcknowledgement {
            code: STALE_ACKNOWLEDGEMENT_CODE,
            entity,
            condition,
            acknowledgement: ack,
        })
        .collect();
    stale_acknowledgements
        .sort_by(|a, b| (&a.entity, &a.condition).cmp(&(&b.entity, &b.condition)));
    findings.sort_by(|a, b| (&a.entity, &a.code).cmp(&(&b.entity, &b.code)));
    configuration.sort_by(|a, b| (&a.code, &a.mem).cmp(&(&b.code, &b.mem)));

    let mut violations_by_code: BTreeMap<String, usize> = BTreeMap::new();
    for f in findings.iter().filter(|f| !f.acknowledged) {
        *violations_by_code.entry(f.code.clone()).or_default() += 1;
    }
    for c in &configuration {
        *violations_by_code.entry(c.code.clone()).or_default() += 1;
    }
    if !stale_acknowledgements.is_empty() {
        violations_by_code.insert(
            STALE_ACKNOWLEDGEMENT_CODE.to_string(),
            stale_acknowledgements.len(),
        );
    }
    let violations = violations_by_code.values().sum();
    let acknowledged = findings.iter().filter(|f| f.acknowledged).count();

    StrictAxis {
        evaluated: STRICT_INCLUDES.to_vec(),
        conditions: HEALTH_CONDITIONS.to_vec(),
        findings,
        configuration,
        stale_acknowledgements,
        violations_by_code,
        violations,
        acknowledged,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::{CheckFinding, CheckRecord};
    use tempfile::TempDir;

    fn record(entity: &str, verdict: &str, code: &str, ts: u64) -> CheckRecord {
        CheckRecord {
            ts,
            entity: entity.to_string(),
            verdict: verdict.to_string(),
            method: Some("owner: the work-down lane; plan: repairs".to_string()),
            entity_hash: "h".to_string(),
            actor: "cli".to_string(),
            client: None,
            role: "checker".to_string(),
            identity: Some("checker-1".to_string()),
            kind: None,
            schema_ref: None,
            finding: Some(CheckFinding {
                code: code.to_string(),
                section: None,
                message: "known open".to_string(),
                evidence: None,
            }),
        }
    }

    fn report_with(findings: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        let v = serde_json::json!({
            "findings": findings,
            "missing_required_outgoing": [{"id": "m--needs-edge", "mem": "m", "missing": [{"cardinality": "at_least_one"}]}],
            "constraints": [],
            "signals": {"counts": {"warn": 1}, "entities": [{"id": "m--loud", "signals": [{"name": "attack_load", "value": 3, "level": "warn", "contributors": []}]}]},
            "anchors": {"m": {"condition": null}},
            "warnings": [{"code": "MOUNT_UNBACKED", "details": {"mem": "ghost", "reason": "missing_ref"}}],
        });
        v.as_object().cloned().unwrap()
    }

    /// The namespace rule: upper-snake claims an engine condition, any other
    /// spelling is the checker's own.
    #[test]
    fn engine_code_shape_and_conditions() {
        assert!(is_engine_code_shape("MISSING_REQUIRED_OUTGOING"));
        assert!(is_engine_code_shape("NOT_A_CONDITION"));
        assert!(!is_engine_code_shape("hidden-premise"));
        assert!(!is_engine_code_shape("Ab"));
        assert!(is_health_condition("DANGLING_LINK_NOT_RELATED"));
        assert!(!is_health_condition("STALE_ACKNOWLEDGEMENT"));
        for code in crate::ops::DanglingLinkKind::ALL_CODES {
            assert!(is_health_condition(code), "{code}");
        }
    }

    /// Unacknowledged findings, a configuration defect and a stale
    /// acknowledgement each count; an acknowledged finding does not; a
    /// later `ok` withdraws an acknowledgement.
    #[test]
    fn axis_counts_unacknowledged_configuration_and_stale() {
        let tmp = TempDir::new().unwrap();
        let ledger = CheckLedger::for_workspace(tmp.path());
        // Acknowledges the missing edge (stands), acknowledges a dangling
        // link on an entity that has since been repaired (stale), and
        // acknowledged then withdrew a signal warning (counts again).
        ledger
            .record(&record(
                "m--needs-edge",
                "failed",
                "MISSING_REQUIRED_OUTGOING",
                1,
            ))
            .unwrap();
        ledger
            .record(&record(
                "m--repaired",
                "failed",
                "DANGLING_LINK_TARGET_MISSING",
                2,
            ))
            .unwrap();
        ledger
            .record(&record("m--loud", "failed", "SIGNAL_WARN", 3))
            .unwrap();
        ledger
            .record(&record("m--loud", "ok", "SIGNAL_WARN", 4))
            .unwrap();

        let report = report_with(serde_json::json!([
            {"id": "m--linker", "axis": "consistency", "code": "DANGLING_LINK_TARGET_MISSING", "detail": {"target_id": "m--nowhere"}},
            {"id": "m--linker", "axis": "conformance", "code": "UNKNOWN_SECTION", "detail": {}},
        ]));
        let axis = compose_strict_axis(&report, Some(&ledger), None);

        assert_eq!(axis.evaluated, STRICT_INCLUDES.to_vec());
        assert_eq!(axis.findings.len(), 3, "{:?}", axis.findings);
        let by_entity: BTreeMap<&str, &StrictFinding> = axis
            .findings
            .iter()
            .map(|f| (f.entity.as_str(), f))
            .collect();
        assert!(by_entity["m--needs-edge"].acknowledged);
        assert_eq!(
            by_entity["m--needs-edge"]
                .acknowledgement
                .as_ref()
                .unwrap()
                .identity
                .as_deref(),
            Some("checker-1")
        );
        assert!(!by_entity["m--linker"].acknowledged);
        assert!(!by_entity["m--loud"].acknowledged, "a later ok withdraws");
        assert_eq!(axis.stale_acknowledgements.len(), 1);
        assert_eq!(axis.stale_acknowledgements[0].entity, "m--repaired");
        assert_eq!(
            axis.stale_acknowledgements[0].condition,
            "DANGLING_LINK_TARGET_MISSING"
        );
        assert_eq!(axis.configuration.len(), 1);
        assert_eq!(axis.configuration[0].code, "MOUNT_UNBACKED");
        assert_eq!(axis.acknowledged, 1);
        let expect: BTreeMap<String, usize> = [
            ("DANGLING_LINK_TARGET_MISSING", 1),
            ("MOUNT_UNBACKED", 1),
            ("SIGNAL_WARN", 1),
            ("STALE_ACKNOWLEDGEMENT", 1),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        assert_eq!(axis.violations_by_code, expect);
        assert_eq!(axis.violations, 4);
        assert!(axis.summary().contains("STALE_ACKNOWLEDGEMENT: 1"));

        // A mem filter drops stale acknowledgements of other mems.
        let scoped = compose_strict_axis(&report, Some(&ledger), Some("other"));
        assert!(scoped.stale_acknowledgements.is_empty());
    }

    /// No ledger, no findings: a clean axis with zero violations.
    #[test]
    fn clean_axis_is_empty() {
        let report = serde_json::json!({"findings": [], "warnings": []});
        let axis = compose_strict_axis(report.as_object().unwrap(), None, None);
        assert_eq!(axis.violations, 0);
        assert!(axis.findings.is_empty());
        assert!(axis.summary().is_empty());
    }
}
