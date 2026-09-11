//! The engine-owned **preparation registry** — the one place that says which
//! preparations exist, on which anchor grains, at which engine touchpoint,
//! and what each grain's PREPARED FORM is.
//!
//! A source declares at most one preparation ([`crate::pipeline::Source::preparation`],
//! a string identifier). The engine refuses any identifier this registry
//! does not know ([`crate::binding::CapabilityError::PreparationUnsupported`],
//! raised by [`crate::binding::validate_binding`] and mirrored on the
//! brief-render path for a record that acquired one by hand) and consults the
//! registry at exactly two touchpoints:
//!
//! - **Touchpoint A — prepared form.** Anchor observation asks the registry
//!   for an artifact's prepared form before hashing it (the engine's one
//!   per-anchor observation site). The standalone `verify-anchors` operation
//!   and the binding-backed verify share that site, so both inherit every
//!   registered preparation without redesign.
//! - **Touchpoint B — delivery units.** The ingest delivery path asks the
//!   registry for a source's unit sequence ([`unitize`]): one file can carry
//!   many delivery units, addressed `<path>#<key>`, and the units of a whole
//!   source form one deterministic total order derived from the units' own
//!   keys ([`Touchpoint::DeliveryUnits`], first entry [`DATED_ENTRIES`]).
//!   A source declaring no delivery preparation keeps file-granularity
//!   delivery unchanged.
//!
//! **Identity.** [`crate::binding::PREPARATION_IMPL_VERSION`] is hashed into
//! every binding's `hash(D)` next to the declared identifier. Landing or
//! changing an implementation bumps the constant, which invalidates every
//! finding keyed on the old hash by construction (`ingest::findings` keys on
//! `hash(D)` alone).
//!
//! **Prepared forms per grain.** The path grains (`span` / `file`) hash their
//! bytes through [`crate::anchor::prepared_content_hash`] (the minimal
//! canonicalization: BOM, line endings, final newline). The `url` grain uses
//! the **same canonicalization over observation-supplied content** — the
//! engine never fetches, so whoever observed the URL supplies the bytes at
//! write time (`AnchorInput::content`) — and defaults to `hash_stability:
//! unstable`, a served page being a moving target. The `entity` grain's
//! prepared form is computed from the live graph, never from supplied bytes:
//! the canonical rendered markdown by default, or — under
//! [`ENTITY_LOAD_BEARING`] — the stable serialization of the type's
//! load-bearing sections.
//!
//! **Non-goal, by standing decision:** PDF / DOCX / audio conversion. An
//! agent with a capable read tool extracts; the raw-byte fallback of the
//! prepared-content hash already drift-detects a binary artifact.

use serde::{Deserialize, Serialize};

use crate::anchor::{AnchorGrain, AnchorHashStability, prepared_content_hash};
use crate::entity::Entity;

/// The engine touchpoint a registered preparation plugs into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Touchpoint {
    /// Touchpoint A: anchor observation asks the registry for the prepared
    /// form an artifact hashes as (content and code-map flavours).
    PreparedForm,
    /// Touchpoint B: the ingest delivery path asks the registry for a
    /// source's unit sequence (the delivery flavour).
    DeliveryUnits,
}

/// One registered preparation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preparation {
    /// The identifier a source declares (`Source::preparation`).
    pub id: &'static str,
    /// Which engine touchpoint consults it.
    pub touchpoint: Touchpoint,
    /// The anchor grains it produces a prepared form for. A binding may
    /// declare it only over a medium whose anchor namespace admits at least
    /// one of these grains (checked at binding validation).
    pub grains: &'static [AnchorGrain],
    /// One sentence for the operator and the refusal payloads.
    pub description: &'static str,
}

/// Content preparation on the `entity` grain: the prepared form is the stable
/// serialization of the entity type's **load-bearing sections** (see
/// [`load_bearing_sections`]), so a dependent's prepared hash breaks when a
/// load-bearing sentence changes and holds when a comma lands in the notes.
pub const ENTITY_LOAD_BEARING: &str = "entity-load-bearing";

/// Delivery preparation on path-shaped sources: a file is a sequence of
/// **dated entries**. A unit begins at every line that opens with an ISO
/// date or date-time (`2026-08-24`, `2026-08-24 10:05`, `2026-08-24T10:05:00Z`,
/// after any leading markdown markers such as `## `, `- `, `> `, `[`); it
/// runs to the next such line. Text before the first entry folds into the
/// first unit; a file with no dated line is one unit keyed
/// [`WHOLE_FILE_UNIT`]. The unit key is the stamp normalized to
/// `YYYY-MM-DDTHH:MM:SS` (missing time parts read `00`), with `.2`, `.3`, …
/// appended to the second, third, … entry carrying the same stamp in one
/// file, in file order — so appending entries never renames an existing
/// unit. The order key is the normalized stamp; across a whole source, units
/// sort by stamp, then path, then key, which is what makes a chronological
/// corpus deliver in its own order regardless of how files were discovered.
/// Undated files (order key empty) come first, in path order. Fractional
/// seconds and zone designators are accepted and ignored for ordering.
pub const DATED_ENTRIES: &str = "dated-entries";

/// The key of the single unit a file yields when a delivery preparation finds
/// no unit boundary in it — the whole file, still addressable as
/// `<path>#whole`.
pub const WHOLE_FILE_UNIT: &str = "whole";

/// Code-map preparation on path-shaped sources (touchpoint A): a scoped
/// file's prepared form is its **interface digest** — imports, exports and
/// declarations with their signatures; comments, formatting and bodies are
/// invisible. The digest is heuristic and language-family aware by file
/// extension: C-like families (JS/TS, Rust, Go, Java, Kotlin, Swift, C#, C,
/// C++, PHP, Dart, Scala) keep top-level declaration lines and class or
/// object member signatures, cut at the body's opening brace; Python keeps
/// imports, `def`/`class` lines (top level and one level in), decorators and
/// upper-case module constants; a Vue single-file component is its script
/// block under the C-like rule; JSON is its canonical compact form; every
/// other file is taken whole. A `tree`-grain anchor under this preparation
/// hashes the digest of every scoped file under the tree (path and file
/// hash, path order), which is what closes the tree grain's
/// recorded-but-unhashed residue for code sources. Values inside
/// declarations (a config object's members, an array literal's contents
/// beyond its first line, a scalar property value) are body, not
/// interface: the digest sees names and signatures. A literal object
/// restructured between one line and many reads as a shape change.
pub const CODE_MAP: &str = "code-map";

