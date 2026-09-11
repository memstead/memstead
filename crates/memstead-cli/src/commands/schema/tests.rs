#![cfg(test)]

use super::*;
use std::path::Path;

fn ctx() -> CliContext {
    CliContext {
        json: false,
        quiet: true,
        role: Default::default(),
        identity: None,
    }
}

/// An unsealed copy of the current built-in is treated as fresh
/// authoring input, and the shipped default-1.3 still speaks the
/// retired exemplar-relation spelling (`to:` / `type:`) — its
/// bytes are sealed, so it converges only at the family's next
/// minted-for-meaning bump. Until then, validating the copy
/// REFUSES with the rename pointer (install-time strict); the
/// sealed original keeps loading through the catalogue's translate
/// path, pinned by the loader suite. A converged copy of the same
/// content validates cleanly — proving the refusal is the spelling
/// alone.
#[test]
fn validate_builtin_default_copy_refuses_retired_exemplar_spelling() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../memstead-schema/builtins/schemas/default-1.3");
    assert!(src.join("schema.yaml").is_file(), "fixture moved: {src:?}");
    let dir = tempfile::tempdir().unwrap();
    let dst = dir.path().join("authoring");
    copy_dir_without_marker(&src, &dst);
    let err = validate(&ctx(), ValidateArgs { path: dst.clone() })
        .expect_err("legacy exemplar spelling refuses as authoring input");
    assert!(
        err.to_string().contains("rel_type"),
        "refusal carries the rename pointer: {err}"
    );

    // Mechanical rename to the mutation vocabulary — then the same
    // content validates cleanly.
    for entry in std::fs::read_dir(dst.join("types")).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        let converged = text
            .replace("\n    - to: ", "\n    - target: ")
            .replace("\n      type: ", "\n      rel_type: ");
        std::fs::write(&path, converged).unwrap();
    }
    validate(&ctx(), ValidateArgs { path: dst })
        .expect("converged default builtin content must validate");
}

fn copy_dir_without_marker(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == "schema-format.json" {
            continue;
        }
        let target = dst.join(&name);
        if entry.file_type().unwrap().is_dir() {
            copy_dir_without_marker(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// The migrate verb on the sealed default-1.3 copy (retired exemplar
/// spelling): the dry run writes nothing and reports the rewrites;
/// `--write` makes the copy validate as authoring input.
#[test]
fn migrate_dry_run_then_write_makes_legacy_copy_validate() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../memstead-schema/builtins/schemas/default-1.3");
    let dir = tempfile::tempdir().unwrap();
    let dst = dir.path().join("authoring");
    copy_dir_without_marker(&src, &dst);
    let snapshot = |d: &Path| -> Vec<(String, Vec<u8>)> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(d.join("types")).unwrap() {
            let entry = entry.unwrap();
            files.push((
                entry.file_name().to_string_lossy().into_owned(),
                std::fs::read(entry.path()).unwrap(),
            ));
        }
        files.sort();
        files
    };
    let before = snapshot(&dst);
    validate(&ctx(), ValidateArgs { path: dst.clone() })
        .expect_err("legacy copy refuses before migration");

    migrate(
        &ctx(),
        MigrateArgs {
            path: dst.clone(),
            write: false,
        },
    )
    .expect("dry run computes");
    assert_eq!(snapshot(&dst), before, "dry run writes nothing");

    migrate(
        &ctx(),
        MigrateArgs {
            path: dst.clone(),
            write: true,
        },
    )
    .expect("write applies");
    assert_ne!(snapshot(&dst), before, "--write rewrites the files");
    validate(&ctx(), ValidateArgs { path: dst.clone() }).expect("migrated copy validates");

    // A second run finds nothing and leaves the bytes alone.
    let after = snapshot(&dst);
    migrate(
        &ctx(),
        MigrateArgs {
            path: dst.clone(),
            write: true,
        },
    )
    .expect("noop run");
    assert_eq!(snapshot(&dst), after);
}

/// A sealed package is refused by the migrate verb the same way
/// `validate` refuses it — the verb never edits a sealed copy.
#[test]
fn migrate_refuses_sealed_package() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../memstead-schema/builtins/schemas/default-1.3");
    let err =
        migrate(&ctx(), MigrateArgs { path, write: true }).expect_err("sealed package must refuse");
    let cli = err.downcast_ref::<CliError>().expect("typed CLI error");
    assert_eq!(cli.code, "SCHEMA_MIGRATE_FAILED");
    assert_eq!(cli.details.as_ref().unwrap()["reason"], "sealed_package");
}

