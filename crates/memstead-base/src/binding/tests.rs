#![cfg(test)]

/// Scaffolded scope is medium-shaped, and every rule it writes is one
/// something interprets. The path mediums keep `**/*` byte-for-byte;
/// `graph` gets the entity vocabulary; `web` gets NO rule, because its
/// namespace has no selector vocabulary and a pattern there would be
/// decorative in exactly the way the graph glob was — printed at an agent
/// as selection while reaching nothing.
#[test]
fn scaffolded_scope_is_medium_shaped_and_never_decorative() {
    let scope_of = |medium: MediumType| {
        scaffold_binding(ScaffoldParams {
            destination_mem: "m",
            source_name: "s",
            pointer: "p",
            medium_type: medium,
            intent: None,
            additional_deny_paths: Vec::new(),
        })
        .binding
        .sources[0]
            .scope
            .iter()
            .map(|r| r.path.clone())
            .collect::<Vec<_>>()
    };

    // Unchanged, and asserted so a graph-shaped fix can never drift them.
    assert_eq!(scope_of(MediumType::Codebase), vec!["**/*".to_string()]);
    assert_eq!(scope_of(MediumType::Filesystem), vec!["**/*".to_string()]);
    assert_eq!(scope_of(MediumType::Git), vec!["**/*".to_string()]);

    // Entity namespace: a legal selector, and one the run time honours.
    assert_eq!(scope_of(MediumType::Graph), vec!["*".to_string()]);
    assert!(
        crate::source_scope::parse_entity_selector("*").is_some(),
        "the graph scaffold writes a selector the parser accepts"
    );

    // No vocabulary exists, so no rule is written.
    assert!(
        scope_of(MediumType::Web).is_empty(),
        "a web facet carries no scope rather than one nothing interprets"
    );
}
use super::*;
use crate::pipeline::PatternMode;

// ---- builders -------------------------------------------------------

fn build_op() -> BuildOperation {
    BuildOperation {
        mode: BuildMode::Discovery,
        trigger: IngestTrigger::Loop,
        batch_size: 20,
        post_actions: None,
    }
}

fn allow(path: &str) -> PatternEntry {
    PatternEntry {
        path: path.to_string(),
        mode: PatternMode::Allow,
    }
}

fn source(
    name: &str,
    medium_type: MediumType,
    pointer: &str,
    scope: Vec<PatternEntry>,
    preparation: Option<&str>,
    change_detection: Option<&str>,
) -> Source {
    Source {
        name: name.to_string(),
        medium_type,
        pointer: pointer.to_string(),
        change_detection: change_detection.map(str::to_string),
        scope,
        engagement: None,
        preparation: preparation.map(str::to_string),
    }
}

fn codebase_source() -> Source {
    source(
        "source-tree",
        MediumType::Codebase,
        "../public",
        vec![allow("../public/**/*.rs")],
        None,
        None,
    )
}

fn binding() -> Binding {
    Binding {
        version: BINDING_VERSION,
        intent: Some("prose for the agent".to_string()),
        sources: vec![codebase_source()],
        reference_mems: vec!["engine".to_string()],
        destination_mem: "plugin".to_string(),
        deny_paths: vec!["VISION.md".to_string(), "dev/**".to_string()],
        coverage_semantics: None,
        rules: Some(serde_json::json!({ "routing": "…" })),
        prune: None,
        operations: Operations {
            build: Some(build_op()),
            sync: Some(SyncOperation {
                trigger: IngestTrigger::Manual,
                batch_size: 20,
            }),
            verify: Some(VerifyOperation {
                trigger: IngestTrigger::Manual,
                batch_size: 20,
                adjudication_cap: DEFAULT_ADJUDICATION_CAP,
                full_resync_every: DEFAULT_FULL_RESYNC_EVERY,
            }),
        },
    }
}

// ---- Binding serde --------------------------------------------------

