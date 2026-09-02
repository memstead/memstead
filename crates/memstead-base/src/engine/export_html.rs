//! `export --format html` (first-author-path plan 11): one
//! self-contained HTML file per mem — the read surface for
//! non-operators. A file you hand to a person: no server, no account,
//! no installed anything.
//!
//! Hard lines:
//! - **Self-contained**: zero network requests on open. External
//!   *links* (`<a href>`) are passive and stay clickable; external
//!   *resources* (images, media) are degraded to plain links naming
//!   their target — user markdown citing a web image must not make
//!   the export dial home. Inline styling only, one file, no asset
//!   directories.
//! - **Sanitised**: raw HTML in user markdown is escaped as text —
//!   the export never embeds or executes user-supplied markup.
//! - **Deterministic** given (store, export date): entities ordered
//!   by (type, id); one date stamp; re-exports diff cleanly.
//! - **Read-only projection of one mem**: cross-mem edges render as
//!   labelled references, stubs render marked, and a read-only mount
//!   exports with its trust class stated in the identity block.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use crate::engine::Engine;
use crate::entity::Entity;
use crate::workspace::MountCapability;

/// Minimal HTML escaping for text and attribute positions.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Resolve `[[target]]` wiki-links in a section body BEFORE markdown
/// parsing: an in-mem target that exists becomes a markdown link to
/// its in-document anchor; a cross-mem target becomes a labelled
/// plain-text reference; a dangling target stays plain text (no
/// dangling anchors, mechanically guaranteed).
///
/// Links inside code blocks and inline code spans are left verbatim,
/// by the one definition every other reader uses
/// ([`crate::markdown::mask_code_blocks_and_spans`]). An export is the
/// surface most likely to carry a code sample *documenting* wiki-link
/// syntax, and rewriting one silently corrupts the sample: what a
/// renderer shows as code is what the engine treats as code, here too.
/// The scan runs over the masked copy and every slice is taken from
/// the original, so the output is byte-identical outside real links.
fn resolve_wiki_links(body: &str, mem: &str, exported_ids: &[String]) -> String {
    let masked = crate::markdown::mask_code_blocks_and_spans(body);
    let mut out = String::with_capacity(body.len());
    let mut cursor = 0usize;
    while let Some(rel) = masked[cursor..].find("[[") {
        let start = cursor + rel;
        out.push_str(&body[cursor..start]);
        let after_start = start + 2;
        match masked[after_start..].find("]]") {
            None => {
                out.push_str(&body[start..]);
                cursor = body.len();
                break;
            }
            Some(rel_end) => {
                let end = after_start + rel_end;
                let target = &body[after_start..end];
                let full_id = if target.contains("--") {
                    target.to_string()
                } else {
                    format!("{mem}--{target}")
                };
                if exported_ids.iter().any(|id| id == &full_id) {
                    // In-document anchor (markdown link keeps the
                    // rendering pipeline uniform).
                    let _ = write!(out, "[{target}](#{full_id})");
                } else if full_id.starts_with(&format!("{mem}--")) {
                    // Dangling in-mem reference — plain text, marked.
                    let _ = write!(out, "{target} *(unresolved)*");
                } else {
                    // Cross-mem reference — labelled, never an anchor.
                    let _ = write!(out, "{full_id} *(other mem)*");
                }
                cursor = end + 2;
            }
        }
    }
    out.push_str(&body[cursor..]);
    out
}

/// Percent-decode a URL fragment for comparison against raw entity
/// ids (pulldown-cmark percent-encodes non-ASCII hrefs; the `id`
/// attributes we emit stay raw UTF-8 — browsers decode fragments, so
/// equality must too).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether a link destination may render as a clickable `<a href>`.
/// Passive web links (http/https/mailto) stay clickable; a fragment
/// link is allowed only when it resolves to an exported entity id (no
/// dangling in-document anchors, mechanically); every other scheme —
/// `javascript:`, `data:`, `file:`, vendor schemes — is neutralised
/// to text: the handed-over file must never execute user-supplied
/// script, not even on click.
fn link_dest_allowed(dest: &str, exported_ids: &[String]) -> bool {
    if let Some(frag) = dest.strip_prefix('#') {
        let decoded = percent_decode(frag);
        return exported_ids.iter().any(|id| id == &decoded);
    }
    let lower = dest.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:")
}

