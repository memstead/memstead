#![cfg(test)]

use super::*;
use serde_json::json;

// --- check_config tests ---

fn minimal_valid_config() -> Value {
    json!({
        "schema": "default@1.0.0"
    })
}

#[test]
fn check_valid_minimal_config() {
    let result = check_config(&minimal_valid_config());
    assert!(result.valid, "errors: {:?}", result.errors);
}

/// The workspace mem config's `format` matches every sibling store:
/// absent means version 1 (the healthy common case — real mems carry no
/// key), the current version parses, and an UNKNOWN version refuses
/// loudly at parse instead of being silently dropped by serde. Before
/// this gate, `"format": 99` on a mem config was ignored end to end:
/// `projection verify` returned clean and wrote a `#verified` baseline
/// over a config a future engine may mean differently.
#[test]
fn mem_config_format_absent_is_v1_and_unknown_refuses() {
    // Absent: parses, reads as version 1, stays off the wire.
    let cfg: MemConfig = serde_json::from_value(minimal_valid_config()).unwrap();
    assert_eq!(cfg.format, None);
    assert_eq!(cfg.format_version(), MEM_CONFIG_FORMAT);
    let wire = serde_json::to_value(&cfg).unwrap();
    assert!(wire.get("format").is_none(), "absent format stays absent");

    // Current version: parses.
    let mut v = minimal_valid_config();
    v["format"] = json!(MEM_CONFIG_FORMAT);
    let cfg: MemConfig = serde_json::from_value(v).unwrap();
    assert_eq!(cfg.format_version(), MEM_CONFIG_FORMAT);

    // Unknown version: refuses at parse, naming both versions.
    let mut v = minimal_valid_config();
    v["format"] = json!(99);
    let err = parse_mem_config(&v).expect_err("format 99 must refuse, never parse");
    let msg = err.to_string();
    assert!(
        msg.contains("99") && msg.contains('1'),
        "the refusal names the found and the supported version: {msg}"
    );

    // check_config carries the same verdict for the validation surface.
    let mut v = minimal_valid_config();
    v["format"] = json!(99);
    let result = check_config(&v);
    assert!(!result.valid, "check_config must refuse an unknown format");
    assert!(
        result.errors.iter().any(|e| e.contains("format")),
        "the error names the field: {:?}",
        result.errors
    );
}

/// The in-config `name` field is optional — configs without a
/// `name` key are valid; the leaf folder name under
/// `__MEMSTEAD:mems/` (or the disk basename on the legacy disk
/// path) is the authoritative identifier instead.
#[test]
fn check_missing_name_now_valid() {
    let config = json!({"schema": "default@1.0.0"});
    let result = check_config(&config);
    assert!(result.valid, "errors: {:?}", result.errors);
}

/// `parse_mem_config` produces a `MemConfig` whose `name` is
/// `None` when the on-disk config omits the field. Pins the
/// Goal 3 wire-shape contract.
#[test]
fn parse_mem_config_name_none_when_field_absent() {
    let config = json!({"schema": "default@1.0.0"});
    let parsed = parse_mem_config(&config).expect("name-less config parses");
    assert!(parsed.name.is_none());
}

/// Round-trip: a `MemConfig` whose `name` is `None` serialises
/// without the `name` key (skip-if-none on the serde attribute).
/// Pins the on-disk minimisation contract.
#[test]
fn mem_config_omits_name_when_none_on_serialize() {
    let cfg = MemConfig {
        format: None,
        name: None,
        title: None,
        subject: None,
        version: None,
        description: None,
        authors: None,
        process_mem: None,
        schema: Some(SchemaRef::new("default", semver::Version::new(1, 0, 0))),
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
        mutation_stamp: None,
        extra: Default::default(),
    };
    let json = serde_json::to_string(&cfg).unwrap();
    assert!(
        !json.contains("\"name\""),
        "serialized config must omit `name` when None, got: {json}"
    );
}

