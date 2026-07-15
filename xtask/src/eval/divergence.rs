//! The divergence mode's view of the pre-registration package.
//!
//! The divergence campaign (plan 02 of the divergence-eval bundle) consumes the
//! committed package at `docs/proof/divergence/prereg/` as its *only*
//! configuration. Two things make the pre-registration real rather than
//! decorative and both live here:
//!
//! 1. **Content-hash pinning.** The package is hashed at campaign start; a run
//!    or a resume against an edited package must refuse rather than silently mix
//!    two designs. [`Package::content_hash`] is that hash; [`Package::verify_pin`]
//!    is the refusal.
//! 2. **The pinned model.** Writers and readers share one model across both arms
//!    (parity); [`Package::single_model`] returns it only when the package's
//!    writer and reader pins agree, so a confounded pair is refused up front.
//!
//! Only the parts the harness reads today are modelled: the campaign parameters
//! ([`Campaign`]), the model pins ([`Models`]), and both arms' tell lists
//! ([`TellLists`], fed to [`super::grade::strip_tells_with`] on the reader path).
//! The slice manifest, the query battery, and the prompts load as the round loop
//! and reader battery grow to need them.

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Machine-readable campaign parameters, from `campaign.json`.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct Campaign {
    pub rounds: usize,
    pub hurry_rounds: Vec<usize>,
    pub reader_checkpoints: Vec<usize>,
    pub integrity_audit_rounds: Vec<usize>,
    pub trials: usize,
    pub writer_allowance_full_tokens: usize,
    pub writer_allowance_hurry_tokens: usize,
    pub reader_budget_tokens: usize,
    /// The pinned model's list output price in USD per token, recorded in
    /// `campaign.json` (amendment A1). The conversion constant behind
    /// [`Campaign::budget_usd`] — for `claude-opus-4-8` this is `$25 / 1M =
    /// 0.000025`.
    pub usd_per_output_token: f64,
    pub contamination_threshold: f64,
    pub cost_cap_tokens: u64,
}

/// One round's execution plan, derived from the campaign schedule — the unit the
/// round-loop driver iterates. Nothing here is hardcoded; every field is
/// resolved from the package's `campaign.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoundPlan {
    /// 1-based round number.
    pub round: usize,
    /// Hurry round — halved writer allowance and the terse writer prompt.
    pub hurry: bool,
    /// The writer token allowance for this round (full or hurry).
    pub writer_allowance_tokens: usize,
    /// Run the reader battery after this round.
    pub reader_checkpoint: bool,
    /// Run the blinded integrity audit after this round.
    pub integrity_audit: bool,
}

impl Campaign {
    /// The executable schedule: one [`RoundPlan`] per round (`1..=rounds`), with
    /// the hurry / reader-checkpoint / integrity-audit flags and the per-round
    /// writer allowance resolved from the package. Call [`Campaign::validate`]
    /// first — this method assumes the schedule references are in range.
    pub fn schedule(&self) -> Vec<RoundPlan> {
        (1..=self.rounds)
            .map(|round| {
                let hurry = self.hurry_rounds.contains(&round);
                RoundPlan {
                    round,
                    hurry,
                    writer_allowance_tokens: if hurry {
                        self.writer_allowance_hurry_tokens
                    } else {
                        self.writer_allowance_full_tokens
                    },
                    reader_checkpoint: self.reader_checkpoints.contains(&round),
                    integrity_audit: self.integrity_audit_rounds.contains(&round),
                }
            })
            .collect()
    }

    /// Refuse a malformed schedule: at least one round, and every hurry /
    /// checkpoint / audit round must fall within `1..=rounds`. A schedule that
    /// references a round the campaign never runs is a package authoring error,
    /// not something to silently drop.
    pub fn validate(&self) -> Result<()> {
        if self.rounds == 0 {
            bail!("campaign.json declares zero rounds");
        }
        let in_range = |rs: &[usize], label: &str| -> Result<()> {
            if let Some(&bad) = rs.iter().find(|&&r| r < 1 || r > self.rounds) {
                bail!(
                    "campaign.json {label} references round {bad}, outside 1..={}",
                    self.rounds
                );
            }
            Ok(())
        };
        in_range(&self.hurry_rounds, "hurry_rounds")?;
        in_range(&self.reader_checkpoints, "reader_checkpoints")?;
        in_range(&self.integrity_audit_rounds, "integrity_audit_rounds")?;
        Ok(())
    }

    /// The dollar budget for a session with `token_allowance` tokens (amendment
    /// A1): allowances are enforced as proportional cost budgets via
    /// `claude -p --max-budget-usd`, `budget_usd = allowance_tokens *
    /// usd_per_output_token` — the pinned model's list output price recorded in
    /// `campaign.json`. Hurry rounds carry half the token allowance, so they
    /// receive literally half the budget.
    #[allow(dead_code)]
    pub fn budget_usd(&self, token_allowance: usize) -> f64 {
        token_allowance as f64 * self.usd_per_output_token
    }
}

/// Which arm — the single variable under test. Arm A is the tolerant markdown
/// directory, Arm B the engine-gated mem.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Arm {
    A,
    B,
}

/// A pair of per-arm text values (a substrate/access block for each arm).
#[derive(Clone, Debug, serde::Deserialize)]
pub struct ArmText {
    pub arm_a: String,
    pub arm_b: String,
}

impl ArmText {
    pub fn get(&self, arm: Arm) -> &str {
        match arm {
            Arm::A => &self.arm_a,
            Arm::B => &self.arm_b,
        }
    }
}

/// The writer/reader prompts, from `prompts.json`.
///
/// Each prompt is a shared skeleton (identical across arms) plus one substrate
/// block that differs only in substrate/access mechanics — the criterion-5
/// parity contract, preserved structurally: [`Prompts::writer`] and
/// [`Prompts::reader`] assemble a prompt by substituting the substrate block and
/// the round slice / query into the shared skeleton, so the two arms' prompts
/// *cannot* differ anywhere but the substrate block.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct Prompts {
    pub writer_full_skeleton: String,
    pub writer_hurry_skeleton: String,
    pub reader_skeleton: String,
    pub writer_substrate: ArmText,
    pub reader_substrate: ArmText,
    /// The arm-neutral integrity-auditor skeleton (criterion 10): a single
    /// skeleton with a `{CORPUS}` placeholder, no per-arm block — the auditor is
    /// never told which arm it audits, so the same prompt drives both arms and
    /// only the (tell-stripped) corpus differs. Machine-encodes `rubrics.md`'s
    /// integrity counting rubric.
    pub auditor_skeleton: String,
}

impl Prompts {
    /// Assemble the writer prompt for `arm` in the given mode, with this round's
    /// source slice substituted in.
    pub fn writer(&self, arm: Arm, hurry: bool, round_slice: &str) -> String {
        let skeleton = if hurry {
            &self.writer_hurry_skeleton
        } else {
            &self.writer_full_skeleton
        };
        skeleton
            .replace("{SUBSTRATE_BLOCK}", self.writer_substrate.get(arm))
            .replace("{ROUND_SLICE_CONTENT}", round_slice)
    }

    /// Assemble the reader prompt for `arm`, with the query substituted in.
    pub fn reader(&self, arm: Arm, query: &str) -> String {
        self.reader_skeleton
            .replace("{SUBSTRATE_BLOCK}", self.reader_substrate.get(arm))
            .replace("{QUERY}", query)
    }

    /// Assemble the integrity-auditor prompt, with the arm's tell-stripped corpus
    /// substituted in. There is deliberately no `arm` parameter: the skeleton is
    /// arm-neutral, so the auditor cannot learn which arm it is scoring from its
    /// prompt (criterion 10's blinding guarantee — the corpus is stripped
    /// separately, exactly like the judge path).
    pub fn auditor(&self, corpus: &str) -> String {
        self.auditor_skeleton.replace("{CORPUS}", corpus)
    }

    /// Refuse a skeleton missing a placeholder the harness must substitute — the
    /// prompt would silently omit the substrate block, the round slice, or the
    /// query, which would break the run rather than fail loudly.
    pub fn validate(&self) -> Result<()> {
        for (name, skeleton) in [
            ("writer_full_skeleton", &self.writer_full_skeleton),
            ("writer_hurry_skeleton", &self.writer_hurry_skeleton),
        ] {
            require(skeleton, "{SUBSTRATE_BLOCK}", name)?;
            require(skeleton, "{ROUND_SLICE_CONTENT}", name)?;
        }
        require(
            &self.reader_skeleton,
            "{SUBSTRATE_BLOCK}",
            "reader_skeleton",
        )?;
        require(&self.reader_skeleton, "{QUERY}", "reader_skeleton")?;
        require(&self.auditor_skeleton, "{CORPUS}", "auditor_skeleton")?;
        Ok(())
    }
}

fn require(skeleton: &str, placeholder: &str, name: &str) -> Result<()> {
    if !skeleton.contains(placeholder) {
        bail!("prompts.json {name} is missing the {placeholder} placeholder");
    }
    Ok(())
}

/// The frozen model pins, from `models.json`.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct Models {
    pub writer: String,
    pub reader: String,
    pub judge: String,
    pub auditor: String,
}

/// Both arms' tell lists, from `tell-lists.json`, flattened for blinding.
///
/// The reader-path blinder strips **both** lists from every answer regardless of
/// which arm produced it — the judge must not infer the arm from either Arm B's
/// mem/tool vocabulary or Arm A's substrate vocabulary. This is the per-arm
/// extension of the hardcoded [`super::grade::strip_tells`].
#[derive(Clone, Debug, Default)]
pub struct TellLists {
    pub arm_a: Vec<String>,
    pub arm_b: Vec<String>,
}

impl TellLists {
    /// Every tell from both arms, for the reader-path blinder that strips both
    /// directions. Pass to [`super::grade::strip_tells_with`].
    pub fn combined(&self) -> Vec<String> {
        let mut all = self.arm_a.clone();
        all.extend(self.arm_b.iter().cloned());
        all
    }

    fn from_json(bytes: &[u8]) -> Result<Self> {
        #[derive(serde::Deserialize)]
        struct File {
            arm_a_tells: Arm,
            arm_b_tells: Arm,
        }
        #[derive(serde::Deserialize)]
        struct Arm {
            #[serde(default)]
            tokens: Vec<String>,
            #[serde(default)]
            phrases: Vec<String>,
        }
        let file: File = serde_json::from_slice(bytes).context("parsing tell-lists.json")?;
        let flatten = |a: Arm| {
            let mut v = a.tokens;
            v.extend(a.phrases);
            v
        };
        Ok(Self {
            arm_a: flatten(file.arm_a_tells),
            arm_b: flatten(file.arm_b_tells),
        })
    }
}

/// The loaded package: its parsed configuration plus the content hash that pins
/// it for the campaign's lifetime.
#[derive(Clone, Debug)]
pub struct Package {
    pub campaign: Campaign,
    pub models: Models,
    pub tell_lists: TellLists,
    pub prompts: Prompts,
    /// Hex SHA-256 over every file in the package directory (see
    /// [`hash_package_dir`]). Recorded at campaign start and re-checked on resume.
    pub content_hash: String,
}

impl Package {
    /// Load and parse the package at `dir`, computing its content hash.
    pub fn load(dir: &Path) -> Result<Self> {
        let campaign: Campaign = read_json(dir, "campaign.json")?;
        campaign.validate()?;
        let models: Models = read_json(dir, "models.json")?;
        let tell_lists = TellLists::from_json(&read_file(dir, "tell-lists.json")?)?;
        let prompts: Prompts = read_json(dir, "prompts.json")?;
        prompts.validate()?;
        let content_hash = hash_package_dir(dir)?;
        Ok(Self {
            campaign,
            models,
            tell_lists,
            prompts,
            content_hash,
        })
    }

    /// The single model both arms' writers and readers run on — the parity the
    /// experiment rests on. Refuses if the writer and reader pins disagree, so a
    /// confounded pair never starts.
    pub fn single_model(&self) -> Result<&str> {
        if self.models.writer != self.models.reader {
            bail!(
                "package model pins are not single-valued: writer={} reader={} — writers and readers must share one model",
                self.models.writer,
                self.models.reader
            );
        }
        Ok(&self.models.writer)
    }

    /// Refuse if the package's current content hash does not match the hash
    /// pinned at campaign start — a run or resume against an edited package must
    /// not silently mix two designs.
    pub fn verify_pin(&self, pinned_hash: &str) -> Result<()> {
        if self.content_hash != pinned_hash {
            bail!(
                "package content hash changed since the campaign was pinned (pinned {}, now {}) — refusing to run against an edited pre-registration package",
                pinned_hash,
                self.content_hash
            );
        }
        Ok(())
    }
}

/// SHA-256 (hex) over every file directly in `dir`, folded in sorted filename
/// order so the hash is deterministic and independent of directory iteration
/// order. Each file contributes its name, a NUL separator, its byte length, and
/// its bytes, so neither a rename nor a content edit can collide. Subdirectories
/// are ignored (the package is flat).
pub fn hash_package_dir(dir: &Path) -> Result<String> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            let name = entry.file_name().to_string_lossy().into_owned();
            files.push((name, std::fs::read(entry.path())?));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = Sha256::new();
    for (name, bytes) in &files {
        hasher.update(name.as_bytes());
        hasher.update([0u8]);
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    Ok(hex(&hasher.finalize()))
}

fn read_file(dir: &Path, name: &str) -> Result<Vec<u8>> {
    let path = dir.join(name);
    std::fs::read(&path).with_context(|| format!("reading {}", path.display()))
}

fn read_json<T: serde::de::DeserializeOwned>(dir: &Path, name: &str) -> Result<T> {
    let bytes = read_file(dir, name)?;
    serde_json::from_slice(&bytes).with_context(|| format!("parsing {name}"))
}

/// Load the query battery from `queries.json` into the harness's shared
/// [`super::TaskSpec`] shape (the reader battery scores against these). Each
/// query's `prompt` is the question the reader answers; its `reference` is the
/// query's `reference_answer` — the blind judge's target. The ground-truth
/// derivation and per-class metadata in the file are not needed by the harness
/// and are ignored here.
///
/// Staged ahead of the CLI wiring that feeds it to `run_campaign`.
#[allow(dead_code)]
pub fn load_queries(dir: &Path) -> Result<Vec<super::TaskSpec>> {
    #[derive(serde::Deserialize)]
    struct QueryFile {
        queries: Vec<QueryRecord>,
    }
    #[derive(serde::Deserialize)]
    struct QueryRecord {
        id: String,
        prompt: String,
        reference_answer: String,
    }
    let file: QueryFile = read_json(dir, "queries.json")?;
    Ok(file
        .queries
        .into_iter()
        .map(|q| super::TaskSpec {
            id: q.id,
            prompt: q.prompt,
            reference: q.reference_answer,
        })
        .collect())
}

/// The mechanical round-slice digest (amendment A2): the byte-identical material
/// both arms' writers ingest for one round, assembled from `git` alone with no
/// LLM pre-summarisation. Four sections computed between the slice's boundary
/// commits (`first_commit`..`last_commit`, author-date-pinned in `slices.json`):
/// the `git log --oneline` commit subjects, the `git diff --stat` diffstat, the
/// `CHANGELOG.md` delta, and the bug-ledger delta. The same string feeds Arm A
/// and Arm B, so the digest is never an arm-distinguishing variable (criterion
/// 5's parity contract is preserved at the call site, structurally).
///
/// Range convention: changes are taken from the commit *before* `first_commit`
/// (`first_commit^`) through `last_commit`, so the slice's own first commit is
/// included and the previous slice's is not. For the repository-root slice
/// (`first_commit` has no parent) the diff base is the empty tree and the log is
/// the full ancestry of `last_commit`. kara's history is linear, so this
/// ancestry range equals the author-date window `slices.json` defines. Staged
/// ahead of the CLI wiring that feeds it to the round loop.
#[allow(dead_code)]
pub fn slice_digest(
    repo: &Path,
    first_commit: &str,
    last_commit: &str,
    changelog_path: &str,
    ledger_path: &str,
) -> Result<String> {
    // The SHA-1 empty-tree object — the diff base for the root slice, whose
    // first commit has no parent to diff against.
    const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

    let parent = format!("{first_commit}^");
    let has_parent = git(repo, &["rev-parse", "--verify", "--quiet", &parent]).is_ok();
    let base = if has_parent { &parent } else { EMPTY_TREE };
    let range = format!("{base}..{last_commit}");

    // Commit subjects for the slice's own commits. The empty tree is not a valid
    // log endpoint, so the root slice logs the full ancestry of `last_commit`.
    let log = if has_parent {
        git(repo, &["log", "--oneline", &range])?
    } else {
        git(repo, &["log", "--oneline", last_commit])?
    };
    let stat = git(repo, &["diff", "--stat", &range])?;
    let changelog = git(repo, &["diff", &range, "--", changelog_path])?;
    let ledger = git(repo, &["diff", &range, "--", ledger_path])?;

    fn or_none(body: &str) -> &str {
        let t = body.trim();
        if t.is_empty() { "(no changes)" } else { t }
    }

    Ok(format!(
        "## Round slice — {first_commit}..{last_commit}\n\n\
         ### Commit log\n{}\n\n\
         ### Diffstat\n{}\n\n\
         ### {changelog_path} changes\n{}\n\n\
         ### Bug ledger changes ({ledger_path})\n{}\n",
        or_none(&log),
        or_none(&stat),
        or_none(&changelog),
        or_none(&ledger),
    ))
}

/// Run `git -C <repo> <args>` and return stdout, or an error carrying stderr —
/// the subprocess style of `replay.rs`.
fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .with_context(|| format!("git {} in {}", args.join(" "), repo.display()))?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Vocabulary-entropy counts over a substrate — a secondary, judge-free metric
/// (reported, never band-moving). Higher counts mean a richer typed vocabulary.
#[allow(dead_code)]
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EntropyCounts {
    /// Distinct frontmatter `type:` values.
    pub distinct_types: usize,
    /// Distinct frontmatter `status:` values.
    pub distinct_status_values: usize,
    /// Distinct relationship labels (ALL-CAPS relation-type tokens).
    pub distinct_relation_labels: usize,
}