/// A directory carrying `schema-format.json` — a sealed package —
/// is named as such instead of being conformance-checked as
/// authoring input. The shipped builtin is exactly that shape.
#[test]
fn validate_names_sealed_package() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../memstead-schema/builtins/schemas/default-1.3");
    let err = validate(&ctx(), ValidateArgs { path }).expect_err("sealed package must refuse");
    let cli = err
        .downcast_ref::<CliError>()
        .expect("error is a typed CliError");
    assert_eq!(cli.code, "SCHEMA_VALIDATION_FAILED");
    assert!(
        cli.message.contains("sealed schema package"),
        "message names the sealed package: {}",
        cli.message,
    );
    assert_eq!(
        cli.details.as_ref().unwrap()["reason"],
        json!("sealed_package"),
    );
}

/// A malformed `schema.yaml` refuses with the typed
/// `SCHEMA_VALIDATION_FAILED` code carrying the path in `details`.
#[test]
fn validate_rejects_malformed_schema_with_typed_code() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("schema.yaml"), "name: [unterminated\n").unwrap();
    let err = validate(
        &ctx(),
        ValidateArgs {
            path: dir.path().to_path_buf(),
        },
    )
    .expect_err("malformed schema must refuse");
    let cli = err
        .downcast_ref::<CliError>()
        .expect("error is a typed CliError");
    assert_eq!(cli.code, "SCHEMA_VALIDATION_FAILED");
    assert_eq!(cli.kind, ExitKind::Validation);
    assert_eq!(
        cli.details.as_ref().unwrap()["path"],
        json!(dir.path()),
        "details echoes the offending path",
    );
}

/// A read resolves a bare name to the newest generation and a pin
/// as given; an unknown name refuses typed with the roster.
#[test]
fn resolve_builtin_read_ref_defaults_bare_names_to_newest() {
    let newest = resolve_builtin_read_ref("planning").expect("bare planning reads");
    let mut all = memstead_schema::SchemaRegistry::builtin().available_versions("planning");
    all.sort();
    assert_eq!(Some(&newest.version), all.last());
    let pinned = resolve_builtin_read_ref("planning@0.1.0").expect("pin reads");
    assert_eq!(pinned.version.to_string(), "0.1.0");
    let err = resolve_builtin_read_ref("not-a-builtin").expect_err("unknown refuses");
    let cli = err.downcast_ref::<CliError>().unwrap();
    assert_eq!(cli.code, "SCHEMA_NOT_FOUND");
    assert!(
        cli.message.contains("planning"),
        "names the roster: {}",
        cli.message
    );
}

/// A bare built-in name resolves to its concrete pin; an explicit
/// `name@version` is accepted; an unknown name refuses typed.
#[test]
fn resolve_builtin_ref_handles_name_pin_and_unknown() {
    // Every built-in ships multiple versions since the plan-06
    // vocabulary bump, so bare names are ambiguous by design and
    // pins resolve explicitly.
    let bare = resolve_builtin_ref("software@0.2.0").expect("software pin resolves");
    assert_eq!(bare.name, "software");
    let pinned = resolve_builtin_ref("planning@0.1.0").expect("explicit pin resolves");
    assert_eq!(pinned.name, "planning");
    assert_eq!(pinned.version.to_string(), "0.1.0");
    resolve_builtin_ref("planning@0.2.0").expect("bumped pin resolves");
    resolve_builtin_ref("planning").expect_err("bare planning is ambiguous");
    let err = resolve_builtin_ref("not-a-builtin").expect_err("unknown name refuses");
    assert_eq!(
        err.downcast_ref::<CliError>().unwrap().code,
        "SCHEMA_NOT_FOUND",
    );
}

