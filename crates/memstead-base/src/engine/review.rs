//! Review marks — one per-mem pointer to the last human-approved
//! state (the operator's review model: diffs accumulate against it,
//! approving moves it, ignoring it entirely is first-class).
//!
//! The mark's value vocabulary is deliberately the existing
//! backend-opaque `changes_since` cursor (git-branch: commit SHA;
//! folder: changelog RFC3339-millis timestamp) — no second
//! state-naming scheme. Storage is mem-repo state via
//! `MemConfig.review_mark` (the `sync_state` precedent): it rides
//! reloads, survives cache wipes, is visible to every sibling process
//! opening the workspace, and is stripped from published archives by
//! the `PublishedMemConfig` allowlist.
//!
//! Marks never gate: no mutation path consults them. `set` takes an
//! explicit target only — never an implicit "now", because writers may
//! have advanced the mem mid-review.

use serde::Serialize;

use super::{Engine, EngineError};

/// One mem's review-mark status, alongside its current head so a
/// single list call answers "what has un-reviewed changes".
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ReviewMarkStatus {
    pub mem: String,
    /// The last human-approved state; `None` is the ordinary markless
    /// state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mark: Option<String>,
    /// Current head cursor (`None` for backends without one).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// Whether the mem is writable (marks on read-only mounts are
    /// visible but not settable).
    pub writable: bool,
}

/// Successful outcome of [`Engine::set_review_mark`].
#[derive(Debug, Clone, Serialize)]
pub struct SetReviewMarkOutcome {
    pub mem: String,
    /// The mark after this call (`None` = cleared).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mark: Option<String>,
    /// The mark before this call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<String>,
    /// Typed non-fatal issues (NOTE_MISSING under require-notes,
    /// MEM_RELOADED from the pre-write drift probe).
    pub warnings: Vec<crate::ops::WarningHint>,
}

impl Engine {
    /// Every mem's review mark (or its absence) with the current head.
    /// Markless mems are ordinary entries, never errors.
    pub fn review_marks(&self) -> Vec<ReviewMarkStatus> {
        self.mounts
            .iter()
            .map(|m| ReviewMarkStatus {
                mem: m.mount.mem.clone(),
                mark: m.mem_config.as_ref().and_then(|c| c.review_mark.clone()),
                head: m.backend.current_head().ok().flatten(),
                writable: m.mount.capability == crate::workspace::MountCapability::Write,
            })
            .collect()
    }