/// A v2 binding round-trips: serialize → deserialize → equal.
#[test]
fn binding_round_trips() {
    let b = binding();
    let json = serde_json::to_string(&b).unwrap();
    let back: Binding = serde_json::from_str(&json).unwrap();
    assert_eq!(back, b);
}

/// The plan's v2 wire example deserializes: inline sources with both
/// halves, the operations block, and coverage semantics as declared.
#[test]
fn plan_shaped_v2_json_deserializes() {
    let src = r#"{
          "version": 2,
          "intent": "prose the building agent reads before every run",
          "sources": [
            {
              "name": "source-tree",
              "type": "codebase",
              "pointer": "../public",
              "change_detection": "auto",
              "scope": [
                { "path": "../public/**/*.rs", "mode": "allow" },
                { "path": "../public/target/**", "mode": "deny" }
              ]
            }
          ],
          "reference_mems": ["engineering"],
          "destination_mem": "engine",
          "deny_paths": ["../dev/**"],
          "coverage_semantics": "exhaustive",
          "operations": {
            "build":  { "mode": "discovery", "trigger": "loop", "batch_size": 20 },
            "sync":   { "trigger": "loop", "batch_size": 20 },
            "verify": { "trigger": "loop", "batch_size": 20,
                        "adjudication_cap": 50, "full_resync_every": 20 }
          }
        }"#;
    let b: Binding = serde_json::from_str(src).unwrap();
    assert_eq!(b.version, 2);
    assert_eq!(b.destination_mem, "engine");
    assert_eq!(b.sources.len(), 1);
    let s = &b.sources[0];
    assert_eq!(s.name, "source-tree");
    assert_eq!(s.medium_type, MediumType::Codebase);
    assert_eq!(s.pointer, "../public");
    assert_eq!(s.change_detection.as_deref(), Some("auto"));
    assert_eq!(s.scope.len(), 2);
    assert_eq!(b.reference_mems, vec!["engineering".to_string()]);
    assert_eq!(b.coverage_semantics, Some(CoverageSemantics::Exhaustive));
    assert_eq!(
        b.operations.build.as_ref().unwrap().mode,
        BuildMode::Discovery
    );
    assert!(b.operations.sync.is_some());
    assert_eq!(b.operations.verify.as_ref().unwrap().adjudication_cap, 50);
}

/// An absent `coverage_semantics` deserializes to `None` ("not
/// stated" — resolved per medium, never a baked-in default), and
/// `one-shot` is the kebab wire form.
#[test]
fn coverage_defaults_and_one_shot_wire_form() {
    let src = r#"{
          "version": 2,
          "destination_mem": "m",
          "operations": { "build": { "mode": "one-shot", "trigger": "manual", "batch_size": 5 } }
        }"#;
    let b: Binding = serde_json::from_str(src).unwrap();
    assert_eq!(b.coverage_semantics, None, "absent = not stated");
    assert_eq!(
        b.operations.build.as_ref().unwrap().mode,
        BuildMode::OneShot
    );
    assert!(b.operations.sync.is_none());
    assert!(b.operations.verify.is_none());
    // one-shot serializes to the kebab form.
    assert_eq!(
        serde_json::to_string(&BuildMode::OneShot).unwrap(),
        r#""one-shot""#
    );
}

/// The tier-3 knobs are additive: a `verify` block without them
/// deserializes to the engine defaults, and a block that sets them
/// round-trips its values.
#[test]
fn verify_tier3_knobs_default_and_round_trip() {
    let src = r#"{
          "version": 2,
          "destination_mem": "m",
          "operations": {
            "build": { "mode": "discovery", "trigger": "loop", "batch_size": 20 },
            "verify": { "trigger": "manual", "batch_size": 20 }
          }
        }"#;
    let b: Binding = serde_json::from_str(src).unwrap();
    let v = b.operations.verify.as_ref().unwrap();
    assert_eq!(v.adjudication_cap, DEFAULT_ADJUDICATION_CAP);
    assert_eq!(v.full_resync_every, DEFAULT_FULL_RESYNC_EVERY);

    // Explicit values round-trip.
    let explicit = VerifyOperation {
        trigger: IngestTrigger::Manual,
        batch_size: 10,
        adjudication_cap: 7,
        full_resync_every: 3,
    };
    let json = serde_json::to_string(&explicit).unwrap();
    let back: VerifyOperation = serde_json::from_str(&json).unwrap();
    assert_eq!(back, explicit);
    assert!(json.contains("adjudication_cap"));
    assert!(json.contains("full_resync_every"));
}

