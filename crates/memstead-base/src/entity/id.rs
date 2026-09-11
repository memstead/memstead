//! Entity ID parsing, generation, and path mapping.

use super::EntityId;
use unicode_normalization::UnicodeNormalization;

/// Cap on the full `mem--slug` entity id (Unicode scalar length).
/// 200 leaves headroom for `mem--`-style prefixes and the `.md`
/// suffix against the 255-byte `NAME_MAX` ceiling on common
/// filesystems. The read-path validator on the MCP surface and the
/// write-path slug derivation share this constant so an entity that
/// the write side accepts is always readable on the same wire.
/// F2 + F4.
pub const ENTITY_ID_MAX_LEN: usize = 200;

/// Error cases for title→slug derivation. `title_to_slug` itself is
/// total — any title produces a slug (residual cases like
/// all-emoji collapse to a deterministic short-hash id) — so it never
/// returns these variants directly. The strict mutation-entry gate
/// [`validate_and_derive_slug`] returns them for control characters
/// and for input that would have fallen back to the hash slug;
/// [`enforce_id_length`] returns [`Self::IdTooLong`] when the
/// derived `mem--slug` exceeds the read-path length cap.
///
/// Loader and parse paths continue to call [`title_to_slug`] so
/// pre-gate entities created with the old permissive pipeline remain
/// readable.
#[derive(Debug, thiserror::Error)]
pub enum SlugError {
    /// The derived `mem--slug` id exceeds [`ENTITY_ID_MAX_LEN`]. The
    /// read-path validator rejects ids past this length, so without
    /// this guard a title that the write path accepts produces an
    /// entity that is silently unreachable on read.
    ///
    /// The bound is on the **composed id** (`<mem>--<slug>`), which is
    /// also the on-disk filename — so the budget is mem-name-dependent
    /// and the same title can be valid in a short-named mem and rejected
    /// in a longer-named one. `input` echoes that composed id (not the
    /// title) so it agrees with `length`: the payload measures one
    /// quantity, the id, end to end. `max` is [`ENTITY_ID_MAX_LEN`] so the
    /// agent can shorten by the exact delta. F2 + F4.
    #[error(
        "entity id \"{input}\" is {length} characters (max {max}); the id is `<mem>--<slug>`, so the title budget shrinks as the mem name grows — shorten the title"
    )]
    IdTooLong {
        /// The composed `<mem>--<slug>` id whose length exceeded the
        /// cap. Echoed as the `input` wire field so `input` and `length`
        /// describe the same measured quantity.
        input: String,
        length: usize,
        max: usize,
    },
    /// Strict mutation-entry rejection: the title is empty,
    /// whitespace-only, or composed exclusively of pipeline-separator
    /// characters (hyphens) so the slug pipeline would have collapsed
    /// it to a hash-fallback id. Recovery: supply a non-empty title
    /// with at least one alphanumeric character. F4.
    #[error("title is empty or contains no slug-meaningful characters")]
    TitleEmpty { input: String },
    /// Strict mutation-entry rejection: the title contains control
    /// characters (newline, tab, other C0/C1 controls). These are
    /// Unicode whitespace, so the slug pipeline silently folds them to
    /// hyphens and accepts the title — but they survive verbatim into the
    /// stored `# H1` heading, which then splits across lines so every
    /// read truncates the title at the first control char (search and
    /// `memstead_entity` see only the prefix). Refused up front with the same
    /// named-offenders + `proposed_slug` recovery shape the invalid-char
    /// guard uses. `control_chars` lists each distinct offender in source
    /// order; `proposed_slug` is the slug the pipeline would produce, for
    /// a mechanical retry with a single-line title. F8.
    #[error(
        "title {input:?} contains control character(s) {control_chars:?} that would split the stored heading — \
         retry with a single-line title (proposed slug: \"{proposed_slug}\")"
    )]
    TitleHasControlChars {
        input: String,
        control_chars: Vec<char>,
        proposed_slug: String,
    },
}

