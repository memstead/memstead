//! Anchors — engine-owned durable provenance records tying an entity to
//! the source artifacts it describes.
//!
//! An anchor is the projection pipeline's single new load-bearing
//! primitive: which artifact (in the medium's own namespace), at which
//! *grain*, under which *provenance class*, at which medium-typed
//! *version*, hashed over the **prepared** artifact form (never raw
//! bytes) where the class carries hash semantics, and the medium's
//! declared *hash stability* — so an unstable-source hash break resolves
//! as [`AnchorState::Recheck`], not [`AnchorState::Drifted`].
//!
//! ## Naming
//!
//! The `Anchor*` family is deliberately distinct from the three
//! provenance-adjacent type families already in the tree — it never
//! reuses `Provenance` / `ProvenanceKind` (the mutation-log record in
//! [`crate::provenance`]), nor `ArchiveProvenance` / `EntityProvenance` /
//! `History` (the authoring-provenance payload in
//! [`memstead_schema::archive_provenance`]). Those stay; anchors are a
//! separate concern (source→entity provenance, not mutation history nor
//! authoring lineage).
//!
//! ## Wire vocabulary (fixed contract)
//!
//! - provenance classes: `anchored` / `derived` / `authored` /
//!   `informed-by`
//! - grains: `span` / `file` / `tree` / `url` / `entity`
//! - hash stability: `stable` / `unstable`
//! - resolution states: `resolves` / `drifted` / `recheck` / `orphaned`
//!
//! The Rust identifiers around this vocabulary are the implementer's
//! choice; the wire strings are the contract and are locked by the
//! `*_wire_strings_are_stable` tests below.
//!
//! ## Storage
//!
//! Anchors persist in an engine-owned sidecar on the mem branch under
//! [`ANCHOR_SIDECAR_PATH`] (`.memstead/anchors.json`) — see
//! [`AnchorSidecar`]. The sidecar is written only through engine commits
//! (the [`crate::backend::MemBackend`] sidecar seam); every external
//! reader already filters the `.memstead/` namespace, so an anchor-only
//! commit yields no entity deltas and does not participate in `_hash`.
//!
//! ## Scope of this module
//!
//! Pure value types, wire (de)serialisation, validation (typed
//! `INVALID_ANCHOR` refusals with recovery detail), and the resolution
//! model. No storage or IO lives here — the backend seam and the
//! mutation/CLI wiring consume these types.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Mem-relative path of the engine-owned anchors sidecar on the mem
/// branch. Lives under the `.memstead/` umbrella every external reader
/// already treats as non-entity, so an anchor-only commit produces zero
/// entity deltas.
pub const ANCHOR_SIDECAR_PATH: &str = ".memstead/anchors.json";

/// Current sidecar document schema version.
pub const ANCHOR_SIDECAR_VERSION: u32 = 1;

/// Stable typed error code returned when an `anchors[]` element is
/// malformed. Mirrors the engine's other typed-envelope codes; the whole
/// mutation refuses and the entity is not written.
pub const INVALID_ANCHOR_CODE: &str = "INVALID_ANCHOR";

// ---------------------------------------------------------------------------
// Provenance class
// ---------------------------------------------------------------------------

/// The epistemic standing of an anchor — how the entity relates to the
/// artifact it references.
///
/// - [`Anchored`](Self::Anchored) — the entity directly reflects specific
///   artifact content (carries hash semantics).
/// - [`Derived`](Self::Derived) — the entity was computed/synthesised from
///   one or more input artifacts (carries hash semantics; lists inputs).
/// - [`Authored`](Self::Authored) — a human/agent authored the entity with
///   the artifact in view (no hash semantics; excluded from drift
///   adjudication).
/// - [`InformedBy`](Self::InformedBy) — the artifact informed the entity
///   without a content-fidelity claim (no hash semantics; excluded from
///   drift adjudication).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AnchorProvenanceClass {
    Anchored,
    Derived,
    Authored,
    InformedBy,
}

impl AnchorProvenanceClass {
    /// Every wire string, in declaration order — the allowed set a
    /// refusal echoes for recovery.
    pub const WIRE_VALUES: &'static [&'static str] =
        &["anchored", "derived", "authored", "informed-by"];

    /// Stable wire form.
    pub fn as_wire(&self) -> &'static str {
        match self {
            AnchorProvenanceClass::Anchored => "anchored",
            AnchorProvenanceClass::Derived => "derived",
            AnchorProvenanceClass::Authored => "authored",
            AnchorProvenanceClass::InformedBy => "informed-by",
        }
    }

    /// Inverse of [`Self::as_wire`]; `None` for an unknown string so the
    /// validator can refuse it typed rather than misclassify.
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "anchored" => Some(AnchorProvenanceClass::Anchored),
            "derived" => Some(AnchorProvenanceClass::Derived),
            "authored" => Some(AnchorProvenanceClass::Authored),
            "informed-by" => Some(AnchorProvenanceClass::InformedBy),
            _ => None,
        }
    }

    /// Whether this class carries hash semantics. `anchored` and
    /// `derived` assert content fidelity and participate in hash-drift
    /// adjudication; `authored` and `informed-by` do not — a content
    /// change under them produces no drift state, and supplying a hash on
    /// them is a validation refusal.
    pub fn is_hash_bearing(&self) -> bool {
        matches!(
            self,
            AnchorProvenanceClass::Anchored | AnchorProvenanceClass::Derived
        )
    }
}

// ---------------------------------------------------------------------------
// Grain
// ---------------------------------------------------------------------------

