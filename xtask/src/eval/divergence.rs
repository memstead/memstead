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
use std::path::Path;

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
}

/// Which arm — the single variable under test. Arm A is the tolerant markdown
/// directory, Arm B the engine-gated mem.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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

/// The role a session played, for cost attribution in the [`Ledger`]. Staged
/// with the ledger ahead of the round-loop driver that constructs these.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
#[derive(Clone, Debug)]
struct Charge {
    arm: Arm,
    role: Role,
    tokens: u64,
}

#[allow(dead_code)]
impl Ledger {
    pub fn new(cap_tokens: u64) -> Self {
        Self {
            cap_tokens,
            charges: Vec::new(),
        }
    }

    /// Attribute `tokens` to `(arm, role)`. The only way tokens enter the ledger,
    /// so every recorded token has an arm and a role.
    pub fn record(&mut self, arm: Arm, role: Role, tokens: u64) {
        self.charges.push(Charge { arm, role, tokens });
    }

    /// Every token recorded, both arms, all roles.
    pub fn total(&self) -> u64 {
        self.charges.iter().map(|c| c.tokens).sum()
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

    /// Would recording `next` more tokens push the running total past the cap?
    /// Checked *before* a session so the campaign can abort cleanly with its
    /// state intact for resume, rather than overspending the cap.
    pub fn would_exceed(&self, next: u64) -> bool {
        self.total().saturating_add(next) > self.cap_tokens
    }

    /// Refuse once the recorded total has passed the cap — the between-sessions
    /// guard that aborts the campaign cleanly (state preserved for resume) rather
    /// than continuing to spend.
    pub fn check_cap(&self) -> Result<()> {
        let total = self.total();
        if total > self.cap_tokens {
            bail!(
                "campaign cost cap exceeded: {total} tokens spent, cap {} — aborting (state preserved for resume)",
                self.cap_tokens
            );
        }
        Ok(())
    }
}

/// What a writer session produced: the tokens it spent and the tools it called.
/// The tool calls are the criterion-5 evidence that Arm B's writes really crossed
/// the MCP mutation surface.
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct WriterOutcome {
    pub tokens: u64,
    pub tool_calls: Vec<String>,
}

/// What a reader session produced: the answer text (blinded before the judge
/// sees it), the tokens spent, and the tools called.
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct ReaderOutcome {
    pub answer: String,
    pub tokens: u64,
    pub tool_calls: Vec<String>,
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
/// this parallel trait.
#[allow(dead_code)]
pub trait DivergenceJudge {
    fn score(&self, reference: &str, blinded_answer: &str) -> Result<(f64, u64)>;
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
        validate_writer_evidence(arm, &out.tool_calls)?;
        ledger.record(arm, Role::Writer, out.tokens);
        ledger.check_cap()?;
    }
    Ok(())
}

/// One reader checkpoint's scored results: per query, `trials` reader sessions per
/// arm, blinded and judged, aggregated into a signed `B − A` delta per query.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct Checkpoint {
    pub round: usize,
    pub results: Vec<super::TaskResult>,
}

/// One round's vocabulary-entropy sample for both arms — the secondary,
/// judge-free divergence signal, computed from substrate bytes after the round's
/// writers ran.
#[allow(dead_code)]
#[derive(Clone, Debug)]
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
    pub entropy_series: Vec<RoundEntropy>,
    pub ledger: Ledger,
}

