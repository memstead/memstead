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
    let form = match preparation {
        None => crate::render::render_entity_markdown(entity, None),
        Some(ENTITY_LOAD_BEARING) => entity_load_bearing_form(entity, type_def),
        Some(_) => return None,
    };
    Some(prepared_content_hash(form.as_bytes()))
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
mod tests {
    use super::*;
    use crate::entity::EntityId;
    use indexmap::IndexMap;
    use memstead_schema::types::{SectionDef, TypeDefinition};

    fn section(key: &str, required: bool, load_bearing: Option<bool>) -> SectionDef {
        let mut v = serde_json::json!({
            "key": key, "heading": key, "required": required, "search_weight": 1.0
        });
        if let Some(lb) = load_bearing {
            v["load_bearing"] = serde_json::json!(lb);
        }
        serde_json::from_value(v).unwrap()
    }

    /// A real builtin type with its sections replaced — the fixture never
    /// has to track `TypeDefinition`'s required-field roster.
    fn type_with(sections: Vec<SectionDef>) -> TypeDefinition {
        let schemas = memstead_schema::builtins::load_builtin_schemas().unwrap();
        let base = schemas
            .iter()
            .find_map(|s| s.get_type("assertion"))
            .expect("a builtin schema declares `assertion`");
        let mut td = (*base).clone();
        td.sections = sections;
        td
    }

    fn entity(sections: &[(&str, &str)]) -> Entity {
        let mut map = IndexMap::new();
        for (k, v) in sections {
            map.insert(k.to_string(), v.to_string());
        }
        Entity {
            id: EntityId::canonical("m--e"),
            title: "E".into(),
            entity_type: "t".into(),
            mem: "m".into(),
            file_path: "e.md".into(),
            metadata: IndexMap::new(),
            sections: map,
            relationships: Vec::new(),
            content_hash: "h".into(),
            stub: false,
            stub_kind: None,
            heading_spans: Default::default(),
            raw_section_headings: Vec::new(),
        }
    }

    #[test]
    fn registry_knows_its_three_flavours_and_nothing_else() {
        assert!(is_registered(ENTITY_LOAD_BEARING));
        assert!(is_registered(DATED_ENTRIES));
        assert!(is_registered(CODE_MAP));
        assert!(!is_registered("pdf-to-markdown"));
        assert!(!is_registered(""));
        assert_eq!(
            registered_identifiers(),
            vec![ENTITY_LOAD_BEARING, DATED_ENTRIES, CODE_MAP]
        );
        let c = lookup(CODE_MAP).unwrap();
        assert_eq!(c.touchpoint, Touchpoint::PreparedForm);
        assert!(applies_to_namespace(c, "path"));
        assert!(applies_to_namespace(c, "path+commit"));
        assert!(!applies_to_namespace(c, "entity"));
        assert!(!applies_to_namespace(c, "url"));
        assert!(delivery_preparation(Some(CODE_MAP)).is_none());
        let p = lookup(ENTITY_LOAD_BEARING).unwrap();
        assert_eq!(p.touchpoint, Touchpoint::PreparedForm);
        assert!(applies_to_namespace(p, "entity"));
        assert!(!applies_to_namespace(p, "path"));
        assert!(!applies_to_namespace(p, "url"));
        let d = lookup(DATED_ENTRIES).unwrap();
        assert_eq!(d.touchpoint, Touchpoint::DeliveryUnits);
        assert!(applies_to_namespace(d, "path"));
        assert!(applies_to_namespace(d, "path+commit"));
        assert!(!applies_to_namespace(d, "entity"));
        assert!(!applies_to_namespace(d, "url"));
        // Touchpoint B lookup: only a delivery flavour answers.
        assert_eq!(
            delivery_preparation(Some(DATED_ENTRIES)).map(|p| p.id),
            Some(DATED_ENTRIES)
        );
        assert!(delivery_preparation(Some(ENTITY_LOAD_BEARING)).is_none());
        assert!(delivery_preparation(Some("pdf-to-markdown")).is_none());
        assert!(delivery_preparation(None).is_none());
        assert!(unitize(ENTITY_LOAD_BEARING, "x").is_none());
        assert!(unitize("pdf-to-markdown", "x").is_none());
    }

    const LOG: &str = "# Ops log\n\nPreamble text.\n\n## 2026-08-24 10:05 boot\nline a\n\n\
                       - 2026-08-24T10:05:00Z boot again\nline b\n2026-08-25 shutdown\nline c\n";

    /// Unitization: entries open at dated lines, the preamble folds into the
    /// first unit, same-stamp entries get an ordinal, an undated file is one
    /// `whole` unit, and the stamp normalizes across the accepted spellings.
    #[test]
    fn dated_entries_unitize_deterministically() {
        let units = unitize(DATED_ENTRIES, LOG).unwrap();
        let keys: Vec<&str> = units.iter().map(|u| u.key.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "2026-08-24T10:05:00",
                "2026-08-24T10:05:00.2",
                "2026-08-25T00:00:00"
            ]
        );
        assert_eq!(
            units[0].start_line, 1,
            "the preamble folds into the first unit"
        );
        assert_eq!((units[0].end_line, units[1].start_line), (7, 8));
        assert_eq!(units[2].end_line, 11);
        assert_eq!(units[1].order_key, "2026-08-24T10:05:00");
        assert!(unit_text(LOG, &units[2]).starts_with("2026-08-25 shutdown"));
        assert_eq!(
            units[2].hash,
            prepared_content_hash(unit_text(LOG, &units[2]).as_bytes())
        );

        let whole = unitize(DATED_ENTRIES, "no stamps here\njust prose\n").unwrap();
        assert_eq!(whole.len(), 1);
        assert_eq!(whole[0].key, WHOLE_FILE_UNIT);
        assert_eq!(whole[0].order_key, "");