/// The tier-3 scheduling knobs never change `hash(D)` — they are excluded
/// with the rest of the `verify` block (scheduling never changes the claim).
#[test]
fn tier3_knobs_do_not_change_the_hash() {
    let base = hash_binding(&binding());
    let mut tuned = binding();
    let v = tuned.operations.verify.as_mut().unwrap();
    v.adjudication_cap = 999;
    v.full_resync_every = 1;
    assert_eq!(
        base,
        hash_binding(&tuned),
        "tier-3 verify knobs are excluded from hash(D)"
    );
}

/// `"mode": "refinement"` is a deleted value — deserialization fails.
#[test]
fn refinement_mode_is_rejected() {
    let src = r#"{
          "version": 2,
          "destination_mem": "m",
          "operations": { "build": { "mode": "refinement", "trigger": "loop", "batch_size": 20 } }
        }"#;
    let err = serde_json::from_str::<Binding>(src).unwrap_err();
    assert!(
        err.to_string().contains("refinement") || err.to_string().contains("unknown variant"),
        "unexpected error: {err}"
    );
}

/// `version` is required — a projection file without it refuses.
#[test]
fn version_is_required() {
    let src = r#"{
          "destination_mem": "m",
          "operations": { "build": { "mode": "discovery", "trigger": "loop", "batch_size": 20 } }
        }"#;
    assert!(serde_json::from_str::<Binding>(src).is_err());
}

// ---- hash(D) --------------------------------------------------------

/// `hash(D)` is stable and recomputable: the same binding hashes
/// identically, and the digest is 64 lowercase hex chars.
#[test]
fn hash_is_stable_and_recomputable() {
    let b = binding();
    let h1 = hash_binding(&b);
    let h2 = hash_binding(&b);
    assert_eq!(h1, h2);
    assert_eq!(h1.len(), 64);
    assert!(
        h1.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
}

/// Changing a source's selection pattern — now an input *inside* the one
/// record — changes the hash.
#[test]
fn changing_a_source_pattern_changes_the_hash() {
    let base = hash_binding(&binding());
    let mut changed = binding();
    changed.sources[0].scope = vec![allow("../public/**/*.md")];
    assert_ne!(base, hash_binding(&changed));
}

/// Changing a source's pointer changes the hash.
#[test]
fn changing_a_source_pointer_changes_the_hash() {
    let base = hash_binding(&binding());
    let mut changed = binding();
    changed.sources[0].pointer = "../elsewhere".to_string();
    assert_ne!(base, hash_binding(&changed));
}

/// Changing `trigger`, `batch_size`, or `post_actions` does **not** change
/// the hash — scheduling never changes what the mem claims. Neither does a
/// source's `engagement` contract (the pre-consolidation exclusion carried
/// forward).
#[test]
fn scheduling_knobs_do_not_change_the_hash() {
    let base = hash_binding(&binding());

    let mut b_trigger = binding();
    b_trigger.operations.build.as_mut().unwrap().trigger = IngestTrigger::Manual;
    assert_eq!(base, hash_binding(&b_trigger), "trigger is excluded");

    let mut b_batch = binding();
    b_batch.operations.build.as_mut().unwrap().batch_size = 999;
    assert_eq!(base, hash_binding(&b_batch), "batch_size is excluded");

    let mut b_post = binding();
    b_post.operations.build.as_mut().unwrap().post_actions =
        Some(serde_json::json!({ "archive_source": false }));
    assert_eq!(base, hash_binding(&b_post), "post_actions is excluded");

    // The sync/verify blocks are excluded too.
    let mut b_sync = binding();
    b_sync.operations.sync = None;
    assert_eq!(base, hash_binding(&b_sync), "sync block is excluded");

    // A source's engagement contract is excluded.
    let mut b_engage = binding();
    b_engage.sources[0].engagement = Some(serde_json::json!({ "readVerb": "Study" }));
    assert_eq!(base, hash_binding(&b_engage), "engagement is excluded");
}

/// Changing `operations.build.mode` — a content-defining input — **does**
/// change the hash.
#[test]
fn changing_build_mode_changes_the_hash() {
    let base = hash_binding(&binding());
    let mut b = binding();
    b.operations.build.as_mut().unwrap().mode = BuildMode::OneShot;
    assert_ne!(base, hash_binding(&b));
}

/// An absent `build` block deserializes (serde default) and still hashes —
/// the build mode simply does not participate in `hash(D)`.
#[test]
fn absent_build_deserializes_and_hashes() {
    let src = r#"{
          "version": 2,
          "destination_mem": "m",
          "operations": { "verify": { "trigger": "manual", "batch_size": 5 } }
        }"#;
    let b: Binding = serde_json::from_str(src).unwrap();
    assert!(b.operations.build.is_none(), "absent build parses to None");
    let h = hash_binding(&b);
    assert_eq!(h.len(), 64);
}

