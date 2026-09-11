//! Schema install and validation: staging a sealed package, validating its shape and its exemplars, and installing it into the workspace.

use super::*;

impl Engine {
    /// Install a schema package onto the workspace's git-branch backend —
    /// the unified `__MEMSTEAD:schemas/<name>@<version>/` ref. `files`
    /// are `(relative-path, bytes)` pairs (`schema.yaml`,
    /// `types/<t>.yaml`, optional `mem-template.json`). Returns the
    /// resulting commit sha; idempotent at the storage layer (an
    /// identical re-install produces no new commit).
    ///
    /// Folder workspaces install schemas by writing under
    /// `<workspace>/.memstead/schemas/` directly; this is the git-branch
    /// path, where the engine owns the mem-repo and the write must
    /// route through it. Errors when no git-branch ops are wired
    /// or no git-branch mount exists to resolve the shared
    /// mem-repo gitdir from. The caller reloads (or restarts) to pick
    /// the new schema into the resolution catalogue.
    pub fn install_schema(
        &self,
        name: &str,
        version: &str,
        files: &[(String, Vec<u8>)],
    ) -> Result<String, EngineError> {
        // Validation gate: the engine refuses to seal a package that the
        // loader would reject or whose section headings cannot round-trip
        // to their keys. Install time is the last moment the author can
        // act — sealed schemas keep loading even when a later rule would
        // refuse them, so nothing invalid may pass this point.
        Self::validate_schema_package(name, version, files)?;
        // Resolve the shared mem-repo gitdir: prefer a live git-branch
        // mount's gitdir (authoritative — that is where the engine reads
        // schemas from), falling back to the workspace's `mem-repo/.git`
        // so a schema can be installed into an empty mem-repo *before*
        // any mem pins it.
        let gitdir = self
            .mounts
            .iter()
            .find_map(|m| match &m.mount.storage {
                crate::workspace::MountStorage::GitBranch { gitdir, .. } => Some(gitdir.clone()),
                _ => None,
            })
            .or_else(|| {
                self.workspace_root()
                    .map(|r| r.join("mem-repo").join(".git"))
            })
            .ok_or_else(|| {
                EngineError::Mem(
                    "schema install requires a mem-repo workspace (no git-branch mount and \
                     no workspace root to resolve the mem-repo gitdir)"
                        .to_string(),
                )
            })?;
        let ops = self.git_branch_ops.as_ref().ok_or_else(|| {
            EngineError::Mem("git-branch ops are not wired on this engine".to_string())
        })?;
        // Seal the package AS-GIVEN: the format marker is a generation
        // claim only the resolver can make (an authored directory source
        // arrives already stamped; a legacy builtin or sealed source
        // arrives unmarked, and absence IS its legacy claim). Stamping
        // here mis-labelled legacy content as current and flipped every
        // bare field's meaning in the sealed copy — the manufacturing
        // defect an earlier plan removed.
        (ops.write_schema)(&gitdir, name, version, files).map_err(EngineError::Backend)
    }

    /// Make a **sealed third-party** schema package resolvable in this
    /// workspace — the install-time half of "a published mem installs
    /// on the strength of the schema it carries".
    ///
    /// The package (package-relative files: `schema.yaml`,
    /// `types/<t>.yaml`, `schema-format.json`) is written into the
    /// workspace's local schema storage, the same storage
    /// [`Self::install_schema`] writes to and the pin resolver reads —
    /// so the mount that follows resolves without a fourth mechanism
    /// and a second mem pinning the same schema finds it already
    /// there. The loaded schema also lands in this engine's live
    /// catalogue, so the caller mounts in the same process without a
    /// reload.
    ///
    /// Two things it deliberately does NOT do. It does not run the
    /// authoring gate ([`Self::validate_schema_package`]): these are a
    /// third party's sealed bytes, the installing user cannot fix
    /// them, and the archive validator has already admitted them under
    /// the sealed reading — applying the authoring gate here would
    /// make an archive that is valid to publish invalid to install.
    /// And it does not append the format marker: presence of the
    /// marker IS the package's metadata-polarity generation, so
    /// injecting one would rewrite the meaning of bytes the publisher
    /// sealed.
    ///
    /// Idempotent: a pin the engine can already resolve returns
    /// [`SchemaStaging::AlreadyResolvable`] with nothing written.
    ///
    /// Shape-agnostic: on a workspace with no sealed-package storage (the
    /// folder shape) the package is still read and checked against the
    /// pin, so a broken embedded schema refuses before any mount side
    /// effect, but nothing is written — the archive-backed mount that
    /// follows resolves the vocabulary from the archive itself
    /// ([`SchemaStaging::CarriedByArchive`]). What that shape forgoes is
    /// the staged copy's extras: `memstead schema <pin>` rendering the
    /// installed package, and a second, writable mem pinning it.
    pub fn stage_sealed_schema(
        &mut self,
        mem: &str,
        pin: &memstead_schema::SchemaRef,
        files: &[(String, Vec<u8>)],
    ) -> Result<SchemaStaging, EngineError> {
        let catalogue: Vec<std::sync::Arc<memstead_schema::Schema>> = self
            .workspace_schemas
            .iter()
            .chain(self.builtin_schemas.iter())
            .cloned()
            .collect();
        if crate::engine::SchemaResolver::new(&catalogue)
            .resolve(pin)
            .is_ok()
        {
            return Ok(SchemaStaging::AlreadyResolvable);
        }
        if files.is_empty() {
            // Nothing embedded to stage. The caller's mount attempt
            // reports the unresolved pin with its own source trail —
            // that IS the actionable error here.
            return Ok(SchemaStaging::NoEmbeddedSchema);
        }

        let unloadable = |reason: String| EngineError::EmbeddedSchemaInvalid {
            mem: mem.to_string(),
            pin: pin.as_display(),
            reason,
        };
        // Read it exactly as the local schema source will read it back
        // — one sealed reader, so admission and re-read cannot diverge.
        let schema =
            memstead_schema::load_sealed_package(files).map_err(|e| unloadable(e.to_string()))?;
        let (name, version) = schema.id();
        if name != pin.name || version != pin.version {
            return Err(unloadable(format!(
                "the package declares '{name}@{version}' but the mem pins {}",
                pin.as_display()
            )));
        }

        if self.sealed_schema_gitdir().is_none() {
            return Ok(SchemaStaging::CarriedByArchive);
        }
        self.write_to_local_schema_source(&name, &version.to_string(), files)?;
        self.workspace_schemas.push(std::sync::Arc::new(schema));
        Ok(SchemaStaging::Staged)
    }

