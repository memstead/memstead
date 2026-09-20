//! The proposal brief: a fork's changes against its source, three ways.
//!
//! `Engine::proposal_brief(fork)` renders the entity-level picture the
//! owner of a mem reviews before merging a fork: every entity the fork
//! added, modified, deleted or renamed against its base (the fork
//! commit, or the ancestor for a fork made before fork commits existed),
//! each with the sections that differ, the target's referrers, the
//! anchor rows it rests on, a mechanics precheck through the target's
//! write gate, a conflict mark where the target moved the same entity
//! since the ancestor, and a re-proposal mark where the target's
//! proposal record already holds a rejection of the same content or id.
//!
//! The wire shape ([`ProposalBrief`]) is the JSON the CLI writes with
//! `--out` and the MCP tool serves as `structured_content`; its
//! `dispositions` map is the skeleton the owner fills and the merge
//! consumes. [`render_proposal_brief`] renders the same data as
//! markdown for a human. Rendering is deterministic: entries sort by
//! slug and nothing carries a clock.
//!
//! The brief is a read. It writes nothing anywhere.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The closed disposition vocabulary, in the order the skeleton lists
/// it. A conflict entity's slot omits `adopt`.
pub const DISPOSITION_VOCABULARY: &[&str] = &["adopt", "adopt_with_changes", "reject"];

/// The disposition of an adopted entity.
pub const DISPOSITION_ADOPT: &str = "adopt";
/// The disposition of an entity the owner adopts with a final body of
/// their own.
pub const DISPOSITION_ADOPT_WITH_CHANGES: &str = "adopt_with_changes";
/// The disposition of a rejected entity.
pub const DISPOSITION_REJECT: &str = "reject";

/// The proposal record's path on a target branch: the sidecar the
/// merge appends to and the brief reads for re-proposal marks.
pub const PROPOSAL_RECORD_PATH: &str = ".memstead/proposals.json";

/// The proposal record's format version.
pub const PROPOSAL_RECORD_VERSION: u32 = 1;

/// The brief of one fork against its source mem.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProposalBrief {
    /// The fork mem's name.
    pub fork: String,
    /// The source mem's name: the target of the proposal.
    pub target: String,
    /// The proposal's id: the fork's name and its base sha, the key the
    /// proposal record uses.
    pub proposal_id: String,
    /// The ancestor on the target (`forkedFrom.sha`).
    pub ancestor: String,
    /// The commit the fork's changes are read against: the fork commit
    /// (`forkedFrom.base`) when one was recorded, the ancestor otherwise.
    pub base: String,
    /// Whether the base is the ancestor itself (a fork made before fork
    /// commits existed recorded no base).
    pub base_is_ancestor: bool,
    /// The fork's branch tip at render time.
    pub fork_tip: String,
    /// The target's branch tip at render time; the merge pins it.
    pub target_tip: String,
    /// Counts over `entries`.
    pub summary: ProposalSummary,
    /// One entry per entity the fork touched, sorted by slug.
    pub entries: Vec<ProposalEntry>,
    /// The disposition skeleton, keyed by slug: the owner fills
    /// `disposition` (and `reason`, and for `adopt_with_changes` a
    /// `body`) and hands the file to the merge.
    pub dispositions: BTreeMap<String, DispositionSlot>,
}

/// Counts over a brief's entries.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalSummary {
    pub added: usize,
    pub modified: usize,
    pub deleted: usize,
    pub renamed: usize,
    pub conflicts: usize,
    pub precheck_failures: usize,
    pub re_proposals: usize,
}

/// What the fork did to one entity, read against the base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalChange {
    Added,
    Modified,
    Deleted,
    Renamed,
}

impl ProposalChange {
    /// Stable wire form.
    pub fn as_wire(self) -> &'static str {
        match self {
            ProposalChange::Added => "added",
            ProposalChange::Modified => "modified",
            ProposalChange::Deleted => "deleted",
            ProposalChange::Renamed => "renamed",
        }
    }
}

