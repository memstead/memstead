//! Re-embed trigger for the `include_dir!` builtins catalogue.
//!
//! Without this, cargo does not invalidate the compiled-in
//! `builtins/schemas/` embed when a schema *directory* is added,
//! removed, or restored — a locally stale embed then makes the
//! retention guard (`tests/builtin_retention.rs`) report a
//! present-on-disk version as absent until a forced rebuild.

fn main() {
    println!("cargo:rerun-if-changed=builtins");
}