// ---- capability matrix + validate -----------------------------------

/// The matrix rows are unchanged by the consolidation.
#[test]
fn capability_matrix_rows() {
    let web = medium_capabilities(MediumType::Web);
    assert!(!web.enumerable && !web.change_signal && !web.base_version_retrievable);
    assert!(!web.glob_deny_legal);
    assert_eq!(web.anchor_namespace, "url");

    let graph = medium_capabilities(MediumType::Graph);
    assert!(graph.enumerable && graph.change_signal && graph.base_version_retrievable);
    assert!(!graph.glob_deny_legal, "graph namespace is not path-shaped");
    assert_eq!(graph.anchor_namespace, "entity");

    for ty in [
        MediumType::Codebase,
        MediumType::Filesystem,
        MediumType::Git,
    ] {
        let c = medium_capabilities(ty);
        assert!(c.enumerable && c.change_signal && c.base_version_retrievable);
        assert!(c.glob_deny_legal, "{ty:?} allows glob deny_paths");
    }
    assert_eq!(
        medium_capabilities(MediumType::Git).anchor_namespace,
        "path+commit"
    );
}

/// An empty source name refuses, and a duplicate source name refuses —
/// per-source state keys must be present and collision-free.
#[test]
fn empty_and_duplicate_source_names_refuse() {
    let mut b = binding();
    b.deny_paths.clear();
    b.sources = vec![
        source("", MediumType::Codebase, "../a", vec![], None, None),
        source("dup", MediumType::Codebase, "../b", vec![], None, None),
        source("dup", MediumType::Codebase, "../c", vec![], None, None),
    ];
    let errs = validate_binding(&b).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| matches!(e, CapabilityError::EmptySourceName)),
        "expected EmptySourceName, got {errs:?}"
    );
    assert!(
        errs.iter().any(|e| matches!(
            e,
            CapabilityError::DuplicateSourceName { name } if name == "dup"
        )),
        "expected DuplicateSourceName, got {errs:?}"
    );
}

