//! Entity ID parsing, generation, and path mapping.

use super::EntityId;
use unicode_normalization::UnicodeNormalization;

/// Cap on the full `vault--slug` entity id (Unicode scalar length).
/// 200 leaves headroom for `vault--`-style prefixes and the `.md`
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
/// [`validate_and_derive_slug`] returns them when the input would have
/// fallen back to the hash slug or would have had characters dropped;
/// [`enforce_id_length`] returns [`Self::IdTooLong`] when the
/// derived `vault--slug` exceeds the read-path length cap.
///
/// Loader and parse paths continue to call [`title_to_slug`] so
/// pre-gate entities created with the old permissive pipeline remain
/// readable.
#[derive(Debug, thiserror::Error)]
pub enum SlugError {
    /// The derived `vault--slug` id exceeds [`ENTITY_ID_MAX_LEN`]. The
    /// read-path validator rejects ids past this length, so without
    /// this guard a title that the write path accepts produces an
    /// entity that is silently unreachable on read.
    ///
    /// The bound is on the **composed id** (`<vault>--<slug>`), which is
    /// also the on-disk filename — so the budget is vault-name-dependent
    /// and the same title can be valid in a short-named vault and rejected
    /// in a longer-named one. `input` echoes that composed id (not the
    /// title) so it agrees with `length`: the payload measures one
    /// quantity, the id, end to end. `max` is [`ENTITY_ID_MAX_LEN`] so the
    /// agent can shorten by the exact delta. F2 + F4.
    #[error(
        "entity id \"{input}\" is {length} characters (max {max}); the id is `<vault>--<slug>`, so the title budget shrinks as the vault name grows — shorten the title"
    )]
    IdTooLong {
        /// The composed `<vault>--<slug>` id whose length exceeded the
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
    /// Strict mutation-entry rejection: one or more characters in the
    /// title would have been dropped by the slug pipeline (anything
    /// that isn't Unicode alphanumeric, whitespace, or hyphen after
    /// NFC + case-fold). `invalid_chars` lists each distinct offending
    /// character in source order; `proposed_slug` is the slug the
    /// permissive pipeline would have produced, suitable for a
    /// mechanical retry against a sanitised title. F10 + F19.
    #[error(
        "title \"{input}\" contains character(s) that the slug pipeline drops: {invalid_chars:?} — \
         retry with a sanitised title (proposed slug: \"{proposed_slug}\")"
    )]
    TitleHasInvalidChars {
        input: String,
        invalid_chars: Vec<char>,
        proposed_slug: String,
    },
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
            SlugError::TitleHasInvalidChars { .. } => "invalid_chars",
            SlugError::TitleHasControlChars { .. } => "control_chars",
        }
    }
}

/// The separator between vault and entity path in IDs.
/// Build an EntityId from vault and title.
pub fn build_id(vault: &str, title: &str) -> Result<EntityId, SlugError> {
    let slug = title_to_slug(title)?;
    let id = EntityId::new(vault, &slug);
    enforce_id_length(id.as_ref())?;
    Ok(id)
}

/// Reject ids whose Unicode scalar length exceeds
/// [`ENTITY_ID_MAX_LEN`]. Shared by [`build_id`] and the engine's
/// `create_entity` / `rename_entity` paths so the write side never
/// produces an id the read side would refuse. The cap is on the
/// composed `<vault>--<slug>` id (which is also the filename), so the
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
/// (`entity-<8-hex>`) rather than failing. Common titles still
/// produce slug == title in their native script — Obsidian-style
/// `[[<title>]]` wiki-link authoring round-trips without lookup.
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

/// Strict slug derivation for mutation entry (`memstead_create`,
/// `memstead_rename`). Runs the same pipeline as [`title_to_slug`] but
/// rejects two residual cases the permissive variant tolerates:
///
/// 1. **Empty / collapses-to-empty.** Empty input, whitespace-only,
///    or hyphen-only input — anything that would force the loader-
///    path hash fallback. Returns [`SlugError::TitleEmpty`].
/// 2. **Character drops.** Any character that the pipeline would
///    strip (non-alphanumeric, non-whitespace, non-hyphen after NFC +
///    Unicode case-fold) causes the mutation to refuse with
///    [`SlugError::TitleHasInvalidChars`] carrying the offending
///    characters and the pipeline's proposed slug for a mechanical
///    retry.
///
/// Loader paths continue to call [`title_to_slug`] so pre-gate
/// entities created with the old permissive pipeline remain
/// readable — only mutation entry runs this strict gate.
pub fn validate_and_derive_slug(title: &str) -> Result<String, SlugError> {
    let normalized: String = title.nfc().collect();
    let case_folded: String = normalized.chars().flat_map(|c| c.to_lowercase()).collect();

    // Control characters (newline, tab, other C0/C1) are Unicode
    // whitespace, so the slug pipeline below would fold them to hyphens
    // and accept the title — but they survive into the stored `# H1`,
    // splitting it across lines and truncating every read of the title.
    // Refuse them before the slug derivation, ahead of the invalid-char
    // check so the more specific control-char condition is reported.
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

    let mut invalid_chars: Vec<char> = Vec::new();
    for c in case_folded.chars() {
        if c.is_whitespace() || c == '-' || c.is_alphanumeric() {
            continue;
        }
        if !invalid_chars.contains(&c) {
            invalid_chars.push(c);
        }
    }

    if !invalid_chars.is_empty() {
        let proposed = title_to_slug(title).unwrap_or_default();
        return Err(SlugError::TitleHasInvalidChars {
            input: title.to_string(),
            invalid_chars,
            proposed_slug: proposed,
        });
    }

    let slug: String = case_folded
        .chars()
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

    Ok(slug)
}

/// Deterministic 8-char hex digest used as the fallback slug when
/// the title contains no Unicode alphanumeric characters (the
/// residual case of [`title_to_slug`]'s pipeline). 32 bits is
/// plenty for collision-resistance inside a single vault; the
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

/// Convert a relative file path to a vault-prefixed entity ID.
///
/// `file_path_to_id("architecture/result.md", "specs")` → `specs--architecture/result`
pub fn file_path_to_id(path: &str, vault: &str) -> EntityId {
    let stripped = path.strip_suffix(".md").unwrap_or(path);
    EntityId::new(vault, stripped)
}

