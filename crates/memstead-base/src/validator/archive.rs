//! Archive-level checks over raw zip bytes.
//!
//! Single pass: walks every entry once, enforces safety + caps + the
//! file-type whitelist, yields entries to downstream modules. Returns
//! the config-file bytes separately from the markdown entries because
//! downstream dispatch differs — config goes to `validator::config`,
//! markdown goes to `validator::strict` + `parse_markdown`.

use std::io::{Cursor, Read};

use memstead_schema::{
    ARCHIVE_CONFIG_PATH, ARCHIVE_META_DIR, ARCHIVE_PROVENANCE_PATH, ARCHIVE_SCHEMA_PREFIX,
};

use super::{SizeCapKind, ValidationError, ValidatorLimits};

/// One markdown file extracted from the archive. `content` is the raw
/// UTF-8 body; BOM stripping is deferred to the strict checker so the
/// raw bytes for offset reporting stay truthful.
#[derive(Debug)]
pub struct MarkdownEntry {
    pub path: String,
    pub content: String,
}

/// One schema-source file extracted from the archive's
/// `.memstead/schema/` tree.
///
/// `archive_path` is relative to the archive root — e.g.
/// `".memstead/schema/schema.yaml"` or `".memstead/schema/types/spec.yaml"`.
/// Carried as `String` so the canonical re-pack and the
/// cache-extraction side effect can write identical bytes back out
/// (CRLF → LF normalization is applied at extract time so re-validation
/// over `canonical_bytes` is a fixpoint).
#[derive(Debug)]
pub struct SchemaFile {
    pub archive_path: String,
    pub content: String,
}

#[derive(Debug)]
pub struct ArchiveEntries {
    pub config_bytes: Vec<u8>,
    pub markdown_files: Vec<MarkdownEntry>,
    pub schema_files: Vec<SchemaFile>,
    /// Raw bytes of the optional authoring-provenance payload
    /// (`.memstead/provenance.json`), or `None` when the archive carries
    /// none. Additive: a provenance-free archive yields `None`, and an
    /// unrecognised future meta member is tolerated-and-ignored (never
    /// surfaced here, never an error).
    pub provenance_bytes: Option<Vec<u8>>,
}