/// `sync` and `verify` over a `web` source each refuse as out-of-scope.
#[test]
fn sync_and_verify_over_web_refuse() {
    // Web binding, no deny_paths (globs illegal), no prep — isolate the op refusal.
    let mut b = binding();
    b.deny_paths.clear();
    b.sources = vec![source(
        "web-source",
        MediumType::Web,
        "https://example.com",
        vec![],
        None,
        None,
    )];
    let errs = validate_binding(&b).unwrap_err();
    let ops: Vec<&str> = errs
        .iter()
        .filter_map(|e| match e {
            CapabilityError::OperationOutOfScope { operation, .. } => Some(*operation),
            _ => None,
        })
        .collect();
    assert!(ops.contains(&"sync"), "sync refused: {errs:?}");
    assert!(ops.contains(&"verify"), "verify refused: {errs:?}");
}

/// Glob `deny_paths` over a `graph` source refuses.
#[test]
fn glob_deny_over_graph_refuses() {
    let mut b = binding();
    b.operations.sync = None;
    b.operations.verify = None;
    b.deny_paths = vec!["some/**".to_string()];
    b.sources = vec![source(
        "graph-source",
        MediumType::Graph,
        "home",
        vec![],
        None,
        None,
    )];
    let errs = validate_binding(&b).unwrap_err();
    assert!(
        errs.iter()
            .any(|e| matches!(e, CapabilityError::GlobDenyIllegal { .. })),
        "expected GlobDenyIllegal, got {errs:?}"
    );
}

/// A source preparation the registry does not know refuses at
/// validation time — the narrowed refusal: same error shape, "not in
/// this engine's registry" semantics, the registered set named.
#[test]
fn unregistered_preparation_refuses() {
    let mut b = binding();
    b.operations.sync = None;
    b.operations.verify = None;
    b.deny_paths.clear();
    b.sources = vec![source(
        "manual-pages",
        MediumType::Filesystem,
        "../docs",
        vec![],
        Some("pdf-to-markdown"),
        None,
    )];
    let errs = validate_binding(&b).unwrap_err();
    let refusal = errs
        .iter()
        .find(|e| {
            matches!(
                e,
                CapabilityError::PreparationUnsupported { preparation, impl_version, .. }
                    if preparation == "pdf-to-markdown" && *impl_version == PREPARATION_IMPL_VERSION
            )
        })
        .unwrap_or_else(|| panic!("expected PreparationUnsupported, got {errs:?}"));
    let msg = refusal.to_string();
    assert!(
        msg.contains("not in this engine's preparation registry"),
        "{msg}"
    );
    assert!(
        msg.contains("entity-load-bearing"),
        "names the registered set: {msg}"
    );
    assert!(
        !msg.contains("facet"),
        "the retired noun stays retired: {msg}"
    );
}

/// A registered preparation validates clean over a medium whose anchor
/// namespace admits its grain (`entity-load-bearing` over `graph`), and
/// refuses over one that does not (the same identifier over `codebase`,
/// where no entity-grain anchor could ever meet it).
#[test]
fn registered_preparation_validates_over_its_namespace_only() {
    let mut ok = binding();
    ok.deny_paths.clear();
    ok.sources = vec![source(
        "claims",
        MediumType::Graph,
        "home",
        vec![allow("*")],
        Some(crate::preparation::ENTITY_LOAD_BEARING),
        None,
    )];
    assert!(
        validate_binding(&ok).is_ok(),
        "registered preparation over its namespace validates clean: {:?}",
        validate_binding(&ok)
    );

    let mut mismatch = binding();
    mismatch.sources = vec![source(
        "source-tree",
        MediumType::Codebase,
        "../public",
        vec![allow("**/*.rs")],
        Some(crate::preparation::ENTITY_LOAD_BEARING),
        None,
    )];
    let errs = validate_binding(&mismatch).unwrap_err();
    assert!(
            errs.iter().any(|e| matches!(
                e,
                CapabilityError::PreparationGrainMismatch { preparation, anchor_namespace, .. }
                    if preparation == crate::preparation::ENTITY_LOAD_BEARING && *anchor_namespace == "path"
            )),
            "expected PreparationGrainMismatch, got {errs:?}"
        );
    assert!(
        !errs
            .iter()
            .any(|e| matches!(e, CapabilityError::PreparationUnsupported { .. })),
        "a registered identifier is never reported as unregistered"
    );
}