    /// Set (or clear, with `target: None`) a mem's review mark to an
    /// explicitly named state. The target is validated against the
    /// backend's cursor vocabulary before anything is written —
    /// git-branch cursors must resolve to a known commit, folder
    /// cursors must parse as the changelog's RFC3339 timestamp shape —
    /// and an invalid target refuses with `INVALID_CURSOR`, leaving
    /// the mark untouched. Provenance (note gating, warn-and-commit)
    /// mirrors `set_mem_sync_state`; the config write commits with
    /// the caller's note.
    pub fn set_review_mark(
        &mut self,
        mem_name: &str,
        target: Option<&str>,
        note: Option<&str>,
    ) -> Result<SetReviewMarkOutcome, EngineError> {
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem_name)
            .ok_or_else(|| EngineError::UnknownMem(mem_name.to_string()))?;
        if self.mounts[mount_idx].mount.capability != crate::workspace::MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(mem_name.to_string()));
        }

        // Validate the explicit target against the backend's cursor
        // vocabulary BEFORE any write. Clearing needs no validation.
        if let Some(target) = target {
            self.validate_review_cursor(mount_idx, mem_name, target)?;
        }

        // Same posture as every other commit-producing mutation.
        let mut warnings = self.reload_if_stale(Some(mem_name));
        if let Some(w) = self.note_missing_warning("set_review_mark", note) {
            warnings.push(w);
        }

        let mounted = &mut self.mounts[mount_idx];
        let mut config = mounted.mem_config.clone().ok_or_else(|| {
            EngineError::InvalidInput(format!(
                "mem '{mem_name}' has no loaded MemConfig — cannot set a review mark \
                 (initialize the mem via `memstead init` or `memstead mem create` first)"
            ))
        })?;

        let previous = config.review_mark.clone();
        config.review_mark = target.map(str::to_string);

        let mut bytes = serde_json::to_vec_pretty(&config).map_err(|e| {
            EngineError::InvalidInput(format!("could not serialize mem config: {e}"))
        })?;
        bytes.push(b'\n');
        mounted.backend.write_mem_config_with_note(&bytes, note)?;
        mounted.mem_config = Some(config);

        // Refresh the head cursor so the next drift probe doesn't
        // surface MEM_RELOADED for the commit we just produced.
        let new_head = mounted.backend.current_head().ok().flatten();
        if let Some(sha) = new_head {
            mounted.last_known_head = Some(sha);
        }

        Ok(SetReviewMarkOutcome {
            mem: mem_name.to_string(),
            mark: target.map(str::to_string),
            previous,
            warnings,
        })
    }

    /// The accumulated per-entity delta from the mem's review mark to
    /// its current head — exactly the envelopes `changes_since`
    /// reports for the mark's cursor. A markless mem refuses with
    /// `REVIEW_MARK_NOT_SET` (marklessness is known from the roster; a
    /// silent empty answer would equate "no mark" with "no changes").
    pub fn review_mark_diff(
        &self,
        mem_name: &str,
        rename_similarity: Option<f32>,
    ) -> Result<crate::ops::ChangesReport, EngineError> {
        let mount = self
            .mounts
            .iter()
            .find(|m| m.mount.mem == mem_name)
            .ok_or_else(|| EngineError::UnknownMem(mem_name.to_string()))?;
        let mark = mount
            .mem_config
            .as_ref()
            .and_then(|c| c.review_mark.clone())
            .ok_or_else(|| EngineError::ReviewMarkNotSet {
                mem: mem_name.to_string(),
            })?;
        self.changes_since(mem_name, &mark, rename_similarity)
    }

    /// Backend-vocabulary validation for an explicit mark target.
    fn validate_review_cursor(
        &self,
        mount_idx: usize,
        mem_name: &str,
        target: &str,
    ) -> Result<(), EngineError> {
        use crate::workspace::MountStorage;
        let invalid = || EngineError::InvalidChangesCursor {
            mem: mem_name.to_string(),
            since: target.to_string(),
        };
        match &self.mounts[mount_idx].mount.storage {
            MountStorage::Folder { .. } => {
                // Folder cursors are the changelog's RFC3339-millis
                // timestamps; format-level validation (existence is not
                // required — any parseable instant is a legal `since`).
                if crate::filesystem::changelog::parse_rfc3339_utc(target).is_none() {
                    return Err(invalid());
                }
                Ok(())
            }
            MountStorage::GitBranch { .. } => {
                // A git-branch cursor must resolve to a known commit —
                // `changes_since` is the authoritative resolver and
                // already refuses unknown SHAs with INVALID_CURSOR.
                self.changes_since(mem_name, target, None).map(|_| ())
            }
            MountStorage::Archive { .. } | MountStorage::InMemory => {
                // Unreachable through set (capability gate refuses
                // first); refuse defensively for direct callers.
                Err(invalid())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::storage::MemWriter;

    const SEED: &str = "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Seed\n\n## Identity\n\nSeed.\n";

    fn folder_engine(tmp: &tempfile::TempDir) -> crate::Engine {
        let dir = tmp.path().join("specs");
        if !dir.exists() {
            std::fs::create_dir_all(&dir).unwrap();
            let writer = crate::storage::FilesystemMemWriter::new(dir.clone());
            MemWriter::write_entity(&writer, std::path::Path::new("seed.md"), SEED.as_bytes())
                .unwrap();
            MemWriter::commit(&writer, "seed", &crate::vcs::CommitContext::internal()).unwrap();
            crate::backend::MemBackend::append_provenance(
                &writer,
                &crate::provenance::Provenance::new(
                    std::time::SystemTime::now(),
                    crate::provenance::ProvenanceKind::Create,
                    Some("specs--seed".into()),
                    crate::vcs::Actor::Cli,
                    None,
                    None,
                ),
            )
            .unwrap();
            // A config file so set_review_mark has a MemConfig to carry
            // the mark (mirrors an initialized mem).
            let config = memstead_schema::config::MemConfig {
                name: None,
                version: None,
                description: None,
                authors: None,
                schema: Some("default@1.0.0".parse().unwrap()),
                write_guidance: Default::default(),
                rules: None,
                publish: None,
                language: None,
                read_mems: Default::default(),
                community: None,
                vcs: None,
                unregistered_at: None,
                sync_state: Default::default(),
                review_mark: None,
                extra: Default::default(),
            };
            let meta = dir.join(memstead_schema::MEM_META_DIR);
            std::fs::create_dir_all(&meta).unwrap();
            std::fs::write(
                meta.join("config.json"),
                serde_json::to_vec_pretty(&config).unwrap(),
            )
            .unwrap();
        }
        let mount = crate::Mount {
            mem: "specs".to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                "default",
                semver::Version::new(1, 0, 0),
            )),
            storage: crate::MountStorage::Folder { path: dir.clone() },
            capability: crate::MountCapability::Write,
            lifecycle: crate::MountLifecycle::Eager,
            cross_linkable: false,
            migration_target: None,
        };
        let backend =
            Box::new(crate::storage::FilesystemMemWriter::new(dir)) as Box<dyn crate::MemBackend>;
        crate::Engine::from_mounts(vec![(mount, backend)]).unwrap()
    }

    fn head(engine: &crate::Engine) -> String {
        engine
            .review_marks()
            .into_iter()
            .find(|s| s.mem == "specs")
            .and_then(|s| s.head)
            .expect("folder head cursor")
    }

    #[test]
    fn mark_lifecycle_persists_validates_and_never_gates() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut engine = folder_engine(&tmp);

        // Markless start: ordinary state, listed as such.
        let status = &engine.review_marks()[0];
        assert_eq!(status.mem, "specs");
        assert!(status.mark.is_none());
        assert!(status.writable);

        // Invalid cursor refuses typed, mark untouched.
        let err = engine
            .set_review_mark("specs", Some("not-a-timestamp"), None)
            .unwrap_err();
        assert_eq!(err.code(), "INVALID_CURSOR");
        assert!(engine.review_marks()[0].mark.is_none());
        // Unknown mem refuses typed.
        let err = engine.set_review_mark("ghost", None, None).unwrap_err();
        assert_eq!(err.code(), "UNKNOWN_MEM");

        // Markless diff refuses typed — never a silent empty answer.
        let err = engine.review_mark_diff("specs", None).unwrap_err();
        assert_eq!(err.code(), "REVIEW_MARK_NOT_SET");

        // Set to the reviewed head; diff-since is now empty.
        let reviewed = head(&engine);
        let outcome = engine
            .set_review_mark("specs", Some(&reviewed), Some("reviewed everything"))
            .unwrap();
        assert_eq!(outcome.mark.as_deref(), Some(reviewed.as_str()));
        assert!(outcome.previous.is_none());
        let diff = engine.review_mark_diff("specs", None).unwrap();
        assert!(diff.changes.is_empty(), "reviewed head → empty: {diff:?}");

        // A mutation past the mark succeeds with no mark-related
        // warning (marks never gate) — and diff-since accumulates it,
        // matching changes_since for the mark's cursor.
        let outcome = engine
            .create_entity(
                crate::CreateEntityArgs {
                    mem: "specs".to_string(),
                    title: "Past The Mark".to_string(),
                    entity_type: "spec".to_string(),
                    sections: [
                        ("identity".to_string(), "x".to_string()),
                        ("purpose".to_string(), "y".to_string()),
                    ]
                    .into_iter()
                    .collect(),
                    metadata: Default::default(),
                    relations: Vec::new(),
                    anchors: Vec::new(),
                    dry_run: false,
                },
                crate::vcs::Actor::App,
                None,
                Some("agent work"),
            )
            .unwrap();
        assert!(
            outcome
                .warnings
                .iter()
                .all(|w| !format!("{w:?}").to_lowercase().contains("mark")),
            "marks never gate or warn: {:?}",
            outcome.warnings
        );
        let diff = engine.review_mark_diff("specs", None).unwrap();
        assert_eq!(diff.changes.len(), 1, "{diff:?}");
        let direct = engine.changes_since("specs", &reviewed, None).unwrap();
        assert_eq!(
            serde_json::to_value(&diff.changes).unwrap(),
            serde_json::to_value(&direct.changes).unwrap(),
            "diff-since must equal changes_since at the mark"
        );

        // Persistence across engine restarts (separate instance, same
        // workspace) — the sibling-visibility half of criterion 1.
        drop(engine);
        let mut second = folder_engine(&tmp);
        assert_eq!(
            second.review_marks()[0].mark.as_deref(),
            Some(reviewed.as_str()),
            "the mark is mem-repo state"
        );

        // Clear returns to markless.
        let outcome = second.set_review_mark("specs", None, None).unwrap();
        assert_eq!(outcome.previous.as_deref(), Some(reviewed.as_str()));
        assert!(second.review_marks()[0].mark.is_none());
    }

    #[test]
    fn noteless_set_under_require_notes_warns_and_commits() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut engine = folder_engine(&tmp);
        engine.set_settings(crate::WorkspaceSettings {
            mutations: crate::workspace::MutationsSection {
                require_notes: Some(true),
            },
            ..Default::default()
        });
        let reviewed = head(&engine);
        let outcome = engine
            .set_review_mark("specs", Some(&reviewed), None)
            .unwrap();
        assert!(
            outcome.warnings.iter().any(
                |w| matches!(w, crate::ops::WarningHint::NoteMissing { tool } if tool == "set_review_mark")
            ),
            "warn-and-commit: {:?}",
            outcome.warnings
        );
        assert_eq!(outcome.mark.as_deref(), Some(reviewed.as_str()));
    }

    #[test]
    fn published_projection_strips_the_mark() {
        // The PublishedMemConfig allowlist strips everything it does
        // not name — pin that the mark stays out (criterion 7's
        // structural half; the export round-trip rides the exporter's
        // own tests).
        let mut config = memstead_schema::config::MemConfig {
            name: None,
            version: Some(semver::Version::new(1, 0, 0)),
            description: None,
            authors: None,
            schema: Some("default@1.0.0".parse().unwrap()),
            write_guidance: Default::default(),
            rules: None,
            publish: None,
            language: None,
            read_mems: Default::default(),
            community: None,
            vcs: None,
            unregistered_at: None,
            sync_state: Default::default(),
            review_mark: None,
            extra: Default::default(),
        };
        config.review_mark = Some("deadbeef".to_string());
        let published = memstead_schema::config::published_config_from(&config, "specs").unwrap();
        let json = serde_json::to_string(&published).unwrap();
        assert!(
            !json.contains("reviewMark") && !json.contains("deadbeef"),
            "published config must strip the mark: {json}"
        );
    }

    #[test]
    fn overview_roster_carries_the_mark_and_its_indicator() {
        // The agents' cold-start read (overview `## Mems`) is a
        // per-mem summary surface: a set mark rides it with the
        // mark≠head indicator, a markless mem stays unmarked (ordinary
        // state, never flagged).
        let tmp = tempfile::TempDir::new().unwrap();
        let mut engine = folder_engine(&tmp);
        let overview_md = |engine: &mut crate::Engine| {
            crate::overview::compose_overview(
                engine,
                crate::overview::OverviewArgs {
                    include: &[],
                    mem: None,
                    rebuild: false,
                    token_budget: 8000,
                    operator_mode: false,
                    suppress_lifecycle: false,
                },
                crate::overview::Surface::Mcp,
            )
            .unwrap()
            .markdown
        };

        // Markless: no mark line anywhere.
        let md = overview_md(&mut engine);
        assert!(
            !md.contains("Review mark"),
            "markless roster must not mention marks: {md}"
        );

        // Mark at head: the line appears, indicator says at-mark.
        let reviewed = head(&engine);
        engine
            .set_review_mark("specs", Some(&reviewed), Some("reviewed"))
            .unwrap();
        let md = overview_md(&mut engine);
        assert!(
            md.contains(&format!("**Review mark:** `{reviewed}`")),
            "roster must carry the mark value: {md}"
        );
        assert!(
            md.contains("head is at the mark"),
            "at-mark indicator missing: {md}"
        );

        // Head moves past the mark: the indicator flips and names the
        // composition path (changes_since with the mark's cursor).
        engine
            .create_entity(
                crate::CreateEntityArgs {
                    mem: "specs".to_string(),
                    title: "Past The Mark Roster".to_string(),
                    entity_type: "spec".to_string(),
                    sections: [
                        ("identity".to_string(), "x".to_string()),
                        ("purpose".to_string(), "y".to_string()),
                    ]
                    .into_iter()
                    .collect(),
                    metadata: Default::default(),
                    relations: Vec::new(),
                    anchors: Vec::new(),
                    dry_run: false,
                },
                crate::vcs::Actor::App,
                None,
                Some("agent work"),
            )
            .unwrap();
        let md = overview_md(&mut engine);
        assert!(
            md.contains("head has moved past the mark") && md.contains("changes_since"),
            "unreviewed indicator missing: {md}"
        );
    }
}