/// A stray `name` field is rejected with `LEGACY_FIELD_PRESENT`
/// regardless of its value (empty or non-empty). Both empty and
/// non-empty shapes collapse onto the legacy tombstone reject.
#[test]
fn check_legacy_name_field_rejected() {
    for value in [json!(""), json!("@test/mem")] {
        let config = json!({"name": value, "schema": "default@1.0.0"});
        let result = check_config(&config);
        assert!(!result.valid, "name={value}: expected reject");
        assert_eq!(
            result.error_code.as_deref(),
            Some("LEGACY_FIELD_PRESENT"),
            "name={value}: expected LEGACY_FIELD_PRESENT envelope"
        );
        assert!(
            result.errors.iter().any(|e| e.contains("Legacy `name`")),
            "name={value}: errors {:?}",
            result.errors
        );
    }
}

#[test]
fn check_missing_schema() {
    let config = json!({});
    let result = check_config(&config);
    assert!(!result.valid);
    assert!(result.errors.iter().any(|e| e.contains("`schema`")));
}

#[test]
fn check_legacy_types_array_rejected() {
    let config = json!({"types": ["spec"], "schema": "default@1.0.0"});
    let result = check_config(&config);
    assert!(!result.valid);
    assert!(result.errors.iter().any(|e| e.contains("Legacy `types:")));
    assert_eq!(
        result.error_code.as_deref(),
        Some("LEGACY_FIELD_PRESENT"),
        "expected LEGACY_FIELD_PRESENT envelope for legacy `types`"
    );
}

#[test]
fn check_schema_wrong_shape() {
    let config = json!({"schema": ["default@1.0.0"]});
    let result = check_config(&config);
    assert!(!result.valid);
}

#[test]
fn check_schema_bare_name_rejected() {
    // Bare-name pins are rejected at load — every mem config must
    // declare an exact `<name>@<version>` pin so cross-mem link
    // matching and archive identity are unambiguous.
    let config = json!({"schema": "default"});
    let result = check_config(&config);
    assert!(!result.valid, "expected bare-name pin to be rejected");
    assert!(
        result.errors.iter().any(|e| e.contains("schema")),
        "errors: {:?}",
        result.errors
    );
}

#[test]
fn check_schema_range_syntax_rejected() {
    for s in [
        "default@^1.0.0",
        "default@~1.0.0",
        "default@latest",
        "default@>=1.0.0",
    ] {
        let config = json!({"schema": s});
        let result = check_config(&config);
        assert!(!result.valid, "expected '{s}' to be rejected");
    }
}

#[test]
fn check_schema_valid_exact_pin() {
    let config = json!({"schema": "default@1.0.0"});
    let result = check_config(&config);
    assert!(result.valid, "errors: {:?}", result.errors);
}

#[test]
fn schema_pin_versioned_parses() {
    let pin: SchemaRef = "software@1.2.3".parse().unwrap();
    assert_eq!(pin.name, "software");
    assert_eq!(pin.version, semver::Version::new(1, 2, 3));
    assert_eq!(pin.as_display(), "software@1.2.3");
}

#[test]
fn schema_pin_bare_name_rejected() {
    // Bare-name pins are rejected at parse — agents must declare the
    // exact version. Bogus name shapes (uppercase, slash, empty) fall
    // through the same gate.
    for bad in ["software", "Default", "foo/bar", "", "  "] {
        assert!(
            bad.parse::<SchemaRef>().is_err(),
            "expected '{bad}' to be rejected"
        );
    }
}