/// Installing a built-in by name collects its schema files *and*
/// its `mem-template.json`.
#[test]
fn resolve_source_for_builtin_includes_schema_and_template() {
    let (schema_ref, files) =
        resolve_source("planning@0.1.0", None).expect("planning source collects");
    assert_eq!(schema_ref.name, "planning");
    let paths: Vec<&str> = files.iter().map(|f| f.archive_path.as_str()).collect();
    assert!(paths.contains(&"schema.yaml"), "got {paths:?}");
    assert!(
        paths.contains(&"mem-template.json"),
        "built-in install must carry the mem-template.json, got {paths:?}",
    );
}

/// `collect_dir_package` + `write_package` round-trip a package
/// (schema.yaml + types + template) onto disk verbatim.
#[test]
fn collect_and_write_package_round_trips() {
    let src = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(src.path().join("types")).unwrap();
    std::fs::write(src.path().join("schema.yaml"), b"name: x\n").unwrap();
    std::fs::write(src.path().join("types/doc.yaml"), b"name: doc\n").unwrap();
    std::fs::write(src.path().join("mem-template.json"), b"{}\n").unwrap();

    let files = collect_dir_package(src.path()).unwrap();
    let dest = tempfile::tempdir().unwrap();
    let pkg = dest.path().join("x@0.1.0");
    write_package(&pkg, &files).unwrap();

    // YAML members gain the installed-location directive; bodies and
    // non-YAML members (mem-template.json) are preserved.
    let schema = std::fs::read_to_string(pkg.join("schema.yaml")).unwrap();
    assert_eq!(
        schema,
        "# yaml-language-server: $schema=../../meta-schemas/schema-manifest.schema.json\nname: x\n",
    );
    let doc = std::fs::read_to_string(pkg.join("types/doc.yaml")).unwrap();
    assert_eq!(
        doc,
        "# yaml-language-server: $schema=../../../meta-schemas/type-definition.schema.json\nname: doc\n",
    );
    assert_eq!(
        std::fs::read(pkg.join("mem-template.json")).unwrap(),
        b"{}\n"
    );
    // Idempotent: a second write reproduces identical files.
    write_package(&pkg, &files).unwrap();
    assert_eq!(
        std::fs::read_to_string(pkg.join("schema.yaml")).unwrap(),
        schema
    );
}

/// The directive retarget replaces an existing leading directive (it
/// does not stack) and prepends one when absent; non-YAML and
/// non-UTF-8 members pass through.
#[test]
fn retarget_yaml_directive_replaces_or_prepends() {
    // Existing (repo-relative) directive is replaced, body kept.
    let existing = b"# yaml-language-server: $schema=../../../generated/schema-manifest.schema.json\nname: y\n";
    let out = String::from_utf8(retarget_yaml_directive("schema.yaml", existing)).unwrap();
    assert_eq!(
        out,
        "# yaml-language-server: $schema=../../meta-schemas/schema-manifest.schema.json\nname: y\n",
    );
    // Absent directive is prepended.
    let bare = retarget_yaml_directive("types/t.yaml", b"name: t\n");
    assert_eq!(
        String::from_utf8(bare).unwrap(),
        "# yaml-language-server: $schema=../../../meta-schemas/type-definition.schema.json\nname: t\n",
    );
    // Non-YAML members untouched.
    assert_eq!(retarget_yaml_directive("README.md", b"# hi\n"), b"# hi\n");
}