impl SlugError {
    /// Stable discriminator for the structured-details `reason` field
    /// on the `INVALID_TITLE` wire envelope. Each surface (MCP, CLI)
    /// reads this when building the response payload.
    pub fn reason(&self) -> &'static str {
        match self {
            SlugError::IdTooLong { .. } => "id_too_long",
            SlugError::TitleEmpty { .. } => "empty",
            SlugError::TitleHasControlChars { .. } => "control_chars",
        }
    }
}

/// The separator between mem and entity path in IDs.
/// Build an EntityId from mem and title.
pub fn build_id(mem: &str, title: &str) -> Result<EntityId, SlugError> {
    let slug = title_to_slug(title)?;
    let id = EntityId::new(mem, &slug);
    enforce_id_length(id.as_ref())?;
    Ok(id)
}

/// Reject ids whose Unicode scalar length exceeds
/// [`ENTITY_ID_MAX_LEN`]. Shared by [`build_id`] and the engine's
/// `create_entity` / `rename_entity` paths so the write side never
/// produces an id the read side would refuse. The cap is on the
/// composed `<mem>--<slug>` id (which is also the filename), so the
/// error echoes the id itself — the `reason`, the echoed `input`, and
/// the reported `length` all describe the id, not the title. F2 + F4.
pub fn enforce_id_length(id: &str) -> Result<(), SlugError> {
    if id.chars().count() > ENTITY_ID_MAX_LEN {
        return Err(SlugError::IdTooLong {
            input: id.to_string(),
            length: id.chars().count(),
            max: ENTITY_ID_MAX_LEN,
        });
    }
    Ok(())
}

/// Convert a title string to a kebab-case slug.
///
/// Pipeline (F1, option B+A):
///
/// 1. **NFC-normalize** so combining sequences fold into precomposed
///    forms (`Café` written NFD becomes `Café` written NFC). One
///    canonical surface form keeps slug equality byte-stable across
///    NFD-storing filesystems (older HFS+) and NFC-default ones
///    (APFS, ext4, NTFS).
/// 2. **Lowercase** via Unicode default case-folding — correct for
///    Latin / Cyrillic / Greek / Armenian; no-op for case-less
///    scripts (CJK, Arabic, Hebrew, Devanagari, Thai, etc.).
/// 3. **Whitespace → hyphen**.
/// 4. **Filter to `is_alphanumeric() || '-'`** — Unicode alphanumeric,
///    not ASCII. Keeps every Latin and non-Latin letter or digit;
///    drops combining marks, punctuation, symbols, emoji, and the
///    reserved `--` / `:` separators by construction.
/// 5. **Collapse hyphen runs, trim**.
///
/// Always returns `Ok(...)`. When the filter leaves the slug empty
/// (all-emoji titles, all-punctuation, all-symbol titles), the slug
/// degrades to a deterministic short hash of the title
/// (`entity-<8-hex>`) rather than failing. Titles that are already
/// slug-form — case-less scripts (`知識グラフ`) and lowercase
/// single-token Latin (`wohnung`) — produce slug == title, so
/// Obsidian-style `[[<title>]]` authoring round-trips without lookup
/// for exactly those titles; any other title (a capital, a space:
/// `Knowledge Graph`) derives a different slug, and the strict
/// wiki-link decoder below refuses the natural form as a link target
/// — such entities are linked by slug (`[[knowledge-graph]]`).
pub fn title_to_slug(title: &str) -> Result<String, SlugError> {
    let normalized: String = title.nfc().collect();
    let slug: String = normalized
        .chars()
        .flat_map(|c| c.to_lowercase())
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        return Ok(format!("entity-{}", short_hash(title)));
    }
    Ok(slug)
}