/// One entity the fork touched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProposalEntry {
    /// The entity's slug: the part of the id after the mem name, the
    /// key the two mems share.
    pub slug: String,
    /// The entity's id in the fork (`<fork>--<slug>`).
    pub fork_id: String,
    /// The entity's id in the target (`<target>--<slug>`).
    pub target_id: String,
    /// What the fork did.
    pub status: ProposalChange,
    /// For a renamed entity: the slug it had at the base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renamed_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_type: Option<String>,
    /// The content hash of the proposed version as it would land in
    /// the target (the fork's body with its self-links under the
    /// target's name); absent for a deletion. The proposal record
    /// stores the same hash, so a rejected body is recognised when a
    /// later fork proposes it again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    /// How the fork's version differs from the base (absent for an
    /// added or deleted entity, and when a side does not parse).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_delta: Option<VersionDelta>,
    /// The target's referrers of this entity: entities in the target
    /// or in other mems holding an edge to it (the fork's own excluded).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub referrers: Vec<Referrer>,
    /// The anchor rows the fork's entity rests on, read from the
    /// fork's sidecar under the fork's ids; no observation is fetched.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub anchors: Vec<AnchorRow>,
    /// The mechanics precheck through the target's write gate.
    pub precheck: Precheck,
    /// Present when the target moved the same entity since the ancestor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict: Option<Conflict>,
    /// Present when the target's proposal record holds a rejection of
    /// the same content hash or the same id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub re_proposal: Vec<ReProposalMark>,
    /// The base's body, with mem qualifiers normalised to the fork's
    /// name (absent for an added entity).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_body: Option<String>,
    /// The fork's body, the proposed version (absent for a deletion).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_body: Option<String>,
}

/// How one side's version differs from the base.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionDelta {
    pub title_differs: bool,
    pub entity_type_differs: bool,
    /// Section keys whose content differs, or that one side has and
    /// the other lacks, in the order the fork's body declares them.
    pub sections: Vec<String>,
    /// Metadata keys whose value differs (the engine-stamped
    /// `created_date` and `last_modified` never count).
    pub metadata: Vec<String>,
    pub relationships_differ: bool,
}

impl VersionDelta {
    /// Whether anything differs.
    pub fn is_empty(&self) -> bool {
        !self.title_differs
            && !self.entity_type_differs
            && self.sections.is_empty()
            && self.metadata.is_empty()
            && !self.relationships_differ
    }
}

/// An entity holding an edge to the target's entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Referrer {
    pub id: String,
    pub rel_type: String,
}

/// One anchor row of the fork's entity, as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorRow {
    pub artifact: String,
    pub grain: String,
    pub class: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<String>,
    /// The last recorded state of the row (`resolves`, `drifted`,
    /// `recheck`, `orphaned`, `span_absent`); absent when the row was
    /// never observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_state: Option<String>,
    /// When that state was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_observed_at: Option<String>,
}

/// The mechanics precheck: would the fork's body pass the target's
/// write gate as it stands. Never a content judgement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Precheck {
    pub outcome: PrecheckOutcome,
    /// The rehearsed operation (`create` or `update`) when one ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    /// The typed refusal the gate returned, when it refused.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<PrecheckFailure>,
    /// Why no operation ran, when none did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Precheck {
    pub fn clean(operation: &str) -> Self {
        Self {
            outcome: PrecheckOutcome::Clean,
            operation: Some(operation.to_string()),
            failures: Vec::new(),
            note: None,
        }
    }

    pub fn failed(operation: &str, failures: Vec<PrecheckFailure>) -> Self {
        Self {
            outcome: PrecheckOutcome::Failed,
            operation: Some(operation.to_string()),
            failures,
            note: None,
        }
    }

    pub fn not_run(note: &str) -> Self {
        Self {
            outcome: PrecheckOutcome::NotRun,
            operation: None,
            failures: Vec::new(),
            note: Some(note.to_string()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrecheckOutcome {
    Clean,
    Failed,
    NotRun,
}

impl PrecheckOutcome {
    pub fn as_wire(self) -> &'static str {
        match self {
            PrecheckOutcome::Clean => "clean",
            PrecheckOutcome::Failed => "failed",
            PrecheckOutcome::NotRun => "not_run",
        }
    }
}

/// One typed refusal of the write gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrecheckFailure {
    pub code: String,
    pub message: String,
}

/// The target moved the same entity since the ancestor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conflict {
    pub kind: ConflictKind,
    /// How the target's version differs from the base (when both parse).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_delta: Option<VersionDelta>,
    /// The target's body, mem qualifiers normalised to the fork's name
    /// (absent when the target deleted the entity).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_body: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    /// The target modified an entity the fork modified, deleted or
    /// renamed.
    TargetModified,
    /// The target deleted (or renamed away) an entity the fork
    /// modified, deleted or renamed.
    TargetDeleted,
    /// The target created an entity with a slug the fork added.
    TargetAdded,
}