/// Count the vocabulary entropy of a substrate directory, mechanically and with
/// no judge (criterion 6). Both substrates are markdown on disk — Arm A a loose
/// directory, Arm B the mem's rendered entity files — so one pure function serves
/// both: it walks every `.md` file, reads the distinct `type:` and `status:`
/// values from each file's leading YAML frontmatter, and counts distinct
/// relationship labels as the ALL-CAPS relation-type tokens in the body (the
/// typed vocabulary Arm B emits and the untyped Arm A directory does not — so the
/// count is itself a divergence signal).
#[allow(dead_code)]
pub fn vocabulary_entropy(dir: &Path) -> Result<EntropyCounts> {
    use std::collections::BTreeSet;
    let mut types = BTreeSet::new();
    let mut statuses = BTreeSet::new();
    let mut rels = BTreeSet::new();

    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let content = std::fs::read_to_string(&path)?;

        // Leading YAML frontmatter (delimited by `---` … `---`) and the body.
        let (frontmatter, body) = match content
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("\n---"))
        {
            Some((fm, after)) => (fm, after),
            None => ("", content.as_str()),
        };
        for line in frontmatter.lines() {
            if let Some(v) = line.strip_prefix("type:") {
                types.insert(v.trim().to_string());
            } else if let Some(v) = line.strip_prefix("status:") {
                statuses.insert(v.trim().to_string());
            }
        }

        // Relationship labels: ALL-CAPS underscore tokens (REFERENCES, DEPENDS_ON).
        for tok in body.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
            let is_label = tok.len() >= 3
                && tok.chars().all(|c| c.is_ascii_uppercase() || c == '_')
                && tok.chars().any(|c| c.is_ascii_uppercase());
            if is_label {
                rels.insert(tok.to_string());
            }
        }
    }
    Ok(EntropyCounts {
        distinct_types: types.len(),
        distinct_status_values: statuses.len(),
        distinct_relation_labels: rels.len(),
    })
}

/// The item-boundary token the auditor corpus uses to separate one corpus item
/// from the next. Applied identically to both arms, so it is never an
/// arm-distinguishing tell; it carries no filename (a filename would leak the
/// substrate), only the concatenated item contents in a deterministic order. The
/// token is deliberately **whitespace-free** so it survives the judge-path
/// blinder's whitespace collapse ([`super::grade::strip_tells_with`] ends with
/// `collapse_ws`) intact — the item boundaries are still recoverable after the
/// corpus is tell-stripped exactly like a reader answer.
pub const CORPUS_ITEM_DELIM: &str = "<<<ITEM_BOUNDARY>>>";

/// Read a substrate directory into `(corpus, item_count)` for the integrity audit
/// (criterion 10): every `.md` file's full contents, concatenated in sorted
/// filename order (deterministic, arm-neutral) and joined by [`CORPUS_ITEM_DELIM`],
/// and the number of items (files for Arm A, rendered entities for Arm B — the
/// normalisation base the rubric divides defects by). Filenames are used only to
/// order the items, never emitted into the corpus, so the auditor cannot infer the
/// arm from a naming convention. The corpus is tell-stripped by the caller before
/// it reaches the auditor, exactly as the judge path blinds a reader answer.
#[allow(dead_code)]
pub fn read_corpus(dir: &Path) -> Result<(String, usize)> {
    let mut items: Vec<(String, String)> = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        items.push((name, std::fs::read_to_string(&path)?));
    }
    items.sort_by(|a, b| a.0.cmp(&b.0));
    let count = items.len();
    let corpus = items
        .into_iter()
        .map(|(_, content)| content)
        .collect::<Vec<_>>()
        .join(&format!("\n\n{CORPUS_ITEM_DELIM}\n\n"));
    Ok((corpus, count))
}

/// The integrity metric for one auditor trial: `defects = duplicates +
/// contradictions`, normalised to corpus size as `defects per 100 items`
/// (rubrics.md). An empty corpus scores 0 rather than dividing by zero.
fn defects_per_100_items(defects: usize, items: usize) -> f64 {
    if items == 0 {
        0.0
    } else {
        defects as f64 * 100.0 / items as f64
    }
}

/// The role a session played, for cost attribution in the [`Ledger`]. Staged
/// with the ledger ahead of the round-loop driver that constructs these.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Role {
    Writer,
    Reader,
    Judge,
    Auditor,
}

/// Per-arm, per-role token accounting across a campaign, carrying the hard cost
/// cap.
///
/// Every writer, reader, judge, and auditor session records its tokens here —
/// including Arm B's refusal-repair retries, which are charged to Arm B's writer
/// cost — so the cap can be checked between sessions and the ledger published
/// as-is whatever the outcome. Attribution is total by construction: [`record`]
/// takes an `arm` and a `role`, so no token source can enter the ledger
/// unattributed.
///
/// Staged ahead of its consumer: the round-loop driver records into this ledger
/// and checks the cap between sessions. Until that driver lands and the CLI wires
/// it, the ledger has no production caller, hence `allow(dead_code)`.
///
/// [`record`]: Ledger::record
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct Ledger {
    cap_tokens: u64,
    charges: Vec<Charge>,
}

#[allow(dead_code)]
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct Charge {
    arm: Arm,
    role: Role,
    /// Raw usage for this session (all four token categories) — published in the
    /// ledger for transparency.
    tokens: u64,
    /// The non-cache portion (fresh input + output) — what the cost cap counts
    /// (amendment A4).
    non_cache_tokens: u64,
}

#[allow(dead_code)]
impl Ledger {
    pub fn new(cap_tokens: u64) -> Self {
        Self {
            cap_tokens,
            charges: Vec::new(),
        }
    }

    /// Rebuild a ledger from persisted charges on resume — the cap comes from the
    /// (pin-verified) package, the charges from the campaign state file, so the
    /// running total and the published breakdown continue across a stop/resume.
    fn from_state(cap_tokens: u64, charges: Vec<Charge>) -> Self {
        Self {
            cap_tokens,
            charges,
        }
    }

    /// A clone of the recorded charges, for persisting into the campaign state so a
    /// resume can rebuild this ledger.
    fn charges_snapshot(&self) -> Vec<Charge> {
        self.charges.clone()
    }

    /// Attribute a session's `tokens` (raw) and `non_cache_tokens` to `(arm,
    /// role)`. The only way tokens enter the ledger, so every recorded token has an
    /// arm and a role. The raw figure is published; the non-cache figure is what the
    /// cap counts (amendment A4).
    pub fn record(&mut self, arm: Arm, role: Role, tokens: u64, non_cache_tokens: u64) {
        self.charges.push(Charge {
            arm,
            role,
            tokens,
            non_cache_tokens,
        });
    }

    /// Every raw token recorded, both arms, all roles — the published grand total.
    pub fn total(&self) -> u64 {
        self.charges.iter().map(|c| c.tokens).sum()
    }

    /// Every non-cache token recorded — the figure the cost cap counts (amendment
    /// A4). Always `≤ total()`.
    pub fn total_non_cache(&self) -> u64 {
        self.charges.iter().map(|c| c.non_cache_tokens).sum()
    }

    /// Every token recorded for one arm.
    pub fn total_for(&self, arm: Arm) -> u64 {
        self.charges
            .iter()
            .filter(|c| c.arm == arm)
            .map(|c| c.tokens)
            .sum()
    }

    /// Tokens recorded for one arm in one role (e.g. Arm B's writer cost, which
    /// includes its refusal-repair retries).
    pub fn total_role(&self, arm: Arm, role: Role) -> u64 {
        self.charges
            .iter()
            .filter(|c| c.arm == arm && c.role == role)
            .map(|c| c.tokens)
            .sum()
    }

    /// Would recording `next` more non-cache tokens push the running non-cache
    /// total past the cap? Checked *before* a session so the campaign can abort
    /// cleanly with its state intact for resume, rather than overspending the cap.
    /// The cap counts non-cache tokens only (amendment A4).
    pub fn would_exceed(&self, next: u64) -> bool {
        self.total_non_cache().saturating_add(next) > self.cap_tokens
    }

    /// Refuse once the recorded non-cache total has passed the cap — the
    /// between-sessions guard that aborts the campaign cleanly (state preserved for
    /// resume) rather than continuing to spend. The cap counts non-cache tokens only
    /// (amendment A4); the raw total is reported alongside for context.
    pub fn check_cap(&self) -> Result<()> {
        let non_cache = self.total_non_cache();
        if non_cache > self.cap_tokens {
            bail!(
                "campaign cost cap exceeded: {non_cache} non-cache tokens spent ({} raw), cap {} — aborting (state preserved for resume)",
                self.total(),
                self.cap_tokens
            );
        }
        Ok(())
    }

    /// A serialisable breakdown of the cost book for the published campaign
    /// result — the ledger's own fields are private, so this is how it leaves the
    /// harness.
    pub fn summary(&self) -> LedgerSummary {
        let total = self.total();
        let non_cache = self.total_non_cache();
        LedgerSummary {
            total_tokens: total,
            non_cache_tokens: non_cache,
            cache_tokens: total.saturating_sub(non_cache),
            cap_tokens: self.cap_tokens,
            arm_a_writer: self.total_role(Arm::A, Role::Writer),
            arm_a_reader: self.total_role(Arm::A, Role::Reader),
            arm_b_writer: self.total_role(Arm::B, Role::Writer),
            arm_b_reader: self.total_role(Arm::B, Role::Reader),
            judge: self.total_role(Arm::A, Role::Judge) + self.total_role(Arm::B, Role::Judge),
            auditor: self.total_role(Arm::A, Role::Auditor)
                + self.total_role(Arm::B, Role::Auditor),
        }
    }
}

/// The published, serialisable form of the cost book: totals against the cap and
/// the per-arm/role breakdown, including Arm B's refusal-repair retries inside
/// `arm_b_writer`.
#[allow(dead_code)]
#[derive(Clone, Debug, serde::Serialize)]
pub struct LedgerSummary {
    /// Raw grand total across both arms and all roles (all four token categories).
    pub total_tokens: u64,
    /// The non-cache portion of `total_tokens` (fresh input + output) — the figure
    /// the cost cap counts (amendment A4). `cap_tokens` bounds this, not
    /// `total_tokens`.
    pub non_cache_tokens: u64,
    /// The cache portion (`total_tokens − non_cache_tokens`) — published for
    /// transparency but not counted by the cap (amendment A4).
    pub cache_tokens: u64,
    /// The non-cache cost cap (amendment A4: 40,000,000).
    pub cap_tokens: u64,
    pub arm_a_writer: u64,
    pub arm_a_reader: u64,
    pub arm_b_writer: u64,
    pub arm_b_reader: u64,
    pub judge: u64,
    pub auditor: u64,
}

/// What a writer session produced: the tokens it spent, the tools it called, and
/// the model it actually ran on. The tool calls are the criterion-5 evidence that
/// Arm B's writes really crossed the MCP mutation surface; `executed_model` is
/// criterion 3's refusal complement — the driver invalidates the round if the
/// session ran on a model other than the pin ([`ensure_model_honored`]).
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct WriterOutcome {
    pub tokens: u64,
    /// The non-cache portion of `tokens` — what the cost cap counts (amendment A4).
    pub non_cache_tokens: u64,
    pub tool_calls: Vec<String>,
    pub executed_model: String,
}

/// What a reader session produced: the answer text (blinded before the judge
/// sees it), the tokens spent, the tools called, and the model it ran on
/// (criterion 3's refusal complement, as for [`WriterOutcome`]).
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct ReaderOutcome {
    pub answer: String,
    pub tokens: u64,
    /// The non-cache portion of `tokens` — what the cost cap counts (amendment A4).
    pub non_cache_tokens: u64,
    pub tool_calls: Vec<String>,
    pub executed_model: String,
}

/// What a judge session produced: the score, its token cost (criterion 7), and
/// the model it ran on. `executed_model` carries criterion 3's refusal complement
/// onto the judge session too — a judge that silently ran on an ambient model
/// invalidates the round ([`ensure_model_honored`]).
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct JudgeOutcome {
    pub score: f64,
    pub tokens: u64,
    /// The non-cache portion of `tokens` — what the cost cap counts (amendment A4).
    pub non_cache_tokens: u64,
    pub executed_model: String,
}

/// What one integrity-auditor trial produced (criterion 10): the two defect counts
/// under the package's counting rubric, the tokens it spent (charged to
/// [`Role::Auditor`]), and the model it ran on. `executed_model` carries criterion
/// 3's refusal complement onto the auditor session too — an auditor that ran on an
/// ambient model invalidates its trial ([`ensure_model_honored`]) rather than
/// contributing a silent count.
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct AuditOutcome {
    pub duplicates: usize,
    pub contradictions: usize,
    pub tokens: u64,
    /// The non-cache portion of `tokens` — what the cost cap counts (amendment A4).
    pub non_cache_tokens: u64,
    pub executed_model: String,
}

/// What one `claude -p` session produced, parsed from its stream-json: the
/// answer text, the tools it called, the tokens it spent, and the model it ran
/// on. The real runner maps this onto a [`WriterOutcome`] (tokens + tool calls +
/// model), a [`ReaderOutcome`] (all four), or a [`JudgeOutcome`].
#[allow(dead_code)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionOutput {
    pub text: String,
    pub tool_calls: Vec<String>,
    /// Raw grand-total usage: fresh input + output + both cache categories. This
    /// is what the ledger publishes for transparency.
    pub tokens: u64,
    /// The non-cache portion of `tokens`: fresh `input_tokens + output_tokens`
    /// only, excluding `cache_read_input_tokens` and `cache_creation_input_tokens`.
    /// This is what the cost cap counts (amendment A4, 2026-07-14): cache reads
    /// cost ~a tenth of fresh tokens, so a raw-count cap fires ~10× too early
    /// relative to real cost. `non_cache_tokens ≤ tokens` always.
    pub non_cache_tokens: u64,
    /// The model the session reported running on — read from the stream's
    /// `model` field (the assistant `message.model`, or a top-level `model` on
    /// the `system`/`result` events). Feeds the outcome's `executed_model` so the
    /// pin-honor guard can invalidate a round that ran on the wrong model.
    pub model: String,
}

