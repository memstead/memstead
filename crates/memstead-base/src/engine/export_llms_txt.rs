//! Render one mem as a single agent-readable Markdown document — the
//! `/llms-full.txt` shape, shared by the served endpoint and
//! `memstead export --format llms-txt`.
//!
//! The document is built to be *swallowed whole*: an agent whose goal is
//! "understand this graph fast" reads one document instead of walking the
//! graph entity by entity. That is why the shape is what it is — every
//! non-stub entity exactly once, in stable order, with its type visible and
//! its references resolved to links that work from inside the flat document.
//!
//! **This lives in the engine so the served and exported documents cannot
//! drift.** It was previously deployment-local, and matching the deployed
//! shape by copying it would have made "matches" a snapshot rather than a
//! property: two copies of a document shape is exactly how a divergence
//! starts. One renderer, two callers.
//!
//! Stubs never appear. A stub is the engine's placeholder for an unresolved
//! reference, not content, and the document's own header promises every
//! non-stub entity — so rendering one would make the header false.
//!
//! Empty sections are kept verbatim. An explicitly empty slot is signal to an
//! agent: it says the schema asks for this and nobody has answered, which is
//! different from the section not existing.

use crate::entity::EntityId;
use crate::render;

/// Deployment-supplied inputs to the document header.
///
/// Everything here is context the *renderer* cannot know: who is serving the
/// document and what else the reader should be pointed at. A CLI export has no
/// deployment identity, so these are optional rather than defaulted — a header
/// that invents an authority would be lying about provenance, which is the one
/// thing this document exists to state plainly.
#[derive(Debug, Clone, Default)]
pub struct LlmsTxtContext {
    /// The serving authority (a host). `None` for a CLI export, whose header
    /// names the mem instead — no deployment is vouching for the bytes.
    pub authority: Option<String>,
    /// Absolute link prefix (origin + any gate prefix). Empty renders entity
    /// references as the relative `entity/<id>`, which is what a document
    /// exported to a file wants.
    pub href_prefix: String,
    /// Cross-origin links the serving surface wants an agent to see. Empty
    /// omits the block entirely — a user exporting their own mem is not
    /// advertising someone else's project.
    pub wider_project: Vec<(String, String)>,
}

/// A markdown link for one entity. `[` / `]` in a title would break the link
/// text, so they are folded to parentheses rather than escaped — the title is
/// display text here, not data being round-tripped.
pub fn entity_md_link(href_prefix: &str, id: &str, title: &str) -> String {
    let text = title.replace('[', "(").replace(']', ")");
    if href_prefix.is_empty() {
        format!("[{text}](entity/{id})")
    } else {
        format!("[{text}]({href_prefix}/entity/{id})")
    }
}

/// Drop the frontmatter block the shared entity render emits. The flat
/// document surfaces the type as a visible line instead; the rest of the
/// frontmatter is agent-budget metadata that would be noise repeated once per
/// entity.
pub fn strip_frontmatter(md: &str) -> String {
    let mut lines = md.lines();
    if lines.next() == Some("---") {
        let mut closed = false;
        let mut body: Vec<&str> = Vec::new();
        for line in lines {
            if !closed && line == "---" {
                closed = true;
                continue;
            }
            if closed {
                body.push(line);
            }
        }
        if closed {
            return body.join("\n").trim_start_matches('\n').to_string();
        }
    }
    md.to_string()
}

