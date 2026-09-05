//! The due axis reads an engine-stamped timestamp with an offset and a
//! roster gate, and refuses the malformed shapes at load.

use memstead_schema::loader::load_schema_from_dir;
use tempfile::TempDir;

const MANIFEST: &str = "name: rosters\nversion: 0.1.0\ndescription: fixture\nwhen_to_use: tests\ntypes:\n  - bundle\nrelationships:\n  mode: strict\n  definitions:\n    - name: PART_OF\n      description: membership\n      default_weight: 3.0\n    - name: _default\n      description: fallback\n      default_weight: 1.0\ncommunity:\n  resolution: 1.0\n  seed: 42\n";

fn bundle_with(due: &str) -> String {
    format!(
        "name: bundle\ndescription: a campaign\nwhen_to_use: tests\nsections:\n  - key: purpose\n    heading: Purpose\n    required: true\n    search_weight: 10.0\n    catch_all: true\n    write_rules: []\nmetadata_fields:\n  - key: status\n    description: s\n    field_type: string\n    default_value: draft\n    enum_values: [draft, complete]\n{due}title_weight: 100.0\ntext_fields: [purpose]\nhierarchy_relationship: PART_OF\nno_self_loop_relationships: [PART_OF]\nupdatable_fields: [title, purpose, status]\nhealth_required_fields: [purpose]\nstaleness_threshold_days: 180\nwrite_rules: []\n"
    )
}

fn load(due: &str) -> Result<memstead_schema::Schema, String> {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("types")).unwrap();
    std::fs::write(tmp.path().join("schema.yaml"), MANIFEST).unwrap();
    std::fs::write(tmp.path().join("types/bundle.yaml"), bundle_with(due)).unwrap();
    load_schema_from_dir(tmp.path()).map_err(|e| e.to_string())
}

#[test]
fn engine_stamped_date_field_offset_and_gate_load() {
    let schema = load(
        "due:\n  date_field: last_modified\n  offset_days: 14\n  status_field: status\n  open_values: [complete]\n  unless_open_via:\n    relationships: [PART_OF]\n    direction: in\n    status_field: status\n    open_values: [draft]\n",
    )
    .unwrap();
    let due = schema.types["bundle"].due.as_ref().unwrap();
    assert_eq!(due.date_field, "last_modified");
    assert_eq!(due.offset_days, 14);
    let gate = due.unless_open_via.as_ref().unwrap();
    assert_eq!(gate.relationships, vec!["PART_OF".to_string()]);
    assert_eq!(gate.open_values, vec!["draft".to_string()]);
    // Without an offset the field defaults to zero and a plain date axis
    // keeps loading exactly as before.
    let plain = load(
        "due:\n  date_field: created_date\n  status_field: status\n  open_values: [complete]\n",
    )
    .unwrap();
    assert_eq!(plain.types["bundle"].due.as_ref().unwrap().offset_days, 0);
}

#[test]
fn malformed_due_shapes_refuse_at_load() {
    let undeclared = load(
        "due:\n  date_field: finished_on\n  status_field: status\n  open_values: [complete]\n",
    )
    .unwrap_err();
    assert!(undeclared.contains("finished_on"), "{undeclared}");
    let no_rel = load(
        "due:\n  date_field: last_modified\n  status_field: status\n  open_values: [complete]\n  unless_open_via:\n    relationships: []\n    direction: in\n    status_field: status\n    open_values: [draft]\n",
    )
    .unwrap_err();
    assert!(no_rel.contains("unless_open_via.relationships"), "{no_rel}");
    let unknown_rel = load(
        "due:\n  date_field: last_modified\n  status_field: status\n  open_values: [complete]\n  unless_open_via:\n    relationships: [OWNS]\n    direction: in\n    status_field: status\n    open_values: [draft]\n",
    )
    .unwrap_err();
    assert!(unknown_rel.contains("OWNS"), "{unknown_rel}");
    let no_open = load(
        "due:\n  date_field: last_modified\n  status_field: status\n  open_values: [complete]\n  unless_open_via:\n    relationships: [PART_OF]\n    direction: in\n    status_field: status\n    open_values: []\n",
    )
    .unwrap_err();
    assert!(no_open.contains("unless_open_via.open_values"), "{no_open}");
    let unknown_key = load(
        "due:\n  date_field: last_modified\n  status_field: status\n  open_values: [complete]\n  grace_days: 3\n",
    );
    assert!(unknown_key.is_err(), "an undeclared due key refuses");
}
