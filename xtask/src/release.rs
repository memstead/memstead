//! `release` — the mechanical leg of cutting an engine release.
//!
//! Encodes the release runbook's local steps as one command so no agent
//! re-derives them from prose: version bump across `[workspace.package]`
//! and every inter-crate pin, `Cargo.lock` refresh, changelog cut with
//! compare links, the docs-vs-binary guard (every `memstead <cmd>` the
//! flagship documents must resolve in a freshly built full binary — the
//! v0.1.0 release shipped 71 minutes before `quickstart` landed, and
//! v0.2.0's plugin front door called a `projection` subcommand the
//! released binary lacked; this guard exists so that class of mismatch
//! blocks the tag), API-docs regeneration, and the test/lint matrix.
//!
//! The outward actions — commit, push, CI wait, tag, gitlink bump — stay
//! with the operator/agent: this command edits, checks, and then prints
//! them. It never runs `git` mutations.
//!
//! Invocation (from `public/`):
//!     cargo run -p xtask -- release <new-version> [--skip-tests]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail, ensure};
use regex::Regex;

#[derive(clap::Args, Debug)]
pub struct ReleaseArgs {
    /// The new workspace version, e.g. `0.4.0` (SemVer, no `v` prefix).
    version: String,
    /// Skip the local test/lint matrix and leave it to CI. The tag must
    /// still wait for CI green — this only skips the local pre-run.
    #[arg(long)]
    skip_tests: bool,
    /// Proceed on a dirty working tree. Testing/recovery only: a real
    /// release starts from a clean, pushed tree.
    #[arg(long)]
    allow_dirty: bool,
    /// Flagship docs directory for the docs-vs-binary guard. Defaults to
    /// `../websites/memstead.ai/flagship` next to the engine checkout
    /// (the private-workspace layout); when absent the guard is skipped
    /// with a warning, never silently.
    #[arg(long)]
    flagship_dir: Option<PathBuf>,
}

