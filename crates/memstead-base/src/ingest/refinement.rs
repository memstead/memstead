//! Refinement-mode brief — the scout/writer two-phase flow. Engine-side port
//! of the plugin's `refinementScoutBlock` / `refinementWriterBlock` and their
//! batch/findings state.
//!
//! One rotation of refinement walks the source files in `batch_size` batches.
//! Each run is either a **scout** pass (read a batch, note discrepancies into a
//! findings file) or a **writer** pass (act on the previous scout's findings).
//! The findings file is the only handover between the two phases:
//!   - a pending findings file present → writer pass (consume + delete it);
//!   - none → scout pass (emit the next batch + a findings-output instruction).
//!
//! Deterministic state (rotation, cursor, shuffled file order) lives engine-side
//! under `<workspace>/.memstead.cache/ingest/refinement/` — the same
//! engine-internal cache the mtime memo and backoff use.
//!
//! **Port note.** The plugin shuffles the file order with `Math.random`; this
//! port uses a small rotation-seeded PRNG so the order still varies across
//! rotations but is reproducible (no `rand` dependency). The behaviour
//! preserved is "each rotation covers the whole source set in a different batch
//! order"; the exact permutation is not load-bearing.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::pipeline::MediumType;

use super::cursor::enumerate_facet_files;
use super::resolve::{ResolvedIngest, ResolvedSource};

/// Findings older than this are treated as stale and dropped (10 minutes).
const FINDINGS_STALE_MS: u128 = 10 * 60 * 1000;

/// Per-ingest refinement batch state (persisted as JSON).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RefinementState {
    #[serde(default)]
    rotation: u64,
    #[serde(default)]
    cursor: usize,
    #[serde(default)]
    file_order: Vec<String>,
}

/// One scout batch: the files to review plus its position in the rotation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    /// The files this batch reviews (a `batch_size` slice of the rotation).
    pub files: Vec<String>,
    /// The current rotation number.
    pub rotation: u64,
    /// This batch's 1-based index within the rotation.
    pub batch_index: usize,
    /// The total number of batches in the rotation.
    pub total_batches: usize,
}

/// The `<workspace>/.memstead.cache/ingest/refinement/` directory.
fn refinement_dir(cache_root: &Path) -> PathBuf {
    cache_root.join("refinement")
}

fn state_path(cache_root: &Path, ingest_name: &str) -> PathBuf {
    refinement_dir(cache_root).join(format!("{ingest_name}.json"))
}

/// The findings-file path a scout writes and a writer consumes.
pub fn findings_path(cache_root: &Path, ingest_name: &str) -> PathBuf {
    refinement_dir(cache_root).join(format!("{ingest_name}-findings.md"))
}

/// The plural source-artifact term for the primary source's medium type —
/// mirrors the plugin's `mediums.json` `source.<type>.artifacts`.
fn source_artifacts(medium_type: MediumType) -> &'static str {
    match medium_type {
        MediumType::Codebase => "source files",
        MediumType::Filesystem => "files",
        MediumType::Graph => "entities",
        MediumType::Git => "commits",
        MediumType::Web => "web pages",
    }
}

/// The (source_artifacts, destination_artifacts) terms for an ingest. The
/// destination is always a graph, so its artifacts are always "entities"
/// (mirroring the plugin's `destinationMediumType` returning `graph`).
fn medium_terms(resolved: &ResolvedIngest) -> (&'static str, &'static str) {
    let source = resolved
        .sources
        .first()
        .map(|s| match s {
            ResolvedSource::Primary(p) => source_artifacts(p.medium_type),
            ResolvedSource::Reference { .. } => "entities",
        })
        .unwrap_or("source files");
    (source, "entities")
}

/// Enumerate the union of every source facet's files (sorted, de-duplicated).
/// Each facet's enumeration applies the ingest's `deny_paths` (the same
/// strategy-invariant deny set the git and mtime slices honour), so a denied
/// file never lands in a refinement batch.
fn enumerate_source_files(resolved: &ResolvedIngest, workspace_root: &Path) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    for source in &resolved.sources {
        if let ResolvedSource::Primary(p) = source {
            files.extend(enumerate_facet_files(
                p,
                &resolved.deny_paths,
                workspace_root,
            ));
        }
    }
    files.sort();
    files.dedup();
    files
}

/// A small rotation-seeded Fisher-Yates shuffle — reproducible, dependency-free.
fn shuffle(files: &mut [String], seed: u64) {
    let mut state = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    for i in (1..files.len()).rev() {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let j = ((state >> 33) as usize) % (i + 1);
        files.swap(i, j);
    }
}