/// Render one markdown body to sanitised HTML: raw HTML becomes
/// escaped text, images (external or not) degrade to plain links
/// naming their target, link destinations outside the allowed set
/// (http/https/mailto/resolvable fragments) neutralise to plain text,
/// and everything else renders through the CommonMark machinery the
/// engine already carries.
fn markdown_to_safe_html(md: &str, exported_ids: &[String]) -> String {
    // The engine's one reader dialect plus the extensions that are
    // rendering-only. Derived from `parser_options()` rather than
    // rebuilt, so a flag added to the reader reaches the renderer too
    // — an independently constructed `Options` here is how two
    // referees start.
    let options = crate::markdown::parser_options() | Options::ENABLE_STRIKETHROUGH;
    let parser = Parser::new_ext(md, options);
    let mut events: Vec<Event> = Vec::new();
    let mut skipping_image: Option<(String, String)> = None; // (url, alt)
    // Depth-tracked suppression of disallowed links: the wrapper is
    // dropped, inner text stays, the destination surfaces as text.
    let mut suppressed_link: Option<String> = None;
    for ev in parser {
        if let Some((_url, alt)) = skipping_image.as_mut() {
            match ev {
                Event::End(TagEnd::Image) => {
                    let (url, alt) = skipping_image.take().unwrap();
                    let label = if alt.trim().is_empty() {
                        format!("image: {url}")
                    } else {
                        format!("image: {alt} ({url})")
                    };
                    if link_dest_allowed(&url, exported_ids) {
                        events.push(Event::Start(Tag::Link {
                            link_type: pulldown_cmark::LinkType::Inline,
                            dest_url: url.clone().into(),
                            title: "".into(),
                            id: "".into(),
                        }));
                        events.push(Event::Text(label.into()));
                        events.push(Event::End(TagEnd::Link));
                    } else {
                        // Disallowed scheme on an image: text only.
                        events.push(Event::Text(format!("[{label}]").into()));
                    }
                }
                Event::Text(t) => alt.push_str(&t),
                _ => {}
            }
            continue;
        }
        match ev {
            // Raw HTML never passes through — escaped as visible text.
            Event::Html(s) | Event::InlineHtml(s) => {
                events.push(Event::Text(s));
            }
            // Images are resources: degrade to a labelled passive link.
            Event::Start(Tag::Image { dest_url, .. }) => {
                skipping_image = Some((dest_url.to_string(), String::new()));
            }
            Event::Start(Tag::Link { dest_url, .. }) if suppressed_link.is_none() => {
                let dest = dest_url.to_string();
                if link_dest_allowed(&dest, exported_ids) {
                    events.push(Event::Start(Tag::Link {
                        link_type: pulldown_cmark::LinkType::Inline,
                        dest_url: dest.into(),
                        title: "".into(),
                        id: "".into(),
                    }));
                } else {
                    suppressed_link = Some(dest);
                }
            }
            Event::End(TagEnd::Link) if suppressed_link.is_some() => {
                let dest = suppressed_link.take().unwrap();
                events.push(Event::Text(format!(" ({dest} — link removed)").into()));
            }
            other => events.push(other),
        }
    }
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, events.into_iter());
    html
}

impl Engine {
    /// Render one mem as a single self-contained HTML document.
    /// `export_date` is an ISO date stamped once in the identity
    /// block — the only environmental input besides the store.
    pub fn render_html_export(
        &self,
        mem: &str,
        export_date: &str,
    ) -> Result<String, crate::engine::EngineError> {
        self.render_html_export_scoped(mem, export_date, None)
    }