/// The accepted title grammar, stated as a rule. THE single sentence
/// every surface that documents titles carries: the CLI's
/// create/rename help embeds it at build time, the MCP
/// `memstead_create` / `memstead_rename` descriptions contain it
/// verbatim (a tool-surface test asserts the containment), and the
/// handbook quotes it naming this constant as its source. A
/// conformance test in this module asserts
/// [`validate_and_derive_slug`]'s behaviour matches the sentence's
/// claim — so neither the prose nor the validator can drift alone.
///
/// The sentence's two claims, executable:
///
/// ```
/// use memstead_base::entity::id::validate_and_derive_slug;
///
/// // "control characters such as tab/newline are rejected"
/// assert!(validate_and_derive_slug("two\nlines").is_err());
///
/// // "characters outside Unicode alphanumerics, whitespace, and
/// // hyphen are dropped from the derived slug" — and reported.
/// let d = validate_and_derive_slug("The Gate's Rule — v2").unwrap();
/// assert_eq!(d.slug, "the-gates-rule-v2");
/// assert_eq!(d.dropped_chars, vec!['\'', '—']);
/// ```
pub const TITLE_GRAMMAR_RULE: &str = "Titles accept any single-line text (control characters such as tab/newline are rejected); the title is stored verbatim as display text, while characters outside Unicode alphanumerics, whitespace, and hyphen are dropped from the derived slug — warning TITLE_CHARS_DROPPED_FROM_SLUG names them";

/// A strict-gate derivation result: the slug plus the distinct title
/// characters (source order, post NFC + case-fold) the pipeline
/// dropped on the way. `dropped_chars` non-empty means the title and
/// its id diverge beyond case/whitespace — the mutation surfaces it
/// as the typed `TITLE_CHARS_DROPPED_FROM_SLUG` warning so the
/// divergence stays visible without being fatal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlugDerivation {
    pub slug: String,
    pub dropped_chars: Vec<char>,
}

/// Strict slug derivation for mutation entry (`memstead_create`,
/// `memstead_rename`). Runs the same pipeline as [`title_to_slug`] —
/// byte-identical slugs for every title — but refuses the residual
/// cases the permissive variant tolerates:
///
/// 1. **Control characters.** They would survive verbatim into the
///    stored `# H1` and split it across lines. Returns
///    [`SlugError::TitleHasControlChars`].
/// 2. **Empty / collapses-to-empty.** Empty input, whitespace-only,
///    hyphen-only, or all-dropped input — anything that would force
///    the loader-path hash fallback. Returns [`SlugError::TitleEmpty`].
///
/// Any other character is admitted: the title is display text, stored
/// verbatim, and characters outside the slug alphabet are dropped from
/// the derived slug and reported in
/// [`SlugDerivation::dropped_chars`] ([`TITLE_GRAMMAR_RULE`]).
///
/// Loader paths continue to call [`title_to_slug`] so pre-gate
/// entities created with the old permissive pipeline remain
/// readable — only mutation entry runs this strict gate.
///
/// ```
/// use memstead_base::entity::id::{SlugError, validate_and_derive_slug};
///
/// // The happy path: Unicode-aware kebab-case, divergence reported.
/// let d = validate_and_derive_slug("Knowledge Graph!").unwrap();
/// assert_eq!(d.slug, "knowledge-graph");
/// assert_eq!(d.dropped_chars, vec!['!']);
///
/// // Refusal 1: control characters.
/// assert!(matches!(
///     validate_and_derive_slug("tab\there"),
///     Err(SlugError::TitleHasControlChars { .. })
/// ));
///
/// // Refusal 2: a title whose slug collapses to empty.
/// assert!(matches!(
///     validate_and_derive_slug("!!!"),
///     Err(SlugError::TitleEmpty { .. })
/// ));
/// ```
pub fn validate_and_derive_slug(title: &str) -> Result<SlugDerivation, SlugError> {
    let normalized: String = title.nfc().collect();
    let case_folded: String = normalized.chars().flat_map(|c| c.to_lowercase()).collect();

    // Control characters (newline, tab, other C0/C1) are Unicode
    // whitespace, so the slug pipeline below would fold them to hyphens
    // and accept the title — but they survive into the stored `# H1`,
    // splitting it across lines and truncating every read of the title.
    // Refuse them before the slug derivation.
    let mut control_chars: Vec<char> = Vec::new();
    for c in case_folded.chars() {
        if c.is_control() && !control_chars.contains(&c) {
            control_chars.push(c);
        }
    }
    if !control_chars.is_empty() {
        let proposed = title_to_slug(title).unwrap_or_default();
        return Err(SlugError::TitleHasControlChars {
            input: title.to_string(),
            control_chars,
            proposed_slug: proposed,
        });
    }

    // Characters outside the slug alphabet are dropped from the id —
    // recorded, not refused: the title is display text, the slug is
    // the sanitised identifier, and the divergence rides back to the
    // caller as a typed warning.
    let mut dropped_chars: Vec<char> = Vec::new();
    for c in case_folded.chars() {
        if c.is_whitespace() || c == '-' || c.is_alphanumeric() {
            continue;
        }
        if !dropped_chars.contains(&c) {
            dropped_chars.push(c);
        }
    }

    let slug: String = case_folded
        .chars()
        .filter(|c| c.is_whitespace() || *c == '-' || c.is_alphanumeric())
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    if slug.is_empty() {
        return Err(SlugError::TitleEmpty {
            input: title.to_string(),
        });
    }

    Ok(SlugDerivation {
        slug,
        dropped_chars,
    })
}