/// The granularity of the artifact reference an anchor carries.
///
/// `span` / `file` / `tree` select within a path-shaped namespace; `url`
/// selects a web resource; `entity` selects another mem's entity. The
/// medium-capability matrix ([`crate::binding::medium_capabilities`])
/// decides which grains a given medium's namespace can support — a
/// mismatch (e.g. `span` on a `url`-namespace medium) refuses typed at
/// validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnchorGrain {
    Span,
    File,
    Tree,
    Url,
    Entity,
}

impl AnchorGrain {
    /// Every wire string, in declaration order.
    pub const WIRE_VALUES: &'static [&'static str] = &["span", "file", "tree", "url", "entity"];

    /// Stable wire form.
    pub fn as_wire(&self) -> &'static str {
        match self {
            AnchorGrain::Span => "span",
            AnchorGrain::File => "file",
            AnchorGrain::Tree => "tree",
            AnchorGrain::Url => "url",
            AnchorGrain::Entity => "entity",
        }
    }

    /// Inverse of [`Self::as_wire`]; `None` for an unknown string.
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "span" => Some(AnchorGrain::Span),
            "file" => Some(AnchorGrain::File),
            "tree" => Some(AnchorGrain::Tree),
            "url" => Some(AnchorGrain::Url),
            "entity" => Some(AnchorGrain::Entity),
            _ => None,
        }
    }

    /// Whether this grain can be expressed in the medium's declared anchor
    /// namespace (the `anchor_namespace` string from the E2 capability
    /// matrix: `path` / `path+commit` / `entity` / `url`).
    ///
    /// - `span` / `file` / `tree` require a path-shaped namespace
    ///   (`path` or `path+commit`);
    /// - `url` requires the `url` namespace;
    /// - `entity` requires the `entity` namespace.
    pub fn supported_by_namespace(&self, anchor_namespace: &str) -> bool {
        let path_shaped = matches!(anchor_namespace, "path" | "path+commit");
        match self {
            AnchorGrain::Span | AnchorGrain::File | AnchorGrain::Tree => path_shaped,
            AnchorGrain::Url => anchor_namespace == "url",
            AnchorGrain::Entity => anchor_namespace == "entity",
        }
    }
}

// ---------------------------------------------------------------------------
// Hash stability
// ---------------------------------------------------------------------------

/// The medium's declared hash stability — whether a change in the
/// prepared-content hash is a reliable drift signal.
///
/// A `stable` medium's hash break resolves [`AnchorState::Drifted`]; an
/// `unstable` medium's hash break resolves [`AnchorState::Recheck`]
/// (the hash may have moved for reasons unrelated to the entity's claim,
/// so the engine flags it for re-examination rather than asserting drift).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnchorHashStability {
    Stable,
    Unstable,
}

impl AnchorHashStability {
    /// Every wire string.
    pub const WIRE_VALUES: &'static [&'static str] = &["stable", "unstable"];

    /// Stable wire form.
    pub fn as_wire(&self) -> &'static str {
        match self {
            AnchorHashStability::Stable => "stable",
            AnchorHashStability::Unstable => "unstable",
        }
    }

    /// Inverse of [`Self::as_wire`]; `None` for an unknown string.
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "stable" => Some(AnchorHashStability::Stable),
            "unstable" => Some(AnchorHashStability::Unstable),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Medium-typed version
// ---------------------------------------------------------------------------

/// A medium-typed pinned version the anchor was recorded against.
///
/// Which variant applies follows from the medium's namespace: a git /
/// `path+commit` medium pins a [`Commit`](Self::Commit); a graph / `entity`
/// medium pins a [`Snapshot`](Self::Snapshot) token; a web / `url` medium
/// pins an [`Etag`](Self::Etag). A plain `path` medium (mtime change
/// signal, no retrievable version) records **absent** — represented as
/// `None` on [`Anchor::at_version`], never a variant here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "lowercase")]
pub enum AnchorVersion {
    /// A git commit id (`path+commit` / git namespace).
    Commit(String),
    /// A graph snapshot token (`entity` namespace).
    Snapshot(String),
    /// A web ETag (`url` namespace).
    Etag(String),
}

// ---------------------------------------------------------------------------
// Anchor
// ---------------------------------------------------------------------------

/// One durable anchor record: an entity's provenance tie to a single
/// source artifact.
///
/// This is the persisted + read shape. Malformed wire input is refused
/// upstream via [`AnchorInput::validate`], which produces this strict type
/// only when every rule holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    /// Artifact reference in the medium's own namespace — a repo-relative
    /// path, a `path@commit`, a URL, or an entity id, interpreted per
    /// [`Self::grain`] and the medium.
    pub artifact: String,
    /// The granularity of [`Self::artifact`].
    pub grain: AnchorGrain,
    /// The anchor's epistemic standing.
    pub class: AnchorProvenanceClass,
    /// The medium-typed pinned version, or `None` when the medium has no
    /// retrievable version (plain `path` / mtime).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_version: Option<AnchorVersion>,
    /// Content hash over the **prepared** artifact form (never raw bytes),
    /// present only when [`Self::class`] carries hash semantics. `None`
    /// for `authored` / `informed-by`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// The medium's declared hash stability — governs whether a hash break
    /// resolves `drifted` or `recheck`.
    pub hash_stability: AnchorHashStability,
    /// For a `derived` class: the input artifact refs the entity was
    /// derived from. Empty for every other class.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derived_from: Vec<String>,
    /// `hash(D)` of the binding that produced this anchor (E2), when a
    /// binding produced it. `None` for a manually-authored anchor with no
    /// producing binding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<String>,
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// A permissive wire-shaped anchor element as it arrives on a mutation's
/// `anchors[]` parameter. All fields are optional / string-typed so an
/// unknown class or grain surfaces as a typed [`AnchorValidationError`]
/// with recovery detail rather than an opaque serde failure. Call
/// [`Self::validate`] to obtain a strict [`Anchor`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnchorInput {
    #[serde(default)]
    pub artifact: Option<String>,
    #[serde(default)]
    pub grain: Option<String>,
    #[serde(default)]
    pub class: Option<String>,
    #[serde(default)]
    pub at_version: Option<AnchorVersion>,
    #[serde(default)]
    pub hash: Option<String>,
    #[serde(default)]
    pub hash_stability: Option<String>,
    #[serde(default)]
    pub derived_from: Option<Vec<String>>,
    #[serde(default)]
    pub binding: Option<String>,
}