/// Parse a `claude --output-format stream-json --verbose` NDJSON stream into a
/// [`SessionOutput`]. Like `claude.rs::parse_stream_json` it collects assistant
/// `text` and `tool_use` items, but it additionally sums the `usage` token counts
/// off the final `result` event — the mount/substrate modes never needed tokens,
/// but the divergence ledger does. `usage` totals input + output (+ cache) tokens
/// where present; unparseable lines are skipped, as the stream interleaves
/// `system` and rate-limit events.
///
/// Staged ahead of the real runner that calls it on each session's output.
#[allow(dead_code)]
pub fn parse_session(stdout: &str) -> Result<SessionOutput> {
    let mut texts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<String> = Vec::new();
    let mut result_text: Option<String> = None;
    // The `result` event carries the session's grand-total usage on a clean run.
    // We track the raw total (all four categories) and its non-cache portion (fresh
    // input + output) in lockstep — the ledger publishes the raw total but the cost
    // cap counts only the non-cache portion (amendment A4).
    let mut result_tokens: u64 = 0;
    let mut result_non_cache: u64 = 0;
    // Assistant events carry a running cumulative usage snapshot. On a
    // `--max-budget-usd` cutoff (amendment A1) the `result` event errors with zero
    // usage, so the last assistant snapshot is the only record of what was spent —
    // taking the snapshot with the largest raw total recovers a budget-cut session's
    // real cost for the ledger. On a clean run the result total dominates. We keep
    // the non-cache figure of the *same* snapshot we take the raw total from, so the
    // two never mix across sources.
    let mut max_assistant_tokens: u64 = 0;
    let mut max_assistant_non_cache: u64 = 0;
    let mut model: Option<String> = None;

    // Returns `(raw, non_cache)` for a usage block: raw is all four token
    // categories; non_cache is fresh `input_tokens + output_tokens` only. Cache
    // reads/creations are excluded from non_cache because they cost a fraction of
    // fresh tokens (amendment A4).
    let split_usage = |usage: &serde_json::Map<String, serde_json::Value>| -> (u64, u64) {
        let get = |key: &str| usage.get(key).and_then(|t| t.as_u64()).unwrap_or(0);
        let non_cache = get("input_tokens") + get("output_tokens");
        let raw = non_cache + get("cache_creation_input_tokens") + get("cache_read_input_tokens");
        (raw, non_cache)
    };

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        // The executed model appears as a top-level `model` (system/result
        // events) or nested under `message.model` (assistant events). Capture the
        // first one seen — the pin-honor guard compares it to the requested pin.
        if model.is_none() {
            let found = v.get("model").and_then(|m| m.as_str()).or_else(|| {
                v.get("message")
                    .and_then(|m| m.get("model"))
                    .and_then(|m| m.as_str())
            });
            if let Some(m) = found {
                model = Some(m.to_string());
            }
        }
        match v.get("type").and_then(|t| t.as_str()) {
            Some("assistant") => {
                if let Some(usage) = v
                    .get("message")
                    .and_then(|m| m.get("usage"))
                    .and_then(|u| u.as_object())
                {
                    let (raw, non_cache) = split_usage(usage);
                    if raw > max_assistant_tokens {
                        max_assistant_tokens = raw;
                        max_assistant_non_cache = non_cache;
                    }
                }
                if let Some(content) = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    for item in content {
                        match item.get("type").and_then(|t| t.as_str()) {
                            Some("text") => {
                                if let Some(t) = item.get("text").and_then(|t| t.as_str())
                                    && !t.is_empty()
                                {
                                    texts.push(t.to_string());
                                }
                            }
                            Some("tool_use") => {
                                if let Some(n) = item.get("name").and_then(|n| n.as_str()) {
                                    tool_calls.push(n.to_string());
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            Some("result") => {
                if let Some(r) = v.get("result").and_then(|r| r.as_str()) {
                    result_text = Some(r.to_string());
                }
                if let Some(usage) = v.get("usage").and_then(|u| u.as_object()) {
                    let (raw, non_cache) = split_usage(usage);
                    result_tokens += raw;
                    result_non_cache += non_cache;
                }
            }
            _ => {}
        }
    }

    let text = if texts.is_empty() {
        result_text.unwrap_or_default()
    } else {
        texts.join("\n")
    };
    // The result-event grand total on a clean run; the last assistant snapshot when
    // a budget cutoff zeroed the result usage. We pick the snapshot with the larger
    // raw total and take *its* non-cache figure, so raw and non-cache never come
    // from different sources.
    let (tokens, non_cache_tokens) = if result_tokens >= max_assistant_tokens {
        (result_tokens, result_non_cache)
    } else {
        (max_assistant_tokens, max_assistant_non_cache)
    };
    Ok(SessionOutput {
        text,
        tool_calls,
        tokens,
        non_cache_tokens,
        model: model.unwrap_or_default(),
    })
}

/// The `claude -p` flags every divergence session shares: stream-json output (so
/// [`parse_session`] can read it), the pinned model, non-interactive permission,
/// and strict MCP config. The per-session prompt and the per-arm tool/MCP flags
/// are added by the arg-builders.
///
/// `budget_usd`, when set, adds `--max-budget-usd` — the proportional cost budget
/// that operationalises the pre-registered token allowance (amendment A1). Both
/// arms receive the flag identically per round; only the substrate access surface
/// differs (criterion 5), so the budget is not an arm-distinguishing variable.
/// The prompt is NOT among these args — it is fed to the session over **stdin** by
/// [`spawn_claude_session`]. Passing a large prompt as a command-line argument hit
/// the OS `ARG_MAX` limit (`E2BIG`/"Argument list too long", observed at round 9
/// where an accumulated corpus / slice digest grew large); stdin has no such limit.
/// `claude -p` with no positional prompt reads it from stdin (verified 2026-07-15).
#[allow(dead_code)]
fn base_session_args(model: &str, budget_usd: Option<f64>) -> Vec<String> {
    let mut args = vec![
        "-p".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--model".to_string(),
        model.to_string(),
        "--permission-mode".to_string(),
        "dontAsk".to_string(),
        "--strict-mcp-config".to_string(),
    ];
    if let Some(budget) = budget_usd {
        args.push("--max-budget-usd".to_string());
        args.push(format!("{budget:.4}"));
    }
    args
}

/// Build the `claude -p` argument vector for a **writer** session.
///
/// The only difference between the arms is the substrate access surface — the
/// treatment under test: Arm A writes markdown files with the filesystem tools;
/// Arm B mutates the mem through `mcp__memstead__*` over the supplied MCP config.
/// The pinned `model` is passed explicitly in every case (criterion 3). The
/// writer allowance is operationalised as `budget_usd` (amendment A1) — a
/// proportional `--max-budget-usd` cap computed by the caller via
/// [`Campaign::budget_usd`]; `claude -p` cannot cap a session's output tokens
/// directly (confirmed 2026-07-14). Pass `None` to omit the cap (the documentary
/// fallback of amendment A1).
#[allow(dead_code)]
fn build_writer_args(
    arm: Arm,
    model: &str,
    budget_usd: Option<f64>,
    mcp_config: Option<&Path>,
) -> Vec<String> {
    let mut args = base_session_args(model, budget_usd);
    args.push("--allowedTools".to_string());
    match (arm, mcp_config) {
        // Arm A: filesystem write tools, no MCP.
        (Arm::A, _) => {
            args.push("Read,Write,Edit,MultiEdit,Grep,Glob,LS".to_string());
        }
        // Arm B: the full memstead mutation surface, over the mem's MCP config.
        (Arm::B, Some(cfg)) => {
            args.push("mcp__memstead__*".to_string());
            args.push("--mcp-config".to_string());
            args.push(cfg.display().to_string());
        }
        // Arm B with no MCP config is a misconfiguration — the real runner
        // provisions the mem and supplies it before calling this.
        (Arm::B, None) => {
            args.push(String::new());
        }
    }
    args
}

/// Build the `claude -p` argument vector for a **reader** session — read-only
/// access to the arm's substrate. Arm A gets filesystem *read* tools (no
/// Write/Edit); Arm B gets only the memstead *read* tools (overview / search /
/// entity — never the mutation tools). Pinned `model` explicit, as for writers;
/// `budget_usd` is the reader's proportional `--max-budget-usd` cap (from the
/// fixed reader budget via [`Campaign::budget_usd`]), or `None` to omit it.
#[allow(dead_code)]
fn build_reader_args(
    arm: Arm,
    model: &str,
    budget_usd: Option<f64>,
    mcp_config: Option<&Path>,
) -> Vec<String> {
    let mut args = base_session_args(model, budget_usd);
    args.push("--allowedTools".to_string());
    match (arm, mcp_config) {
        (Arm::A, _) => {
            args.push("Read,Grep,Glob,LS".to_string());
        }
        (Arm::B, Some(cfg)) => {
            args.push(
                "mcp__memstead__memstead_overview,mcp__memstead__memstead_search,mcp__memstead__memstead_entity"
                    .to_string(),
            );
            args.push("--mcp-config".to_string());
            args.push(cfg.display().to_string());
        }
        (Arm::B, None) => {
            args.push(String::new());
        }
    }
    args
}

/// Drives writer and reader sessions against each arm's materialised substrate.
/// The real impl shells to `claude -p` with the pinned model against a temp
/// markdown directory (Arm A) or a throwaway mem over MCP (Arm B); tests use a
/// deterministic stub, so the loop, the evidence guard, the ledger, and the cost
/// cap are all verified without a network call.
#[allow(dead_code)]
pub trait DivergenceRunner {
    /// One writer session for `arm`, invoked with the pinned `model` explicitly
    /// (criterion 3) and the round's token allowance. The session mutates the
    /// arm's substrate as a side effect; the return value reports its cost and
    /// tool calls.
    fn write(
        &self,
        arm: Arm,
        model: &str,
        prompt: &str,
        token_allowance: usize,
    ) -> Result<WriterOutcome>;

    /// One reader session for `arm`, invoked with the pinned `model` and the fixed
    /// reader budget. Answers the query from the arm's substrate; the answer is
    /// blinded ([`super::grade::strip_tells_with`]) before it reaches the judge.
    fn read(
        &self,
        arm: Arm,
        model: &str,
        prompt: &str,
        token_budget: usize,
    ) -> Result<ReaderOutcome>;

    /// The on-disk directory holding `arm`'s current substrate (Arm A the loose
    /// markdown directory, Arm B the mem's rendered entity files). The loop reads
    /// it to compute [`vocabulary_entropy`] after each round.
    fn substrate_dir(&self, arm: Arm) -> &Path;
}

/// Scores a blinded answer against a reference and reports its own token cost, so
/// the judge's tokens enter the ledger (criterion 7). The mount/substrate modes'
/// [`super::Judge`] reports no tokens; the divergence campaign needs them, hence
/// this parallel trait. The judge is invoked with the pinned `model` explicitly
/// (criterion 3) and reports the model it ran on in [`JudgeOutcome`], so a judge
/// that could not honor the pin invalidates its round like any other session.
#[allow(dead_code)]
pub trait DivergenceJudge {
    fn score(&self, model: &str, reference: &str, blinded_answer: &str) -> Result<JudgeOutcome>;
}

/// Scores one arm's whole corpus for internal defects under the package's counting
/// rubric and reports its own token cost (criterion 10, the integrity co-primary
/// endpoint). Like [`DivergenceJudge`], it is a parallel trait to the mount-mode
/// judge because it reports tokens and an executed model. The `auditor_prompt` it
/// receives is the arm-neutral auditor skeleton assembled around the arm's
/// already-tell-stripped corpus (no arm label anywhere), so the auditor scores
/// blind — it is invoked with the pinned `model` explicitly (criterion 3) and
/// reports the model it ran on so a trial that could not honor the pin invalidates
/// rather than counting as zero.
#[allow(dead_code)]
pub trait DivergenceAuditor {
    fn audit(&self, model: &str, auditor_prompt: &str) -> Result<AuditOutcome>;
}

/// Criterion 3's refusal complement: a session that ran on a model other than
/// its pinned `requested` id invalidates its round rather than silently
/// contributing an ambient-model result. Applied to every writer, reader, and
/// judge session after it returns, comparing the model it reported running on
/// (`executed`) against the pin the subprocess was invoked with.
#[allow(dead_code)]
fn ensure_model_honored(role: &str, requested: &str, executed: &str) -> Result<()> {
    if executed != requested {
        bail!(
            "{role} session ran on model {executed:?}, not the pinned {requested:?} — the pin was not honored; round invalid"
        );
    }
    Ok(())
}

/// Criterion 5: a writer session for Arm B must show at least one `memstead_*`
/// mutation call — proof its write crossed the engine's gate rather than touching
/// disk directly. A round where Arm B wrote without any mutation call is invalid.
/// Arm A (the tolerant directory) carries no such requirement.
#[allow(dead_code)]
pub fn validate_writer_evidence(arm: Arm, tool_calls: &[String]) -> Result<()> {
    if arm == Arm::B {
        let mutated = tool_calls.iter().any(|t| {
            let t = t.to_lowercase();
            ["memstead_create", "memstead_update", "memstead_relate"]
                .iter()
                .any(|m| t.contains(m))
        });
        if !mutated {
            bail!(
                "Arm B writer session made no memstead_* mutation call — the write did not cross the MCP gate; round invalid"
            );
        }
    }
    Ok(())
}

/// Drive every writer round of the campaign: for each round in the schedule, run
/// one writer session per arm with that round's slice and allowance, validate the
/// MCP-mutation evidence, record the cost, and check the cap between sessions.
/// Returns the accumulated ledger (writer costs). The substrates are mutated in
/// place by the runner; `slices[i]` is round `i+1`'s source content.
///
/// Staged ahead of the reader battery and the CLI wiring; the full campaign
/// driver composes this with the reader checkpoints.
#[allow(dead_code)]
pub fn run_writer_rounds<R: DivergenceRunner>(
    runner: &R,
    package: &Package,
    slices: &[String],
) -> Result<Ledger> {
    let model = package.single_model()?.to_string();
    let schedule = package.campaign.schedule();
    require_slice_count(slices.len(), schedule.len())?;
    let mut ledger = Ledger::new(package.campaign.cost_cap_tokens);
    for rp in &schedule {
        drive_writers(
            runner,
            package,
            &model,
            rp,
            &slices[rp.round - 1],
            &mut ledger,
        )?;
    }
    Ok(ledger)
}

fn require_slice_count(got: usize, want: usize) -> Result<()> {
    if got != want {
        bail!("expected {want} round slices, got {got}");
    }
    Ok(())
}

/// One round's writer phase: a session per arm, evidence-checked, billed, and
/// cap-checked. Shared by [`run_writer_rounds`] and [`run_campaign`] so both
/// drive writers identically.
fn drive_writers<R: DivergenceRunner>(
    runner: &R,
    package: &Package,
    model: &str,
    rp: &RoundPlan,
    slice: &str,
    ledger: &mut Ledger,
) -> Result<()> {
    for arm in [Arm::A, Arm::B] {
        let prompt = package.prompts.writer(arm, rp.hurry, slice);
        let out = runner.write(arm, model, &prompt, rp.writer_allowance_tokens)?;
        ensure_model_honored("writer", model, &out.executed_model)?;
        validate_writer_evidence(arm, &out.tool_calls)?;
        ledger.record(arm, Role::Writer, out.tokens, out.non_cache_tokens);
        ledger.check_cap()?;
    }
    Ok(())
}

/// One reader checkpoint's scored results: per query, `trials` reader sessions per
/// arm, blinded and judged, aggregated into a signed `B − A` delta per query.
#[allow(dead_code)]
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Checkpoint {
    pub round: usize,
    pub results: Vec<super::TaskResult>,
}

/// One integrity-audit checkpoint's scored result (criterion 10): `trials`
/// blinded auditor sessions per arm score the corpus for defects, normalised to
/// `defects per 100 items`, aggregated with the same mean/stderr treatment as the
/// reader battery. The delta orientation is **A − B** (tolerant minus engine-gated,
/// so a positive delta means Arm A is dirtier — enforcement kept Arm B cleaner),
/// mirroring the package's integrity band, and opposite the accuracy checkpoint's
/// `B − A`. `arm_a_items` / `arm_b_items` publish the normalisation base.
#[allow(dead_code)]
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct IntegrityCheckpoint {
    pub round: usize,
    pub arm_a_items: usize,
    pub arm_b_items: usize,
    /// `on` = Arm A per-trial defects-per-100, `off` = Arm B, so `delta = A − B`.
    pub result: super::TaskResult,
}

/// One round's vocabulary-entropy sample for both arms — the secondary,
/// judge-free divergence signal, computed from substrate bytes after the round's
/// writers ran.
#[allow(dead_code)]
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RoundEntropy {
    pub round: usize,
    pub arm_a: EntropyCounts,
    pub arm_b: EntropyCounts,
}

/// The whole campaign's output: the per-checkpoint scored results, the per-round
/// entropy series, and the cost ledger. The per-query delta orientation is
/// `B − A` (engine-gated minus tolerant), matching the package's accuracy band.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct CampaignResult {
    pub checkpoints: Vec<Checkpoint>,
    /// The integrity co-primary endpoint (criterion 10): one entry per audit
    /// checkpoint. Every entry is produced by real auditor sessions — there is no
    /// code path that fabricates a zero-filled integrity delta, so a scheduled
    /// audit that produced no result refuses the whole campaign rather than
    /// emitting silent zeros (see [`run_campaign`]).
    pub integrity_checkpoints: Vec<IntegrityCheckpoint>,
    pub entropy_series: Vec<RoundEntropy>,
    pub ledger: Ledger,
}

/// The published, serialisable campaign artifact: the per-checkpoint scored
/// results, the per-round entropy series, and the cost book. `Ledger` itself is
/// not serialisable (private fields), so it enters as its [`LedgerSummary`].
#[allow(dead_code)]
#[derive(serde::Serialize)]
pub struct CampaignReport<'a> {
    pub checkpoints: &'a [Checkpoint],
    pub integrity_checkpoints: &'a [IntegrityCheckpoint],
    pub entropy_series: &'a [RoundEntropy],
    pub cost: LedgerSummary,
}

impl CampaignResult {
    /// The serialisable view of this result.
    #[allow(dead_code)]
    pub fn report(&self) -> CampaignReport<'_> {
        CampaignReport {
            checkpoints: &self.checkpoints,
            integrity_checkpoints: &self.integrity_checkpoints,
            entropy_series: &self.entropy_series,
            cost: self.ledger.summary(),
        }
    }

    /// Serialise the result to pretty JSON — the campaign artifact plan 03
    /// publishes.
    #[allow(dead_code)]
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(&self.report())?)
    }
}

/// Resume state, persisted between rounds so a killed campaign continues without
/// re-running finished writer rounds. It pins the package by the content hash
/// recorded at campaign start: a resume against an edited package refuses
/// (criterion 2's refusal complement) rather than silently mixing two designs.
#[allow(dead_code)]
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct CampaignState {
    pub pinned_hash: String,
    /// Highest round whose writer phase (and any checkpoint) fully completed.
    pub completed_rounds: usize,
    /// The full result accumulators, persisted after every completed round so a
    /// stop/resume rebuilds them rather than losing every prior round's scores. Only
    /// the counter used to persist, which made a resumed campaign emit an
    /// *incomplete* `result.json` (missing the rounds it skipped). Persisting these
    /// makes resume produce the same result as one uninterrupted run.
    #[serde(default)]
    checkpoints: Vec<Checkpoint>,
    #[serde(default)]
    integrity_checkpoints: Vec<IntegrityCheckpoint>,
    #[serde(default)]
    entropy_series: Vec<RoundEntropy>,
    /// The cost ledger's charges (the ledger is rebuilt from these + the package
    /// cap on resume), so the running total and cap enforcement carry across.
    #[serde(default)]
    charges: Vec<Charge>,
}

#[allow(dead_code)]
impl CampaignState {
    fn load(path: &Path) -> Result<Self> {
        serde_json::from_slice(&std::fs::read(path)?)
            .with_context(|| format!("reading campaign state {}", path.display()))
    }

    fn save(&self, path: &Path) -> Result<()> {
        std::fs::write(path, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("writing campaign state {}", path.display()))
    }
}

/// A published **partial** campaign artifact, built from the persisted state of an
/// incomplete run (e.g. one that ran out of usage before round 10). It carries the
/// real scored deltas for the rounds that completed — the same numbers a full run
/// would report for those rounds — but is explicitly marked partial and carries **no
/// band verdict**: the pre-registration binds a band verdict to a completed campaign
/// only, and a partial one never receives one. This exists so an interrupted
/// campaign still yields genuine, usable evidence rather than nothing.
#[derive(serde::Serialize)]
pub struct PartialReport<'a> {
    /// Always true — a marker so no downstream reader mistakes this for a full result.
    pub partial: bool,
    /// How many rounds completed (of the scheduled total). The reader checkpoints and
    /// integrity audits present below are exactly those whose round completed.
    pub completed_rounds: usize,
    pub note: String,
    #[serde(flatten)]
    pub report: CampaignReport<'a>,
}

/// Build the partial-artifact JSON from a persisted campaign state file. Reads the
/// accumulators saved after each completed round and reconstructs the same report
/// shape a full run emits, wrapped in the partial marker. No band verdict is
/// computed here (or anywhere for a partial) — the caller publishes this under a
/// `partial/` path per the pre-registration.
#[allow(dead_code)]
pub fn partial_report_json(state_path: &Path, cap_tokens: u64) -> Result<String> {
    let state = CampaignState::load(state_path)?;
    let ledger = Ledger::from_state(cap_tokens, state.charges);
    let report = CampaignReport {
        checkpoints: &state.checkpoints,
        integrity_checkpoints: &state.integrity_checkpoints,
        entropy_series: &state.entropy_series,
        cost: ledger.summary(),
    };
    let partial = PartialReport {
        partial: true,
        completed_rounds: state.completed_rounds,
        note: "Incomplete campaign — NO band verdict. Per the pre-registration a band \
               verdict binds only a completed campaign; this artifact reports the real \
               scored deltas for the rounds that finished and nothing more."
            .to_string(),
        report,
    };
    serde_json::to_string_pretty(&partial).context("serialising partial campaign report")
}

