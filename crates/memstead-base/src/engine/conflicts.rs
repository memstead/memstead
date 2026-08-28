//! Merge-conflict resolution for folder mems (backlog-sweep plan 07,
//! decision 20).
//!
//! A hand-committed folder mem lives inside the user's own git
//! repository, so an ordinary merge can write conflict markers into
//! entity files. At that moment every other door is locked by design:
//! the loader refuses the file (naming this operation as the remedy),
//! and the guards correctly block git verbs and raw edits against mem
//! content. This module is the one sanctioned door — the agent judges
//! each conflict on its content and the engine is the pair of hands:
//! the chosen side is validated as an entity BEFORE it lands (a broken
//! ours side never launders into the mem), and the resolution commits
//! as an attributed, note-carrying mutation like any other write.
//!
//! Scope is deliberately narrow: per-entity, two sides (ours/theirs),
//! folder backend only. A merged-content resolution is out of scope by
//! design — an agent wanting a merge resolves to one side as the base
//! and then edits through the normal mutation surface, which preserves
//! validation and provenance; the operation's note is the designated
//! place to record "base for a manual merge; discarded side: <which>".
//! The git-branch backend's mem-repo is engine-managed and cannot
//! acquire merge conflicts through supported use, so it refuses typed
//! rather than pretending applicability.

use std::path::Path;

use crate::entity::id::file_path_to_id;
use crate::entity::{EntityId, loader, parser, source::EntitySource};
use crate::provenance::{Provenance, ProvenanceKind};
use crate::vcs::{Actor, ClientId, CommitContext};
use crate::workspace::{MountCapability, MountStorage};

use super::{Engine, EngineError};

/// Which side of a git merge conflict to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictSide {
    Ours,
    Theirs,
}

impl ConflictSide {
    /// Parse the wire token (`"ours"` / `"theirs"`). `None` for an
    /// unrecognized token so the calling surface raises a typed error
    /// naming the bad value.
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "ours" => Some(Self::Ours),
            "theirs" => Some(Self::Theirs),
            _ => None,
        }
    }

    /// The wire token for this side.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Ours => "ours",
            Self::Theirs => "theirs",
        }
    }
}

/// One conflicted entity file found in a folder mem.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConflictedEntity {
    /// The entity id the file's path derives to — the handle
    /// `resolve_merge_conflict` accepts.
    pub id: EntityId,
    pub mem: String,
    /// Mem-relative file path, for human orientation.
    pub file_path: String,
}

/// Outcome of a successful [`Engine::resolve_merge_conflict`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct ResolveConflictOutcome {
    pub id: EntityId,
    /// The side that was kept (`"ours"` / `"theirs"`).
    pub side: &'static str,
    pub write_id: String,
    /// Carries `CONFIG_WRITE_INTERVENED` when the mutation version stamp this
    /// resolution triggered merged over another writer's config change
    /// (04/03, criterion 3). Empty on the ordinary path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<crate::ops::WarningHint>,
}

/// Extract one side of a git merge conflict from raw file content.
///
/// Handles the standard two-way marker layout and the diff3 variant
/// (`|||||||` base section, dropped from both sides). Operates on raw
/// lines exactly as git wrote them — git places markers at line starts
/// without regard for markdown structure. `Err` carries a description
/// of the malformation (e.g. a start marker with no closing marker).
pub fn extract_conflict_side(content: &str, side: ConflictSide) -> Result<String, String> {
    #[derive(PartialEq)]
    enum State {
        Normal,
        Ours,
        Base,
        Theirs,
    }
    let mut state = State::Normal;
    let mut out: Vec<&str> = Vec::new();
    for (n, line) in content.lines().enumerate() {
        match state {
            State::Normal => {
                if line.starts_with("<<<<<<< ") {
                    state = State::Ours;
                } else {
                    out.push(line);
                }
            }
            State::Ours => {
                if line.starts_with("|||||||") {
                    state = State::Base;
                } else if line.trim_end() == "=======" {
                    state = State::Theirs;
                } else if line.starts_with(">>>>>>> ") {
                    return Err(format!(
                        "line {}: end marker before `=======` separator",
                        n + 1
                    ));
                } else if side == ConflictSide::Ours {
                    out.push(line);
                }
            }
            State::Base => {
                if line.trim_end() == "=======" {
                    state = State::Theirs;
                }
                // base-section lines belong to neither side
            }
            State::Theirs => {
                if line.starts_with(">>>>>>> ") {
                    state = State::Normal;
                } else if side == ConflictSide::Theirs {
                    out.push(line);
                }
            }
        }
    }
    if state != State::Normal {
        return Err("unterminated conflict block (no `>>>>>>> ` end marker)".to_string());
    }
    let mut resolved = out.join("\n");
    if content.ends_with('\n') && !resolved.ends_with('\n') {
        resolved.push('\n');
    }
    Ok(resolved)
}