/// Strict wiki-link grammar refusal. Returned by [`wiki_link_to_id`]
/// when the input between `[[...]]` (after alias / `.md` strip) does
/// not resolve to a slug-form `EntityId`. Two variants matching the
/// two grammars a wiki-link target carries:
///
/// - [`Self::InvalidVaultName`] — Tier-2 prefix `[[vault:slug]]`'s
///   vault name fails [`validate_vault_name_grammar`]. Recovery is
///   manual: vault names are fixed identifiers in the workspace, not
///   free-form text the agent can slugify.
/// - [`Self::InvalidTarget`] — the slug-form path fails
///   [`validate_id_path_grammar`]. Carries the
///   [`title_to_slug`]-derived suggestion (omitted when the input
///   has no meaningful slug equivalent — empty, all-punctuation,
///   all-emoji).
#[derive(Debug, thiserror::Error, Clone)]
pub enum WikiLinkError {
    #[error("vault prefix '{raw}' is not a valid vault name: {reason}")]
    InvalidVaultName { raw: String, reason: String },
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
    validate_id_path_grammar(&derived).is_ok().then_some(derived)
}

/// Convert a wiki-link target to a vault-prefixed entity ID, refusing
/// non-slug-form inputs.
///
/// Recognises three grammars:
/// - **Tier 0** `[[<vault>--<slug>]]` — cross-vault dash-form,
///   symmetric with every engine-emitted ID: body wiki-links accept
///   the canonical `<vault>--<slug>` form the engine writes elsewhere
///   so an agent can author the same grammar in both directions.
///   `<vault>` must match the single-segment vault-name grammar
///   (`[a-z0-9-]+`, no `/`); hierarchical vault names stay on the
///   Tier-2 colon-form. Cross-vault routing is policy-gated downstream in
///   the alias-synthesis pass (same code path that already gates
///   body-link → REFERENCES emission).
/// - **Tier 1** `[[slug]]` or `[[a/b/c]]` — same-vault, resolves to
///   `<current_vault>--<slug>`.
/// - **Tier 2** `[[leaf:slug]]` — cross-vault, same vault-repo, resolves
///   to `<leaf>--<slug>`. Hierarchical paths are first-class: the
///   prefix accepts the full `team/sub-vault` form, so
///   `[[team/sub-vault:auth-service]]` resolves to
///   `team/sub-vault--auth-service`. Tier-1 with a
///   hierarchical-vault dash-prefix (`[[team/sub-vault--auth-service]]`)
///   remains unsupported — that combination is genuinely ambiguous
///   between a cross-vault reference into a hierarchical vault and a
///   same-vault entity at a hierarchical slug. Operators authoring
///   such references must use the colon Tier-2 form.
///
/// Strips `[[` / `]]`, Obsidian alias (`|display`), `../` prefixes, `.md`
/// suffix, and a redundant leading `<current_vault>--` (so an agent that
/// writes the canonical fully-qualified id `[[vault--slug]]` produces the
/// same `EntityId` as the bare-slug form `[[slug]]` instead of doubly-
/// prefixing into `vault--vault--slug`).
///
/// Strictness: any input whose Tier-2 prefix fails
/// [`validate_vault_name_grammar`] or whose resolved slug fails
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
/// (`[[team/sub-vault--target]]`). The combination is grammatically
/// ambiguous between a cross-vault reference into a hierarchical vault
/// and a same-vault entity at a hierarchical slug; the refusal carries
/// the canonical colon form (`team/sub-vault:target`) as `suggested`.
pub fn wiki_link_to_id(link: &str, current_vault: &str) -> Result<EntityId, WikiLinkError> {
    let stripped = strip_wiki_link_decorations(link);

    if !stripped.contains("::")
        && let Some(colon_idx) = stripped.find(':')
    {
        let (prefix, rest) = stripped.split_at(colon_idx);
        let slug_part = &rest[1..];
        if !prefix.is_empty() && !slug_part.is_empty() {
            if let Err(reason) = validate_vault_name_grammar(prefix) {
                return Err(WikiLinkError::InvalidVaultName {
                    raw: prefix.to_string(),
                    reason,
                });
            }
            if let Err(reason) = validate_id_path_grammar(slug_part) {
                let suggested = wiki_link_suggestion(slug_part)
                    .map(|s| format!("{prefix}:{s}"));
                return Err(WikiLinkError::InvalidTarget {
                    raw: stripped.to_string(),
                    suggested,
                    reason,
                });
            }
            return Ok(EntityId::new(prefix, slug_part));
        }
    }

    // Tier 0 — cross-vault dash form `<vault>--<slug>`. Symmetric
    // with every engine-emitted ID. Recognises only
    // single-segment vault names (no `/` in the prefix); the
    // hierarchical-vault dash form is grammatically ambiguous (see
    // the dash/slash refusal further down) and stays on the colon
    // Tier-2 form. Routes to the named vault even when it differs
    // from `current_vault` — the cross-vault policy gate fires in
    // the alias-synthesis pass, not here.
    if let Some(dash_idx) = stripped.find("--") {
        let prefix = &stripped[..dash_idx];
        let suffix = &stripped[dash_idx + 2..];
        if !prefix.is_empty()
            && !suffix.is_empty()
            && !prefix.contains('/')
            && validate_vault_name_grammar(prefix).is_ok()
            && validate_id_path_grammar(suffix).is_ok()
        {
            return Ok(EntityId::new(prefix, suffix));
        }
    }

    let slug = if !current_vault.is_empty() {
        let self_prefix = format!("{current_vault}--");
        stripped.strip_prefix(self_prefix.as_str()).unwrap_or(&stripped)
    } else {
        &stripped
    };
    // A slug carrying BOTH `/` and `--` is grammatically ambiguous —
    // it could be a cross-vault reference into a hierarchical vault
    // (`team/sub-vault--target` → vault `team/sub-vault`, slug `target`)
    // or a same-vault entity at a hierarchical slug that happens to
    // contain `--`. The docstring above pins the canonical disambiguation
    // (colon-form for cross-vault) but the dash form silently collapsed
    // to the same-vault interpretation pre-fix, landing phantom stubs
    // for any agent writing `[[team/sub-vault--target]]` in body text.
    // Refuse and surface the colon-form as the recovery hint.
    if let Some(dash_idx) = slug.find("--")
        && slug[..dash_idx].contains('/')
    {
        let prefix = &slug[..dash_idx];
        let suffix = &slug[dash_idx + 2..];
        let cross_vault_form = format!("{prefix}:{suffix}");
        let same_vault_form = if current_vault.is_empty() {
            format!("<current-vault>:{slug}")
        } else {
            format!("{current_vault}:{slug}")
        };
        return Err(WikiLinkError::InvalidTarget {
            raw: stripped.to_string(),
            suggested: Some(cross_vault_form),
            reason: format!(
                "wiki-link target contains both '/' and '--', which is ambiguous \
                 between a cross-vault reference into a hierarchical vault and a \
                 same-vault entity at a hierarchical slug; use the colon form \
                 '[[{prefix}:{suffix}]]' for a cross-vault reference, or \
                 '[[{same_vault_form}]]' for a same-vault entity whose slug \
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
    Ok(EntityId::new(current_vault, slug))
}

/// Permissive wiki-link decoder for read-side scanners that must
/// tolerate pre-strict-gate on-disk drift (e.g. dangling-link
/// reporters, body-link scanners on stored entities, archive readers
/// for non-canonical sources). Returns an `EntityId` even for
/// non-slug-form input — non-conformant chars flow through
/// unchanged. Mutation paths MUST
/// NOT use this helper; they use [`wiki_link_to_id`] and propagate
/// the typed refusal.
pub fn wiki_link_to_id_lenient(link: &str, current_vault: &str) -> EntityId {
    let stripped = strip_wiki_link_decorations(link);

    if !stripped.contains("::")
        && let Some(colon_idx) = stripped.find(':')
    {
        let (prefix, rest) = stripped.split_at(colon_idx);
        let slug_part = &rest[1..];
        if !prefix.is_empty() && !slug_part.is_empty() {
            return EntityId::new(prefix, slug_part);
        }
    }

    // Tier 0 — cross-vault dash form. Read-side mirror of the strict
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
            && validate_vault_name_grammar(prefix).is_ok()
            && validate_id_path_grammar(suffix).is_ok()
        {
            return EntityId::new(prefix, suffix);
        }
    }

    let slug = if !current_vault.is_empty() {
        let self_prefix = format!("{current_vault}--");
        stripped.strip_prefix(self_prefix.as_str()).unwrap_or(&stripped)
    } else {
        &stripped
    };
    EntityId::new(current_vault, slug)
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
fn strip_wiki_link_decorations(link: &str) -> String {
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
/// The path is relative to the vault directory.
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
/// any Unicode numeric (`\p{N}`), and hyphen. Vault names stay
/// ASCII — see [`validate_vault_name_grammar`]. F1 (option B+A).
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

/// Validate a vault name (left side of `--`). Hierarchical paths are
/// first-class: vault names accept `<segment>(/<segment>)*` where each
/// segment matches the single-segment rule (`[a-z0-9-]+`). Leading slashes,
/// trailing slashes, double slashes, and any character outside the
/// allowed segment alphabet are explicit refusals.
///
/// Flat (single-segment) names work unchanged — the
/// regex's `(/<segment>)*` tail matches zero or more times. The
/// storage representation uses the full path for the
/// `__MEMSTEAD` config blob (`__MEMSTEAD:vaults/<path>/config.json`), the
/// branch ref (`refs/heads/<path>`), and the in-memory router key.
pub fn validate_vault_name_grammar(vault: &str) -> Result<&str, String> {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"^[a-z0-9-]+(/[a-z0-9-]+)*$").unwrap());
    if re.is_match(vault) {
        Ok(vault)
    } else {
        Err(format!(
            "vault name '{vault}' must match ^[a-z0-9-]+(/[a-z0-9-]+)*$ \
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
mod tests {
    use super::*;

    /// Vault-name grammar accepts hierarchical paths and refuses
    /// malformations. Flat (single-segment) names continue to work —
    /// the regex's `(/<segment>)*` tail matches zero or more times.
    #[test]
    fn validate_vault_name_grammar_accepts_hierarchical_paths() {
        // Flat layouts (regression).
        assert!(validate_vault_name_grammar("specs").is_ok());
        assert!(validate_vault_name_grammar("my-vault").is_ok());
        assert!(validate_vault_name_grammar("v1").is_ok());
        // Hierarchical layouts.
        assert!(validate_vault_name_grammar("team/sub-vault").is_ok());
        assert!(validate_vault_name_grammar("a/b/c/d").is_ok());
        assert!(validate_vault_name_grammar("planning/2026-q1").is_ok());
    }

    /// Grammar refusals are explicit. Each malformation
    /// case (`/team`, `team/`, `team//sub`,
    /// uppercase / underscore / dot) returns an `Err`.
    #[test]
    fn validate_vault_name_grammar_refuses_malformations() {
        // Leading slash.
        assert!(validate_vault_name_grammar("/team/sub").is_err());
        // Trailing slash.
        assert!(validate_vault_name_grammar("team/sub/").is_err());
        // Double slash.
        assert!(validate_vault_name_grammar("team//sub").is_err());
        // Empty.
        assert!(validate_vault_name_grammar("").is_err());
        // Uppercase.
        assert!(validate_vault_name_grammar("Team/Sub").is_err());
        // Underscore (not in allowed alphabet).
        assert!(validate_vault_name_grammar("team_sub").is_err());
        assert!(validate_vault_name_grammar("team/sub_vault").is_err());
        // Dot.
        assert!(validate_vault_name_grammar("team.sub").is_err());
        // Space.
        assert!(validate_vault_name_grammar("team sub").is_err());
    }

    #[test]
    fn title_to_slug_basic() {
        assert_eq!(title_to_slug("My Entity").unwrap(), "my-entity");
        assert_eq!(title_to_slug("My  Entity  Name").unwrap(), "my-entity-name");
    }

    /// F1 (B+A) behaviour change: precomposed Latin diacritics are
    /// preserved in the slug rather than transliterated to ASCII.
    /// `Große Änderung` was `grosse-aenderung` pre-F1; it is now
    /// `große-änderung`. Same applies to `naïve`, `Café résumé`,
    /// `Łódź`, etc. — slug matches title in every script the
    /// Unicode `is_alphanumeric` predicate accepts.
    #[test]
    fn title_to_slug_german() {
        assert_eq!(title_to_slug("Große Änderung").unwrap(), "große-änderung");
        assert_eq!(title_to_slug("Björn").unwrap(), "björn");
    }

    #[test]
    fn title_to_slug_diacritics() {
        assert_eq!(title_to_slug("Café résumé").unwrap(), "café-résumé");
        assert_eq!(title_to_slug("naïve").unwrap(), "naïve");
    }

    #[test]
    fn title_to_slug_special_chars() {
        assert_eq!(title_to_slug("Hello, World!").unwrap(), "hello-world");
        assert_eq!(title_to_slug("--leading--trailing--").unwrap(), "leading-trailing");
    }

    #[test]
    fn title_to_slug_polish() {
        assert_eq!(title_to_slug("Łódź").unwrap(), "łódź");
    }

    /// F1 (B+A): CJK titles round-trip cleanly. No transliteration,
    /// no hash — the slug equals the title.
    #[test]
    fn title_to_slug_cjk() {
        assert_eq!(title_to_slug("日本語のタイトル").unwrap(), "日本語のタイトル");
        // Spaces still collapse to hyphens.
        assert_eq!(title_to_slug("中文 標題").unwrap(), "中文-標題");
        // Mixed CJK + Latin + digits.
        assert_eq!(title_to_slug("Project 日本 v2").unwrap(), "project-日本-v2");
    }

    /// F1 (B+A): cased non-Latin scripts (Cyrillic, Greek, Armenian)
    /// case-fold to lowercase the same way Latin does.
    #[test]
    fn title_to_slug_cyrillic() {
        assert_eq!(title_to_slug("Москва").unwrap(), "москва");
        assert_eq!(title_to_slug("Москва-проект").unwrap(), "москва-проект");
        assert_eq!(title_to_slug("ПРОЕКТ ПЛАН").unwrap(), "проект-план");
    }

    /// F1 (B+A): Right-to-left scripts. Hebrew and Arabic letters
    /// are `\p{Lo}` (case-less); they pass through unchanged.
    /// Hebrew niqqud and Arabic harakat are `\p{Mn}` (nonspacing
    /// marks) carrying the Unicode `Other_Alphabetic` property, so
    /// Rust's `is_alphanumeric` treats them as alphabetic and the
    /// slug filter keeps them — wiki-link round-trip is exact for
    /// titles that include vowelization. (The wiki-link regex
    /// accepts the wider `\p{Mn}`/`\p{Mc}` class for the same
    /// reason; see `slug_path_regex` in `validator/strict.rs`.)
    #[test]
    fn title_to_slug_rtl() {
        // Hebrew with niqqud — niqqud is preserved; spaces become hyphens.
        assert_eq!(title_to_slug("תַּפְקִיד עברי").unwrap(), "תַּפְקִיד-עברי");
        // Arabic with harakat — harakat preserved (same Other_Alphabetic property).
        assert_eq!(title_to_slug("مَرْحَبًا").unwrap(), "مَرْحَبًا");
        // Plain Hebrew without vowelization (the more common case)
        // round-trips letter-for-letter.
        assert_eq!(title_to_slug("שלום עולם").unwrap(), "שלום-עולם");
    }

    /// F1 (option A backstop): titles whose pipeline yields an
    /// empty slug fall through to a deterministic short-hash id
    /// rather than failing. Covers all-emoji, all-symbol,
    /// all-punctuation, and empty/whitespace inputs.
    #[test]
    fn title_to_slug_residual_falls_back_to_hash() {
        // All emoji.
        let emoji = title_to_slug("🚀✨").unwrap();
        assert!(emoji.starts_with("entity-"), "got {emoji}");
        assert_eq!(emoji.len(), "entity-".len() + 8);
        // Same input always produces same hash (deterministic).
        assert_eq!(emoji, title_to_slug("🚀✨").unwrap());
        // Different inputs produce different hashes.
        assert_ne!(emoji, title_to_slug("🌟").unwrap());

        // Empty / whitespace / punctuation-only all hit the same path.
        assert!(title_to_slug("").unwrap().starts_with("entity-"));
        assert!(title_to_slug("   ").unwrap().starts_with("entity-"));
        assert!(title_to_slug("\t\n").unwrap().starts_with("entity-"));
        assert!(title_to_slug("---").unwrap().starts_with("entity-"));
        assert!(title_to_slug("!!!").unwrap().starts_with("entity-"));
        assert!(title_to_slug("!?.,;").unwrap().starts_with("entity-"));
    }

    /// NFC normalization is load-bearing for cross-platform safety:
    /// a `Café` written NFD (`Cafe` + combining-acute U+0301) and
    /// one written NFC (single codepoint U+00E9) must produce the
    /// same slug. Pre-F1 the pipeline NFD-decomposed and stripped
    /// combining marks, yielding `cafe` for both — that path is
    /// gone, so the NFC normalization step is what holds the
    /// invariant now.
    #[test]
    fn title_to_slug_nfc_normalization() {
        let nfc = "Café";              // single-codepoint é
        let nfd = "Cafe\u{0301}";      // e + combining acute
        assert_ne!(nfc, nfd, "NFC and NFD forms must differ at the byte level");
        assert_eq!(
            title_to_slug(nfc).unwrap(),
            title_to_slug(nfd).unwrap(),
            "NFC and NFD inputs must produce the same slug",
        );
    }

    /// F4: the strict mutation-entry gate rejects empty titles with
    /// `TitleEmpty` so the wire envelope can carry `reason: "empty"`
    /// rather than silently producing a hash-fallback slug.
    #[test]
    fn validate_and_derive_slug_rejects_empty() {
        // `"\t\n"` is no longer here: it contains control characters, so
        // the more specific control-char guard fires first (see
        // `validate_and_derive_slug_rejects_control_chars`). These cases
        // hold no control chars and collapse to an empty slug.
        for empty in ["", "   ", "---", " - - - ", "-"] {
            let err = validate_and_derive_slug(empty).unwrap_err();
            let SlugError::TitleEmpty { input } = err else {
                panic!("expected TitleEmpty for {empty:?}, got {err:?}");
            };
            assert_eq!(input, empty);
        }
    }

    /// F10 + F19: any character the permissive pipeline would drop
    /// (emoji, punctuation, math/currency symbols, path separators)
    /// causes the strict gate to refuse with the offending chars and
    /// a `proposed_slug` for mechanical retry.
    #[test]
    fn validate_and_derive_slug_rejects_invalid_chars() {
        let cases: &[(&str, &[char], &str)] = &[
            ("Hello, World!", &[',', '!'], "hello-world"),
            ("Café — résumé", &['—'], "café-résumé"),
            ("🚀 launch", &['🚀'], "launch"),
            ("price € 100", &['€'], "price-100"),
            ("../escape", &['.', '/'], "escape"),
            ("path/to/entity", &['/'], "pathtoentity"),
            ("a\\b", &['\\'], "ab"),
        ];
        for (title, expected_invalid, expected_proposed) in cases {
            let err = validate_and_derive_slug(title).unwrap_err();
            let SlugError::TitleHasInvalidChars {
                input,
                invalid_chars,
                proposed_slug,
            } = err
            else {
                panic!("expected TitleHasInvalidChars for {title:?}, got {err:?}");
            };
            assert_eq!(input, *title);
            assert_eq!(invalid_chars, *expected_invalid, "title={title:?}");
            assert_eq!(proposed_slug, *expected_proposed, "title={title:?}");
        }
    }

    /// F8: control characters (newline, tab,
    /// carriage return, other C0 controls) are refused with
    /// `TitleHasControlChars` rather than silently folded to hyphens —
    /// they would otherwise split the stored `# H1` and truncate every
    /// read of the title. The proposed slug is the single-line form.
    #[test]
    fn validate_and_derive_slug_rejects_control_chars() {
        let cases: &[(&str, &[char], &str)] = &[
            ("Tab\tand\nnewline title", &['\t', '\n'], "tab-and-newline-title"),
            ("line\rreturn", &['\r'], "line-return"),
            ("null\u{0}byte", &['\u{0}'], "nullbyte"),
        ];
        for (title, expected_control, expected_proposed) in cases {
            let err = validate_and_derive_slug(title).unwrap_err();
            let SlugError::TitleHasControlChars {
                input,
                control_chars,
                proposed_slug,
            } = err
            else {
                panic!("expected TitleHasControlChars for {title:?}, got {err:?}");
            };
            assert_eq!(input, *title);
            assert_eq!(control_chars, *expected_control, "title={title:?}");
            assert_eq!(proposed_slug, *expected_proposed, "title={title:?}");
        }
    }

    /// A plain space is whitespace but NOT a control character, so it
    /// must keep folding to a hyphen (the control-char guard does not
    /// narrow ordinary whitespace handling).
    #[test]
    fn validate_and_derive_slug_space_is_not_control() {
        assert_eq!(validate_and_derive_slug("a b c").unwrap(), "a-b-c");
    }

    /// Success path — titles whose every character survives the
    /// pipeline round-trip cleanly produce the same slug as
    /// `title_to_slug` would.
    #[test]
    fn validate_and_derive_slug_success() {
        let cases: &[(&str, &str)] = &[
            ("My Entity", "my-entity"),
            ("Große Änderung", "große-änderung"),
            ("日本語のタイトル", "日本語のタイトル"),
            ("--leading--trailing--", "leading-trailing"),
            ("Project 日本 v2", "project-日本-v2"),
        ];
        for (title, expected) in cases {
            let got = validate_and_derive_slug(title)
                .unwrap_or_else(|e| panic!("expected ok for {title:?}, got {e:?}"));
            assert_eq!(&got, expected, "title={title:?}");
            // Must agree with the permissive pipeline for accepted titles.
            assert_eq!(got, title_to_slug(title).unwrap(), "title={title:?}");
        }
    }

    /// The strict gate runs the same NFC normalization as the
    /// permissive pipeline, so NFC and NFD spellings of the same
    /// title produce the same slug (or both reject).
    #[test]
    fn validate_and_derive_slug_nfc_normalization() {
        let nfc = "Café";
        let nfd = "Cafe\u{0301}";
        assert_eq!(
            validate_and_derive_slug(nfc).unwrap(),
            validate_and_derive_slug(nfd).unwrap(),
        );
    }

    /// SlugError::reason() returns the stable discriminator each
    /// surface uses on the `details.reason` field.
    #[test]
    fn slug_error_reason_discriminator() {
        let e = SlugError::TitleEmpty {
            input: "".to_string(),
        };
        assert_eq!(e.reason(), "empty");
        let e = SlugError::TitleHasInvalidChars {
            input: "x!".to_string(),
            invalid_chars: vec!['!'],
            proposed_slug: "x".to_string(),
        };
        assert_eq!(e.reason(), "invalid_chars");
        let e = SlugError::IdTooLong {
            input: "specs--x".to_string(),
            length: 201,
            max: 200,
        };
        assert_eq!(e.reason(), "id_too_long");
        let e = SlugError::TitleHasControlChars {
            input: "a\nb".to_string(),
            control_chars: vec!['\n'],
            proposed_slug: "a-b".to_string(),
        };
        assert_eq!(e.reason(), "control_chars");
    }

    /// F2 + F4: a title that derives a slug whose full
    /// `vault--slug` id sits at the 200-char ceiling is accepted;
    /// one byte over is rejected with a recovery-friendly error.
    /// `build_id` is the canonical write-side entry, so both
    /// behaviours land here.
    #[test]
    fn build_id_enforces_length_cap() {
        let vault = "specs";
        // vault.len()=5, "--"=2 → 7-char prefix. Slug of 193 chars
        // produces a 200-char id; 194 chars trips the cap.
        let just_fits = "a".repeat(ENTITY_ID_MAX_LEN - vault.len() - 2);
        let ok = build_id(vault, &just_fits).expect("at-cap id must pass");
        assert_eq!(ok.as_ref().chars().count(), ENTITY_ID_MAX_LEN);

        let one_over = "a".repeat(ENTITY_ID_MAX_LEN - vault.len() - 2 + 1);
        let err = build_id(vault, &one_over).unwrap_err();
        let SlugError::IdTooLong { input, length, max } = err else {
            panic!("expected IdTooLong, got {err:?}");
        };
        // `input` echoes the composed id, not the title, so it agrees
        // with `length`.
        assert_eq!(input, format!("{vault}--{one_over}"));
        assert_eq!(input.chars().count(), length);
        assert_eq!(length, ENTITY_ID_MAX_LEN + 1);
        assert_eq!(max, ENTITY_ID_MAX_LEN);
    }

    #[test]
    fn build_id_basic() {
        assert_eq!(build_id("specs", "My Entity").unwrap().0, "specs--my-entity");
    }

    /// F1 (B+A): non-Latin titles round-trip through `build_id`.
    #[test]
    fn build_id_non_latin() {
        assert_eq!(
            build_id("specs", "日本語のタイトル").unwrap().0,
            "specs--日本語のタイトル",
        );
        assert_eq!(
            build_id("specs", "Москва-проект").unwrap().0,
            "specs--москва-проект",
        );
    }

    #[test]
    fn file_path_to_id_basic() {
        assert_eq!(
            file_path_to_id("architecture/result-entity.md", "specs").0,
            "specs--architecture/result-entity"
        );
        assert_eq!(
            file_path_to_id("result-entity.md", "specs").0,
            "specs--result-entity"
        );
    }

    #[test]
    fn wiki_link_to_id_basic() {
        assert_eq!(
            wiki_link_to_id("result-entity", "specs").unwrap().0,
            "specs--result-entity"
        );
        assert_eq!(
            wiki_link_to_id("parent/child/entity", "specs").unwrap().0,
            "specs--parent/child/entity"
        );
    }

    #[test]
    fn wiki_link_to_id_strips_alias() {
        assert_eq!(
            wiki_link_to_id("target|Display Name", "specs").unwrap().0,
            "specs--target"
        );
    }

    #[test]
    fn wiki_link_to_id_strips_prefix_and_suffix() {
        assert_eq!(
            wiki_link_to_id("../parent/entity.md", "specs").unwrap().0,
            "specs--parent/entity"
        );
    }

    /// Agents writing the canonical fully-qualified id `[[vault--slug]]`
    /// must not be doubly-prefixed into `vault--vault--slug`.
    #[test]
    fn wiki_link_to_id_strips_redundant_self_prefix() {
        assert_eq!(
            wiki_link_to_id("specs--result-entity", "specs").unwrap().0,
            "specs--result-entity"
        );
        assert_eq!(
            wiki_link_to_id("test-vault-mini--engine", "test-vault-mini").unwrap().0,
            "test-vault-mini--engine"
        );
        assert_eq!(
            wiki_link_to_id("specs--target.md|Display", "specs").unwrap().0,
            "specs--target"
        );
        assert_eq!(
            wiki_link_to_id("specs--parent/child", "specs").unwrap().0,
            "specs--parent/child"
        );
        // Self-prefix stripping is one-shot, not iterative — a second
        // embedded `<current_vault>--` is preserved so cross-vault-style
        // drift stays visible.
        assert_eq!(
            wiki_link_to_id("specs--specs--slug", "specs").unwrap().0,
            "specs--specs--slug"
        );
    }

    /// Cross-vault dash form `[[<vault>--<slug>]]` routes to the named
    /// vault rather than silently re-prepending the source vault into a
    /// phantom `specs--other--entity` stub.
    /// The cross-vault policy gate (alias-synthesis pass) refuses the
    /// auto-stub when the workspace policy denies the direction —
    /// that gate is exercised in engine-layer tests.
    #[test]
    fn wiki_link_to_id_tier_zero_cross_vault_dash_form() {
        assert_eq!(
            wiki_link_to_id("other--entity", "specs").unwrap().0,
            "other--entity"
        );
        assert_eq!(
            wiki_link_to_id("nonexistent-vault--target", "specs").unwrap().0,
            "nonexistent-vault--target"
        );
    }

    /// Tier-0 dash form collapses cleanly when the named vault is the
    /// source vault — equivalent to the self-prefix-strip fast path
    /// for bare-slug authoring.
    #[test]
    fn wiki_link_to_id_tier_zero_self_vault_dash_form() {
        assert_eq!(
            wiki_link_to_id("specs--target", "specs").unwrap().0,
            "specs--target"
        );
    }

    /// Tier-0 only admits single-segment vault names — the
    /// hierarchical dash form stays on the colon Tier-2 recovery
    /// path. The pre-existing slash-dash ambiguity refusal is what
    /// fires here (cross-vault into a hierarchical vault is
    /// grammatically ambiguous with a same-vault hierarchical slug).
    #[test]
    fn wiki_link_to_id_tier_zero_refuses_hierarchical_prefix() {
        let err = wiki_link_to_id("team/sub-vault--target", "specs").unwrap_err();
        match err {
            WikiLinkError::InvalidTarget { suggested, .. } => {
                assert_eq!(suggested.as_deref(), Some("team/sub-vault:target"));
            }
            other => panic!("expected InvalidTarget, got {other:?}"),
        }
    }

    /// Section anchors strip as a display decoration alongside `|alias`,
    /// `../`, and `.md`. Single anchor, multi-anchor, and the
    /// combined anchor+alias form all collapse to the underlying
    /// slug-form id.
    #[test]
    fn wiki_link_to_id_strips_section_anchor() {
        assert_eq!(
            wiki_link_to_id("login-service#identity", "specs").unwrap().0,
            "specs--login-service"
        );
        assert_eq!(
            wiki_link_to_id("specs--login-service#identity", "specs").unwrap().0,
            "specs--login-service"
        );
        // Multi-anchor — strip from first `#`.
        assert_eq!(
            wiki_link_to_id("specs--target#a#b", "specs").unwrap().0,
            "specs--target"
        );
        // Combined anchor + alias.
        assert_eq!(
            wiki_link_to_id("specs--target#section|Display", "specs").unwrap().0,
            "specs--target"
        );
    }

    /// Cross-vault routing and anchor stripping compose: a cross-vault
    /// anchored form resolves under tier-0 and strips the anchor.
    #[test]
    fn wiki_link_to_id_cross_vault_anchored_composes() {
        assert_eq!(
            wiki_link_to_id("other--target#section", "specs").unwrap().0,
            "other--target"
        );
    }

    /// Empty `current_vault` opts out of self-prefix stripping so a
    /// literal leading `--` (which would never legitimately occur, but
    /// could collide with `format!("{vault}--", vault="")`) stays intact.
    #[test]
    fn wiki_link_to_id_empty_vault_does_not_strip() {
        assert_eq!(wiki_link_to_id("--weird", "").unwrap().0, "----weird");
    }

    #[test]
    fn wiki_link_to_id_tier_two_cross_vault() {
        assert_eq!(
            wiki_link_to_id("engine:health", "plugin").unwrap().0,
            "engine--health"
        );
        assert_eq!(
            wiki_link_to_id("engine:architecture/result", "plugin").unwrap().0,
            "engine--architecture/result"
        );
    }

    #[test]
    fn wiki_link_to_id_tier_two_self_prefix_collapses() {
        assert_eq!(wiki_link_to_id("specs:foo", "specs").unwrap().0, "specs--foo");
        assert_eq!(
            wiki_link_to_id("specs:foo", "specs").unwrap(),
            wiki_link_to_id("foo", "specs").unwrap()
        );
    }

    #[test]
    fn wiki_link_to_id_tier_two_combines_with_alias_and_md() {
        assert_eq!(
            wiki_link_to_id("engine:health.md|See health", "plugin").unwrap().0,
            "engine--health"
        );
    }

    #[test]
    fn wiki_link_to_id_tier_two_accepts_hierarchical_prefix() {
        assert_eq!(
            wiki_link_to_id("external/engine:health", "plugin").unwrap().0,
            "external/engine--health"
        );
    }

    #[test]
    fn wiki_link_to_id_tier_one_strips_hierarchical_self_prefix() {
        assert_eq!(
            wiki_link_to_id("team/sub-vault--auth-service", "team/sub-vault").unwrap().0,
            "team/sub-vault--auth-service"
        );
    }

    #[test]
    fn wiki_link_to_id_tier_one_bare_slug_from_hierarchical_vault() {
        assert_eq!(
            wiki_link_to_id("auth-service", "team/sub-vault").unwrap().0,
            "team/sub-vault--auth-service"
        );
    }

    /// `::` is reserved syntax — strict refusal. The slug-grammar gate
    /// refuses the `:` character outright.
    #[test]
    fn wiki_link_to_id_double_colon_refuses() {
        let err = wiki_link_to_id("engine::health", "plugin").unwrap_err();
        assert!(matches!(err, WikiLinkError::InvalidTarget { .. }), "got {err:?}");
    }

    /// Empty halves around the colon refuse under strict mode — the
    /// `:` character isn't in the slug-grammar character class, so
    /// the Tier-1 fallback fails.
    #[test]
    fn wiki_link_to_id_empty_tier_two_halves_refuse() {
        assert!(matches!(
            wiki_link_to_id(":foo", "specs").unwrap_err(),
            WikiLinkError::InvalidTarget { .. }
        ));
        assert!(matches!(
            wiki_link_to_id("engine:", "specs").unwrap_err(),
            WikiLinkError::InvalidTarget { .. }
        ));
    }

    /// Natural-form (uppercase + whitespace) refuses with
    /// `InvalidTarget` and a `title_to_slug`-derived suggestion the
    /// agent lifts directly into a retry.
    #[test]
    fn wiki_link_to_id_natural_form_refuses_with_suggestion() {
        let err = wiki_link_to_id("Knowledge Graph", "specs").unwrap_err();
        let WikiLinkError::InvalidTarget { raw, suggested, .. } = err else {
            panic!("expected InvalidTarget, got {err:?}");
        };
        assert_eq!(raw, "Knowledge Graph");
        assert_eq!(suggested.as_deref(), Some("knowledge-graph"));
    }

    /// Tier-2 with natural-form slug suggests `vault:slug`
    /// preserving the prefix. The agent rewrites only the slug part.
    #[test]
    fn wiki_link_to_id_tier_two_natural_slug_refuses_with_prefixed_suggestion() {
        let err = wiki_link_to_id("engine:Health Check", "plugin").unwrap_err();
        let WikiLinkError::InvalidTarget { raw, suggested, .. } = err else {
            panic!("expected InvalidTarget, got {err:?}");
        };
        assert_eq!(raw, "engine:Health Check");
        assert_eq!(suggested.as_deref(), Some("engine:health-check"));
    }

    /// Tier-1 dash form
    /// with `/` in the would-be vault prefix is grammatically
    /// ambiguous (cross-vault into a hierarchical vault vs same-vault
    /// hierarchical slug). Refusal carries the colon-form as
    /// `suggested` so the agent's recovery is a one-character edit.
    #[test]
    fn wiki_link_to_id_hierarchical_dash_form_refuses_with_colon_suggestion() {
        let err = wiki_link_to_id("team/sub-vault--auth-service", "test").unwrap_err();
        let WikiLinkError::InvalidTarget { raw, suggested, reason } = err else {
            panic!("expected InvalidTarget, got {err:?}");
        };
        assert_eq!(raw, "team/sub-vault--auth-service");
        assert_eq!(suggested.as_deref(), Some("team/sub-vault:auth-service"));
        // Reason names both disambiguations.
        assert!(
            reason.contains("team/sub-vault:auth-service"),
            "reason must surface the cross-vault colon form: {reason}"
        );
        assert!(
            reason.contains("test:team/sub-vault--auth-service"),
            "reason must surface the same-vault hierarchical form: {reason}"
        );
    }

    /// For self-prefixed dash form, tier-0 splits on the FIRST `--`, so
    /// `[[test--team/sub--target]]` resolves as vault `test`, slug
    /// `team/sub--target` — a same-vault entity with a hierarchical
    /// slug containing `--`. The ambiguity gate in the slug position
    /// does not apply because the vault/slug boundary is
    /// pinned by tier-0's grammar.
    #[test]
    fn wiki_link_to_id_self_prefixed_dash_form_resolves_via_tier_zero() {
        let id = wiki_link_to_id("test--team/sub--target", "test").unwrap();
        assert_eq!(id.vault(), "test");
        assert_eq!(id.path(), "team/sub--target");
    }

    /// A bare hierarchical slug (no `--`) continues to
    /// resolve to a same-vault entity. The refusal is keyed on the
    /// simultaneous presence of `/` AND `--`, not on `/` alone.
    #[test]
    fn wiki_link_to_id_bare_hierarchical_slug_still_resolves() {
        let id = wiki_link_to_id("team/sub-vault", "test").unwrap();
        assert_eq!(id.vault(), "test");
        assert_eq!(id.path(), "team/sub-vault");
    }

    /// Colon-form for cross-vault hierarchical reference
    /// continues to resolve correctly (the canonical disambiguation).
    #[test]
    fn wiki_link_to_id_hierarchical_colon_form_resolves_cross_vault() {
        let id = wiki_link_to_id("team/sub-vault:auth-service", "test").unwrap();
        assert_eq!(id.vault(), "team/sub-vault");
        assert_eq!(id.path(), "auth-service");
    }

    /// Flat `[[<other-vault>--<slug>]]` routes to the named vault under
    /// tier-0 rather than silently re-prefixing with the source vault into
    /// a phantom `test--other--target` stub. The cross-vault policy
    /// gate enforces routing legality in the alias-synthesis pass —
    /// that gate is exercised in engine-layer tests.
    #[test]
    fn wiki_link_to_id_flat_foreign_dash_form_routes_via_tier_zero() {
        let id = wiki_link_to_id("other--target", "test").unwrap();
        assert_eq!(id.vault(), "other");
        assert_eq!(id.path(), "target");
    }

    /// Tier-2 with non-ASCII vault prefix refuses with
    /// `InvalidVaultName`. Vault names are ASCII-only operator
    /// identifiers; the agent cannot auto-slugify them.
    #[test]
    fn wiki_link_to_id_tier_two_bad_vault_refuses_with_distinct_error() {
        let err = wiki_link_to_id("Other Vault:foo", "plugin").unwrap_err();
        let WikiLinkError::InvalidVaultName { raw, .. } = err else {
            panic!("expected InvalidVaultName, got {err:?}");
        };
        assert_eq!(raw, "Other Vault");
    }

    /// Pathological inputs (empty, all punctuation) refuse
    /// with `suggested: None`.
    #[test]
    fn wiki_link_to_id_pathological_input_no_suggestion() {
        let err = wiki_link_to_id("!!!", "specs").unwrap_err();
        let WikiLinkError::InvalidTarget { suggested, .. } = err else {
            panic!("expected InvalidTarget, got {err:?}");
        };
        assert!(suggested.is_none(), "got {suggested:?}");
    }

    /// Slug-form across every script family the slug
    /// pipeline accepts round-trips through the strict gate.
    #[test]
    fn wiki_link_to_id_accepts_slug_form_across_scripts() {
        let cases: &[(&str, &str)] = &[
            ("knowledge-graph", "v--knowledge-graph"),
            ("الرسم-البياني-للمعرفة", "v--الرسم-البياني-للمعرفة"),
            ("ज्ञान-ग्राफ", "v--ज्ञान-ग्राफ"),
            ("知识图谱", "v--知识图谱"),
            ("知識グラフ", "v--知識グラフ"),
            ("กราฟความรู้", "v--กราฟความรู้"),
            ("ידע-גרף", "v--ידע-גרף"),
        ];
        for (input, expected) in cases {
            let id = wiki_link_to_id(input, "v").unwrap_or_else(|e| {
                panic!("expected ok for {input:?}, got {e:?}")
            });
            assert_eq!(&id.0, expected, "input={input:?}");
        }
    }

    /// The lenient decoder preserves pre-strict behaviour
    /// for read-side scanners that must tolerate on-disk drift.
    /// Round-trip equivalence on the inputs the strict gate accepts.
    #[test]
    fn wiki_link_to_id_lenient_matches_strict_on_valid_input() {
        let inputs = &["knowledge-graph", "engine:health", "parent/child"];
        for input in inputs {
            let strict = wiki_link_to_id(input, "specs").unwrap();
            let lenient = wiki_link_to_id_lenient(input, "specs");
            assert_eq!(strict, lenient, "input={input:?}");
        }
    }

    /// The lenient decoder accepts what the strict gate
    /// refuses, surfacing the literal drift for read-side reporting.
    #[test]
    fn wiki_link_to_id_lenient_admits_drift() {
        assert_eq!(
            wiki_link_to_id_lenient("Knowledge Graph", "specs").0,
            "specs--Knowledge Graph"
        );
        assert_eq!(
            wiki_link_to_id_lenient("engine::health", "plugin").0,
            "plugin--engine::health"
        );
    }

    #[test]
    fn entity_id_parts() {
        let id = EntityId::new("specs", "parent/child");
        assert_eq!(id.vault(), "specs");
        assert_eq!(id.path(), "parent/child");
        assert_eq!(id.name(), "child");
    }

    #[test]
    fn entity_id_no_vault() {
        let id = EntityId("result-entity".to_string());
        assert_eq!(id.vault(), "");
        assert_eq!(id.path(), "result-entity");
        assert_eq!(id.name(), "result-entity");
    }

    #[test]
    fn id_to_file_path_basic() {
        let id = EntityId::new("specs", "architecture/result-entity");
        assert_eq!(id_to_file_path(&id), "architecture/result-entity.md");
    }

    #[test]
    fn validate_rel_type_valid() {
        assert_eq!(validate_rel_type("PART_OF").unwrap(), "PART_OF");
        assert_eq!(validate_rel_type("uses").unwrap(), "USES");
    }

    #[test]
    fn validate_rel_type_invalid() {
        assert!(validate_rel_type("has spaces").is_err());
        assert!(validate_rel_type("").is_err());
    }
}