/// A typed `INVALID_ANCHOR` refusal. The whole mutation refuses and the
/// entity is not written; [`Self::detail`] carries the recovery payload
/// (offending value + allowed set) the agent fixes from.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AnchorValidationError {
    /// Provenance class is absent or not one of the allowed wire strings.
    #[error("unknown anchor provenance class {got:?}; allowed: {}", allowed.join(", "))]
    UnknownClass {
        got: Option<String>,
        allowed: &'static [&'static str],
    },
    /// Grain is absent or not one of the allowed wire strings.
    #[error("unknown anchor grain {got:?}; allowed: {}", allowed.join(", "))]
    UnknownGrain {
        got: Option<String>,
        allowed: &'static [&'static str],
    },
    /// Hash stability, when supplied, is not an allowed wire string.
    #[error("unknown anchor hash stability {got:?}; allowed: {}", allowed.join(", "))]
    UnknownHashStability {
        got: String,
        allowed: &'static [&'static str],
    },
    /// The artifact reference is missing or empty.
    #[error("anchor is missing its artifact reference")]
    MissingArtifact,
    /// A content hash was supplied on a class that carries no hash
    /// semantics (`authored` / `informed-by`).
    #[error("anchor class '{class}' carries no hash semantics — a content hash is not permitted")]
    HashOnNonHashClass { class: &'static str },
    /// The grain cannot be expressed in the medium's anchor namespace
    /// (per the E2 capability matrix), e.g. `span` on a non-path medium.
    #[error(
        "anchor grain '{grain}' is unsupported by a '{medium_type}' medium: its \
         '{anchor_namespace}' namespace does not admit that grain"
    )]
    GrainNamespaceUnsupported {
        grain: &'static str,
        medium_type: String,
        anchor_namespace: &'static str,
    },
}

impl AnchorValidationError {
    /// The stable typed code — always [`INVALID_ANCHOR_CODE`].
    pub fn code(&self) -> &'static str {
        INVALID_ANCHOR_CODE
    }

    /// Structured recovery detail for the typed envelope: the offending
    /// field, its bad value, and the allowed set where one applies.
    pub fn detail(&self) -> BTreeMap<String, serde_json::Value> {
        let mut d = BTreeMap::new();
        match self {
            AnchorValidationError::UnknownClass { got, allowed } => {
                d.insert("field".into(), "class".into());
                d.insert("got".into(), serde_json::json!(got));
                d.insert("allowed".into(), serde_json::json!(allowed));
            }
            AnchorValidationError::UnknownGrain { got, allowed } => {
                d.insert("field".into(), "grain".into());
                d.insert("got".into(), serde_json::json!(got));
                d.insert("allowed".into(), serde_json::json!(allowed));
            }
            AnchorValidationError::UnknownHashStability { got, allowed } => {
                d.insert("field".into(), "hash_stability".into());
                d.insert("got".into(), serde_json::json!(got));
                d.insert("allowed".into(), serde_json::json!(allowed));
            }
            AnchorValidationError::MissingArtifact => {
                d.insert("field".into(), "artifact".into());
            }
            AnchorValidationError::HashOnNonHashClass { class } => {
                d.insert("field".into(), "hash".into());
                d.insert("class".into(), serde_json::json!(class));
            }
            AnchorValidationError::GrainNamespaceUnsupported {
                grain,
                medium_type,
                anchor_namespace,
            } => {
                d.insert("field".into(), "grain".into());
                d.insert("grain".into(), serde_json::json!(grain));
                d.insert("medium_type".into(), serde_json::json!(medium_type));
                d.insert(
                    "anchor_namespace".into(),
                    serde_json::json!(anchor_namespace),
                );
            }
        }
        d
    }
}