/// Deterministic 8-char hex digest used as the fallback slug when
/// the title contains no Unicode alphanumeric characters (the
/// residual case of [`title_to_slug`]'s pipeline). 32 bits is
/// plenty for collision-resistance inside a single mem; the
/// fallback only fires for titles that contain no
/// agent-meaningful characters anyway, so the opaque form is
/// acceptable. F1 (option A backstop).
fn short_hash(input: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(input.as_bytes());
    format!(
        "{:02x}{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2], digest[3]
    )
}

/// Convert a relative file path to a mem-prefixed entity ID.
///
/// `file_path_to_id("architecture/result.md", "specs")` → `specs--architecture/result`
pub fn file_path_to_id(path: &str, mem: &str) -> EntityId {
    let stripped = path.strip_suffix(".md").unwrap_or(path);
    EntityId::new(mem, stripped)
}

/// Strict wiki-link grammar refusal. Returned by [`wiki_link_to_id`]
/// when the input between `[[...]]` (after alias / `.md` strip) does
/// not resolve to a slug-form `EntityId`. Two variants matching the
/// two grammars a wiki-link target carries:
///
/// - [`Self::InvalidMemName`] — Tier-2 prefix `[[mem:slug]]`'s
///   mem name fails [`validate_mem_name_grammar`]. Recovery is
///   manual: mem names are fixed identifiers in the workspace, not
///   free-form text the agent can slugify.
/// - [`Self::InvalidTarget`] — the slug-form path fails
///   [`validate_id_path_grammar`]. Carries the
///   [`title_to_slug`]-derived suggestion (omitted when the input
///   has no meaningful slug equivalent — empty, all-punctuation,
///   all-emoji).
#[derive(Debug, thiserror::Error, Clone)]
pub enum WikiLinkError {
    #[error("mem prefix '{raw}' is not a valid mem name: {reason}")]
    InvalidMemName { raw: String, reason: String },
    #[error("wiki-link target '{raw}' is not slug-form: {reason}")]
    InvalidTarget {
        raw: String,
        suggested: Option<String>,
        reason: String,
    },
}

/// Compute the [`title_to_slug`]-derived suggestion for a malformed
/// wiki-link target. Returns `None` when the slug pipeline produces
/// either an empty result or the deterministic hash fallback
/// (`entity-<8hex>`) — both signal that the input has no canonical
/// form the agent can mechanically lift into a retry.
fn wiki_link_suggestion(raw: &str) -> Option<String> {
    let derived = title_to_slug(raw).ok()?;
    if derived.is_empty() || derived.starts_with("entity-") {
        return None;
    }
    validate_id_path_grammar(&derived)
        .is_ok()
        .then_some(derived)
}