#[test]
fn schema_pin_serde_round_trip() {
    let versioned: SchemaRef = serde_json::from_str(r#""software@1.0.0""#).unwrap();
    assert_eq!(versioned.as_display(), "software@1.0.0");
    let as_json = serde_json::to_string(&versioned).unwrap();
    assert_eq!(as_json, r#""software@1.0.0""#);
}

#[test]
fn publish_rejects_missing_schema() {
    // Archives record a concrete schema version; a config without a
    // `schema` field cannot be published.
    let json = json!({ "version": "1.0.0" });
    let config = parse_mem_config(&json).unwrap();
    let err = published_config_from(&config, "demo").unwrap_err();
    assert!(matches!(err, PublishConversionError::MissingSchema));
}

#[test]
fn publish_accepts_versioned_pin() {
    let json = json!({
        "version": "1.0.0",
        "schema": "software@2.3.4"
    });
    let config = parse_mem_config(&json).unwrap();
    let published = published_config_from(&config, "demo").expect("versioned pin publishes");
    assert_eq!(published.name, "demo");
    assert_eq!(published.schema.name, "software");
    assert_eq!(published.schema.version, semver::Version::new(2, 3, 4));
}

#[test]
fn check_legacy_default_schema_field_is_ignored() {
    // `defaultSchema` was an author-only tombstone field pre-2026-04.
    // It's captured into `extra` and surfaces as an unknown-field
    // warning without invalidating an otherwise well-formed config.
    let config = json!({
        "schema": "default@1.0.0",
        "defaultSchema": "spec"
    });
    let result = check_config(&config);
    assert!(result.valid, "errors: {:?}", result.errors);
}

#[test]
fn config_preserves_unknown_fields_on_roundtrip() {
    // Any unknown top-level field (legacy `defaultSchema`, future
    // fields, typos) is captured into `extra` and re-emitted
    // unchanged — guarantees no silent data loss on read-modify-write.
    let raw = json!({
        "schema": "default@1.0.0",
        "defaultSchema": "concept"
    });
    let cfg: MemConfig = serde_json::from_value(raw).expect("config deserialized");
    assert!(
        cfg.extra.contains_key("defaultSchema"),
        "legacy field should be preserved in extra: {:?}",
        cfg.extra
    );

    let reserialized = serde_json::to_value(&cfg).expect("config reserialized");
    assert_eq!(
        reserialized.get("defaultSchema").and_then(|v| v.as_str()),
        Some("concept"),
        "round-trip should preserve the legacy field"
    );
}

#[test]
fn check_unknown_keys_warned() {
    let config = json!({
        "schema": "default@1.0.0",
        "unknownKey": "value"
    });
    let result = check_config(&config);
    assert!(result.valid);
    assert!(result.warnings.iter().any(|w| w.contains("unknownKey")));
}

/// Tombstone keys produce a hard error, not a soft "unknown key"
/// warning. The unknown-key sweep must skip them so callers see
/// exactly one signal per legacy key.
#[test]
fn legacy_tombstone_does_not_double_warn() {
    let config = json!({ "name": "x", "schema": "default@1.0.0" });
    let result = check_config(&config);
    assert!(!result.valid);
    let unknown_warning = result
        .warnings
        .iter()
        .any(|w| w.contains("Unknown config key 'name'"));
    assert!(
        !unknown_warning,
        "legacy tombstone must not also surface as unknown-key warning: {:?}",
        result.warnings
    );
}

// --- MemConfig slimdown ---

#[test]
fn slim_config_with_only_retained_core_fields_loads() {
    // The post-slimdown engine reads a minimal config carrying only
    // the fields it actually uses. `schema`, `vcs`, `writeGuidance`
    // are the intent-level retained set; serde-required collection
    // defaults fill the rest. The mem leaf identity is path-derived
    // (Goal 3 of mem-repo-restructure) so the in-config `name`
    // field is now a tombstone (Goal 10). Cross-mem authorization
    // moved to `.memstead/workspace.toml`'s `[cross_mem_links]` section.
    let raw = json!({
        "schema": "default@1.0.0",
        "writeGuidance": {
            "style": "structured",
            "audience": "agent"
        },
        "vcs": { "gitdir": ".git", "worktree": "." }
    });
    let check = check_config(&raw);
    assert!(check.valid, "errors: {:?}", check.errors);
    let parsed = parse_mem_config(&raw).expect("slim config parses");
    assert!(parsed.name.is_none());
    assert_eq!(parsed.write_guidance.len(), 2);
    assert_eq!(
        parsed.write_guidance.get("style").and_then(|v| v.as_str()),
        Some("structured")
    );
    assert!(parsed.vcs.is_some());
    assert!(parsed.extra.is_empty());
}

#[test]
fn legacy_projections_block_lands_in_extra_without_error() {
    // Pre-rewrite configs carrying `projections` / `mediums` blocks
    // are no longer interpreted by the engine, but round-tripping
    // them must not fail — the blocks fall into `MemConfig.extra`
    // so read-modify-write preserves authorship. check_config emits
    // a "Unknown config key" warning per unrecognised top-level key.
    let raw = json!({
        "schema": "default@1.0.0",
        "mediums": {
            "codebase": {
                "type": "codebase",
                "scope": { "tree": [{ "path": "src/", "mode": "allow" }] }
            }
        },
        "projections": {
            "p1": {
                "intent": "test",
                "sources": [{ "medium_ref": "codebase" }],
                "destination": { "medium_ref": "graph" }
            }
        }
    });
    let check = check_config(&raw);
    assert!(
        check.valid,
        "legacy projections/mediums must load without errors: {:?}",
        check.errors
    );
    let projection_warned = check.warnings.iter().any(|w| w.contains("projections"));
    let mediums_warned = check.warnings.iter().any(|w| w.contains("mediums"));
    assert!(
        projection_warned && mediums_warned,
        "unknown-key warnings expected for projections and mediums: {:?}",
        check.warnings
    );

    let parsed = parse_mem_config(&raw).expect("legacy config parses");
    assert!(
        parsed.extra.contains_key("projections"),
        "legacy `projections` must land in extra: {:?}",
        parsed.extra.keys().collect::<Vec<_>>()
    );
    assert!(
        parsed.extra.contains_key("mediums"),
        "legacy `mediums` must land in extra: {:?}",
        parsed.extra.keys().collect::<Vec<_>>()
    );
}

#[test]
fn write_guidance_round_trips_as_string_map() {
    // `writeGuidance` is an opaque `HashMap<String, Value>` now.
    // Round-trip a map with string / array / object / number values
    // to confirm every JSON shape survives verbatim — the engine
    // must not interpret or normalise its contents.
    let raw = json!({
        "schema": "default@1.0.0",
        "writeGuidance": {
            "style": "structured",
            "patterns": ["extract", "summarise"],
            "nested": { "depth": 2, "flag": true },
            "count": 42
        }
    });
    let parsed = parse_mem_config(&raw).expect("config parses");
    assert_eq!(parsed.write_guidance.len(), 4);
    assert_eq!(
        parsed.write_guidance.get("style").and_then(|v| v.as_str()),
        Some("structured")
    );
    let wire = serde_json::to_value(&parsed).expect("reserialize");
    let guidance = wire
        .get("writeGuidance")
        .and_then(|v| v.as_object())
        .expect("writeGuidance present in wire form");
    assert_eq!(guidance.len(), 4);
    assert_eq!(
        guidance.get("style").and_then(|v| v.as_str()),
        Some("structured")
    );
    assert_eq!(
        guidance
            .get("patterns")
            .and_then(|v| v.as_array())
            .map(|a| a.len()),
        Some(2)
    );
    assert_eq!(
        guidance
            .get("nested")
            .and_then(|v| v.get("depth"))
            .and_then(|v| v.as_u64()),
        Some(2)
    );
}

#[test]
fn write_guidance_empty_map_omits_from_wire() {
    // `skip_serializing_if = "HashMap::is_empty"` keeps an unset
    // writeGuidance off the wire entirely so existing minimal
    // configs don't gain an empty `{}` after a round-trip.
    let parsed: MemConfig = serde_json::from_value(minimal_valid_config()).unwrap();
    assert!(parsed.write_guidance.is_empty());
    let wire = serde_json::to_value(&parsed).unwrap();
    assert!(
        wire.get("writeGuidance").is_none(),
        "empty writeGuidance must be omitted from the wire: {wire}"
    );
}

#[test]
fn published_config_strips_extra_and_write_guidance() {
    // `PublishedMemConfig` uses `deny_unknown_fields` with a
    // fixed whitelist — the catchall `extra` and the pass-through
    // `writeGuidance` both fall off the projection. This guards
    // against a future reviewer adding either to the whitelist by
    // mistake.
    let mut extra = HashMap::new();
    extra.insert(
        "projections".to_string(),
        json!({ "p1": { "intent": "x" } }),
    );
    let mut guidance = HashMap::new();
    guidance.insert("style".to_string(), json!("structured"));
    let mut sync_state = BTreeMap::new();
    sync_state.insert(
        "engine-graph/source-files".to_string(),
        "deadbeef".to_string(),
    );
    let cfg = MemConfig {
        format: None,
        name: Some("demo".to_string()),
        title: None,
        subject: None,
        version: Some(semver::Version::new(0, 1, 0)),
        description: None,
        authors: None,
        process_mem: None,
        schema: Some("default@1.0.0".parse().unwrap()),
        write_guidance: guidance,
        rules: None,
        publish: None,
        language: None,
        read_mems: BTreeMap::new(),
        community: None,
        vcs: None,
        unregistered_at: None,
        sync_state,
        review_mark: None,
        mutation_stamp: None,
        extra,
    };
    let published = published_config_from(&cfg, "").expect("publish projection");
    let wire = serde_json::to_value(&published).expect("serialize");
    assert!(
        wire.get("projections").is_none(),
        "extra must not leak into published wire: {wire}"
    );
    assert!(
        wire.get("writeGuidance").is_none(),
        "writeGuidance must not leak into published wire: {wire}"
    );
    assert!(
        wire.get("syncState").is_none(),
        "syncState must not leak into published wire: {wire}"
    );
}

// --- legacy tombstones — kept to lock behaviour after slimdown ---

// --- migration tests ---

// --- flatten tests ---

// --- shadow detection tests ---

// --- CRUD dry run tests ---

#[test]
fn update_config_field_protected() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = tmp.path().join("config.json");

    let mut config = minimal_valid_config();
    let err =
        update_config_field(&config_path, &mut config, "name", json!("new"), true).unwrap_err();
    assert!(err.to_string().contains("protected"));
}

#[test]
fn update_config_field_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = tmp.path().join("config.json");

    let mut config = minimal_valid_config();
    let err = update_config_field(&config_path, &mut config, "banana", json!("yellow"), true)
        .unwrap_err();
    assert!(err.to_string().contains("not a recognized"));
}

