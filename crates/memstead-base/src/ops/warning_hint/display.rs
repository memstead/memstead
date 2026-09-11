//! The agent-facing text of each `WarningHint` variant, reachable via `WarningHint::message`.

use super::*;

impl fmt::Display for WarningHint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WarningHint::SchemaPinMismatch {
                mem,
                config_pin,
                mount_pin,
            } => write!(
                f,
                "mem '{mem}': the workspace mount expects schema '{mount_pin}' but the \
                 mem's own config pins '{config_pin}' — the config pin is authoritative and \
                 was used; align the mounts.json entry or the mem config to clear this"
            ),
            WarningHint::MountUnbacked {
                mem,
                reason,
                location,
            } => match reason {
                MountUnbackedReason::MissingRef => write!(
                    f,
                    "mount '{mem}' is unbacked: its branch {location} does not exist \
                     (the mem was never created there, or the branch was deleted); create \
                     it, point the mount at the right branch, or remove the mount"
                ),
                MountUnbackedReason::MissingPath => write!(
                    f,
                    "mount '{mem}' is unbacked: its path {location} does not exist; \
                     restore the folder or remove the mount"
                ),
                MountUnbackedReason::Empty => write!(
                    f,
                    "mount '{mem}' is unbacked: {location} exists but holds no entity \
                     (an empty mem serves nothing); author into it or remove the mount"
                ),
            },
            WarningHint::SectionHeadingDivergence {
                entity_id,
                section_key,
                writing_heading,
                existing_heading,
            } => write!(
                f,
                "entity '{entity_id}': section '{section_key}' is being written under \
                 heading '{writing_heading}' but the file carried '{existing_heading}' for \
                 the same section — the write commits and the regenerated file uses \
                 '{writing_heading}'; the previous heading text is replaced"
            ),
            WarningHint::SchemaHeadingRoundtripViolation {
                mem,
                schema_ref,
                violations,
            } => {
                let list = violations
                    .iter()
                    .map(|v| {
                        format!(
                            "type '{}' section '{}' heading '{}' (derives to '{}')",
                            v.type_name, v.key, v.heading, v.derived_key
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                write!(
                    f,
                    "mem '{mem}': pinned schema '{schema_ref}' declares section heading(s) \
                     that cannot round-trip to their key(s): {list}. The mem keeps loading, \
                     but writes to these sections fork content into a second heading or the \
                     catch-all. Fix the schema's heading/key pairs and reinstall — new \
                     installs of such a schema are refused"
                )
            }
            WarningHint::MissingRequiredSection {
                key,
                heading,
                write_rules,
                ..
            } => {
                write!(
                    f,
                    "required section '{key}' (heading \"{heading}\") is empty — \
                     entity will show as unhealthy"
                )?;
                if !write_rules.is_empty() {
                    write!(f, ". Writing guidance:")?;
                    for rule in write_rules {
                        write!(f, "\n  - {rule}")?;
                    }
                }
                Ok(())
            }
            WarningHint::MissingRequiredField {
                key,
                entity_type,
                description,
                enum_values,
            } => {
                write!(
                    f,
                    "required metadata field '{key}' on type '{entity_type}' was not \
                     supplied — entity landed with a placeholder. {description}"
                )?;
                if !enum_values.is_empty() {
                    write!(f, " Allowed values: [{}].", enum_values.join(", "))?;
                }
                Ok(())
            }
            WarningHint::UndeclaredRelationshipOpen { message, .. } => f.write_str(message),
            WarningHint::DuplicateRelationship { rel_type, from, to } => write!(
                f,
                "relationship {rel_type} from {from} to {to} already exists — no-op"
            ),
            WarningHint::NoSuchRelationship { rel_type, from, to } => write!(
                f,
                "relationship {rel_type} from {from} to {to} does not exist — no-op"
            ),
            WarningHint::UnknownIncludeKey { key, allowed } => write!(
                f,
                "unknown include key '{key}' ignored. Allowed: [{}]",
                allowed.join(", ")
            ),
            WarningHint::LimitClamped { requested, actual } => write!(
                f,
                "limit clamped from {requested} to {actual} (max for memstead_health)"
            ),
            WarningHint::TitleNormalizedToSlugNoop {
                requested_title,
                current_slug,
            } => write!(
                f,
                "requested title '{requested_title}' normalises to the existing slug \
                 '{current_slug}' — no change written to disk"
            ),
            WarningHint::TitleCharsDroppedFromSlug {
                title,
                dropped_chars,
                slug,
            } => write!(
                f,
                "title '{title}' keeps its characters as display text, but the derived \
                 slug '{slug}' drops {dropped_chars:?} — link this entity by its slug"
            ),
            WarningHint::UpdateNoop { id } => write!(
                f,
                "update on {id} produced bytes-identical content — no \
                 disk write, no commit, content_hash unchanged"
            ),
            WarningHint::StubFilterExcludesAll { entity_type } => write!(
                f,
                "stub=true combined with entity_type='{entity_type}' excludes every \
                 stub — stubs carry no entity_type. Drop entity_type to list stubs."
            ),
            WarningHint::UnknownFilterKey {
                key,
                scoped_type,
                declared_on_other_types,
            } => {
                let on_other = !declared_on_other_types.is_empty();
                let scoped_matches_other = matches!(
                    scoped_type.as_deref(),
                    Some(t) if declared_on_other_types.iter().any(|o| o == t)
                );
                if let Some(t) = scoped_type.as_deref() {
                    if on_other && !scoped_matches_other {
                        let word = type_word_for(declared_on_other_types);
                        let items = format_types_clause(declared_on_other_types);
                        return write!(
                            f,
                            "filter '{key}' applied with strict type-exclusion semantics — exists on {word} {items} but the query scoped to type '{t}', where it is not declared. All entities of type '{t}' will be excluded; scope to a declaring type to apply the filter."
                        );
                    }
                    return write!(
                        f,
                        "unknown filter key '{key}' for type '{t}' — filter ignored"
                    );
                }
                if on_other {
                    let word = type_word_for(declared_on_other_types);
                    let items = format_types_clause(declared_on_other_types);
                    return write!(
                        f,
                        "filter '{key}' applied with strict type-exclusion semantics — only entities of {word} {items} will match. Scope explicitly via entity_type=… to suppress this warning."
                    );
                }
                write!(
                    f,
                    "unknown filter key '{key}' — no reachable schema declares it — filter ignored"
                )
            }
            WarningHint::FieldNotFilterable { field } => {
                write!(f, "field '{field}' is not filterable — filter ignored")
            }
            WarningHint::FilterValueMultiMember { key, value } => write!(
                f,
                "filter '{key}={value}' targets a csv-array field but the value contains a comma — \
                 csv fields match a single member, so the full value matches nothing. Filter on one \
                 member at a time (e.g. `{key}={first}`)",
                first = value.split(',').next().map(str::trim).unwrap_or("").trim(),
            ),
            WarningHint::FilterValueNotInEnum {
                key,
                value,
                allowed,
            } => write!(
                f,
                "filter '{key}={value}' is not an allowed value for '{key}' — allowed: [{}]. \
                 The filter applies as written and matches nothing.",
                allowed.join(", ")
            ),
            WarningHint::NeighbourhoodCapped { kept, total } => write!(
                f,
                "related_to neighbourhood has {total} entities; ranked by proximity and bounded to \
                 the nearest {kept}. Narrow with `depth` or filters to see fewer, more specific hits."
            ),
            WarningHint::SearchResultsTruncated { kept, budget } => write!(
                f,
                "results trimmed to the highest-ranked {kept} hits to fit the {budget}-token budget. \
                 `_total` is the full match count — page the rest with `offset`, narrow the query, \
                 or raise `token_budget`."
            ),
            WarningHint::RangeFilterKeyMalformed { key } => write!(
                f,
                "range filter key '{key}' must start with 'min_'/'max_' or end with '_before'/'_after' — filter ignored"
            ),
            WarningHint::UnknownRangeFilterField {
                field,
                key,
                scoped_type,
                declared_on_other_types,
            } => {
                let on_other = !declared_on_other_types.is_empty();
                let scoped_matches_other = matches!(
                    scoped_type.as_deref(),
                    Some(t) if declared_on_other_types.iter().any(|o| o == t)
                );
                if let Some(t) = scoped_type.as_deref() {
                    if on_other && !scoped_matches_other {
                        let word = type_word_for(declared_on_other_types);
                        let items = format_types_clause(declared_on_other_types);
                        return write!(
                            f,
                            "range filter field '{field}' (from key '{key}') applied with strict type-exclusion semantics — exists on {word} {items} but the query scoped to type '{t}', where it is not declared. All entities of type '{t}' will be excluded; scope to a declaring type to apply the filter."
                        );
                    }
                    return write!(
                        f,
                        "unknown range filter field '{field}' (from key '{key}') for type '{t}' — filter ignored"
                    );
                }
                if on_other {
                    let word = type_word_for(declared_on_other_types);
                    let items = format_types_clause(declared_on_other_types);
                    return write!(
                        f,
                        "range filter field '{field}' (from key '{key}') applied with strict type-exclusion semantics — only entities of {word} {items} will match. Scope explicitly via entity_type=… to suppress this warning."
                    );
                }
                write!(
                    f,
                    "unknown range filter field '{field}' (from key '{key}') — no reachable schema declares it — filter ignored"
                )
            }
            WarningHint::FieldNotRangeFilterable { field } => write!(
                f,
                "field '{field}' is not range-filterable — filter ignored"
            ),
            WarningHint::SearchMemIndexUnavailable { mem, reason, error } => {
                match (*reason, error.as_deref()) {
                    ("missing_index", _) => {
                        write!(f, "mem '{mem}' has no search index — query returns no hits")
                    }
                    ("query_failed", Some(e)) => {
                        write!(f, "search index for mem '{mem}' errored: {e}")
                    }
                    _ => write!(f, "search index for mem '{mem}' is unavailable ({reason})"),
                }
            }
            WarningHint::TitleTrimmed { original, trimmed } => write!(
                f,
                "title trimmed of surrounding whitespace: {original:?} → {trimmed:?}"
            ),
            WarningHint::SuspiciousNestedPrefix {
                from,
                resolved_id,
                candidate_target,
                section,
                prefix_mounted,
            } => {
                if *prefix_mounted {
                    write!(
                        f,
                        "wiki-link in {from}#{section} resolves to {resolved_id}: \
                         target missing in mem {}",
                        resolved_id.mem()
                    )?;
                } else {
                    write!(
                        f,
                        "wiki-link in {from}#{section} resolves to {resolved_id}: \
                         prefix '{}' is not a mounted mem (it only matches a mem \
                         name's last segment, the mem-rename drift pattern)",
                        resolved_id.mem()
                    )?;
                }
                if let Some(cand) = candidate_target {
                    write!(f, "; did you mean {cand}?")?;
                }
                Ok(())
            }
            WarningHint::InlineWikiLinkAutoStubbed { from, stubs } => {
                write!(
                    f,
                    "{from} contained {n} inline wiki-link(s) that auto-created stub \
                     entities — review whether the stubs were intended; if not, \
                     remove the inline syntax or wrap the example in a fenced/quoted \
                     form. Auto-stubbed targets:",
                    n = stubs.len(),
                )?;
                for s in stubs {
                    write!(f, "\n  - {s}")?;
                }
                Ok(())
            }
            WarningHint::SelfLinkIgnored { id } => write!(
                f,
                "{id} contains a body wiki-link to its own id — the self-referential edge \
                 was dropped (a self-link carries no navigational value). The entity was \
                 created/updated normally; remove the `[[{slug}]]` link if it was a mistake",
                slug = id.name(),
            ),
            WarningHint::CrossSchemaLinkUndeclared {
                from,
                target,
                source_schema,
                target_schema,
            } => write!(
                f,
                "{from} body-links {target}, but schema {source_schema} declares no \
                 cross_mem_relationships entry for schema '{target_schema}' (and no \
                 wildcard), so NO edge was emitted — the link is prose only. The write \
                 succeeded. To make such citations real edges, declare '{target_schema}' \
                 (or a `to_schema: \"*\"` wildcard) under the source schema's \
                 cross_mem_relationships",
            ),
            WarningHint::CrossMemTargetMemUncreated {
                from_mem,
                to_mem,
                target_id,
            } => write!(
                f,
                "cross-mem relate from '{from_mem}' to '{target_id}': \
                 target mem '{to_mem}' is not mounted in the workspace — \
                 the auto-stub has no schema resolution until the mem is created. \
                 If '{to_mem}' is a typo, fix the relate; if forward-reference \
                 is intended, create the mem to promote the stub."
            ),
            WarningHint::NoteMissing { tool } => write!(
                f,
                "{tool} called without a `note` while \
                 `[mutations].require_notes = true`; the write carries no \
                 provenance line"
            ),
            WarningHint::IgnoredReadonlyField { field, supplied } => write!(
                f,
                "'{field}' is auto-managed by the engine — the supplied \
                 value '{supplied}' was discarded and the engine value \
                 stamped instead"
            ),
            WarningHint::OuterRepoNotIgnoringMemRepo {
                outer_repo_root,
                workspace_root,
            } => write!(
                f,
                "workspace at '{workspace_root}' is embedded inside the git \
                 repository at '{outer_repo_root}' but the outer .gitignore \
                 does not list 'mem-repo/'. Add 'mem-repo/' (or the \
                 workspace-relative equivalent) to the outer repo's \
                 .gitignore to keep mem-repo-git out of the outer index."
            ),
            WarningHint::SignalThresholdCrossed {
                entity_id,
                signal,
                value,
                old_level,
                new_level,
            } => write!(
                f,
                "signal '{signal}' on {entity_id} crossed a declared threshold: \
                 {old_level} → {new_level} (value {value})"
            ),
            WarningHint::MissingRequiredOutgoing {
                entity_type,
                entity_id,
                missing,
            } => {
                write!(
                    f,
                    "{entity_id} ({entity_type}) is missing required outgoing edges — \
                     schema declares {n} `required_outgoing` block(s) still unsatisfied:",
                    n = missing.len(),
                )?;
                for block in missing {
                    write!(
                        f,
                        "\n  - [{}] cardinality={}",
                        block.relationships.join(", "),
                        block.cardinality,
                    )?;
                }
                Ok(())
            }
            WarningHint::ConstraintUnsatisfied {
                entity_type,
                entity_id,
                violations,
            } => {
                write!(
                    f,
                    "{entity_id} ({entity_type}) violates {n} declared constraint(s):",
                    n = violations.len(),
                )?;
                for v in violations {
                    write!(f, "\n  - {}", v.describe())?;
                }
                Ok(())
            }
            WarningHint::DuplicateSectionHeading {
                entity_id,
                section_key,
                heading,
                occurrences,
            } => write!(
                f,
                "{entity_id} declared `## {heading}` {occurrences} times — \
                 section '{section_key}' kept the first occurrence's body \
                 and dropped the rest. The next read-modify-write will \
                 collapse the markdown to one heading."
            ),
            WarningHint::UnanchoredMention {
                binding,
                entity,
                artifact,
                section,
                ..
            } => write!(
                f,
                "`{entity}` names `{artifact}` in section `{section}` and carries no anchor on \
                 it, so no verify under `{binding}` watches that claim. Add the anchor \
                 (`memstead_update` with `anchors`), or exclude the artifact with a rationale \
                 (`memstead projection exclude {binding}`).",
            ),
            WarningHint::OutOfBandEditsUndetected { mem } => write!(
                f,
                "mem '{mem}' is folder-backed, so its drift cursor is its own change ledger and \
                 only the engine writes it: an edit made to its files by anything else is not \
                 detected, and reads keep serving the pre-edit content. Reconcile on demand with \
                 `memstead health --include ledger`.",
            ),
            WarningHint::ShortIdResolved { given, resolved } => write!(
                f,
                "entity id `{given}` carried no mem prefix; exactly one mounted mem holds that \
                 slug, so this call acted on `{resolved}`. Write the full id to keep the target \
                 fixed if another mem gains the slug.",
            ),
            WarningHint::ConfigWriteIntervened { mem, fields } => write!(
                f,
                "mem '{mem}' config had changed since this engine last read it: another writer \
                 set {}. This write was applied on top of theirs, so nothing of theirs was \
                 lost.",
                fields.join(", "),
            ),
            WarningHint::MemReloaded {
                mem,
                old_head,
                new_head,
                entities_loaded,
            } if old_head.is_empty() => write!(
                f,
                "mem '{mem}' was reloaded — its branch appeared at {new_head} \
                 (born, fetched or pushed into place since the engine last \
                 read the mem, which had loaded it empty). {entities_loaded} \
                 entities loaded; response carries fresh content. Re-derive \
                 any conclusions that depended on the prior content of this \
                 mem before continuing."
            ),
            WarningHint::MemReloaded {
                mem,
                old_head,
                new_head,
                entities_loaded,
            } => write!(
                f,
                "mem '{mem}' was reloaded — on-disk HEAD advanced from \
                 {old_head} to {new_head} (a sibling writer or out-of-band \
                 commit landed since the engine last read the mem). \
                 {entities_loaded} entities reloaded; response carries \
                 fresh content. Re-derive any conclusions that depended on \
                 the prior content of this mem before continuing. Call \
                 `memstead_changes_since since={old_head}` for the per-entity \
                 diff."
            ),
            WarningHint::MemRosterChanged {
                added,
                removed,
                quarantined,
                failures,
            } => write!(
                f,
                "the mount roster changed and the engine reconciled it before serving this \
                 call — added: [{}], removed: [{}], quarantined: [{}]{}. Cached hashes for a \
                 removed mem are void; an operation naming it refuses MEM_UNMOUNTED.",
                added.join(", "),
                removed.join(", "),
                quarantined.join(", "),
                if failures.is_empty() {
                    String::new()
                } else {
                    format!("; not applied: {}", failures.join("; "))
                }
            ),
            WarningHint::AutoStubCreated { stub_id, pending } => {
                if *pending {
                    write!(
                        f,
                        "target '{stub_id}' does not exist — a stub would be \
                         auto-created by the real call. Promote it via \
                         memstead_create first, or let the real call create \
                         the stub (adoption preserves the incoming edge)."
                    )
                } else {
                    write!(
                        f,
                        "target '{stub_id}' did not exist — stub auto-created. \
                         Promote it via memstead_create when authoring the real \
                         entity (stub adoption preserves the incoming edge)."
                    )
                }
            }
            WarningHint::DerivationBaselineRefreshed { from, rel_type, to } => write!(
                f,
                "derivation baseline refreshed: '{from}' -[{rel_type}]-> '{to}' — the edge \
                 already existed; its baseline now records the target's current content \
                 hash (reviewed, still holds). Nothing else changed."
            ),
            WarningHint::ParsedRelationInvalid {
                entity_id,
                rel_type,
                target,
                reason,
                origin,
                recovery: _,
            } => {
                let recovery_msg = if origin == "readonly" {
                    "Source mem is mounted read-only; the engine cannot \
                     rewrite the markdown. Either remove the mount \
                     (`memstead uninstall <mem>`) or accept the dropped \
                     relation."
                } else {
                    "Fix the source markdown (via memstead_update / \
                     memstead_relate — `details.recovery` carries the abstract \
                     action) or adjust the schema."
                };
                write!(
                    f,
                    "parsed relation {rel_type} from {entity_id} to \
                     {target} was dropped — reason: {reason}, origin: \
                     {origin}. The entity loaded but the relation does \
                     not appear in the in-memory graph. {recovery_msg}"
                )
            }
            WarningHint::ResidualStubForReadOnlyReferrers { id, referrers } => write!(
                f,
                "{id} was deleted from disk but {n} read-only-mount \
                 referrer(s) still target it; the in-memory entity is \
                 demoted to a stub at the same id so the surviving \
                 incoming edges keep a valid target. Surviving referrers: \
                 [{}]. Either accept the stub or remove the source mount \
                 (`memstead uninstall <mem>`) — read-only content cannot \
                 be rewritten by the engine.",
                referrers
                    .iter()
                    .map(|r| r.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
                n = referrers.len(),
            ),
            WarningHint::AmbiguousDescriptionDelimiter {
                from,
                rel_type,
                target,
                trailing,
            } => write!(
                f,
                "{from} → {target} ({rel_type}): trailing content {trailing:?} \
                 after `]]` did not match the canonical em-dash delimiter ` — ` \
                 (U+2014); content dropped, the relation parses with no \
                 description. Restore with `memstead_relate {from} {rel_type} \
                 {target} --description \"<text>\"` (or hand-edit using \
                 ` — `) if the text was intentional."
            ),
            WarningHint::ParseMissingRequiredDescription {
                from,
                rel_type,
                target,
            } => write!(
                f,
                "{from} → {target} ({rel_type}): rel-type declares \
                 `per_edge_description: required` but the row has no \
                 trailing em-dash description. Add one via `memstead_relate \
                 {from} {rel_type} {target} --description \"<text>\"` (or \
                 hand-edit the markdown using ` — `)."
            ),
            WarningHint::ParseDescriptionNotPermitted {
                from,
                rel_type,
                target,
            } => write!(
                f,
                "{from} → {target} ({rel_type}): rel-type declares \
                 `per_edge_description: forbidden` but the markdown row \
                 carries a trailing description. The description is \
                 dropped from the in-memory graph and the next render \
                 normalises the row to the simple form. Drop the trailing \
                 text from the source markdown if it should not round-trip."
            ),
            WarningHint::MemReattachedAfterUnregister {
                mem,
                unregistered_at,
            } => write!(
                f,
                "mem '{mem}' was reattached to pre-existing storage \
                 that carried an `unregistered_at: {unregistered_at}` \
                 tombstone marker. The entities from the prior session \
                 were adopted; the tombstone has been cleared. If this \
                 reattach was unexpected, run `memstead mem delete \
                 {mem}` to destroy the storage and start fresh."
            ),
            WarningHint::ReadMemsMigratedToMounts {
                mems,
                from_host_mems,
            } => write!(
                f,
                "legacy `readMems` registrations were migrated to \
                 workspace-level read-only mounts: [{}] (previously \
                 attached to writable mem(s) [{}]). The legacy key was \
                 removed from the config; this migration runs once. \
                 Remove a migrated read-mem with `memstead uninstall \
                 <name>`.",
                mems.join(", "),
                from_host_mems.join(", "),
            ),
            WarningHint::EngineVersionSkew {
                mem,
                stamped_engine,
                running_engine,
                stamped_schema,
                direction,
            } => write!(
                f,
                "mem '{mem}': the last mutation was performed by engine \
                 v{stamped_engine} (against schema {stamped_schema}); \
                 this binary is engine v{running_engine} ({}). Informative \
                 only — the next mutation re-stamps. If behaviour \
                 differs from the last session, the binary changed \
                 between them.",
                match direction {
                    crate::build_info::SkewDirection::StampedNewer =>
                        "the mem was last written by a NEWER binary than this one",
                    crate::build_info::SkewDirection::StampedOlder =>
                        "the mem was last written by an OLDER binary than this one",
                },
            ),
            WarningHint::SchemaGenerationsBehind {
                mem,
                pinned,
                newest,
            } => write!(
                f,
                "mem '{mem}' pins built-in schema {pinned}, but the \
                 catalogue registers newer generations up to {newest}. \
                 The pin keeps working (retained versions stay sealed); \
                 migrate via `memstead mem set-schema` when ready.",
            ),
            WarningHint::FolderMemProvenance { mem } => write!(
                f,
                "mem '{mem}' was created on folder storage with no \
                 version control. Provenance here is the changelog \
                 ledger (`.memstead/changes.jsonl`), which records \
                 every mutation with its note — but there are no \
                 commits: the `write_id` mutations return is a \
                 synthetic token rather than a commit, and it is not a \
                 change cursor — poll this mem with the `ts` of the last \
                 ledger entry you read. The content is not durable until \
                 the surrounding repository commits it."
            ),
            WarningHint::SchemaAuthoringSourceMissing {
                schema_ref,
                stamped_path,
                mems,
            } => write!(
                f,
                "schema '{schema_ref}' (pinned by {}) was installed from \
                 '{stamped_path}', and that authoring package is no longer \
                 there. The engine keeps running on its sealed copy — \
                 nothing is broken — but the source the seal came from is \
                 gone: restore or move back the package, or re-install \
                 from its new location to re-stamp.",
                mems.join(", ")
            ),
            WarningHint::SchemaAuthoringSourceDiverged {
                schema_ref,
                stamped_path,
                mems,
                detail,
            } => write!(
                f,
                "schema '{schema_ref}' (pinned by {}) no longer matches \
                 its authoring package at '{stamped_path}': {detail}. The \
                 engine keeps running on its sealed copy; if the authoring \
                 change is intended, bump the version and `memstead schema \
                 install` it.",
                mems.join(", ")
            ),
            WarningHint::SchemaUnstampedSourceRot {
                schema_ref,
                mems,
                detail,
            } => write!(
                f,
                "schema '{schema_ref}' (pinned by {}) has no install-provenance \
                 stamp, and its sealed package no longer passes current-language \
                 authoring validation: {detail}. The mem keeps running on the \
                 tolerantly-loaded seal — nothing is broken — but the package is \
                 no longer installable as authored. Re-author it under the \
                 current language and `memstead schema install` it (which also \
                 stamps it, so future drift is checked).",
                mems.join(", ")
            ),
            WarningHint::MemFilesNotDeleted {
                mem,
                reason,
                path,
                error,
            } => match (reason.as_str(), path.as_deref(), error.as_deref()) {
                ("rmdir_failed", Some(p), Some(e)) => write!(
                    f,
                    "mem '{mem}' was unregistered but rmdir of \
                         {p:?} failed: {e}. Files remain on disk; agent \
                         may follow up with manual cleanup."
                ),
                ("rmdir_failed", Some(p), None) => write!(
                    f,
                    "mem '{mem}' was unregistered but rmdir of \
                         {p:?} failed. Files remain on disk."
                ),
                ("backend_prune_failed", _, Some(e)) => write!(
                    f,
                    "mem '{mem}' was unregistered but backend \
                         artifact cleanup failed: {e}. The mem-repo \
                         branch and/or `__MEMSTEAD:mems/.../config.json` \
                         entry may survive; rerun delete with the same \
                         arguments or have an operator inspect."
                ),
                ("backend_prune_failed", _, None) => write!(
                    f,
                    "mem '{mem}' was unregistered but backend \
                         artifact cleanup failed. The mem-repo branch \
                         and/or `__MEMSTEAD` config entry may survive."
                ),
                _ => write!(
                    f,
                    "mem '{mem}' was unregistered but \
                         `delete_files: true` did not run to completion \
                         (reason: {reason})."
                ),
            },
        }
    }
}
