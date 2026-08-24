//! Embedded builtin schemas, baked into the binary via `include_dir!`.
//!
//! Every directory under `builtins/schemas/` is loaded as a first-class schema
//! and registered in the default `SchemaRegistry`. Ships `default` (the legacy
//! 10-knowledge-type bundle) plus domain-specific schemas (`ingest`,
//! `planning`, `project`, `software`) that mems may pin via
//! `schema = "<name>@<version>"` in their per-mem config.

use std::sync::Arc;

use include_dir::{Dir, include_dir};

use crate::loader::{self, SchemaLoadError};
use crate::schema::Schema;

static BUILTIN_SCHEMAS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/builtins/schemas");

/// Access to the embedded `builtins/schemas` directory.
///
/// Exposed so the source-collection path
/// (`crate::source::collect_schema_source`) can lift raw YAML bytes out
/// of the binary when a mem pins a builtin schema but has no
/// workspace or cache copy.
pub(crate) fn builtin_schemas_dir() -> &'static Dir<'static> {
    &BUILTIN_SCHEMAS
}

/// Read a built-in schema package's optional `mem-template.json` —
/// the `MemConfig` starter a client (`memstead mem create`, the
/// planning skills) fills and passes through the `write_guidance`
/// create-parameter. Returns the parsed JSON object when the package
/// ships a template, `None` when it does not (`default`, `ingest`) or
/// when `name` is not a built-in.
///
/// The template is opaque to the engine (schema-strictness D8): it is
/// surfaced verbatim for the client to fill `<REQUIRED: …>`
/// placeholders. Loaded from the embedded `builtins/schemas/` tree so a
/// mem pinning a built-in resolves its template without a workspace
/// or cache copy, mirroring [`builtin_schemas_dir`].
pub fn builtin_mem_template(name: &str) -> Option<serde_json::Value> {
    let file = BUILTIN_SCHEMAS.get_file(format!("{name}/mem-template.json").as_str())?;
    serde_json::from_slice(file.contents()).ok()
}

/// One embedded built-in schema package: its identity plus every file
/// it ships, addressed relative to the package directory and sorted by
/// path. The raw-bytes view of the catalogue — [`load_builtin_schemas`]
/// is the parsed view. Consumed by the retention guard
/// (`tests/builtin_retention.rs`), which seals each shipped package's
/// content hash in `builtins/MANIFEST.toml`: a shipped `(name,
/// version)` must exist in every future binary with byte-identical
/// content, so a rebuild can never strand a workspace pinning it.
pub struct BuiltinPackage {
    pub name: String,
    pub version: String,
    /// `(path-relative-to-package-dir, bytes)`, sorted by path.
    pub files: Vec<(String, &'static [u8])>,
}

/// Enumerate every embedded built-in package with its raw file bytes.
/// Identity comes from each package's `schema.yaml` (`name:` /
/// `version:` keys); the directory name is organisational only.
pub fn builtin_packages() -> Vec<BuiltinPackage> {
    fn collect_files(dir: &Dir<'static>, root: &str, out: &mut Vec<(String, &'static [u8])>) {
        for file in dir.files() {
            let rel = file
                .path()
                .strip_prefix(root)
                .unwrap_or(file.path())
                .display()
                .to_string();
            out.push((rel, file.contents()));
        }
        for sub in dir.dirs() {
            collect_files(sub, root, out);
        }
    }

    let mut out = Vec::new();
    for dir in BUILTIN_SCHEMAS.dirs() {
        let root = dir.path().display().to_string();
        let manifest = dir
            .get_file(format!("{root}/schema.yaml").as_str())
            .and_then(|f| f.contents_utf8());
        let Some(manifest) = manifest else { continue };
        let header: Option<(String, String)> =
            serde_yaml_ng::from_str::<serde_yaml_ng::Value>(manifest)
                .ok()
                .and_then(|v| {
                    let name = v.get("name")?.as_str()?.to_string();
                    let version = v.get("version")?.as_str()?.to_string();
                    Some((name, version))
                });
        let Some((name, version)) = header else {
            continue;
        };
        let mut files = Vec::new();
        collect_files(dir, &root, &mut files);
        files.sort_by(|a, b| a.0.cmp(&b.0));
        out.push(BuiltinPackage {
            name,
            version,
            files,
        });
    }
    out
}