/// Convert a wiki-link target to a mem-prefixed entity ID, refusing
/// non-slug-form inputs.
///
/// Recognises three grammars:
/// - **Tier 0** `[[<mem>--<slug>]]` — cross-mem dash-form,
///   symmetric with every engine-emitted ID: body wiki-links accept
///   the canonical `<mem>--<slug>` form the engine writes elsewhere
///   so an agent can author the same grammar in both directions.
///   `<mem>` must match the single-segment mem-name grammar
///   (`[a-z0-9-]+`, no `/`); hierarchical mem names stay on the
///   Tier-2 colon-form. Cross-mem routing is policy-gated downstream in
///   the alias-synthesis pass (same code path that already gates
///   body-link → REFERENCES emission).
/// - **Tier 1** `[[slug]]` or `[[a/b/c]]` — same-mem, resolves to
///   `<current_mem>--<slug>`.
/// - **Tier 2** `[[leaf:slug]]` — cross-mem, same mem-repo, resolves
///   to `<leaf>--<slug>`. Hierarchical paths are first-class: the
///   prefix accepts the full `team/sub-mem` form, so
///   `[[team/sub-mem:auth-service]]` resolves to
///   `team/sub-mem--auth-service`. Tier-1 with a
///   hierarchical-mem dash-prefix (`[[team/sub-mem--auth-service]]`)
///   remains unsupported — that combination is genuinely ambiguous
///   between a cross-mem reference into a hierarchical mem and a
///   same-mem entity at a hierarchical slug. Operators authoring
///   such references must use the colon Tier-2 form.
///
/// Strips `[[` / `]]`, Obsidian alias (`|display`), `../` prefixes, `.md`
/// suffix, and a redundant leading `<current_mem>--` (so an agent that
/// writes the canonical fully-qualified id `[[mem--slug]]` produces the
/// same `EntityId` as the bare-slug form `[[slug]]` instead of doubly-
/// prefixing into `mem--mem--slug`).
///
/// Strictness: any input whose Tier-2 prefix fails
/// [`validate_mem_name_grammar`] or whose resolved slug fails
/// [`validate_id_path_grammar`] refuses with [`WikiLinkError`]. There is
/// no permissive form that constructs an `EntityId` from any character
/// sequence between the brackets — callers
/// (`extract_inline_links`, the relate path's body scanners)
/// propagate the refusal so an agent's `[[Knowledge Graph]]` body
/// link can no longer land a malformed auto-stub. Read-side scanners
/// that must tolerate pre-strict on-disk drift use
/// [`wiki_link_to_id_lenient`].
///
/// Hierarchical-dash ambiguity: the Tier-1 fallback refuses inputs whose post-
/// self-prefix-strip slug contains BOTH `/` and `--`
/// (`[[team/sub-mem--target]]`). The combination is grammatically
/// ambiguous between a cross-mem reference into a hierarchical mem
/// and a same-mem entity at a hierarchical slug; the refusal carries
/// the canonical colon form (`team/sub-mem:target`) as `suggested`.
pub fn wiki_link_to_id(link: &str, current_mem: &str) -> Result<EntityId, WikiLinkError> {
    let stripped = strip_wiki_link_decorations(link);

    if !stripped.contains("::")
        && let Some(colon_idx) = stripped.find(':')
    {
        let (prefix, rest) = stripped.split_at(colon_idx);
        let slug_part = &rest[1..];
        if !prefix.is_empty() && !slug_part.is_empty() {
            if let Err(reason) = validate_mem_name_grammar(prefix) {
                return Err(WikiLinkError::InvalidMemName {
                    raw: prefix.to_string(),
                    reason,
                });
            }
            if let Err(reason) = validate_id_path_grammar(slug_part) {
                let suggested = wiki_link_suggestion(slug_part).map(|s| format!("{prefix}:{s}"));
                return Err(WikiLinkError::InvalidTarget {
                    raw: stripped.to_string(),
                    suggested,
                    reason,
                });
            }
            return Ok(EntityId::new(prefix, slug_part));
        }
    }

    // Tier 0 — cross-mem dash form `<mem>--<slug>`. Symmetric
    // with every engine-emitted ID. Recognises only
    // single-segment mem names (no `/` in the prefix); the
    // hierarchical-mem dash form is grammatically ambiguous (see
    // the dash/slash refusal further down) and stays on the colon
    // Tier-2 form. Routes to the named mem even when it differs
    // from `current_mem` — the cross-mem policy gate fires in
    // the alias-synthesis pass, not here.
    if let Some(dash_idx) = stripped.find("--") {
        let prefix = &stripped[..dash_idx];
        let suffix = &stripped[dash_idx + 2..];
        if !prefix.is_empty()
            && !suffix.is_empty()
            && !prefix.contains('/')
            && validate_mem_name_grammar(prefix).is_ok()
            && validate_id_path_grammar(suffix).is_ok()
        {
            return Ok(EntityId::new(prefix, suffix));
        }
    }

    let slug = if !current_mem.is_empty() {
        let self_prefix = format!("{current_mem}--");
        stripped
            .strip_prefix(self_prefix.as_str())
            .unwrap_or(&stripped)
    } else {
        &stripped
    };
    // A slug carrying BOTH `/` and `--` is grammatically ambiguous —
    // it could be a cross-mem reference into a hierarchical mem
    // (`team/sub-mem--target` → mem `team/sub-mem`, slug `target`)
    // or a same-mem entity at a hierarchical slug that happens to
    // contain `--`. The docstring above pins the canonical disambiguation
    // (colon-form for cross-mem) but the dash form silently collapsed
    // to the same-mem interpretation pre-fix, landing phantom stubs
    // for any agent writing `[[team/sub-mem--target]]` in body text.
    // Refuse and surface the colon-form as the recovery hint.
    if let Some(dash_idx) = slug.find("--")
        && slug[..dash_idx].contains('/')
    {
        let prefix = &slug[..dash_idx];
        let suffix = &slug[dash_idx + 2..];
        let cross_mem_form = format!("{prefix}:{suffix}");
        let same_mem_form = if current_mem.is_empty() {
            format!("<current-mem>:{slug}")
        } else {
            format!("{current_mem}:{slug}")
        };
        return Err(WikiLinkError::InvalidTarget {
            raw: stripped.to_string(),
            suggested: Some(cross_mem_form),
            reason: format!(
                "wiki-link target contains both '/' and '--', which is ambiguous \
                 between a cross-mem reference into a hierarchical mem and a \
                 same-mem entity at a hierarchical slug; use the colon form \
                 '[[{prefix}:{suffix}]]' for a cross-mem reference, or \
                 '[[{same_mem_form}]]' for a same-mem entity whose slug \
                 contains '--'"
            ),
        });
    }
    if let Err(reason) = validate_id_path_grammar(slug) {
        return Err(WikiLinkError::InvalidTarget {
            raw: stripped.to_string(),
            suggested: wiki_link_suggestion(slug),
            reason,
        });
    }
    Ok(EntityId::new(current_mem, slug))
}

