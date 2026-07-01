//! `memstead schema validate <path>` — load a schema package from disk
//! and report whether it conforms to the engine's schema rules.
//!
//! Validation runs the same loader the engine uses at boot
//! (`memstead_schema::loader::load_schema_from_dir`), so a package that
//! validates here is one the engine will accept. Parse failures carry
//! the YAML layer's line/column in their message; structural failures
//! (undeclared relationship vocabulary, type/file mismatch, missing
//! `_default` weight, …) carry the engine's typed diagnostic.
//!
//! `memstead schema install <name|path>` copies a schema package into
//! the current workspace's local schema storage so a vault can pin it.
//! It resolves the source — a built-in name (`planning`, `planning@0.1.0`)
//! or a path to a package directory — validates it, and writes the
//! package (including any `vault-template.json`) under the folder
//! backend's fixed `<workspace>/.memstead/schemas/<name>@<version>/`
//! location. Installing a built-in forks it into local storage, which
//! shadows the built-in per the resolution order — the customization
//! entry point. Idempotent: re-running reproduces the same files.
//! Git-branch workspaces are not yet a destination (their schemas live
//! on the `__MEMSTEAD:schemas/` ref, which routes through the engine).
//!
//! `validate` is flavour-agnostic and touches no workspace; `install`
//! needs the workspace root (the install destination) but no engine
//! instance — it writes folder-backend schema storage directly, which
//! is the documented folder authoring mechanism (not vault-repo state).

use std::path::{Path, PathBuf};

use clap::{Args as ClapArgs, Subcommand};
use serde_json::json;

use memstead_schema::SchemaRef;

use crate::CliError;
use crate::output::{ExitKind, print_json, print_markdown};
use crate::setup::{CliContext, WorkspaceShape};

#[derive(ClapArgs, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub command: SchemaCommand,
}

#[derive(Subcommand, Debug)]
pub enum SchemaCommand {
    /// Validate a schema package directory (`schema.yaml` plus an
    /// optional `types/*.yaml`) against the engine's schema loader —
    /// the same validation the engine runs at load. Exits non-zero
    /// (`SCHEMA_VALIDATION_FAILED`) on any conformance error, with the
    /// YAML line/column in the message where the parse layer provides
    /// it.
    Validate(ValidateArgs),

    /// Install a schema package into the current folder workspace's
    /// `.memstead/schemas/<name>@<version>/` so a vault can pin it.
    /// `<source>` is a built-in name (`planning`, `planning@0.1.0`) or a
    /// path to a package directory. Validates before copying; idempotent.
    Install(InstallArgs),
}

#[derive(ClapArgs, Debug)]
pub struct ValidateArgs {
    /// Path to the schema package directory (the folder containing
    /// `schema.yaml`).
    pub path: PathBuf,
}

#[derive(ClapArgs, Debug)]
pub struct InstallArgs {
    /// Built-in schema name (`planning`, `planning@0.1.0`) or a path to
    /// a schema package directory.
    pub source: String,
}

pub fn run(ctx: &CliContext, args: Args) -> anyhow::Result<()> {
    match args.command {
        SchemaCommand::Validate(a) => validate(ctx, a),
        SchemaCommand::Install(a) => install(ctx, a),
    }
}

fn validate(ctx: &CliContext, args: ValidateArgs) -> anyhow::Result<()> {
    match memstead_schema::loader::load_schema_from_dir(&args.path) {
        Ok(schema) => {
            let (name, version) = schema.id();
            let type_count = schema.types.len();
            if ctx.json {
                print_json(&json!({
                    "ok": true,
                    "schema": format!("{name}@{version}"),
                    "types": type_count,
                    "path": args.path,
                }))?;
            } else {
                print_markdown(&format!(
                    "# Schema valid\n\n`{name}@{version}` — {type_count} type(s) at `{}`\n",
                    args.path.display(),
                ));
            }
            Ok(())
        }
        Err(e) => Err(CliError::new(
            ExitKind::Validation,
            "SCHEMA_VALIDATION_FAILED",
            format!("schema at {} is invalid: {e}", args.path.display()),
        )
        .with_details(json!({
            "path": args.path,
            "error": e.to_string(),
        }))
        .into()),
    }
}