/// Quoted-phrase preparation on the text-bearing grains (touchpoint A): the
/// artifact `<path-or-url-or-entity-id>#<phrase>` addresses **the occurrence
/// of a literal phrase** in a text, and its prepared form is the phrase
/// itself. While the text still carries the phrase (an exact substring
/// match after the minimal canonicalization of line endings), the anchor
/// resolves; when the words are gone the unit is absent and the anchor reads
/// `orphaned`, whatever else changed around it. A citation anchor, then: "this
/// document still says X", quiet under every rewrite that keeps the words
/// and loud the moment they leave. Applies to `span` (a file, read live),
/// `url` (observation-supplied content: the engine never fetches) and
/// `entity` (the live entity's canonical markdown). An artifact under this
/// preparation that names no `#<phrase>` keeps the grain's whole-text form.
pub const QUOTED_PHRASE: &str = "quoted-phrase";

/// The registry — every preparation this engine implements. The refusal in
/// [`crate::binding::validate_binding`] is exactly "not in this list".
pub const REGISTRY: &[Preparation] = &[
    Preparation {
        id: ENTITY_LOAD_BEARING,
        touchpoint: Touchpoint::PreparedForm,
        grains: &[AnchorGrain::Entity],
        description: "an entity's prepared form is the stable serialization of its type's \
                      load-bearing sections (explicitly declared, else the required sections, \
                      else every section) — notes-only edits keep dependents' anchors resolving",
    },
    Preparation {
        id: DATED_ENTRIES,
        touchpoint: Touchpoint::DeliveryUnits,
        grains: &[AnchorGrain::Span],
        description: "a file is a sequence of entries opening with an ISO date or date-time; \
                      each entry is one delivery unit `<path>#<stamp>`, and a source's units \
                      deliver in stamp order, identical on every pass — a chronological corpus \
                      (logs, transcripts, journals, mail threads) is never shuffled",
    },
    Preparation {
        id: CODE_MAP,
        touchpoint: Touchpoint::PreparedForm,
        grains: &[AnchorGrain::File, AnchorGrain::Span, AnchorGrain::Tree],
        description: "a scoped code file's prepared form is its interface digest (imports, \
                      exports, declarations and their signatures; comments, formatting and \
                      bodies invisible), and a tree's is the digest of every scoped file under \
                      it — an anchor drifts when an interface changes and stays quiet when \
                      only an implementation does",
    },
    Preparation {
        id: QUOTED_PHRASE,
        touchpoint: Touchpoint::PreparedForm,
        grains: &[AnchorGrain::Span, AnchorGrain::Url, AnchorGrain::Entity],
        description: "an artifact `<path-or-url-or-entity>#<phrase>` addresses the occurrence of \
                      a literal phrase in a text and its prepared form is the phrase itself — the \
                      anchor resolves while the file, the observed page or the entity still says \
                      those words and reads orphaned once they are gone, whatever else changed",
    },
];

/// Every registered preparation.
pub fn registry() -> &'static [Preparation] {
    REGISTRY
}

/// Look a declared identifier up.
pub fn lookup(id: &str) -> Option<&'static Preparation> {
    REGISTRY.iter().find(|p| p.id == id)
}

/// Whether `id` names a registered preparation.
pub fn is_registered(id: &str) -> bool {
    lookup(id).is_some()
}

/// The registered identifiers, in registry order — the recovery payload of
/// the unknown-identifier refusal.
pub fn registered_identifiers() -> Vec<&'static str> {
    REGISTRY.iter().map(|p| p.id).collect()
}

/// The delivery preparation a source declares, if its declared identifier
/// is a registered touchpoint-B entry — `None` for no declaration, an
/// unregistered identifier, or a prepared-form (touchpoint A) flavour.
pub fn delivery_preparation(declared: Option<&str>) -> Option<&'static Preparation> {
    lookup(declared?).filter(|p| p.touchpoint == Touchpoint::DeliveryUnits)
}

/// Whether a registered preparation can apply over a medium whose anchor
/// namespace is `anchor_namespace` (see
/// [`crate::binding::medium_capabilities`]): at least one of the
/// preparation's grains must be expressible there. `entity-load-bearing`
/// over a `codebase` source would never meet an entity-grain anchor, so it
/// is refused at declaration rather than silently never applying.
pub fn applies_to_namespace(preparation: &Preparation, anchor_namespace: &str) -> bool {
    preparation
        .grains
        .iter()
        .any(|g| g.supported_by_namespace(anchor_namespace))
}

// ---------------------------------------------------------------------------
// Per-grain prepared forms
// ---------------------------------------------------------------------------

/// The medium-declared default hash stability per grain. A `url` anchor
/// defaults to `unstable` — a served page is a moving target, so a hash
/// break resolves `recheck`, never `drifted`, unless the author asserts
/// `stable` explicitly. Every other grain keeps its `stable` default.
pub fn default_hash_stability(grain: AnchorGrain) -> AnchorHashStability {
    match grain {
        AnchorGrain::Url => AnchorHashStability::Unstable,
        AnchorGrain::Span | AnchorGrain::File | AnchorGrain::Tree | AnchorGrain::Entity => {
            AnchorHashStability::Stable
        }
    }
}

/// The `url` grain's canonicalization entry: the prepared form of a URL
/// artifact is the observation-supplied content under the same minimal
/// canonicalization the path grains use, so a `url` anchor's recorded hash
/// means the same thing a `file` anchor's does. The engine never fetches;
/// the observer supplies the bytes.
pub fn url_prepared_hash(content: &[u8]) -> String {
    prepared_content_hash(content)
}

/// The prepared-content hash of **supplied** content for a grain — the
/// write-time observation an agent performs when it hands the engine what it
/// read (`AnchorInput::content`). `None` for a grain whose prepared form
/// is never computed from supplied bytes: `entity` (computed from the live
/// graph, so a supplied rendering could disagree with the store) and `tree`
/// (its prepared form is a digest over the enumerated scoped files —
/// observation-side work no single supplied byte-string can represent).
pub fn supplied_content_hash(grain: AnchorGrain, content: &[u8]) -> Option<String> {
    match grain {
        AnchorGrain::Span | AnchorGrain::File => Some(prepared_content_hash(content)),
        AnchorGrain::Url => Some(url_prepared_hash(content)),
        AnchorGrain::Tree | AnchorGrain::Entity => None,
    }
}

// ---------------------------------------------------------------------------
// Touchpoint A for path grains: the prepared form of a file or tree
// ---------------------------------------------------------------------------

/// What a path-grain observation yields under a source's preparation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathPrepared {
    /// The prepared-content hash to record or compare.
    Hash(String),
    /// The grain has no prepared form at this single-artifact touchpoint (a
    /// `tree`, whose digest needs its files enumerated — the caller's job):
    /// observe no hash here, resolve `recheck` where no digest arrives.
    NoHash,
    /// The artifact addresses a sub-file unit the file no longer yields (a
    /// `<path>#<key>` span under a delivery preparation): an absent artifact.
    UnitAbsent,
}