#[test]
fn update_config_field_allowed() {
    let tmp = tempfile::tempdir().unwrap();
    let config_path = tmp.path().join("config.json");

    let mut config = minimal_valid_config();
    let result =
        update_config_field(&config_path, &mut config, "language", json!("en"), true).unwrap();
    assert!(result.valid, "errors: {:?}", result.errors);
    assert_eq!(config["language"], "en");
}

// --- is_encompassed_by tests ---

// --- Graph medium scope validation ---

// --- version parsing (semver) ---

#[test]
fn parse_accepts_valid_semver_version() {
    let cfg = json!({
        "schema": "default@1.0.0",
        "version": "1.2.3-beta.4"
    });
    let parsed = parse_mem_config(&cfg).expect("valid semver should parse");
    let v = parsed.version.expect("version present");
    assert_eq!(v.major, 1);
    assert_eq!(v.minor, 2);
    assert_eq!(v.patch, 3);
    assert!(!v.pre.is_empty());
}

#[test]
fn parse_rejects_invalid_semver_version() {
    // "1.2" is not valid semver — must be MAJOR.MINOR.PATCH.
    let cfg = json!({
        "schema": "default@1.0.0",
        "version": "1.2"
    });
    let err = parse_mem_config(&cfg).expect_err("invalid semver must fail at parse");
    let msg = format!("{err}");
    assert!(
        msg.contains("version"),
        "error should mention version: {msg}"
    );
}