        assert_eq!(
            leading_stamp("[2026-02-30] bad day"),
            None,
            "day out of range"
        );
        assert_eq!(
            leading_stamp("2026-08-24T25:00 x"),
            None,
            "hour out of range"
        );
        assert_eq!(leading_stamp("v2026-08-24"), None, "not at the line start");
        assert_eq!(leading_stamp("2026-08-2400"), None, "digits run on");
        assert_eq!(
            leading_stamp("> **2026-08-24T10:05:00.250+02:00** note").as_deref(),
            Some("2026-08-24T10:05:00")
        );
        assert_eq!(
            unit_id("logs/ops.md", "2026-08-25T00:00:00"),
            "logs/ops.md#2026-08-25T00:00:00"
        );
        assert_eq!(
            split_unit_id("logs/ops.md#2026-08-25T00:00:00"),
            ("logs/ops.md", Some("2026-08-25T00:00:00"))
        );
        assert_eq!(split_unit_id("logs/ops.md"), ("logs/ops.md", None));
    }

    const JS: &str = "// Auth module\nimport axios from 'axios'\nimport { t } from '@/i18n'\n\n/* block\n   comment */\nconst RETRIES = 3\n\nexport default {\n  name: 'Auth',\n  props: ['user'],\n  data() {\n    return { token: null, busy: false }\n  },\n  methods: {\n    async login(user, password) {\n      // body\n      const r = await axios.post('/login', { user, password })\n      return r.data\n    },\n    logout() {\n      this.token = null\n    }\n  }\n}\n\nexport function helper(a, b) {\n  return a + b\n}\n\nexport const LIMIT = { max: 10 }\n";

    /// The code map keeps the interface and nothing else: comments,
    /// formatting and implementation bodies are invisible; a signature or
    /// export change is visible; the digest is the same for JS and for the
    /// script block of a Vue component.
    #[test]
    fn code_map_digest_sees_interfaces_not_bodies() {
        let digest = code_map_digest("src/auth.js", JS);
        assert_eq!(
            digest,
            "import axios from 'axios'\nimport{t}from '@/i18n'\nconst RETRIES=\n\
             export default\nname:\nprops:['user']\ndata()\nmethods:\n\
             async login(user,password)\nlogout()\nexport function helper(a,b)\n\
             export const LIMIT="
        );
        // A top-level value is body whatever its shape: a ternary or a
        // binary expression wrapped by a formatter, a member chain, an
        // object opened with `({`; a function-valued binding keeps its
        // signature, with or without parentheses around a lone parameter.
        let value_forms = [
            "export const base = cfg.API ? cfg.API : 'x'\n",
            "export const base = cfg.API\n  ? cfg.API\n  : 'x'\n",
            "export const base =\n  'a' +\n  'b'\n",
            "export const base = new Client({\n  region: 'eu',\n  retries: 3,\n})\n",
            "export const base = axios\n  .create(cfg)\n  .interceptors\n",
        ];
        let cut: Vec<String> = value_forms
            .iter()
            .map(|t| code_map_digest("cfg.js", t))
            .collect();
        assert!(cut.iter().all(|d| d == "export const base="), "{cut:?}");
        assert_eq!(
            code_map_digest(
                "s.js",
                "const store = new Vuex.Store({\n  state: { n: 1 },\n  mutations: {\n    inc(s) { s.n += 1 }\n  }\n})\n"
            ),
            code_map_digest(
                "s.js",
                "const store = new Vuex.Store({\n  state: { n: 1 },\n  mutations: {\n    inc(s) { s.n += 2 }\n  }\n})\n"
            )
        );
        assert_eq!(
            code_map_digest("d.js", "export default new Vuetify({\n  theme: 'x',\n})\n"),
            "export default"
        );
        assert_eq!(
            code_map_digest("f.js", "const f = x => x.id\n"),
            code_map_digest("f.js", "const f = (x) => x.id\n")
        );
        assert_eq!(
            code_map_digest("f.js", "const f = (x) => x.id\n"),
            "const f=x=>"
        );
        assert_eq!(
            code_map_digest(
                "f.js",
                "export const g = async (a, b) => {\n  return a\n}\n"
            ),
            "export const g=async(a,b)=>"
        );
        // Brace style and quoted keys are formatting.
        let knr = "class S {\n  login(user, password) {\n    return 1\n  }\n  logout() {\n  }\n}\n";
        let allman = "class S\n{\n  login(user, password)\n  {\n    return 1\n  }\n  logout()\n  {\n  }\n}\n";
        assert_eq!(
            code_map_digest("s.js", knr),
            "class S\nlogin(user,password)\nlogout()"
        );
        assert_eq!(
            code_map_digest("s.js", allman),
            code_map_digest("s.js", knr)
        );
        // Formatter line wrapping in every shape a formatter produces: an
        // arrow's expression body after `=>`, a property arrow with or
        // without parentheses, a call statement wrapped inside a body (which
        // never enters the digest), CommonJS `exports` values, a union type
        // led by `|`, rustfmt-wrapped generics.
        let same = |a: &str, b: &str, why: &str| {
            assert_eq!(
                code_map_digest("w.js", a),
                code_map_digest("w.js", b),
                "{why}"
            );
        };
        same(
            "export const pick = state => state.items.filter(i => i.active).map(i => i.id)\n",
            "export const pick = state =>\n  state.items\n    .filter(i => i.active)\n    .map(i => i.id)\n",
            "arrow expression body wrapped",
        );
        assert_eq!(
            code_map_digest("w.js", "export const pick = (state) => state.items\n"),
            "export const pick=state=>"
        );
        same(
            "export default {\n  select: state => state.items.filter(i => i.active),\n}\n",
            "export default {\n  select: state =>\n    state.items.filter(i => i.active),\n}\n",
            "property arrow body wrapped",
        );
        same(
            "module.exports = {\n  validate: (v) => {\n    return v\n  },\n}\n",
            "module.exports = {\n  validate: v => {\n    return v\n  },\n}\n",
            "arrowParens on a property arrow",
        );
        assert_eq!(
            code_map_digest(
                "w.js",
                "module.exports = {\n  validate: v => {\n    return v\n  },\n}\n"
            ),
            "module.exports=\nvalidate:v=>"
        );
        same(
            "export function setup(app) {\n  registerPlugin(app, options, extra)\n}\n",
            "export function setup(app) {\n  registerPlugin(\n    app,\n    options,\n    extra\n  )\n}\n",
            "wrapped call statement in a function body",
        );
        assert_eq!(
            code_map_digest(
                "w.js",
                "export function setup(app) {\n  registerPlugin(\n    app,\n    options,\n    extra\n  )\n}\n"
            ),
            "export function setup(app)"
        );
        same(
            "class S {\n  run() {\n    helper(a, b, c)\n  }\n}\n",
            "class S {\n  run() {\n    helper(\n      a,\n      b,\n      c\n    )\n  }\n}\n",
            "wrapped call in a class method body",
        );
        assert_eq!(
            code_map_digest(
                "s.rs",
                "impl S {\n    pub fn run(&self) {\n        helper(\n            a,\n            b,\n        )\n    }\n}\n"
            ),
            code_map_digest(
                "s.rs",
                "impl S {\n    pub fn run(&self) {\n        helper(a, b)\n    }\n}\n"
            )
        );
        same(
            "exports.base = cfg.API ? cfg.API : 'http://localhost'\n",
            "exports.base = cfg.API\n  ? cfg.API\n  : 'http://localhost'\n",
            "exports ternary wrapped",
        );
        same(
            "module.exports = mongoose.model('User', schema).plugin(paginate)\n",
            "module.exports = mongoose\n  .model('User', schema)\n  .plugin(paginate)\n",
            "module.exports chain wrapped",
        );
        assert_eq!(
            code_map_digest(
                "w.js",
                "exports.TIMEOUT = compute(\n  settings,\n  defaults\n)\n"
            ),
            "exports.TIMEOUT="
        );
        assert_eq!(
            code_map_digest(
                "t.ts",
                "export type Mode = 'discovery' | 'sync' | 'verify'\n"
            ),
            code_map_digest(
                "t.ts",
                "export type Mode =\n  | 'discovery'\n  | 'sync'\n  | 'verify'\n"
            )
        );
        assert_ne!(
            code_map_digest("t.ts", "export type Mode = 'discovery' | 'sync'\n"),
            code_map_digest(
                "t.ts",
                "export type Mode = 'discovery' | 'sync' | 'verify'\n"
            ),
            "a union member is interface"
        );
        assert_eq!(
            code_map_digest(
                "g.rs",
                "pub fn all(&self) -> Result<Vec<String>, Error> {\n    todo!()\n}\n"
            ),
            code_map_digest(
                "g.rs",
                "pub fn all(\n    &self,\n) -> Result<\n    Vec<String>,\n    Error,\n> {\n    todo!()\n}\n"
            )
        );
        // rustfmt and prettier breaking a value or a type onto the next line
        // (`pub const`, a struct field's type, a type alias, a class field, a
        // bare object key); callback bodies never enter the digest; a
        // bracket-opened value is skipped whole.
        assert_eq!(
            code_map_digest(
                "c.rs",
                "pub const DESCRIPTION: &str =\n    \"a long description\";\n"
            ),
            code_map_digest(
                "c.rs",
                "pub const DESCRIPTION: &str = \"a long description\";\n"
            )
        );
        assert_eq!(
            code_map_digest("c.rs", "pub const DESCRIPTION: &str = \"x\";\n"),
            "pub const DESCRIPTION:&str="
        );
        assert_eq!(
            code_map_digest(
                "f.rs",
                "pub struct H {\n    pub handler:\n        Box<dyn Fn(&str) -> Result<(), Error> + Send>,\n}\n"
            ),
            code_map_digest(
                "f.rs",
                "pub struct H {\n    pub handler: Box<dyn Fn(&str) -> Result<(), Error> + Send>,\n}\n"
            )
        );
        assert_eq!(
            code_map_digest(
                "t.rs",
                "pub type Handler =\n    Box<dyn Fn(&str) -> Result<(), Error>>;\n"
            ),
            code_map_digest(
                "t.rs",
                "pub type Handler = Box<dyn Fn(&str) -> Result<(), Error>>;\n"
            )
        );
        assert!(code_map_digest("t.rs", "pub type Handler =\n    Box<X>;\n").contains("Box<X>"));
        same(
            "class Api {\n  static url = 'a' + 'b';\n  private readonly base = x || 'y';\n}\n",
            "class Api {\n  static url =\n    'a' +\n    'b';\n  private readonly base =\n    x || 'y';\n}\n",
            "class fields wrapped after =",
        );
        assert_eq!(
            code_map_digest("w.js", "class Api {\n  static url = 'a';\n}\n"),
            "class Api\nstatic url="
        );
        same(
            "export default {\n  message: 'a' + 'b',\n  data() {\n    return {}\n  },\n}\n",
            "export default {\n  message:\n    'a' +\n    'b',\n  data() {\n    return {}\n  },\n}\n",
            "bare key wrapped away from its value",
        );
        assert!(
            code_map_digest(
                "w.js",
                "export default {\n  message:\n    'a' +\n    'b',\n}\n"
            )
            .contains("message:")
        );
        same(
            "it('logs in', async () => {\n  const r = await login()\n  expect(r).toBe(1)\n})\n",
            "it('logs in', async () => {\n  const r = await login();\n  expect(r).toBe(2);\n});\n",
            "a callback body is body",
        );
        same(
            "export function setup(app) {\n  setTimeout(() => {\n    app.start(1)\n  }, 10)\n}\n",
            "export function setup(app) {\n  setTimeout(() => {\n    app.start(2)\n  }, 10)\n}\n",
            "a callback body inside a function body",
        );
        same(
            "export default {\n  created() {\n    setTimeout(() => {\n      this.a = 1\n    }, 5)\n  },\n}\n",
            "export default {\n  created() {\n    setTimeout(() => {\n      this.a = 2\n    }, 5)\n  },\n}\n",
            "a callback body inside a member body",
        );
        assert_eq!(
            code_map_digest(
                "r.js",
                "export const routes = [\n  { path: '/', meta: { auth: true } },\n  { path: '/x' },\n]\n"
            ),
            "export const routes="
        );
        // Interface and enum members and destructured names are interface.
        let api = "export interface Api {\n  name: string\n  load(id: string): Promise<void>\n}\n";
        assert_eq!(
            code_map_digest("a.ts", api),
            "export interface Api\nname:string\nload(id:string):Promise<void>"
        );
        assert_ne!(
            code_map_digest("a.ts", api),
            code_map_digest("a.ts", &api.replace("name: string", "name: number"))
        );
        assert_ne!(
            code_map_digest("a.ts", api),
            code_map_digest(
                "a.ts",
                &api.replace("load(id: string)", "load(id: string, force: boolean)")
            )
        );
        let color = "export enum Color {\n  Red,\n  Green = 2,\n}\n";
        assert_eq!(
            code_map_digest("e.ts", color),
            "export enum Color\nRed\nGreen=2"
        );
        assert_ne!(
            code_map_digest("e.ts", color),
            code_map_digest("e.ts", &color.replace("Green = 2,", "Green = 2,\n  Blue,"))
        );
        assert_eq!(
            code_map_digest("q.js", "const { a, b } = require('./x')\n"),
            "const{a,b}="
        );
        assert_ne!(
            code_map_digest("q.js", "const { a, b } = require('./x')\n"),
            code_map_digest("q.js", "const { a, c } = require('./x')\n")
        );
        // A formatter's wrap of a typed member and of a destructuring pattern
        // digests as the one-line form, and an edit inside the wrap is seen.
        let wrapped_api = "export interface Api {\n  name: string\n  load(\n    id: string,\n    force: boolean,\n  ): Promise<void>\n}\n";
        assert_eq!(
            code_map_digest("a.ts", wrapped_api),
            code_map_digest(
                "a.ts",
                "export interface Api {\n  name: string\n  load(id: string, force: boolean): Promise<void>\n}\n"
            )
        );
        assert_ne!(
            code_map_digest("a.ts", wrapped_api),
            code_map_digest(
                "a.ts",
                &wrapped_api.replace(
                    "force: boolean,\n",
                    "force: boolean,\n    options: LoadOptions,\n"
                )
            )
        );
        let wrapped_require = "const {\n  a,\n  b,\n} = require('./x')\n";
        assert_eq!(code_map_digest("q.js", wrapped_require), "const{a,b}=");
        assert_ne!(
            code_map_digest("q.js", wrapped_require),
            code_map_digest("q.js", &wrapped_require.replace("  b,\n", "  c,\n"))
        );
        assert_eq!(
            code_map_digest("q.js", "export const [\n  first,\n  second,\n] = pair()\n"),
            "export const[first,second]="
        );
        assert_eq!(
            code_map_digest(
                "o.js",
                "export default {\n  'name': 'X',\n  props: ['a'],\n}\n"
            ),
            code_map_digest(
                "o.js",
                "export default {\n  name: 'X',\n  props: ['a'],\n}\n"
            )
        );
        let h = |text: &str| prepared_content_hash(code_map_digest("src/auth.js", text).as_bytes());
        let base = h(JS);
        // Comment, formatting, body: invisible.
        assert_eq!(h(&JS.replace("// body", "// rewritten comment")), base);
        assert_eq!(h(&JS.replace("  return a + b", "    return   a+b")), base);
        assert_eq!(h(&JS.replace("/login", "/session")), base);
        assert_eq!(h(&JS.replace("return r.data", "return r.data.user")), base);
        assert_eq!(
            h(&JS.replace("max: 10", "max: 20")),
            base,
            "a value is body"
        );
        // Formatting inside a declaration: invisible (comma spacing, a
        // wrapped signature, semicolons, quote style, a comment in the
        // parameter list).
        assert_eq!(
            h(&JS.replace("login(user, password)", "login(user,password)")),
            base
        );
        assert_eq!(
            h(&JS.replace(
                "login(user, password)",
                "login(\n      user,\n      password\n    )"
            )),
            base
        );
        assert_eq!(
            h(&JS.replace("import axios from 'axios'", "import axios from \"axios\";")),
            base
        );
        assert_eq!(
            h(&JS.replace("helper(a, b)", "helper (a /* first */, b)")),
            base
        );
        assert_eq!(
            h(&JS.replace("export const LIMIT = {", "export const LIMIT={")),
            base
        );
        // Formatter-class rewrites: a brace-wrapped import list, a trailing
        // comma on the last member, a wrapped array, a wrapped parameter
        // list with a trailing comma; and a scalar value is body in every form.
        assert_eq!(
            h(&JS.replace(
                "import { t } from '@/i18n'",
                "import {\n  t,\n} from '@/i18n'"
            )),
            base
        );
        assert_eq!(h(&JS.replace("props: ['user'],", "props: ['user']")), base);
        assert_eq!(
            h(&JS.replace("props: ['user'],", "props: [\n    'user',\n  ],")),
            base
        );
        assert_eq!(
            h(&JS.replace(
                "login(user, password)",
                "login(\n      user,\n      password,\n    )"
            )),
            base
        );
        assert_eq!(
            h(&JS.replace("name: 'Auth',", "name: 'Login',")),
            base,
            "a scalar value is body"
        );
        // A member added inside a wrapped import list is visible.
        assert_ne!(
            h(&JS.replace(
                "import { t } from '@/i18n'",
                "import {\n  t,\n  n,\n} from '@/i18n'"
            )),
            base
        );
        assert_eq!(
            code_map_digest("x.js", "export {\n  a,\n  b,\n} from './x'\n"),
            code_map_digest("x.js", "export { a, b } from './x'\n")
        );
        // Signature, export, import: visible.
        assert_ne!(
            h(&JS.replace("login(user, password)", "login(user, password, remember)")),
            base
        );
        assert_ne!(
            h(&JS.replace("export function helper", "function helper")),
            base
        );
        assert_ne!(h(&JS.replace("import axios from 'axios'\n", "")), base);
        assert_ne!(
            h(&JS.replace("props: ['user']", "props: ['user', 'tenant']")),
            base
        );
        // The same script inside a Vue component digests identically; the
        // template and style are not interface.
        let vue = format!(
            "<template>\n  <div @click=\"login\">{{{{ t('hi') }}}}</div>\n</template>\n\n<script>\n{JS}</script>\n\n<style scoped>\n.a {{ color: red }}\n</style>\n"
        );
        assert_eq!(code_map_digest("src/Auth.vue", &vue), digest);
        assert_eq!(
            code_map_digest("src/Auth.vue", &vue.replace("color: red", "color: blue")),
            digest
        );
        // Non-code files are taken whole; JSON canonicalizes formatting away.
        assert_eq!(
            code_map_digest("README.md", "# hi\n\ntext\n"),
            "# hi\n\ntext\n"
        );
        assert_eq!(
            code_map_digest(
                "package.json",
                "{\n  \"name\": \"x\",\n  \"version\": \"1\"\n}\n"
            ),
            code_map_digest("package.json", "{\"name\":\"x\",\"version\":\"1\"}")
        );
    }

    const PY: &str = "# -*- coding: utf-8 -*-\nimport os\nfrom typing import List\n\nTIMEOUT = 30  # seconds\n\n\
                      def load(path: str, *, strict: bool = False) -> List[str]:\n    \"\"\"Docstring.\"\"\"\n    with open(path) as f:\n        return f.readlines()\n\n\
                      class Loader:\n    retries = 3\n\n    @property\n    def name(self):\n        return 'x'\n\n    def run(self,\n            arg):\n        def inner():\n            pass\n        return arg\n";

    #[test]
    fn code_map_digest_python_and_rust() {
        assert_eq!(
            code_map_digest("pivot.py", PY),
            "import os\nfrom typing import List\nTIMEOUT=\n\
             def load(path:str,*,strict:bool=False)->List[str]\nclass Loader\n\
             @property\ndef name(self)\ndef run(self,arg)"
        );
        assert_eq!(
            code_map_digest(
                "pivot.py",
                &PY.replace(
                    "def run(self,\n            arg):",
                    "def run(\n        self,\n        arg,\n    ):"
                )
            ),
            code_map_digest("pivot.py", PY),
            "a formatter's trailing comma in a wrapped def is invisible"
        );
        assert_eq!(
            code_map_digest("i.py", "from typing import (\n    Dict,\n    List,\n)\n"),
            code_map_digest("i.py", "from typing import Dict, List\n"),
            "black's parenthesized import list is formatting"
        );
        let h = |t: &str| prepared_content_hash(code_map_digest("pivot.py", t).as_bytes());
        assert_eq!(
            h(PY),
            h(&PY.replace("return f.readlines()", "return list(f)"))
        );
        assert_eq!(h(PY), h(&PY.replace("Docstring.", "Another docstring.")));
        assert_ne!(
            h(PY),
            h(&PY.replace("def run(self,", "def run(self, extra,"))
        );

        let rs = "//! Module docs\nuse std::fmt;\n\n/// A thing.\n#[derive(Debug)]\npub struct Thing {\n    pub id: u32,\n    secret: String,\n}\n\nimpl Thing {\n    pub fn new(id: u32) -> Self {\n        Self { id, secret: String::new() }\n    }\n    fn hidden(&self) {}\n}\n";
        assert_eq!(
            code_map_digest("src/thing.rs", rs),
            "use std::fmt\n#[derive(Debug)]\npub struct Thing\npub id:u32\nimpl Thing\n\
             pub fn new(id:u32)->Self\nfn hidden(&self)"
        );
    }

    /// The plain tree digest is order-insensitive on input, sorted on
    /// output, and moves on any byte change and on any file joining or
    /// leaving — the whole-content posture of a `file` anchor lifted to the
    /// directory, so a plain `tree` anchor adjudicates deterministically.
    #[test]
    fn plain_tree_digest_is_sorted_and_content_sensitive() {
        let files = vec![
            ("src/b.rs".to_string(), b"fn b() {}\n".to_vec()),
            ("src/a.rs".to_string(), b"fn a() {}\n".to_vec()),
        ];
        let base = plain_tree_digest(&files);
        assert!(
            base.starts_with(&format!(
                "{}  src/a.rs\n",
                prepared_content_hash(b"fn a() {}\n")
            )),
            "rows sort by path regardless of input order"
        );
        let reordered = vec![files[1].clone(), files[0].clone()];
        assert_eq!(plain_tree_digest(&reordered), base);
        let body_edit = vec![
            files[0].clone(),
            ("src/a.rs".to_string(), b"fn a() { /* edit */ }\n".to_vec()),
        ];
        assert_ne!(
            plain_tree_digest(&body_edit),
            base,
            "any byte change moves the plain digest (unlike the code map)"
        );
        let mut joined = files.clone();
        joined.push(("src/c.rs".to_string(), b"fn c() {}\n".to_vec()));
        assert_ne!(plain_tree_digest(&joined), base, "a joining file moves it");
        let left = vec![files[0].clone()];
        assert_ne!(plain_tree_digest(&left), base, "a leaving file moves it");
    }

    /// A tree's map changes when a file joins, leaves, or changes its
    /// interface, and holds when only a body changes; the observation rule
    /// routes each grain to its prepared form.
    #[test]
    fn code_map_tree_digest_and_path_rule() {
        let files = vec![
            ("src/b.js".to_string(), "export const B = 1\n".to_string()),
            ("src/a.js".to_string(), JS.to_string()),
        ];
        let base = code_map_tree_digest(&files);
        assert!(base.starts_with(&format!(
            "{}  src/a.js\n",
            prepared_content_hash(code_map_digest("src/a.js", JS).as_bytes())
        )));
        let body_edit = vec![
            files[0].clone(),
            ("src/a.js".to_string(), JS.replace("/login", "/session")),
        ];
        assert_eq!(
            code_map_tree_digest(&body_edit),
            base,
            "a body edit leaves the tree map"
        );
        let sig_edit = vec![
            files[0].clone(),
            (
                "src/a.js".to_string(),
                JS.replace("logout()", "logout(everywhere)"),
            ),
        ];
        assert_ne!(code_map_tree_digest(&sig_edit), base);
        let mut joined = files.clone();
        joined.push(("src/c.js".to_string(), "export const C = 1\n".to_string()));
        assert_ne!(code_map_tree_digest(&joined), base);

        let digest_hash = prepared_content_hash(code_map_digest("src/a.js", JS).as_bytes());
        assert_eq!(
            path_prepared_hash(Some(CODE_MAP), "src/a.js", AnchorGrain::File, JS.as_bytes()),
            PathPrepared::Hash(digest_hash.clone())
        );
        assert_eq!(
            path_prepared_hash(
                Some(CODE_MAP),
                "src/a.js#L1-L3",
                AnchorGrain::Span,
                JS.as_bytes()
            ),
            PathPrepared::Hash(digest_hash)
        );
        assert_eq!(
            path_prepared_hash(None, "src/a.js", AnchorGrain::File, JS.as_bytes()),
            PathPrepared::Hash(prepared_content_hash(JS.as_bytes())),
            "no preparation: the bytes, byte-for-byte as before"
        );
        assert_eq!(
            path_prepared_hash(Some(CODE_MAP), "src", AnchorGrain::Tree, b""),
            PathPrepared::NoHash,
            "a tree needs enumeration; the caller supplies it"
        );
        assert_eq!(
            path_prepared_hash(None, "src", AnchorGrain::Tree, b""),
            PathPrepared::NoHash
        );
        let log = "2026-08-24 one\nbody\n2026-08-25 two\nbody\n";
        assert!(matches!(
            path_prepared_hash(
                Some(DATED_ENTRIES),
                "log.md#2026-08-25T00:00:00",
                AnchorGrain::Span,
                log.as_bytes()
            ),
            PathPrepared::Hash(_)
        ));
        assert_eq!(
            path_prepared_hash(
                Some(DATED_ENTRIES),
                "log.md#2026-08-26T00:00:00",
                AnchorGrain::Span,
                log.as_bytes()
            ),
            PathPrepared::UnitAbsent
        );
        assert_eq!(
            path_prepared_hash(
                Some(DATED_ENTRIES),
                "log.md",
                AnchorGrain::File,
                log.as_bytes()
            ),
            PathPrepared::Hash(prepared_content_hash(log.as_bytes()))
        );
    }

    /// Measurement harness for a real corpus (the WOENENN acceptance case):
    /// `MEMSTEAD_CODE_MAP_CORPUS=<repo root>` plus `MEMSTEAD_CODE_MAP_ALLOW`
    /// and `MEMSTEAD_CODE_MAP_DENY` (comma-separated globs) and optionally
    /// `MEMSTEAD_CODE_MAP_COMMITS=<n>` (history depth, default 200). Prints
    /// the raw-versus-digest size of the scoped corpus and, over the last n
    /// commits, how many changed scoped files changed their interface. Run
    /// with `--ignored --nocapture`; it is not a pass/fail test.
    #[test]
    #[ignore]
    fn measure_code_map_over_corpus() {
        use crate::pipeline::{MediumType, PatternEntry, PatternMode, Source};
        let Ok(root) = std::env::var("MEMSTEAD_CODE_MAP_CORPUS") else {
            eprintln!("MEMSTEAD_CODE_MAP_CORPUS unset; nothing measured");
            return;
        };
        let root = std::path::PathBuf::from(root);
        let split = |v: &str| -> Vec<String> {
            v.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        };
        let allows = split(&std::env::var("MEMSTEAD_CODE_MAP_ALLOW").unwrap_or_default());
        let denies = split(&std::env::var("MEMSTEAD_CODE_MAP_DENY").unwrap_or_default());
        let commits: usize = std::env::var("MEMSTEAD_CODE_MAP_COMMITS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(200);
        let mut scope: Vec<PatternEntry> = allows
            .iter()
            .map(|p| PatternEntry {
                path: p.clone(),
                mode: PatternMode::Allow,
            })
            .collect();
        scope.extend(denies.iter().map(|p| PatternEntry {
            path: p.clone(),
            mode: PatternMode::Deny,
        }));
        let source = Source {
            name: "corpus".into(),
            medium_type: MediumType::Codebase,
            pointer: String::new(),
            change_detection: Some("git".into()),
            scope,
            engagement: None,
            preparation: Some(CODE_MAP.into()),
        };
        let files = crate::ingest::cursor::enumerate_facet_files(&source, &[], &root);
        let mut by_family: std::collections::BTreeMap<&str, (usize, usize, usize, usize, usize)> =
            std::collections::BTreeMap::new();
        for f in &files {
            let Ok(bytes) = std::fs::read(root.join(f)) else {
                continue;
            };
            let text = String::from_utf8_lossy(&bytes);
            let digest = code_map_digest(f, &text);
            let ext = f.rsplit('.').next().unwrap_or("");
            let fam = match family_of(f) {
                Family::CLike => {
                    if ext == "py" {
                        "py"
                    } else {
                        "js"
                    }
                }
                Family::Rust => "rust",
                Family::Vue => "vue",
                Family::Python => "py",
                Family::Json => "json",
                Family::Text => "other",
            };
            let e = by_family.entry(fam).or_default();
            e.0 += 1;
            e.1 += text.len();
            e.2 += digest.len();
            e.3 += crate::chunking::estimate_tokens(&text);
            e.4 += crate::chunking::estimate_tokens(&digest);
        }
        let (mut n, mut rb, mut db, mut rt, mut dt) = (0, 0, 0, 0, 0);
        eprintln!(
            "| family | files | raw bytes | digest bytes | raw tokens | digest tokens | digest/raw |"
        );
        eprintln!("| --- | --- | --- | --- | --- | --- | --- |");
        for (fam, (c, b1, b2, t1, t2)) in &by_family {
            eprintln!(
                "| {fam} | {c} | {b1} | {b2} | {t1} | {t2} | {:.1}% |",
                100.0 * *b2 as f64 / (*b1).max(1) as f64
            );
            n += c;
            rb += b1;
            db += b2;
            rt += t1;
            dt += t2;
        }
        eprintln!(
            "| total | {n} | {rb} | {db} | {rt} | {dt} | {:.1}% |",
            100.0 * db as f64 / rb.max(1) as f64
        );

        // History: of the scoped files changed by each of the last N commits,
        // how many changed their interface digest?
        let git = |args: &[&str]| -> String {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .expect("git");
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        let mut builder = globset::GlobSetBuilder::new();
        for a in &allows {
            builder.add(globset::Glob::new(a).unwrap());
        }
        let allow_set = builder.build().unwrap();
        let mut dbuilder = globset::GlobSetBuilder::new();
        for d in &denies {
            dbuilder.add(globset::Glob::new(d).unwrap());
        }
        let deny_set = dbuilder.build().unwrap();
        let shas: Vec<String> = git(&["log", "--format=%H", "-n", &commits.to_string(), "--", "."])
            .lines()
            .map(String::from)
            .collect();
        let (mut commits_seen, mut commits_touching, mut commits_interface) =
            (0usize, 0usize, 0usize);
        let (mut files_changed, mut files_interface, mut files_body_only) =
            (0usize, 0usize, 0usize);
        for sha in &shas {
            commits_seen += 1;
            let parent = format!("{sha}~1");
            let names = git(&["diff", "--name-only", &parent, sha]);
            let mut touched = false;
            let mut iface = false;
            for f in names.lines() {
                if !allow_set.is_match(f) || deny_set.is_match(f) {
                    continue;
                }
                let old = git(&["show", &format!("{parent}:{f}")]);
                let new = git(&["show", &format!("{sha}:{f}")]);
                if old.is_empty() || new.is_empty() {
                    continue;
                }
                if prepared_content_hash(old.as_bytes()) == prepared_content_hash(new.as_bytes()) {
                    continue;
                }
                touched = true;
                files_changed += 1;
                if prepared_content_hash(code_map_digest(f, &old).as_bytes())
                    != prepared_content_hash(code_map_digest(f, &new).as_bytes())
                {
                    files_interface += 1;
                    iface = true;
                } else {
                    files_body_only += 1;
                }
            }
            if touched {
                commits_touching += 1;
            }
            if iface {
                commits_interface += 1;
            }
        }
        eprintln!();
        eprintln!(
            "history: {commits_seen} commits inspected, {commits_touching} touched a scoped file's content, {commits_interface} of those changed an interface"
        );
        eprintln!(
            "files: {files_changed} scoped file changes, {files_interface} interface changes, {files_body_only} body-only ({:.1}% of file changes would not drift a code-map anchor)",
            100.0 * files_body_only as f64 / files_changed.max(1) as f64
        );
    }

    /// Keys are stable under growth: appending entries leaves every existing
    /// unit's key and hash untouched, so a change run delivers only the new
    /// unit; an edited entry delivers as modified, a removed one as deleted.
    #[test]
    fn unit_keys_survive_growth_and_diff_delivers_only_what_changed() {
        let before = unitize(DATED_ENTRIES, LOG).unwrap();
        let grown = format!("{LOG}2026-08-26 09:00 restart\nline d\n");
        let after = unitize(DATED_ENTRIES, &grown).unwrap();
        assert_eq!(
            &after[..3],
            &before[..],
            "existing units are byte-identical"
        );
        let delta = diff_units(&before, &after);
        assert_eq!(delta.len(), 1);
        assert_eq!(delta[0].0.key, "2026-08-26T09:00:00");
        assert_eq!(delta[0].1, UnitChange::Added);

        let edited = LOG.replace("line c", "line c, revised");
        let delta = diff_units(&before, &unitize(DATED_ENTRIES, &edited).unwrap());
        assert_eq!(
            delta
                .iter()
                .map(|(u, c)| (u.key.as_str(), *c))
                .collect::<Vec<_>>(),
            vec![("2026-08-25T00:00:00", UnitChange::Modified)]
        );

        let shrunk = LOG.replace("2026-08-25 shutdown\nline c\n", "");
        let delta = diff_units(&before, &unitize(DATED_ENTRIES, &shrunk).unwrap());
        assert_eq!(
            delta
                .iter()
                .map(|(u, c)| (u.key.as_str(), *c))
                .collect::<Vec<_>>(),
            vec![("2026-08-25T00:00:00", UnitChange::Deleted)]
        );
        assert!(diff_units(&before, &before).is_empty());
    }

    #[test]
    fn url_defaults_unstable_every_other_grain_stable() {
        assert_eq!(
            default_hash_stability(AnchorGrain::Url),
            AnchorHashStability::Unstable
        );
        for g in [
            AnchorGrain::Span,
            AnchorGrain::File,
            AnchorGrain::Tree,
            AnchorGrain::Entity,
        ] {
            assert_eq!(default_hash_stability(g), AnchorHashStability::Stable);
        }
    }

    /// The url grain's prepared form IS the path grains' canonicalization:
    /// same bytes, same hash, and the same noise (CRLF, BOM, final newline)
    /// is invisible.
    #[test]
    fn url_prepared_form_is_the_shared_canonicalization() {
        let a = url_prepared_hash(b"<p>hello</p>\n");
        assert_eq!(a, prepared_content_hash(b"<p>hello</p>\n"));
        assert_eq!(a, url_prepared_hash(b"\xEF\xBB\xBF<p>hello</p>\r\n\r\n"));
        assert_ne!(a, url_prepared_hash(b"<p>hello!</p>\n"));
        assert_eq!(
            supplied_content_hash(AnchorGrain::Url, b"<p>hello</p>").as_deref(),
            Some(a.as_str())
        );
        assert!(supplied_content_hash(AnchorGrain::File, b"x").is_some());
        assert!(supplied_content_hash(AnchorGrain::Span, b"x").is_some());
        assert!(supplied_content_hash(AnchorGrain::Tree, b"x").is_none());
        assert!(supplied_content_hash(AnchorGrain::Entity, b"x").is_none());
    }

    #[test]
    fn load_bearing_resolves_explicit_then_required_then_all() {
        let explicit = type_with(vec![
            section("claim", true, Some(true)),
            section("evidence", true, Some(false)),
            section("notes", false, None),
        ]);
        let keys: Vec<_> = load_bearing_sections(&explicit)
            .iter()
            .map(|s| s.key.as_str())
            .collect();
        assert_eq!(keys, vec!["claim"]);

        let required = type_with(vec![
            section("claim", true, None),
            section("evidence", true, Some(false)),
            section("notes", false, None),
        ]);
        let keys: Vec<_> = load_bearing_sections(&required)
            .iter()
            .map(|s| s.key.as_str())
            .collect();
        assert_eq!(
            keys,
            vec!["claim"],
            "a required section opted out is excluded"
        );

        let none = type_with(vec![section("a", false, None), section("b", false, None)]);
        let keys: Vec<_> = load_bearing_sections(&none)
            .iter()
            .map(|s| s.key.as_str())
            .collect();
        assert_eq!(keys, vec!["a", "b"], "no declaration at all: every section");
    }

    /// The anker metric, mechanised: a notes-only edit leaves the prepared
    /// hash intact; a load-bearing edit breaks it.
    #[test]
    fn notes_edit_keeps_the_hash_load_bearing_edit_breaks_it() {
        let td = type_with(vec![
            section("decision", true, None),
            section("notes", false, None),
        ]);
        let base = entity(&[("decision", "We ship."), ("notes", "first draft")]);
        let notes_edit = entity(&[("decision", "We ship."), ("notes", "first draft, revised")]);
        let claim_edit = entity(&[("decision", "We do not ship."), ("notes", "first draft")]);
        let h = |e: &Entity| entity_prepared_hash(e, Some(&td), Some(ENTITY_LOAD_BEARING)).unwrap();
        assert_eq!(h(&base), h(&notes_edit));
        assert_ne!(h(&base), h(&claim_edit));

        // The default form (no preparation) sees BOTH edits — today's
        // behaviour, byte-for-byte the canonical rendered markdown.
        let d = |e: &Entity| entity_prepared_hash(e, Some(&td), None).unwrap();
        assert_ne!(d(&base), d(&notes_edit));
        assert_eq!(
            d(&base),
            prepared_content_hash(crate::render::render_entity_markdown(&base, None).as_bytes())
        );

        // An unregistered identifier computes nothing.
        assert!(entity_prepared_hash(&base, Some(&td), Some("pdf-to-markdown")).is_none());
    }

    /// Content moving between two load-bearing sections changes the form
    /// (keys are part of it); trailing whitespace inside a section does not.
    #[test]
    fn form_is_keyed_and_trimmed() {
        let td = type_with(vec![
            section("claim", true, None),
            section("evidence", true, None),
        ]);
        let a = entity(&[("claim", "x"), ("evidence", "y")]);
        let b = entity(&[("claim", "y"), ("evidence", "x")]);
        let c = entity(&[("claim", "x  \n\n"), ("evidence", "\n y")]);
        let form = |e: &Entity| entity_load_bearing_form(e, Some(&td));
        assert_ne!(form(&a), form(&b));
        assert_eq!(form(&a), form(&c));
        assert_eq!(form(&a), "## claim\n\nx\n\n## evidence\n\ny\n\n");
        // No type definition: every section the entity carries, its order.
        assert_eq!(
            entity_load_bearing_form(&entity(&[("z", "1"), ("a", "2")]), None),
            "## z\n\n1\n\n## a\n\n2\n\n"
        );
    }
}