/// Run a fallible session `attempts` times, returning the first success. A single
/// `claude` session occasionally produces output the strict parsers reject — an
/// auditor that omits its `CONTRADICTIONS:` marker, a judge whose score line is
/// malformed — and without a retry that one flaky session aborts the entire
/// multi-hour campaign (observed: round 5 died on a missing auditor marker after
/// ~4.5 h). Retrying re-runs only the *stateless* sessions (reader/judge/auditor —
/// never a writer, whose mem mutations would duplicate on a re-run); it takes the
/// first *parseable* result, so it fixes a format failure without ever selecting on
/// the value (both arms retry identically — no bias). If every attempt fails the
/// last error propagates, so a genuinely broken session still aborts. Each retry
/// spends tokens; the count is kept small.
fn with_retries<T>(
    label: &str,
    attempts: usize,
    mut f: impl FnMut() -> Result<T>,
) -> Result<T> {
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 1..=attempts {
        match f() {
            Ok(v) => return Ok(v),
            Err(e) => {
                eprintln!("{label} session attempt {attempt}/{attempts} failed: {e}");
                last_err = Some(e);
            }
        }
    }
    Err(last_err.expect("attempts >= 1, so at least one error was recorded"))
}

/// Drive the full campaign on `slices` (one per round) and `queries` (the reader
/// battery): every round runs its writer sessions; at each reader checkpoint the
/// battery runs `trials` blinded, judged reader sessions per arm per query. One
/// ledger spans writers, readers, and judges; the cost cap is checked between
/// sessions throughout. Fully driven by the runner/judge traits, so a stub
/// exercises the whole loop without a network call (criterion 1).
/// `state_path`, when given, makes the campaign resumable: the **full result**
/// (checkpoints, integrity, entropy, ledger) is persisted after every round, and a
/// restart with the same path rebuilds those accumulators and continues after the
/// last completed round (refusing if the package was edited since it was pinned), so
/// a resumed campaign yields the same complete result as one uninterrupted run.
/// Passing `None` runs a fresh, non-resumable campaign.
///
/// **Graceful stop:** if a file named `STOP` exists in the state directory at a
/// round boundary, the campaign persists its progress, deletes the marker, and
/// returns `Ok(None)` — a clean pause (never an error, never a partial result), safe
/// because the substrate then reflects exactly the completed rounds. Re-running the
/// same command resumes. A full completion returns `Ok(Some(result))`.
#[allow(dead_code)]
pub fn run_campaign<R: DivergenceRunner, J: DivergenceJudge, A: DivergenceAuditor>(
    runner: &R,
    judge: &J,
    auditor: &A,
    package: &Package,
    slices: &[String],
    queries: &[super::TaskSpec],
    state_path: Option<&Path>,
) -> Result<Option<CampaignResult>> {
    let model = package.single_model()?.to_string();
    // The judge is pinned to its own model id from the package (criterion 3:
    // every subprocess is invoked with its pinned model explicitly). It happens
    // to equal the writer/reader pin in the committed package, but it is read
    // from `models.judge` rather than assumed equal.
    let judge_model = package.models.judge.clone();
    // The auditor is pinned likewise (criterion 10 reuses criterion 3's posture).
    let auditor_model = package.models.auditor.clone();
    let schedule = package.campaign.schedule();
    require_slice_count(slices.len(), schedule.len())?;
    let tells = package.tell_lists.combined();
    let cap = package.campaign.cost_cap_tokens;

    // Determine where to start and rebuild the accumulators. A resume verifies the
    // package pin and reloads every prior round's scores + the ledger from the state
    // file, then picks up after the last completed round; a fresh run seeds an empty
    // state at round 0. The `STOP` marker lives in the state directory.
    let stop_marker = state_path.and_then(|p| p.parent()).map(|d| d.join("STOP"));
    let (start_round, mut checkpoints, mut integrity_checkpoints, mut entropy_series, mut ledger) =
        match state_path {
            Some(path) if path.exists() => {
                let state = CampaignState::load(path)?;
                package.verify_pin(&state.pinned_hash)?;
                (
                    state.completed_rounds + 1,
                    state.checkpoints,
                    state.integrity_checkpoints,
                    state.entropy_series,
                    Ledger::from_state(cap, state.charges),
                )
            }
            _ => {
                let fresh = CampaignState {
                    pinned_hash: package.content_hash.clone(),
                    completed_rounds: 0,
                    checkpoints: Vec::new(),
                    integrity_checkpoints: Vec::new(),
                    entropy_series: Vec::new(),
                    charges: Vec::new(),
                };
                if let Some(path) = state_path {
                    fresh.save(path)?;
                }
                (
                    1,
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Ledger::new(cap),
                )
            }
        };
    // Audits already present from a prior session (reloaded above); the final
    // completeness check counts these plus the audits this session runs.
    let reloaded_audits = integrity_checkpoints.len();

    for rp in &schedule {
        if rp.round < start_round {
            continue;
        }
        // Graceful stop at a round boundary: the substrate reflects exactly the
        // completed rounds here, so a resume re-runs the *next* round's writers
        // cleanly (no half-written round). Consume the marker and pause.
        if let Some(marker) = &stop_marker
            && marker.exists()
        {
            let _ = std::fs::remove_file(marker);
            eprintln!(
                "STOP marker found — pausing cleanly after round {}. Re-run the same command to resume.",
                rp.round - 1
            );
            return Ok(None);
        }
        drive_writers(
            runner,
            package,
            &model,
            rp,
            &slices[rp.round - 1],
            &mut ledger,
        )?;
        // Vocabulary entropy from each arm's substrate bytes, after this round's
        // writers ran — the secondary divergence signal (criterion 6).
        entropy_series.push(RoundEntropy {
            round: rp.round,
            arm_a: vocabulary_entropy(runner.substrate_dir(Arm::A))?,
            arm_b: vocabulary_entropy(runner.substrate_dir(Arm::B))?,
        });
        if rp.reader_checkpoint {
            let mut results = Vec::with_capacity(queries.len());
            for query in queries {
                let mut b_scores = Vec::with_capacity(package.campaign.trials);
                let mut a_scores = Vec::with_capacity(package.campaign.trials);
                for _ in 0..package.campaign.trials {
                    for arm in [Arm::A, Arm::B] {
                        let prompt = package.prompts.reader(arm, &query.prompt);
                        let out = with_retries("reader", 3, || {
                            runner.read(
                                arm,
                                &model,
                                &prompt,
                                package.campaign.reader_budget_tokens,
                            )
                        })?;
                        ensure_model_honored("reader", &model, &out.executed_model)?;
                        ledger.record(arm, Role::Reader, out.tokens, out.non_cache_tokens);
                        let blinded = super::grade::strip_tells_with(&out.answer, &tells);
                        let jout = with_retries("judge", 3, || {
                            judge.score(&judge_model, &query.reference, &blinded)
                        })?;
                        ensure_model_honored("judge", &judge_model, &jout.executed_model)?;
                        ledger.record(arm, Role::Judge, jout.tokens, jout.non_cache_tokens);
                        ledger.check_cap()?;
                        match arm {
                            Arm::A => a_scores.push(jout.score),
                            Arm::B => b_scores.push(jout.score),
                        }
                    }
                }
                // on = B, off = A, so TaskResult.delta = on_mean - off_mean = B - A.
                results.push(super::TaskResult::new(query.id.clone(), b_scores, a_scores));
            }
            checkpoints.push(Checkpoint {
                round: rp.round,
                results,
            });
        }
        if rp.integrity_audit {
            // Blinded integrity audit (criterion 10, co-primary endpoint). Per
            // arm: read the corpus once, tell-strip it exactly like the judge
            // path (so arm identity never reaches the auditor), then run `trials`
            // auditor sessions and normalise each to defects-per-100-items.
            let mut a_defects = Vec::with_capacity(package.campaign.trials);
            let mut b_defects = Vec::with_capacity(package.campaign.trials);
            let mut a_items = 0usize;
            let mut b_items = 0usize;
            for arm in [Arm::A, Arm::B] {
                let (corpus, items) = read_corpus(runner.substrate_dir(arm))?;
                let blinded = super::grade::strip_tells_with(&corpus, &tells);
                // Assemble the arm-neutral auditor prompt around the tell-stripped
                // corpus — the harness owns prompt assembly here as it does for the
                // writer/reader paths.
                let prompt = package.prompts.auditor(&blinded);
                match arm {
                    Arm::A => a_items = items,
                    Arm::B => b_items = items,
                }
                for _ in 0..package.campaign.trials {
                    let out =
                        with_retries("auditor", 3, || auditor.audit(&auditor_model, &prompt))?;
                    ensure_model_honored("auditor", &auditor_model, &out.executed_model)?;
                    ledger.record(arm, Role::Auditor, out.tokens, out.non_cache_tokens);
                    ledger.check_cap()?;
                    let dp100 = defects_per_100_items(out.duplicates + out.contradictions, items);
                    match arm {
                        Arm::A => a_defects.push(dp100),
                        Arm::B => b_defects.push(dp100),
                    }
                }
            }
            // on = A, off = B, so TaskResult.delta = A − B (higher = Arm A dirtier).
            let result =
                super::TaskResult::new(format!("integrity@{}", rp.round), a_defects, b_defects);
            integrity_checkpoints.push(IntegrityCheckpoint {
                round: rp.round,
                arm_a_items: a_items,
                arm_b_items: b_items,
                result,
            });
        }
        // Persist the full progress so a stop/resume after this round rebuilds every
        // prior round's scores and the ledger — not just the round counter.
        if let Some(path) = state_path {
            CampaignState {
                pinned_hash: package.content_hash.clone(),
                completed_rounds: rp.round,
                checkpoints: checkpoints.clone(),
                integrity_checkpoints: integrity_checkpoints.clone(),
                entropy_series: entropy_series.clone(),
                charges: ledger.charges_snapshot(),
            }
            .save(path)?;
        }
    }
    // Criterion 10's refusal complement: every audit checkpoint executed this run
    // must have produced an integrity result. Integrity checkpoints are only ever
    // created by the real auditor sessions above (which record `Role::Auditor`
    // tokens), so a count mismatch means a scheduled audit silently produced
    // nothing — refuse the whole campaign rather than emit a result with missing
    // integrity deltas. This makes the old always-zero `Role::Auditor` aggregation
    // structurally impossible.
    // Reaching here means every scheduled round ran (this session plus any
    // resumed-from predecessor). Every audit must be present: the ones this session
    // ran (scheduled at or after `start_round`) plus the ones reloaded from a prior
    // session's state. Counting both keeps the "no silent zero-filled integrity
    // delta" guarantee across a stop/resume.
    let expected_audits = reloaded_audits
        + schedule
            .iter()
            .filter(|rp| rp.integrity_audit && rp.round >= start_round)
            .count();
    if integrity_checkpoints.len() != expected_audits {
        bail!(
            "campaign incomplete: {expected_audits} integrity-audit checkpoint(s) expected but {} present — refusing to emit a result with missing integrity deltas",
            integrity_checkpoints.len()
        );
    }
    Ok(Some(CampaignResult {
        checkpoints,
        integrity_checkpoints,
        entropy_series,
        ledger,
    }))
}

// ===========================================================================
// The real subprocess glue (plan 03): concrete runner / judge / auditor that
// shell to `claude -p`, plus the mechanical slice extractor. The whole harness
// above is trait-driven and stub-tested; these are the thin production impls the
// CLI wires into `run_campaign`.
// ===========================================================================

/// The pinned source repo's changelog and bug-ledger paths — the two files the
/// mechanical round digest (amendment A2) diffs per slice. Fixed by the source
/// repository record (`prereg/source-repo.md`): kara keeps a Keep-a-Changelog
/// `CHANGELOG.md` and a structured bug ledger at `docs/bug-ledger.jsonl`.
const SOURCE_CHANGELOG_PATH: &str = "CHANGELOG.md";
const SOURCE_LEDGER_PATH: &str = "docs/bug-ledger.jsonl";

/// Assemble every round's mechanical digest (amendment A2) from the pinned source
/// repository, reading the boundary commits from the package's `slices.json`.
/// Returns one digest per slice in `index` order — round `i`'s writer input. The
/// same string feeds both arms, so the digest is never an arm-distinguishing
/// variable (criterion 5's parity contract, preserved at the call site).
pub fn extract_round_slices(repo: &Path, manifest_path: &Path) -> Result<Vec<String>> {
    #[derive(serde::Deserialize)]
    struct Manifest {
        slices: Vec<Slice>,
    }
    #[derive(serde::Deserialize)]
    struct Slice {
        index: usize,
        first_commit: String,
        last_commit: String,
    }
    let bytes = std::fs::read(manifest_path)
        .with_context(|| format!("reading slice manifest {}", manifest_path.display()))?;
    let manifest: Manifest =
        serde_json::from_slice(&bytes).context("parsing slices.json slice manifest")?;
    let mut slices = manifest.slices;
    slices.sort_by_key(|s| s.index);
    let mut out = Vec::with_capacity(slices.len());
    for s in &slices {
        out.push(slice_digest(
            repo,
            &s.first_commit,
            &s.last_commit,
            SOURCE_CHANGELOG_PATH,
            SOURCE_LEDGER_PATH,
        )?);
    }
    Ok(out)
}

/// Parse the auditor's two defect counts from its answer text (criterion 10). The
/// auditor skeleton pins the format to `DUPLICATES: <n>` and `CONTRADICTIONS: <n>`,
/// each on its own line; this reads the first integer on the marker's line,
/// case-insensitively. A missing marker is an error, never a silent zero — an
/// auditor that did not answer in the pinned format must invalidate its trial
/// rather than contribute a fabricated clean score.
fn parse_audit_counts(text: &str) -> Result<(usize, usize)> {
    let lower = text.to_lowercase();
    let duplicates = count_on_marker_line(&lower, "duplicates")
        .context("auditor output has no `DUPLICATES: <n>` count")?;
    let contradictions = count_on_marker_line(&lower, "contradictions")
        .context("auditor output has no `CONTRADICTIONS: <n>` count")?;
    Ok((duplicates, contradictions))
}