#[test]
fn parse_rejects_non_semver_garbage_version() {
    let cfg = json!({
        "schema": "default@1.0.0",
        "version": "potato"
    });
    let err = parse_mem_config(&cfg).expect_err("garbage must fail at parse");
    let msg = format!("{err}");
    assert!(
        msg.contains("version"),
        "error should mention version: {msg}"
    );
}

// --- readMems: `{ source: { type, … } }` entries — no path or
//     version fields (the cached archive's config is authoritative) ---

#[test]
fn parse_accepts_read_mems_with_local_source() {
    let cfg = json!({
        "schema": "default@1.0.0",
        "readMems": {
            "internal-notes": { "source": { "type": "local" } }
        }
    });
    let check = check_config(&cfg);
    assert!(check.valid, "errors: {:?}", check.errors);
    let parsed = parse_mem_config(&cfg).expect("valid readMems must parse");
    let spec = parsed
        .read_mems
        .get("internal-notes")
        .expect("entry present");
    assert!(matches!(spec.source, ReadMemSource::Local));
}

#[test]
fn parse_accepts_read_mems_with_url_source() {
    let cfg = json!({
        "schema": "default@1.0.0",
        "readMems": {
            "aws-patterns": {
                "source": {
                    "type": "url",
                    "url": "https://example.com/aws-patterns.mem"
                }
            }
        }
    });
    let check = check_config(&cfg);
    assert!(check.valid, "errors: {:?}", check.errors);
    let parsed = parse_mem_config(&cfg).expect("valid readMems must parse");
    let spec = parsed.read_mems.get("aws-patterns").expect("entry present");
    match &spec.source {
        ReadMemSource::Url { url } => {
            assert_eq!(url, "https://example.com/aws-patterns.mem")
        }
        _ => panic!("expected Url source, got {:?}", spec.source),
    }
}