/// Permissive wiki-link decoder for read-side scanners that must
/// tolerate pre-strict-gate on-disk drift (e.g. dangling-link
/// reporters, body-link scanners on stored entities, archive readers
/// for non-canonical sources). Returns an `EntityId` even for
/// non-slug-form input — non-conformant chars flow through
/// unchanged. Mutation paths MUST
/// NOT use this helper; they use [`wiki_link_to_id`] and propagate
/// the typed refusal.
pub fn wiki_link_to_id_lenient(link: &str, current_mem: &str) -> EntityId {
    // Strip decorations and trim to a FIXPOINT: each pass can expose
    // work for another — an alias/anchor cut exposes trailing
    // whitespace (`foo |label` → `foo `), and trimming that whitespace
    // can expose a `.md` suffix the pass could not see (`x.md\r` →
    // `x.md` → `x`; fuzz finding, corpus member `crash-b256aad3…`).
    // The tolerant path must land on ids the generator round-trips
    // (parse→generate is a fixpoint after one round), so it normalises
    // until nothing changes. The loop terminates because every pass
    // only ever shortens the string. Deliberately NOT in the shared
    // helper: the strict decoder runs one pass and its grammar gate
    // still refuses the leftover shapes.
    let mut stripped = strip_wiki_link_decorations(link);
    loop {
        let next = strip_wiki_link_decorations(stripped.trim_end());
        if next == stripped {
            break;
        }
        stripped = next;
    }
    let stripped = stripped.trim_end();

    if !stripped.contains("::")
        && let Some(colon_idx) = stripped.find(':')
    {
        let (prefix, rest) = stripped.split_at(colon_idx);
        let slug_part = &rest[1..];
        if !prefix.is_empty() && !slug_part.is_empty() {
            return EntityId::new(prefix, slug_part);
        }
    }

    // Tier 0 — cross-mem dash form. Read-side mirror of the strict
    // decoder's recognition so dangling-link reports and graph
    // inspectors interpret on-disk `[[other--target]]` the same way
    // the mutation gate writes it. Pre-strict drift on older entities
    // keeps the bare-slug fallback below for
    // shapes the tier-0 doesn't admit (empty prefix, hierarchical
    // prefix, malformed slug).
    if let Some(dash_idx) = stripped.find("--") {
        let prefix = &stripped[..dash_idx];
        let suffix = &stripped[dash_idx + 2..];
        if !prefix.is_empty()
            && !suffix.is_empty()
            && !prefix.contains('/')
            && validate_mem_name_grammar(prefix).is_ok()
            && validate_id_path_grammar(suffix).is_ok()
        {
            return EntityId::new(prefix, suffix);
        }
    }

    let slug = if !current_mem.is_empty() {
        let self_prefix = format!("{current_mem}--");
        stripped
            .strip_prefix(self_prefix.as_str())
            .unwrap_or(stripped)
    } else {
        stripped
    };
    EntityId::new(current_mem, slug)
}