fn install(ctx: &CliContext, args: InstallArgs) -> anyhow::Result<()> {
    let (shape, root) = ctx.workspace_shape().ok_or_else(|| {
        CliError::new(
            ExitKind::Generic,
            "NO_WORKSPACE",
            "not inside a Memstead workspace (no `.memstead/workspace.toml` in any ancestor)"
                .to_string(),
        )
    })?;
    let (schema_ref, files) = resolve_source(&args.source)?;

    match shape {
        WorkspaceShape::Filesystem => {
            // Folder backend: write the package under `.memstead/schemas/`.
            let pkg_dir = root
                .join(".memstead")
                .join("schemas")
                .join(format!("{}@{}", schema_ref.name, schema_ref.version));
            write_package(&pkg_dir, &files)?;
            if ctx.json {
                print_json(&json!({
                    "ok": true,
                    "schema": format!("{}@{}", schema_ref.name, schema_ref.version),
                    "backend": "folder",
                    "path": pkg_dir,
                    "files": files.iter().map(|f| &f.archive_path).collect::<Vec<_>>(),
                }))?;
            } else {
                print_markdown(&format!(
                    "# Schema installed\n\n`{}@{}` → `{}` ({} file(s))\n",
                    schema_ref.name,
                    schema_ref.version,
                    pkg_dir.display(),
                    files.len(),
                ));
            }
            Ok(())
        }
        WorkspaceShape::VaultRepo => install_to_git_branch(ctx, &schema_ref, &files),
    }
}

/// Install onto the git-branch backend — write the package onto the
/// workspace's `__MEMSTEAD:schemas/` ref through the engine (which owns
/// vault-repo state). Only present in the `vault-repo`-featured build;
/// the basis binary refuses (it has no git-branch engine).
#[cfg(feature = "vault-repo")]
fn install_to_git_branch(
    ctx: &CliContext,
    schema_ref: &SchemaRef,
    files: &[memstead_schema::SchemaSourceFile],
) -> anyhow::Result<()> {
    use crate::setup::CliEngine;
    let engine = match ctx.cli_engine()? {
        CliEngine::VaultRepo(e) => e,
        CliEngine::Filesystem(_) => {
            return Err(CliError::new(
                ExitKind::Generic,
                "INTERNAL",
                "workspace resolved as vault-repo but engine came back filesystem".to_string(),
            )
            .into());
        }
    };
    let pairs: Vec<(String, Vec<u8>)> = files
        .iter()
        .map(|f| (f.archive_path.clone(), f.bytes.clone()))
        .collect();
    let commit = engine
        .install_schema(&schema_ref.name, &schema_ref.version.to_string(), &pairs)
        .map_err(|e| {
            CliError::new(ExitKind::Generic, e.code(), e.to_string()).with_details(e.details())
        })?;
    if ctx.json {
        print_json(&json!({
            "ok": true,
            "schema": format!("{}@{}", schema_ref.name, schema_ref.version),
            "backend": "git-branch",
            "ref": format!("__MEMSTEAD:schemas/{}@{}", schema_ref.name, schema_ref.version),
            "commit": commit,
        }))?;
    } else {
        print_markdown(&format!(
            "# Schema installed\n\n`{}@{}` → `__MEMSTEAD:schemas/{}@{}` (commit `{}`)\n",
            schema_ref.name,
            schema_ref.version,
            schema_ref.name,
            schema_ref.version,
            commit,
        ));
    }
    Ok(())
}

#[cfg(not(feature = "vault-repo"))]
fn install_to_git_branch(
    _ctx: &CliContext,
    _schema_ref: &SchemaRef,
    _files: &[memstead_schema::SchemaSourceFile],
) -> anyhow::Result<()> {
    Err(CliError::new(
        ExitKind::Generic,
        "VAULT_REPO_NOT_SUPPORTED",
        "this binary was built without git-branch support — use the `memstead` binary to \
         install a schema into a vault-repo workspace."
            .to_string(),
    )
    .into())
}

