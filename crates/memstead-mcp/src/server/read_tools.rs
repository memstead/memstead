//! The read tools: every verb that takes no note and no role, plus the unified variants that sit with their tool.

use super::*;

#[tool_router(router = read_tool_router, vis = "pub(crate)")]
impl McpServer {
    // ----------------------------------------------------------------------
    // Read-only graph tools
    // ----------------------------------------------------------------------

    #[tool(
        name = "memstead_entity",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(super) fn memstead_entity(
        &self,
        Parameters(p): Parameters<EntityParams>,
    ) -> CallToolResult {
        if let Some(err) = validate_entity_id(&p.id) {
            return err;
        }
        let id = EntityId::canonical(&p.id);

        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        // Cross-mem forms take the FULL lazy-mount load before answering:
        // `include_relations` renders INCOMING edges, which can originate
        // in any mem, and `include_context` computes the workspace-global
        // community clustering — either one over a partial store is a
        // silently incomplete answer, the failure class the lazy-mount
        // observability contract forbids. Declared signals and labelling
        // are cross-mem forms too (an `in`-direction signal reads
        // incoming edges, a neighbour pair reads the counterpart record,
        // and the labelling view counts the cross-mem edges it excludes),
        // so a mem whose schema declares either also takes the full
        // load. The plain read against a schema declaring neither stays
        // scoped to the target mem (the reload below), preserving the
        // lazy win.
        let declares_cross_mem_serving = engine.schema_for(id.mem()).is_some_and(|s| {
            s.manifest.relationships.labelling.is_some()
                || s.types.values().any(|td| !td.signals.is_empty())
        });
        if p.include_relations.unwrap_or(false)
            || p.include_context.unwrap_or(false)
            || declares_cross_mem_serving
        {
            engine.ensure_mems_loaded(None);
        }
        let drift_warnings = engine.reload_if_stale(Some(id.mem()));
        // Drain the stashed structured notices: attached to the
        // response's `structured_content` below (and the markdown
        // `MEM_RELOADED` warning still rides the text channel).
        let (engine, mem_changed_notices) = engine.finish();
        let entity = match engine.get_entity(&id) {
            Some(e) => e.clone(),
            // Drift must survive the error path too: a sibling that
            // deleted X advanced this engine's head during the reload
            // above, so a bare `not_found_error` would consume the
            // drained notice and silently swallow the whole reload
            // window. Attach it on the success channel split.
            None => {
                // A quarantined mem's entities are deliberately not in
                // the store; refusing ENTITY_NOT_FOUND there would be
                // dishonest ("honest absence beats partial truth" —
                // the read names the quarantine, not a phantom miss).
                // Likewise a mem that left the roster: MEM_UNMOUNTED.
                if engine.quarantine_reason(id.mem()).is_some()
                    || engine.recently_unmounted(id.mem())
                {
                    let err = engine.unknown_mem_error(id.mem());
                    return attach_drift_to_error(
                        engine_err_unified(err, &engine),
                        &drift_warnings,
                        mem_changed_notices,
                    );
                }
                return attach_drift_to_error(
                    not_found_error(engine.store(), &id),
                    &drift_warnings,
                    mem_changed_notices,
                );
            }
        };
        let schema_anchor = mem_schema_ref_unified(&engine, id.mem());

        let sections_filter = p.sections.as_deref();
        // Declared aggregate signals — computed once, served on both
        // channels (frontmatter headline + `## Signals` append on the
        // text channel, `_signals` on the envelope). `None` for types
        // declaring none keeps both channels byte-identical.
        let computed_signals = engine.computed_signals(&entity);
        let computed_labelling = engine.computed_labelling(&entity);
        let mut md = render::render_entity_markdown_with_signals(
            &entity,
            sections_filter,
            computed_signals.as_deref(),
            computed_labelling.as_ref(),
        );

        if p.include_relations.unwrap_or(false) {
            let outgoing = engine.store().outgoing(&id).to_vec();
            let incoming = engine.store().incoming(&id).to_vec();
            md.push_str(&render::render_relations_markdown(
                id.as_ref(),
                &outgoing,
                &incoming,
            ));
        }

        if p.include_context.unwrap_or(false)
            && let Some(ctx) = engine.context(&id)
        {
            let cluster_id = ctx.community.clone().unwrap_or_else(|| "unknown".into());
            md.push_str(&render::render_community_context_section(&ctx, &cluster_id));
        }

        if let Some(ref s) = schema_anchor {
            inject_md_mem_schema(&mut md, s);
        }

        let mut extra_fm: Vec<(&str, &str)> = vec![("_hash", &entity.content_hash)];
        if let Some(ref s) = schema_anchor {
            extra_fm.push(("_mem_schema", s.as_str()));
        }

        // Structured envelope
        // rides alongside the chunked markdown text channel. Built
        // off the *unchunked* entity so consumers can branch on full
        // field shapes regardless of which chunk the text channel
        // ships; sections-filtering still applies so a narrowed read
        // narrows both channels. `_tokens` reflects the rendered body
        // (post-filter, post-opt-in); `_tokens_unfiltered_body`
        // surfaces when the filter dropped any sections, matching
        // the markdown renderer's signal that there is "more entity"
        // to read. Renamed from `_tokens_full` because the
        // previous name implied a monotonic relationship the opt-in
        // path can invert.
        let rendered_body_tokens = estimate_tokens(&md);
        let full_tokens = if sections_filter.is_some() {
            let full_body = render::render_entity_markdown(&entity, None);
            Some(estimate_tokens(&full_body))
        } else {
            None
        };
        // Origin now rides inside the shared envelope builder (the
        // structural fix for cold-start 0-8-0 F9/F13 — every surface
        // that composes an entity read carries it, not just this one).
        // Incoming edges join the envelope's `relationships` array when
        // the caller opted into relations, mirroring the text channel's
        // `## Relations` section (F15).
        let incoming_for_envelope = if p.include_relations.unwrap_or(false) {
            Some(engine.store().incoming(&id).to_vec())
        } else {
            None
        };
        let mut structured = render::build_entity_envelope(
            &entity,
            rendered_body_tokens,
            full_tokens,
            sections_filter,
            schema_anchor.as_deref(),
            engine.mem_origin_class(id.mem()),
            engine.store().outgoing(&id),
            incoming_for_envelope.as_deref(),
            computed_signals.as_deref(),
            computed_labelling.as_ref(),
        );
        if let Some(obj) = structured.as_object_mut() {
            // Authoring provenance carried in the installed archive. Emitted
            // only when the mem ships a provenance payload; `history`
            // makes the "full commit history not shipped" decision
            // observable, and `rationale` is `null` when this entity was
            // authored without a note — absence reported as absence, never
            // a fabricated value. A mem with no payload omits the field.
            if let Some(prov) = engine.archive_provenance_for(id.mem()) {
                let mut block = serde_json::Map::new();
                block.insert("history".into(), serde_json::json!(prov.history));
                let rec = prov.entity(id.path());
                block.insert(
                    "rationale".into(),
                    rec.and_then(|r| r.rationale.as_ref())
                        .map(|s| serde_json::json!(s))
                        .unwrap_or(serde_json::Value::Null),
                );
                if let Some(r) = rec {
                    if let Some(kind) = &r.kind {
                        block.insert("kind".into(), serde_json::json!(kind));
                    }
                    if let Some(ts) = &r.timestamp {
                        block.insert("timestamp".into(), serde_json::json!(ts));
                    }
                    if let Some(actor) = &r.actor {
                        block.insert("actor".into(), serde_json::json!(actor));
                    }
                }
                obj.insert("provenance".into(), serde_json::Value::Object(block));
            }
            // Mutation provenance, opt-in:
            // created-by / last-modified-by with actor, client,
            // declared role, and timestamp — derived from the
            // append-only mutation record, which no verb can edit.
            // Distinct key from the archive-provenance block above
            // (that one describes an installed archive's authoring
            // payload). Default responses are byte-unchanged; on a
            // mount whose seam records no history (archives) the
            // block states unavailability instead of fabricating.
            if p.include_provenance.unwrap_or(false) {
                let block = match engine.entity_provenance(id.mem(), id.as_ref()) {
                    Ok(prov) => serde_json::to_value(&prov).unwrap_or(serde_json::Value::Null),
                    Err(e) => serde_json::json!({
                        "unavailable": e.to_string(),
                    }),
                };
                obj.insert("mutation_provenance".into(), block);
            }
            // Provenance anchors. Additive, emitted only when the
            // entity has anchors so a reader that predates anchors is unaffected. Carries
            // the stored anchor records plus their class/grain composition
            // (derived inputs; tree-grain fan-out on its own axis) and, for a
            // path-medium mem, each anchor's live resolution `state`
            // (resolves / drifted / recheck / orphaned — additive per-anchor
            // field). A present hash-bearing anchor adjudicates its recorded
            // prepared-content hash against the observed one, so `drifted` is
            // deterministic on a stable medium; `state` is absent when the
            // source is unobserved (a `url` grain, or an `entity` grain whose
            // mem is not mounted), never fabricated.
            // An unreadable sidecar is a condition on the read, never an
            // absent `anchors` key: the entity's anchors are unknown.
            if let Some(why) = engine.anchors_sidecar_error(id.mem()) {
                obj.insert(
                    "anchors_sidecar_error".into(),
                    serde_json::json!({
                        "code": "ANCHORS_SIDECAR_UNREADABLE",
                        "mem": id.mem(),
                        "reason": &why,
                    }),
                );
                md.push_str(&format!(
                    "\n\n> **ANCHORS_SIDECAR_UNREADABLE** — mem `{}`: {why}. This entity's \
                     provenance anchors are unknown, not absent.\n",
                    id.mem()
                ));
            }
            let resolved = engine.entity_anchors_resolved(&id);
            if !resolved.is_empty() {
                let anchors: Vec<memstead_base::anchor::Anchor> =
                    resolved.iter().map(|r| r.anchor.clone()).collect();
                let composition = memstead_base::anchor::compose_entity_anchors(&anchors);
                obj.insert(
                    "anchors".into(),
                    serde_json::to_value(&resolved).unwrap_or(serde_json::Value::Null),
                );
                obj.insert(
                    "anchor_composition".into(),
                    serde_json::to_value(&composition).unwrap_or(serde_json::Value::Null),
                );
            }
        }

        let budget = p.token_budget.unwrap_or(self.token_budget);
        attach_mem_changed_to_result(
            match apply_chunking(&md, budget, p.chunk, &extra_fm) {
                Ok(result) => md_with_structured(
                    prepend_drift_warnings_md(result, &drift_warnings),
                    structured,
                ),
                Err(e) => prepend_drift_warnings_to_result_text(
                    tool_error("INVALID_INPUT", &e),
                    &drift_warnings,
                ),
            },
            mem_changed_notices,
        )
    }