/// The impl version is hashed for EVERY source, with or without a
/// declared preparation: the hash a prior engine generation computed
/// (impl version 0) differs from the live one, so every finding keyed on
/// it is invalidated by construction when the constant bumps.
#[test]
fn impl_version_is_hashed_into_every_binding() {
    let plain = binding();
    assert!(plain.sources.iter().all(|s| s.preparation.is_none()));
    let live = hash_binding(&plain);
    assert_eq!(
        live,
        hash_binding_at_impl_version(&plain, PREPARATION_IMPL_VERSION)
    );
    assert_ne!(
        live,
        hash_binding_at_impl_version(&plain, 0),
        "the pre-registry generation's hash differs from the live one"
    );
    assert_ne!(
        live,
        hash_binding_at_impl_version(&plain, PREPARATION_IMPL_VERSION + 1)
    );

    let mut prepared = plain.clone();
    prepared.sources[0].preparation = Some(crate::preparation::ENTITY_LOAD_BEARING.to_string());
    assert_ne!(
        hash_binding(&prepared),
        live,
        "the identifier is hashed too"
    );
}

/// Every combination the matrix marks legal validates clean:
/// codebase / filesystem / git / graph bindings with build+sync+verify all
/// pass (graph carries no glob deny_paths, none carry preparation).
#[test]
fn legal_combinations_validate_clean() {
    // codebase / filesystem / git — path-shaped, deny_paths legal.
    for ty in [
        MediumType::Codebase,
        MediumType::Filesystem,
        MediumType::Git,
    ] {
        let mut b = binding();
        b.sources = vec![source(
            "f",
            ty,
            "../src",
            vec![allow("../src/**")],
            None,
            None,
        )];
        assert!(
            validate_binding(&b).is_ok(),
            "{ty:?} build+sync+verify should validate clean"
        );
    }
    // graph — build+sync+verify legal, but only without glob deny_paths.
    let mut graph_binding = binding();
    graph_binding.deny_paths.clear();
    graph_binding.sources = vec![source("g", MediumType::Graph, "home", vec![], None, None)];
    assert!(
        validate_binding(&graph_binding).is_ok(),
        "graph build+sync+verify with no glob deny should validate clean"
    );
}

// ---- prune block ------------------------------------------------------

/// The `prune` block is additive: a binding without it deserializes to
/// `prune: None`, an empty block enables prune and round-trips, and the
/// retired `guarantee` key of an earlier record is ignored on read.
#[test]
fn prune_block_is_additive_and_round_trips() {
    let src = r#"{
          "version": 2,
          "destination_mem": "m",
          "operations": { "build": { "mode": "discovery", "trigger": "loop", "batch_size": 20 } }
        }"#;
    let b: Binding = serde_json::from_str(src).unwrap();
    assert!(b.prune.is_none(), "absent prune parses to None");

    let enabled = r#"{
          "version": 2,
          "destination_mem": "m",
          "prune": {},
          "operations": { "build": { "mode": "discovery", "trigger": "loop", "batch_size": 20 } }
        }"#;
    let b: Binding = serde_json::from_str(enabled).unwrap();
    assert_eq!(b.prune, Some(PruneConfig::default()));
    let json = serde_json::to_string(&PruneConfig::default()).unwrap();
    assert_eq!(json, "{}");

    // A record written before the guarantee vocabulary was retired still
    // loads; the key carried no behaviour the engine still distinguishes.
    let legacy = r#"{
          "version": 2,
          "destination_mem": "m",
          "prune": { "guarantee": "never-clobber" },
          "operations": { "build": { "mode": "discovery", "trigger": "loop", "batch_size": 20 } }
        }"#;
    let b: Binding = serde_json::from_str(legacy).unwrap();
    assert_eq!(b.prune, Some(PruneConfig::default()));
}