pub fn run(args: ReleaseArgs) -> Result<()> {
    let root = workspace_root();

    // 1. Preflight: clean tree, sane version string.
    ensure!(
        semver_shape(&args.version),
        "`{}` is not a plain SemVer version (expected e.g. 0.4.0)",
        args.version
    );
    if !args.allow_dirty {
        let status = capture(&root, "git", &["status", "--porcelain"])?;
        ensure!(
            status.trim().is_empty(),
            "working tree is dirty — a release cuts from a clean, committed tree \
             (--allow-dirty for testing/recovery):\n{status}"
        );
    }
    assert_dist_app_set(&root)?;

    // 2. Current version from the single source of truth.
    let cargo_toml_path = root.join("Cargo.toml");
    let cargo_toml = fs::read_to_string(&cargo_toml_path)
        .with_context(|| format!("reading {}", cargo_toml_path.display()))?;
    let old = current_workspace_version(&cargo_toml)?;
    ensure!(
        old != args.version,
        "workspace is already at {old} — nothing to bump"
    );
    println!("release: {old} → {}", args.version);

    // 3. Changelog discipline: entries land with the feature, not at
    //    release time. An empty [Unreleased] means the release notes were
    //    never written — refuse before touching anything.
    let changelog_path = root.join("CHANGELOG.md");
    let changelog = fs::read_to_string(&changelog_path)
        .with_context(|| format!("reading {}", changelog_path.display()))?;
    ensure!(
        !unreleased_section(&changelog)?.trim().is_empty(),
        "CHANGELOG.md `[Unreleased]` is empty — author the release notes first \
         (what changed since v{old}, Keep-a-Changelog sections), then re-run"
    );

    // 4. Version bump: [workspace.package] plus every inter-crate pin.
    let (bumped, replaced) = bump_versions(&cargo_toml, &old, &args.version)?;
    fs::write(&cargo_toml_path, bumped)?;
    println!("release: Cargo.toml — {replaced} version strings bumped");

    // 5. Cargo.lock follows the manifests.
    run_streamed(&root, "cargo", &["update", "--workspace"])?;

    // 6. Cut the changelog: [Unreleased] → dated section + compare links.
    let cut = cut_changelog(&changelog, &old, &args.version, &today_utc())?;
    fs::write(&changelog_path, cut)?;
    println!(
        "release: CHANGELOG.md — [Unreleased] cut to [{}]",
        args.version
    );

    // 7. Docs-vs-binary guard against a freshly built full binary.
    run_streamed(&root, "cargo", &["build", "-p", "memstead-cli"])?;
    let flagship = args
        .flagship_dir
        .unwrap_or_else(|| root.join("../websites/memstead.ai/flagship"));
    if flagship.is_dir() {
        docs_vs_binary_guard(&root, &flagship)?;
    } else {
        eprintln!(
            "release: WARNING — flagship dir {} absent, docs-vs-binary guard \
             SKIPPED (fine on a public-only checkout; run it from the private \
             workspace before tagging)",
            flagship.display()
        );
    }

    // 8. Generated docs stay in lockstep (the pre-push hook enforces this;
    //    doing it here keeps the release commit self-contained).
    crate::generate_docs(crate::GenerateDocsArgs {
        output: root.join("docs-site/src/content/docs/reference"),
    })?;

    // 9. The runbook's green gate, locally (CI re-runs the same legs).
    if args.skip_tests {
        println!("release: --skip-tests — the tag still waits for CI green");
    } else {
        run_streamed(&root, "./run-tests.sh", &[])?;
        run_streamed(
            &root,
            "cargo",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--features",
                "mem-repo",
                "--",
                "-D",
                "warnings",
            ],
        )?;
        run_streamed(
            &root,
            "cargo",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--no-default-features",
                "--",
                "-D",
                "warnings",
            ],
        )?;
        run_streamed(&root, "cargo", &["fmt", "--check"])?;
    }

    // 10. The outward steps stay human/agent-gated — print them exactly.
    let v = &args.version;
    println!(
        "\nrelease: mechanical leg done. Outward steps (in order, each gated \
         on the one before):\n\
         \n  1. Review the diff, then commit with a narrative message:\n\
         \x20        git add Cargo.toml Cargo.lock CHANGELOG.md docs-site\n\
         \x20        git commit   # release: {v} — <why this release exists>\n\
         \n  2. Push and wait for ALL public CI green (the bundle rule — \
         never tag on red or pending):\n\
         \x20        git push origin main\n\
         \n  3. Tag — the tag push is the entire binary-release trigger \
         (build → attest → GitHub Release → Homebrew tap):\n\
         \x20        git tag -a v{v} -m \"v{v}\" && git push origin v{v}\n\
         \n  4. Bump the public gitlink in the private repo (bundle rule (a)).\n\
         \n  5. Registries when launching: scripts/publish-crates.sh / \
         publish-npm.sh — always --dry-run first.\n\
         \n  6. Verify from the real channel: install.sh into a scratch \
         CARGO_HOME, `memstead --version` = {v}, one documented command \
         end-to-end, `gh attestation verify` on a downloaded artifact.\n"
    );
    Ok(())
}

fn workspace_root() -> PathBuf {
    // xtask lives at <root>/xtask — the engine workspace root is its parent.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a parent directory")
        .to_path_buf()
}

/// The dist-app set this workspace intends to release. cargo-dist ships
/// every *publishable* crate that carries any binary target unless the
/// crate opts out (`[package.metadata.dist] dist = false`) — and Cargo
/// auto-detects `src/bin/*.rs` and `src/main.rs` as binary targets. That
/// pair of defaults is exactly how the internal `emit_json_schemas` dev
/// tool shipped as a third "memstead-schema" app (installer + Homebrew
/// formula) in v0.2.0/v0.3.0, unnoticed for two releases. Growing this
/// list is a product decision — never an accident.
const EXPECTED_DIST_APPS: [&str; 2] = ["memstead-cli", "memstead-mcp"];