/// Resume state, persisted between rounds so a killed campaign continues without
/// re-running finished writer rounds. It pins the package by the content hash
/// recorded at campaign start: a resume against an edited package refuses
/// (criterion 2's refusal complement) rather than silently mixing two designs.
#[allow(dead_code)]
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CampaignState {
    pub pinned_hash: String,
    /// Highest round whose writer phase (and any checkpoint) fully completed.
    pub completed_rounds: usize,
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

/// Drive the full campaign on `slices` (one per round) and `queries` (the reader
/// battery): every round runs its writer sessions; at each reader checkpoint the
/// battery runs `trials` blinded, judged reader sessions per arm per query. One
/// ledger spans writers, readers, and judges; the cost cap is checked between
/// sessions throughout. Fully driven by the runner/judge traits, so a stub
/// exercises the whole loop without a network call (criterion 1).
/// `state_path`, when given, makes the campaign resumable: the state is persisted
/// after every round, and a restart with the same path skips the rounds already
/// completed (refusing if the package was edited since it was pinned). Passing
/// `None` runs a fresh, non-resumable campaign.
#[allow(dead_code)]
pub fn run_campaign<R: DivergenceRunner, J: DivergenceJudge>(
    runner: &R,
    judge: &J,
    package: &Package,
    slices: &[String],
    queries: &[super::TaskSpec],
    state_path: Option<&Path>,
) -> Result<CampaignResult> {
    let model = package.single_model()?.to_string();
    let schedule = package.campaign.schedule();
    require_slice_count(slices.len(), schedule.len())?;
    let tells = package.tell_lists.combined();
    let mut ledger = Ledger::new(package.campaign.cost_cap_tokens);
    let mut checkpoints = Vec::new();
    let mut entropy_series = Vec::new();

    // Determine where to start. A resume verifies the package pin and picks up
    // after the last completed round; a fresh run seeds the state at round 0.
    let start_round = match state_path {
        Some(path) if path.exists() => {
            let state = CampaignState::load(path)?;
            package.verify_pin(&state.pinned_hash)?;
            state.completed_rounds + 1
        }
        _ => {
            if let Some(path) = state_path {
                CampaignState {
                    pinned_hash: package.content_hash.clone(),
                    completed_rounds: 0,
                }
                .save(path)?;
            }
            1
        }
    };

    for rp in &schedule {
        if rp.round < start_round {
            continue;
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
                        let out = runner.read(
                            arm,
                            &model,
                            &prompt,
                            package.campaign.reader_budget_tokens,
                        )?;
                        ledger.record(arm, Role::Reader, out.tokens);
                        let blinded = super::grade::strip_tells_with(&out.answer, &tells);
                        let (score, judge_tokens) = judge.score(&query.reference, &blinded)?;
                        ledger.record(arm, Role::Judge, judge_tokens);
                        ledger.check_cap()?;
                        match arm {
                            Arm::A => a_scores.push(score),
                            Arm::B => b_scores.push(score),
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
        // Persist progress so a kill after this round resumes past it.
        if let Some(path) = state_path {
            CampaignState {
                pinned_hash: package.content_hash.clone(),
                completed_rounds: rp.round,
            }
            .save(path)?;
        }
    }
    Ok(CampaignResult {
        checkpoints,
        entropy_series,
        ledger,
    })
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
                 "reader_budget_tokens": 8000, "contamination_threshold": 0.5,
                 "cost_cap_tokens": 20000000 }"#,
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
                "reader_substrate": { "arm_a": "Read files.", "arm_b": "Read the mem." }
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
                "reader_substrate": { "arm_a": "a", "arm_b": "b" }
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
                 "reader_budget_tokens": 8000, "contamination_threshold": 0.5,
                 "cost_cap_tokens": 20000000 }"#,
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
        led.record(Arm::A, Role::Writer, 100);
        led.record(Arm::B, Role::Writer, 120);
        // Arm B's refusal-repair retry is charged to Arm B's writer cost.
        led.record(Arm::B, Role::Writer, 30);
        led.record(Arm::A, Role::Reader, 50);
        led.record(Arm::A, Role::Judge, 10);

        assert_eq!(led.total(), 310);
        assert_eq!(led.total_for(Arm::B), 150);
        assert_eq!(led.total_for(Arm::A), 160);
        assert_eq!(led.total_role(Arm::B, Role::Writer), 150);
        assert_eq!(led.total_role(Arm::A, Role::Reader), 50);
        assert_eq!(led.total_role(Arm::A, Role::Auditor), 0);
    }

    #[test]
    fn ledger_cost_cap_guards_before_and_after() {
        let mut led = Ledger::new(1_000);
        led.record(Arm::A, Role::Writer, 900);
        // Before a session: a 200-token session would exceed; a 100-token one fits.
        assert!(led.would_exceed(200));
        assert!(!led.would_exceed(100));
        // Still within cap after 900.
        assert!(led.check_cap().is_ok());
        // Overspend, then the between-sessions check refuses.
        led.record(Arm::B, Role::Writer, 200);
        assert_eq!(led.total(), 1_100);
        let err = led.check_cap().unwrap_err().to_string();
        assert!(err.contains("cost cap exceeded"), "{err}");
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
                seen: RefCell::new(Vec::new()),
                writes: RefCell::new(0),
                arm_a_dir: tempfile::tempdir().unwrap(),
                arm_b_dir: tempfile::tempdir().unwrap(),
            }
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
            std::fs::write(dir.path().join(format!("{n}.md")), content)?;
            Ok(WriterOutcome {
                tokens: self.writer_tokens,
                tool_calls,
            })
        }

        fn read(
            &self,
            arm: Arm,
            _model: &str,
            _prompt: &str,
            _budget: usize,
        ) -> Result<ReaderOutcome> {
            let q = match arm {
                Arm::A => self.a_quality,
                Arm::B => self.b_quality,
            };
            Ok(ReaderOutcome {
                answer: format!("q={q}"),
                tokens: self.reader_tokens,
                tool_calls: vec![],
            })
        }

        fn substrate_dir(&self, arm: Arm) -> &Path {
            match arm {
                Arm::A => self.arm_a_dir.path(),
                Arm::B => self.arm_b_dir.path(),
            }
        }
    }

    /// Scores the stub reader's `q=<x>` answer back to `x`, spending a fixed 5
    /// tokens per judgment.
    struct StubJudge;
    impl DivergenceJudge for StubJudge {
        fn score(&self, _reference: &str, blinded_answer: &str) -> Result<(f64, u64)> {
            let x = blinded_answer
                .strip_prefix("q=")
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            Ok((x, 5))
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
        let result =
            run_campaign(&runner, &judge, &pkg, &ten_slices(), &two_queries(), None).unwrap();

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
        assert_eq!(
            result.ledger.total(),
            2_000 + 48 * 10 + 48 * 5,
            "writers + readers + judges"
        );
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
        }
        .save(&state_path)
        .unwrap();

        let runner = StubRunner::new(100);
        let judge = StubJudge;
        run_campaign(
            &runner,
            &judge,
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
        }
        .save(&state_path)
        .unwrap();

        let runner = StubRunner::new(100);
        let judge = StubJudge;
        let err = run_campaign(
            &runner,
            &judge,
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
}
