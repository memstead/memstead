//! Wiki-link rewriter — shared lexical discipline with
//! [`crate::entity::parser::extract_inline_links`].
//!
//! Operates on the same masked-text model: fenced code blocks and
//! inline code spans are excluded from rewriting (matches inside them
//! remain bit-identical). Used by `Engine::rename_entity`'s
//! referrer-walk: [`rewrite_bare_slug`] rewrites same-mem
//! self-references and referrers, [`rewrite_cross_mem_slug`] rewrites
//! cross-mem referrers.

use regex::Regex;

/// Mask fenced code blocks and inline code spans with spaces of equal
/// length so byte offsets in the masked text match the original. This
/// is the offset-preserving sibling of the masking
/// [`crate::entity::parser::extract_inline_links`] performs — the
/// extractor doesn't need offset preservation because it never maps
/// back to the original text, but a rewriter does.
fn mask_for_link_scan(text: &str) -> String {
    let code_masked = crate::entity::parser::mask_code_blocks(text);
    let inline_code_re = Regex::new(r"`[^`]+`").unwrap();
    inline_code_re
        .replace_all(&code_masked, |caps: &regex::Captures<'_>| {
            " ".repeat(caps[0].len())
        })
        .into_owned()
}

/// Rewrite every same-mem `[[<old_slug>]]` (with or without a
/// `|label` suffix) in `text` to `[[<new_slug>]]`, preserving the
/// label and surrounding bytes verbatim. Wiki-links inside fenced
/// code blocks or inline code spans are not touched.
///
/// Cross-mem forms (`[[<mem>:<slug>]]`, `[[<mem>--<slug>]]`)
/// are deliberately out of scope here — the renaming entity's own
/// body uses the bare-slug form for self-references, while external
/// referrer rewriting (which has its own mem prefix shape) is
/// handled by [`rewrite_cross_mem_slug`].
///
/// Returns the rewritten text and a count of how many matches were
/// rewritten (so callers can short-circuit when nothing changed).
pub(crate) fn rewrite_bare_slug(text: &str, old_slug: &str, new_slug: &str) -> (String, usize) {
    let masked = mask_for_link_scan(text);
    let link_re = Regex::new(r"\[\[([^\]]+)\]\]").unwrap();
    let mut out = String::with_capacity(text.len());
    let mut last_end = 0usize;
    let mut rewritten = 0usize;

    for cap in link_re.captures_iter(&masked) {
        let whole = cap.get(0).unwrap();
        let inner = cap.get(1).unwrap();
        // Map the masked-text slice back to the original — offsets
        // match by construction (mask_for_link_scan preserves them).
        let inner_str = &text[inner.start()..inner.end()];
        let (target, label) = match inner_str.find('|') {
            Some(i) => (&inner_str[..i], Some(&inner_str[i + 1..])),
            None => (inner_str, None),
        };

        out.push_str(&text[last_end..whole.start()]);
        if target == old_slug {
            out.push_str("[[");
            out.push_str(new_slug);
            if let Some(lbl) = label {
                out.push('|');
                out.push_str(lbl);
            }
            out.push_str("]]");
            rewritten += 1;
        } else {
            out.push_str(&text[whole.start()..whole.end()]);
        }
        last_end = whole.end();
    }
    out.push_str(&text[last_end..]);
    (out, rewritten)
}

/// Rewrite every cross-mem wiki-link in `text` whose mem half
/// matches `old_mem` and slug half matches `old_slug`, changing the
/// slug to `new_slug` and leaving the separator (`:` or `--`) and any
/// `|label` suffix intact. Both legal cross-mem forms are handled:
///
/// - `[[<old_mem>:<old_slug>]]` → `[[<old_mem>:<new_slug>]]`
/// - `[[<old_mem>--<old_slug>]]` → `[[<old_mem>--<new_slug>]]`
///
/// Matches inside fenced code blocks and inline code spans are not
/// rewritten (same discipline as [`rewrite_bare_slug`]). Slug halves
/// that don't equal `old_slug` are left alone — this function only
/// rewrites the renamed entity's cross-mem references, not every
/// reference from `old_mem`.
///
/// Returns the rewritten text plus a count of how many matches were
/// changed.
pub(crate) fn rewrite_cross_mem_slug(
    text: &str,
    old_mem: &str,
    old_slug: &str,
    new_slug: &str,
) -> (String, usize) {
    let masked = mask_for_link_scan(text);
    let link_re = Regex::new(r"\[\[([^\]]+)\]\]").unwrap();
    let mut out = String::with_capacity(text.len());
    let mut last_end = 0usize;
    let mut rewritten = 0usize;

    for cap in link_re.captures_iter(&masked) {
        let whole = cap.get(0).unwrap();
        let inner = cap.get(1).unwrap();
        let inner_str = &text[inner.start()..inner.end()];
        let (target, label) = match inner_str.find('|') {
            Some(i) => (&inner_str[..i], Some(&inner_str[i + 1..])),
            None => (inner_str, None),
        };

        out.push_str(&text[last_end..whole.start()]);

        let rewritten_inner = match split_cross_mem_target(target) {
            Some((mem, sep, slug)) if mem == old_mem && slug == old_slug => {
                Some(format!("{old_mem}{sep}{new_slug}"))
            }
            _ => None,
        };

        if let Some(new_target) = rewritten_inner {
            out.push_str("[[");
            out.push_str(&new_target);
            if let Some(lbl) = label {
                out.push('|');
                out.push_str(lbl);
            }
            out.push_str("]]");
            rewritten += 1;
        } else {
            out.push_str(&text[whole.start()..whole.end()]);
        }
        last_end = whole.end();
    }
    out.push_str(&text[last_end..]);
    (out, rewritten)
}

