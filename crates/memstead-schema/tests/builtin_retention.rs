//! Built-in retention guard: a `(name, version)` once shipped as a
//! built-in exists in every future binary, with byte-identical
//! content.
//!
//! `builtins/MANIFEST.toml` is the append-only ledger of every
//! ever-shipped built-in version, each sealed with a content hash over
//! its package files. This test enforces the ledger in both
//! directions:
//!
//! - every manifest entry must exist in the compiled catalogue with a
//!   matching hash — deleting a shipped version's directory, or
//!   editing a shipped version's bytes in place, fails CI (the
//!   2026-08-06 `ingest` 0.1.0 → 0.2.0 in-place bump stranded a
//!   workspace for a day; behaviour changes are a NEW version
//!   directory, never an edit);
//! - every compiled catalogue entry must be listed in the manifest —
//!   shipping a new version requires exactly one appended entry and
//!   nothing more.
//!
//! To append a new entry, run this test and copy the `[[shipped]]`
//! block the failure message prints.

use sha2::{Digest, Sha256};

/// Canonical content hash of a package: SHA-256 over each file's
/// relative path and bytes, in path-sorted order, NUL-separated.
fn package_hash(files: &[(String, &'static [u8])]) -> String {
    let mut hasher = Sha256::new();
    for (path, bytes) in files {
        hasher.update(path.as_bytes());
        hasher.update([0u8]);
        hasher.update(bytes);
        hasher.update([0u8]);
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[derive(serde::Deserialize)]
struct Manifest {
    #[serde(default)]
    shipped: Vec<ShippedEntry>,
}

#[derive(serde::Deserialize)]
struct ShippedEntry {
    name: String,
    version: String,
    sha256: String,
}

fn load_manifest() -> Manifest {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/builtins/MANIFEST.toml");
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("builtins/MANIFEST.toml must exist: {e}"));
    toml::from_str(&text).expect("builtins/MANIFEST.toml must parse")
}

#[test]
fn every_shipped_builtin_is_retained_with_sealed_content() {
    let manifest = load_manifest();
    let packages = memstead_schema::builtins::builtin_packages();

    // Direction 1: every manifest entry present, bytes sealed.
    for entry in &manifest.shipped {
        let Some(pkg) = packages
            .iter()
            .find(|p| p.name == entry.name && p.version == entry.version)
        else {
            panic!(
                "shipped built-in {}@{} is ABSENT from the compiled catalogue — \
                 shipped versions are never removed; restore its directory under \
                 builtins/schemas/",
                entry.name, entry.version,
            );
        };
        let actual = package_hash(&pkg.files);
        assert_eq!(
            actual, entry.sha256,
            "shipped built-in {}@{} was edited in place — its content no longer \
             matches the sealed manifest hash. A shipped version's bytes are \
             sealed; behaviour changes are a NEW version directory.",
            entry.name, entry.version,
        );
    }

    // Direction 2: every catalogue entry listed — appending the new
    // `[[shipped]]` block(s) below is the whole ceremony for a new
    // version.
    let unlisted: Vec<String> = packages
        .iter()
        .filter(|p| {
            !manifest
                .shipped
                .iter()
                .any(|e| e.name == p.name && e.version == p.version)
        })
        .map(|p| {
            format!(
                "[[shipped]]\nname = \"{}\"\nversion = \"{}\"\nsha256 = \"{}\"\n",
                p.name,
                p.version,
                package_hash(&p.files),
            )
        })
        .collect();
    assert!(
        unlisted.is_empty(),
        "built-in version(s) not listed in builtins/MANIFEST.toml — append:\n\n{}",
        unlisted.join("\n"),
    );
}

/// The incident version: `ingest@0.1.0` (in-place-bumped away on
/// 2026-08-06, restored by this plan) resolves as a distinct schema
/// beside 0.2.0, with the recorded difference — 0.1.0 enumerates its
/// cross-mem destinations, 0.2.0 uses the wildcard — intact.
#[test]
fn ingest_versions_resolve_as_distinct_schemas() {
    let schemas = memstead_schema::builtins::load_builtin_schemas().expect("catalogue loads");
    let find = |version: &str| {
        schemas
            .iter()
            .find(|s| {
                let (name, v) = s.id();
                name == "ingest" && v.to_string() == version
            })
            .unwrap_or_else(|| panic!("ingest@{version} must be in the catalogue"))
    };
    let v01 = find("0.1.0");
    let v02 = find("0.2.0");
    let wildcarded = |s: &memstead_schema::Schema| {
        s.manifest
            .cross_mem_relationships
            .iter()
            .any(|e| e.to_schema == "*")
    };
    assert!(
        !wildcarded(v01),
        "ingest@0.1.0 predates the cross-mem wildcard and must not carry one"
    );
    assert!(
        wildcarded(v02),
        "ingest@0.2.0 carries the cross-mem wildcard destination"
    );
}