/// Strip `[[`/`]]`, the Obsidian alias suffix `|display`, the section
/// anchor `#section` (plus any trailing `#sub` etc — stripped from the
/// first `#` onward), leading `../` segments, and the trailing `.md`
/// suffix from a raw wiki-link token. Shared by the strict and
/// lenient decoders so the pre-grammar-gate textual normalisation is
/// byte-equivalent on both paths.
///
/// `#anchor` strip: Obsidian-style section anchors are display-only at
/// the graph layer; the engine has no semantic use for them. Strip
/// from the first `#` onward so multi-anchor forms like
/// `target#a#b` collapse to `target` in one pass. Ordered after the
/// `|alias` strip so `target#section|display` correctly drops both
/// (the alias strip drops `|display` first, leaving `target#section`;
/// the anchor strip then drops `#section`).
pub(crate) fn strip_wiki_link_decorations(link: &str) -> String {
    let cleaned = link.trim_start_matches("[[").trim_end_matches("]]").trim();
    let target = match cleaned.find('|') {
        Some(i) => &cleaned[..i],
        None => cleaned,
    };
    let target_no_anchor = match target.find('#') {
        Some(i) => &target[..i],
        None => target,
    };
    let target_no_dotdot = target_no_anchor.trim_start_matches("../");
    target_no_dotdot
        .strip_suffix(".md")
        .unwrap_or(target_no_dotdot)
        .to_string()
}