/// Rewrite `[[…]]` wiki-links to markdown links, resolving each occurrence
/// under a three-rule precedence:
///
/// 1. **Full ids** (`[[mem--slug]]`) — unambiguous, always resolve.
/// 2. **Local bare slugs** — a slug present in the source mem binds there,
///    however many other mems reuse it. That is the engine's authoring
///    semantics: a bare wiki-link is a same-mem reference.
/// 3. **Foreign bare slugs** — a slug owned by exactly one *other* mem
///    resolves to it; a slug two foreign mems both own **stays raw text**.
///    Guessing between them would fabricate a reference the author never
///    made, and a visibly unresolved `[[slug]]` is the honest output.
///
/// **Code is never rewritten.** Resolution scans the engine's masked view —
/// the same one every other reader uses — and slices from the original, so a
/// fenced or inline code sample documenting wiki-link syntax comes out
/// byte-identical. A hand-rolled string walk without masking is exactly the
/// defect the HTML exporter was fixed for; sharing the masked view is what
/// stops this renderer reintroducing it.
///
/// `id_titles` must contain **no stubs**. A stub is an unresolved reference
/// the engine materialised, not content — and this document excludes stubs,
/// so resolving a link to one would emit a link to a page the document itself
/// does not contain. Worse, because an engine-authored bare wiki-link creates
/// a *local* stub, stubs in the map make rule 2 match every bare slug and
/// rules 3's foreign passes unreachable.
pub fn linkify_wikilinks(
    body: String,
    id_titles: &[(String, String)],
    href_prefix: &str,
    source_mem: &str,
) -> String {
    let mut by_id: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    let mut local: std::collections::HashMap<&str, (&str, &str)> = std::collections::HashMap::new();
    let mut foreign: std::collections::HashMap<&str, Option<(&str, &str)>> =
        std::collections::HashMap::new();
    for (id, title) in id_titles {
        by_id.insert(id.as_str(), title.as_str());
        if let Some((mem, slug)) = id.split_once("--") {
            if mem == source_mem {
                local.insert(slug, (id.as_str(), title.as_str()));
            } else {
                foreign
                    .entry(slug)
                    .and_modify(|e| *e = None)
                    .or_insert(Some((id.as_str(), title.as_str())));
            }
        }
    }

    let resolve = |inner: &str| -> Option<(String, String)> {
        if let Some(title) = by_id.get(inner) {
            return Some((inner.to_string(), title.to_string()));
        }
        if let Some((id, title)) = local.get(inner) {
            return Some((id.to_string(), title.to_string()));
        }
        // `Some(None)` is an ambiguous foreign slug — deliberately unresolved.
        foreign
            .get(inner)
            .copied()
            .flatten()
            .map(|(id, title)| (id.to_string(), title.to_string()))
    };

    // Offsets come from the masked view; every byte emitted comes from the
    // original, so masking can only change WHICH spans are rewritten, never
    // the bytes of the ones that are not.
    let masked = crate::markdown::mask_code_blocks_and_spans(&body);
    let mut out = String::with_capacity(body.len());
    let mut cursor = 0usize;
    let bytes = masked.as_bytes();
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == b'['
            && bytes[i + 1] == b'['
            && let Some(rel) = masked[i + 2..].find("]]")
        {
            let inner_start = i + 2;
            let inner_end = inner_start + rel;
            // The link text is read from the ORIGINAL: the masked view is
            // only a map of where code is.
            let inner = &body[inner_start..inner_end];
            // The engine's wiki-link grammar is `[[target]]`,
            // `[[target|label]]` and `[[target#Section]]` — the entity parser
            // and the strict validator both read all three. Only the TARGET
            // half resolves; an author-supplied label wins as link text,
            // because that is what the author chose a reader to see.
            //
            // Passing the whole span to the resolver instead made every
            // aliased reference miss, and land in the plain-text arm — which
            // then printed the internal id and the pipe into prose exactly
            // where the author had written a display label.
            let (target, label) = match inner.split_once('|') {
                Some((t, l)) => (t.trim(), Some(l.trim())),
                None => (inner.trim(), None),
            };
            let target = target.split('#').next().unwrap_or(target).trim();
            out.push_str(&body[cursor..i]);
            match resolve(target) {
                Some((id, title)) => {
                    out.push_str(&entity_md_link(href_prefix, &id, label.unwrap_or(&title)))
                }
                // Unresolvable: name it as PLAIN TEXT — never a link, never
                // surviving `[[…]]` syntax.
                //
                // A link would invent a target: for an ambiguous foreign slug
                // it would pick one of two arbitrarily, and for a stub target
                // it would point at a page this document deliberately
                // excludes. Leaving the brackets would put internal wiki-link
                // syntax in front of a reader the header promised a
                // self-contained document to. Plain text is the third option,
                // and the only one that is neither a guess nor a leak.
                // Print the label when the author gave one, else the target —
                // never the raw `target|label` span, which would put an
                // internal id and a pipe in front of the reader.
                None => out.push_str(label.unwrap_or(target)),
            }
            cursor = inner_end + 2;
            i = inner_end + 2;
            continue;
        }
        i += 1;
    }
    out.push_str(&body[cursor..]);
    out
}