/// The `prune` policy never changes `hash(D)` (it is a maintenance
/// policy, excluded like the sync/verify blocks).
#[test]
fn prune_does_not_change_the_hash() {
    let base = hash_binding(&binding());
    let mut pruned = binding();
    pruned.prune = Some(PruneConfig::default());
    assert_eq!(
        base,
        hash_binding(&pruned),
        "prune policy is excluded from hash(D)"
    );
}

/// A `prune` block validates clean over every medium, `web` included:
/// prune proposes and never merges, so no medium capability gates it.
#[test]
fn prune_validates_clean_over_every_medium() {
    let mut nc = binding();
    nc.prune = Some(PruneConfig::default());
    assert!(validate_binding(&nc).is_ok());

    let mut cf = binding();
    cf.operations.sync = None;
    cf.operations.verify = None;
    cf.deny_paths.clear();
    cf.prune = Some(PruneConfig::default());
    cf.sources = vec![source(
        "web-source",
        MediumType::Web,
        "https://example.com",
        vec![],
        None,
        None,
    )];
    assert!(validate_binding(&cf).is_ok());
}

/// A `web` binding scaffolded build-only (no sync/verify, no deny, no prep)
/// validates clean — the matrix-filtered default.
#[test]
fn web_build_only_validates_clean() {
    let mut b = binding();
    b.operations.sync = None;
    b.operations.verify = None;
    b.deny_paths.clear();
    b.sources = vec![source(
        "web-source",
        MediumType::Web,
        "https://example.com",
        vec![],
        None,
        None,
    )];
    assert!(validate_binding(&b).is_ok());
}

// ---- coverage semantics: resolution / refusal / hash stability ------

/// A clean web source carries NO scope: the medium has no scope
/// vocabulary, so any rule on it is uninterpretable and refuses. This
/// helper used to hand out `**/*` — which made every web fixture carry a
/// decorative rule, and is why the defect went unnoticed here.
fn web_source(name: &str) -> Source {
    source(
        name,
        MediumType::Web,
        "https://example.test",
        vec![],
        None,
        None,
    )
}

/// Resolution: an undeclared field resolves per binding — all
/// sources enumerable → exhaustive; at least one non-enumerable
/// source → curated (a mixed binding claims the weaker of its
/// parts). An explicit `curated` validates over any medium and
/// resolves to curated, declared.
#[test]
fn coverage_resolves_per_medium_when_undeclared() {
    let enumerable = binding();
    assert_eq!(enumerable.coverage_semantics, None);
    let eff = effective_coverage_semantics(&enumerable);
    assert_eq!(eff.value, CoverageSemantics::Exhaustive);
    assert!(!eff.declared, "resolved, not declared");
    validate_binding(&enumerable).expect("undeclared over enumerable validates");

    // Mixed: one enumerable + one web source → curated.
    let mut mixed = binding();
    mixed.sources.push(web_source("front"));
    // web has no change signal — drop sync/verify so only coverage
    // resolution is under test.
    mixed.operations.sync = None;
    mixed.operations.verify = None;
    mixed.deny_paths.clear();
    let eff = effective_coverage_semantics(&mixed);
    assert_eq!(eff.value, CoverageSemantics::Curated);
    assert!(!eff.declared);
    validate_binding(&mixed).expect("undeclared over web validates (resolves, never refuses)");

    // Explicit curated over any medium: validates, declared.
    let mut curated = mixed.clone();
    curated.coverage_semantics = Some(CoverageSemantics::Curated);
    validate_binding(&curated).expect("explicit curated validates over any medium");
    let eff = effective_coverage_semantics(&curated);
    assert_eq!(eff.value, CoverageSemantics::Curated);
    assert!(eff.declared);
}