/// Walk the archive, enforce archive-level rules, return entries in
/// sorted path order. Fails with a typed `ValidationError` on any
/// violation. Performs no I/O; reads from the provided byte slice.
pub fn extract_entries(
    bytes: &[u8],
    limits: &ValidatorLimits,
) -> Result<ArchiveEntries, ValidationError> {
    if bytes.len() as u64 > limits.max_compressed_archive {
        return Err(ValidationError::SizeCapExceeded {
            kind: SizeCapKind::CompressedArchive,
            got: bytes.len() as u64,
            limit: limits.max_compressed_archive,
        });
    }

    let cursor = Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| ValidationError::Zip(e.to_string()))?;

    if archive.len() as u32 > limits.max_file_count {
        return Err(ValidationError::SizeCapExceeded {
            kind: SizeCapKind::EntryCount,
            got: archive.len() as u64,
            limit: limits.max_file_count as u64,
        });
    }

    let mut config_bytes: Option<Vec<u8>> = None;
    let mut markdown_files: Vec<MarkdownEntry> = Vec::new();
    let mut schema_files: Vec<SchemaFile> = Vec::new();
    let mut provenance_bytes: Option<Vec<u8>> = None;
    let mut seen_paths: Vec<String> = Vec::new();
    let mut uncompressed_total: u64 = 0;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| ValidationError::Zip(e.to_string()))?;

        if entry.is_dir() {
            continue;
        }

        if entry.is_symlink() {
            return Err(ValidationError::Symlink(entry.name().to_string()));
        }

        // `enclosed_name()` normalizes absolute POSIX paths (`/foo`) and
        // Windows drive-letter prefixes (`C:\foo`) by stripping them
        // into innocent-looking relative paths, which masks adversarial
        // intent. Catch those shapes on the raw name first so the
        // rejection carries the original, truthful path.
        let raw_name = entry.name();
        if raw_name.starts_with('/') || raw_name.starts_with('\\') {
            return Err(ValidationError::Zip(format!(
                "unsafe entry path: {raw_name}"
            )));
        }
        let raw_bytes = raw_name.as_bytes();
        if raw_bytes.len() >= 2
            && raw_bytes[1] == b':'
            && raw_bytes[0].is_ascii_alphabetic()
        {
            return Err(ValidationError::Zip(format!(
                "unsafe entry path: {raw_name}"
            )));
        }

        let enclosed = entry.enclosed_name().ok_or_else(|| {
            ValidationError::Zip(format!("unsafe entry path: {}", entry.name()))
        })?;
        let path_string = enclosed
            .to_str()
            .ok_or_else(|| ValidationError::Zip(format!("non-UTF-8 entry path: {}", entry.name())))?
            .replace('\\', "/");

        if path_string.len() > limits.max_path_length {
            return Err(ValidationError::PathTooLong {
                path: path_string.clone(),
                len: path_string.len(),
                limit: limits.max_path_length,
            });
        }

        let depth = path_string.split('/').count();
        if depth > limits.max_path_depth {
            return Err(ValidationError::PathTooDeep {
                path: path_string.clone(),
                depth,
                limit: limits.max_path_depth,
            });
        }

        if seen_paths.iter().any(|p| p == &path_string) {
            return Err(ValidationError::DuplicateEntry(path_string));
        }

        let meta_dir_prefix = format!("{ARCHIVE_META_DIR}/");
        let is_config = path_string == ARCHIVE_CONFIG_PATH;
        let is_schema = is_schema_path(&path_string);
        let is_provenance = path_string == ARCHIVE_PROVENANCE_PATH;
        // `.md` files inside the meta dir are NOT entities — without
        // this guard a `.memstead/notes.md` would slip past the
        // whitelist as markdown.
        let is_markdown =
            path_string.ends_with(".md") && !path_string.starts_with(&meta_dir_prefix);
        // Forward-compat: a *top-level* member under the engine-owned
        // `.memstead/` meta dir that none of the recognised kinds claim is
        // an additive payload a newer writer added (the next
        // `provenance.json`-shaped file). Tolerate-and-ignore it (still
        // size-cap-enforced below, then discarded) so a future-meta-bearing
        // archive installs without error on an engine that does not
        // recognise the member. Inert: never loaded or served.
        //
        // Deliberately narrow — the existing strict boundaries stay:
        // a `.md` file inside the meta dir is still rejected (it must not
        // slip past as a non-entity), and the `.memstead/schema/` subtree
        // stays strict (an ill-formed schema member is rejected, not
        // silently ignored, so archives can't smuggle payloads under the
        // schema prefix). Future additive payloads live as new top-level
        // meta files, not under the schema subtree. Members OUTSIDE the
        // meta dir are still rejected.
        let is_ignored_meta = path_string.starts_with(&meta_dir_prefix)
            && !is_config
            && !is_schema
            && !is_provenance
            && !path_string.ends_with(".md")
            && !path_string.starts_with(ARCHIVE_SCHEMA_PREFIX);
        if !is_config && !is_markdown && !is_schema && !is_provenance && !is_ignored_meta {
            return Err(ValidationError::UnknownFile(path_string));
        }

        let per_entry_cap = if is_config {
            limits.max_config_file
        } else {
            limits.max_uncompressed_entry
        };

        let mut buf = Vec::new();
        let mut reader = (&mut entry).take(per_entry_cap + 1);
        reader
            .read_to_end(&mut buf)
            .map_err(|e| ValidationError::Zip(e.to_string()))?;

        if buf.len() as u64 > per_entry_cap {
            let kind = if is_config {
                SizeCapKind::ConfigFile
            } else {
                SizeCapKind::UncompressedEntry
            };
            return Err(ValidationError::SizeCapExceeded {
                kind,
                got: buf.len() as u64,
                limit: per_entry_cap,
            });
        }

        uncompressed_total = uncompressed_total.saturating_add(buf.len() as u64);
        if uncompressed_total > limits.max_uncompressed_archive {
            return Err(ValidationError::SizeCapExceeded {
                kind: SizeCapKind::UncompressedArchive,
                got: uncompressed_total,
                limit: limits.max_uncompressed_archive,
            });
        }

        seen_paths.push(path_string.clone());

        // An unrecognised meta member has now passed the size caps; drop
        // it (forward-compat tolerate-and-ignore) without surfacing it.
        if is_ignored_meta {
            continue;
        }

        if is_config {
            config_bytes = Some(buf);
        } else if is_provenance {
            // Raw bytes surfaced for the caller to parse into
            // `ArchiveProvenance`; the validator does not interpret the
            // payload (a malformed payload is the install path's call to
            // downgrade to "provenance absent", not an archive-shape error).
            provenance_bytes = Some(buf);
        } else {
            let content = match std::str::from_utf8(&buf) {
                Ok(s) => s.to_string(),
                Err(e) => {
                    return Err(ValidationError::Utf8 {
                        path: path_string,
                        offset: e.valid_up_to(),
                    });
                }
            };
            // Normalize CRLF → LF so every downstream pass (strict
            // checker, parse_markdown, generate_markdown, canonical
            // re-pack) sees the same bytes regardless of the
            // publisher's editor. Without this, two semantically
            // identical archives produce different canonical_bytes.
            let content = content.replace("\r\n", "\n");
            if is_schema {
                schema_files.push(SchemaFile {
                    archive_path: path_string,
                    content,
                });
            } else {
                markdown_files.push(MarkdownEntry {
                    path: path_string,
                    content,
                });
            }
        }
    }

    let config_bytes = config_bytes.ok_or(ValidationError::MissingConfig)?;

    markdown_files.sort_by(|a, b| a.path.cmp(&b.path));
    schema_files.sort_by(|a, b| a.archive_path.cmp(&b.archive_path));

    Ok(ArchiveEntries {
        config_bytes,
        markdown_files,
        schema_files,
        provenance_bytes,
    })
}