impl crate::Engine {
    /// Render `mem` as one Markdown document in the `/llms-full.txt` shape.
    ///
    /// Refuses an unmounted mem with [`EngineError::UnknownMem`] rather than
    /// emitting an empty document — a document that says "Entities: 0" about a
    /// mem this workspace never mounted is a confident wrong answer, and the
    /// caller can tell the two apart only if the engine does.
    ///
    /// [`EngineError::UnknownMem`]: crate::engine::EngineError::UnknownMem
    pub fn render_llms_txt(
        &self,
        mem: &str,
        ctx: &LlmsTxtContext,
    ) -> Result<String, crate::engine::EngineError> {
        let mounted = self
            .mounts
            .iter()
            .find(|m| m.mount.mem == mem)
            .ok_or_else(|| crate::engine::EngineError::UnknownMem(mem.to_string()))?;

        let schema_pin = mounted
            .mount
            .schema
            .as_ref()
            .map(|s| s.as_display())
            .unwrap_or_default();
        let config = self.mem_config_for(mem);
        let subject = config
            .and_then(|c| c.description.clone())
            .unwrap_or_else(|| mem.to_string());
        // The provenance line says who vouches. With an authority that is the
        // deployment; without one it is the workspace the export came from.
        // Printing "this deployment vouches" into a file exported from a
        // laptop would make the header's one load-bearing sentence false.
        let provenance = match (self.mem_origin_class(mem), ctx.authority.is_some()) {
            (crate::render::OriginClass::FirstParty, true) => {
                "first-party (this deployment vouches for the content as its own)"
            }
            (crate::render::OriginClass::ThirdParty, true) => {
                "third-party (this deployment does not vouch for the content)"
            }
            (crate::render::OriginClass::FirstParty, false) => {
                "first-party (authored in the workspace this export came from)"
            }
            (crate::render::OriginClass::ThirdParty, false) => {
                "third-party (a read-only mount — someone else's published content)"
            }
        };

        // Every non-stub entity of THIS mem, once, in stable id order.
        let id_titles = self.entity_id_titles();
        let mut ids: Vec<String> = self
            .store
            .all_entities()
            .filter(|e| e.mem == mem && !e.stub)
            .map(|e| e.id.to_string())
            .collect();
        ids.sort();
        let count = ids.len();

        // The header names an authority only when one is serving. A CLI export
        // names the mem: no deployment is vouching for these bytes, and saying
        // otherwise would put a false provenance line at the top of the one
        // document written to be read whole and believed.
        let heading = match &ctx.authority {
            Some(a) => format!("# {a} — {subject}\n\nAuthority: {a}\n"),
            // A mem with no description falls back to its own name as the
            // subject, which would render "# srcmem — srcmem". Say it once.
            None if subject == mem => format!("# {mem}\n\nMem: {mem}\n"),
            None => format!("# {mem} — {subject}\n\nMem: {mem}\n"),
        };
        let wider = if ctx.wider_project.is_empty() {
            String::new()
        } else {
            let lines: String = ctx
                .wider_project
                .iter()
                .map(|(url, what)| format!("- {url} — {what}.\n"))
                .collect();
            // Trailing blank line: the served document has always had one
            // between the list and the closing sentence, and without it the
            // sentence becomes a lazy continuation of the last list item in
            // Markdown. A `contains`-based test cannot see the difference,
            // which is exactly why it went unnoticed.
            format!("The wider project:\n{lines}\n")
        };
        let links_sentence = if ctx.href_prefix.is_empty() {
            "Entity references are relative links to that entity's own page."
        } else {
            "Entity references are absolute links to that entity's own page."
        };

        let mut out = format!(
            "{heading}\
Subject: {subject}\n\
Schema: {schema_pin}\n\
Entities: {count}\n\
Provenance: {provenance}\n\n\
{wider}\
Every non-stub entity of this Memstead graph follows, once, with its type and \
sections. {links_sentence}\n\n\
---\n\n"
        );

        for id in &ids {
            let Some(entity) = self.get_entity(&EntityId::canonical(id)) else {
                continue;
            };
            let md = strip_frontmatter(&render::render_entity_markdown(entity, None));
            let typed = match md.split_once('\n') {
                Some((title_line, rest)) => format!(
                    "{title_line}\n\n_Type: {}_\n\n{}",
                    entity.entity_type,
                    rest.trim_start_matches('\n')
                ),
                None => format!("{md}\n\n_Type: {}_", entity.entity_type),
            };
            out.push_str(&linkify_wikilinks(typed, &id_titles, &ctx.href_prefix, mem));
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("\n---\n\n");
        }

        Ok(out)
    }