/// Mirror cargo-dist's app selection over the workspace members and
/// refuse when the derived set differs from [`EXPECTED_DIST_APPS`]. The
/// mirror is intentionally simple (publishable + has a binary target +
/// not opted out); if it ever diverges from real dist behaviour, the
/// failure mode is a loud false alarm at release time — never a silent
/// extra app in the release.
fn assert_dist_app_set(root: &Path) -> Result<()> {
    let manifest: toml::Value = fs::read_to_string(root.join("Cargo.toml"))?
        .parse()
        .context("parsing workspace Cargo.toml")?;
    let members = manifest
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(|m| m.as_array())
        .context("workspace Cargo.toml has no members list")?;

    let mut apps = Vec::new();
    for member in members {
        let rel = member.as_str().context("non-string workspace member")?;
        let dir = root.join(rel);
        let pkg: toml::Value = fs::read_to_string(dir.join("Cargo.toml"))
            .with_context(|| format!("reading {rel}/Cargo.toml"))?
            .parse()
            .with_context(|| format!("parsing {rel}/Cargo.toml"))?;
        let package = pkg.get("package").context("member without [package]")?;

        let publishable = package
            .get("publish")
            .and_then(|p| p.as_bool())
            .unwrap_or(true);
        let dist_opted_out = package
            .get("metadata")
            .and_then(|m| m.get("dist"))
            .and_then(|d| d.get("dist"))
            .and_then(|v| v.as_bool())
            == Some(false);
        let has_bin_target = pkg.get("bin").is_some()
            || dir.join("src/main.rs").is_file()
            || fs::read_dir(dir.join("src/bin")).is_ok_and(|entries| {
                entries
                    .flatten()
                    .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("rs"))
            });

        if publishable && !dist_opted_out && has_bin_target {
            apps.push(
                package
                    .get("name")
                    .and_then(|n| n.as_str())
                    .context("member package without name")?
                    .to_owned(),
            );
        }
    }
    apps.sort();
    let mut expected: Vec<&str> = EXPECTED_DIST_APPS.to_vec();
    expected.sort_unstable();
    ensure!(
        apps == expected,
        "dist-app set changed: the release would ship {apps:?}, expected \
         {expected:?}. A new binary target in a publishable crate becomes its \
         own installer + Homebrew formula (the v0.2.0 emit_json_schemas \
         accident). If intended, grow EXPECTED_DIST_APPS deliberately; if not, \
         opt the crate out with `[package.metadata.dist] dist = false` or make \
         the binary non-publishable."
    );
    println!(
        "release: dist-app set OK ({})",
        EXPECTED_DIST_APPS.join(", ")
    );
    Ok(())
}

fn semver_shape(v: &str) -> bool {
    let mut parts = v.split('.');
    let ok =
        |s: Option<&str>| s.is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    ok(parts.next()) && ok(parts.next()) && ok(parts.next()) && parts.next().is_none()
}

/// Read `[workspace.package] version` — the single source of truth.
fn current_workspace_version(cargo_toml: &str) -> Result<String> {
    let value: toml::Value = cargo_toml.parse().context("parsing Cargo.toml")?;
    value
        .get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .context("Cargo.toml has no [workspace.package] version")
}

/// Replace every `version = "<old>"` with the new version and verify the
/// count matches expectation: one `[workspace.package]` line plus one per
/// inter-crate pin (`path = "crates/…", version = "<old>"`). A mismatch
/// means the pin set changed shape — refuse loudly instead of guessing.
fn bump_versions(cargo_toml: &str, old: &str, new: &str) -> Result<(String, usize)> {
    let needle = format!("version = \"{old}\"");
    let found = cargo_toml.matches(&needle).count();
    let pin_re = Regex::new(&format!(
        r#"(?m)^memstead-[a-z-]+ = \{{ path = "crates/[^"]+", version = "{}" \}}"#,
        regex::escape(old)
    ))
    .expect("static regex");
    let expected = 1 + pin_re.find_iter(cargo_toml).count();
    ensure!(
        found == expected && expected > 1,
        "expected {expected} occurrences of `{needle}` (workspace.package + \
         inter-crate pins) but found {found} — the pin set changed shape; \
         update the release tooling to match Cargo.toml"
    );
    Ok((
        cargo_toml.replace(&needle, &format!("version = \"{new}\"")),
        found,
    ))
}

/// The body of `## [Unreleased]` up to the next `## [` heading.
fn unreleased_section(changelog: &str) -> Result<&str> {
    let start = changelog
        .find("## [Unreleased]")
        .context("CHANGELOG.md has no `## [Unreleased]` section")?;
    let body = &changelog[start + "## [Unreleased]".len()..];
    Ok(match body.find("\n## [") {
        Some(end) => &body[..end],
        None => body,
    })
}