#[test]
fn parse_accepts_empty_read_mems_map() {
    let cfg = json!({
        "schema": "default@1.0.0",
        "readMems": {}
    });
    let parsed = parse_mem_config(&cfg).expect("empty readMems must parse");
    assert!(parsed.read_mems.is_empty());
}

#[test]
fn parse_accepts_omitted_read_mems() {
    let cfg = json!({
        "schema": "default@1.0.0"
    });
    let parsed = parse_mem_config(&cfg).expect("omitted readMems must parse");
    assert!(parsed.read_mems.is_empty());
}

#[test]
fn check_rejects_read_mem_without_source() {
    let cfg = json!({
        "schema": "default@1.0.0",
        "readMems": { "p": {} }
    });
    let check = check_config(&cfg);
    assert!(!check.valid);
    assert!(
        check.errors.iter().any(|e| e.contains("source")),
        "errors: {:?}",
        check.errors
    );
}

#[test]
fn check_rejects_read_mem_with_unknown_source_type() {
    let cfg = json!({
        "schema": "default@1.0.0",
        "readMems": {
            "p": { "source": { "type": "ftp", "url": "ftp://..." } }
        }
    });
    let check = check_config(&cfg);
    assert!(!check.valid);
    assert!(
        check
            .errors
            .iter()
            .any(|e| e.contains("unknown source type")),
        "errors: {:?}",
        check.errors
    );
}

#[test]
fn check_rejects_url_source_with_empty_url() {
    let cfg = json!({
        "schema": "default@1.0.0",
        "readMems": {
            "p": { "source": { "type": "url", "url": "" } }
        }
    });
    let check = check_config(&cfg);
    assert!(!check.valid);
    assert!(
        check.errors.iter().any(|e| e.contains("url source")),
        "errors: {:?}",
        check.errors
    );
}

#[test]
fn check_rejects_registry_source_type_reserved_for_phase_d() {
    let cfg = json!({
        "schema": "default@1.0.0",
        "readMems": {
            "p": { "source": { "type": "registry" } }
        }
    });
    let check = check_config(&cfg);
    // `registry` is a reserved future source type but is not yet
    // accepted by the schema — validation must reject it until the
    // registry ships.
    assert!(!check.valid);
    assert!(
        check
            .errors
            .iter()
            .any(|e| e.contains("unknown source type")),
        "errors: {:?}",
        check.errors
    );
}

/// Guards the `BTreeMap` choice: serialized read_mems must come out
/// in key-sorted order regardless of insertion order, so config files
/// on disk and log output are reproducible. A future "optimisation"
/// that reintroduces `HashMap` would break this.
#[test]
fn read_mems_serialization_order_is_key_sorted() {
    let cfg = json!({
        "schema": "default@1.0.0",
        "readMems": {
            "zebra": { "source": { "type": "local" } },
            "alpha": { "source": { "type": "local" } },
            "mango": { "source": { "type": "local" } }
        }
    });
    let parsed = parse_mem_config(&cfg).expect("valid config must parse");
    let reserialized = serde_json::to_string(&parsed).expect("serialization must succeed");
    let alpha = reserialized.find("alpha").expect("alpha present");
    let mango = reserialized.find("mango").expect("mango present");
    let zebra = reserialized.find("zebra").expect("zebra present");
    assert!(
        alpha < mango && mango < zebra,
        "expected alpha < mango < zebra, got: {reserialized}"
    );
}

// ----- vcs field -----