/// One embedded built-in package by identity, or `None` when no
/// built-in carries that `(name, version)`.
pub fn builtin_package(name: &str, version: &str) -> Option<BuiltinPackage> {
    builtin_packages()
        .into_iter()
        .find(|p| p.name == name && p.version == version)
}

/// The file name a package's README travels under.
pub const PACKAGE_README_FILE: &str = "README.md";

/// Render a package README for the package it ships in: every
/// `<name>@<x.y.z>` reference to the package's OWN name is rewritten to
/// the resolved `<name>@<version>`. References to other schemas, to
/// names that merely contain this one (`my-default@1.0.0`), and bare
/// names without a pin are left alone; the bytes themselves are never
/// touched (a shipped built-in is sealed by the retention guard).
///
/// Why at render time: sibling versions of a built-in ship the README
/// of their first generation verbatim, so 17 of 25 sealed packages
/// stated a version that was not theirs. A docs-only correction does
/// not justify a new schema generation, and editing a sealed package
/// in place is refused by design; the resolved manifest is the one
/// source of the identity, so the reader gets it from there.
pub fn render_package_readme(name: &str, version: &str, readme: &str) -> String {
    let needle = format!("{name}@");
    let mut out = String::with_capacity(readme.len());
    let mut rest = readme;
    while let Some(at) = rest.find(&needle) {
        let (before, tail) = rest.split_at(at);
        let after = &tail[needle.len()..];
        // Word boundary before the name: not part of a longer name.
        let bounded = before
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '-' || c == '_'));
        let pin_len = semver_prefix_len(after);
        if bounded && pin_len > 0 {
            out.push_str(before);
            out.push_str(&needle);
            out.push_str(version);
            rest = &after[pin_len..];
        } else {
            out.push_str(before);
            out.push_str(&needle);
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// Length of a leading `MAJOR.MINOR.PATCH` in `s`, or 0 when `s` does
/// not start with one (a trailing `.4` or a pre-release tag ends the
/// match at the patch number; a fourth component refuses the match).
fn semver_prefix_len(s: &str) -> usize {
    let mut len = 0;
    for part in 0..3 {
        let digits = s[len..].chars().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 {
            return 0;
        }
        len += digits;
        if part < 2 {
            if !s[len..].starts_with('.') {
                return 0;
            }
            len += 1;
        }
    }
    // A fourth dotted number is not a semver pin.
    if s[len..].starts_with('.') && s[len + 1..].starts_with(|c: char| c.is_ascii_digit()) {
        return 0;
    }
    len
}

/// Load every embedded schema into owned `Schema` values.
pub fn load_builtin_schemas() -> Result<Vec<Arc<Schema>>, SchemaLoadError> {
    let mut out = Vec::new();
    for dir in BUILTIN_SCHEMAS.dirs() {
        let schema = load_builtin_dir(dir)?;
        out.push(Arc::new(schema));
    }
    Ok(out)
}

fn load_builtin_dir(dir: &Dir<'_>) -> Result<Schema, SchemaLoadError> {
    let manifest_file = dir.get_file(format!("{}/schema.yaml", dir.path().display()).as_str());
    let manifest_text = manifest_file
        .and_then(|f| f.contents_utf8())
        .ok_or_else(|| SchemaLoadError::Io {
            path: dir.path().join("schema.yaml"),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "embedded schema.yaml missing or not utf-8",
            ),
        })?;

    let mut types: Vec<(String, String)> = Vec::new();
    let types_path = format!("{}/types", dir.path().display());
    if let Some(types_dir) = dir.get_dir(types_path.as_str()) {
        for file in types_dir.files() {
            if file.path().extension().and_then(|s| s.to_str()) != Some("yaml") {
                continue;
            }
            let Some(stem) = file
                .path()
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_owned)
            else {
                continue;
            };
            let Some(contents) = file.contents_utf8() else {
                return Err(SchemaLoadError::Io {
                    path: file.path().to_path_buf(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "embedded type yaml is not utf-8",
                    ),
                });
            };
            types.push((stem, contents.to_string()));
        }
    }

    // New builtin generations carry the format marker; sealed prior
    // generations don't and keep their legacy written meaning.
    let marker_path = format!(
        "{}/{}",
        dir.path().display(),
        loader::SCHEMA_FORMAT_MARKER_FILE
    );
    let format = if dir.get_file(marker_path.as_str()).is_some() {
        loader::MetadataPolarityFormat::RequiredOptIn
    } else {
        loader::MetadataPolarityFormat::Legacy
    };
    loader::load_schema_from_memory_with_format(manifest_text, &types, format)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_package_readme_rewrites_own_pins_only() {
        let readme = "Pin `default@1.0.0` (or `default@1.0.0`); see planning@0.1.0 and \
                      my-default@1.0.0; bare `default` stays; default@1.0.0.1 is not a pin; \
                      default@1.0.0-rc1 keeps its tag";
        let out = render_package_readme("default", "1.3.0", readme);
        assert_eq!(
            out,
            "Pin `default@1.3.0` (or `default@1.3.0`); see planning@0.1.0 and \
             my-default@1.0.0; bare `default` stays; default@1.0.0.1 is not a pin; \
             default@1.3.0-rc1 keeps its tag"
        );
        // Unchanged input when nothing matches, byte for byte.
        assert_eq!(render_package_readme("software", "0.4.0", readme), readme);
        assert_eq!(render_package_readme("default", "1.3.0", ""), "");
    }

    #[test]
    fn every_builtin_readme_renders_its_own_identity() {
        let mut rendered = 0;
        for pkg in builtin_packages() {
            let Some((_, bytes)) = pkg.files.iter().find(|(p, _)| p == PACKAGE_README_FILE) else {
                continue;
            };
            let readme = std::str::from_utf8(bytes).expect("README is UTF-8");
            let out = render_package_readme(&pkg.name, &pkg.version, readme);
            let own = format!("{}@{}", pkg.name, pkg.version);
            let needle = format!("{}@", pkg.name);
            for (i, _) in out.match_indices(&needle) {
                let bounded = out[..i]
                    .chars()
                    .next_back()
                    .is_none_or(|c| !(c.is_alphanumeric() || c == '-' || c == '_'));
                let tail = &out[i + needle.len()..];
                if bounded && semver_prefix_len(tail) > 0 {
                    assert!(
                        tail.starts_with(&pkg.version),
                        "{own}: README still states {}",
                        &out[i..i + needle.len() + semver_prefix_len(tail)]
                    );
                }
            }
            assert!(
                out.contains(&own) || !readme.contains(&needle),
                "{own}: a README that pins its own name must render the resolved pin"
            );
            rendered += 1;
        }
        assert!(rendered > 0, "at least one built-in ships a README");
    }

    /// The three scaffolding-bearing built-ins ship a parseable
    /// `mem-template.json` carrying their instance writeGuidance key;
    /// the deprecated literal `goal`/`avoid` (now in the schema's
    /// `default_writing_guidance`) must NOT be present.
    #[test]
    fn builtin_mem_templates_carry_instance_keys_only() {
        let cases = [
            ("planning", "phase_context"),
            ("project", "scope"),
            ("software", "stack"),
        ];
        for (name, instance_key) in cases {
            let tpl = builtin_mem_template(name)
                .unwrap_or_else(|| panic!("{name} must ship a mem-template.json"));
            assert!(
                tpl["language"].is_string(),
                "{name}: template carries language"
            );
            let wg = &tpl["writeGuidance"];
            assert!(
                wg.get(instance_key).is_some(),
                "{name}: template carries instance key {instance_key}",
            );
            assert!(
                wg.get("goal").is_none() && wg.get("avoid").is_none(),
                "{name}: template must not carry the deprecated literal goal/avoid (schema owns those)",
            );
        }
    }

    /// Packages without a template (and unknown names) resolve to None.
    #[test]
    fn builtin_mem_template_absent_is_none() {
        assert!(builtin_mem_template("default").is_none());
        assert!(builtin_mem_template("ingest").is_none());
        assert!(builtin_mem_template("not-a-builtin").is_none());
    }

    /// The added template files are inert to schema loading — every
    /// built-in still loads (the loader reads only schema.yaml + types/).
    #[test]
    fn all_builtins_still_load_with_templates_present() {
        let schemas = load_builtin_schemas().expect("built-ins load");
        assert!(
            schemas.iter().any(|s| s.manifest.name == "planning"),
            "planning still loads alongside its mem-template.json",
        );
    }
}
