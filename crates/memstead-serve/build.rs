//! Optionally embeds a built static site into `site/` so `include_dir!` can
//! bake it into the binary. The site is a **deployment input**, not a constant:
//! the directory to embed is named by the `MEMSTEAD_SERVE_SITE_DIST` build-time
//! environment variable (an absolute path, or one relative to this crate).
//! When it is unset or missing, `site/` is reset to just the `.gitkeep`
//! placeholder and the handlers serve a built-in placeholder landing — so a
//! plain `cargo build` is deployment-agnostic and embeds no specific site.
//!
//! A deployment sets `MEMSTEAD_SERVE_SITE_DIST` to its own built `dist/`; that
//! wiring lives in the deployment config, outside this crate.

use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let site_dst = manifest_dir.join("site");

    println!("cargo:rerun-if-env-changed=MEMSTEAD_SERVE_SITE_DIST");

    // Always start from a clean `site/` (only `.gitkeep`), so a build with no
    // configured site embeds nothing and serves the placeholder — and a stale
    // mirror from a previous configured build never lingers.
    if let Err(e) = wipe_except_gitkeep(&site_dst) {
        println!("cargo:warning=failed to reset crate site/: {e}");
        return;
    }

    let Some(raw) = std::env::var_os("MEMSTEAD_SERVE_SITE_DIST") else {
        return; // no site configured — placeholder landing ships
    };
    let raw = PathBuf::from(raw);
    let site_src = if raw.is_absolute() {
        raw
    } else {
        manifest_dir.join(raw)
    };
    println!("cargo:rerun-if-changed={}", site_src.display());

    if !site_src.exists() {
        println!(
            "cargo:warning=MEMSTEAD_SERVE_SITE_DIST points at a missing path ({}); serving placeholder",
            site_src.display()
        );
        return;
    }
    if let Err(e) = copy_recursive(&site_src, &site_dst) {
        println!("cargo:warning=failed to mirror configured site into crate site/: {e}");
    }
}

/// Remove everything under `dst` except the `.gitkeep` placeholder, creating
/// the directory if absent.
fn wipe_except_gitkeep(dst: &Path) -> std::io::Result<()> {
    if !dst.exists() {
        return fs::create_dir_all(dst);
    }
    for entry in fs::read_dir(dst)? {
        let entry = entry?;
        if entry.file_name() == ".gitkeep" {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            fs::remove_dir_all(&path)?;
        } else {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

fn copy_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_recursive(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}