/// The first run of ASCII digits on the line where `marker` first appears in the
/// (already-lowercased) text. Scoped to the marker's own line so a marker with no
/// number does not accidentally read the next line's count.
fn count_on_marker_line(lower: &str, marker: &str) -> Option<usize> {
    let idx = lower.find(marker)?;
    let line = lower[idx + marker.len()..].lines().next().unwrap_or("");
    let digits: String = line
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

/// Spawn one `claude -p` session with `args` from `cwd` and parse its stream-json.
///
/// A non-zero exit is tolerated only when the stream still parsed a model id: the
/// `--max-budget-usd` cutoff (amendment A1) terminates an over-budget session
/// non-zero but emits a usable stream, and that must count as a real (budgeted)
/// session whose partial usage enters the ledger — not a spawn failure. A
/// non-zero exit with nothing parseable is a genuine failure and refuses.
fn spawn_claude_session(
    executable: &str,
    args: &[String],
    prompt: &str,
    cwd: &Path,
) -> Result<SessionOutput> {
    use std::io::Write;
    use std::process::Stdio;
    std::fs::create_dir_all(cwd)
        .with_context(|| format!("creating session working dir {}", cwd.display()))?;
    // The prompt goes over stdin, never as an argv entry — a large corpus/digest
    // prompt as a command-line argument hits ARG_MAX (E2BIG). `claude -p` with no
    // positional prompt reads it from stdin.
    let mut child = std::process::Command::new(executable)
        .args(args)
        .current_dir(cwd)
        // Give the MCP server room to finish its handshake before the first turn.
        .env("MCP_TIMEOUT", "60000")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!("spawning `{executable}` — is the claude CLI installed and on PATH?")
        })?;
    {
        let mut stdin = child
            .stdin
            .take()
            .context("claude session stdin was not piped")?;
        stdin
            .write_all(prompt.as_bytes())
            .context("writing prompt to claude session stdin")?;
        // Dropping `stdin` here closes it so `claude -p` sees EOF and starts.
    }
    let output = child
        .wait_with_output()
        .with_context(|| format!("waiting on `{executable}` session"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed = parse_session(&stdout);
    if !output.status.success() {
        match &parsed {
            // Budget cutoff or soft stop: the session ran and reported a model, so
            // its usage is real and the round is valid.
            Ok(s) if !s.model.is_empty() => {}
            _ => bail!(
                "claude session exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        }
    }
    parsed
}

/// The real divergence runner: shells to `claude -p` per session against each
/// arm's materialised substrate. Arm A is a loose markdown directory the writer
/// runs *inside* (its filesystem tools land there and accumulate across rounds);
/// Arm B is a folder-backed mem mounted over MCP, the writer running from an empty
/// sandbox so its built-in file tools find no codebase — only the mem is reachable.
/// The writer/reader allowance is passed as the per-session `--max-budget-usd` cap
/// (amendment A1); tests drive the same loop through a deterministic stub.
pub struct ClaudeDivergenceRunner {
    pub executable: String,
    /// Arm A substrate: a persistent markdown directory the writer runs inside.
    pub arm_a_dir: PathBuf,
    /// Arm B substrate: the folder-backed mem's entity directory (workspace root).
    pub arm_b_workspace: PathBuf,
    /// MCP config mounting Arm B's mem (memstead server pointed at the workspace).
    pub arm_b_mcp_config: PathBuf,
    /// Empty working directory Arm B sessions run from, so claude's built-in file
    /// tools find no codebase and the single variable (substrate access) holds.
    pub sandbox_dir: PathBuf,
    /// The pinned model's list output price, converting a token allowance into the
    /// `--max-budget-usd` cost budget (amendment A1): `budget_usd = allowance ×
    /// this`. `None` omits the cap (A1's documentary fallback).
    pub usd_per_output_token: Option<f64>,
}

impl ClaudeDivergenceRunner {
    fn budget(&self, token_allowance: usize) -> Option<f64> {
        self.usd_per_output_token
            .map(|rate| token_allowance as f64 * rate)
    }
}

impl DivergenceRunner for ClaudeDivergenceRunner {
    fn write(
        &self,
        arm: Arm,
        model: &str,
        prompt: &str,
        token_allowance: usize,
    ) -> Result<WriterOutcome> {
        let budget = self.budget(token_allowance);
        let (args, cwd) = match arm {
            Arm::A => (
                build_writer_args(Arm::A, model, budget, None),
                self.arm_a_dir.as_path(),
            ),
            Arm::B => (
                build_writer_args(Arm::B, model, budget, Some(&self.arm_b_mcp_config)),
                self.sandbox_dir.as_path(),
            ),
        };
        let out = spawn_claude_session(&self.executable, &args, prompt, cwd)?;
        Ok(WriterOutcome {
            tokens: out.tokens,
            non_cache_tokens: out.non_cache_tokens,
            tool_calls: out.tool_calls,
            executed_model: out.model,
        })
    }

    fn read(
        &self,
        arm: Arm,
        model: &str,
        prompt: &str,
        token_budget: usize,
    ) -> Result<ReaderOutcome> {
        let budget = self.budget(token_budget);
        let (args, cwd) = match arm {
            Arm::A => (
                build_reader_args(Arm::A, model, budget, None),
                self.arm_a_dir.as_path(),
            ),
            Arm::B => (
                build_reader_args(Arm::B, model, budget, Some(&self.arm_b_mcp_config)),
                self.sandbox_dir.as_path(),
            ),
        };
        let out = spawn_claude_session(&self.executable, &args, prompt, cwd)?;
        Ok(ReaderOutcome {
            answer: out.text,
            tokens: out.tokens,
            non_cache_tokens: out.non_cache_tokens,
            tool_calls: out.tool_calls,
            executed_model: out.model,
        })
    }

    fn substrate_dir(&self, arm: Arm) -> &Path {
        match arm {
            Arm::A => &self.arm_a_dir,
            Arm::B => &self.arm_b_workspace,
        }
    }
}

/// The real blind judge: shells to `claude -p` with the label-free grading prompt
/// and the pinned model, reporting its tokens (criterion 7) and executed model
/// (criterion 3). No tools; runs from an empty sandbox.
pub struct ClaudeDivergenceJudge {
    pub executable: String,
    pub sandbox_dir: PathBuf,
}

impl DivergenceJudge for ClaudeDivergenceJudge {
    fn score(&self, model: &str, reference: &str, blinded_answer: &str) -> Result<JudgeOutcome> {
        let prompt = super::judge::build_judge_prompt(reference, blinded_answer);
        let mut args = base_session_args(model, None);
        args.push("--allowedTools".to_string());
        args.push(String::new());
        args.push("--system-prompt".to_string());
        args.push(super::judge::JUDGE_SYSTEM.to_string());
        let out = spawn_claude_session(&self.executable, &args, &prompt, &self.sandbox_dir)?;
        let score = super::judge::parse_score(&out.text)?;
        Ok(JudgeOutcome {
            score,
            tokens: out.tokens,
            non_cache_tokens: out.non_cache_tokens,
            executed_model: out.model,
        })
    }
}

/// The real blind integrity auditor (criterion 10): shells to `claude -p` with the
/// arm-neutral auditor prompt (skeleton + tell-stripped corpus, assembled by the
/// harness) and the pinned model, parsing the two defect counts and reporting its
/// tokens and executed model. No tools; runs from an empty sandbox.
pub struct ClaudeDivergenceAuditor {
    pub executable: String,
    pub sandbox_dir: PathBuf,
}

impl DivergenceAuditor for ClaudeDivergenceAuditor {
    fn audit(&self, model: &str, auditor_prompt: &str) -> Result<AuditOutcome> {
        let mut args = base_session_args(model, None);
        args.push("--allowedTools".to_string());
        args.push(String::new());
        let out = spawn_claude_session(&self.executable, &args, auditor_prompt, &self.sandbox_dir)?;
        let (duplicates, contradictions) = parse_audit_counts(&out.text)?;
        Ok(AuditOutcome {
            duplicates,
            contradictions,
            tokens: out.tokens,
            non_cache_tokens: out.non_cache_tokens,
            executed_model: out.model,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::fs;
    use std::path::PathBuf;

    fn write_fixture_package(dir: &Path) {
        fs::write(
            dir.join("campaign.json"),
            r#"{ "rounds": 10, "hurry_rounds": [3,6,9], "reader_checkpoints": [1,3,5,10],
                 "integrity_audit_rounds": [5,10], "trials": 3,
                 "writer_allowance_full_tokens": 8000, "writer_allowance_hurry_tokens": 4000,
                 "reader_budget_tokens": 8000, "usd_per_output_token": 0.000025,
                 "contamination_threshold": 0.5, "cost_cap_tokens": 20000000 }"#,
        )
        .unwrap();
        fs::write(
            dir.join("models.json"),
            r#"{ "writer": "claude-opus-4-8", "reader": "claude-opus-4-8",
                 "judge": "claude-opus-4-8", "auditor": "claude-opus-4-8" }"#,
        )
        .unwrap();
        fs::write(
            dir.join("tell-lists.json"),
            r#"{ "arm_a_tells": { "tokens": ["wikilink"], "phrases": ["the notes directory"] },
                 "arm_b_tells": { "tokens": ["memstead"], "phrases": ["the mounted mem"] } }"#,
        )
        .unwrap();
        fs::write(
            dir.join("prompts.json"),
            r#"{
                "writer_full_skeleton": "Maintain the base.\n\n{SUBSTRATE_BLOCK}\n\nMaterial:\n\n{ROUND_SLICE_CONTENT}",
                "writer_hurry_skeleton": "Quickly update.\n\n{SUBSTRATE_BLOCK}\n\nMaterial:\n\n{ROUND_SLICE_CONTENT}",
                "reader_skeleton": "Answer directly.\n\n{SUBSTRATE_BLOCK}\n\nQuestion: {QUERY}",
                "writer_substrate": { "arm_a": "Use files.", "arm_b": "Use the mem tools." },
                "reader_substrate": { "arm_a": "Read files.", "arm_b": "Read the mem." },
                "auditor_skeleton": "{CORPUS}"
            }"#,
        )
        .unwrap();
    }

    fn tmp() -> PathBuf {
        tempfile::tempdir().unwrap().keep()
    }

    #[test]
    fn loads_campaign_models_and_both_tell_lists() {
        let dir = tmp();
        write_fixture_package(&dir);
        let pkg = Package::load(&dir).unwrap();
        assert_eq!(pkg.campaign.rounds, 10);
        assert_eq!(pkg.campaign.hurry_rounds, vec![3, 6, 9]);
        assert_eq!(pkg.campaign.trials, 3);
        assert_eq!(pkg.campaign.cost_cap_tokens, 20_000_000);
        assert_eq!(pkg.single_model().unwrap(), "claude-opus-4-8");
        let all = pkg.tell_lists.combined();
        assert!(all.iter().any(|t| t == "the mounted mem"));
        assert!(all.iter().any(|t| t == "wikilink"));
    }

    #[test]
    fn content_hash_is_stable_and_edit_sensitive() {
        let dir = tmp();
        write_fixture_package(&dir);
        let h1 = hash_package_dir(&dir).unwrap();
        // Re-hashing the untouched directory yields the same hash.
        assert_eq!(h1, hash_package_dir(&dir).unwrap());
        assert_eq!(h1.len(), 64, "sha-256 hex is 64 chars");
        // Editing any file changes the hash.
        fs::write(dir.join("campaign.json"), r#"{ "rounds": 11 }"#).unwrap();
        assert_ne!(h1, hash_package_dir(&dir).unwrap());
    }

    #[test]
    fn verify_pin_refuses_an_edited_package() {
        let dir = tmp();
        write_fixture_package(&dir);
        let pkg = Package::load(&dir).unwrap();
        assert!(pkg.verify_pin(&pkg.content_hash).is_ok());
        assert!(
            pkg.verify_pin("0000000000000000000000000000000000000000000000000000000000000000")
                .is_err()
        );
    }

    #[test]
    fn writer_and_reader_prompts_assemble_with_substitutions() {
        let dir = tmp();
        write_fixture_package(&dir);
        let p = Package::load(&dir).unwrap().prompts;

        let wa = p.writer(Arm::A, false, "SLICE-XYZ");
        assert!(
            wa.contains("Use files."),
            "substrate block substituted: {wa}"
        );
        assert!(wa.contains("SLICE-XYZ"), "round slice substituted: {wa}");
        assert!(
            !wa.contains("{SUBSTRATE_BLOCK}") && !wa.contains("{ROUND_SLICE_CONTENT}"),
            "no placeholders left: {wa}"
        );

        let wh = p.writer(Arm::B, true, "S");
        assert!(
            wh.starts_with("Quickly update."),
            "hurry skeleton used: {wh}"
        );
        assert!(wh.contains("Use the mem tools."));

        let r = p.reader(Arm::A, "How many bugs are open?");
        assert!(
            r.contains("Read files.") && r.contains("How many bugs are open?"),
            "{r}"
        );
        assert!(!r.contains("{QUERY}"), "{r}");
    }

    #[test]
    fn writer_prompts_differ_only_in_the_substrate_block() {
        // Criterion-5 parity, mechanically: the two arms' assembled prompts are
        // identical once the substrate block is accounted for.
        let dir = tmp();
        write_fixture_package(&dir);
        let p = Package::load(&dir).unwrap().prompts;
        let a = p.writer(Arm::A, false, "SLICE");
        let b = p.writer(Arm::B, false, "SLICE");
        assert_ne!(a, b, "the substrate blocks differ, so the prompts differ");
        // Swapping A's substrate text for B's turns the A prompt into the B prompt
        // exactly — proving the substrate block is the ONLY difference.
        assert_eq!(a.replace("Use files.", "Use the mem tools."), b);
    }

    #[test]
    fn prompts_missing_a_placeholder_are_refused() {
        let dir = tmp();
        write_fixture_package(&dir);
        fs::write(
            dir.join("prompts.json"),
            r#"{
                "writer_full_skeleton": "No slice placeholder.\n\n{SUBSTRATE_BLOCK}",
                "writer_hurry_skeleton": "{SUBSTRATE_BLOCK} {ROUND_SLICE_CONTENT}",
                "reader_skeleton": "{SUBSTRATE_BLOCK} {QUERY}",
                "writer_substrate": { "arm_a": "a", "arm_b": "b" },
                "reader_substrate": { "arm_a": "a", "arm_b": "b" },
                "auditor_skeleton": "{CORPUS}"
            }"#,
        )
        .unwrap();
        let err = Package::load(&dir).unwrap_err().to_string();
        assert!(err.contains("ROUND_SLICE_CONTENT"), "{err}");
    }

    #[test]
    fn schedule_resolves_flags_and_allowances_from_the_package() {
        let dir = tmp();
        write_fixture_package(&dir);
        let pkg = Package::load(&dir).unwrap();
        let sched = pkg.campaign.schedule();
        assert_eq!(sched.len(), 10, "one plan per round");
        assert_eq!(sched[0].round, 1);
        assert_eq!(sched.last().unwrap().round, 10);

        // Hurry rounds 3/6/9 carry the halved allowance and the hurry flag.
        for r in [3usize, 6, 9] {
            let rp = &sched[r - 1];
            assert!(rp.hurry, "round {r} should be hurry");
            assert_eq!(rp.writer_allowance_tokens, 4000);
        }
        // A full round carries the full allowance and no hurry flag.
        assert!(!sched[0].hurry);
        assert_eq!(sched[0].writer_allowance_tokens, 8000);

        // Reader checkpoints at 1/3/5/10, integrity audits at 5/10.
        let checkpoints: Vec<usize> = sched
            .iter()
            .filter(|p| p.reader_checkpoint)
            .map(|p| p.round)
            .collect();
        assert_eq!(checkpoints, vec![1, 3, 5, 10]);
        let audits: Vec<usize> = sched
            .iter()
            .filter(|p| p.integrity_audit)
            .map(|p| p.round)
            .collect();
        assert_eq!(audits, vec![5, 10]);
    }

    #[test]
    fn validate_refuses_an_out_of_range_schedule() {
        let dir = tmp();
        write_fixture_package(&dir);
        // A reader checkpoint at round 11 in a 10-round campaign.
        fs::write(
            dir.join("campaign.json"),
            r#"{ "rounds": 10, "hurry_rounds": [3], "reader_checkpoints": [1, 11],
                 "integrity_audit_rounds": [10], "trials": 3,
                 "writer_allowance_full_tokens": 8000, "writer_allowance_hurry_tokens": 4000,
                 "reader_budget_tokens": 8000, "usd_per_output_token": 0.000025,
                 "contamination_threshold": 0.5, "cost_cap_tokens": 20000000 }"#,
        )
        .unwrap();
        let err = Package::load(&dir).unwrap_err().to_string();
        assert!(
            err.contains("reader_checkpoints references round 11"),
            "{err}"
        );
    }

    #[test]
    fn validate_refuses_zero_rounds() {
        let c = Campaign {
            rounds: 0,
            hurry_rounds: vec![],
            reader_checkpoints: vec![],
            integrity_audit_rounds: vec![],
            trials: 3,
            writer_allowance_full_tokens: 8000,
            writer_allowance_hurry_tokens: 4000,
            reader_budget_tokens: 8000,
            usd_per_output_token: 0.000025,
            contamination_threshold: 0.5,
            cost_cap_tokens: 20_000_000,
        };
        assert!(c.validate().is_err());
    }

    /// Group an integer with thousands commas, matching how `bands.md` writes
    /// token counts (`8000` -> `8,000`, `20000000` -> `20,000,000`).
    fn with_commas(n: u64) -> String {
        let s = n.to_string();
        let bytes = s.as_bytes();
        let mut out = String::new();
        for (i, b) in bytes.iter().enumerate() {
            if i > 0 && (bytes.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(*b as char);
        }
        out
    }

    /// Guards the drift risk between the machine files the harness reads
    /// (`campaign.json`, `prompts.json`) and the human documents the
    /// pre-registration exposition lives in (`bands.md`, `arms.md`): the two must
    /// agree, and nothing else enforces it. Skips when the package is not in this
    /// checkout (e.g. a published crate without the docs tree).
    #[test]
    fn load_queries_reads_the_committed_battery_into_taskspecs() {
        let pkg_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../docs/proof/divergence/prereg");
        if !pkg_dir.join("queries.json").exists() {
            return; // package not in this checkout; nothing to load
        }
        let queries = load_queries(&pkg_dir).unwrap();
        // The battery is twelve queries, four classes x three.
        assert_eq!(queries.len(), 12, "twelve-query battery");
        for q in &queries {
            assert!(!q.id.is_empty());
            assert!(!q.prompt.is_empty(), "{} has a question", q.id);
            assert!(!q.reference.is_empty(), "{} has a reference answer", q.id);
        }
        // A known query id is present with its reference intact.
        assert!(queries.iter().any(|q| q.id == "A2-ledger-totals"));
    }

    fn arg_pairs(args: &[String]) -> std::collections::HashMap<String, String> {
        // `-p` is a bare flag now (the prompt goes over stdin), so only `--flag value`
        // pairs are mapped.
        args.windows(2)
            .filter(|w| w[0].starts_with("--"))
            .map(|w| (w[0].clone(), w[1].clone()))
            .collect()
    }

    #[test]
    fn writer_args_carry_the_pinned_model_and_arm_tools() {
        let cfg = std::path::Path::new("/tmp/mem.json");
        let a = build_writer_args(Arm::A, "claude-opus-4-8", None, None);
        let b = build_writer_args(Arm::B, "claude-opus-4-8", None, Some(cfg));

        // Criterion 3: both sessions are invoked with the pinned model.
        assert_eq!(arg_pairs(&a).get("--model").unwrap(), "claude-opus-4-8");
        assert_eq!(arg_pairs(&b).get("--model").unwrap(), "claude-opus-4-8");
        // The prompt is never an argv entry (it goes over stdin) — no ARG_MAX risk.
        assert!(a.iter().any(|x| x == "-p") && !a.iter().any(|x| x == "PROMPT"));

        // Arm A writes files, no MCP; Arm B mutates the mem over MCP (criterion 5).
        let a_tools = arg_pairs(&a).get("--allowedTools").unwrap().clone();
        assert!(
            a_tools.contains("Write") && a_tools.contains("Edit"),
            "{a_tools}"
        );
        assert!(!a.iter().any(|x| x == "--mcp-config"));
        let b_tools = arg_pairs(&b).get("--allowedTools").unwrap().clone();
        assert_eq!(b_tools, "mcp__memstead__*");
        assert_eq!(arg_pairs(&b).get("--mcp-config").unwrap(), "/tmp/mem.json");
    }

    #[test]
    fn reader_args_are_read_only_per_arm() {
        let cfg = std::path::Path::new("/tmp/mem.json");
        let a = build_reader_args(Arm::A, "m", None, None);
        let b = build_reader_args(Arm::B, "m", None, Some(cfg));

        // Arm A reader has read tools but not Write/Edit.
        let a_tools = arg_pairs(&a).get("--allowedTools").unwrap().clone();
        assert!(
            a_tools.contains("Read") && a_tools.contains("Grep"),
            "{a_tools}"
        );
        assert!(
            !a_tools.contains("Write") && !a_tools.contains("Edit"),
            "{a_tools}"
        );

        // Arm B reader has the memstead read tools, never the mutation tools.
        let b_tools = arg_pairs(&b).get("--allowedTools").unwrap().clone();
        assert!(
            b_tools.contains("memstead_overview") && b_tools.contains("memstead_search"),
            "{b_tools}"
        );
        assert!(
            !b_tools.contains("memstead_create")
                && !b_tools.contains("memstead_update")
                && !b_tools.contains("memstead_relate"),
            "reader must not be able to mutate: {b_tools}"
        );
    }

    #[test]
    fn writer_and_reader_args_differ_only_in_access_surface() {
        // The prompt, model, and base flags are shared; the tools/MCP differ.
        let cfg = std::path::Path::new("/tmp/mem.json");
        let wb = build_writer_args(Arm::B, "m", None, Some(cfg));
        let rb = build_reader_args(Arm::B, "m", None, Some(cfg));
        // Both are print-mode sessions on the same pinned model (prompt via stdin).
        assert!(wb.iter().any(|x| x == "-p") && rb.iter().any(|x| x == "-p"));
        assert_eq!(arg_pairs(&wb).get("--model"), arg_pairs(&rb).get("--model"));
        // Writer can mutate; reader cannot.
        assert_eq!(
            arg_pairs(&wb).get("--allowedTools").unwrap(),
            "mcp__memstead__*"
        );
        assert!(!arg_pairs(&rb).get("--allowedTools").unwrap().contains('*'));
    }

    #[test]
    fn allowance_maps_to_a_proportional_max_budget_usd_flag() {
        // Amendment A1: the writer allowance is enforced as a proportional
        // `--max-budget-usd` cap, budget_usd = allowance_tokens * usd_per_output_token.
        let dir = tmp();
        write_fixture_package(&dir);
        let campaign = Package::load(&dir).unwrap().campaign;

        // claude-opus-4-8 output price (0.000025 USD/token) turns the pinned
        // allowances into $0.20 (full) and $0.10 (hurry) — hurry is literally half.
        let full = campaign.budget_usd(campaign.writer_allowance_full_tokens);
        let hurry = campaign.budget_usd(campaign.writer_allowance_hurry_tokens);
        assert!((full - 0.20).abs() < 1e-9, "full budget: {full}");
        assert!((hurry - 0.10).abs() < 1e-9, "hurry budget: {hurry}");
        assert!(
            (full - 2.0 * hurry).abs() < 1e-9,
            "hurry is half the full budget"
        );

        // The flag is emitted only when a budget is supplied; the value is the
        // dollar figure to four decimals. Both arms carry it identically — it is
        // not an arm-distinguishing variable.
        let with = build_writer_args(Arm::A, "m", Some(full), None);
        assert_eq!(arg_pairs(&with).get("--max-budget-usd").unwrap(), "0.2000");
        let with_b = build_writer_args(
            Arm::B,
            "m",
            Some(full),
            Some(std::path::Path::new("/tmp/mem.json")),
        );
        assert_eq!(
            arg_pairs(&with_b).get("--max-budget-usd").unwrap(),
            "0.2000"
        );
        let without = build_writer_args(Arm::A, "m", None, None);
        assert!(!without.iter().any(|x| x == "--max-budget-usd"));
    }

    /// Build a tiny fixture git repo with three commits touching a changelog, a
    /// JSONL bug ledger, and a source file. Returns the repo dir and the three
    /// commit SHAs (root first). No timestamps reach the digest (the git
    /// invocations read subjects/diffstats only), so it is deterministic.
    fn fixture_git_repo() -> (PathBuf, String, String, String) {
        let repo = tmp();
        let run = |args: &[&str]| -> String {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        run(&["init", "-q"]);
        fs::create_dir(repo.join("docs")).unwrap();

        // c1 (root): seed the changelog and ledger.
        fs::write(repo.join("CHANGELOG.md"), "# Changelog\n\n## v1\n- seed\n").unwrap();
        fs::write(
            repo.join("docs/bug-ledger.jsonl"),
            "{\"id\":\"B-1\",\"status\":\"open\"}\n",
        )
        .unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "seed the ledger and changelog"]);
        let c1 = run(&["rev-parse", "HEAD"]);

        // c2: a code change, a changelog entry, ledger B-1 fixed + B-2 opened.
        fs::write(repo.join("src.rs"), "fn main() {}\n").unwrap();
        fs::write(
            repo.join("CHANGELOG.md"),
            "# Changelog\n\n## v2\n- fixed B-1\n\n## v1\n- seed\n",
        )
        .unwrap();
        fs::write(
            repo.join("docs/bug-ledger.jsonl"),
            "{\"id\":\"B-1\",\"status\":\"fixed\"}\n{\"id\":\"B-2\",\"status\":\"open\"}\n",
        )
        .unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "fix(codegen): resolve B-1"]);
        let c2 = run(&["rev-parse", "HEAD"]);

        // c3: another change; ledger B-2 fixed.
        fs::write(repo.join("src.rs"), "fn main() { let x = 1; }\n").unwrap();
        fs::write(
            repo.join("docs/bug-ledger.jsonl"),
            "{\"id\":\"B-1\",\"status\":\"fixed\"}\n{\"id\":\"B-2\",\"status\":\"fixed\"}\n",
        )
        .unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "fix(parser): resolve B-2"]);
        let c3 = run(&["rev-parse", "HEAD"]);

        (repo, c1, c2, c3)
    }

    #[test]
    fn slice_digest_assembles_the_four_mechanical_sections() {
        let (repo, c1, c2, c3) = fixture_git_repo();

        // Slice [c2, c3] — a mid-history slice, the common case: base is c2^ = c1.
        let d = slice_digest(&repo, &c2, &c3, "CHANGELOG.md", "docs/bug-ledger.jsonl").unwrap();

        // All four sections present.
        assert!(d.contains("### Commit log"), "{d}");
        assert!(d.contains("### Diffstat"), "{d}");
        assert!(d.contains("### CHANGELOG.md changes"), "{d}");
        assert!(
            d.contains("### Bug ledger changes (docs/bug-ledger.jsonl)"),
            "{d}"
        );

        // The commit log carries the slice's OWN commits (c2, c3) and excludes
        // the base commit c1 — proving the first^..last range boundary.
        assert!(
            d.contains("resolve B-1") && d.contains("resolve B-2"),
            "{d}"
        );
        assert!(
            !d.contains("seed the ledger and changelog"),
            "base commit c1 must be excluded from the slice log: {d}"
        );

        // Diffstat names the files that changed across the slice.
        assert!(d.contains("src.rs") && d.contains("CHANGELOG.md"), "{d}");
        // The ledger delta shows B-2 flipping to fixed within the slice.
        assert!(d.contains("B-2"), "ledger delta present: {d}");

        // Pure function of (repo, range): the identical string feeds both arms,
        // so the digest can never diverge between Arm A and Arm B.
        let again = slice_digest(&repo, &c2, &c3, "CHANGELOG.md", "docs/bug-ledger.jsonl").unwrap();
        assert_eq!(d, again, "digest is deterministic / arm-neutral");

        // Sanity: c1 exists and is distinct (guards the fixture).
        assert_ne!(c1, c2);
    }

    #[test]
    fn slice_digest_handles_the_repository_root_slice() {
        // The root slice's first commit has no parent, so the diff base is the
        // empty tree and the log is the full ancestry — here just c1 itself.
        let (repo, c1, _c2, _c3) = fixture_git_repo();
        let d = slice_digest(&repo, &c1, &c1, "CHANGELOG.md", "docs/bug-ledger.jsonl").unwrap();
        assert!(
            d.contains("seed the ledger and changelog"),
            "root slice logs its own commit: {d}"
        );
        // The whole seeded changelog/ledger is the delta against the empty tree.
        assert!(d.contains("# Changelog") && d.contains("B-1"), "{d}");
    }

    #[test]
    fn extract_round_slices_reads_the_manifest_and_digests_each_slice_in_order() {
        let (repo, c1, c2, c3) = fixture_git_repo();
        // A two-slice manifest, deliberately out of order to prove index sorting:
        // slice 2 is [c2,c3], slice 1 is the root [c1,c1].
        let manifest = repo.join("slices.json");
        fs::write(
            &manifest,
            format!(
                r#"{{"slices":[
                    {{"index":2,"first_commit":"{c2}","last_commit":"{c3}"}},
                    {{"index":1,"first_commit":"{c1}","last_commit":"{c1}"}}
                ]}}"#
            ),
        )
        .unwrap();
        let slices = extract_round_slices(&repo, &manifest).unwrap();
        assert_eq!(slices.len(), 2, "one digest per manifest slice");
        // Sorted by index: slice 1 (root) first, slice 2 second.
        assert!(
            slices[0].contains("seed the ledger and changelog"),
            "slice[0] is the root slice: {}",
            slices[0]
        );
        assert!(
            slices[1].contains("resolve B-1") && slices[1].contains("resolve B-2"),
            "slice[1] is [c2,c3]: {}",
            slices[1]
        );
        // Each digest matches the standalone slice_digest for the same range — the
        // extractor is a thin manifest-driven wrapper, not a second implementation.
        let direct =
            slice_digest(&repo, &c2, &c3, "CHANGELOG.md", "docs/bug-ledger.jsonl").unwrap();
        assert_eq!(slices[1], direct);
    }

    #[test]
    fn parse_audit_counts_reads_the_two_marked_counts() {
        let (d, c) = parse_audit_counts("DUPLICATES: 3\nCONTRADICTIONS: 1").unwrap();
        assert_eq!((d, c), (3, 1));
        // Case-insensitive, with leading reasoning the skeleton forbids but a model
        // might still emit — the marked line is what counts.
        let (d, c) =
            parse_audit_counts("some noise\nduplicates:  0\ncontradictions: 12\n").unwrap();
        assert_eq!((d, c), (0, 12));
    }

    #[test]
    fn parse_audit_counts_refuses_a_missing_marker() {
        // A missing count must error, never silently score a clean zero.
        assert!(parse_audit_counts("DUPLICATES: 2").is_err());
        assert!(parse_audit_counts("no counts at all").is_err());
        // A marker with no number on its line does not steal the next line's count.
        assert!(parse_audit_counts("DUPLICATES:\nCONTRADICTIONS: 4").is_err());
    }

    #[test]
    fn parse_session_extracts_text_tools_and_usage_tokens() {
        // A minimal claude stream-json: an assistant turn with text + a tool_use,
        // then a result event carrying the usage token counts.
        let stream = r#"
{"type":"system","subtype":"init"}
{"type":"assistant","message":{"content":[{"type":"text","text":"Recorded the change."},{"type":"tool_use","name":"mcp__memstead__memstead_create"}]}}
not-json-skip-me
{"type":"result","result":"Recorded the change.","usage":{"input_tokens":1200,"output_tokens":300,"cache_read_input_tokens":50}}
"#;
        let out = parse_session(stream).unwrap();
        assert_eq!(out.text, "Recorded the change.");
        assert_eq!(out.tool_calls, vec!["mcp__memstead__memstead_create"]);
        assert_eq!(out.tokens, 1200 + 300 + 50, "usage tokens summed");
        assert_eq!(
            out.non_cache_tokens,
            1200 + 300,
            "non-cache excludes the 50 cache_read tokens (amendment A4)"
        );
    }

    #[test]
    fn parse_session_recovers_budget_cut_usage_from_assistant_events() {
        // A `--max-budget-usd` cutoff (amendment A1): the assistant events carry the
        // running cumulative usage, but the `result` event errors with zero usage.
        // The ledger must still see the spent tokens, so the max assistant snapshot
        // is used when the result total is zero.
        let stream = r#"
{"type":"assistant","message":{"model":"claude-opus-4-8","usage":{"input_tokens":2,"output_tokens":2,"cache_read_input_tokens":17047,"cache_creation_input_tokens":2931},"content":[{"type":"text","text":"Partial essay…"}]}}
{"type":"result","subtype":"error_max_budget_usd","is_error":true,"usage":{"input_tokens":0,"output_tokens":0}}
"#;
        let out = parse_session(stream).unwrap();
        assert_eq!(out.model, "claude-opus-4-8");
        assert_eq!(
            out.tokens,
            2 + 2 + 17047 + 2931,
            "budget-cut usage recovered from the assistant snapshot, not the zeroed result"
        );
        assert_eq!(
            out.non_cache_tokens, 2 + 2,
            "budget-cut non-cache is the assistant snapshot's fresh input + output, excluding its cache tokens"
        );
        assert_eq!(out.text, "Partial essay…");
    }

    #[test]
    fn parse_session_falls_back_to_result_text_and_zero_tokens() {
        let stream = r#"{"type":"result","result":"just the result"}"#;
        let out = parse_session(stream).unwrap();
        assert_eq!(out.text, "just the result");
        assert!(out.tool_calls.is_empty());
        assert_eq!(out.tokens, 0, "no usage block → zero tokens");
    }

    #[test]
    fn load_queries_from_a_fixture() {
        let dir = tmp();
        fs::write(
            dir.join("queries.json"),
            r#"{ "queries": [
                { "id": "S1", "class": "status-filter", "prompt": "which open?",
                  "ground_truth": { "x": 1 }, "reference_answer": "these are open" }
            ] }"#,
        )
        .unwrap();
        let queries = load_queries(&dir).unwrap();
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].id, "S1");
        assert_eq!(queries[0].prompt, "which open?");
        assert_eq!(queries[0].reference, "these are open");
    }

    #[test]
    fn committed_package_machine_files_match_their_prose_sources() {
        let pkg_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../docs/proof/divergence/prereg");
        if !pkg_dir.join("campaign.json").exists() {
            return; // package not present in this build context; nothing to guard
        }

        // campaign.json numeric values must appear (as bands.md formats them) in
        // bands.md.
        let campaign: Campaign =
            serde_json::from_slice(&std::fs::read(pkg_dir.join("campaign.json")).unwrap()).unwrap();
        let bands = std::fs::read_to_string(pkg_dir.join("bands.md")).unwrap();
        let want = |needle: String| {
            assert!(
                bands.contains(&needle),
                "bands.md does not contain {needle:?} — campaign.json has drifted from bands.md"
            );
        };
        want(with_commas(campaign.writer_allowance_full_tokens as u64));
        want(with_commas(campaign.writer_allowance_hurry_tokens as u64));
        want(with_commas(campaign.cost_cap_tokens));
        let join = |v: &[usize]| {
            v.iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        want(join(&campaign.hurry_rounds));
        want(join(&campaign.reader_checkpoints));
        want(campaign.contamination_threshold.to_string());

        // prompts.json prompt text must appear verbatim in arms.md.
        let prompts: Prompts =
            serde_json::from_slice(&std::fs::read(pkg_dir.join("prompts.json")).unwrap()).unwrap();
        let arms = std::fs::read_to_string(pkg_dir.join("arms.md")).unwrap();
        let want_arms = |needle: &str| {
            assert!(
                arms.contains(needle),
                "arms.md does not contain {:?} — prompts.json has drifted from arms.md",
                &needle[..needle.len().min(60)]
            );
        };
        want_arms(&prompts.writer_substrate.arm_a);
        want_arms(&prompts.writer_substrate.arm_b);
        want_arms(&prompts.reader_substrate.arm_a);
        want_arms(&prompts.reader_substrate.arm_b);
        // The skeleton opening (before the first placeholder) is one blockquote
        // line in arms.md.
        want_arms(
            prompts
                .writer_full_skeleton
                .split("\n\n{SUBSTRATE_BLOCK}")
                .next()
                .unwrap(),
        );
        want_arms(
            prompts
                .reader_skeleton
                .split("\n\n{SUBSTRATE_BLOCK}")
                .next()
                .unwrap(),
        );

        // The arm-neutral auditor skeleton must appear verbatim in rubrics.md (its
        // prose home is the integrity counting rubric, not arms.md), placeholder
        // included — it lives there in a fenced block, so the whole string matches.
        let rubrics = std::fs::read_to_string(pkg_dir.join("rubrics.md")).unwrap();
        assert!(
            rubrics.contains(&prompts.auditor_skeleton),
            "rubrics.md does not contain the auditor_skeleton verbatim — prompts.json has drifted from rubrics.md"
        );
    }

    #[test]
    fn vocabulary_entropy_counts_types_statuses_and_relation_labels() {
        let dir = tmp();
        // Two typed entities (Arm-B-shaped): distinct types {spec, decision},
        // statuses {accepted, proposed}, relation labels {REFERENCES, DEPENDS_ON}.
        fs::write(
            dir.join("a.md"),
            "---\ntype: spec\nstatus: accepted\n---\n\nBody DEPENDS_ON other. REFERENCES x.",
        )
        .unwrap();
        fs::write(
            dir.join("b.md"),
            "---\ntype: decision\nstatus: proposed\n---\n\nIt REFERENCES a.",
        )
        .unwrap();
        // A non-markdown file is ignored.
        fs::write(dir.join("note.txt"), "type: ignored\nBOGUS").unwrap();

        let e = vocabulary_entropy(&dir).unwrap();
        assert_eq!(e.distinct_types, 2, "spec, decision");
        assert_eq!(e.distinct_status_values, 2, "accepted, proposed");
        assert_eq!(e.distinct_relation_labels, 2, "REFERENCES, DEPENDS_ON");
    }

    #[test]
    fn vocabulary_entropy_of_untyped_arm_a_notes_is_low() {
        let dir = tmp();
        // Arm-A-shaped: a free-string type, no status, untyped wikilinks (no
        // ALL-CAPS relation labels).
        fs::write(
            dir.join("x.md"),
            "---\ntype: note\n---\n\nSee [[other-note]] and [[third]].",
        )
        .unwrap();
        let e = vocabulary_entropy(&dir).unwrap();
        assert_eq!(e.distinct_types, 1);
        assert_eq!(e.distinct_status_values, 0);
        assert_eq!(
            e.distinct_relation_labels, 0,
            "untyped links carry no labels"
        );
    }

    #[test]
    fn ledger_attributes_tokens_by_arm_and_role() {
        let mut led = Ledger::new(1_000);
        led.record(Arm::A, Role::Writer, 100, 40);
        led.record(Arm::B, Role::Writer, 120, 50);
        // Arm B's refusal-repair retry is charged to Arm B's writer cost.
        led.record(Arm::B, Role::Writer, 30, 10);
        led.record(Arm::A, Role::Reader, 50, 20);
        led.record(Arm::A, Role::Judge, 10, 5);

        assert_eq!(led.total(), 310);
        // The cap-counted figure is the non-cache sum, always ≤ the raw total.
        assert_eq!(led.total_non_cache(), 40 + 50 + 10 + 20 + 5);
        assert_eq!(led.total_for(Arm::B), 150);
        assert_eq!(led.total_for(Arm::A), 160);
        assert_eq!(led.total_role(Arm::B, Role::Writer), 150);
        assert_eq!(led.total_role(Arm::A, Role::Reader), 50);
        assert_eq!(led.total_role(Arm::A, Role::Auditor), 0);
    }

    #[test]
    fn ledger_cost_cap_guards_before_and_after() {
        let mut led = Ledger::new(1_000);
        led.record(Arm::A, Role::Writer, 900, 900);
        // Before a session: a 200-token session would exceed; a 100-token one fits.
        assert!(led.would_exceed(200));
        assert!(!led.would_exceed(100));
        // Still within cap after 900.
        assert!(led.check_cap().is_ok());
        // Overspend, then the between-sessions check refuses.
        led.record(Arm::B, Role::Writer, 200, 200);
        assert_eq!(led.total(), 1_100);
        let err = led.check_cap().unwrap_err().to_string();
        assert!(err.contains("cost cap exceeded"), "{err}");
    }

    #[test]
    fn with_retries_takes_the_first_success_and_gives_up_after_all_fail() {
        use std::cell::Cell;
        // Fails twice, succeeds on the third attempt → the success is returned and
        // exactly three attempts were made.
        let calls = Cell::new(0);
        let got = with_retries("t", 3, || {
            calls.set(calls.get() + 1);
            if calls.get() < 3 {
                anyhow::bail!("flaky")
            }
            Ok(42)
        });
        assert_eq!(got.unwrap(), 42);
        assert_eq!(calls.get(), 3, "retried until the first success");

        // Every attempt fails → the last error propagates after `attempts` tries.
        let calls = Cell::new(0);
        let err = with_retries::<()>("t", 3, || {
            calls.set(calls.get() + 1);
            anyhow::bail!("always {}", calls.get())
        });
        assert!(err.is_err());
        assert_eq!(calls.get(), 3, "gave up after exactly `attempts` tries");
        assert!(err.unwrap_err().to_string().contains("always 3"), "last error propagates");
    }

    #[test]
    fn cost_cap_counts_non_cache_only() {
        // Amendment A4: the brake counts fresh input + output, never cache reads —
        // a session that is almost all cache is cheap in real money and must not
        // trip a cap sized for real cost.
        let mut led = Ledger::new(1_000);
        // Raw 5,000 tokens but only 500 non-cache (the other 4,500 are cache reads).
        led.record(Arm::A, Role::Writer, 5_000, 500);
        assert_eq!(led.total(), 5_000, "raw total published as-is");
        assert_eq!(led.total_non_cache(), 500, "cap counts the non-cache portion");
        assert!(
            led.check_cap().is_ok(),
            "5,000 raw but only 500 non-cache is well under the 1,000 cap"
        );
        assert!(!led.would_exceed(400), "500 + 400 = 900 fits");
        assert!(led.would_exceed(600), "500 + 600 = 1,100 exceeds");
        // The published summary carries the raw total, the cap-counted non-cache
        // figure, and the cache split.
        let s = led.summary();
        assert_eq!(s.total_tokens, 5_000);
        assert_eq!(s.non_cache_tokens, 500);
        assert_eq!(s.cache_tokens, 4_500);
    }

    /// A deterministic runner stub. Writers record the (arm, model, allowance) of
    /// every session so the loop's wiring can be checked, spend a fixed number of
    /// tokens, and emit arm-appropriate tool calls (Arm B a real mutation unless
    /// told to omit it, so the evidence guard can be exercised both ways). Readers
    /// answer with a per-arm quality encoded as `q=<x>`, which the stub judge reads
    /// back — so a B > A delta can be arranged deterministically.
    struct StubRunner {
        writer_tokens: u64,
        reader_tokens: u64,
        a_quality: f64,
        b_quality: f64,
        arm_b_omits_mutation: bool,
        reader_emits_tells: bool,
        /// When set, writer sessions plant arm-identifying tells into the substrate
        /// they write, so the integrity-audit corpus carries them and the blinder
        /// must strip them before the auditor sees the corpus (criterion 10's
        /// blinding complement).
        writer_emits_tells: bool,
        /// When set, every writer/reader session reports this model instead of the
        /// pinned one it was invoked with — simulating a session that silently ran
        /// on an ambient model, which the pin-honor guard must reject.
        executed_model_override: Option<String>,
        seen: RefCell<Vec<(Arm, String, usize)>>,
        writes: RefCell<usize>,
        arm_a_dir: tempfile::TempDir,
        arm_b_dir: tempfile::TempDir,
    }

    impl StubRunner {
        fn new(writer_tokens: u64) -> Self {
            Self {
                writer_tokens,
                reader_tokens: 10,
                a_quality: 0.6,
                b_quality: 0.9,
                arm_b_omits_mutation: false,
                reader_emits_tells: false,
                writer_emits_tells: false,
                executed_model_override: None,
                seen: RefCell::new(Vec::new()),
                writes: RefCell::new(0),
                arm_a_dir: tempfile::tempdir().unwrap(),
                arm_b_dir: tempfile::tempdir().unwrap(),
            }
        }

        /// The model a session reports running on: the pinned `model` it was
        /// invoked with, unless an override forces a mismatch.
        fn reported_model(&self, model: &str) -> String {
            self.executed_model_override
                .clone()
                .unwrap_or_else(|| model.to_string())
        }
    }

    impl DivergenceRunner for StubRunner {
        fn write(
            &self,
            arm: Arm,
            model: &str,
            _prompt: &str,
            allowance: usize,
        ) -> Result<WriterOutcome> {
            self.seen
                .borrow_mut()
                .push((arm, model.to_string(), allowance));
            let n = {
                let mut w = self.writes.borrow_mut();
                *w += 1;
                *w
            };
            // Simulate substrate growth: Arm B writes a typed entity with a fresh
            // type + status + relation label (rich, growing vocabulary); Arm A
            // writes an untyped note (one type, no relation labels).
            let (dir, content, tool_calls) = match arm {
                Arm::A => (
                    &self.arm_a_dir,
                    "---\ntype: note\n---\n\nSee [[other]].".to_string(),
                    vec!["Write".to_string()],
                ),
                Arm::B if self.arm_b_omits_mutation => (
                    &self.arm_b_dir,
                    format!("---\ntype: t{n}\nstatus: accepted\n---\n\nREFERENCES x."),
                    vec!["memstead_search".to_string()],
                ),
                Arm::B => (
                    &self.arm_b_dir,
                    format!("---\ntype: t{n}\nstatus: accepted\n---\n\nREFERENCES x."),
                    vec!["memstead_create".to_string()],
                ),
            };
            // Optionally plant arm-identifying tells the audit-path blinder must
            // strip before the corpus reaches the auditor.
            let content = if self.writer_emits_tells {
                format!(
                    "{content}\n\nRecorded in the mounted mem via memstead_create; see the notes directory wikilink."
                )
            } else {
                content
            };
            std::fs::write(dir.path().join(format!("{n}.md")), content)?;
            Ok(WriterOutcome {
                tokens: self.writer_tokens,
                non_cache_tokens: self.writer_tokens,
                tool_calls,
                executed_model: self.reported_model(model),
            })
        }

        fn read(
            &self,
            arm: Arm,
            model: &str,
            _prompt: &str,
            _budget: usize,
        ) -> Result<ReaderOutcome> {
            let q = match arm {
                Arm::A => self.a_quality,
                Arm::B => self.b_quality,
            };
            // Optionally leak arm-identifying tells the blinder must strip before
            // the answer reaches the judge.
            let answer = if self.reader_emits_tells {
                format!("According to the mounted mem, via memstead_search, q={q}")
            } else {
                format!("q={q}")
            };
            Ok(ReaderOutcome {
                answer,
                tokens: self.reader_tokens,
                non_cache_tokens: self.reader_tokens,
                tool_calls: vec![],
                executed_model: self.reported_model(model),
            })
        }

        fn substrate_dir(&self, arm: Arm) -> &Path {
            match arm {
                Arm::A => self.arm_a_dir.path(),
                Arm::B => self.arm_b_dir.path(),
            }
        }
    }

    /// A judge that records every (blinded) answer AND every model id it is
    /// invoked with, so a test can prove no tell reached it and that it is pinned.
    struct RecordingJudge {
        seen: RefCell<Vec<String>>,
        models: RefCell<Vec<String>>,
    }
    impl RecordingJudge {
        fn new() -> Self {
            Self {
                seen: RefCell::new(Vec::new()),
                models: RefCell::new(Vec::new()),
            }
        }
    }
    impl DivergenceJudge for RecordingJudge {
        fn score(
            &self,
            model: &str,
            _reference: &str,
            blinded_answer: &str,
        ) -> Result<JudgeOutcome> {
            self.seen.borrow_mut().push(blinded_answer.to_string());
            self.models.borrow_mut().push(model.to_string());
            Ok(JudgeOutcome {
                score: 0.5,
                tokens: 5,
                non_cache_tokens: 5,
                executed_model: model.to_string(),
            })
        }
    }

    /// Scores the stub reader's `q=<x>` answer back to `x`, spending a fixed 5
    /// tokens per judgment. Honors the pin (reports the model it was invoked with).
    struct StubJudge;
    impl DivergenceJudge for StubJudge {
        fn score(
            &self,
            model: &str,
            _reference: &str,
            blinded_answer: &str,
        ) -> Result<JudgeOutcome> {
            let x = blinded_answer
                .strip_prefix("q=")
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            Ok(JudgeOutcome {
                score: x,
                tokens: 5,
                non_cache_tokens: 5,
                executed_model: model.to_string(),
            })
        }
    }

    /// A judge that silently runs on a different model than the pin — its round
    /// must be invalidated by the pin-honor guard.
    struct WrongModelJudge;
    impl DivergenceJudge for WrongModelJudge {
        fn score(
            &self,
            _model: &str,
            _reference: &str,
            _blinded_answer: &str,
        ) -> Result<JudgeOutcome> {
            Ok(JudgeOutcome {
                score: 0.5,
                tokens: 5,
                non_cache_tokens: 5,
                executed_model: "ambient-model".to_string(),
            })
        }
    }

    /// Counts corpus defects deterministically and blind: `duplicates` = items
    /// beyond the first that share an identical body (redundant items), so a corpus
    /// of identical notes (Arm A in the stub substrate) scores dirtier than one of
    /// distinct typed entities (Arm B) — arranged so the integrity delta A − B is
    /// positive without the auditor ever being told the arm. Spends 7 tokens per
    /// trial and honors the pin.
    struct StubAuditor;
    impl DivergenceAuditor for StubAuditor {
        fn audit(&self, model: &str, auditor_prompt: &str) -> Result<AuditOutcome> {
            use std::collections::BTreeSet;
            // The corpus reached us through the judge-path blinder, which collapses
            // whitespace — so split on the whitespace-free boundary token and trim
            // each item before counting identical (redundant) items.
            let items: Vec<&str> = auditor_prompt
                .split(CORPUS_ITEM_DELIM)
                .map(|s| s.trim())
                .collect();
            let distinct: BTreeSet<&str> = items.iter().copied().collect();
            let duplicates = items.len().saturating_sub(distinct.len());
            Ok(AuditOutcome {
                duplicates,
                contradictions: 0,
                tokens: 7,
                non_cache_tokens: 7,
                executed_model: model.to_string(),
            })
        }
    }

    /// Records every (blinded) corpus AND every model id it is invoked with, so a
    /// test can prove no tell reached the auditor and that it is pinned.
    struct RecordingAuditor {
        seen: RefCell<Vec<String>>,
        models: RefCell<Vec<String>>,
    }
    impl RecordingAuditor {
        fn new() -> Self {
            Self {
                seen: RefCell::new(Vec::new()),
                models: RefCell::new(Vec::new()),
            }
        }
    }
    impl DivergenceAuditor for RecordingAuditor {
        fn audit(&self, model: &str, auditor_prompt: &str) -> Result<AuditOutcome> {
            self.seen.borrow_mut().push(auditor_prompt.to_string());
            self.models.borrow_mut().push(model.to_string());
            Ok(AuditOutcome {
                duplicates: 1,
                contradictions: 0,
                tokens: 7,
                non_cache_tokens: 7,
                executed_model: model.to_string(),
            })
        }
    }

    /// An auditor that silently runs on a different model than the pin — its trial
    /// must be invalidated by the pin-honor guard rather than counted as zero.
    struct WrongModelAuditor;
    impl DivergenceAuditor for WrongModelAuditor {
        fn audit(&self, _model: &str, _blinded_corpus: &str) -> Result<AuditOutcome> {
            Ok(AuditOutcome {
                duplicates: 3,
                contradictions: 0,
                tokens: 7,
                non_cache_tokens: 7,
                executed_model: "ambient-model".to_string(),
            })
        }
    }

    fn two_queries() -> Vec<crate::eval::TaskSpec> {
        vec![
            crate::eval::TaskSpec {
                id: "q1".into(),
                prompt: "how many?".into(),
                reference: "ten".into(),
            },
            crate::eval::TaskSpec {
                id: "q2".into(),
                prompt: "what state?".into(),
                reference: "open".into(),
            },
        ]
    }

    fn ten_slices() -> Vec<String> {
        (1..=10).map(|i| format!("round {i} source")).collect()
    }

    #[test]
    fn writer_rounds_drive_both_arms_and_bill_the_ledger() {
        let dir = tmp();
        write_fixture_package(&dir);
        let pkg = Package::load(&dir).unwrap();
        let runner = StubRunner::new(100);
        let ledger = run_writer_rounds(&runner, &pkg, &ten_slices()).unwrap();

        // 10 rounds x 2 arms = 20 sessions x 100 tokens.
        assert_eq!(ledger.total(), 2_000);
        assert_eq!(ledger.total_role(Arm::B, Role::Writer), 1_000);
        assert_eq!(ledger.total_role(Arm::A, Role::Writer), 1_000);

        let seen = runner.seen.borrow();
        assert_eq!(seen.len(), 20);
        // Every session was invoked with the pinned model (criterion 3).
        assert!(seen.iter().all(|(_, m, _)| m == "claude-opus-4-8"));
        // Hurry rounds 3/6/9 carry the 4000 allowance, full rounds 8000.
        let arm_a_allowances: Vec<usize> = seen
            .iter()
            .filter(|(a, _, _)| *a == Arm::A)
            .map(|(_, _, al)| *al)
            .collect();
        assert_eq!(
            arm_a_allowances,
            vec![8000, 8000, 4000, 8000, 8000, 4000, 8000, 8000, 4000, 8000]
        );
    }

    #[test]
    fn writer_rounds_refuse_arm_b_without_a_mutation_call() {
        let dir = tmp();
        write_fixture_package(&dir);
        let pkg = Package::load(&dir).unwrap();
        let mut runner = StubRunner::new(100);
        runner.arm_b_omits_mutation = true;
        let err = run_writer_rounds(&runner, &pkg, &ten_slices())
            .unwrap_err()
            .to_string();
        assert!(err.contains("did not cross the MCP gate"), "{err}");
    }

    #[test]
    fn writer_rounds_abort_on_the_cost_cap() {
        let dir = tmp();
        write_fixture_package(&dir);
        let pkg = Package::load(&dir).unwrap();
        // Fixture cap is 20,000,000; 11M per session exceeds it within round 1.
        let runner = StubRunner::new(11_000_000);
        let err = run_writer_rounds(&runner, &pkg, &ten_slices())
            .unwrap_err()
            .to_string();
        assert!(err.contains("cost cap exceeded"), "{err}");
    }

    #[test]
    fn writer_rounds_refuse_a_slice_count_mismatch() {
        let dir = tmp();
        write_fixture_package(&dir);
        let pkg = Package::load(&dir).unwrap();
        let runner = StubRunner::new(100);
        let err = run_writer_rounds(&runner, &pkg, &["only one".to_string()])
            .unwrap_err()
            .to_string();
        assert!(err.contains("expected 10 round slices, got 1"), "{err}");
    }

    #[test]
    fn run_campaign_produces_checkpoints_ledger_and_b_minus_a_delta() {
        let dir = tmp();
        write_fixture_package(&dir);
        let pkg = Package::load(&dir).unwrap();
        let runner = StubRunner::new(100);
        let judge = StubJudge;
        let result = run_campaign(
            &runner,
            &judge,
            &StubAuditor,
            &pkg,
            &ten_slices(),
            &two_queries(),
            None,
        )
        .unwrap()
        .expect("campaign completed, not stopped");

        // Reader checkpoints at rounds 1, 3, 5, 10.
        let rounds: Vec<usize> = result.checkpoints.iter().map(|c| c.round).collect();
        assert_eq!(rounds, vec![1, 3, 5, 10]);
        // Each checkpoint scored both queries with delta = B - A = 0.9 - 0.6 = 0.3.
        for cp in &result.checkpoints {
            assert_eq!(cp.results.len(), 2);
            for r in &cp.results {
                assert!((r.delta - 0.3).abs() < 1e-9, "delta {} != 0.3", r.delta);
                assert!((r.on_mean - 0.9).abs() < 1e-9); // on = B
                assert!((r.off_mean - 0.6).abs() < 1e-9); // off = A
            }
        }

        // The entropy series has one sample per round, and shows the divergence:
        // Arm B's typed vocabulary grows while Arm A's untyped notes stay flat.
        assert_eq!(
            result.entropy_series.len(),
            10,
            "one entropy sample per round"
        );
        let first = &result.entropy_series[0];
        let last = result.entropy_series.last().unwrap();
        assert_eq!(first.round, 1);
        assert_eq!(last.round, 10);
        assert_eq!(first.arm_b.distinct_types, 1);
        assert_eq!(
            last.arm_b.distinct_types, 10,
            "Arm B type vocabulary grew each round"
        );
        assert_eq!(
            last.arm_a.distinct_types, 1,
            "Arm A stayed a single untyped 'note'"
        );
        assert_eq!(
            last.arm_b.distinct_relation_labels, 1,
            "Arm B entities carry a typed relation label"
        );
        assert_eq!(
            last.arm_a.distinct_relation_labels, 0,
            "untyped links carry no labels"
        );

        // Ledger spans writers, readers, and judges.
        // Writers: 10 rounds x 2 arms x 100 = 2000.
        assert_eq!(result.ledger.total_role(Arm::A, Role::Writer), 1_000);
        // Reader sessions: 4 checkpoints x 2 queries x 3 trials x 2 arms = 48;
        // readers 10 tokens each, judges 5 each.
        assert_eq!(
            result.ledger.total_role(Arm::B, Role::Reader),
            4 * 2 * 3 * 10
        );
        // Auditor sessions: 2 audit rounds x 2 arms x 3 trials = 12, at 7 tokens.
        assert_eq!(
            result.ledger.total(),
            2_000 + 48 * 10 + 48 * 5 + 12 * 7,
            "writers + readers + judges + auditors"
        );
    }

    #[test]
    fn campaign_result_serialises_to_a_report_artifact() {
        let dir = tmp();
        write_fixture_package(&dir);
        let pkg = Package::load(&dir).unwrap();
        let result = run_campaign(
            &StubRunner::new(100),
            &StubJudge,
            &StubAuditor,
            &pkg,
            &ten_slices(),
            &two_queries(),
            None,
        )
        .unwrap()
        .expect("campaign completed, not stopped");

        let json = result.to_json().unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();

        // Four checkpoints, each with two scored queries carrying the B-A delta.
        assert_eq!(v["checkpoints"].as_array().unwrap().len(), 4);
        assert_eq!(v["checkpoints"][0]["results"].as_array().unwrap().len(), 2);
        assert!((v["checkpoints"][0]["results"][0]["delta"].as_f64().unwrap() - 0.3).abs() < 1e-9);
        // Ten entropy samples.
        assert_eq!(v["entropy_series"].as_array().unwrap().len(), 10);
        assert_eq!(
            v["entropy_series"][9]["arm_b"]["distinct_types"]
                .as_u64()
                .unwrap(),
            10
        );
        // Two integrity checkpoints (audit rounds 5 and 10), each carrying the
        // A − B integrity delta and the per-arm item counts.
        let ic = v["integrity_checkpoints"].as_array().unwrap();
        assert_eq!(ic.len(), 2);
        assert_eq!(ic[0]["round"].as_u64().unwrap(), 5);
        assert_eq!(ic[1]["round"].as_u64().unwrap(), 10);
        assert!((ic[1]["result"]["delta"].as_f64().unwrap() - 90.0).abs() < 1e-9);
        assert_eq!(ic[1]["arm_a_items"].as_u64().unwrap(), 10);
        // The cost book: writers + readers + judges + auditors = 2720 + 84.
        assert_eq!(v["cost"]["total_tokens"].as_u64().unwrap(), 2_804);
        assert_eq!(v["cost"]["arm_b_writer"].as_u64().unwrap(), 1_000);
        assert_eq!(v["cost"]["auditor"].as_u64().unwrap(), 84);
    }

    #[test]
    fn judge_is_invoked_with_the_pinned_model() {
        // Criterion 3's refusal complement, judge half: every judge session is
        // invoked with the package's pinned model id explicitly.
        let dir = tmp();
        write_fixture_package(&dir);
        let pkg = Package::load(&dir).unwrap();
        let judge = RecordingJudge::new();
        run_campaign(
            &StubRunner::new(100),
            &judge,
            &StubAuditor,
            &pkg,
            &ten_slices(),
            &two_queries(),
            None,
        )
        .unwrap();
        let models = judge.models.borrow();
        assert!(!models.is_empty(), "the judge was invoked");
        assert!(
            models.iter().all(|m| m == &pkg.models.judge),
            "every judge invocation carried the pinned judge model {:?}, got {:?}",
            pkg.models.judge,
            models
        );
    }

    #[test]
    fn a_writer_on_the_wrong_model_invalidates_the_round() {
        // Criterion 3's refusal complement: a session that ran on an ambient model
        // instead of the pin invalidates its round rather than contributing silently.
        let dir = tmp();
        write_fixture_package(&dir);
        let pkg = Package::load(&dir).unwrap();
        let mut runner = StubRunner::new(100);
        runner.executed_model_override = Some("ambient-model".to_string());
        let err = run_campaign(
            &runner,
            &StubJudge,
            &StubAuditor,
            &pkg,
            &ten_slices(),
            &two_queries(),
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("writer") && err.contains("not honored"),
            "{err}"
        );
    }

    #[test]
    fn a_judge_on_the_wrong_model_invalidates_the_round() {
        // The pin-honor guard covers the judge session too, not just writers.
        let dir = tmp();
        write_fixture_package(&dir);
        let pkg = Package::load(&dir).unwrap();
        let err = run_campaign(
            &StubRunner::new(100),
            &WrongModelJudge,
            &StubAuditor,
            &pkg,
            &ten_slices(),
            &two_queries(),
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("judge") && err.contains("not honored"),
            "{err}"
        );
    }

    #[test]
    fn parse_session_extracts_the_executed_model() {
        // The model is read from the assistant message; a top-level `model` on the
        // system/result events is the fallback. The real runner maps it onto the
        // outcome's executed_model for the pin-honor guard.
        let from_message = r#"
{"type":"system","subtype":"init","model":"claude-opus-4-8"}
{"type":"assistant","message":{"model":"claude-opus-4-8","content":[{"type":"text","text":"ok"}]}}
{"type":"result","result":"ok","usage":{"output_tokens":3}}
"#;
        assert_eq!(
            parse_session(from_message).unwrap().model,
            "claude-opus-4-8"
        );

        // A stream with no model field anywhere → empty (which the guard rejects).
        let no_model = r#"{"type":"result","result":"ok"}"#;
        assert_eq!(parse_session(no_model).unwrap().model, "");
    }

    #[test]
    fn reader_answers_are_blinded_before_the_judge() {
        // Criterion 4: an answer embedding either arm's substrate vocabulary is
        // stripped before it reaches the judge — it never arrives raw.
        let dir = tmp();
        write_fixture_package(&dir);
        let pkg = Package::load(&dir).unwrap();
        let mut runner = StubRunner::new(100);
        runner.reader_emits_tells = true;
        let judge = RecordingJudge::new();
        run_campaign(
            &runner,
            &judge,
            &StubAuditor,
            &pkg,
            &ten_slices(),
            &two_queries(),
            None,
        )
        .unwrap();

        let seen = judge.seen.borrow();
        assert!(!seen.is_empty(), "the judge scored at least one answer");
        for input in seen.iter() {
            let low = input.to_lowercase();
            assert!(
                !low.contains("mounted mem"),
                "Arm-B phrase reached the judge: {input}"
            );
            assert!(
                !low.contains("memstead"),
                "tool token reached the judge: {input}"
            );
            // The substantive content survived the scrub.
            assert!(
                input.contains("q="),
                "the answer content was preserved: {input}"
            );
        }
    }

    #[test]
    fn a_resume_reloads_all_prior_results_not_just_the_counter() {
        // The whole point of the persistence: a resume must reconstruct the full
        // result (every checkpoint, integrity delta, entropy sample, ledger charge),
        // not emit a partial one missing the skipped rounds.
        let dir = tmp();
        write_fixture_package(&dir);
        let pkg = Package::load(&dir).unwrap();
        let state_path = dir.join("state.json");
        let runner = StubRunner::new(100);

        // First pass: a full run that persists its state after every round.
        let full = run_campaign(
            &runner,
            &StubJudge,
            &StubAuditor,
            &pkg,
            &ten_slices(),
            &two_queries(),
            Some(&state_path),
        )
        .unwrap()
        .expect("completed");

        // Second pass with the same state: every round is already done, so the loop
        // runs nothing — the result must come entirely from the reloaded state and
        // be identical to the uninterrupted run, not empty.
        let resumed = run_campaign(
            &runner,
            &StubJudge,
            &StubAuditor,
            &pkg,
            &ten_slices(),
            &two_queries(),
            Some(&state_path),
        )
        .unwrap()
        .expect("completed");

        assert_eq!(resumed.checkpoints.len(), 4, "all four reader checkpoints");
        assert_eq!(
            resumed.checkpoints.iter().map(|c| c.round).collect::<Vec<_>>(),
            full.checkpoints.iter().map(|c| c.round).collect::<Vec<_>>(),
        );
        assert_eq!(resumed.integrity_checkpoints.len(), 2, "both audits");
        assert_eq!(resumed.entropy_series.len(), 10, "every round's entropy");
        assert_eq!(
            resumed.ledger.total(),
            full.ledger.total(),
            "the reloaded ledger carries the full running total"
        );
    }

    #[test]
    fn a_stop_marker_pauses_cleanly_without_a_partial_result() {
        let dir = tmp();
        write_fixture_package(&dir);
        let pkg = Package::load(&dir).unwrap();
        let state_path = dir.join("state.json");
        // A STOP marker in the state dir pauses at the next round boundary.
        std::fs::write(dir.join("STOP"), b"").unwrap();
        let runner = StubRunner::new(100);

        let out = run_campaign(
            &runner,
            &StubJudge,
            &StubAuditor,
            &pkg,
            &ten_slices(),
            &two_queries(),
            Some(&state_path),
        )
        .unwrap();

        assert!(
            out.is_none(),
            "a STOP marker pauses (returns None), never a partial result"
        );
        // The marker is consumed so a resume does not immediately re-stop.
        assert!(!dir.join("STOP").exists(), "STOP marker consumed on pause");
    }

    #[test]
    fn partial_report_reconstructs_completed_rounds_with_no_verdict() {
        // The out-of-usage safeguard: a persisted state becomes a labelled partial
        // artifact carrying the real deltas for completed rounds and no band verdict.
        let dir = tmp();
        write_fixture_package(&dir);
        let pkg = Package::load(&dir).unwrap();
        let state_path = dir.join("state.json");
        let runner = StubRunner::new(100);
        run_campaign(
            &runner,
            &StubJudge,
            &StubAuditor,
            &pkg,
            &ten_slices(),
            &two_queries(),
            Some(&state_path),
        )
        .unwrap()
        .expect("completed");

        let json = partial_report_json(&state_path, pkg.campaign.cost_cap_tokens).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["partial"], true, "marked partial");
        assert_eq!(v["completed_rounds"], 10);
        // The real scored deltas are present (report flattened in), and no band
        // verdict is emitted anywhere for a partial.
        assert_eq!(v["checkpoints"].as_array().unwrap().len(), 4);
        assert_eq!(v["integrity_checkpoints"].as_array().unwrap().len(), 2);
        assert!(
            v.get("verdict").is_none() && v.get("band").is_none(),
            "a partial carries no band verdict"
        );
        assert!(v["note"].as_str().unwrap().contains("NO band verdict"));
    }

    #[test]
    fn resume_skips_completed_rounds_without_rerunning_writers() {
        let dir = tmp();
        write_fixture_package(&dir);
        let pkg = Package::load(&dir).unwrap();
        let state_path = dir.join("state.json");
        // Pre-seed state: rounds 1..=5 already completed, pinned to this package.
        CampaignState {
            pinned_hash: pkg.content_hash.clone(),
            completed_rounds: 5,
            ..Default::default()
        }
        .save(&state_path)
        .unwrap();

        let runner = StubRunner::new(100);
        let judge = StubJudge;
        run_campaign(
            &runner,
            &judge,
            &StubAuditor,
            &pkg,
            &ten_slices(),
            &two_queries(),
            Some(&state_path),
        )
        .unwrap();

        // Only rounds 6..=10 ran their writers — rounds 1..=5 were not re-run.
        let seen = runner.seen.borrow();
        let arm_a_rounds_run = seen.iter().filter(|(a, _, _)| *a == Arm::A).count();
        assert_eq!(arm_a_rounds_run, 5, "only rounds 6-10 write");
        // Allowances seen are those of rounds 6-10 (rounds 6 and 9 are hurry → 4000).
        let allowances: Vec<usize> = seen
            .iter()
            .filter(|(a, _, _)| *a == Arm::A)
            .map(|(_, _, al)| *al)
            .collect();
        assert_eq!(allowances, vec![4000, 8000, 8000, 4000, 8000]);
        // State advanced to round 10.
        assert_eq!(
            CampaignState::load(&state_path).unwrap().completed_rounds,
            10
        );
    }

    #[test]
    fn resume_refuses_a_package_edited_since_it_was_pinned() {
        let dir = tmp();
        write_fixture_package(&dir);
        let pkg = Package::load(&dir).unwrap();
        let state_path = dir.join("state.json");
        // State pinned to a different (earlier) package hash — as if the package
        // was edited mid-campaign.
        CampaignState {
            pinned_hash: "0000000000000000000000000000000000000000000000000000000000000000".into(),
            completed_rounds: 3,
            ..Default::default()
        }
        .save(&state_path)
        .unwrap();

        let runner = StubRunner::new(100);
        let judge = StubJudge;
        let err = run_campaign(
            &runner,
            &judge,
            &StubAuditor,
            &pkg,
            &ten_slices(),
            &two_queries(),
            Some(&state_path),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("package content hash changed"), "{err}");
    }

    #[test]
    fn fresh_run_seeds_and_advances_the_state_file() {
        let dir = tmp();
        write_fixture_package(&dir);
        let pkg = Package::load(&dir).unwrap();
        let state_path = dir.join("state.json");
        assert!(!state_path.exists());

        let runner = StubRunner::new(100);
        let judge = StubJudge;
        run_campaign(
            &runner,
            &judge,
            &StubAuditor,
            &pkg,
            &ten_slices(),
            &two_queries(),
            Some(&state_path),
        )
        .unwrap();

        let state = CampaignState::load(&state_path).unwrap();
        assert_eq!(state.completed_rounds, 10);
        assert_eq!(state.pinned_hash, pkg.content_hash);
    }

    #[test]
    fn single_model_refuses_a_confounded_pair() {
        let dir = tmp();
        write_fixture_package(&dir);
        fs::write(
            dir.join("models.json"),
            r#"{ "writer": "model-a", "reader": "model-b", "judge": "j", "auditor": "x" }"#,
        )
        .unwrap();
        let pkg = Package::load(&dir).unwrap();
        assert!(pkg.single_model().is_err());
    }

    #[test]
    fn integrity_audit_produces_an_a_minus_b_delta_and_bills_the_auditor() {
        // Criterion 10: at each audit checkpoint, per arm, n-trial auditor sessions
        // score the corpus; the campaign result carries the per-checkpoint integrity
        // delta (A − B). The stub substrate is dirtier on Arm A (identical notes,
        // all duplicates) than Arm B (distinct typed entities), so A − B is positive.
        let dir = tmp();
        write_fixture_package(&dir);
        let pkg = Package::load(&dir).unwrap();
        let runner = StubRunner::new(100);
        let result = run_campaign(
            &runner,
            &StubJudge,
            &StubAuditor,
            &pkg,
            &ten_slices(),
            &two_queries(),
            None,
        )
        .unwrap()
        .expect("campaign completed, not stopped");

        // Audit checkpoints at rounds 5 and 10 (integrity_audit_rounds).
        let ic = &result.integrity_checkpoints;
        assert_eq!(
            ic.iter().map(|c| c.round).collect::<Vec<_>>(),
            vec![5, 10],
            "one integrity checkpoint per audit round"
        );

        // Round 5: 5 items per arm. Arm A's five identical notes → 4 duplicates →
        // 80 defects/100; Arm B's five distinct entities → 0. Delta A − B = 80.
        let r5 = &ic[0];
        assert_eq!(r5.arm_a_items, 5);
        assert_eq!(r5.arm_b_items, 5);
        assert!(
            (r5.result.on_mean - 80.0).abs() < 1e-9,
            "{}",
            r5.result.on_mean
        ); // A
        assert!((r5.result.off_mean - 0.0).abs() < 1e-9); // B
        assert!(
            (r5.result.delta - 80.0).abs() < 1e-9,
            "integrity delta A − B = {}",
            r5.result.delta
        );

        // Round 10: 10 items per arm → 9 duplicates on Arm A → 90; Arm B still 0.
        let r10 = &ic[1];
        assert_eq!(r10.arm_a_items, 10);
        assert!(
            (r10.result.delta - 90.0).abs() < 1e-9,
            "{}",
            r10.result.delta
        );

        // The auditor is billed — the old always-zero Role::Auditor is impossible now.
        // 2 audit rounds x 3 trials x 7 tokens per arm = 42 each.
        assert_eq!(result.ledger.total_role(Arm::A, Role::Auditor), 42);
        assert_eq!(result.ledger.total_role(Arm::B, Role::Auditor), 42);
        assert!(result.ledger.summary().auditor > 0);
    }

    #[test]
    fn auditor_input_is_blinded_before_the_auditor() {
        // Criterion 10's blinding complement: the auditor's input is tell-stripped
        // exactly like the judge path — arm identity never reaches it. The writer
        // plants both arms' substrate vocabulary into the corpus; none survives.
        let dir = tmp();
        write_fixture_package(&dir);
        let pkg = Package::load(&dir).unwrap();
        let mut runner = StubRunner::new(100);
        runner.writer_emits_tells = true;
        let auditor = RecordingAuditor::new();
        run_campaign(
            &runner,
            &StubJudge,
            &auditor,
            &pkg,
            &ten_slices(),
            &two_queries(),
            None,
        )
        .unwrap();

        let seen = auditor.seen.borrow();
        assert!(!seen.is_empty(), "the auditor scored at least one corpus");
        for corpus in seen.iter() {
            let low = corpus.to_lowercase();
            for tell in [
                "the mounted mem",
                "memstead",
                "the notes directory",
                "wikilink",
            ] {
                assert!(
                    !low.contains(tell),
                    "tell {tell:?} reached the auditor: {corpus}"
                );
            }
        }
        // And every auditor invocation carried the pinned auditor model.
        let models = auditor.models.borrow();
        assert!(
            models.iter().all(|m| m == &pkg.models.auditor),
            "every auditor invocation carried the pinned auditor model {:?}, got {:?}",
            pkg.models.auditor,
            models
        );
    }

    #[test]
    fn an_auditor_on_the_wrong_model_invalidates_the_trial() {
        // Criterion 10's pin-honor complement: an auditor session that ran on an
        // ambient model instead of the pin invalidates its trial rather than
        // contributing a silent count.
        let dir = tmp();
        write_fixture_package(&dir);
        let pkg = Package::load(&dir).unwrap();
        let err = run_campaign(
            &StubRunner::new(100),
            &StubJudge,
            &WrongModelAuditor,
            &pkg,
            &ten_slices(),
            &two_queries(),
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("auditor") && err.contains("not honored"),
            "{err}"
        );
    }
}