    /// `(id, title)` for every entity in the store — the lookup the link
    /// rewriter needs. Workspace-wide on purpose: a wiki-link may reach into
    /// another mounted mem, and resolving it is what makes the flat document
    /// navigable.
    fn entity_id_titles(&self) -> Vec<(String, String)> {
        // Stubs are excluded. This document omits stub entities, so linking to
        // one would point at a page the document does not contain — and since
        // an engine-authored bare wiki-link materialises a LOCAL stub, leaving
        // stubs in would make every bare slug resolve locally and the
        // foreign-slug rules unreachable.
        let mut out: Vec<(String, String)> = self
            .store
            .all_entities()
            .filter(|e| !e.stub)
            .map(|e| (e.id.to_string(), e.title.clone()))
            .collect();
        out.sort();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles() -> Vec<(String, String)> {
        vec![
            ("engine--mem".to_string(), "Mem".to_string()),
            ("flagship--mem".to_string(), "Mem".to_string()),
            ("engine--pipeline".to_string(), "Pipeline".to_string()),
        ]
    }

    /// The three passes, and their precedence. A full id always resolves; a
    /// bare slug binds mem-locally first (the engine's authoring semantics);
    /// and a slug two FOREIGN mems both own stays raw text rather than being
    /// guessed — a fabricated reference the author never made is worse than a
    /// visibly unresolved one.
    #[test]
    fn wiki_links_resolve_in_three_passes_and_never_guess() {
        let t = titles();

        // Pass 1 — full id.
        assert_eq!(
            linkify_wikilinks("See [[engine--pipeline]].".to_string(), &t, "", "engine"),
            "See [Pipeline](entity/engine--pipeline)."
        );

        // Pass 3 wins over pass 2 — the slug exists locally, so it binds
        // locally however many other mems reuse it.
        assert_eq!(
            linkify_wikilinks("A [[mem]].".to_string(), &t, "", "engine"),
            "A [Mem](entity/engine--mem)."
        );
        assert_eq!(
            linkify_wikilinks("A [[mem]].".to_string(), &t, "", "flagship"),
            "A [Mem](entity/flagship--mem)."
        );

        // Pass 2 — unique in exactly one foreign mem, so it resolves.
        assert_eq!(
            linkify_wikilinks("A [[pipeline]].".to_string(), &t, "", "flagship"),
            "A [Pipeline](entity/engine--pipeline)."
        );

        // Ambiguous across two FOREIGN mems and absent locally: named, not
        // guessed — and not left as wiki-link syntax either.
        assert_eq!(
            linkify_wikilinks("A [[mem]].".to_string(), &t, "", "plugin"),
            "A mem.",
            "an ambiguous foreign slug degrades to plain text, never a guess"
        );
    }

    /// Exactly two link forms exist, selected by whether a base is given.
    /// A third — root-relative `/entity/<id>` — must never appear: the
    /// document is read both from a file and from a served page, and only
    /// these two work in both places.
    #[test]
    fn only_two_link_forms_are_emitted() {
        let t = titles();
        let rel = linkify_wikilinks("[[engine--mem]]".to_string(), &t, "", "engine");
        assert_eq!(rel, "[Mem](entity/engine--mem)");
        assert!(!rel.contains("(/entity/"), "never root-relative: {rel}");

        let abs = linkify_wikilinks(
            "[[engine--mem]]".to_string(),
            &t,
            "https://example.com",
            "engine",
        );
        assert_eq!(abs, "[Mem](https://example.com/entity/engine--mem)");
    }

    /// A title carrying brackets would break the markdown link text, so they
    /// fold to parentheses. The link target is unaffected.
    #[test]
    fn bracketed_titles_cannot_break_the_link() {
        let t = vec![("m--x".to_string(), "A [bracketed] title".to_string())];
        assert_eq!(
            linkify_wikilinks("[[m--x]]".to_string(), &t, "", "m"),
            "[A (bracketed) title](entity/m--x)"
        );
    }

    /// Wiki-link syntax inside code is documentation, not a reference. A
    /// fenced block or inline span showing `[[slug]]` must come out
    /// byte-identical — the defect the HTML exporter was fixed for, which a
    /// hand-rolled string walk reintroduces the moment nobody checks.
    #[test]
    fn code_spans_and_fences_are_never_rewritten() {
        let t = titles();
        let body = "Prose [[engine--mem]] resolves.\n\n\
             Inline `[[engine--mem]]` does not.\n\n\
             ```\n[[engine--mem]]\n```\n";
        let out = linkify_wikilinks(body.to_string(), &t, "", "engine");

        assert!(
            out.contains("Prose [Mem](entity/engine--mem) resolves."),
            "prose still resolves: {out}"
        );
        assert!(
            out.contains("Inline `[[engine--mem]]` does not."),
            "an inline code span is left alone: {out}"
        );
        assert!(
            out.contains("```\n[[engine--mem]]\n```"),
            "a fenced block is left alone: {out}"
        );
    }

    /// The engine's grammar has three wiki-link forms and this renderer must
    /// read all of them. Only the target half resolves; an author-supplied
    /// label is what a reader sees, and a `#Section` suffix addresses within
    /// the target rather than naming a different one.
    ///
    /// Passing the whole span to the resolver made every aliased reference
    /// miss and fall to the plain-text arm — printing the internal id and the
    /// pipe into prose, precisely where the author had written a label.
    #[test]
    fn alias_and_anchor_wiki_link_forms_resolve() {
        let t = titles();

        // `[[target|label]]` — resolves, and the LABEL is the link text.
        assert_eq!(
            linkify_wikilinks("See [[engine--mem|the mem]].".to_string(), &t, "", "engine"),
            "See [the mem](entity/engine--mem)."
        );
        // `[[target#Section]]` — the anchor addresses within the target.
        assert_eq!(
            linkify_wikilinks(
                "See [[engine--mem#Identity]].".to_string(),
                &t,
                "",
                "engine"
            ),
            "See [Mem](entity/engine--mem)."
        );
        // Both at once, and on a bare slug rather than a full id.
        assert_eq!(
            linkify_wikilinks("See [[mem#Identity|here]].".to_string(), &t, "", "engine"),
            "See [here](entity/engine--mem)."
        );
        // Unresolvable WITH a label: the reader gets the label, never the
        // internal target or the pipe.
        assert_eq!(
            linkify_wikilinks("See [[ghost|that thing]].".to_string(), &t, "", "engine"),
            "See that thing."
        );
    }

    /// A reference to nothing is named in plain text — the same treatment an
    /// ambiguous foreign slug gets, for the same reason: say what could not be
    /// resolved without inventing a target, and without leaking internal
    /// wiki-link syntax into a document promised as self-contained.
    ///
    /// The one place `[[…]]` legitimately survives is inside code, which is
    /// never rewritten at all.
    #[test]
    fn an_unresolvable_reference_degrades_to_plain_text() {
        let t = titles();
        assert_eq!(
            linkify_wikilinks("See [[ghost]].".to_string(), &t, "", "engine"),
            "See ghost."
        );
        // A full id whose target is a stub is the same case: stubs are absent
        // from the map by design, so the reference cannot resolve — and this
        // is the shape the auto-generated `## Relationships` block emits.
        assert_eq!(
            linkify_wikilinks(
                "- **USES**: [[engine--phantom]]".to_string(),
                &t,
                "",
                "engine"
            ),
            "- **USES**: engine--phantom"
        );
    }

    /// Frontmatter is dropped; a body that has none is returned untouched.
    #[test]
    fn frontmatter_is_stripped_only_when_present() {
        assert_eq!(
            strip_frontmatter("---\ntype: spec\n---\n\n# Title\n\nBody."),
            "# Title\n\nBody."
        );
        assert_eq!(strip_frontmatter("# Title\n\nBody."), "# Title\n\nBody.");
        // An unterminated block is not frontmatter — returning the body
        // half-eaten would silently lose content.
        assert_eq!(strip_frontmatter("---\nno close"), "---\nno close");
    }
}