impl AnchorInput {
    /// Validate this wire element into a strict [`Anchor`], or refuse
    /// typed.
    ///
    /// `medium` — the resolving medium's `(type_name, anchor_namespace)`
    /// pair, when the mutation resolved one. When `Some`, the grain is
    /// checked against the namespace (the capability-matrix refusal);
    /// when `None` (no medium context — a manually-authored anchor), the
    /// namespace check is skipped and only the vocabulary + hash-semantics
    /// rules apply.
    ///
    /// Rules enforced (each a typed [`AnchorValidationError`]):
    /// - class present and known;
    /// - grain present and known;
    /// - artifact reference present and non-empty;
    /// - a hash is supplied only on a hash-bearing class;
    /// - hash stability, when supplied, is a known wire string (defaults
    ///   to `stable` when absent);
    /// - grain supported by the medium's namespace (when `medium` given).
    pub fn validate(&self, medium: Option<(&str, &str)>) -> Result<Anchor, AnchorValidationError> {
        let class = match self
            .class
            .as_deref()
            .and_then(AnchorProvenanceClass::from_wire)
        {
            Some(c) => c,
            None => {
                return Err(AnchorValidationError::UnknownClass {
                    got: self.class.clone(),
                    allowed: AnchorProvenanceClass::WIRE_VALUES,
                });
            }
        };
        let grain = match self.grain.as_deref().and_then(AnchorGrain::from_wire) {
            Some(g) => g,
            None => {
                return Err(AnchorValidationError::UnknownGrain {
                    got: self.grain.clone(),
                    allowed: AnchorGrain::WIRE_VALUES,
                });
            }
        };

        let artifact = self
            .artifact
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or(AnchorValidationError::MissingArtifact)?;

        // Hash stability: default `stable` when absent; refuse an unknown
        // supplied value.
        let hash_stability = match self.hash_stability.as_deref() {
            None => AnchorHashStability::Stable,
            Some(s) => AnchorHashStability::from_wire(s).ok_or_else(|| {
                AnchorValidationError::UnknownHashStability {
                    got: s.to_string(),
                    allowed: AnchorHashStability::WIRE_VALUES,
                }
            })?,
        };

        // A hash is only meaningful on a hash-bearing class.
        let hash = self
            .hash
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if hash.is_some() && !class.is_hash_bearing() {
            return Err(AnchorValidationError::HashOnNonHashClass {
                class: class.as_wire(),
            });
        }

        // Grain must be expressible in the medium's namespace.
        if let Some((medium_type, namespace)) = medium
            && !grain.supported_by_namespace(namespace)
        {
            // Resolve the namespace to its `&'static str` so the error
            // carries a stable value even though the input came borrowed.
            let anchor_namespace = match namespace {
                "path" => "path",
                "path+commit" => "path+commit",
                "entity" => "entity",
                "url" => "url",
                _ => "path",
            };
            return Err(AnchorValidationError::GrainNamespaceUnsupported {
                grain: grain.as_wire(),
                medium_type: medium_type.to_string(),
                anchor_namespace,
            });
        }

        Ok(Anchor {
            artifact,
            grain,
            class,
            at_version: self.at_version.clone(),
            hash,
            hash_stability,
            derived_from: self.derived_from.clone().unwrap_or_default(),
            binding: self
                .binding
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        })
    }
}

// ---------------------------------------------------------------------------
// Prepared-content hash
// ---------------------------------------------------------------------------

/// Compute the **prepared-content hash** of a path-grain artifact's bytes —
/// the value [`Anchor::hash`] records and hash-drift adjudication compares.
///
/// The prepared form is a deliberate, minimal canonicalization that keeps the
/// hash stable across meaningless byte noise while preserving every
/// content-bearing byte. For UTF-8 text:
///
/// - a leading BOM (U+FEFF) is stripped;
/// - CRLF / lone-CR line endings normalize to LF;
/// - trailing newlines are trimmed (final-newline presence is noise).
///
/// Interior whitespace is untouched — trailing spaces inside a line can be
/// content (markdown hard breaks), so only the two classic cross-tool noise
/// sources (encoding marks, line-ending convention) and the final-newline
/// question are canonicalized. Non-UTF-8 (binary) bytes hash as-is — no text
/// canonicalization applies to them.
///
/// The hash form reuses the house convention — SHA-256, lowercase hex,
/// truncated to 16 characters — shared by entity content hashes
/// ([`crate::entity::parser::compute_hash`]) and the change-detection digest
/// aggregate, so the engine keeps one hash shape rather than growing a
/// second normalization.
pub fn prepared_content_hash(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    let digest = match std::str::from_utf8(bytes) {
        Ok(text) => {
            let text = text.strip_prefix('\u{feff}').unwrap_or(text);
            let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
            Sha256::digest(normalized.trim_end_matches('\n').as_bytes())
        }
        Err(_) => Sha256::digest(bytes),
    };
    crate::hex_lower(&digest)[..16].to_string()
}

/// One verify-observed prepared-content hash, addressed to the anchor(s) it
/// backfills: the `(entity, artifact)` pair a hash-less hash-bearing anchor
/// is keyed by in the sidecar, plus the hash the observation computed. The
/// verify pass collects these; the engine's sidecar writer records them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedArtifactHash {
    /// The entity id (`mem--slug`) whose anchor the hash belongs to.
    pub entity: String,
    /// The anchor's artifact reference, exactly as stored.
    pub artifact: String,
    /// The prepared-content hash observed for the artifact.
    pub hash: String,
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// The resolved state of one anchor against the current medium.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnchorState {
    /// The artifact is present and matches (hash equal, or a non-hash
    /// class whose artifact still exists).
    Resolves,
    /// The artifact is present but its prepared-content hash differs and
    /// the medium is `stable` — a real content drift.
    Drifted,
    /// The artifact is present but drift cannot be asserted — the medium
    /// is `unstable`, or the hash is unavailable on one side. Flagged for
    /// re-examination, never reported as drift.
    Recheck,
    /// The artifact the anchor references is no longer present in the
    /// medium.
    Orphaned,
}

impl AnchorState {
    /// Stable wire form.
    pub fn as_wire(&self) -> &'static str {
        match self {
            AnchorState::Resolves => "resolves",
            AnchorState::Drifted => "drifted",
            AnchorState::Recheck => "recheck",
            AnchorState::Orphaned => "orphaned",
        }
    }
}

/// What the engine observed about an anchor's artifact when resolving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactObservation {
    /// The artifact could not be found in the medium.
    Absent,
    /// The artifact is present; `current_hash` is its prepared-content
    /// hash when the medium could compute one (`None` when the medium has
    /// no hash for it this pass — e.g. enumeration without preparation).
    Present { current_hash: Option<String> },
}