impl Engine {
    /// Resolve a mem name to its writable FOLDER mount. The visibility
    /// gate mirrors `search`'s (quarantined or invisible → the same
    /// `UNKNOWN_MEM` refusal); a visible non-folder mem refuses
    /// `CONFLICT_RESOLVE_UNSUPPORTED_BACKEND`.
    fn folder_mount(&self, mem: &str) -> Result<(usize, std::path::PathBuf), EngineError> {
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem)
            .ok_or_else(|| self.unknown_mem_error(mem))?;
        if self.quarantine_reason(mem).is_some() {
            return Err(self.unknown_mem_error(mem));
        }
        if self.mounts[mount_idx].mount.capability != MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(mem.to_string()));
        }
        match &self.mounts[mount_idx].mount.storage {
            MountStorage::Folder { path } => Ok((mount_idx, path.clone())),
            _ => Err(EngineError::MergeConflictUnsupportedBackend {
                mem: mem.to_string(),
            }),
        }
    }

    /// List every entity file carrying git merge-conflict markers.
    ///
    /// `mem: Some(name)` scopes to that mem and refuses typed when it
    /// is unknown or not folder-backed; `None` sweeps every writable
    /// folder mem (non-folder mounts are simply not applicable and are
    /// skipped — the unscoped sweep answers "what is conflicted",
    /// never "which backends exist").
    pub fn list_merge_conflicts(
        &self,
        mem: Option<&str>,
    ) -> Result<Vec<ConflictedEntity>, EngineError> {
        let targets: Vec<(String, std::path::PathBuf)> = match mem {
            Some(name) => {
                let (_, root) = self.folder_mount(name)?;
                vec![(name.to_string(), root)]
            }
            None => self
                .mounts
                .iter()
                .filter(|m| m.mount.capability == MountCapability::Write)
                .filter_map(|m| match &m.mount.storage {
                    MountStorage::Folder { path } => Some((m.mount.mem.clone(), path.clone())),
                    _ => None,
                })
                .collect(),
        };
        let mut out = Vec::new();
        for (mem_name, root) in targets {
            let (entries, _read_errors) = EntitySource::Directory { root }
                .read_all()
                .map_err(|e| EngineError::InvalidInput(format!("read mem directory: {e}")))?;
            for entry in entries {
                if parser::has_merge_conflict_markers(&entry.content) {
                    out.push(ConflictedEntity {
                        id: file_path_to_id(&entry.relative_path, &mem_name),
                        mem: mem_name.clone(),
                        file_path: entry.relative_path,
                    });
                }
            }
        }
        out.sort_by(|a, b| a.id.0.cmp(&b.id.0));
        Ok(out)
    }

    /// Resolve one conflicted entity to the chosen side.
    ///
    /// The chosen side must parse as a valid entity against the mem's
    /// schema BEFORE anything is written — resolution never launders an
    /// invalid entity into the mem. On success the resolved content is
    /// written through the mem's backend, committed with an attributed
    /// [`CommitContext`] (note included when given), recorded in the
    /// provenance ledger, and the mem is reloaded so the entity reads
    /// validly and the conflict load-error clears.
    pub fn resolve_merge_conflict(
        &mut self,
        id: &EntityId,
        side: ConflictSide,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Result<ResolveConflictOutcome, EngineError> {
        let mem = id.mem().to_string();
        let (mount_idx, root) = self.folder_mount(&mem)?;

        // Locate the file whose path derives to the requested id. The
        // conflicted entity is NOT in the store (its file refused to
        // load), so the lookup goes over the source files directly.
        let (entries, _read_errors) = EntitySource::Directory { root }
            .read_all()
            .map_err(|e| EngineError::InvalidInput(format!("read mem directory: {e}")))?;
        let Some(entry) = entries
            .into_iter()
            .find(|e| file_path_to_id(&e.relative_path, &mem) == *id)
        else {
            return Err(EngineError::NotFound { id: id.to_string() });
        };
        if !parser::has_merge_conflict_markers(&entry.content) {
            return Err(EngineError::NotConflicted { id: id.to_string() });
        }

        let resolved = extract_conflict_side(&entry.content, side).map_err(|m| {
            EngineError::InvalidInput(format!(
                "malformed conflict markers in {}: {m}",
                entry.relative_path
            ))
        })?;

        // Validate the chosen side as an entity BEFORE any write —
        // resolution never launders an invalid entity into the mem.
        // Load-grade first: the chosen side must itself be free of
        // conflict markers (a nested conflict from a recursive merge
        // leaves residue in one side), or the mem would refuse to load
        // it right back. Then write-grade: the tolerant parser accepts
        // nearly anything, so the schema checks the mutation surface
        // applies to section shape run here too — unknown section keys
        // and content-format violations refuse with the same typed
        // validation errors a write would raise. Missing required
        // sections stay soft on purpose, matching `memstead_update`'s
        // permissive posture: resolution is an update-kind mutation on
        // an entity that already exists, and refusing here could leave
        // BOTH sides unresolvable — a locked door again.
        if parser::has_merge_conflict_markers(&resolved) {
            return Err(EngineError::InvalidInput(format!(
                "the {} side of {} still carries conflict markers (nested conflict) — \
                 refusing to write it; resolve the other side or repair upstream first",
                side.as_wire(),
                entry.relative_path
            )));
        }
        let schema = self
            .schemas
            .get(&mem)
            .cloned()
            .ok_or_else(|| self.unknown_mem_error(&mem))?;
        let resolved_type = loader::resolve_type_for_entry(&schema, &resolved);
        let parsed = parser::parse_markdown(
            &resolved,
            &entry.relative_path,
            resolved_type.as_ref(),
            &mem,
        )?;
        crate::runtime_validator::validate_section_keys(
            parsed.entity.sections.keys().map(String::as_str),
            resolved_type.as_ref(),
        )?;
        let mut heading_buf: Vec<&str> = Vec::new();
        let catch_all =
            crate::runtime_validator::catch_all_context(resolved_type.as_ref(), &mut heading_buf);
        crate::runtime_validator::validate_section_content(
            parsed
                .entity
                .sections
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str())),
            catch_all,
        )?;

        let backend = self.mounts[mount_idx].backend.as_ref();
        backend.write_entity(Path::new(&entry.relative_path), resolved.as_bytes())?;
        let ctx = CommitContext {
            actor,
            client: client.cloned(),
            tool: Some("resolve_conflict"),
            note: note.map(String::from),
            role: self.current_role,
            identity: self.current_identity.clone(),
            logical_operation_id: None,
            entity_ids: None,
        };
        let write_id = backend.commit(
            &format!("memstead: resolve-conflict {id} (side: {})", side.as_wire()),
            &ctx,
        )?;
        backend.append_provenance(
            &Provenance::new(
                std::time::SystemTime::now(),
                ProvenanceKind::Update,
                Some(id.to_string()),
                actor,
                client.cloned(),
                note.map(String::from),
            )
            .with_role(self.current_role)
            .with_identity(self.current_identity.clone()),
        )?;
        self.record_self_write(mount_idx, &write_id);
        let stamp_warnings = self.stamp_mutation_versions(mount_idx);

        // Reload so the resolved entity enters the store and the
        // conflict load-error clears — the caller's next read sees a
        // clean mem, not a stale refusal.
        self.reload_each_writable_mem()?;

        Ok(ResolveConflictOutcome {
            warnings: stamp_warnings,
            id: id.clone(),
            side: side.as_wire(),
            write_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFLICTED: &str = "---\ntype: spec\n---\n# Torn\n\n## Identity\n\n\
<<<<<<< HEAD\nours line\n||||||| base\nbase line\n=======\ntheirs line\n\
>>>>>>> feature\n\n## Purpose\n\nshared tail\n";

    /// Build a booted folder workspace with one mem (`specs`) whose
    /// files are exactly `files`. Returns `(tempdir, engine)`.
    fn folder_workspace(files: &[(&str, &str)]) -> (tempfile::TempDir, Engine) {
        use crate::workspace::{Mount, MountLifecycle};
        use crate::workspace_store::WorkspaceStoreAdapter;

        let tmp = tempfile::TempDir::new().unwrap();
        let mem_dir = tmp.path().join("specs");
        std::fs::create_dir_all(&mem_dir).unwrap();
        for (name, content) in files {
            std::fs::write(mem_dir.join(name), content).unwrap();
        }
        let memstead = tmp.path().join(".memstead");
        std::fs::create_dir_all(&memstead).unwrap();
        std::fs::write(
            memstead.join("workspace.toml"),
            "format = \"memstead-git-branch-2\"\n\n[persistence_adapter]\nname = \"file-two-layer\"\n",
        )
        .unwrap();
        let mount = Mount {
            mem: "specs".to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                "default",
                semver::Version::new(1, 0, 0),
            )),
            storage: MountStorage::Folder { path: mem_dir },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        crate::FileWorkspaceStore::new()
            .save_state(
                tmp.path(),
                &crate::workspace::Workspace {
                    mounts: vec![mount],
                    settings: crate::workspace::WorkspaceSettings::default(),
                },
            )
            .unwrap();
        let engine = Engine::from_workspace_root(tmp.path()).expect("workspace boots");
        (tmp, engine)
    }

    const CLEAN: &str = "---\ntype: spec\n---\n# Fine\n\n## Identity\n\nis\n\n## Purpose\n\nok\n";

    /// Plan 07 criteria 1/3/4 on a live folder workspace: the load
    /// refusal names the resolve remedy, the conflicted entity lists,
    /// resolving to theirs makes the mem load clean with the entity
    /// valid, and the resolution lands in the provenance ledger with
    /// its note. Complements: resolving the already-clean entity
    /// refuses `NOT_CONFLICTED`; a missing id refuses not-found.
    #[test]
    fn conflicted_folder_entity_lists_resolves_and_reads_clean() {
        let (tmp, mut engine) = folder_workspace(&[("torn.md", CONFLICTED), ("fine.md", CLEAN)]);

        // The parse failure an agent hits names the remedy.
        let errors = engine.load_errors();
        assert_eq!(errors.len(), 1, "exactly the conflicted file refuses");
        assert!(
            errors[0].1.contains("memstead conflicts resolve"),
            "load error names the resolve operation: {}",
            errors[0].1
        );

        // The conflicted entity is identified; the clean one is not.
        let listed = engine.list_merge_conflicts(None).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id.as_ref(), "specs--torn");
        assert_eq!(listed[0].file_path, "torn.md");

        // Resolve to theirs.
        let id = EntityId("specs--torn".into());
        let outcome = engine
            .resolve_merge_conflict(
                &id,
                ConflictSide::Theirs,
                Actor::Cli,
                None,
                Some("keeping upstream wording"),
            )
            .expect("resolution succeeds");
        assert_eq!(outcome.side, "theirs");

        // The mem loads clean and the entity reads validly.
        assert!(
            engine.load_errors().is_empty(),
            "{:?}",
            engine.load_errors()
        );
        let entity = engine.get_entity(&id).expect("resolved entity is loaded");
        assert!(!entity.stub);
        assert!(
            entity
                .sections
                .get("identity")
                .unwrap()
                .contains("theirs line"),
            "the kept side's content is live: {:?}",
            entity.sections.get("identity")
        );
        let on_disk = std::fs::read_to_string(tmp.path().join("specs").join("torn.md")).unwrap();
        assert!(!on_disk.contains("<<<<<<<") && !on_disk.contains("ours line"));

        // Provenance: the resolution is an attributed ledger entry
        // carrying the note — never an untracked file swap.
        let ledger = std::fs::read_to_string(
            tmp.path()
                .join("specs")
                .join(".memstead")
                .join("changes.jsonl"),
        )
        .expect("folder provenance ledger exists");
        assert!(
            ledger.contains("specs--torn") && ledger.contains("keeping upstream wording"),
            "ledger records the resolution with its note: {ledger}"
        );

        // Complements: already-clean refuses NOT_CONFLICTED; unknown
        // id refuses not-found.
        let err = engine
            .resolve_merge_conflict(&id, ConflictSide::Ours, Actor::Cli, None, None)
            .unwrap_err();
        assert_eq!(err.code(), "NOT_CONFLICTED");
        let err = engine
            .resolve_merge_conflict(
                &EntityId("specs--absent".into()),
                ConflictSide::Ours,
                Actor::Cli,
                None,
                None,
            )
            .unwrap_err();
        assert_eq!(err.code(), "ENTITY_NOT_FOUND");
    }

    /// Complement: a fenced code example DOCUMENTING conflict markers
    /// is legal content — it loads without a conflict refusal and does
    /// not list as conflicted (the detector evaluates masked content).
    #[test]
    fn fenced_marker_example_is_not_a_conflict() {
        let doc = "---\ntype: spec\n---\n# Git Lore\n\n## Identity\n\n\
```text\n<<<<<<< HEAD\nexample\n=======\nexample\n>>>>>>> branch\n```\n\n\
## Purpose\n\nteaching\n";
        let (_tmp, engine) = folder_workspace(&[("lore.md", doc)]);
        assert!(
            engine.load_errors().is_empty(),
            "{:?}",
            engine.load_errors()
        );
        assert!(engine.list_merge_conflicts(None).unwrap().is_empty());
        assert!(engine.get_entity(&EntityId("specs--lore".into())).is_some());
    }

    /// Plan 07 criterion 2: a chosen side that fails entity validation
    /// refuses with the validation error and writes nothing. The
    /// fixture is a nested conflict (recursive-merge shape): the
    /// theirs side still carries marker residue after extraction, so
    /// writing it would put the mem right back into the unloadable
    /// state — resolution refuses; the clean ours side resolves.
    #[test]
    fn invalid_chosen_side_refuses_and_writes_nothing() {
        let nested = "---\ntype: spec\n---\n# Nested\n\n## Identity\n\n\
<<<<<<< HEAD\nours\n=======\n<<<<<<< inner\ntheirs-a\n=======\ntheirs-b\n\
>>>>>>> inner\n>>>>>>> outer\n\n## Purpose\n\np\n";
        let (tmp, mut engine) = folder_workspace(&[("nested.md", nested)]);

        let id = EntityId("specs--nested".into());
        let err = engine
            .resolve_merge_conflict(&id, ConflictSide::Theirs, Actor::Cli, None, None)
            .unwrap_err();
        assert_eq!(err.code(), "INVALID_INPUT", "got: {err}");
        assert!(
            err.to_string().contains("still carries conflict markers"),
            "refusal names the residue: {err}"
        );
        // Nothing was written: the original markers are still on disk.
        let on_disk = std::fs::read_to_string(tmp.path().join("specs").join("nested.md")).unwrap();
        assert!(
            on_disk.contains("<<<<<<< HEAD"),
            "file untouched on refusal"
        );

        // The ours side is clean and resolves fine.
        engine
            .resolve_merge_conflict(&id, ConflictSide::Ours, Actor::Cli, None, None)
            .expect("clean side resolves");
        assert!(engine.load_errors().is_empty());
    }

    #[test]
    fn extract_sides_and_diff3_base_drops() {
        let ours = extract_conflict_side(CONFLICTED, ConflictSide::Ours).unwrap();
        assert!(ours.contains("ours line"));
        assert!(!ours.contains("theirs line") && !ours.contains("base line"));
        assert!(ours.contains("shared tail"));
        let theirs = extract_conflict_side(CONFLICTED, ConflictSide::Theirs).unwrap();
        assert!(theirs.contains("theirs line"));
        assert!(!theirs.contains("ours line") && !theirs.contains("base line"));
        assert!(!theirs.contains("<<<<<<<") && !theirs.contains(">>>>>>>"));
    }

    #[test]
    fn malformed_markers_refuse() {
        let unterminated = "a\n<<<<<<< HEAD\nours\n=======\ntheirs\n";
        assert!(extract_conflict_side(unterminated, ConflictSide::Ours).is_err());
        let inverted = "a\n<<<<<<< HEAD\nours\n>>>>>>> feature\n";
        assert!(extract_conflict_side(inverted, ConflictSide::Ours).is_err());
    }
}