impl ConflictKind {
    pub fn as_wire(self) -> &'static str {
        match self {
            ConflictKind::TargetModified => "target_modified",
            ConflictKind::TargetDeleted => "target_deleted",
            ConflictKind::TargetAdded => "target_added",
        }
    }

    fn describe(self) -> &'static str {
        match self {
            ConflictKind::TargetModified => "the target modified this entity since the ancestor",
            ConflictKind::TargetDeleted => "the target deleted this entity since the ancestor",
            ConflictKind::TargetAdded => {
                "the target created an entity with this slug since the ancestor"
            }
        }
    }
}

/// A rejection in the target's proposal record that this entry matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReProposalMark {
    /// `content_hash` (the same proposed body) or `id` (the same slug).
    pub matched_by: String,
    /// The recorded proposal's id.
    pub proposal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// One slot of the disposition skeleton.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DispositionSlot {
    /// Empty in the skeleton; one of `accepts` once filled.
    pub disposition: String,
    /// Empty in the skeleton; required for `reject` and
    /// `adopt_with_changes`.
    pub reason: String,
    /// The values this slot accepts: the closed vocabulary, without
    /// `adopt` on a conflict entity.
    pub accepts: Vec<String>,
    /// For `adopt_with_changes`: the owner's final body in the shape a
    /// create takes (`title`, `sections`, `metadata`); absent in the
    /// skeleton.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<serde_json::Value>,
}

impl DispositionSlot {
    /// The empty slot for an entry, `adopt` withheld on a conflict.
    pub fn skeleton(conflict: bool) -> Self {
        let accepts = DISPOSITION_VOCABULARY
            .iter()
            .filter(|v| !(conflict && **v == DISPOSITION_ADOPT))
            .map(|v| (*v).to_string())
            .collect();
        Self {
            disposition: String::new(),
            reason: String::new(),
            accepts,
            body: None,
        }
    }
}

// ---------------------------------------------------------------------------
// The proposal record
// ---------------------------------------------------------------------------

/// The proposal record a target branch carries at
/// [`PROPOSAL_RECORD_PATH`]: one entry per merged proposal with every
/// disposition, so a rejected claim cannot return unseen. The merge
/// writes it; the brief reads it. Unknown keys are tolerated so an
/// older reader survives a newer writer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProposalRecord {
    #[serde(default = "default_record_version")]
    pub version: u32,
    #[serde(default)]
    pub proposals: Vec<ProposalRecordEntry>,
}

fn default_record_version() -> u32 {
    PROPOSAL_RECORD_VERSION
}

/// One merged proposal in the record.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProposalRecordEntry {
    /// The proposal's id: the fork's name and its base sha.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposer: Option<String>,
    #[serde(default)]
    pub ancestor: String,
    #[serde(default)]
    pub base: String,
    #[serde(default)]
    pub target_tip: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_by: Option<String>,
    #[serde(default)]
    pub at: String,
    /// Per slug: the disposition, the reason and the content hash of
    /// the proposed version (never its body).
    #[serde(default)]
    pub entities: BTreeMap<String, RecordedDisposition>,
}

/// One entity's disposition in the record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedDisposition {
    pub disposition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