fn load_state(cache_root: &Path, ingest_name: &str) -> Option<RefinementState> {
    let bytes = std::fs::read(state_path(cache_root, ingest_name)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn save_state(cache_root: &Path, ingest_name: &str, state: &RefinementState) {
    let path = state_path(cache_root, ingest_name);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut bytes) = serde_json::to_vec_pretty(state) {
        bytes.push(b'\n');
        let _ = std::fs::write(path, bytes);
    }
}

/// Advance to the next scout batch, starting (and shuffling) a new rotation when
/// the current one is exhausted. `None` when the ingest has no source files.
pub fn next_batch(
    resolved: &ResolvedIngest,
    workspace_root: &Path,
    cache_root: &Path,
) -> Option<Batch> {
    let batch_size = (resolved.batch_size as usize).max(1);
    let all_files = enumerate_source_files(resolved, workspace_root);
    if all_files.is_empty() {
        return None;
    }

    let mut state = load_state(cache_root, &resolved.name).unwrap_or_default();
    if state.file_order.is_empty() || state.cursor >= state.file_order.len() {
        // New rotation: bump the counter (only after a completed prior rotation)
        // and reshuffle the whole source set.
        let rotation = state.rotation + u64::from(!state.file_order.is_empty());
        let mut file_order = all_files;
        shuffle(&mut file_order, rotation);
        state = RefinementState {
            rotation,
            cursor: 0,
            file_order,
        };
    }

    let end = (state.cursor + batch_size).min(state.file_order.len());
    let files = state.file_order[state.cursor..end].to_vec();
    let batch_index = state.cursor / batch_size + 1;
    let total_batches = state.file_order.len().div_ceil(batch_size);
    state.cursor += files.len();
    save_state(cache_root, &resolved.name, &state);

    Some(Batch {
        files,
        rotation: state.rotation,
        batch_index,
        total_batches,
    })
}

/// Read a pending findings file for the ingest, or `None` when absent, stale
/// (> 10 minutes old — the file is deleted), empty, or an explicit "No findings."
pub fn read_pending_findings(cache_root: &Path, ingest_name: &str) -> Option<String> {
    let path = findings_path(cache_root, ingest_name);
    let content = std::fs::read_to_string(&path).ok()?;
    if let Ok(meta) = std::fs::metadata(&path)
        && let Ok(modified) = meta.modified()
        && let Ok(age) = SystemTime::now().duration_since(modified)
        && age.as_millis() > FINDINGS_STALE_MS
    {
        let _ = std::fs::remove_file(&path);
        return None;
    }
    let trimmed = content.trim();
    let lower = trimmed.to_lowercase();
    if trimmed.is_empty() || lower == "no findings." || lower == "no findings" {
        return None;
    }
    Some(trimmed.to_string())
}

/// Delete a consumed findings file (best-effort) — the writer phase does this
/// after reading it.
pub fn clear_findings(cache_root: &Path, ingest_name: &str) {
    let _ = std::fs::remove_file(findings_path(cache_root, ingest_name));
}

/// Render the `## Mode: refinement — scout phase` block. Byte-for-byte the
/// plugin's `refinementScoutBlock`.
pub fn render_refinement_scout(
    resolved: &ResolvedIngest,
    batch: &Batch,
    cache_root: &Path,
) -> String {
    let (source_artifacts, dest_artifacts) = medium_terms(resolved);
    let findings = findings_path(cache_root, &resolved.name);
    let findings = findings.to_string_lossy();
    let mut lines: Vec<String> = vec![
        format!(
            "> [scout | {}] rotation {}, batch {}/{} ({} {})",
            resolved.name,
            batch.rotation,
            batch.batch_index,
            batch.total_batches,
            batch.files.len(),
            source_artifacts
        ),
        String::new(),
        "## Mode: refinement — scout phase".to_string(),
        String::new(),
        format!(
            "The scout phase reads {source_artifacts} closely and notes discrepancies against the \
             existing destination {dest_artifacts}; the writer phase (next iteration of this \
             ingest) acts on those notes. Findings file is the only handover between the two phases."
        ),
        String::new(),
        "### This batch".to_string(),
        String::new(),
    ];
    for file in &batch.files {
        lines.push(format!("- {file}"));
    }
    lines.push(String::new());
    lines.push("### Output".to_string());
    lines.push(String::new());
    lines.push("Write findings to the file below via Bash:".to_string());
    lines.push(String::new());
    lines.push("```bash".to_string());
    lines.push(format!("cat > \"{findings}\" << 'FINDINGS_EOF'"));
    lines.push("# findings here".to_string());
    lines.push("FINDINGS_EOF".to_string());
    lines.push("```".to_string());
    lines.push(String::new());
    lines.push("If nothing meaningful turns up: write `No findings.` to the file. The next iteration's writer phase reads what you put there and acts. Quality debt that is real but out-of-scope for this batch — record it as `coverage_gap` / `verification_target` / `inconsistency` in the paired process mem if available, and note that you did so in the findings file.".to_string());
    format!("{}\n", lines.join("\n"))
}

/// Render the `## Mode: refinement — writer phase` block. Byte-for-byte the
/// plugin's `refinementWriterBlock`.
pub fn render_refinement_writer(resolved: &ResolvedIngest, findings: &str) -> String {
    let (source_artifacts, _) = medium_terms(resolved);
    let lines: Vec<String> = vec![
        format!("> [writer | {}] acting on scout findings", resolved.name),
        String::new(),
        "## Mode: refinement — writer phase".to_string(),
        String::new(),
        format!(
            "The previous iteration's scout produced the findings below. Read the cited \
             {source_artifacts} yourself before acting — the scout was working under context \
             pressure and may have missed nuance. If you find debt the scout missed, address it too."
        ),
        String::new(),
        "### Scout findings".to_string(),
        String::new(),
        findings.to_string(),
    ];
    format!("{}\n", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::resolve::ResolvedPrimarySource;
    use crate::pipeline::{IngestMode, IngestTrigger, PatternEntry, PatternMode};

    fn resolved(name: &str, batch_size: u32) -> ResolvedIngest {
        ResolvedIngest {
            name: name.to_string(),
            mode: IngestMode::Refinement,
            trigger: IngestTrigger::Loop,
            batch_size,
            deny_paths: vec![],
            projection_ref: format!("{name}/p"),
            projection_mem: name.to_string(),
            projection_name: "p".to_string(),
            intent: None,
            sources: vec![ResolvedSource::Primary(ResolvedPrimarySource {
                facet_ref: "f".to_string(),
                medium: "m".to_string(),
                medium_type: MediumType::Codebase,
                medium_pointer: String::new(),
                declared_change_detection: None,
                scope: vec![PatternEntry {
                    path: "**/*.rs".to_string(),
                    mode: PatternMode::Allow,
                }],
                preparation: None,
            })],
            destination_mem: name.to_string(),
            rules: None,
            post_actions: None,
        }
    }

    #[test]
    fn medium_terms_use_source_type_and_graph_destination() {
        let r = resolved("r", 20);
        assert_eq!(medium_terms(&r), ("source files", "entities"));
    }

    #[test]
    fn scout_block_lists_the_batch_and_output_instruction() {
        let r = resolved("ref", 20);
        let batch = Batch {
            files: vec!["a.rs".to_string(), "b.rs".to_string()],
            rotation: 0,
            batch_index: 1,
            total_batches: 3,
        };
        let out = render_refinement_scout(&r, &batch, Path::new("/c"));
        assert!(out.starts_with("> [scout | ref] rotation 0, batch 1/3 (2 source files)\n\n"));
        assert!(out.contains("## Mode: refinement — scout phase"));
        assert!(out.contains("### This batch\n\n- a.rs\n- b.rs\n"));
        assert!(out.contains("cat > \"/c/refinement/ref-findings.md\" << 'FINDINGS_EOF'"));
    }

    #[test]
    fn writer_block_carries_the_findings() {
        let r = resolved("ref", 20);
        let out = render_refinement_writer(&r, "spec X is stale");
        assert!(out.starts_with("> [writer | ref] acting on scout findings\n\n"));
        assert!(out.contains("## Mode: refinement — writer phase"));
        assert!(out.ends_with("### Scout findings\n\nspec X is stale\n"));
    }

    /// Batching walks the shuffled file set across a rotation, then reshuffles a
    /// new rotation once exhausted.
    #[test]
    fn next_batch_walks_a_rotation_then_starts_a_new_one() {
        let ws = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let root = ws.path();
        for i in 0..5 {
            std::fs::write(root.join(format!("f{i}.rs")), "").unwrap();
        }
        let r = resolved("ref", 2);

        let b1 = next_batch(&r, root, cache.path()).unwrap();
        assert_eq!(b1.rotation, 0);
        assert_eq!(b1.batch_index, 1);
        assert_eq!(b1.total_batches, 3); // ceil(5/2)
        assert_eq!(b1.files.len(), 2);

        let b2 = next_batch(&r, root, cache.path()).unwrap();
        assert_eq!(b2.batch_index, 2);
        let b3 = next_batch(&r, root, cache.path()).unwrap();
        assert_eq!(b3.batch_index, 3);
        assert_eq!(b3.files.len(), 1); // remainder

        // Rotation exhausted → next batch starts rotation 1.
        let b4 = next_batch(&r, root, cache.path()).unwrap();
        assert_eq!(b4.rotation, 1);
        assert_eq!(b4.batch_index, 1);

        // Every file appears exactly once across a rotation.
        let mut seen: Vec<String> = [b1.files, b2.files, b3.files].concat();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 5, "the rotation covers all files");
    }

    #[test]
    fn findings_empty_or_no_findings_read_as_none() {
        let cache = tempfile::tempdir().unwrap();
        let dir = refinement_dir(cache.path());
        std::fs::create_dir_all(&dir).unwrap();
        let fp = findings_path(cache.path(), "ref");

        std::fs::write(&fp, "  \n").unwrap();
        assert_eq!(read_pending_findings(cache.path(), "ref"), None);
        std::fs::write(&fp, "No findings.").unwrap();
        assert_eq!(read_pending_findings(cache.path(), "ref"), None);
        std::fs::write(&fp, "spec X is stale").unwrap();
        assert_eq!(
            read_pending_findings(cache.path(), "ref").as_deref(),
            Some("spec X is stale")
        );
    }
}