/// Refusal: an explicit `exhaustive` with at least one
/// non-enumerable source refuses, naming the source, the medium,
/// and `curated` as the remedy — alongside other refusals of the
/// same binding, not replacing them. Complements: a binding whose
/// ONLY problem is this one still reports it; an explicit
/// `exhaustive` over enumerable sources is NOT refused.
#[test]
fn explicit_exhaustive_over_non_enumerable_refuses() {
    // Only-problem case: clean web binding, explicit exhaustive.
    let mut only = binding();
    only.sources = vec![web_source("front")];
    only.operations.sync = None;
    only.operations.verify = None;
    only.deny_paths.clear();
    only.coverage_semantics = Some(CoverageSemantics::Exhaustive);
    let errs = validate_binding(&only).expect_err("must refuse");
    assert_eq!(errs.len(), 1, "only this refusal: {errs:?}");
    match &errs[0] {
        CapabilityError::CoverageExhaustiveUnsupported {
            source_name,
            medium_type,
        } => {
            assert_eq!(source_name, "front");
            assert_eq!(medium_type, "web");
        }
        other => panic!("expected CoverageExhaustiveUnsupported, got {other:?}"),
    }
    let msg = errs[0].to_string();
    assert!(
        msg.contains("'front'") && msg.contains("'web'") && msg.contains("curated"),
        "refusal names source, medium, and the curated remedy: {msg}"
    );

    // Alongside other refusals: keep sync declared (web has no change
    // signal) — both refusals must be reported together.
    let mut multi = binding();
    multi.sources = vec![web_source("front")];
    multi.operations.verify = None;
    multi.deny_paths.clear();
    multi.coverage_semantics = Some(CoverageSemantics::Exhaustive);
    assert!(multi.operations.sync.is_some(), "fixture declares sync");
    let errs = validate_binding(&multi).expect_err("must refuse");
    assert!(
        errs.iter()
            .any(|e| matches!(e, CapabilityError::CoverageExhaustiveUnsupported { .. })),
        "coverage refusal present: {errs:?}"
    );
    assert!(
        errs.iter()
            .any(|e| matches!(e, CapabilityError::OperationOutOfScope { .. })),
        "reported alongside the sync refusal, not replacing it: {errs:?}"
    );

    // Complement: explicit exhaustive over enumerable is NOT refused.
    let mut ok = binding();
    ok.coverage_semantics = Some(CoverageSemantics::Exhaustive);
    validate_binding(&ok).expect("explicit exhaustive over enumerable validates");
}

/// Hash stability: the hash serialises the RESOLVED value, never
/// the `Option`. Over enumerable sources, an undeclared field
/// hashes byte-identically to an explicit `exhaustive` (== the
/// pre-optionality bytes, whose serialized projection was the
/// same `"exhaustive"` value). Over a non-enumerable source, an
/// undeclared field hashes identically to an explicit `curated`
/// (the moved-once, stable-thereafter hash) and differently from
/// the enumerable case's resolution.
#[test]
fn hash_serialises_the_resolved_coverage_value() {
    // Enumerable: None == Some(Exhaustive), byte-for-byte.
    let undeclared = binding();
    let mut declared = binding();
    declared.coverage_semantics = Some(CoverageSemantics::Exhaustive);
    assert_eq!(
        hash_binding(&undeclared),
        hash_binding(&declared),
        "undeclared over enumerable keeps the pre-optionality hash"
    );
    // ...and an explicit curated moves it (a genuine coverage change).
    let mut curated = binding();
    curated.coverage_semantics = Some(CoverageSemantics::Curated);
    assert_ne!(hash_binding(&undeclared), hash_binding(&curated));

    // Non-enumerable: None == Some(Curated) — the one-time move is
    // to the curated hash, stable thereafter.
    let mut web_undeclared = binding();
    web_undeclared.sources = vec![web_source("front")];
    let mut web_curated = web_undeclared.clone();
    web_curated.coverage_semantics = Some(CoverageSemantics::Curated);
    assert_eq!(
        hash_binding(&web_undeclared),
        hash_binding(&web_curated),
        "undeclared over web resolves (and hashes) as curated"
    );
}