/// Resolve one anchor against a current observation, honouring the class's
/// hash semantics and the medium's declared stability.
///
/// - `authored` / `informed-by` are excluded from hash-drift adjudication:
///   they [`Resolves`](AnchorState::Resolves) as long as the artifact
///   exists, [`Orphaned`](AnchorState::Orphaned) when it does not — a
///   content change never produces a drift state for them.
/// - `anchored` / `derived` compare the recorded prepared-content hash to
///   the current one: equal ⇒ resolves; different ⇒ `drifted` on a stable
///   medium, `recheck` on an unstable one; unavailable on either side ⇒
///   `recheck` (cannot adjudicate).
pub fn resolve_anchor(anchor: &Anchor, observation: &ArtifactObservation) -> AnchorState {
    let current_hash = match observation {
        ArtifactObservation::Absent => return AnchorState::Orphaned,
        ArtifactObservation::Present { current_hash } => current_hash,
    };
    if !anchor.class.is_hash_bearing() {
        return AnchorState::Resolves;
    }
    match (&anchor.hash, current_hash) {
        (Some(recorded), Some(current)) if recorded == current => AnchorState::Resolves,
        (Some(_), Some(_)) => match anchor.hash_stability {
            AnchorHashStability::Stable => AnchorState::Drifted,
            AnchorHashStability::Unstable => AnchorState::Recheck,
        },
        // Missing hash on either side — cannot adjudicate drift.
        _ => AnchorState::Recheck,
    }
}

/// Per-entity provenance-class + grain composition, computed from an
/// entity's anchor list. Tree-grain fan-out is surfaced distinctly so a
/// single entity anchored to a large tree is never laundered into
/// full per-file credit — the count of tree anchors is visible on its own
/// axis, and downstream (E3b) reads the fan-out counts from resolution.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityAnchorComposition {
    /// Anchor count keyed by provenance-class wire string.
    pub by_class: BTreeMap<String, usize>,
    /// Anchor count keyed by grain wire string.
    pub by_grain: BTreeMap<String, usize>,
    /// The `derived_from` input lists of every `derived` anchor, in
    /// anchor order — E3b's derived-input provenance.
    pub derived_inputs: Vec<Vec<String>>,
    /// Artifact refs of every `tree`-grain anchor — the fan-out axis. A
    /// tree anchor is one row here regardless of how many files the tree
    /// contains; the file count is an observation resolution supplies, not
    /// a credit this composition grants.
    pub tree_grain_artifacts: Vec<String>,
}

/// Compose an entity's anchors into class/grain counts, derived inputs,
/// and the tree-grain fan-out axis.
pub fn compose_entity_anchors(anchors: &[Anchor]) -> EntityAnchorComposition {
    let mut comp = EntityAnchorComposition::default();
    for a in anchors {
        *comp
            .by_class
            .entry(a.class.as_wire().to_string())
            .or_insert(0) += 1;
        *comp
            .by_grain
            .entry(a.grain.as_wire().to_string())
            .or_insert(0) += 1;
        if a.class == AnchorProvenanceClass::Derived {
            comp.derived_inputs.push(a.derived_from.clone());
        }
        if a.grain == AnchorGrain::Tree {
            comp.tree_grain_artifacts.push(a.artifact.clone());
        }
    }
    comp
}

// ---------------------------------------------------------------------------
// Sidecar document
// ---------------------------------------------------------------------------

/// The engine-owned anchors sidecar document persisted at
/// [`ANCHOR_SIDECAR_PATH`] on the mem branch: entity id → its anchors.
///
/// Written only through engine commits (the [`crate::backend::MemBackend`]
/// sidecar seam). Rename rewrites the key atomically in the same commit as
/// the entity move; delete drops the key in the same commit as the entity
/// delete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorSidecar {
    /// Document schema version.
    pub version: u32,
    /// Entity id (`mem--slug`) → its anchors. An entity with no anchors
    /// carries no key (an empty vec is pruned on write).
    #[serde(default)]
    pub entities: BTreeMap<String, Vec<Anchor>>,
}

impl Default for AnchorSidecar {
    fn default() -> Self {
        Self {
            version: ANCHOR_SIDECAR_VERSION,
            entities: BTreeMap::new(),
        }
    }
}