    /// [`Self::render_html_export`] reduced to a chain: only the mem's
    /// entities in `chain` are rendered (stubs in the chain listed as
    /// such), links to entities outside it render as unresolved markers,
    /// and the identity block names the chain. `None` is the whole mem,
    /// byte-identical to the unscoped export.
    pub fn render_html_export_scoped(
        &self,
        mem: &str,
        export_date: &str,
        chain: Option<&crate::graph::chain::ChainSet>,
    ) -> Result<String, crate::engine::EngineError> {
        let in_scope = |e: &Entity| chain.is_none_or(|c| c.contains(&e.id));
        let mounted = self
            .mounts
            .iter()
            .find(|m| m.mount.mem == mem)
            .ok_or_else(|| crate::engine::EngineError::UnknownMem(mem.to_string()))?;
        let third_party = mounted.mount.capability == MountCapability::ReadOnly;
        let config = self.mem_config_for(mem);
        // The mem's schema, for the declared section headings below. A
        // mem whose schema did not resolve still exports — every
        // section falls back to its key rather than the export failing.
        let schema = self.schemas.get(mem);
        let schema_ref = self
            .schemas
            .get(mem)
            .map(|s| {
                let (n, v) = s.id();
                format!("{n}@{v}")
            })
            .unwrap_or_else(|| "(unresolved)".to_string());

        // Entities of this mem, deterministic order: (type, id).
        // Stubs are collected separately and rendered marked.
        let mut entities: Vec<&Entity> = self
            .store
            .all_entities()
            .filter(|e| e.mem == mem && !e.stub && in_scope(e))
            .collect();
        entities.sort_by(|a, b| {
            a.entity_type
                .cmp(&b.entity_type)
                .then_with(|| a.id.as_ref().cmp(b.id.as_ref()))
        });
        let mut stubs: Vec<&Entity> = self
            .store
            .all_entities()
            .filter(|e| e.mem == mem && e.stub && in_scope(e))
            .collect();
        stubs.sort_by(|a, b| a.id.as_ref().cmp(b.id.as_ref()));
        let exported_ids: Vec<String> = entities.iter().map(|e| e.id.to_string()).collect();

        let mut out = String::new();
        out.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
        out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
        let doc_title = config
            .and_then(|c| c.title.clone())
            .unwrap_or_else(|| mem.to_string());
        let _ = writeln!(out, "<title>{}</title>", esc(&doc_title));
        out.push_str(
            "<style>\n\
             body{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;\
             max-width:52rem;margin:0 auto;padding:2rem 1rem;line-height:1.55;color:#1a1a1a;}\n\
             h1{border-bottom:2px solid #ddd;padding-bottom:.3rem;}\n\
             section.entity{border-top:1px solid #ddd;margin-top:2rem;padding-top:1rem;}\n\
             table.meta{border-collapse:collapse;font-size:.9rem;margin:.5rem 0;}\n\
             table.meta td{border:1px solid #ddd;padding:.15rem .5rem;}\n\
             table.meta td:first-child{color:#555;}\n\
             nav ul{columns:2;list-style:none;padding-left:0;}\n\
             nav li{margin:.15rem 0;}\n\
             .identity{background:#f6f6f6;border:1px solid #ddd;padding:.75rem 1rem;\
             border-radius:4px;font-size:.95rem;}\n\
             .badge{display:inline-block;background:#eee;border-radius:3px;\
             padding:0 .4rem;font-size:.8rem;color:#555;}\n\
             .stub{color:#888;font-style:italic;}\n\
             .reltable{font-size:.9rem;}\n\
             @media print{nav ul{columns:1;}}\n\
             </style>\n</head>\n<body>\n",
        );

        // Identity block.
        let _ = write!(
            out,
            "<h1>{}</h1>\n<div class=\"identity\">\n",
            esc(&doc_title)
        );
        let _ = writeln!(out, "<div><strong>Mem:</strong> {}</div>", esc(mem));
        if let Some(desc) = config.and_then(|c| c.description.as_deref())
            && !desc.is_empty()
        {
            let _ = writeln!(
                out,
                "<div><strong>Description:</strong> {}</div>",
                esc(desc)
            );
        }
        if let Some(subject) = config.and_then(|c| c.subject.as_ref()) {
            let _ = writeln!(
                out,
                "<div><strong>Subject:</strong> {}</div>",
                esc(&subject.scope)
            );
        }
        let _ = writeln!(
            out,
            "<div><strong>Schema:</strong> {}</div>",
            esc(&schema_ref)
        );
        let trust = if third_party {
            "third-party (read-only mount — someone else's published content, quoted here)"
        } else {
            "first-party (writable mem of this workspace)"
        };
        let _ = writeln!(out, "<div><strong>Origin:</strong> {trust}</div>");
        if let Some(chain) = chain {
            let _ = writeln!(
                out,
                "<div><strong>Chain:</strong> {} — only the entities reachable from the root are \
                 rendered; links to entities outside the chain are marked unresolved</div>",
                esc(&chain.describe())
            );
        }
        let _ = write!(
            out,
            "<div><strong>Exported:</strong> {} · {} entities</div>\n</div>\n",
            esc(export_date),
            entities.len()
        );

        // Type-grouped navigation index.
        let mut by_type: BTreeMap<&str, Vec<&Entity>> = BTreeMap::new();
        for e in &entities {
            by_type.entry(e.entity_type.as_str()).or_default().push(e);
        }
        out.push_str("<nav>\n<h2>Index</h2>\n");
        for (ty, list) in &by_type {
            let _ = write!(out, "<h3>{} ({})</h3>\n<ul>\n", esc(ty), list.len());
            for e in list {
                let _ = writeln!(
                    out,
                    "<li><a href=\"#{}\">{}</a></li>",
                    esc(e.id.as_ref()),
                    esc(&e.title)
                );
            }
            out.push_str("</ul>\n");
        }
        out.push_str("</nav>\n");

        // Entities.
        for e in &entities {
            let _ = write!(
                out,
                "<section class=\"entity\" id=\"{}\">\n<h2>{}</h2>\n<span class=\"badge\">{}</span> <span class=\"badge\">{}</span>\n",
                esc(e.id.as_ref()),
                esc(&e.title),
                esc(&e.entity_type),
                esc(e.id.as_ref()),
            );
            if !e.metadata.is_empty() {
                out.push_str("<table class=\"meta\">\n");
                for (k, v) in &e.metadata {
                    let _ = writeln!(
                        out,
                        "<tr><td>{}</td><td>{}</td></tr>",
                        esc(k),
                        esc(&v.to_frontmatter_string())
                    );
                }
                out.push_str("</table>\n");
            }
            for (key, body) in &e.sections {
                if body.trim().is_empty() {
                    continue;
                }
                // Show the heading the schema author declared, not the
                // engine's storage key. This export is the one artifact
                // handed to somebody with nothing installed, and the
                // declared heading is the only place an author gets to
                // control how their model reads to an outsider —
                // rendering `summary` where they wrote `Summary` was
                // the export path reaching for the field nearest to
                // hand, never a decision.
                //
                // The key still governs identity elsewhere (anchors are
                // derived from entity ids, and stay untouched), so
                // display and stability stay separable.
                let heading = schema
                    .and_then(|s| s.get_type(&e.entity_type))
                    .and_then(|t| {
                        t.sections
                            .iter()
                            .find(|s| &s.key == key)
                            .map(|s| s.heading.clone())
                    })
                    .unwrap_or_else(|| key.clone());
                let _ = writeln!(out, "<h3>{}</h3>", esc(&heading));
                let resolved = resolve_wiki_links(body, mem, &exported_ids);
                out.push_str(&markdown_to_safe_html(&resolved, &exported_ids));
            }
            if !e.relationships.is_empty() {
                // `Relationships` as the engine writes it on disk, not the
                // lowercase slot name. This block is auto-managed rather
                // than schema-declared, so it has no `heading` to read —
                // but it sits beside headings that now carry the author's
                // words, and was the last storage-flavoured one left.
                out.push_str("<h3>Relationships</h3>\n<ul class=\"reltable\">\n");
                let mut rels = e.relationships.clone();
                rels.sort_by(|a, b| {
                    a.rel_type
                        .cmp(&b.rel_type)
                        .then_with(|| a.target.as_ref().cmp(b.target.as_ref()))
                });
                for r in &rels {
                    let target_id = r.target.to_string();
                    let in_doc = exported_ids.iter().any(|id| id == &target_id);
                    let is_stub_target = self.store.get(&r.target).map(|t| t.stub).unwrap_or(false);
                    if in_doc {
                        let _ = writeln!(
                            out,
                            "<li>{} → <a href=\"#{}\">{}</a></li>",
                            esc(&r.rel_type),
                            esc(&target_id),
                            esc(&target_id)
                        );
                    } else if is_stub_target {
                        let _ = writeln!(
                            out,
                            "<li>{} → <span class=\"stub\">{} (stub — unresolved reference)</span></li>",
                            esc(&r.rel_type),
                            esc(&target_id)
                        );
                    } else {
                        let _ = writeln!(
                            out,
                            "<li>{} → {} <span class=\"badge\">other mem</span></li>",
                            esc(&r.rel_type),
                            esc(&target_id)
                        );
                    }
                }
                out.push_str("</ul>\n");
            }
            out.push_str("</section>\n");
        }

        if !stubs.is_empty() {
            out.push_str(
                "<section class=\"entity\">\n<h2>Unresolved references (stubs)</h2>\n<ul>\n",
            );
            for s in &stubs {
                let _ = writeln!(
                    out,
                    "<li class=\"stub\">{} — referenced but never written</li>",
                    esc(s.id.as_ref())
                );
            }
            out.push_str("</ul>\n</section>\n");
        }

        out.push_str("</body>\n</html>\n");
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    use crate::backend::MemBackend;
    use crate::engine::test_helpers::{cli_actor, folder_mount};
    use crate::storage::FilesystemMemWriter;
    use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

    /// The export scan uses the one definition of "not a link": a
    /// `[[…]]` inside a code block or inline span is a code sample, not
    /// a reference, and an export is the surface most likely to carry
    /// one — a page documenting wiki-link syntax. Rewriting it
    /// corrupts the sample it is trying to show.
    #[test]
    fn wiki_link_resolution_leaves_code_verbatim() {
        let exported = vec!["m--real".to_string()];
        for body in [
            "```\n[[m--real]]\n```",
            "~~~\n[[m--real]]\n~~~",
            "    [[m--real]]",
            "> ```\n> [[m--real]]\n> ```",
            "An inline `[[m--real]]` sample.",
            "A double ``[[m--real]]`` sample.",
        ] {
            assert_eq!(
                resolve_wiki_links(body, "m", &exported),
                body,
                "code content must survive byte-identical: {body:?}"
            );
        }
    }

    /// …and a dangling target inside code is not marked either — the
    /// `*(unresolved)*` suffix is just as much a corruption.
    #[test]
    fn wiki_link_resolution_does_not_mark_code_as_unresolved() {
        let body = "```\n[[m--ghost]]\n```\n\n    [[m--other-ghost]]\n";
        assert_eq!(resolve_wiki_links(body, "m", &[]), body);
    }

    /// Complement: prose links on either side of a code block still
    /// resolve exactly as before — anchor, unresolved marker, and
    /// cross-mem label alike.
    #[test]
    fn wiki_link_resolution_still_rewrites_prose() {
        let exported = vec!["m--real".to_string()];
        let body =
            "See [[m--real]].\n\n```\n[[m--real]]\n```\n\nAnd [[m--ghost]] and [[other--thing]].\n";
        let out = resolve_wiki_links(body, "m", &exported);
        assert!(out.contains("[m--real](#m--real)"), "{out}");
        assert!(out.contains("m--ghost *(unresolved)*"), "{out}");
        assert!(out.contains("other--thing *(other mem)*"), "{out}");
        assert!(
            out.contains("```\n[[m--real]]\n```"),
            "code untouched: {out}"
        );
    }

    /// An unterminated `[[` still passes through verbatim rather than
    /// truncating the body — the offset walk must not lose the tail.
    #[test]
    fn wiki_link_resolution_passes_through_an_unterminated_open() {
        let body = "text [[not-closed and more text after\n";
        assert_eq!(resolve_wiki_links(body, "m", &[]), body);
    }

    /// Pre-boot on-disk fixture: the renderer is a read surface, so
    /// the fixture is written as existing markdown (including a
    /// cross-mem wiki-link and hostile content that the write path
    /// would gate) and the engine boots over it.
    fn fixture_engine(tmp: &TempDir) -> Engine {
        let mem_dir = tmp.path().to_path_buf();
        std::fs::write(
            mem_dir.join("bösenberg-söhne-rev-21.md"),
            "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Bösenberg & Söhne — Rev. 2.1\n\n## Identity\n\nCited in [[target-entity]] and [[über-ziele]] and cross-mem [[other--far-away]].\n\n<script>alert('x')</script>\n\n![diagram](https://evil.example/x.png)\n\nSee [docs](https://example.org/page), [broken](#bogus-frag), [evil](javascript:alert(2)).\n\n## Purpose\n\nZweck mit Umlauten: äöüß.\n",
        )
        .unwrap();
        std::fs::write(
            mem_dir.join("über-ziele.md"),
            "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Über-Ziele\n\n## Identity\n\nUmlaut-slug anchor target.\n\n## Purpose\n\nP.\n",
        )
        .unwrap();
        std::fs::write(
            mem_dir.join("target-entity.md"),
            "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Target Entity\n\n## Identity\n\nThe link target.\n\n## Purpose\n\nAnchors resolve here.\n",
        )
        .unwrap();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap()
    }

    /// Every `href="#..."` in the document must point at an existing
    /// `id="..."` — the no-dangling-anchor complement, mechanical.
    fn assert_no_dangling_anchors(html: &str) {
        let mut ids: Vec<&str> = Vec::new();
        for part in html.split("id=\"").skip(1) {
            if let Some(end) = part.find('"') {
                ids.push(&part[..end]);
            }
        }
        for part in html.split("href=\"#").skip(1) {
            if let Some(end) = part.find('"') {
                let anchor = percent_decode(&part[..end]);
                assert!(
                    ids.iter().any(|id| *id == anchor),
                    "dangling in-document anchor #{anchor}"
                );
            }
        }
    }

    /// The export shows the heading the schema author declared, not
    /// the engine's storage key.
    ///
    /// The load-bearing fixture is `out_of_scope` → `Out of Scope`: the
    /// interior word stays lowercase, so no capitalisation rule
    /// reconstructs it from the key. (`current_state` → `Current State`
    /// is also asserted, but title-casing the key would produce it, so
    /// on its own it would not have caught a renderer that guessed.)
    /// That is the point of the finding: the declared heading is the
    /// only place an author controls how their model reads to someone
    /// who cannot see the markdown, and guessing is not reading.
    ///
    /// Anchors are unaffected: they derive from entity ids, and the
    /// no-dangling-anchor sweep runs here too.
    #[test]
    fn html_export_renders_the_declared_heading_not_the_section_key() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        std::fs::write(
            mem_dir.join("open-question.md"),
            "---\ntype: inquiry\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\n\
             status: open\nurgency: medium\n---\n# Open Question\n\n## Question\n\nQ?\n\n\
             ## Significance\n\nS.\n\n## Current State\n\nWhere things stand.\n",
        )
        .unwrap();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();

        let html = engine.render_html_export("specs", "2026-08-15").unwrap();

        assert!(
            html.contains("<h3>Current State</h3>"),
            "must render the declared heading; got:\n{html}"
        );
        assert!(
            !html.contains("<h3>current_state</h3>"),
            "must not render the storage key as a heading; got:\n{html}"
        );
        // The capitalised-only cases come along for free.
        assert!(html.contains("<h3>Question</h3>"), "got:\n{html}");
        assert!(html.contains("<h3>Significance</h3>"), "got:\n{html}");
        assert_no_dangling_anchors(&html);

        // The case a capitalisation rule cannot fake: `out_of_scope` is
        // declared `Out of Scope`, interior word lowercase. A renderer
        // that title-cased the key would emit "Out Of Scope" and fail
        // here — which is what makes this the load-bearing assertion.
        let tmp2 = TempDir::new().unwrap();
        let goal_dir = tmp2.path().to_path_buf();
        std::fs::write(
            goal_dir.join("second-goal.md"),
            "---\ntype: goal\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\n\
             priority: high\nstatus: active\n---\n# Second Goal\n\n## Statement\n\nS.\n\n\
             ## Rationale\n\nR.\n\n## Success Criteria\n\nC.\n\n## Out of Scope\n\n\
             Everything else.\n",
        )
        .unwrap();
        let goal_writer = FilesystemMemWriter::new(goal_dir.clone());
        let goal_mount = Mount {
            mem: "plans".to_string(),
            schema: Some(memstead_schema::SchemaRef::new(
                "planning",
                semver::Version::new(0, 4, 0),
            )),
            storage: MountStorage::Folder { path: goal_dir },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let goal_engine = Engine::from_mounts(vec![(
            goal_mount,
            Box::new(goal_writer) as Box<dyn MemBackend>,
        )])
        .unwrap();
        let goal_html = goal_engine
            .render_html_export("plans", "2026-08-15")
            .unwrap();
        assert!(
            goal_html.contains("<h3>Out of Scope</h3>"),
            "the interior word must stay lowercase, as declared; got:\n{goal_html}"
        );
        assert!(
            !goal_html.contains("<h3>Out Of Scope</h3>")
                && !goal_html.contains("<h3>out_of_scope</h3>"),
            "neither a title-cased guess nor the storage key; got:\n{goal_html}"
        );

        // Byte-deterministic given store and export date.
        let again = engine.render_html_export("specs", "2026-08-15").unwrap();
        assert_eq!(html, again, "export must be byte-deterministic");
    }

    /// Criteria 1–4 over one fixture: rendering, sanitisation,
    /// resource degradation, anchors, determinism, localized change.
    #[test]
    fn html_export_renders_sanitises_and_stays_self_contained() {
        let tmp = TempDir::new().unwrap();
        let mut engine = fixture_engine(&tmp);

        let html = engine.render_html_export("specs", "2026-08-10").unwrap();

        // Identity block + index + entities.
        assert!(
            html.contains("<strong>Mem:</strong> specs"),
            "identity block"
        );
        assert!(html.contains("<strong>Exported:</strong> 2026-08-10"));
        assert!(html.contains("<nav>"), "type-grouped index");
        assert!(
            html.contains("Bösenberg &amp; Söhne — Rev. 2.1"),
            "widened title escaped: {html}"
        );
        assert!(html.contains("äöüß"), "umlauts verbatim");

        // Sanitisation: no script tag survives; the text is escaped.
        assert!(!html.contains("<script>"), "raw HTML must not pass through");
        assert!(html.contains("&lt;script&gt;"), "escaped as visible text");

        // External image degraded to a passive link; external links stay.
        assert!(!html.contains("<img"), "no image element: {html}");
        assert!(
            html.contains("<a href=\"https://evil.example/x.png\">image: diagram (https://evil.example/x.png)</a>"),
            "image degraded to labelled link: {html}"
        );
        assert!(html.contains("<a href=\"https://example.org/page\">docs</a>"));

        // Wiki-links: in-mem → anchor; cross-mem → labelled, no anchor.
        assert!(
            html.contains("href=\"#specs--target-entity\""),
            "in-doc anchor"
        );
        assert!(html.contains("other--far-away"), "cross-mem labelled");
        assert!(
            !html.contains("href=\"#other--far-away\""),
            "cross-mem never an anchor"
        );

        // Umlaut-slug wiki-link: pulldown percent-encodes the href;
        // the checker decodes, so this is the form that used to be
        // untested.
        assert!(html.contains("ber-ziele"), "umlaut target linked: {html}");

        // User fragment link to nowhere: neutralised to text — never
        // a dangling anchor.
        assert!(
            !html.contains("href=\"#bogus-frag\""),
            "dangling fragment neutralised"
        );
        assert!(html.contains("(#bogus-frag — link removed)"), "{html}");

        // javascript: scheme: never a clickable href — even on click,
        // the handed-over file must not execute user script.
        assert!(
            !html.contains("href=\"javascript:"),
            "javascript scheme stripped: {html}"
        );
        assert!(
            html.contains("link removed"),
            "neutralised destination surfaced"
        );
        assert_no_dangling_anchors(&html);

        // Stub marking: the cross-mem auto-stub never lands in a
        // folder mem without policy — but an in-mem stub does.
        // (See the relationships list: targets that are stubs are
        // marked; asserted in the wiki-link block above via absence.)

        // Zero external resources: nothing in the markup fetches.
        for fetching in [
            "<img",
            "<video",
            "<audio",
            "<iframe",
            "<link ",
            "<script src",
            "@import",
            "url(",
        ] {
            assert!(
                !html.contains(fetching),
                "self-containment violated by {fetching}"
            );
        }

        // Determinism: same store + date → same bytes.
        let again = engine.render_html_export("specs", "2026-08-10").unwrap();
        assert_eq!(html, again, "byte-deterministic");

        // Localized change: edit one entity; the untouched entity's
        // section block stays byte-identical.
        let untouched_block = {
            let start = html.find("id=\"specs--bösenberg-söhne-rev-21\"").unwrap();
            let end = html[start..].find("</section>").unwrap() + start;
            html[start..end].to_string()
        };
        let (actor, client) = cli_actor();
        let mut edit = crate::engine::UpdateEntityArgs {
            anchors: Vec::new(),
            id: crate::entity::EntityId::new("specs", "target-entity"),
            expected_hash: None,
            sections: indexmap::IndexMap::from_iter([(
                "purpose".to_string(),
                "Geändert.".to_string(),
            )]),
            append_sections: indexmap::IndexMap::new(),
            patch_sections: indexmap::IndexMap::new(),
            sections_unset: Vec::new(),
            metadata: indexmap::IndexMap::new(),
            metadata_unset: Vec::new(),
            declare_relations: Vec::new(),
            dry_run: false,
            relations_unset: Vec::new(),
            anchors_unset: Vec::new(),
        };
        let _ = &mut edit;
        engine
            .update_entity(edit, actor, Some(&client), None)
            .expect("edit lands");
        let after = engine.render_html_export("specs", "2026-08-10").unwrap();
        assert_ne!(html, after, "edit changes the export");
        assert!(
            after.contains(&untouched_block),
            "untouched entity's region byte-identical after the edit"
        );
        assert!(after.contains("Geändert."), "edited content present");
    }

    /// Criterion 5: a read-only mount exports with its trust class in
    /// the identity block. Criterion 6's refusal parity: unknown mem
    /// refuses UNKNOWN_MEM like the other formats.
    #[test]
    fn read_only_origin_stated_and_unknown_mem_refuses() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        std::fs::write(
            mem_dir.join("note.md"),
            "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# Foreign Note\n\n## Identity\n\nI.\n\n## Purpose\n\nP.\n",
        )
        .unwrap();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mount = Mount {
            mem: "foreign".to_string(),
            schema: Some("default@1.0.0".parse().unwrap()),
            storage: MountStorage::Folder { path: mem_dir },
            capability: MountCapability::ReadOnly,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let engine =
            Engine::from_mounts(vec![(mount, Box::new(writer) as Box<dyn MemBackend>)]).unwrap();
        let html = engine.render_html_export("foreign", "2026-08-10").unwrap();
        assert!(
            html.contains("third-party (read-only mount"),
            "trust class stated: {html}"
        );

        let err = engine.render_html_export("nope", "2026-08-10").unwrap_err();
        assert_eq!(err.code(), "UNKNOWN_MEM");
    }
}