/// Compute the file path for an entity given its ID and base directory.
/// The path is relative to the mem directory.
///
/// `specs--architecture/result-entity` → `architecture/result-entity.md`
pub fn id_to_file_path(id: &EntityId) -> String {
    format!("{}.md", id.path())
}

/// Validate that an `EntityId`'s path matches the wiki-link grammar
/// (`^[\p{Ll}\p{Lo}\p{Lm}\p{N}-]+(/[\p{Ll}\p{Lo}\p{Lm}\p{N}-]+)*$`).
/// Same regex the strict ingress validator applies to inline
/// `[[...]]` targets — keeping the two gates aligned ensures the
/// relate-target path doesn't admit ids that would fail an in-body
/// wiki-link parse.
///
/// Accepted character classes match what [`title_to_slug`] produces:
/// Unicode lowercase letters (`\p{Ll}`), case-less letters
/// (`\p{Lo}` — CJK, Arabic, Hebrew, Devanagari, Thai, …), modifier
/// letters (`\p{Lm}` — e.g. Japanese prolonged-sound mark `ー`),
/// any Unicode numeric (`\p{N}`), and hyphen. Mem names stay
/// ASCII — see [`validate_mem_name_grammar`]. F1 (option B+A).
///
/// Returns the original path on success, an error message on failure.
/// Callers wrap the failure into a typed envelope (e.g.
/// `INVALID_ENTITY_ID`).
pub fn validate_id_path_grammar(path: &str) -> Result<&str, String> {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(
            r"^[\p{Ll}\p{Lo}\p{Lm}\p{Mn}\p{Mc}\p{N}-]+(/[\p{Ll}\p{Lo}\p{Lm}\p{Mn}\p{Mc}\p{N}-]+)*$",
        )
        .unwrap()
    });
    if re.is_match(path) {
        Ok(path)
    } else {
        Err(format!(
            "id path '{path}' does not match the wiki-link grammar — \
             entity slugs must be lowercase Unicode letters / digits / \
             hyphens, with path segments separated by '/'"
        ))
    }
}

/// Validate a mem name (left side of `--`). Hierarchical paths are
/// first-class: mem names accept `<segment>(/<segment>)*` where each
/// segment matches the single-segment rule (`[a-z0-9-]+`). Leading slashes,
/// trailing slashes, double slashes, and any character outside the
/// allowed segment alphabet are explicit refusals.
///
/// Flat (single-segment) names work unchanged — the
/// regex's `(/<segment>)*` tail matches zero or more times. The
/// storage representation uses the full path for the
/// `__MEMSTEAD` config blob (`__MEMSTEAD:mems/<path>/config.json`), the
/// branch ref (`refs/heads/<path>`), and the in-memory router key.
pub fn validate_mem_name_grammar(mem: &str) -> Result<&str, String> {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"^[a-z0-9-]+(/[a-z0-9-]+)*$").unwrap());
    if re.is_match(mem) {
        Ok(mem)
    } else {
        Err(format!(
            "mem name '{mem}' must match ^[a-z0-9-]+(/[a-z0-9-]+)*$ \
             (lowercase ASCII / digits / hyphens, optionally segmented \
             by '/' for hierarchical layouts; no leading, trailing, or \
             double slashes)"
        ))
    }
}

/// Validate relationship type. Input is case-insensitive and canonicalised
/// to uppercase; only ASCII letters and underscores are permitted.
pub fn validate_rel_type(rel_type: &str) -> Result<String, String> {
    let cleaned = rel_type.to_uppercase();
    if cleaned.chars().all(|c| c.is_ascii_uppercase() || c == '_') && !cleaned.is_empty() {
        Ok(cleaned)
    } else {
        Err(format!(
            "Invalid relationship type: \"{rel_type}\". Only ASCII letters and underscores allowed (input is canonicalised to uppercase)."
        ))
    }
}

#[cfg(test)]
mod tests;