    /// The gitdir that holds sealed third-party packages (the
    /// `__MEMSTEAD:schemas/` ref): the first git-branch mount's, else the
    /// workspace's `mem-repo/.git`. `None` on a folder-shaped workspace,
    /// whose `.memstead/schemas/` is the authoring tier and never holds
    /// sealed bytes.
    fn sealed_schema_gitdir(&self) -> Option<PathBuf> {
        self.mounts
            .iter()
            .find_map(|m| match &m.mount.storage {
                crate::workspace::MountStorage::GitBranch { gitdir, .. } => Some(gitdir.clone()),
                _ => None,
            })
            .or_else(|| {
                self.workspace_root()
                    .map(|r| r.join("mem-repo").join(".git"))
            })
            .filter(|g| g.is_dir())
    }

    /// Write a schema package into the workspace's local schema
    /// storage — the git-branch `__MEMSTEAD:schemas/` ref when the
    /// workspace is mem-repo-shaped. Shares
    /// [`Self::install_schema`]'s gitdir resolution so authored and
    /// staged packages land in one place.
    fn write_to_local_schema_source(
        &self,
        name: &str,
        version: &str,
        files: &[(String, Vec<u8>)],
    ) -> Result<(), EngineError> {
        let gitdir = self.sealed_schema_gitdir().ok_or_else(|| {
            EngineError::Mem(
                "staging a schema requires a mem-repo workspace — the folder workspace's \
                     `.memstead/schemas/` is the authoring tier and never holds sealed \
                     third-party packages"
                    .to_string(),
            )
        })?;
        let ops = self.git_branch_ops.as_ref().ok_or_else(|| {
            EngineError::Mem("git-branch ops are not wired on this engine".to_string())
        })?;
        (ops.write_schema)(&gitdir, name, version, files).map_err(EngineError::Backend)?;
        Ok(())
    }

    /// Validate a schema package's files before they are sealed. Runs
    /// the full loader (structural + semantic) plus the section-heading
    /// round-trip gate, and checks the manifest's declared identity
    /// matches the `(name, version)` the package is being installed
    /// under — a mismatch would seal the schema under a ref its own
    /// manifest contradicts.
    ///
    /// `pub` so the below-boot install path (memstead-git-branch's
    /// repair surface) runs the SAME gate as this booted path — the
    /// two must never fork into separate validation regimes.
    pub fn validate_schema_package(
        name: &str,
        version: &str,
        files: &[(String, Vec<u8>)],
    ) -> Result<(), EngineError> {
        let invalid = |message: String| EngineError::SchemaPackageInvalid {
            name: name.to_string(),
            version: version.to_string(),
            message,
        };
        let manifest_yaml = files
            .iter()
            .find(|(rel, _)| rel == "schema.yaml")
            .map(|(_, bytes)| String::from_utf8_lossy(bytes).into_owned())
            .ok_or_else(|| invalid("package has no schema.yaml".to_string()))?;
        let types: Vec<(String, String)> = files
            .iter()
            .filter_map(|(rel, bytes)| {
                rel.strip_prefix("types/")
                    .and_then(|f| f.strip_suffix(".yaml"))
                    .map(|stem| {
                        (
                            stem.to_string(),
                            String::from_utf8_lossy(bytes).into_owned(),
                        )
                    })
            })
            .collect();
        // Install validates the CURRENT language (the author acts
        // now) — the retired `optional:` key refuses here, and absent
        // required keys mean optional.
        let schema = memstead_schema::load_schema_from_memory_with_format(
            &manifest_yaml,
            &types,
            memstead_schema::loader::MetadataPolarityFormat::RequiredOptIn,
        )
        .map_err(|e| invalid(e.to_string()))?;
        memstead_schema::check_section_heading_roundtrip(&schema)
            .map_err(|e| invalid(e.to_string()))?;
        memstead_schema::check_reserved_metadata_keys(&schema)
            .map_err(|e| invalid(e.to_string()))?;
        memstead_schema::check_section_formats(&schema).map_err(|e| invalid(e.to_string()))?;
        let (declared_name, declared_version) =
            (schema.manifest.name.as_str(), schema.version.to_string());
        if declared_name != name || declared_version != version {
            return Err(invalid(format!(
                "manifest declares '{declared_name}@{declared_version}' but the package is \
                 being installed as '{name}@{version}'"
            )));
        }
        Self::validate_schema_exemplars(&std::sync::Arc::new(schema)).map_err(invalid)?;
        Ok(())
    }