#[test]
fn vcs_config_round_trips_through_serde_with_both_fields() {
    let cfg = json!({
        "schema": "default@1.0.0",
        "vcs": { "gitdir": "../.git", "worktree": ".." }
    });
    let parsed = parse_mem_config(&cfg).expect("valid config must parse");
    let vcs = parsed.vcs.as_ref().expect("vcs must be Some");
    assert_eq!(vcs.gitdir, "../.git");
    assert_eq!(vcs.worktree, "..");

    // Round-trip.
    let reserialized = serde_json::to_value(&parsed).unwrap();
    let round = parse_mem_config(&reserialized).expect("round-trip parse");
    assert_eq!(round.vcs.as_ref().unwrap().gitdir, "../.git");
    assert_eq!(round.vcs.as_ref().unwrap().worktree, "..");
}

#[test]
fn vcs_config_worktree_defaults_to_dot_when_omitted() {
    let cfg = json!({
        "schema": "default@1.0.0",
        "vcs": { "gitdir": ".git" }
    });
    let parsed = parse_mem_config(&cfg).expect("valid config must parse");
    let vcs = parsed.vcs.as_ref().expect("vcs must be Some");
    assert_eq!(vcs.gitdir, ".git");
    assert_eq!(vcs.worktree, ".", "worktree must default to \".\"");
}

#[test]
fn vcs_config_absent_is_none() {
    let cfg = json!({ "schema": "default@1.0.0"  });
    let parsed = parse_mem_config(&cfg).expect("valid config must parse");
    assert!(parsed.vcs.is_none(), "missing vcs must deserialize to None");
}

#[test]
fn vcs_field_tolerates_legacy_string_value() {
    // Legacy macOS-app sentinel. The tolerant deserializer must keep
    // the config loadable (returning None) without touching the
    // user's file by hand.
    let cfg = json!({
        "schema": "default@1.0.0",
        "vcs": "system"
    });
    let parsed = parse_mem_config(&cfg).expect("legacy vcs string must parse");
    assert!(
        parsed.vcs.is_none(),
        "legacy string must deserialize to None"
    );
}

#[test]
fn published_config_strips_vcs() {
    // `published_config_from` must drop the `vcs` block — VCS layout
    // is workspace-local mechanics, never part of the published
    // mem's identity. This is guaranteed by the whitelist
    // projection: `PublishedMemConfig` has no `vcs` field, so a
    // MemConfig carrying `vcs: Some(...)` projects to a
    // PublishedMemConfig with no `vcs` on the wire.
    let mut cfg = MemConfig {
        format: None,
        name: Some("demo".to_string()),
        title: None,
        subject: None,
        version: Some(semver::Version::new(0, 1, 0)),
        description: None,
        authors: None,
        process_mem: None,
        schema: Some("default@1.0.0".parse().unwrap()),
        write_guidance: HashMap::new(),
        rules: None,
        publish: None,
        language: None,
        read_mems: BTreeMap::new(),
        community: None,
        vcs: None,
        unregistered_at: None,
        sync_state: BTreeMap::new(),
        review_mark: None,
        mutation_stamp: None,
        extra: HashMap::new(),
    };
    cfg.vcs = Some(VcsConfig {
        gitdir: ".git".to_string(),
        worktree: ".".to_string(),
    });
    let published = published_config_from(&cfg, "").expect("valid projection");
    let wire = serde_json::to_value(&published).expect("serialize");
    assert!(
        wire.get("vcs").is_none(),
        "published wire form must not carry vcs: got {wire}"
    );
}

// ----- belongsTo legacy tombstone -----

/// `belongsTo` is now a tombstone — cross-mem authorization
/// migrated to the workspace-level `[cross_mem_links]` section in
/// `.memstead/workspace.toml`. A per-mem config blob carrying `belongsTo` is
/// rejected with `LEGACY_FIELD_PRESENT`.
#[test]
fn belongs_to_field_is_legacy_tombstone() {
    let cfg = json!({
        "schema": "default@1.0.0",
        "belongsTo": ["main"]
    });
    let result = check_config(&cfg);
    assert!(!result.valid, "belongsTo presence must fail validation");
    assert_eq!(result.error_code.as_deref(), Some("LEGACY_FIELD_PRESENT"));
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.contains("belongsTo") && e.contains("cross_mem_links")),
        "tombstone error must name the field and the replacement section: {:?}",
        result.errors
    );
}