impl ProposalRecord {
    /// Parse the record's bytes; a malformed record is an error the
    /// caller names, never an empty record.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    /// The rejections that match an entry: by the proposed content hash
    /// (the same body) or by slug (the same id), in record order.
    pub fn rejections_matching(
        &self,
        slug: &str,
        content_hash: Option<&str>,
    ) -> Vec<ReProposalMark> {
        let mut marks = Vec::new();
        for proposal in &self.proposals {
            for (recorded_slug, recorded) in &proposal.entities {
                if recorded.disposition != DISPOSITION_REJECT {
                    continue;
                }
                let by_hash = matches!(
                    (content_hash, recorded.content_hash.as_deref()),
                    (Some(a), Some(b)) if a == b
                );
                if by_hash {
                    marks.push(ReProposalMark {
                        matched_by: "content_hash".to_string(),
                        proposal: proposal.id.clone(),
                        reason: recorded.reason.clone(),
                    });
                } else if recorded_slug == slug {
                    marks.push(ReProposalMark {
                        matched_by: "id".to_string(),
                        proposal: proposal.id.clone(),
                        reason: recorded.reason.clone(),
                    });
                }
            }
        }
        marks
    }
}

/// The proposal id the record keys by: the fork's name and its base sha.
pub fn proposal_id(fork: &str, base: &str) -> String {
    format!("{fork}@{base}")
}

// ---------------------------------------------------------------------------
// The markdown rendering
// ---------------------------------------------------------------------------

/// The fence that wraps a body: four backticks, so a body carrying a
/// three-backtick block renders whole.
const BODY_FENCE: &str = "````";

/// Render the brief for a human. Deterministic: the same brief renders
/// the same bytes.
pub fn render_proposal_brief(brief: &ProposalBrief) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# Proposal brief: `{}` against `{}`\n\n",
        brief.fork, brief.target
    ));
    out.push_str(&format!("- Proposal: `{}`\n", brief.proposal_id));
    out.push_str(&format!(
        "- Ancestor on `{}`: `{}`\n",
        brief.target, brief.ancestor
    ));
    if brief.base_is_ancestor {
        out.push_str(&format!(
            "- Base: `{}` (no fork commit recorded; the ancestor is the base)\n",
            brief.base
        ));
    } else {
        out.push_str(&format!("- Base (the fork commit): `{}`\n", brief.base));
    }
    out.push_str(&format!("- Fork tip: `{}`\n", brief.fork_tip));
    out.push_str(&format!(
        "- Target tip: `{}` (at render time; the merge pins it)\n",
        brief.target_tip
    ));
    let s = &brief.summary;
    out.push_str(&format!(
        "- Entities: {} (added {}, modified {}, deleted {}, renamed {}); conflicts {}; precheck failures {}; re-proposals {}\n\n",
        brief.entries.len(),
        s.added,
        s.modified,
        s.deleted,
        s.renamed,
        s.conflicts,
        s.precheck_failures,
        s.re_proposals
    ));
    out.push_str(
        "Dispositions: in the JSON form (`--json`, or `--out <file>`), fill each \
         `dispositions.<slug>.disposition` with one of `adopt`, `adopt_with_changes`, `reject`; \
         `reject` and `adopt_with_changes` need a `reason`; `adopt_with_changes` carries the \
         owner's final body as `body` in the shape a create takes (`title`, `sections`, \
         `metadata`); a conflict entity accepts no `adopt`. The merge validates the file \
         against these shas.\n",
    );
    if brief.entries.is_empty() {
        out.push_str("\nThe fork has no change against its base.\n");
        return out;
    }
    for entry in &brief.entries {
        out.push('\n');
        render_entry(&mut out, brief, entry);
    }
    out
}