/// `[Unreleased]` → fresh empty `[Unreleased]` + dated version section;
/// compare links at the bottom updated to match.
fn cut_changelog(changelog: &str, old: &str, new: &str, date: &str) -> Result<String> {
    let with_section = changelog.replacen(
        "## [Unreleased]",
        &format!("## [Unreleased]\n\n## [{new}] - {date}"),
        1,
    );
    ensure!(
        with_section != *changelog,
        "CHANGELOG.md has no `## [Unreleased]` section"
    );
    let old_link_prefix = "[Unreleased]: ";
    let start = with_section
        .find(old_link_prefix)
        .context("CHANGELOG.md has no `[Unreleased]:` compare link")?;
    let end = with_section[start..]
        .find('\n')
        .map(|i| start + i)
        .unwrap_or(with_section.len());
    let new_links = format!(
        "[Unreleased]: https://github.com/memstead/memstead/compare/v{new}...HEAD\n\
         [{new}]: https://github.com/memstead/memstead/compare/v{old}...v{new}"
    );
    let mut out = String::with_capacity(with_section.len() + new_links.len());
    out.push_str(&with_section[..start]);
    out.push_str(&new_links);
    out.push_str(&with_section[end..]);
    Ok(out)
}

/// Every `memstead <cmd> [<sub>]` phrase the flagship documents must
/// resolve via `--help` in the freshly built full binary. Prose phrases
/// that merely start with "memstead" ("memstead binaries run …") live in
/// `xtask/docs-guard-allow.txt` — an explicit, reviewed list, never a
/// silent skip.
fn docs_vs_binary_guard(root: &Path, flagship: &Path) -> Result<()> {
    let bin = root.join("target/debug/memstead");
    let allow = fs::read_to_string(root.join("xtask/docs-guard-allow.txt")).unwrap_or_default();
    let allowed: Vec<&str> = allow
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();

    // candidate phrase → one file it appears in (for the error message)
    let mut candidates: BTreeMap<(String, Option<String>), String> = BTreeMap::new();
    for entry in fs::read_dir(flagship)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let text = fs::read_to_string(&path)?;
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        for (cmd, sub) in extract_doc_commands(&text) {
            candidates.entry((cmd, sub)).or_insert_with(|| name.clone());
        }
    }

    let resolves = |args: &[&str]| -> bool {
        Command::new(&bin)
            .args(args)
            .arg("--help")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };

    let mut violations = Vec::new();
    let mut checked = 0usize;
    for ((cmd, sub), file) in &candidates {
        let phrase = match sub {
            Some(s) => format!("{cmd} {s}"),
            None => cmd.clone(),
        };
        if allowed.contains(&cmd.as_str()) || allowed.contains(&phrase.as_str()) {
            continue;
        }
        checked += 1;
        let ok = match sub {
            Some(s) => resolves(&[cmd, s]) || resolves(&[cmd]),
            None => resolves(&[cmd]),
        };
        if !ok {
            violations.push(format!("  memstead {phrase}   (documented in {file})"));
        }
    }
    if !violations.is_empty() {
        bail!(
            "docs-vs-binary guard FAILED — the flagship documents commands the \
             binary being tagged does not have (the v0.1.0 failure class):\n{}\n\
             Fix the binary, or — if a phrase is prose, not a command — add it \
             to xtask/docs-guard-allow.txt with a comment.",
            violations.join("\n")
        );
    }
    println!("release: docs-vs-binary guard OK ({checked} documented invocations resolve)");
    Ok(())
}

/// `memstead <token> [<token>]` phrases in documentation prose/code spans.
fn extract_doc_commands(md: &str) -> Vec<(String, Option<String>)> {
    let re = Regex::new(r"(?:^|[\s`(>*])memstead ([a-z][a-z-]*)(?: ([a-z][a-z-]*))?")
        .expect("static regex");
    re.captures_iter(md)
        .map(|c| (c[1].to_owned(), c.get(2).map(|m| m.as_str().to_owned())))
        .collect()
}

/// Today (UTC) as `YYYY-MM-DD`, no date dependency: Howard Hinnant's
/// `civil_from_days` over the Unix epoch.
fn today_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before the Unix epoch")
        .as_secs();
    civil_from_days((secs / 86_400) as i64)
}