    #[tool(
        name = "memstead_search",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(super) fn memstead_search(
        &self,
        Parameters(p): Parameters<SearchParams>,
    ) -> CallToolResult {
        let filters = p.filters.clone().unwrap_or_default();

        // Telemetry: one line per invocation with flags only — no
        // query strings, no entity content. Enables Tier 2 (regex / fuzzy /
        // nested / per-term field) to be decided from real usage data.
        let q = p.query.as_ref();
        tracing::info!(
            any_term_count = q.map(|q| q.any.len()).unwrap_or(0),
            has_phrase = q.is_some_and(|q| q.phrase.is_some()),
            has_not = q.is_some_and(|q| !q.not.is_empty()),
            has_field = q.is_some_and(|q| q.field.is_some()),
            has_expand = p.expand_via.as_ref().is_some_and(|v| !v.is_empty()),
            mem_scope = if p.mem.is_some() { "one" } else { "all" },
            "memstead_search invoked"
        );

        // Snapshot the mem filter for drift detection before the scope
        // construction below moves `p.mem`.
        let mem_filter = p.mem.clone();

        let scope = SearchScope {
            query: p.query,
            mem: p.mem,
            entity_type: p.entity_type,
            limit: p.limit,
            offset: p.offset,
            filters,
            // Thread the
            // agent's `range_filters` input into the engine arg.
            // The engine's `collect_range_filter_warnings` was
            // ready-but-unreachable before this wiring.
            range_filters: p.range_filters.unwrap_or_default(),
            edge_type: p.edge_type,
            related_to: p.related_to.map(EntityId),
            depth: p.depth,
            expand_via: p.expand_via,
            expand_depth: p.expand_depth,
            direction: p.direction.unwrap_or_default(),
            stub: p.stub,
            token_budget: p.token_budget,
        };
        let offset = scope.offset.unwrap_or(0);

        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        // Graph-walking forms cross mem boundaries — `related_to` is a
        // BFS over the whole graph and `expand_via` follows edges
        // wherever they lead — so they take the full lazy-mount load
        // rather than walking a partial store. A plain mem-filtered
        // text search stays scoped: its answer lives in one mem.
        if scope.related_to.is_some() || scope.expand_via.is_some() {
            engine.ensure_mems_loaded(None);
        }
        let drift_warnings = engine.reload_if_stale(mem_filter.as_deref());
        let (engine, mem_changed_notices) = engine.finish();
        let result = match engine.search(&scope) {
            Ok(r) => r,
            Err(e) => {
                return attach_drift_to_error(
                    engine_err_unified(e, &engine),
                    &drift_warnings,
                    mem_changed_notices,
                );
            }
        };

        let md = render::render_search_markdown(&result, offset);
        // Structured envelope
        // on `structured_content`, rendered markdown on the text
        // channel. Search results have a useful human-readable
        // canonical form (the rendered prose with score lines) and
        // a typed branching shape — both ship in one call.
        // Every hit carries `origin` (first-party / third-party), stamped
        // by the shared envelope builder so the CLI's `--json` and this
        // `structured_content` agree key for key.
        let envelope =
            render::build_search_envelope(&result, offset, &|m| engine.mem_origin_class(m));
        let structured = serde_json::to_value(&envelope).unwrap_or(serde_json::Value::Null);
        attach_mem_changed_to_result(
            md_with_structured(prepend_drift_warnings_md(md, &drift_warnings), structured),
            mem_changed_notices,
        )
    }