fn render_entry(out: &mut String, brief: &ProposalBrief, e: &ProposalEntry) {
    let title = e.title.as_deref().unwrap_or("(no title)");
    out.push_str(&format!(
        "## `{}`: {} ({})\n\n",
        e.slug,
        e.status.as_wire(),
        title
    ));
    out.push_str(&format!(
        "- Type: {}\n",
        e.entity_type.as_deref().unwrap_or("(unknown)")
    ));
    out.push_str(&format!(
        "- Fork id: `{}`; target id: `{}`\n",
        e.fork_id, e.target_id
    ));
    if let Some(from) = &e.renamed_from {
        out.push_str(&format!("- Renamed from: `{from}`\n"));
    }
    if let Some(hash) = &e.content_hash {
        out.push_str(&format!("- Proposed content hash: `{hash}`\n"));
    }
    match e.status {
        ProposalChange::Added => {
            out.push_str("- Difference against base: the whole entity is new\n")
        }
        ProposalChange::Deleted => {
            out.push_str("- Difference against base: the entity is removed\n")
        }
        ProposalChange::Modified | ProposalChange::Renamed => {
            out.push_str(&format!(
                "- Differences (fork against base): {}\n",
                render_delta(e.fork_delta.as_ref())
            ));
        }
    }
    if e.referrers.is_empty() {
        out.push_str("- Target referrers: none\n");
    } else {
        let list: Vec<String> = e
            .referrers
            .iter()
            .map(|r| format!("`{}` ({})", r.id, r.rel_type))
            .collect();
        out.push_str(&format!("- Target referrers: {}\n", list.join(", ")));
    }
    if e.anchors.is_empty() {
        out.push_str("- Anchors: none\n");
    } else {
        out.push_str("- Anchors:\n");
        for a in &e.anchors {
            let span = match &a.span {
                Some(s) => format!("; span: \"{s}\""),
                None => String::new(),
            };
            let state = match (&a.last_state, &a.last_observed_at) {
                (Some(state), Some(at)) => format!("; last state: {state} at {at}"),
                (Some(state), None) => format!("; last state: {state}"),
                (None, _) => "; last state: unobserved".to_string(),
            };
            out.push_str(&format!(
                "  - `{}` ({}, {}{span}{state})\n",
                a.artifact, a.grain, a.class
            ));
        }
    }
    match e.precheck.outcome {
        PrecheckOutcome::Clean => out.push_str(&format!(
            "- Precheck ({} in `{}`): clean\n",
            e.precheck.operation.as_deref().unwrap_or("gate"),
            brief.target
        )),
        PrecheckOutcome::Failed => {
            out.push_str(&format!(
                "- Precheck ({} in `{}`): FAILED\n",
                e.precheck.operation.as_deref().unwrap_or("gate"),
                brief.target
            ));
            for f in &e.precheck.failures {
                out.push_str(&format!("  - `{}`: {}\n", f.code, f.message));
            }
        }
        PrecheckOutcome::NotRun => out.push_str(&format!(
            "- Precheck: not run ({})\n",
            e.precheck.note.as_deref().unwrap_or("no operation applies")
        )),
    }
    if let Some(c) = &e.conflict {
        let target_side = match &c.target_delta {
            Some(d) => format!("; target against base: {}", render_delta(Some(d))),
            None => String::new(),
        };
        out.push_str(&format!(
            "- CONFLICT: {} ({}{target_side}); `adopt` is withheld, the owner supplies the merged body or rejects\n",
            c.kind.describe(),
            c.kind.as_wire()
        ));
    }
    for mark in &e.re_proposal {
        let reason = match &mark.reason {
            Some(r) => format!(": {r}"),
            None => String::new(),
        };
        let how = match mark.matched_by.as_str() {
            "content_hash" => "the same content",
            _ => "the same id",
        };
        out.push_str(&format!(
            "- RE-PROPOSAL: rejected in proposal `{}` ({how}){reason}\n",
            mark.proposal
        ));
    }
    if let Some(c) = &e.conflict {
        out.push_str("\n### Base version\n\n");
        render_body(out, e.base_body.as_deref());
        out.push_str("\n### Fork version\n\n");
        render_body(out, e.fork_body.as_deref());
        out.push_str("\n### Target version\n\n");
        render_body(out, c.target_body.as_deref());
        return;
    }
    match e.status {
        ProposalChange::Added => {
            out.push_str("\n### Proposed body\n\n");
            render_body(out, e.fork_body.as_deref());
        }
        ProposalChange::Deleted => {
            out.push_str("\n### Body at base\n\n");
            render_body(out, e.base_body.as_deref());
        }
        ProposalChange::Modified | ProposalChange::Renamed => {
            out.push_str("\n### Fork version\n\n");
            render_body(out, e.fork_body.as_deref());
        }
    }
}