    /// Validate every type exemplar a schema carries by running it
    /// through the REAL create validation stage:
    /// an in-memory engine is booted with the candidate schema pinned
    /// on a virtual mem, and each exemplar is submitted as a
    /// `dry_run` create — the same gates a real write runs (sections,
    /// metadata + enums, rel-type vocabulary, edge shape, description
    /// posture), commit-free by construction. Placeholder relation
    /// targets are bare slugs scoped to the virtual mem, so target
    /// existence is never checked (an absent target is the legal
    /// would-be-stub path).
    ///
    /// Returns the defect as a message naming the type — the caller
    /// wraps it in its own typed envelope (`SchemaPackageInvalid` on
    /// the install path). There is deliberately no warn-and-carry
    /// mode: a non-conformant exemplar refuses, because the whole
    /// value of an exemplar is the impossibility of drift.
    ///
    /// `pub` so the built-in suite gates every shipped exemplar
    /// through the SAME validator (a broken built-in exemplar fails
    /// CI), and the below-boot install path shares the gate via
    /// [`Self::validate_schema_package`].
    pub fn validate_schema_exemplars(
        schema: &std::sync::Arc<memstead_schema::Schema>,
    ) -> Result<(), String> {
        let with_exemplars: Vec<&str> = schema
            .manifest
            .types
            .iter()
            .filter(|t| {
                schema
                    .types
                    .get(t.as_str())
                    .is_some_and(|td| td.exemplar.is_some())
            })
            .map(String::as_str)
            .collect();
        if with_exemplars.is_empty() {
            return Ok(());
        }

        let (name, version) = schema.id();
        let mem = "exemplar";
        let mount = crate::workspace::Mount {
            mem: mem.to_string(),
            schema: Some(memstead_schema::SchemaRef::new(name, version)),
            storage: crate::workspace::MountStorage::InMemory,
            capability: crate::workspace::MountCapability::Write,
            lifecycle: crate::workspace::MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let backend = Box::new(crate::storage::InMemoryBackend::new()) as Box<dyn MemBackend>;
        let mut engine = Engine::from_mounts_with_schemas_dir_and_extra(
            vec![(mount, backend)],
            None,
            vec![schema.clone()],
        )
        .map_err(|e| format!("exemplar validation could not boot: {e}"))?;

        for type_name in with_exemplars {
            let td = schema
                .types
                .get(type_name)
                .expect("filtered on presence above");
            let ex = td.exemplar.as_ref().expect("filtered on presence above");
            let mut relations = Vec::with_capacity(ex.relations.len());
            for r in &ex.relations {
                let target = r.target_slug();
                if target.contains("--") || target.trim().is_empty() {
                    return Err(format!(
                        "type '{type_name}' exemplar relation target '{target}' must be a bare \
                         placeholder slug (no `--`, non-empty) — exemplars live outside \
                         any mem",
                    ));
                }
                relations.push(crate::ops::RelateArg {
                    target: crate::entity::EntityId::new(mem, target),
                    rel_type: r.rel_type_name().to_string(),
                    description: r.description.clone(),
                });
            }
            let args = crate::engine::CreateEntityArgs {
                anchors: Vec::new(),
                mem: mem.to_string(),
                title: ex.title.clone(),
                entity_type: type_name.to_string(),
                sections: ex.sections.clone(),
                metadata: ex.metadata.clone(),
                relations,
                dry_run: true,
            };
            if let Err(e) = engine.create_entity(args, crate::vcs::Actor::Cli, None, None) {
                return Err(format!(
                    "type '{type_name}' exemplar does not conform: [{}] {e}",
                    e.code()
                ));
            }
        }
        Ok(())
    }
}