    // ----------------------------------------------------------------------
    // Community detection + schema tools
    // ----------------------------------------------------------------------

    #[tool(
        name = "memstead_overview",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        meta = always_load_meta()
    )]
    pub(super) fn memstead_overview(
        &self,
        Parameters(p): Parameters<OverviewParams>,
    ) -> CallToolResult {
        // The schemas catalogue includes rule-referenced unpinned
        // schemas via `engine.workspace_schemas()`.
        let unified = self.unified_engine();
        self.memstead_overview_unified(p, unified.clone())
    }

    /// Unified-engine path for [`Self::memstead_overview`]. Body lifted to
    /// [`memstead_base::overview::compose_overview`] so the full CLI
    /// surfaces the same rich-content output via the same composer.
    /// This wrapper handles drift-warning collection, error-envelope
    /// mapping, and response-cap chunking; the composer produces the
    /// markdown body + warnings + extra-frontmatter.
    pub(super) fn memstead_overview_unified(
        &self,
        p: OverviewParams,
        unified: Arc<Mutex<memstead_base::Engine>>,
    ) -> CallToolResult {
        let mut engine = crate::lock_engine!(unified);
        // Overview's community detection is workspace-global BY CONTRACT
        // (a `mem` filter only scopes which clusters are reported, never
        // the partition), so even a mem-scoped overview takes the full
        // lazy-mount load — cluster ids computed over a partial store
        // would be a different partition presented as the global one.
        engine.ensure_mems_loaded(None);
        let drift_warnings = engine.reload_if_stale(p.mem.as_deref());

        let include = p.include.clone().unwrap_or_default();
        let args = memstead_base::overview::OverviewArgs {
            include: &include,
            mem: p.mem.as_deref(),
            rebuild: p.rebuild.unwrap_or(false) && p.chunk.unwrap_or(1) <= 1,
            token_budget: p
                .token_budget
                .unwrap_or(memstead_base::overview::DEFAULT_OVERVIEW_BUDGET),
            operator_mode: self.operator_mode,
            // The section is a truthful function of which tools this
            // server exposes: an embedder (or a workspace's
            // `[mcp].disabled_tools`) that withholds both lifecycle tools
            // gets no section naming them.
            suppress_lifecycle: self.disabled_tools.contains("memstead_mem_create")
                && self.disabled_tools.contains("memstead_mem_delete"),
        };

        let out = match memstead_base::overview::compose_overview(
            &mut engine,
            args,
            memstead_base::overview::Surface::Mcp,
        ) {
            Ok(o) => o,
            Err(memstead_base::overview::ComposeOverviewError::InvalidIncludeKeySchemaTypes) => {
                let msg = "include key 'schema_types' was removed; \
                           call memstead_schema(name=...) for full schema bodies."
                    .to_string();
                return tool_error_with_payload(
                    "INVALID_INPUT",
                    &msg,
                    envelope(
                        "INVALID_INPUT",
                        msg.clone(),
                        serde_json::json!({ "message": msg }),
                    ),
                );
            }
            Err(memstead_base::overview::ComposeOverviewError::MemQuarantined(name)) => {
                let err = engine.unknown_mem_error(&name);
                return engine_err_unified(err, &engine);
            }
            Err(memstead_base::overview::ComposeOverviewError::UnknownMem {
                name,
                writable_mems,
            }) => {
                let msg = format!(
                    "unknown mem: \"{name}\". Writable mems: [{}]",
                    writable_mems.join(", ")
                );
                return tool_error_with_payload(
                    "UNKNOWN_MEM",
                    &msg,
                    envelope(
                        "UNKNOWN_MEM",
                        msg.clone(),
                        serde_json::json!({
                            "name": name,
                            "writable_mems": writable_mems,
                        }),
                    ),
                );
            }
        };

        // Promote the composer's extra-frontmatter into the
        // `apply_chunking` shape (`Vec<(&str, &str)>`). The slots stay
        // alive through `out` for the duration of this call.
        let extra_fm: Vec<(&str, &str)> = out
            .extra_frontmatter
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        match apply_chunking(&out.markdown, self.token_budget, p.chunk, &extra_fm) {
            Ok(r) => md_response(prepend_drift_warnings_md(r, &drift_warnings)),
            Err(e) => tool_error("INVALID_INPUT", &e),
        }
    }

    #[tool(
        name = "memstead_schema",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(super) fn memstead_schema(
        &self,
        Parameters(p): Parameters<SchemaParams>,
    ) -> CallToolResult {
        // The unified engine exposes `schemas()` as a HashMap keyed by
        // mem name (one schema per mem per V1). suggest_name is
        // not available on the unified surface (the per-mem HashMap
        // has no fuzzy index); not-found errors carry an empty
        // suggestions list — wire shape stays consistent.
        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        let drift_warnings = engine.reload_if_stale(None);
        let (engine, mem_changed_notices) = engine.finish();

        // Resolve the effective schema name. Accept exactly one of
        // `name` (canonical) or `mem` (mount-roster lookup); the
        // pair `(Some, Some)` and `(None, None)` are typed input
        // errors so an agent that misreads the API gets a precise
        // failure rather than silent fallback. `mem` resolves
        // through the same cascade as `name` once the engine maps
        // the mem to its pinned `schema_ref`.
        let effective_name: String = match (p.name.as_deref(), p.mem.as_deref()) {
            (Some(_), Some(_)) => {
                let msg =
                    "memstead_schema accepts exactly one of `name` or `mem`, not both.".to_string();
                return attach_drift_to_error(
                    tool_error_with_payload(
                        "INVALID_INPUT",
                        &msg,
                        envelope(
                            "INVALID_INPUT",
                            msg.clone(),
                            serde_json::json!({ "message": msg }),
                        ),
                    ),
                    &drift_warnings,
                    mem_changed_notices,
                );
            }
            (None, None) => {
                let msg = "memstead_schema requires either `name` or `mem`.".to_string();
                return attach_drift_to_error(
                    tool_error_with_payload(
                        "INVALID_INPUT",
                        &msg,
                        envelope(
                            "INVALID_INPUT",
                            msg.clone(),
                            serde_json::json!({ "message": msg }),
                        ),
                    ),
                    &drift_warnings,
                    mem_changed_notices,
                );
            }
            (Some(name), None) => name.to_string(),
            (None, Some(mem)) => match engine.mount(mem) {
                Some(m) => m.schema.as_ref().map(|s| s.to_string()).unwrap_or_default(),
                None if engine.quarantine_reason(mem).is_some() => {
                    let err = engine.unknown_mem_error(mem);
                    return attach_drift_to_error(
                        engine_err_unified(err, &engine),
                        &drift_warnings,
                        mem_changed_notices,
                    );
                }
                None => {
                    let known_mems: Vec<String> =
                        engine.mounts().iter().map(|m| m.mem.clone()).collect();
                    let msg = format!("unknown mem: \"{mem}\"");
                    return attach_drift_to_error(
                        tool_error_with_payload(
                            "UNKNOWN_MEM",
                            &msg,
                            envelope(
                                "UNKNOWN_MEM",
                                msg.clone(),
                                serde_json::json!({
                                    "name": mem,
                                    "known_mems": known_mems,
                                }),
                            ),
                        ),
                        &drift_warnings,
                        mem_changed_notices,
                    );
                }
            },
        };

        // Lookup: name@version path uses parsed pin; bare-name path
        // picks the first matching schema by name. Cascade covers
        // mem-pinned, workspace-loaded, and embedded built-in
        // catalogues so any pin `memstead_mem_create` would accept also
        // resolves through `memstead_schema` — agents reading
        // `memstead_overview`'s lifecycle namespaces can introspect their
        // schemas without first creating a mem.
        let schema_arc: Option<std::sync::Arc<memstead_schema::Schema>> =
            if effective_name.contains('@') {
                match effective_name.parse::<memstead_schema::SchemaRef>() {
                    Ok(parsed) => find_schema_unified(&engine, &parsed).cloned(),
                    Err(_) => None,
                }
            } else {
                find_schema_by_name(&engine, &effective_name).cloned()
            };
        let schema = match schema_arc {
            Some(s) => s,
            None => {
                let msg = format!("schema not found: \"{effective_name}\"");
                return attach_drift_to_error(
                    tool_error_with_payload(
                        "ENTITY_NOT_FOUND",
                        &msg,
                        envelope(
                            "ENTITY_NOT_FOUND",
                            msg.clone(),
                            serde_json::json!({
                                "id": effective_name,
                                "suggestions": Vec::<String>::new(),
                            }),
                        ),
                    ),
                    &drift_warnings,
                    mem_changed_notices,
                );
            }
        };

        // `used_by` — every writable mem whose pinned schema
        // resolves to this one. Iterate mounts(), compare each
        // mount.schema with the matched schema's canonical pin.
        let canon = format!("{}@{}", schema.manifest.name, schema.version);
        let mut used_by: Vec<String> = engine
            .mounts()
            .iter()
            .filter(|m| m.schema.as_ref().map(|s| s.to_string()).as_deref() == Some(canon.as_str()))
            .map(|m| m.mem.clone())
            .collect();
        used_by.sort();

        // Resolve the optional `verbosity` toggle. Absent → lite: a fresh
        // session following the schema-discovery contract pays the
        // skeleton price (~7 KB), not the full-prose price (~52 KB); the
        // full body stays one explicit `verbosity: "full"` away. An
        // unrecognized value is a typed `INVALID_INPUT` naming the bad
        // value rather than a silent fallback to full/lite — the same
        // anti-silent-no-op principle the write-path plans enforce.
        let verbosity = match p.verbosity.as_deref() {
            None => render::SchemaVerbosity::Lite,
            Some(v) => match render::SchemaVerbosity::from_wire(v) {
                Some(sv) => sv,
                None => {
                    let msg = format!("unknown verbosity: \"{v}\" — expected \"full\" or \"lite\"");
                    return attach_drift_to_error(
                        tool_error_with_payload(
                            "INVALID_INPUT",
                            &msg,
                            envelope(
                                "INVALID_INPUT",
                                msg.clone(),
                                serde_json::json!({
                                    "value": v,
                                    "allowed": ["full", "lite"],
                                }),
                            ),
                        ),
                        &drift_warnings,
                        mem_changed_notices,
                    );
                }
            },
        };
        // Trust origin governs de-framing: a third-party schema is served
        // structural-only regardless of the requested `verbosity` (the
        // prose-instruction fields never reach the agent as instructions).
        let origin = engine.schema_origin(&schema);
        // Serving-shape controls (an earlier plana): `types`
        // scopes the per-type prose; the token budget guards the
        // unscoped full reply with a visible degrade instead of a
        // response-cap overflow.
        let payload = match render::build_schema_payload_scoped(
            &schema,
            used_by,
            verbosity,
            origin,
            p.types.as_deref(),
            Some(p.token_budget.unwrap_or(render::DEFAULT_SCHEMA_FULL_BUDGET)),
        ) {
            Ok(v) => v,
            Err(unknown) => {
                let msg = format!(
                    "unknown entity type(s) in `types`: [{}] — valid types: [{}]",
                    unknown.unknown.join(", "),
                    unknown.known.join(", ")
                );
                return attach_drift_to_error(
                    tool_error_with_payload(
                        "UNKNOWN_ENTITY_TYPE",
                        &msg,
                        envelope(
                            "UNKNOWN_ENTITY_TYPE",
                            msg.clone(),
                            serde_json::json!({
                                "unknown": unknown.unknown,
                                "known_types": unknown.known,
                            }),
                        ),
                    ),
                    &drift_warnings,
                    mem_changed_notices,
                );
            }
        };
        let mut res = json_response(&payload);
        for w in &drift_warnings {
            res = append_warning_hint(res, w);
        }
        attach_mem_changed_to_result(res, mem_changed_notices)
    }

    // ----------------------------------------------------------------------
    // Admin tools
    // ----------------------------------------------------------------------

    #[tool(
        name = "memstead_health",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(super) fn memstead_health(
        &self,
        Parameters(p): Parameters<HealthParams>,
    ) -> CallToolResult {
        // include_config: true is served end-to-end via the unified
        // accessors (gitdir_for / worktree_for / mem_head_sha /
        // mem_config_for). `mutations` + `plugin` remain server
        // state on `self`.
        //
        // Caveat: git-branch mounts return `None` from
        // `mem_config_for` until the backend's config-read path
        // lifts; under include_config: true their per-mem entries
        // emit without `write_guidance` / `extra` (the `vcs` block
        // is present instead).
        let unified = self.unified_engine();
        self.memstead_health_unified(p, unified.clone())
    }

    /// Body of [`Self::memstead_health`]. Default shape (`summary`,
    /// totals, distributions, rosters, `mem_schemas`) plus the
    /// eight `include` detail sections (orphans, stubs,
    /// most_connected, missing_fields, stale, dangling_links, tags,
    /// missing_required_outgoing). `include_config: true` adds
    /// `mutations`, `plugin`, and per-mem `vcs` / `write_guidance`
    /// / `extra` — the vcs subobject for git-branch mounts uses
    /// the worktree heuristic from
    /// [`memstead_base::Engine::worktree_for`].
    pub(super) fn memstead_health_unified(
        &self,
        p: HealthParams,
        unified: Arc<Mutex<memstead_base::Engine>>,
    ) -> CallToolResult {
        let mut engine = crate::lock_engine!(unified);
        // Health always takes the FULL lazy-mount load, mirroring
        // overview: a `mem` filter scopes what is REPORTED, but the
        // answer's cross-mem components — dangling-link adjudication
        // (targets live anywhere; an unloaded real target reads as a
        // stub and would be reported as a broken link) and the
        // workspace-global community partition — are only truthful over
        // a complete store. The second final grade demonstrated 28
        // false dangling links on a mem-scoped health over a lazy
        // workspace; a partial-store count presented as truth is the
        // forbidden rendering.
        engine.ensure_mems_loaded(None);
        let drift_warnings = engine.reload_if_stale(p.mem.as_deref());
        let (mut engine, mem_changed_notices) = engine.finish();

        let include = p.include.unwrap_or_default();
        let args = memstead_base::ops::health_compose::HealthArgs {
            mem: p.mem.as_deref(),
            include: &include,
            limit: p.limit,
            target_schema: p.target_schema.as_deref(),
            include_config: p.include_config,
            strict: false,
            today: None,
        };
        // Server-owned config the engine does not carry — prebuilt here so the
        // composer inserts the bytes verbatim (and stays free of the MCP
        // server's config types).
        let plugin_json: serde_json::Map<String, serde_json::Value> = self
            .plugin
            .iter()
            .map(|(k, v)| {
                let json = serde_json::to_value(v).unwrap_or(serde_json::Value::Null);
                (k.clone(), json)
            })
            .collect();
        let config = memstead_base::ops::health_compose::HealthConfig {
            mutations: serde_json::json!({ "require_notes": self.mutations.require_notes }),
            plugin: serde_json::Value::Object(plugin_json),
        };

        let result = match memstead_projection::health::compose_health(
            &mut engine,
            &args,
            drift_warnings,
            &config,
        ) {
            Ok(v) => v,
            Err(memstead_base::ops::health_compose::ComposeHealthError::MemQuarantined(name)) => {
                let err = engine.unknown_mem_error(&name);
                return engine_err_unified(err, &engine);
            }
            Err(memstead_base::ops::health_compose::ComposeHealthError::UnknownMem {
                name,
                writable_mems,
            }) => {
                let msg = format!(
                    "unknown mem: \"{name}\". Writable mems: [{}]",
                    writable_mems.join(", ")
                );
                return tool_error_with_payload(
                    "UNKNOWN_MEM",
                    &msg,
                    envelope(
                        "UNKNOWN_MEM",
                        msg.clone(),
                        serde_json::json!({
                            "name": name,
                            "writable_mems": writable_mems,
                        }),
                    ),
                );
            }
            Err(memstead_base::ops::health_compose::ComposeHealthError::InvalidTargetSchema {
                raw,
                reason,
            }) => {
                let msg = format!("invalid target_schema {raw:?}: {reason}");
                return tool_error_with_payload(
                    "INVALID_INPUT",
                    &msg,
                    envelope(
                        "INVALID_INPUT",
                        msg.clone(),
                        serde_json::json!({ "target_schema": raw, "reason": reason }),
                    ),
                );
            }
            Err(memstead_base::ops::health_compose::ComposeHealthError::Engine(e)) => {
                return engine_err_unified(e, &engine);
            }
        };

        let res = json_response(&result);
        let res = match p
            .mem
            .as_deref()
            .and_then(|v| mem_schema_ref_unified(&engine, v))
        {
            Some(s) => with_mem_schema_anchor(res, &s),
            None => res,
        };
        let res = attach_mem_changed_to_result(res, mem_changed_notices);
        // #57: the text channel is chunkable markdown rendered from the
        // final structured payload (which ships whole), so a multi-include
        // report can't overflow the response cap. Done last — after the
        // anchor / mem-changed post-processing that mutates
        // `structured_content`.
        finalize_health_text(res, p.token_budget.unwrap_or(self.token_budget), p.chunk)
    }

    #[tool(
        name = "memstead_changes_since",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(super) fn memstead_changes_since(
        &self,
        Parameters(p): Parameters<ChangesSinceParams>,
    ) -> CallToolResult {
        // The engine's
        // `changes_since` populates `notes` and `memstead_ref` on every
        // git-branch call — the rename map is note-driven, so the
        // walk happens regardless. `include_notes` becomes a
        // renderer-side filter on the wire response: when `false`,
        // strip the fields so the wire shape matches the
        // `include_notes: false` contract.
        let include_notes = p.include_notes;
        let unified = self.unified_engine();
        let mut engine = crate::lock_engine!(unified);
        let drift_warnings = engine.reload_if_stale(Some(&p.mem));
        let (engine, mem_changed_notices) = engine.finish();
        let mem_for_anchor = p.mem.clone();
        let res = match engine.changes_since(&p.mem, &p.since, p.rename_similarity) {
            Ok(mut report) => {
                if !include_notes {
                    report.notes = None;
                    report.memstead_ref = None;
                }
                let mut res = json_response(&report);
                for w in &drift_warnings {
                    res = append_warning_hint(res, w);
                }
                match mem_schema_ref_unified(&engine, &mem_for_anchor) {
                    Some(s) => with_mem_schema_anchor(res, &s),
                    None => res,
                }
            }
            Err(e) => {
                // Delegate to the typed-envelope translator so the wire
                // `code` matches `EngineError::code()` for the underlying
                // variant. A bad `since` cursor now arrives as the typed
                // `EngineError::InvalidChangesCursor` (code `INVALID_CURSOR`,
                // `details.mem` + untruncated `details.since`) — lifted
                // from the backend's typed marker in `Engine::changes_since`
                // rather than sniffed out of a raw backend message string here.
                // Genuine backend faults still surface `MEM_ERROR`.
                // The structured notice rides via the shared
                // `attach_mem_changed_to_result` below; prepend the
                // `MEM_RELOADED` text line here so the error path
                // carries the same channel split a success carries.
                prepend_drift_warnings_to_result_text(
                    engine_err_unified(e, &engine),
                    &drift_warnings,
                )
            }
        };
        attach_mem_changed_to_result(res, mem_changed_notices)
    }

    #[tool(
        name = "memstead_diff",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(super) fn memstead_diff(&self, Parameters(p): Parameters<DiffParams>) -> CallToolResult {
        let unified = self.unified_engine();
        let engine = crate::lock_engine!(unified);
        let config = memstead_base::ops::DiffConfig {
            rename_similarity: p
                .rename_similarity
                .unwrap_or(memstead_base::ops::RENAME_SIMILARITY_DEFAULT),
            include_content: p.include_content,
            include_ripple: p.include_ripple,
        };
        match engine.diff(&p.mem, &p.ref_a, &p.ref_b, Some(config)) {
            Ok(diff) => json_response(&diff),
            Err(e) => engine_err_unified(e, &engine),
        }
    }
}