/// Decompose a cross-mem wiki-link target half into
/// `(mem, separator, slug)`. Returns `None` for bare-slug forms.
///
/// Recognised separators (in order): `:` (preferred Tier-2 form),
/// `--` (the EntityId's own format, ambiguous with same-mem slugs
/// containing dashes — disambiguation is the caller's concern). The
/// `:` form wins when both could match because the parser canonicalises
/// new cross-mem wiki-links to `:`.
fn split_cross_mem_target(target: &str) -> Option<(&str, &'static str, &str)> {
    if target.contains("::") {
        // `::` is reserved syntax in the parser — don't split.
        return None;
    }
    if let Some(idx) = target.find(':') {
        let (mem, rest) = target.split_at(idx);
        let slug = &rest[1..];
        if !mem.is_empty() && !slug.is_empty() && !mem.contains('/') {
            return Some((mem, ":", slug));
        }
    }
    if let Some(idx) = target.find("--") {
        let (mem, rest) = target.split_at(idx);
        let slug = &rest[2..];
        if !mem.is_empty() && !slug.is_empty() && !mem.contains('/') {
            return Some((mem, "--", slug));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_bare_self_reference() {
        let (out, n) = rewrite_bare_slug("see [[old-slug]] for context", "old-slug", "new-slug");
        assert_eq!(out, "see [[new-slug]] for context");
        assert_eq!(n, 1);
    }

    #[test]
    fn preserves_label_on_rewrite() {
        let (out, n) = rewrite_bare_slug("[[old-slug|the link]]", "old-slug", "new-slug");
        assert_eq!(out, "[[new-slug|the link]]");
        assert_eq!(n, 1);
    }

    #[test]
    fn leaves_unrelated_links_alone() {
        let (out, n) = rewrite_bare_slug(
            "[[other]] then [[old-slug]] then [[third]]",
            "old-slug",
            "new-slug",
        );
        assert_eq!(out, "[[other]] then [[new-slug]] then [[third]]");
        assert_eq!(n, 1);
    }

    #[test]
    fn skips_matches_inside_fenced_code_block() {
        let input = "before [[old-slug]]\n```\ncode [[old-slug]]\n```\nafter [[old-slug]]";
        let (out, n) = rewrite_bare_slug(input, "old-slug", "new-slug");
        assert!(out.contains("```\ncode [[old-slug]]\n```"));
        assert!(out.starts_with("before [[new-slug]]"));
        assert!(out.ends_with("after [[new-slug]]"));
        assert_eq!(n, 2);
    }

    #[test]
    fn skips_matches_inside_inline_code() {
        let input = "prose [[old-slug]] then `code [[old-slug]] here` again [[old-slug]]";
        let (out, n) = rewrite_bare_slug(input, "old-slug", "new-slug");
        assert!(out.contains("`code [[old-slug]] here`"));
        assert_eq!(
            out,
            "prose [[new-slug]] then `code [[old-slug]] here` again [[new-slug]]"
        );
        assert_eq!(n, 2);
    }

    #[test]
    fn cross_mem_form_is_not_rewritten_by_bare_slug_pass() {
        let (out, n) = rewrite_bare_slug(
            "[[specs:old-slug]] and [[old-slug]]",
            "old-slug",
            "new-slug",
        );
        assert_eq!(out, "[[specs:old-slug]] and [[new-slug]]");
        assert_eq!(n, 1);
    }

    #[test]
    fn returns_zero_count_when_nothing_matches() {
        let (out, n) = rewrite_bare_slug("no links here", "old-slug", "new-slug");
        assert_eq!(out, "no links here");
        assert_eq!(n, 0);
    }

    #[test]
    fn cross_mem_rewrites_colon_form() {
        let (out, n) = rewrite_cross_mem_slug(
            "see [[specs:old-name]] now",
            "specs",
            "old-name",
            "new-name",
        );
        assert_eq!(out, "see [[specs:new-name]] now");
        assert_eq!(n, 1);
    }

    #[test]
    fn cross_mem_rewrites_double_hyphen_form() {
        let (out, n) =
            rewrite_cross_mem_slug("see [[specs--old-name]]", "specs", "old-name", "new-name");
        assert_eq!(out, "see [[specs--new-name]]");
        assert_eq!(n, 1);
    }

    #[test]
    fn cross_mem_preserves_label() {
        let (out, n) = rewrite_cross_mem_slug(
            "[[specs:old-name|the spec]]",
            "specs",
            "old-name",
            "new-name",
        );
        assert_eq!(out, "[[specs:new-name|the spec]]");
        assert_eq!(n, 1);
    }

    #[test]
    fn cross_mem_skips_other_mems_and_other_slugs() {
        let (out, n) = rewrite_cross_mem_slug(
            "[[memos:old-name]] [[specs:other]] [[specs:old-name]]",
            "specs",
            "old-name",
            "new-name",
        );
        assert_eq!(out, "[[memos:old-name]] [[specs:other]] [[specs:new-name]]");
        assert_eq!(n, 1);
    }

    #[test]
    fn cross_mem_skips_bare_slug() {
        let (out, n) = rewrite_cross_mem_slug(
            "[[old-name]] [[specs:old-name]]",
            "specs",
            "old-name",
            "new-name",
        );
        assert_eq!(out, "[[old-name]] [[specs:new-name]]");
        assert_eq!(n, 1);
    }

    #[test]
    fn cross_mem_skips_inside_code_block() {
        let input = "[[specs:old-name]]\n```\n[[specs:old-name]]\n```\n[[specs:old-name]]";
        let (out, n) = rewrite_cross_mem_slug(input, "specs", "old-name", "new-name");
        assert!(out.contains("```\n[[specs:old-name]]\n```"));
        assert_eq!(n, 2);
    }
}