impl AnchorSidecar {
    /// Parse sidecar bytes; an absent/empty payload yields an empty
    /// document so callers need not special-case a fresh mem.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(Self::default());
        }
        serde_json::from_slice(bytes)
    }

    /// Serialise to canonical pretty JSON with a trailing newline —
    /// diff-friendly on the mem branch.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut s = serde_json::to_string_pretty(self).expect("anchor sidecar serialises");
        s.push('\n');
        s.into_bytes()
    }

    /// The anchors recorded for `entity_id`, or an empty slice.
    pub fn get(&self, entity_id: &str) -> &[Anchor] {
        self.entities
            .get(entity_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Replace `entity_id`'s anchors. An empty list prunes the key so the
    /// sidecar never accumulates empty rows.
    pub fn set(&mut self, entity_id: &str, anchors: Vec<Anchor>) {
        if anchors.is_empty() {
            self.entities.remove(entity_id);
        } else {
            self.entities.insert(entity_id.to_string(), anchors);
        }
    }

    /// Drop `entity_id`'s anchors entirely (delete leg). Idempotent.
    pub fn remove(&mut self, entity_id: &str) {
        self.entities.remove(entity_id);
    }

    /// Move `from`'s anchors to `to` (rename leg), leaving zero rows under
    /// the old id. No-op when `from` has no anchors. When `to` already has
    /// anchors they are overwritten — a rename onto a live id is refused
    /// upstream, so this is the residual-stub case only.
    pub fn rename(&mut self, from: &str, to: &str) {
        if let Some(anchors) = self.entities.remove(from) {
            self.entities.insert(to.to_string(), anchors);
        }
    }

    /// Whether the document holds no anchors for any entity.
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- wire vocabulary is the contract -----------------------------------

    #[test]
    fn class_wire_strings_are_stable() {
        assert_eq!(AnchorProvenanceClass::Anchored.as_wire(), "anchored");
        assert_eq!(AnchorProvenanceClass::Derived.as_wire(), "derived");
        assert_eq!(AnchorProvenanceClass::Authored.as_wire(), "authored");
        assert_eq!(AnchorProvenanceClass::InformedBy.as_wire(), "informed-by");
        for w in AnchorProvenanceClass::WIRE_VALUES {
            assert_eq!(AnchorProvenanceClass::from_wire(w).unwrap().as_wire(), *w);
        }
        assert!(AnchorProvenanceClass::from_wire("bogus").is_none());
    }

    #[test]
    fn grain_wire_strings_are_stable() {
        for w in AnchorGrain::WIRE_VALUES {
            assert_eq!(AnchorGrain::from_wire(w).unwrap().as_wire(), *w);
        }
        assert_eq!(
            AnchorGrain::WIRE_VALUES,
            &["span", "file", "tree", "url", "entity"]
        );
        assert!(AnchorGrain::from_wire("chunk").is_none());
    }

    #[test]
    fn stability_and_state_wire_strings_are_stable() {
        assert_eq!(AnchorHashStability::Stable.as_wire(), "stable");
        assert_eq!(AnchorHashStability::Unstable.as_wire(), "unstable");
        assert_eq!(AnchorState::Resolves.as_wire(), "resolves");
        assert_eq!(AnchorState::Drifted.as_wire(), "drifted");
        assert_eq!(AnchorState::Recheck.as_wire(), "recheck");
        assert_eq!(AnchorState::Orphaned.as_wire(), "orphaned");
    }

    #[test]
    fn only_anchored_and_derived_are_hash_bearing() {
        assert!(AnchorProvenanceClass::Anchored.is_hash_bearing());
        assert!(AnchorProvenanceClass::Derived.is_hash_bearing());
        assert!(!AnchorProvenanceClass::Authored.is_hash_bearing());
        assert!(!AnchorProvenanceClass::InformedBy.is_hash_bearing());
    }

    // -- grain / namespace matrix ------------------------------------------

    #[test]
    fn grain_namespace_support_matches_capability_matrix() {
        // path-shaped grains need path / path+commit.
        for g in [AnchorGrain::Span, AnchorGrain::File, AnchorGrain::Tree] {
            assert!(g.supported_by_namespace("path"));
            assert!(g.supported_by_namespace("path+commit"));
            assert!(!g.supported_by_namespace("url"));
            assert!(!g.supported_by_namespace("entity"));
        }
        assert!(AnchorGrain::Url.supported_by_namespace("url"));
        assert!(!AnchorGrain::Url.supported_by_namespace("path"));
        assert!(AnchorGrain::Entity.supported_by_namespace("entity"));
        assert!(!AnchorGrain::Entity.supported_by_namespace("path"));
    }

    // -- validation refusals -----------------------------------------------

    fn valid_input() -> AnchorInput {
        AnchorInput {
            artifact: Some("src/lib.rs".into()),
            grain: Some("file".into()),
            class: Some("anchored".into()),
            hash_stability: Some("stable".into()),
            hash: Some("abc123".into()),
            ..Default::default()
        }
    }

    #[test]
    fn validate_accepts_a_well_formed_anchor() {
        let a = valid_input().validate(Some(("codebase", "path"))).unwrap();
        assert_eq!(a.artifact, "src/lib.rs");
        assert_eq!(a.grain, AnchorGrain::File);
        assert_eq!(a.class, AnchorProvenanceClass::Anchored);
        assert_eq!(a.hash.as_deref(), Some("abc123"));
        assert_eq!(a.hash_stability, AnchorHashStability::Stable);
    }

    #[test]
    fn validate_defaults_hash_stability_to_stable() {
        let mut i = valid_input();
        i.hash_stability = None;
        let a = i.validate(None).unwrap();
        assert_eq!(a.hash_stability, AnchorHashStability::Stable);
    }

    #[test]
    fn validate_refuses_unknown_class() {
        let mut i = valid_input();
        i.class = Some("guessed".into());
        let err = i.validate(None).unwrap_err();
        assert_eq!(err.code(), INVALID_ANCHOR_CODE);
        assert!(matches!(err, AnchorValidationError::UnknownClass { .. }));
        assert_eq!(err.detail()["field"], serde_json::json!("class"));
    }

    #[test]
    fn validate_refuses_unknown_grain() {
        let mut i = valid_input();
        i.grain = Some("paragraph".into());
        let err = i.validate(None).unwrap_err();
        assert!(matches!(err, AnchorValidationError::UnknownGrain { .. }));
    }

    #[test]
    fn validate_refuses_missing_artifact() {
        let mut i = valid_input();
        i.artifact = Some("   ".into());
        let err = i.validate(None).unwrap_err();
        assert!(matches!(err, AnchorValidationError::MissingArtifact));
        i.artifact = None;
        assert!(matches!(
            valid_input_with_artifact(None).validate(None).unwrap_err(),
            AnchorValidationError::MissingArtifact
        ));
        let _ = i;
    }

    fn valid_input_with_artifact(a: Option<String>) -> AnchorInput {
        AnchorInput {
            artifact: a,
            ..valid_input()
        }
    }

    #[test]
    fn validate_refuses_hash_on_non_hash_class() {
        let mut i = valid_input();
        i.class = Some("authored".into());
        // hash still supplied → refuse
        let err = i.validate(None).unwrap_err();
        assert!(matches!(
            err,
            AnchorValidationError::HashOnNonHashClass { class: "authored" }
        ));
    }

    #[test]
    fn validate_accepts_non_hash_class_without_hash() {
        let mut i = valid_input();
        i.class = Some("informed-by".into());
        i.hash = None;
        let a = i.validate(None).unwrap();
        assert_eq!(a.class, AnchorProvenanceClass::InformedBy);
        assert!(a.hash.is_none());
    }

    #[test]
    fn validate_refuses_grain_unsupported_by_medium_namespace() {
        // span grain on a web (url namespace) medium.
        let mut i = valid_input();
        i.grain = Some("span".into());
        i.class = Some("authored".into());
        i.hash = None;
        let err = i.validate(Some(("web", "url"))).unwrap_err();
        match err {
            AnchorValidationError::GrainNamespaceUnsupported {
                grain,
                anchor_namespace,
                ..
            } => {
                assert_eq!(grain, "span");
                assert_eq!(anchor_namespace, "url");
            }
            other => panic!("expected GrainNamespaceUnsupported, got {other:?}"),
        }
    }

    #[test]
    fn validate_skips_namespace_check_without_medium_context() {
        // span grain, no medium → namespace rule not applied.
        let mut i = valid_input();
        i.grain = Some("span".into());
        assert!(i.validate(None).is_ok());
    }

    // -- prepared-content hash ----------------------------------------------

    /// The prepared form is stable across meaningless byte noise: BOM,
    /// line-ending convention, and final-newline presence never move the
    /// hash — a real content change always does.
    #[test]
    fn prepared_hash_is_stable_across_byte_noise() {
        let base = prepared_content_hash(b"fn a() {}\nfn b() {}\n");
        // CRLF and lone-CR line endings normalize away.
        assert_eq!(prepared_content_hash(b"fn a() {}\r\nfn b() {}\r\n"), base);
        assert_eq!(prepared_content_hash(b"fn a() {}\rfn b() {}\r"), base);
        // Final-newline presence (missing, single, several) is noise.
        assert_eq!(prepared_content_hash(b"fn a() {}\nfn b() {}"), base);
        assert_eq!(prepared_content_hash(b"fn a() {}\nfn b() {}\n\n\n"), base);
        // A leading UTF-8 BOM is stripped.
        assert_eq!(
            prepared_content_hash("\u{feff}fn a() {}\nfn b() {}\n".as_bytes()),
            base
        );
        // A real content change moves the hash.
        assert_ne!(prepared_content_hash(b"fn a() {}\nfn c() {}\n"), base);
        // House hash shape: 16 lowercase hex chars.
        assert_eq!(base.len(), 16);
        assert!(
            base.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    /// Interior whitespace is content, not noise: a trailing space inside a
    /// line (markdown hard break) changes the hash.
    #[test]
    fn prepared_hash_preserves_interior_whitespace() {
        assert_ne!(
            prepared_content_hash(b"line one  \nline two\n"),
            prepared_content_hash(b"line one\nline two\n")
        );
    }

    /// Non-UTF-8 bytes hash raw — no text canonicalization is applied, and
    /// any byte change moves the hash.
    #[test]
    fn prepared_hash_hashes_binary_bytes_raw() {
        let bin_a = [0xff_u8, 0xfe, 0x00, 0x0d, 0x0a];
        let bin_b = [0xff_u8, 0xfe, 0x00, 0x0a];
        assert_ne!(prepared_content_hash(&bin_a), prepared_content_hash(&bin_b));
        // Deterministic.
        assert_eq!(prepared_content_hash(&bin_a), prepared_content_hash(&bin_a));
    }

    // -- resolution --------------------------------------------------------

    fn anchor(
        class: AnchorProvenanceClass,
        hash: Option<&str>,
        stab: AnchorHashStability,
    ) -> Anchor {
        Anchor {
            artifact: "src/lib.rs".into(),
            grain: AnchorGrain::File,
            class,
            at_version: None,
            hash: hash.map(str::to_string),
            hash_stability: stab,
            derived_from: Vec::new(),
            binding: None,
        }
    }

    #[test]
    fn resolves_when_hash_matches() {
        let a = anchor(
            AnchorProvenanceClass::Anchored,
            Some("h1"),
            AnchorHashStability::Stable,
        );
        let obs = ArtifactObservation::Present {
            current_hash: Some("h1".into()),
        };
        assert_eq!(resolve_anchor(&a, &obs), AnchorState::Resolves);
    }

    #[test]
    fn stable_hash_break_drifts_unstable_rechecks() {
        let stable = anchor(
            AnchorProvenanceClass::Anchored,
            Some("h1"),
            AnchorHashStability::Stable,
        );
        let unstable = anchor(
            AnchorProvenanceClass::Anchored,
            Some("h1"),
            AnchorHashStability::Unstable,
        );
        let obs = ArtifactObservation::Present {
            current_hash: Some("h2".into()),
        };
        assert_eq!(resolve_anchor(&stable, &obs), AnchorState::Drifted);
        assert_eq!(resolve_anchor(&unstable, &obs), AnchorState::Recheck);
    }

    #[test]
    fn absent_artifact_is_orphaned() {
        let a = anchor(
            AnchorProvenanceClass::Anchored,
            Some("h1"),
            AnchorHashStability::Stable,
        );
        assert_eq!(
            resolve_anchor(&a, &ArtifactObservation::Absent),
            AnchorState::Orphaned
        );
    }

    #[test]
    fn non_hash_classes_never_drift() {
        for class in [
            AnchorProvenanceClass::Authored,
            AnchorProvenanceClass::InformedBy,
        ] {
            let a = anchor(class, None, AnchorHashStability::Stable);
            // Content moved underneath — still resolves (excluded from
            // hash-drift adjudication).
            let obs = ArtifactObservation::Present {
                current_hash: Some("whatever".into()),
            };
            assert_eq!(resolve_anchor(&a, &obs), AnchorState::Resolves);
            // But an absent artifact is still orphaned.
            assert_eq!(
                resolve_anchor(&a, &ArtifactObservation::Absent),
                AnchorState::Orphaned
            );
        }
    }

    #[test]
    fn unavailable_hash_rechecks_not_drifts() {
        let a = anchor(
            AnchorProvenanceClass::Anchored,
            Some("h1"),
            AnchorHashStability::Stable,
        );
        let obs = ArtifactObservation::Present { current_hash: None };
        assert_eq!(resolve_anchor(&a, &obs), AnchorState::Recheck);
    }

    // -- composition -------------------------------------------------------

    #[test]
    fn composition_counts_classes_grains_and_tree_fanout() {
        let anchors = vec![
            Anchor {
                artifact: "a.rs".into(),
                grain: AnchorGrain::File,
                class: AnchorProvenanceClass::Anchored,
                at_version: None,
                hash: Some("h".into()),
                hash_stability: AnchorHashStability::Stable,
                derived_from: Vec::new(),
                binding: None,
            },
            Anchor {
                artifact: "src/".into(),
                grain: AnchorGrain::Tree,
                class: AnchorProvenanceClass::Derived,
                at_version: None,
                hash: Some("t".into()),
                hash_stability: AnchorHashStability::Stable,
                derived_from: vec!["a.rs".into(), "b.rs".into()],
                binding: None,
            },
        ];
        let comp = compose_entity_anchors(&anchors);
        assert_eq!(comp.by_class["anchored"], 1);
        assert_eq!(comp.by_class["derived"], 1);
        assert_eq!(comp.by_grain["file"], 1);
        assert_eq!(comp.by_grain["tree"], 1);
        // Tree fan-out is a distinct axis — one row, never per-file credit.
        assert_eq!(comp.tree_grain_artifacts, vec!["src/".to_string()]);
        assert_eq!(
            comp.derived_inputs,
            vec![vec!["a.rs".to_string(), "b.rs".to_string()]]
        );
    }

    // -- sidecar round-trip -------------------------------------------------

    #[test]
    fn sidecar_round_trips_and_prunes_empty() {
        let mut sc = AnchorSidecar::default();
        assert!(sc.is_empty());
        let a = anchor(
            AnchorProvenanceClass::Anchored,
            Some("h1"),
            AnchorHashStability::Stable,
        );
        sc.set("specs--x", vec![a.clone()]);
        assert_eq!(sc.get("specs--x").len(), 1);

        let bytes = sc.to_bytes();
        let round = AnchorSidecar::from_bytes(&bytes).unwrap();
        assert_eq!(round, sc);

        // Setting empty prunes the key.
        sc.set("specs--x", vec![]);
        assert!(sc.is_empty());
        assert!(sc.get("specs--x").is_empty());
    }

    #[test]
    fn sidecar_rename_leaves_zero_rows_under_old_id() {
        let mut sc = AnchorSidecar::default();
        sc.set(
            "specs--old",
            vec![anchor(
                AnchorProvenanceClass::Anchored,
                Some("h"),
                AnchorHashStability::Stable,
            )],
        );
        sc.rename("specs--old", "specs--new");
        assert!(sc.get("specs--old").is_empty());
        assert_eq!(sc.get("specs--new").len(), 1);
    }

    #[test]
    fn sidecar_remove_drops_entity_anchors() {
        let mut sc = AnchorSidecar::default();
        sc.set(
            "specs--gone",
            vec![anchor(
                AnchorProvenanceClass::Anchored,
                Some("h"),
                AnchorHashStability::Stable,
            )],
        );
        sc.remove("specs--gone");
        assert!(sc.get("specs--gone").is_empty());
        // Idempotent.
        sc.remove("specs--gone");
    }

    #[test]
    fn empty_bytes_parse_as_empty_sidecar() {
        assert!(AnchorSidecar::from_bytes(b"").unwrap().is_empty());
        assert!(AnchorSidecar::from_bytes(b"  \n ").unwrap().is_empty());
    }

    #[test]
    fn anchor_json_shape_omits_empty_optionals() {
        let a = anchor(
            AnchorProvenanceClass::Anchored,
            Some("h1"),
            AnchorHashStability::Stable,
        );
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["artifact"], "src/lib.rs");
        assert_eq!(v["grain"], "file");
        assert_eq!(v["class"], "anchored");
        assert_eq!(v["hash"], "h1");
        assert_eq!(v["hash_stability"], "stable");
        // Absent optionals are skipped, not null.
        assert!(v.get("at_version").is_none());
        assert!(v.get("derived_from").is_none());
        assert!(v.get("binding").is_none());
    }

    #[test]
    fn anchor_version_serialises_tagged() {
        let a = Anchor {
            at_version: Some(AnchorVersion::Commit("deadbeef".into())),
            ..anchor(
                AnchorProvenanceClass::Anchored,
                Some("h"),
                AnchorHashStability::Stable,
            )
        };
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["at_version"]["kind"], "commit");
        assert_eq!(v["at_version"]["value"], "deadbeef");
    }
}