/// Resolve `<source>` (a path to a package dir, or a built-in name /
/// `name@version`) to its pin and the package files to write.
fn resolve_source(source: &str) -> anyhow::Result<(SchemaRef, Vec<memstead_schema::SchemaSourceFile>)> {
    let as_path = Path::new(source);
    if as_path.is_dir() {
        // Path source — validate with the engine loader before copying.
        let schema = memstead_schema::load_schema_from_dir(as_path).map_err(|e| {
            CliError::new(
                ExitKind::Validation,
                "SCHEMA_VALIDATION_FAILED",
                format!("package at {source} is invalid: {e}"),
            )
            .with_details(json!({ "path": source, "error": e.to_string() }))
        })?;
        let (name, version) = schema.id();
        let files = collect_dir_package(as_path)?;
        Ok((SchemaRef::new(name, version), files))
    } else {
        // Name source — resolve against the built-in catalogue.
        let schema_ref = resolve_builtin_ref(source)?;
        let mut files = memstead_schema::collect_schema_source(None, None, &schema_ref).map_err(|e| {
            CliError::new(
                ExitKind::Validation,
                "SCHEMA_NOT_FOUND",
                format!("could not collect source for {}: {e}", schema_ref.as_display()),
            )
        })?;
        // Built-in packages may ship a `vault-template.json`; install it
        // alongside the schema so the scaffolding travels with the fork.
        if let Some(tpl) = memstead_schema::builtins::builtin_vault_template(&schema_ref.name) {
            files.push(memstead_schema::SchemaSourceFile {
                archive_path: "vault-template.json".to_string(),
                bytes: serde_json::to_vec_pretty(&tpl).unwrap_or_default(),
            });
        }
        Ok((schema_ref, files))
    }
}

/// Resolve a built-in source string (`planning` or `planning@0.1.0`) to
/// a concrete pin against the embedded catalogue.
fn resolve_builtin_ref(source: &str) -> anyhow::Result<SchemaRef> {
    let reg = memstead_schema::SchemaRegistry::builtin();
    if source.contains('@') {
        let r: SchemaRef = source.parse().map_err(|e: String| {
            CliError::new(
                ExitKind::Validation,
                "INVALID_INPUT",
                format!("invalid schema pin {source:?}: {e}"),
            )
        })?;
        if reg.get(&r.name, &r.version).is_none() {
            return Err(CliError::new(
                ExitKind::Validation,
                "SCHEMA_NOT_FOUND",
                format!("no built-in schema {source} — pass a path to install a non-built-in package"),
            )
            .into());
        }
        Ok(r)
    } else {
        match reg.resolve_by_name(source) {
            Ok(Some(s)) => {
                let (n, v) = s.id();
                Ok(SchemaRef::new(n, v))
            }
            Ok(None) => Err(CliError::new(
                ExitKind::Validation,
                "SCHEMA_NOT_FOUND",
                format!(
                    "no built-in schema named {source:?} — pass a path to install a non-built-in \
                     package, or a `name@version` pin"
                ),
            )
            .into()),
            Err(e) => Err(CliError::new(
                ExitKind::Validation,
                "INVALID_INPUT",
                format!("built-in name {source:?} is ambiguous: {e}"),
            )
            .into()),
        }
    }
}

/// Collect the package files from an on-disk directory: `schema.yaml`,
/// `types/*.yaml`, and the optional `vault-template.json` / `README.md`.
fn collect_dir_package(dir: &Path) -> anyhow::Result<Vec<memstead_schema::SchemaSourceFile>> {
    use memstead_schema::SchemaSourceFile;
    let mut out = vec![SchemaSourceFile {
        archive_path: "schema.yaml".to_string(),
        bytes: std::fs::read(dir.join("schema.yaml"))?,
    }];
    let types = dir.join("types");
    if types.is_dir() {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&types)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("yaml"))
            .collect();
        paths.sort();
        for p in paths {
            if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                out.push(SchemaSourceFile {
                    archive_path: format!("types/{name}"),
                    bytes: std::fs::read(&p)?,
                });
            }
        }
    }
    for opt in ["vault-template.json", "README.md"] {
        let p = dir.join(opt);
        if p.is_file() {
            out.push(SchemaSourceFile {
                archive_path: opt.to_string(),
                bytes: std::fs::read(&p)?,
            });
        }
    }
    Ok(out)
}

/// Write the resolved package files under `pkg_dir`, creating parent
/// directories. The `# yaml-language-server:` directive on each YAML is
/// rewritten to the installed-location form so an editor resolves it
/// against the workspace's published `.memstead/meta-schemas/` rather
/// than the package source's repo-relative path. Idempotent —
/// re-running reproduces identical files.
fn write_package(pkg_dir: &Path, files: &[memstead_schema::SchemaSourceFile]) -> anyhow::Result<()> {
    for f in files {
        let dest = pkg_dir.join(&f.archive_path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                CliError::new(
                    ExitKind::Generic,
                    "IO_ERROR",
                    format!("could not create {}: {e}", parent.display()),
                )
            })?;
        }
        let bytes = retarget_yaml_directive(&f.archive_path, &f.bytes);
        std::fs::write(&dest, &bytes).map_err(|e| {
            CliError::new(
                ExitKind::Generic,
                "IO_ERROR",
                format!("could not write {}: {e}", dest.display()),
            )
        })?;
    }
    Ok(())
}