/// Recognize the archive-side layout of an embedded schema package:
/// the manifest (`.memstead/schema/schema.yaml`) and per-type files
/// (`.memstead/schema/types/<stem>.yaml`). Matching the loader's
/// on-disk layout keeps embed + extract symmetric — anything outside
/// this shape is rejected as an unknown file so archives can't smuggle
/// arbitrary payloads past the whitelist under a schema prefix.
fn is_schema_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix(ARCHIVE_SCHEMA_PREFIX) else {
        return false;
    };
    if rest == "schema.yaml" {
        return true;
    }
    let Some(rest) = rest.strip_prefix("types/") else {
        return false;
    };
    if !rest.ends_with(".yaml") {
        return false;
    }
    let stem = &rest[..rest.len() - ".yaml".len()];
    !stem.is_empty() && !stem.contains('/')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    /// Minimal archive builder for tests. Writes a DEFLATE-compressed
    /// zip with the given entries to a Vec<u8>.
    fn build_archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let cursor = Cursor::new(&mut buf);
            let mut w = zip::ZipWriter::new(cursor);
            let options = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, content) in entries {
                w.start_file(*name, options).unwrap();
                w.write_all(content).unwrap();
            }
            w.finish().unwrap();
        }
        buf
    }

    fn ok_config() -> &'static [u8] {
        br#"{"format":3,"name":"v","version":"0.1.0","schema":"default@1.0.0"}"#
    }

    #[test]
    fn accepts_minimal_valid_archive() {
        let zip = build_archive(&[
            (".memstead/config.json", ok_config()),
            ("foo.md", b"# Foo\n"),
        ]);
        let entries = extract_entries(&zip, &ValidatorLimits::DEFAULT).unwrap();
        assert_eq!(entries.markdown_files.len(), 1);
        assert_eq!(entries.markdown_files[0].path, "foo.md");
        // A provenance-free archive surfaces no provenance — absent, not
        // an error.
        assert!(entries.provenance_bytes.is_none());
    }

    /// The optional `.memstead/provenance.json` payload is recognised and
    /// surfaced verbatim on `ArchiveEntries` — not rejected as an unknown
    /// file, not mistaken for a markdown entity (it lives under the meta
    /// dir).
    #[test]
    fn recognises_provenance_member() {
        let prov = br#"{"format":1,"history":"summarised","entities":{}}"#;
        let zip = build_archive(&[
            (".memstead/config.json", ok_config()),
            (".memstead/provenance.json", prov),
            ("foo.md", b"# Foo\n"),
        ]);
        let entries = extract_entries(&zip, &ValidatorLimits::DEFAULT).unwrap();
        assert_eq!(
            entries.provenance_bytes.as_deref(),
            Some(&prov[..]),
            "provenance bytes surface verbatim"
        );
        assert_eq!(entries.markdown_files.len(), 1, "provenance is not an entity");
    }

    /// Forward-compat: an unrecognised member under the engine-owned
    /// `.memstead/` meta dir (a payload a newer writer added) is tolerated
    /// and ignored — the archive still extracts cleanly, so a future-meta-
    /// bearing archive installs without error on an engine that does not
    /// know the member. This is the additive contract the provenance
    /// payload relies on. An unknown member OUTSIDE the meta dir is still
    /// rejected.
    #[test]
    fn tolerates_unknown_meta_member_but_rejects_unknown_root_member() {
        let tolerated = build_archive(&[
            (".memstead/config.json", ok_config()),
            (".memstead/future-payload.json", br#"{"x":1}"#),
            ("foo.md", b"# Foo\n"),
        ]);
        let entries = extract_entries(&tolerated, &ValidatorLimits::DEFAULT)
            .expect("unknown meta member must be tolerated");
        assert_eq!(entries.markdown_files.len(), 1);
        assert!(
            entries.provenance_bytes.is_none(),
            "an unrecognised meta member is ignored, not surfaced as provenance"
        );

        let rejected = build_archive(&[
            (".memstead/config.json", ok_config()),
            ("stray.txt", b"not allowed at root"),
            ("foo.md", b"# Foo\n"),
        ]);
        let err = extract_entries(&rejected, &ValidatorLimits::DEFAULT).unwrap_err();
        assert!(
            matches!(err, ValidationError::UnknownFile(ref p) if p == "stray.txt"),
            "unknown non-meta member must still be rejected, got {err:?}"
        );
    }

    /// A meta member under a foreign (non-`.memstead/`) dir alongside a
    /// current `.memstead/` config is rejected — it falls outside the
    /// whitelist. Guards that only `.memstead/` is the tolerated layout.
    #[test]
    fn rejects_mixed_meta_dir_layout() {
        let zip = build_archive(&[
            (".memstead/config.json", ok_config()),
            (".other/schema/schema.yaml", b"name: default\n"),
            ("foo.md", b"# Foo\n"),
        ]);
        let err = extract_entries(&zip, &ValidatorLimits::DEFAULT).unwrap_err();
        match err {
            ValidationError::UnknownFile(path) => {
                assert!(path.starts_with(".other/"), "path={path}");
            }
            other => panic!("expected UnknownFile for the foreign meta member, got {other:?}"),
        }
    }

    /// `.md` files inside the meta dir are engine-internal, not
    /// entities — they must not pass the whitelist as markdown.
    #[test]
    fn rejects_markdown_inside_meta_dir() {
        let zip = build_archive(&[
            (".memstead/config.json", ok_config()),
            (".memstead/notes.md", b"# not an entity\n"),
        ]);
        let err = extract_entries(&zip, &ValidatorLimits::DEFAULT).unwrap_err();
        assert!(matches!(err, ValidationError::UnknownFile(_)), "got {err:?}");
    }

    #[test]
    fn markdown_files_are_sorted() {
        let zip = build_archive(&[
            (".memstead/config.json", ok_config()),
            ("z.md", b"# Z\n"),
            ("a.md", b"# A\n"),
            ("m.md", b"# M\n"),
        ]);
        let entries = extract_entries(&zip, &ValidatorLimits::DEFAULT).unwrap();
        let paths: Vec<_> = entries.markdown_files.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["a.md", "m.md", "z.md"]);
    }

    #[test]
    fn rejects_corrupt_zip() {
        let err = extract_entries(b"not a zip at all", &ValidatorLimits::DEFAULT).unwrap_err();
        assert!(matches!(err, ValidationError::Zip(_)));
    }

    #[test]
    fn rejects_missing_config() {
        let zip = build_archive(&[("foo.md", b"# Foo\n")]);
        let err = extract_entries(&zip, &ValidatorLimits::DEFAULT).unwrap_err();
        assert!(matches!(err, ValidationError::MissingConfig));
    }

    #[test]
    fn rejects_non_markdown_non_config_file() {
        let zip = build_archive(&[
            (".memstead/config.json", ok_config()),
            ("binary.exe", b"\x7fELF"),
        ]);
        let err = extract_entries(&zip, &ValidatorLimits::DEFAULT).unwrap_err();
        match err {
            ValidationError::UnknownFile(p) => assert_eq!(p, "binary.exe"),
            other => panic!("expected UnknownFile, got {other:?}"),
        }
    }

    #[test]
    fn accepts_schema_package_entries() {
        // The whitelist must admit the two shapes `export_mem` embeds:
        // the manifest at `.memstead/schema/schema.yaml` and per-type YAMLs
        // under `.memstead/schema/types/`. Anything else under
        // `.memstead/schema/` is still an unknown file (asserted separately).
        let zip = build_archive(&[
            (".memstead/config.json", ok_config()),
            ("foo.md", b"# Foo\n"),
            (".memstead/schema/schema.yaml", b"name: default\n"),
            (".memstead/schema/types/spec.yaml", b"name: spec\n"),
        ]);
        let entries = extract_entries(&zip, &ValidatorLimits::DEFAULT).unwrap();
        assert_eq!(entries.schema_files.len(), 2);
        let paths: Vec<&str> = entries
            .schema_files
            .iter()
            .map(|s| s.archive_path.as_str())
            .collect();
        assert_eq!(
            paths,
            vec![".memstead/schema/schema.yaml", ".memstead/schema/types/spec.yaml"]
        );
    }

    #[test]
    fn rejects_unknown_schema_subpath() {
        // Anything under `.memstead/schema/` that isn't the manifest or a
        // type file must still be rejected — otherwise archives could
        // smuggle arbitrary payloads behind the whitelist.
        let zip = build_archive(&[
            (".memstead/config.json", ok_config()),
            (".memstead/schema/unexpected.json", b"{}"),
        ]);
        let err = extract_entries(&zip, &ValidatorLimits::DEFAULT).unwrap_err();
        assert!(matches!(err, ValidationError::UnknownFile(_)));
    }

    #[test]
    fn rejects_nested_schema_type_file() {
        let zip = build_archive(&[
            (".memstead/config.json", ok_config()),
            (".memstead/schema/types/nested/subtype.yaml", b"name: subtype\n"),
        ]);
        let err = extract_entries(&zip, &ValidatorLimits::DEFAULT).unwrap_err();
        assert!(matches!(err, ValidationError::UnknownFile(_)));
    }

    /// An unknown root-level file (not part of the whitelist) is
    /// rejected as an unknown file — no special-casing survives.
    #[test]
    fn rejects_unknown_root_file() {
        let zip = build_archive(&[
            (".memstead/config.json", ok_config()),
            ("some-root.json", br#"{"format":3,"name":"v","version":"0.1.0"}"#),
        ]);
        let err = extract_entries(&zip, &ValidatorLimits::DEFAULT).unwrap_err();
        assert!(
            matches!(err, ValidationError::UnknownFile(ref p) if p == "some-root.json"),
            "unknown root file must be rejected as UnknownFile, got {err:?}"
        );
    }

    #[test]
    fn rejects_zip_slip() {
        let zip = build_archive(&[
            (".memstead/config.json", ok_config()),
            ("../escape.md", b"# Escape\n"),
        ]);
        let err = extract_entries(&zip, &ValidatorLimits::DEFAULT).unwrap_err();
        assert!(matches!(err, ValidationError::Zip(_)));
    }

    #[test]
    fn rejects_nested_zip_slip() {
        let zip = build_archive(&[
            (".memstead/config.json", ok_config()),
            ("subdir/../../escape.md", b"# Escape\n"),
        ]);
        let err = extract_entries(&zip, &ValidatorLimits::DEFAULT).unwrap_err();
        assert!(matches!(err, ValidationError::Zip(_)));
    }

    // Raw zip-bytes helpers below exercise rejection paths that the
    // cooperative `ZipWriter` API normalizes away or refuses to emit:
    // duplicate filenames, absolute paths, drive letters, symlinks.

    /// Minimal CRC32/ISO-HDLC — zip expects this exact polynomial in
    /// both the local file header and the central directory entry.
    /// Inlined to avoid a dev-dependency just for the raw-zip helper.
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    /// Hand-crafts a stored-only (uncompressed) zip with arbitrary
    /// entry names — including shapes that `ZipWriter` rejects or
    /// rewrites. Produces a structurally valid archive the `zip` crate
    /// can open; validator-level checks are the ones doing the rejecting.
    fn raw_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        let mut central: Vec<u8> = Vec::new();
        let mut count: u16 = 0;

        for (name, content) in entries {
            let name_bytes = name.as_bytes();
            let crc = crc32(content);
            let size = content.len() as u32;
            let offset = out.len() as u32;

            // Local file header (PK\3\4)
            out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
            out.extend_from_slice(&10u16.to_le_bytes()); // version needed
            out.extend_from_slice(&0u16.to_le_bytes()); // flags
            out.extend_from_slice(&0u16.to_le_bytes()); // stored
            out.extend_from_slice(&0u16.to_le_bytes()); // mtime
            out.extend_from_slice(&0u16.to_le_bytes()); // mdate
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes()); // compressed
            out.extend_from_slice(&size.to_le_bytes()); // uncompressed
            out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // extra length
            out.extend_from_slice(name_bytes);
            out.extend_from_slice(content);

            // Central directory file header (PK\1\2)
            central.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
            central.extend_from_slice(&20u16.to_le_bytes()); // version made by
            central.extend_from_slice(&10u16.to_le_bytes()); // version needed
            central.extend_from_slice(&0u16.to_le_bytes()); // flags
            central.extend_from_slice(&0u16.to_le_bytes()); // stored
            central.extend_from_slice(&0u16.to_le_bytes()); // mtime
            central.extend_from_slice(&0u16.to_le_bytes()); // mdate
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes()); // extra
            central.extend_from_slice(&0u16.to_le_bytes()); // comment
            central.extend_from_slice(&0u16.to_le_bytes()); // disk number
            central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
            central.extend_from_slice(&offset.to_le_bytes()); // LFH offset
            central.extend_from_slice(name_bytes);

            count += 1;
        }

        let cd_offset = out.len() as u32;
        let cd_size = central.len() as u32;
        out.extend_from_slice(&central);

        // End of central directory (PK\5\6)
        out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
        out.extend_from_slice(&0u16.to_le_bytes()); // disk number
        out.extend_from_slice(&0u16.to_le_bytes()); // disk with CD start
        out.extend_from_slice(&count.to_le_bytes()); // entries this disk
        out.extend_from_slice(&count.to_le_bytes()); // entries total
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // comment length

        out
    }

    // Duplicate-entry path: verified empirically with `raw_zip` that
    // the zip 8 reader itself collapses duplicates during central-
    // directory parse (only the last entry survives, `archive.len()`
    // returns 1). The `seen_paths` guard in `extract_entries` is
    // therefore only reachable if a future zip-crate release preserves
    // duplicates — kept as defense-in-depth, cannot be exercised in a
    // unit test against current zip 8.x.

    #[test]
    fn rejects_absolute_path_entry() {
        let zip = raw_zip(&[
            (".memstead/config.json", ok_config()),
            ("/etc/passwd.md", b"# Escape\n"),
        ]);
        let err = extract_entries(&zip, &ValidatorLimits::DEFAULT).unwrap_err();
        // `enclosed_name()` refuses absolute paths → validator maps to
        // `Zip` with the unsafe-entry reason.
        match err {
            ValidationError::Zip(reason) => {
                assert!(
                    reason.contains("unsafe entry path"),
                    "unexpected reason: {reason}"
                );
            }
            other => panic!("expected Zip(unsafe entry), got {other:?}"),
        }
    }

    #[test]
    fn rejects_windows_drive_letter_entry() {
        // Backslashes are allowed inside zip member names on Windows-
        // produced archives; `enclosed_name()` normalizes separators
        // and then refuses drive-prefixed absolute paths.
        let zip = raw_zip(&[
            (".memstead/config.json", ok_config()),
            ("C:\\evil.md", b"# Escape\n"),
        ]);
        let err = extract_entries(&zip, &ValidatorLimits::DEFAULT).unwrap_err();
        assert!(matches!(err, ValidationError::Zip(_)));
    }

    #[test]
    fn rejects_symlink_entry() {
        // The cooperative writer exposes `add_symlink`, which is the
        // only path that sets the external-attributes bit the reader
        // interprets as `is_symlink() == true`. Defense-in-depth test
        // for R5: even if an adversary smuggles a symlink past the
        // whitelist, the structural check fires.
        let mut buf: Vec<u8> = Vec::new();
        {
            let cursor = Cursor::new(&mut buf);
            let mut w = zip::ZipWriter::new(cursor);
            w.start_file(".memstead/config.json", SimpleFileOptions::default())
                .unwrap();
            w.write_all(ok_config()).unwrap();
            w.add_symlink("link.md", "target.md", SimpleFileOptions::default())
                .unwrap();
            w.finish().unwrap();
        }
        let err = extract_entries(&buf, &ValidatorLimits::DEFAULT).unwrap_err();
        match err {
            ValidationError::Symlink(name) => assert_eq!(name, "link.md"),
            other => panic!("expected Symlink, got {other:?}"),
        }
    }

    #[test]
    fn rejects_compressed_archive_too_large() {
        let mut limits = ValidatorLimits::DEFAULT;
        limits.max_compressed_archive = 64;
        let zip = build_archive(&[
            (".memstead/config.json", ok_config()),
            ("foo.md", b"# Foo\n"),
        ]);
        let err = extract_entries(&zip, &limits).unwrap_err();
        assert!(matches!(
            err,
            ValidationError::SizeCapExceeded {
                kind: SizeCapKind::CompressedArchive,
                ..
            }
        ));
    }

    #[test]
    fn rejects_single_entry_too_large() {
        let mut limits = ValidatorLimits::DEFAULT;
        limits.max_uncompressed_entry = 10;
        let zip = build_archive(&[
            (".memstead/config.json", ok_config()),
            ("big.md", &[b'x'; 100]),
        ]);
        let err = extract_entries(&zip, &limits).unwrap_err();
        assert!(matches!(
            err,
            ValidationError::SizeCapExceeded {
                kind: SizeCapKind::UncompressedEntry,
                ..
            }
        ));
    }

    #[test]
    fn rejects_config_file_too_large() {
        let mut limits = ValidatorLimits::DEFAULT;
        limits.max_config_file = 10;
        let zip = build_archive(&[
            (".memstead/config.json", &[b'x'; 50]),
            ("foo.md", b"# Foo\n"),
        ]);
        let err = extract_entries(&zip, &limits).unwrap_err();
        assert!(matches!(
            err,
            ValidationError::SizeCapExceeded {
                kind: SizeCapKind::ConfigFile,
                ..
            }
        ));
    }

    #[test]
    fn rejects_uncompressed_sum_too_large() {
        let mut limits = ValidatorLimits::DEFAULT;
        limits.max_uncompressed_archive = 30;
        let zip = build_archive(&[
            (".memstead/config.json", ok_config()),
            ("a.md", &[b'x'; 20]),
            ("b.md", &[b'x'; 20]),
        ]);
        let err = extract_entries(&zip, &limits).unwrap_err();
        assert!(matches!(
            err,
            ValidationError::SizeCapExceeded {
                kind: SizeCapKind::UncompressedArchive,
                ..
            }
        ));
    }

    #[test]
    fn rejects_entry_count_too_large() {
        let mut limits = ValidatorLimits::DEFAULT;
        limits.max_file_count = 2;
        let zip = build_archive(&[
            (".memstead/config.json", ok_config()),
            ("a.md", b"# A\n"),
            ("b.md", b"# B\n"),
        ]);
        let err = extract_entries(&zip, &limits).unwrap_err();
        assert!(matches!(
            err,
            ValidationError::SizeCapExceeded {
                kind: SizeCapKind::EntryCount,
                ..
            }
        ));
    }

    #[test]
    fn rejects_path_too_long() {
        let mut limits = ValidatorLimits::DEFAULT;
        limits.max_path_length = 10;
        let long_name = format!("{}.md", "a".repeat(20));
        let zip = build_archive(&[
            (".memstead/config.json", ok_config()),
            (long_name.as_str(), b"# x\n"),
        ]);
        let err = extract_entries(&zip, &limits).unwrap_err();
        assert!(matches!(err, ValidationError::PathTooLong { .. }));
    }

    #[test]
    fn rejects_path_too_deep() {
        let mut limits = ValidatorLimits::DEFAULT;
        limits.max_path_depth = 2;
        let zip = build_archive(&[
            (".memstead/config.json", ok_config()),
            ("a/b/c/d.md", b"# x\n"),
        ]);
        let err = extract_entries(&zip, &limits).unwrap_err();
        assert!(matches!(err, ValidationError::PathTooDeep { .. }));
    }

    #[test]
    fn rejects_non_utf8_markdown_content() {
        let zip = build_archive(&[
            (".memstead/config.json", ok_config()),
            ("bad.md", &[0xff, 0xfe, 0xff]),
        ]);
        let err = extract_entries(&zip, &ValidatorLimits::DEFAULT).unwrap_err();
        assert!(matches!(err, ValidationError::Utf8 { .. }));
    }
}