fn render_delta(delta: Option<&VersionDelta>) -> String {
    let Some(d) = delta else {
        return "(a side does not parse)".to_string();
    };
    if d.is_empty() {
        return "none beyond mem qualifiers".to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    if d.title_differs {
        parts.push("title".to_string());
    }
    if d.entity_type_differs {
        parts.push("type".to_string());
    }
    if !d.sections.is_empty() {
        parts.push(format!(
            "sections {}",
            d.sections
                .iter()
                .map(|s| format!("`{s}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !d.metadata.is_empty() {
        parts.push(format!(
            "metadata {}",
            d.metadata
                .iter()
                .map(|s| format!("`{s}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if d.relationships_differ {
        parts.push("relationships".to_string());
    }
    parts.join("; ")
}

fn render_body(out: &mut String, body: Option<&str>) {
    match body {
        Some(b) => {
            out.push_str(BODY_FENCE);
            out.push_str("markdown\n");
            out.push_str(b);
            if !b.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(BODY_FENCE);
            out.push('\n');
        }
        None => out.push_str("(absent)\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(slug: &str, status: ProposalChange, conflict: Option<ConflictKind>) -> ProposalEntry {
        ProposalEntry {
            slug: slug.to_string(),
            fork_id: format!("f--{slug}"),
            target_id: format!("t--{slug}"),
            status,
            renamed_from: None,
            title: Some(slug.to_uppercase()),
            entity_type: Some("spec".to_string()),
            content_hash: Some("abc".to_string()),
            fork_delta: Some(VersionDelta {
                sections: vec!["identity".to_string()],
                ..Default::default()
            }),
            referrers: vec![Referrer {
                id: "t--alpha".to_string(),
                rel_type: "DEPENDS_ON".to_string(),
            }],
            anchors: vec![AnchorRow {
                artifact: "https://example.org/p".to_string(),
                grain: "url".to_string(),
                class: "anchored".to_string(),
                span: Some("the words".to_string()),
                last_state: None,
                last_observed_at: None,
            }],
            precheck: Precheck::clean("update"),
            conflict: conflict.map(|kind| Conflict {
                kind,
                target_delta: Some(VersionDelta {
                    sections: vec!["identity".to_string()],
                    ..Default::default()
                }),
                target_body: Some("# T\n".to_string()),
            }),
            re_proposal: Vec::new(),
            base_body: Some("# B\n".to_string()),
            fork_body: Some("# F\n".to_string()),
        }
    }

    fn brief() -> ProposalBrief {
        let entries = vec![
            entry("beta", ProposalChange::Modified, None),
            entry(
                "alpha",
                ProposalChange::Modified,
                Some(ConflictKind::TargetModified),
            ),
        ];
        let dispositions = entries
            .iter()
            .map(|e| {
                (
                    e.slug.clone(),
                    DispositionSlot::skeleton(e.conflict.is_some()),
                )
            })
            .collect();
        ProposalBrief {
            fork: "f".to_string(),
            target: "t".to_string(),
            proposal_id: proposal_id("f", "b"),
            ancestor: "a".to_string(),
            base: "b".to_string(),
            base_is_ancestor: false,
            fork_tip: "c".to_string(),
            target_tip: "d".to_string(),
            summary: ProposalSummary {
                modified: 2,
                conflicts: 1,
                ..Default::default()
            },
            entries,
            dispositions,
        }
    }

    /// The skeleton carries the closed vocabulary, and a conflict slot
    /// withholds `adopt`; the wire keys are the ones the merge reads.
    #[test]
    fn skeleton_vocabulary_is_closed_and_a_conflict_withholds_adopt() {
        let b = brief();
        let json = serde_json::to_value(&b).unwrap();
        assert_eq!(
            json["dispositions"]["beta"],
            serde_json::json!({
                "disposition": "",
                "reason": "",
                "accepts": ["adopt", "adopt_with_changes", "reject"]
            })
        );
        assert_eq!(
            json["dispositions"]["alpha"]["accepts"],
            serde_json::json!(["adopt_with_changes", "reject"])
        );
        for key in [
            "fork",
            "target",
            "proposal_id",
            "ancestor",
            "base",
            "base_is_ancestor",
            "fork_tip",
            "target_tip",
            "summary",
            "entries",
            "dispositions",
        ] {
            assert!(json.get(key).is_some(), "top-level key {key}");
        }
        assert_eq!(json["entries"][0]["status"], "modified");
        assert_eq!(json["entries"][0]["precheck"]["outcome"], "clean");
        assert_eq!(json["entries"][1]["conflict"]["kind"], "target_modified");
        // The JSON round-trips through the same types the merge reads.
        let back: ProposalBrief = serde_json::from_value(json).unwrap();
        assert_eq!(back, b);
    }

    /// The markdown names the four shas, one block per entity, the
    /// differing sections, the referrers, the anchor row with its span
    /// and unobserved state, the precheck, the conflict with its three
    /// versions; and it renders the same bytes twice.
    #[test]
    fn markdown_is_deterministic_and_names_every_axis() {
        let b = brief();
        let md = render_proposal_brief(&b);
        assert_eq!(md, render_proposal_brief(&b));
        for needle in [
            "# Proposal brief: `f` against `t`",
            "- Ancestor on `t`: `a`",
            "- Base (the fork commit): `b`",
            "- Fork tip: `c`",
            "- Target tip: `d`",
            "## `alpha`: modified (ALPHA)",
            "## `beta`: modified (BETA)",
            "- Differences (fork against base): sections `identity`",
            "- Target referrers: `t--alpha` (DEPENDS_ON)",
            "  - `https://example.org/p` (url, anchored; span: \"the words\"; last state: unobserved)",
            "- Precheck (update in `t`): clean",
            "- CONFLICT: the target modified this entity since the ancestor (target_modified; target against base: sections `identity`)",
            "### Base version",
            "### Fork version",
            "### Target version",
            "````markdown\n# T\n````",
        ] {
            assert!(md.contains(needle), "missing {needle:?} in:\n{md}");
        }
        // No clock, no em dash.
        assert!(!md.contains('\u{2014}'));
    }

    /// The fallback line for a fork without a recorded base, and an
    /// empty brief.
    #[test]
    fn markdown_names_the_ancestor_fallback_and_the_empty_case() {
        let mut b = brief();
        b.base_is_ancestor = true;
        b.entries.clear();
        b.dispositions.clear();
        let md = render_proposal_brief(&b);
        assert!(md.contains("- Base: `b` (no fork commit recorded; the ancestor is the base)"));
        assert!(md.contains("The fork has no change against its base."));
    }

    /// The record parses with unknown keys tolerated, an absent record
    /// is the caller's empty default, and the rejection match names
    /// the hash before the id.
    #[test]
    fn record_parses_and_matches_rejections_by_hash_then_id() {
        let bytes = br#"{"version": 1, "future": true, "proposals": [{"id": "f@b", "proposer": "p", "ancestor": "a", "base": "b", "target_tip": "d", "merged_by": "m", "at": "2026-09-20T00:00:00Z", "entities": {"alpha": {"disposition": "reject", "reason": "weak", "content_hash": "h1"}, "beta": {"disposition": "adopt", "content_hash": "h2"}}}]}"#;
        let record = ProposalRecord::from_bytes(bytes).unwrap();
        assert_eq!(record.proposals.len(), 1);
        // Same body under a new slug: matched by hash.
        let marks = record.rejections_matching("gamma", Some("h1"));
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].matched_by, "content_hash");
        assert_eq!(marks[0].proposal, "f@b");
        assert_eq!(marks[0].reason.as_deref(), Some("weak"));
        // Same slug, different body: matched by id.
        let marks = record.rejections_matching("alpha", Some("h9"));
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].matched_by, "id");
        // An adopted entity never marks.
        assert!(record.rejections_matching("beta", Some("h2")).is_empty());
        // Malformed bytes refuse rather than read as empty.
        assert!(ProposalRecord::from_bytes(b"not json").is_err());
        assert_eq!(ProposalRecord::default().proposals.len(), 0);
    }
}