/// The prepared-content hash of one path-grain artifact's bytes under
/// `preparation` — the one rule anchor observation and the write-time
/// `content` path share, so a hash recorded at write time is the hash a
/// later observation computes. No preparation (or one that does not prepare
/// path grains): the file's bytes under the minimal canonicalization for
/// `file`/`span`. [`DATED_ENTRIES`]: a `<path>#<key>`
/// span hashes its unit ([`PathPrepared::UnitAbsent`] when the key is gone),
/// a bare file its bytes. [`CODE_MAP`]: `file`/`span` hash the interface
/// digest. A `tree` needs its files enumerated, which is the caller's job
/// under every preparation ([`code_map_tree_digest`] /
/// [`plain_tree_digest`]), so it answers `NoHash` here.
pub fn path_prepared_hash(
    preparation: Option<&str>,
    artifact: &str,
    grain: AnchorGrain,
    bytes: &[u8],
) -> PathPrepared {
    let (path, locator) = split_unit_id(artifact);
    match (preparation, grain) {
        (Some(QUOTED_PHRASE), AnchorGrain::Span | AnchorGrain::Url) if locator.is_some() => {
            quoted_phrase_prepared(locator.unwrap_or_default(), &String::from_utf8_lossy(bytes))
        }
        (_, AnchorGrain::Url | AnchorGrain::Entity | AnchorGrain::Tree) => PathPrepared::NoHash,
        (Some(DATED_ENTRIES), AnchorGrain::Span) if locator.is_some() => {
            let text = String::from_utf8_lossy(bytes);
            match unitize(DATED_ENTRIES, &text)
                .and_then(|units| units.into_iter().find(|u| Some(u.key.as_str()) == locator))
            {
                Some(unit) => PathPrepared::Hash(unit.hash),
                None => PathPrepared::UnitAbsent,
            }
        }
        (Some(CODE_MAP), AnchorGrain::File | AnchorGrain::Span) => {
            let text = String::from_utf8_lossy(bytes);
            PathPrepared::Hash(prepared_content_hash(
                code_map_digest(path, &text).as_bytes(),
            ))
        }
        (_, AnchorGrain::File | AnchorGrain::Span) => {
            PathPrepared::Hash(prepared_content_hash(bytes))
        }
    }
}

/// Touchpoint A under [`QUOTED_PHRASE`]: the prepared form of `text` for the
/// artifact `…#<phrase>`. The phrase is matched as an exact substring after
/// the minimal canonicalization the path grains share (a BOM and `\r\n`
/// line endings never decide a citation); present, the prepared form is the
/// phrase and the hash is stable for as long as the words stand; absent, the
/// unit is gone ([`PathPrepared::UnitAbsent`]) and the anchor reads
/// `orphaned`. An empty phrase addresses nothing and is absent by
/// definition, so a `#` with nothing after it can never resolve clean.
pub fn quoted_phrase_prepared(phrase: &str, text: &str) -> PathPrepared {
    let phrase = phrase.trim();
    if phrase.is_empty() {
        return PathPrepared::UnitAbsent;
    }
    let canonical_text = canonical_text(text);
    let canonical_phrase = canonical_text_owned(phrase);
    if canonical_text.contains(canonical_phrase.as_str()) {
        PathPrepared::Hash(prepared_content_hash(canonical_phrase.as_bytes()))
    } else {
        PathPrepared::UnitAbsent
    }
}

/// The text a phrase is matched against: BOM stripped, `\r\n` folded to
/// `\n` — the same two normalizations `prepared_content_hash` applies, so a
/// phrase that resolves on one platform resolves on every other.
fn canonical_text(text: &str) -> String {
    canonical_text_owned(text)
}

fn canonical_text_owned(text: &str) -> String {
    text.strip_prefix('\u{feff}')
        .unwrap_or(text)
        .replace("\r\n", "\n")
}