fn civil_from_days(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn capture(cwd: &Path, cmd: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(cmd)
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("running `{cmd} {}`", args.join(" ")))?;
    ensure!(
        out.status.success(),
        "`{cmd} {}` failed:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn run_streamed(cwd: &Path, cmd: &str, args: &[&str]) -> Result<()> {
    println!("release: running `{cmd} {}`", args.join(" "));
    let status = Command::new(cmd)
        .args(args)
        .current_dir(cwd)
        .status()
        .with_context(|| format!("running `{cmd}`"))?;
    ensure!(status.success(), "`{cmd} {}` failed", args.join(" "));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dist_app_set_is_exactly_the_intended_two() {
        // Runs against the live workspace on every test run, so a new
        // accidental binary target fails CI on the push that adds it —
        // not two releases later (the emit_json_schemas lesson).
        assert_dist_app_set(&workspace_root()).unwrap();
    }

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), "1970-01-01");
        // 2024-01-01T00:00:00Z = 1_704_067_200s
        assert_eq!(civil_from_days(1_704_067_200 / 86_400), "2024-01-01");
        // 2000-02-29 (leap day) = 946_684_800 + 59 days
        assert_eq!(civil_from_days(946_684_800 / 86_400 + 59), "2000-02-29");
    }

    #[test]
    fn bump_replaces_workspace_and_pins_only() {
        let toml = r#"[workspace.package]
version = "0.3.0"
edition = "2024"

[workspace.dependencies]
memstead-schema = { path = "crates/memstead-schema", version = "0.3.0" }
memstead-base = { path = "crates/memstead-base", version = "0.3.0" }
serde = { version = "1", features = ["derive"] }
"#;
        let (out, n) = bump_versions(toml, "0.3.0", "0.4.0").unwrap();
        assert_eq!(n, 3);
        assert!(out.contains(r#"version = "0.4.0""#));
        assert!(!out.contains(r#"version = "0.3.0""#));
        assert!(out.contains(r#"serde = { version = "1""#));
    }

    #[test]
    fn bump_refuses_on_count_mismatch() {
        // A pin left behind on an old version → count off → refuse.
        let toml = r#"[workspace.package]
version = "0.3.0"
[workspace.dependencies]
memstead-base = { path = "crates/memstead-base", version = "0.2.0" }
"#;
        assert!(bump_versions(toml, "0.3.0", "0.4.0").is_err());
    }

    #[test]
    fn changelog_cut_inserts_section_and_links() {
        let log = "# Changelog\n\n## [Unreleased]\n\n- a change\n\n## [0.3.0] - 2026-07-11\n\nold\n\n[Unreleased]: https://github.com/memstead/memstead/compare/v0.3.0...HEAD\n[0.3.0]: https://github.com/memstead/memstead/compare/v0.2.0...v0.3.0\n";
        let out = cut_changelog(log, "0.3.0", "0.4.0", "2026-08-01").unwrap();
        assert!(out.contains("## [Unreleased]\n\n## [0.4.0] - 2026-08-01\n\n- a change"));
        assert!(
            out.contains(
                "[Unreleased]: https://github.com/memstead/memstead/compare/v0.4.0...HEAD"
            )
        );
        assert!(
            out.contains("[0.4.0]: https://github.com/memstead/memstead/compare/v0.3.0...v0.4.0")
        );
        assert!(
            out.contains("[0.3.0]: https://github.com/memstead/memstead/compare/v0.2.0...v0.3.0")
        );
    }

    #[test]
    fn empty_unreleased_is_detected() {
        let log = "# Changelog\n\n## [Unreleased]\n\n## [0.3.0] - 2026-07-11\n\nx\n";
        assert!(unreleased_section(log).unwrap().trim().is_empty());
        let log2 = "# Changelog\n\n## [Unreleased]\n\n- something\n\n## [0.3.0] - 2026-07-11\n";
        assert!(!unreleased_section(log2).unwrap().trim().is_empty());
    }

    #[test]
    fn doc_command_extraction_finds_one_and_two_token_forms() {
        let md = "Run: memstead install <scope>/<name>\nthe memstead binaries run fine\n`memstead mem set-schema x y`\n";
        let cmds = extract_doc_commands(md);
        assert!(cmds.contains(&("install".into(), None)));
        assert!(cmds.contains(&("binaries".into(), Some("run".into()))));
        assert!(cmds.contains(&("mem".into(), Some("set-schema".into()))));
    }
}