/// The installed-location `# yaml-language-server:` directive for a
/// package member, or `None` for non-YAML members (README,
/// vault-template.json). Paths are relative to the member's location
/// under `.memstead/schemas/<name>@<version>/` and resolve to the
/// workspace's `.memstead/meta-schemas/` published by engine boot.
fn directive_for(archive_path: &str) -> Option<&'static str> {
    if archive_path == "schema.yaml" {
        Some("# yaml-language-server: $schema=../../meta-schemas/schema-manifest.schema.json")
    } else if archive_path.starts_with("types/") && archive_path.ends_with(".yaml") {
        Some("# yaml-language-server: $schema=../../../meta-schemas/type-definition.schema.json")
    } else {
        None
    }
}

/// Replace a leading `# yaml-language-server:` directive (or prepend one)
/// so the installed YAML points at the workspace-published meta-schema.
/// Non-YAML members and non-UTF-8 bytes pass through verbatim.
fn retarget_yaml_directive(archive_path: &str, bytes: &[u8]) -> Vec<u8> {
    let Some(directive) = directive_for(archive_path) else {
        return bytes.to_vec();
    };
    let Ok(text) = std::str::from_utf8(bytes) else {
        return bytes.to_vec();
    };
    let body = if text.starts_with("# yaml-language-server:") {
        text.split_once('\n').map(|(_, rest)| rest).unwrap_or("")
    } else {
        text
    };
    format!("{directive}\n{body}").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn ctx() -> CliContext {
        CliContext { json: false, quiet: true }
    }

    /// A shipped built-in package validates cleanly — the loader the
    /// command runs is the same one the engine boots with.
    #[test]
    fn validate_accepts_builtin_default_schema() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../memstead-schema/builtins/schemas/default");
        assert!(path.join("schema.yaml").is_file(), "fixture moved: {path:?}");
        validate(&ctx(), ValidateArgs { path }).expect("default builtin must validate");
    }

    /// A malformed `schema.yaml` refuses with the typed
    /// `SCHEMA_VALIDATION_FAILED` code carrying the path in `details`.
    #[test]
    fn validate_rejects_malformed_schema_with_typed_code() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("schema.yaml"), "name: [unterminated\n").unwrap();
        let err = validate(&ctx(), ValidateArgs { path: dir.path().to_path_buf() })
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

    /// A bare built-in name resolves to its concrete pin; an explicit
    /// `name@version` is accepted; an unknown name refuses typed.
    #[test]
    fn resolve_builtin_ref_handles_name_pin_and_unknown() {
        let bare = resolve_builtin_ref("planning").expect("planning resolves");
        assert_eq!(bare.name, "planning");
        let pinned = resolve_builtin_ref("planning@0.1.0").expect("explicit pin resolves");
        assert_eq!(pinned, bare);
        let err = resolve_builtin_ref("not-a-builtin").expect_err("unknown name refuses");
        assert_eq!(
            err.downcast_ref::<CliError>().unwrap().code,
            "SCHEMA_NOT_FOUND",
        );
    }

    /// Installing a built-in by name collects its schema files *and*
    /// its `vault-template.json`.
    #[test]
    fn resolve_source_for_builtin_includes_schema_and_template() {
        let (schema_ref, files) = resolve_source("planning").expect("planning source collects");
        assert_eq!(schema_ref.name, "planning");
        let paths: Vec<&str> = files.iter().map(|f| f.archive_path.as_str()).collect();
        assert!(paths.contains(&"schema.yaml"), "got {paths:?}");
        assert!(
            paths.contains(&"vault-template.json"),
            "built-in install must carry the vault-template.json, got {paths:?}",
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
        std::fs::write(src.path().join("vault-template.json"), b"{}\n").unwrap();

        let files = collect_dir_package(src.path()).unwrap();
        let dest = tempfile::tempdir().unwrap();
        let pkg = dest.path().join("x@0.1.0");
        write_package(&pkg, &files).unwrap();

        // YAML members gain the installed-location directive; bodies and
        // non-YAML members (vault-template.json) are preserved.
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
        assert_eq!(std::fs::read(pkg.join("vault-template.json")).unwrap(), b"{}\n");
        // Idempotent: a second write reproduces identical files.
        write_package(&pkg, &files).unwrap();
        assert_eq!(std::fs::read_to_string(pkg.join("schema.yaml")).unwrap(), schema);
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
}