/// The code map of a tree: one line per scoped file under it, `<file digest
/// hash>  <path>`, in path order — hashed by the caller through
/// [`prepared_content_hash`]. A file joining, leaving, or changing its
/// interface changes the tree's map; an implementation edit does not.
pub fn code_map_tree_digest(files: &[(String, String)]) -> String {
    let mut rows: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    rows.sort();
    rows.iter()
        .map(|(path, text)| {
            format!(
                "{}  {path}",
                prepared_content_hash(code_map_digest(path, text).as_bytes())
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The plain digest of a tree: one line per scoped file under it,
/// `<prepared-content hash>  <path>`, in path order — hashed by the caller
/// through [`prepared_content_hash`]. Any byte change in any scoped file,
/// and any file joining or leaving the tree, changes the digest — the same
/// whole-content posture a `file` anchor has, lifted to the directory. This
/// is the no-preparation counterpart of [`code_map_tree_digest`]; it is what
/// lets a plain `tree` anchor adjudicate deterministically instead of
/// resting in `recheck` forever.
pub fn plain_tree_digest(files: &[(String, Vec<u8>)]) -> String {
    let mut rows: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
        .collect();
    rows.sort();
    rows.iter()
        .map(|(path, bytes)| format!("{}  {path}", prepared_content_hash(bytes)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The interface digest of one file's text under [`CODE_MAP`].
pub fn code_map_digest(path: &str, text: &str) -> String {
    match family_of(path) {
        Family::Text => text.to_string(),
        Family::Json => serde_json::from_str::<serde_json::Value>(text)
            .map(|v| v.to_string())
            .unwrap_or_else(|_| text.to_string()),
        Family::Vue => {
            declaration_lines(&strip_c_comments(&vue_script_blocks(text)), Family::CLike)
        }
        Family::CLike => declaration_lines(&strip_c_comments(text), Family::CLike),
        Family::Rust => declaration_lines(&strip_c_comments(text), Family::Rust),
        Family::Python => declaration_lines(&strip_python_comments(text), Family::Python),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    CLike,
    /// C-like braces, but a struct field is interface only with `pub`
    /// (a `key: Type` line without it is body), and enum variants are.
    Rust,
    Python,
    Json,
    Vue,
    Text,
}

fn family_of(path: &str) -> Family {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = match name.rsplit_once('.') {
        Some((_, ext)) => ext.to_ascii_lowercase(),
        None => return Family::Text,
    };
    match ext.as_str() {
        "rs" => Family::Rust,
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" | "mts" | "cts" | "go" | "java" | "kt"
        | "kts" | "swift" | "cs" | "c" | "h" | "cc" | "cpp" | "hpp" | "m" | "mm" | "php"
        | "dart" | "scala" => Family::CLike,
        "py" | "pyi" => Family::Python,
        "json" => Family::Json,
        "vue" | "svelte" => Family::Vue,
        _ => Family::Text,
    }
}

/// The concatenated `<script>` blocks of a single-file component (the
/// template and styles are not interface).
fn vue_script_blocks(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut out = String::new();
    let mut from = 0;
    while let Some(open) = lower[from..].find("<script") {
        let open = from + open;
        let Some(tag_end) = lower[open..].find('>') else {
            break;
        };
        let body_start = open + tag_end + 1;
        let Some(close) = lower[body_start..].find("</script") else {
            out.push_str(&text[body_start..]);
            break;
        };
        out.push_str(&text[body_start..body_start + close]);
        out.push('\n');
        from = body_start + close + 8;
    }
    out
}

/// Strip `//` line comments and `/* */` block comments, outside string
/// literals (a `'`/`"` string ends at its line; a template literal may span
/// lines). Newlines are kept so line structure survives.
fn strip_c_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_str: Option<char> = None;
    let mut escape = false;
    while let Some(c) = chars.next() {
        if let Some(q) = in_str {
            out.push(c);
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == q || (c == '\n' && q != '`') {
                in_str = None;
            }
            continue;
        }
        match c {
            '"' | '\'' | '`' => {
                in_str = Some(c);
                out.push(c);
            }
            '/' => match chars.peek() {
                Some('/') => {
                    for n in chars.by_ref() {
                        if n == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                }
                Some('*') => {
                    chars.next();
                    let mut prev = '\0';
                    for n in chars.by_ref() {
                        if n == '\n' {
                            out.push('\n');
                        }
                        if prev == '*' && n == '/' {
                            break;
                        }
                        prev = n;
                    }
                }
                _ => out.push(c),
            },
            _ => out.push(c),
        }
    }
    out
}

/// Strip `#` comments outside strings and drop triple-quoted strings whole
/// (docstrings and block literals are never interface).
fn strip_python_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut in_str: Option<char> = None;
    let mut triple: Option<char> = None;
    while i < bytes.len() {
        let c = bytes[i];
        if let Some(q) = triple {
            if c == q && i + 2 < bytes.len() && bytes[i + 1] == q && bytes[i + 2] == q {
                triple = None;
                i += 3;
                continue;
            }
            if c == '\n' {
                out.push('\n');
            }
            i += 1;
            continue;
        }
        if let Some(q) = in_str {
            out.push(c);
            if c == '\\' && i + 1 < bytes.len() {
                out.push(bytes[i + 1]);
                i += 2;
                continue;
            }
            if c == q || c == '\n' {
                in_str = None;
            }
            i += 1;
            continue;
        }
        match c {
            '"' | '\'' => {
                if i + 2 < bytes.len() && bytes[i + 1] == c && bytes[i + 2] == c {
                    triple = Some(c);
                    i += 3;
                    continue;
                }
                in_str = Some(c);
                out.push(c);
            }
            '#' => {
                while i < bytes.len() && bytes[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            _ => out.push(c),
        }
        i += 1;
    }
    out
}

const C_LIKE_TOP_LEVEL: &[&str] = &[
    "import ",
    "export ",
    "module.exports",
    "exports.",
    "function ",
    "async function ",
    "class ",
    "interface ",
    "type ",
    "enum ",
    "declare ",
    "const ",
    "let ",
    "var ",
    "pub ",
    "fn ",
    "struct ",
    "trait ",
    "impl ",
    "impl<",
    "mod ",
    "use ",
    "static ",
    "macro_rules!",
    "package ",
    "func ",
    "namespace ",
    "using ",
    "#include",
    "#[",
    "@",
    "public ",
    "private ",
    "protected ",
    "abstract ",
    "final ",
    "override ",
    "typedef ",
    "extern ",
    "template",
    "def ",
];

const C_LIKE_MEMBER: &[&str] = &[
    "pub ",
    "fn ",
    "public ",
    "private ",
    "protected ",
    "static ",
    "abstract ",
    "override ",
    "readonly ",
    "async ",
    "get ",
    "set ",
    "constructor",
    "#[",
    "@",
];

/// A member signature: an optionally qualified identifier followed by a
/// parameter list.
fn method_re() -> &'static regex::Regex {
    static METHOD: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    METHOD.get_or_init(|| {
        regex::Regex::new(r"^(?:(?:async|static|get|set|public|private|protected|override)\s+)*[A-Za-z_$][\w$]*\s*(?:<[^>]*>)?\s*\(").unwrap()
    })
}

/// A property member (`key: …`, an optional `readonly`/`?`), whatever its
/// value: an object literal's member, an interface member's type, a bare
/// key a formatter broke away from its value. Scalar values are cut later.
fn property_re() -> &'static regex::Regex {
    static PROPERTY: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    PROPERTY.get_or_init(|| {
        regex::Regex::new(r#"^(?:readonly\s+)?(?:['"]?)[A-Za-z_$][\w$]*(?:['"]?)\??\s*:"#).unwrap()
    })
}

/// An interface's method member: a signature carrying a return type and no
/// body (`load(id: string): Promise<void>`, an optional `?` after the name,
/// a trailing `;`). A ternary statement (`f(a) ? b : c`) is not one: its
/// `?` is followed by a space, a member's never is.
fn typed_member_re() -> &'static regex::Regex {
    static TYPED_MEMBER: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    TYPED_MEMBER.get_or_init(|| {
        regex::Regex::new(
            r"^(?:readonly\s+)?[A-Za-z_$][\w$]*\??\s*(?:<[^>]*>)?\s*\(.*\)\s*:\s*[^{;]+[;,]?$",
        )
        .unwrap()
    })
}

fn typed_member(line: &str) -> bool {
    typed_member_re().is_match(line) && !line.contains("? ")
}

/// An enum member (`Blue`, `Blue = 2,`): a capitalized bare identifier line.
fn enum_member_re() -> &'static regex::Regex {
    static ENUM_MEMBER: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    ENUM_MEMBER
        .get_or_init(|| regex::Regex::new(r"^[A-Z][A-Za-z0-9_]*(?:\s*=\s*[^,]+)?,?$").unwrap())
}

/// Whether a kept depth-1 or depth-2 line was kept ONLY as a member
/// signature (not by a keyword prefix, not as a property): such a line must
/// open a body once joined, or it was a wrapped call statement, not a
/// signature.
fn member_signature_only(line: &str, depth: i32) -> bool {
    depth >= 1
        && !C_LIKE_MEMBER.iter().any(|p| line.starts_with(p))
        && !property_re().is_match(line)
        && !typed_member(line)
        && method_re().is_match(line)
}

fn c_like_keeps(line: &str, depth: i32, next_opens_body: bool, properties: bool) -> bool {
    let method = method_re();
    let property = property_re();
    // A member signature opens a body (`{`, on this line or, Allman-style,
    // on the next), or is wrapped across lines (ends with `(` or `,`); a
    // finished call statement ends with `)` and is followed by anything else.
    let signature_shaped = |line: &str| {
        method.is_match(line)
            && !line.starts_with("if ")
            && !line.starts_with("for ")
            && !line.starts_with("while ")
            && !line.starts_with("switch ")
            && !line.starts_with("return ")
            && !line.starts_with("catch ")
            && (line.ends_with('{')
                || line.ends_with('(')
                || line.ends_with(',')
                || (line.ends_with(')') && next_opens_body))
    };
    match depth {
        0 => C_LIKE_TOP_LEVEL.iter().any(|p| line.starts_with(p)),
        1 => {
            C_LIKE_MEMBER.iter().any(|p| line.starts_with(p))
                || signature_shaped(line)
                || (properties && (property.is_match(line) || typed_member(line)))
                || enum_member_re().is_match(line)
        }
        2 => signature_shaped(line),
        _ => false,
    }
}

fn python_keeps(line: &str, indent: usize) -> bool {
    static CONSTANT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let constant = CONSTANT
        .get_or_init(|| regex::Regex::new(r"^(?:[A-Z_][A-Z0-9_]*|__all__)\s*[:=]").unwrap());
    let decl = line.starts_with("def ")
        || line.starts_with("async def ")
        || line.starts_with("class ")
        || line.starts_with('@');
    if indent == 0 {
        decl || line.starts_with("import ") || line.starts_with("from ") || constant.is_match(line)
    } else {
        indent <= 4 && decl
    }
}

/// For a Python module constant (`NAME = value`, `NAME: type = value`), the
/// byte index just past the `=`; `None` for any other line.
fn python_constant_cut(line: &str) -> Option<usize> {
    static CONSTANT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let constant = CONSTANT.get_or_init(|| {
        regex::Regex::new(r"^(?:[A-Z_][A-Z0-9_]*|__all__)(?:\s*:\s*[^=]+?)?\s*=").unwrap()
    });
    constant.find(line).map(|m| m.end())
}

fn paren_balance(s: &str) -> i32 {
    s.chars()
        .map(|c| match c {
            '(' => 1,
            ')' => -1,
            _ => 0,
        })
        .sum()
}

fn brace_delta(s: &str) -> i32 {
    s.chars()
        .map(|c| match c {
            '{' => 1,
            '}' => -1,
            _ => 0,
        })
        .sum()
}

/// Cut a declaration at its body: the first `{` outside parentheses for a
/// C-like line (an import, `use`, or re-export list is not a body and is
/// kept whole), the trailing `:` for Python.
fn cut_at_body(sig: &str, family: Family) -> String {
    match family {
        Family::Python => sig.trim_end_matches(':').trim_end().to_string(),
        _ if sig.starts_with("import ")
            || sig.starts_with("use ")
            || sig.starts_with("export {")
            || sig.starts_with("export type {")
            || sig.starts_with("export * ") =>
        {
            sig.trim_end().to_string()
        }
        _ => {
            // The body opens at the first `{` outside parentheses, or, for
            // an arrow whose body is an expression, right after the `=>`:
            // an expression body is body whatever line it wraps onto.
            let mut depth = 0i32;
            let mut cut = sig.len();
            let bytes = sig.as_bytes();
            for (i, c) in sig.char_indices() {
                match c {
                    '(' | '[' => depth += 1,
                    ')' | ']' => depth -= 1,
                    '{' if depth <= 0 => {
                        cut = i;
                        break;
                    }
                    '=' if depth <= 0 && bytes.get(i + 1) == Some(&b'>') => {
                        let rest = sig[i + 2..].trim_start();
                        if !rest.starts_with('{') {
                            cut = i + 2;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            sig[..cut].trim_end().to_string()
        }
    }
}

/// Normalize a kept declaration so that formatting inside it is invisible:
/// trailing semicolons dropped, double quotes read as single quotes, runs of
/// whitespace collapsed, and no whitespace next to punctuation — so
/// `f(a,b)`, `f(a, b)` and a signature wrapped across lines digest alike,
/// while every token that carries meaning survives.
fn normalize_signature(sig: &str) -> String {
    let collapsed: Vec<&str> = sig
        .trim()
        .trim_end_matches(';')
        .split_whitespace()
        .collect();
    let joined = collapsed.join(" ").replace('"', "'");
    let is_punct = |c: char| "()[]{},;:=<>|&?!-+*/.".contains(c);
    let mut out = String::with_capacity(joined.len());
    let chars: Vec<char> = joined.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == ' ' {
            let before = chars[..i].iter().rev().find(|x| **x != ' ').copied();
            let after = chars[i + 1..].iter().find(|x| **x != ' ').copied();
            if before.is_some_and(is_punct) || after.is_some_and(is_punct) {
                continue;
            }
        }
        out.push(c);
    }
    // Trailing commas are a formatter's choice, never a signature's: drop
    // one before a closing bracket and at the end of the line.
    let out = out
        .replace(",)", ")")
        .replace(",]", "]")
        .replace(",}", "}")
        .replace(",>", ">");
    let out = out.trim_end_matches(',').to_string();
    // A union type wrapped by a formatter leads its first member with `|`.
    let out = out.replace("=|", "=");
    // A kept line that opens a body keeps nothing of the brace itself.
    let out = out.trim_end_matches('{').trim_end().to_string();
    // Formatter defaults that are not signatures: a quoted property key
    // reads as the bare key; `(x)=>` reads as `x=>`; a Python import list's
    // parentheses (black's wrapped form) vanish.
    static QUOTED_KEY: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static ARROW_PARENS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let quoted_key =
        QUOTED_KEY.get_or_init(|| regex::Regex::new(r"^'([A-Za-z_$][\w$]*)':").unwrap());
    let arrow_parens =
        ARROW_PARENS.get_or_init(|| regex::Regex::new(r"\(([A-Za-z_$][\w$]*)\)=>").unwrap());
    let out = quoted_key.replace(&out, "$1:").into_owned();
    let out = arrow_parens.replace_all(&out, "$1=>").into_owned();
    // A scalar property value (`name: 'Auth'`, `port: 3000`) is body, exactly
    // as a one-line literal's members are: keep the key alone.
    static SCALAR_PROPERTY: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let scalar = SCALAR_PROPERTY
        .get_or_init(|| regex::Regex::new(r#"^([A-Za-z_$][\w$]*:)(?:'|`|-?\d)"#).unwrap());
    let out = match scalar.captures(&out) {
        Some(caps) => caps[1].to_string(),
        None => out,
    };
    if out.starts_with("from ") || out.starts_with("import ") {
        return out
            .replace('(', " ")
            .replace(')', "")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
    }
    out
}

/// A top-level binding whose value is not a function is cut right after its
/// `=` (and a default export that is not a function or object literal right
/// after `default`): the value is body, so its wrapping, its operators and
/// its literal shape never reach the digest. A function-valued binding keeps
/// its signature. `None` when the line is not such a binding.
fn cut_value_binding(line: &str) -> Option<String> {
    static BINDING: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static DEFAULT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let binding = BINDING.get_or_init(|| {
        regex::Regex::new(
            r"^((?:(?:export\s+)?(?:pub(?:\([^)]*\))?\s+)?(?:(?:static|readonly|private|public|protected|declare|override|const|let|var)\s+)*(?:[A-Za-z_$][\w$]*|\{[^}]*\}|\[[^\]]*\])(?:\s*:\s*[^=]+?)?|(?:module\.)?exports(?:\.[A-Za-z_$][\w$]*)?)\s*=)\s*(.*)$",
        )
        .unwrap()
    });
    let default =
        DEFAULT.get_or_init(|| regex::Regex::new(r"^(export\s+default)\s+(.*)$").unwrap());
    let function_like = |value: &str| {
        static ARROW: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let arrow = ARROW
            .get_or_init(|| regex::Regex::new(r"^(?:async\s+)?[A-Za-z_$][\w$]*\s*=>").unwrap());
        value.starts_with('(')
            || value.starts_with("async ")
            || value.starts_with("async(")
            || value.starts_with("function")
            || value.starts_with("class")
            || arrow.is_match(value)
    };
    if let Some(caps) = binding.captures(line) {
        let value = caps[2].trim();
        // The `=` of an arrow `=>` (a property whose type annotation the
        // regex read as `key: (params)`) is not an assignment.
        if value.starts_with('>') {
            return None;
        }
        // A module's object-literal export (`module.exports = {`) is an
        // interface container like `export default {`: its members stay.
        let module_export = caps[1].starts_with("exports") || caps[1].starts_with("module.exports");
        // An empty value means a formatter broke it onto the next line:
        // still a value, still body (the caller skips that line).
        if function_like(value) || (module_export && value.starts_with('{')) {
            return None;
        }
        return Some(caps[1].to_string());
    }
    if let Some(caps) = default.captures(line) {
        let value = caps[2].trim();
        if value.is_empty() || value.starts_with('{') || function_like(value) {
            return None;
        }
        return Some(caps[1].to_string());
    }
    None
}

fn angle_balance(s: &str) -> i32 {
    // Generics only: `->` and `=>` carry a `>` that is not a bracket.
    let s = s.replace("->", " ").replace("=>", " ");
    s.chars()
        .map(|c| match c {
            '<' => 1,
            '>' => -1,
            _ => 0,
        })
        .sum::<i32>()
        .max(0)
}

fn bracket_balance(s: &str) -> i32 {
    s.chars()
        .map(|c| match c {
            '[' => 1,
            ']' => -1,
            _ => 0,
        })
        .sum()
}

/// A binding whose destructuring pattern a formatter wrapped: the pattern
/// opens on the binding line (`const {`, `let [`) and closes on a later one.
fn opens_destructure(line: &str) -> bool {
    static DESTRUCTURE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = DESTRUCTURE
        .get_or_init(|| regex::Regex::new(r"^(?:export\s+)?(?:const|let|var)\s+[\{\[]").unwrap());
    re.is_match(line) && brace_delta(line) + bracket_balance(line) > 0
}

/// An import, `use`, or re-export list wraps across lines on its braces;
/// nothing else joins on a brace (a brace elsewhere opens a body).
fn is_list_declaration(sig: &str) -> bool {
    sig.starts_with("import ")
        || sig.starts_with("use ")
        || sig.starts_with("export {")
        || sig.starts_with("export type {")
}

fn declaration_lines(stripped: &str, family: Family) -> String {
    let lines: Vec<&str> = stripped.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut depth: i32 = 0;
    // After a binding whose value was cut as body, the value's own lines
    // are skipped whole until every bracket the value opened has closed:
    // (brace, bracket, paren) balances accumulated from the binding line.
    let mut body_skip: Option<(i32, i32, i32)> = None;
    let mut i = 0;
    while i < lines.len() {
        let raw = lines[i];
        let line = raw.trim();
        if line.is_empty() {
            i += 1;
            continue;
        }
        if let Some((braces, brackets, parens)) = body_skip {
            let braces = braces + brace_delta(raw);
            let brackets = brackets + bracket_balance(raw);
            let parens = parens + paren_balance(raw);
            depth += brace_delta(raw);
            body_skip = if braces <= 0 && brackets <= 0 && parens <= 0 {
                None
            } else {
                Some((braces, brackets, parens))
            };
            i += 1;
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        let next_opens_body = lines[i + 1..]
            .iter()
            .map(|l| l.trim())
            .find(|l| !l.is_empty())
            .is_some_and(|l| l.starts_with('{'));
        let keep = match family {
            Family::Python => python_keeps(line, indent),
            _ => c_like_keeps(line, depth, next_opens_body, family != Family::Rust),
        };
        if keep
            && family == Family::Python
            && indent == 0
            && let Some(eq) = python_constant_cut(line)
        {
            // A module constant's value is body, as a JS binding's is.
            out.push(normalize_signature(&line[..eq]));
            i += 1;
            continue;
        }
        // A destructuring pattern a formatter wrapped (`const {\n  a,\n  b,\n} = …`)
        // joins across its brackets before the binding rule sees it, so the
        // names inside are interface in the wrapped form as in the one-line form.
        let mut binding_end = i;
        let destructured = if keep && family != Family::Python && opens_destructure(line) {
            let mut joined = line.to_string();
            while brace_delta(&joined) + bracket_balance(&joined) > 0
                && binding_end + 1 < lines.len()
                && binding_end - i < 60
            {
                binding_end += 1;
                if lines[binding_end].trim().is_empty() {
                    continue;
                }
                joined.push(' ');
                joined.push_str(lines[binding_end].trim());
            }
            Some(joined)
        } else {
            None
        };
        let binding_line = destructured.as_deref().unwrap_or(line);
        if keep
            && family != Family::Python
            && !(depth >= 1 && enum_member_re().is_match(line))
            && let Some(cut) = cut_value_binding(binding_line)
        {
            // (An enum member's explicit value is interface, not a binding's
            // body: `Green = 2,` keeps its value.)
            // The value is body: keep the binding's name, count its braces,
            // and skip the value's lines until every bracket it opened closes.
            out.push(normalize_signature(&cut));
            let span = &lines[i..=binding_end];
            let mut opened = (
                span.iter().map(|l| brace_delta(l)).sum::<i32>(),
                span.iter().map(|l| bracket_balance(l)).sum::<i32>(),
                span.iter().map(|l| paren_balance(l)).sum::<i32>(),
            );
            depth += opened.0;
            i = binding_end + 1;
            if binding_line.trim_end().ends_with('=') {
                // The value starts on the next non-empty line: consume it
                // and whatever it opens.
                while i < lines.len() && lines[i].trim().is_empty() {
                    i += 1;
                }
                if i < lines.len() {
                    let v = lines[i];
                    opened = (
                        opened.0 + brace_delta(v),
                        opened.1 + bracket_balance(v),
                        opened.2 + paren_balance(v),
                    );
                    depth += brace_delta(v);
                    i += 1;
                }
            }
            if opened.0 > 0 || opened.1 > 0 || opened.2 > 0 {
                body_skip = Some(opened);
            }
            continue;
        }
        if keep {
            // A signature wrapped across lines joins on its parentheses, a
            // wrapped array literal on its brackets, a wrapped import or
            // export list on its braces — so a formatter's line width is
            // invisible and a member added inside a wrapped list is not.
            let mut sig = line.to_string();
            let mut j = i;
            let c_like = family != Family::Python;
            // Continuation: an open bracket of any kind, or (C-like) a next
            // line a formatter led with an operator (`|` in a union type,
            // `.` in a chain, `?`/`:` in a ternary, `&&`/`||`/`+`).
            let next_is_operator_led = |k: usize| {
                c_like
                    && lines[k + 1..]
                        .iter()
                        .map(|l| l.trim())
                        .find(|l| !l.is_empty())
                        .is_some_and(|l| {
                            l.starts_with('|')
                                || l.starts_with('&')
                                || l.starts_with('?')
                                || l.starts_with(':')
                                || l.starts_with('.')
                                || l.starts_with('+')
                        })
            };
            // A line that ends by opening a body (`{`) joins nothing: what
            // follows is body, even inside a callback argument's parentheses.
            // An import or export list's brace opens a list, not a body.
            // A line that ends in `=` or `:` (a formatter broke the value or
            // the type onto the next line) joins its continuation.
            let ends_open = |s: &str| {
                let t = s.trim_end();
                c_like && (t.ends_with('=') || t.ends_with(':'))
            };
            while (!sig.trim_end().ends_with('{') || is_list_declaration(&sig))
                && (paren_balance(&sig) > 0
                    || bracket_balance(&sig) > 0
                    || (c_like && angle_balance(&sig) > 0)
                    || (is_list_declaration(&sig) && brace_delta(&sig) > 0)
                    || ends_open(&sig)
                    || next_is_operator_led(j))
                && j + 1 < lines.len()
                && j - i < 60
            {
                j += 1;
                if lines[j].trim().is_empty() {
                    continue;
                }
                sig.push(' ');
                sig.push_str(lines[j].trim());
            }
            // A line kept only as a member signature must open a body once
            // joined; a wrapped call statement joins to `name(args)` with no
            // body after it and is dropped, as its one-line form would be.
            // Judged on the joined signature: a typed member a formatter
            // wrapped (`load(\n  id: string,\n): Promise<void>`) is typed.
            let opens_body = sig.trim_end().ends_with('{')
                || sig.contains("=>")
                || lines[j + 1..]
                    .iter()
                    .map(|l| l.trim())
                    .find(|l| !l.is_empty())
                    .is_some_and(|l| l.starts_with('{'));
            if c_like && member_signature_only(&sig, depth) && !opens_body {
                for l in &lines[i..=j] {
                    depth += brace_delta(l);
                }
                i = j + 1;
                continue;
            }
            out.push(normalize_signature(&cut_at_body(&sig, family)));
            if c_like {
                for l in &lines[i..=j] {
                    depth += brace_delta(l);
                }
            }
            i = j + 1;
        } else {
            if family != Family::Python {
                depth += brace_delta(raw);
            }
            i += 1;
        }
    }
    out.join("\n")
}

/// The load-bearing sections of a type, in the type's declared order:
///
/// 1. the sections declaring `load_bearing: true`, when any does;
/// 2. otherwise the required sections, minus any declaring
///    `load_bearing: false`, when that leaves at least one;
/// 3. otherwise every section — a type with no required sections and no
///    declaration has no notes/claim split the engine can honour, and an
///    empty set would hash to a constant that never drifts.
pub fn load_bearing_sections(
    type_def: &memstead_schema::types::TypeDefinition,
) -> Vec<&memstead_schema::types::SectionDef> {
    let explicit: Vec<_> = type_def
        .sections
        .iter()
        .filter(|s| s.load_bearing == Some(true))
        .collect();
    if !explicit.is_empty() {
        return explicit;
    }
    let required: Vec<_> = type_def
        .sections
        .iter()
        .filter(|s| s.required && s.load_bearing != Some(false))
        .collect();
    if !required.is_empty() {
        return required;
    }
    type_def.sections.iter().collect()
}

/// The `entity-load-bearing` prepared form: the entity's load-bearing
/// sections serialized stably — each as `## <key>`, a blank line, the
/// trimmed content, a blank line — in the type's declared section order.
/// Keyed by section KEY (not heading) so a heading rename in the schema
/// does not read as a content change; a section the entity does not carry
/// is skipped. Title, metadata, and relationships are outside the form:
/// the anchor's artifact is the entity id, and a rename orphans the anchor
/// on its own. Without a type definition (a type the mem's schema does not
/// declare) every section the entity carries is load-bearing, in the
/// entity's own order.
pub fn entity_load_bearing_form(
    entity: &Entity,
    type_def: Option<&memstead_schema::types::TypeDefinition>,
) -> String {
    fn push(out: &mut String, key: &str, content: &str) {
        out.push_str("## ");
        out.push_str(key);
        out.push_str("\n\n");
        out.push_str(content.trim());
        out.push_str("\n\n");
    }
    let mut out = String::new();
    match type_def {
        Some(td) => {
            for section in load_bearing_sections(td) {
                if let Some(content) = entity.sections.get(&section.key) {
                    push(&mut out, &section.key, content);
                }
            }
        }
        None => {
            for (key, content) in &entity.sections {
                push(&mut out, key, content);
            }
        }
    }
    out
}

/// Touchpoint A for the `entity` grain: the prepared-content hash of an
/// entity under the source's declared preparation. `None` declares
/// nothing — the canonical rendered markdown, byte-for-byte today's form.
/// [`ENTITY_LOAD_BEARING`] hashes [`entity_load_bearing_form`]. An
/// identifier the registry does not know yields `None`: the form cannot be
/// computed, and observation reports the anchor unobserved rather than
/// hashing a fabricated form (validation refuses such a record at every
/// edit path; only a hand-edited file reaches here).
pub fn entity_prepared_hash(
    entity: &Entity,
    type_def: Option<&memstead_schema::types::TypeDefinition>,
    preparation: Option<&str>,
) -> Option<String> {
    match entity_prepared(entity, type_def, preparation, None) {
        PathPrepared::Hash(h) => Some(h),
        PathPrepared::NoHash | PathPrepared::UnitAbsent => None,
    }
}

/// [`entity_prepared_hash`] with the artifact's `#<locator>` in hand: under
/// [`QUOTED_PHRASE`] the locator is the phrase and the prepared form is the
/// phrase where the entity's canonical markdown still carries it
/// ([`PathPrepared::UnitAbsent`] where it does not); every other
/// preparation ignores the locator and answers as [`entity_prepared_hash`]
/// does, [`PathPrepared::NoHash`] for an identifier the registry does not
/// prepare entities under.
pub fn entity_prepared(
    entity: &Entity,
    type_def: Option<&memstead_schema::types::TypeDefinition>,
    preparation: Option<&str>,
    locator: Option<&str>,
) -> PathPrepared {
    let form = match preparation {
        None => crate::render::render_entity_markdown(entity, None),
        Some(ENTITY_LOAD_BEARING) => entity_load_bearing_form(entity, type_def),
        Some(QUOTED_PHRASE) => {
            let rendered = crate::render::render_entity_markdown(entity, None);
            return match locator {
                Some(phrase) => quoted_phrase_prepared(phrase, &rendered),
                None => PathPrepared::Hash(prepared_content_hash(rendered.as_bytes())),
            };
        }
        Some(_) => return PathPrepared::NoHash,
    };
    PathPrepared::Hash(prepared_content_hash(form.as_bytes()))
}

// ---------------------------------------------------------------------------
// Touchpoint B: delivery units
// ---------------------------------------------------------------------------

/// One delivery unit of a file under a delivery preparation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryUnit {
    /// The unit's key, unique within its file; the addressed form is
    /// `<path>#<key>` ([`unit_id`]).
    pub key: String,
    /// The intrinsic key the source's units sort by (a normalized stamp for
    /// [`DATED_ENTRIES`]); empty for a [`WHOLE_FILE_UNIT`].
    pub order_key: String,
    /// First line of the unit, 1-based.
    pub start_line: usize,
    /// Last line of the unit, 1-based, inclusive.
    pub end_line: usize,
    /// The prepared-content hash of the unit's text — what a span anchor
    /// over the unit records, and what a change run compares.
    pub hash: String,
}

/// How a unit changed between two states of its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UnitChange {
    /// The key is new.
    Added,
    /// The key existed; the unit's text changed.
    Modified,
    /// The key is gone.
    Deleted,
}

/// The addressed form of a unit: `<path>#<key>`.
pub fn unit_id(path: &str, key: &str) -> String {
    format!("{path}#{key}")
}

/// Split an artifact id into its path and, when it addresses a unit, the
/// unit key after the first `#`.
pub fn split_unit_id(id: &str) -> (&str, Option<&str>) {
    match id.find('#') {
        Some(cut) => (&id[..cut], Some(&id[cut + 1..])),
        None => (id, None),
    }
}

/// Touchpoint B: the delivery units of one file's content under a delivery
/// preparation, in file order. `None` when `preparation` is not a registered
/// delivery preparation (the caller keeps file-granularity delivery).
pub fn unitize(preparation: &str, content: &str) -> Option<Vec<DeliveryUnit>> {
    match preparation {
        DATED_ENTRIES => Some(dated_entries(content)),
        _ => None,
    }
}

/// The text of one unit, lines `start_line..=end_line` of `content`.
pub fn unit_text(content: &str, unit: &DeliveryUnit) -> String {
    content
        .lines()
        .skip(unit.start_line.saturating_sub(1))
        .take(unit.end_line + 1 - unit.start_line.max(1))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The units that differ between two states of one file, keyed by unit key:
/// a key only in `after` is [`UnitChange::Added`], a key in both whose hash
/// differs is [`UnitChange::Modified`] (the `after` unit), a key only in
/// `before` is [`UnitChange::Deleted`] (the `before` unit, so its order key
/// still places it). Unchanged units are not delivered again.
pub fn diff_units(
    before: &[DeliveryUnit],
    after: &[DeliveryUnit],
) -> Vec<(DeliveryUnit, UnitChange)> {
    let old: std::collections::BTreeMap<&str, &DeliveryUnit> =
        before.iter().map(|u| (u.key.as_str(), u)).collect();
    let new: std::collections::BTreeMap<&str, &DeliveryUnit> =
        after.iter().map(|u| (u.key.as_str(), u)).collect();
    let mut out = Vec::new();
    for u in after {
        match old.get(u.key.as_str()) {
            None => out.push((u.clone(), UnitChange::Added)),
            Some(prev) if prev.hash != u.hash => out.push((u.clone(), UnitChange::Modified)),
            Some(_) => {}
        }
    }
    for u in before {
        if !new.contains_key(u.key.as_str()) {
            out.push((u.clone(), UnitChange::Deleted));
        }
    }
    out
}

fn dated_entries(content: &str) -> Vec<DeliveryUnit> {
    let lines: Vec<&str> = content.lines().collect();
    let starts: Vec<(usize, String)> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, line)| leading_stamp(line).map(|stamp| (i, stamp)))
        .collect();
    if starts.is_empty() {
        return vec![DeliveryUnit {
            key: WHOLE_FILE_UNIT.to_string(),
            order_key: String::new(),
            start_line: 1,
            end_line: lines.len().max(1),
            hash: prepared_content_hash(content.as_bytes()),
        }];
    }
    let mut seen: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut units = Vec::with_capacity(starts.len());
    for (n, (start, stamp)) in starts.iter().enumerate() {
        // The preamble (anything before the first stamp) folds into the
        // first unit: it is context for the entries, never a unit of its own.
        let from = if n == 0 { 0 } else { *start };
        let to = starts.get(n + 1).map_or(lines.len(), |(next, _)| *next);
        let text = lines[from..to].join("\n");
        let count = seen
            .entry(stamp.clone())
            .and_modify(|c| *c += 1)
            .or_insert(1);
        let key = if *count == 1 {
            stamp.clone()
        } else {
            format!("{stamp}.{count}")
        };
        units.push(DeliveryUnit {
            key,
            order_key: stamp.clone(),
            start_line: from + 1,
            end_line: to,
            hash: prepared_content_hash(text.as_bytes()),
        });
    }
    units
}

/// The ISO stamp a line opens with (after leading markdown markers),
/// normalized to `YYYY-MM-DDTHH:MM:SS`; `None` when the line opens with
/// anything else or the stamp is out of range.
fn leading_stamp(line: &str) -> Option<String> {
    static STAMP: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = STAMP.get_or_init(|| {
        regex::Regex::new(
            r"^(\d{4})-(\d{2})-(\d{2})(?:[T ](\d{2}):(\d{2})(?::(\d{2}))?(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?)?\b",
        )
        .expect("the stamp regex compiles")
    });
    let s = line.trim_start_matches(|c: char| {
        c.is_whitespace() || matches!(c, '#' | '-' | '*' | '>' | '[' | '(' | '|' | '`' | '+')
    });
    let caps = re.captures(s)?;
    let num = |i: usize| -> u32 {
        caps.get(i)
            .map(|m| m.as_str().parse().unwrap_or(0))
            .unwrap_or(0)
    };
    let (y, mo, d, h, mi, sec) = (num(1), num(2), num(3), num(4), num(5), num(6));
    let days_in_month = match mo {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => 29,
        _ => return None,
    };
    if !(1..=days_in_month).contains(&d) || h > 23 || mi > 59 || sec > 59 {
        return None;
    }
    Some(format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{sec:02}"))
}

#[cfg(test)]
mod tests;
