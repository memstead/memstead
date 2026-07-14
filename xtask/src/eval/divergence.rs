//! The divergence mode's view of the pre-registration package.
//!
//! The divergence campaign (plan 02 of the divergence-eval bundle) consumes the
//! committed package at `docs/proof/divergence/prereg/` as its *only*
//! configuration. Two things make the pre-registration real rather than
//! decorative and both live here:
//!
//! 1. **Content-hash pinning.** The package is hashed at campaign start; a run
//!    or a resume against an edited package must refuse rather than silently mix
//!    two designs. [`Package::content_hash`] is that hash; [`Package::verify_pin`]
//!    is the refusal.
//! 2. **The pinned model.** Writers and readers share one model across both arms
//!    (parity); [`Package::single_model`] returns it only when the package's
//!    writer and reader pins agree, so a confounded pair is refused up front.
//!
//! Only the parts the harness reads today are modelled: the campaign parameters
//! ([`Campaign`]), the model pins ([`Models`]), and both arms' tell lists
//! ([`TellLists`], fed to [`super::grade::strip_tells_with`] on the reader path).
//! The slice manifest, the query battery, and the prompts load as the round loop
//! and reader battery grow to need them.

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::path::Path;

/// Machine-readable campaign parameters, from `campaign.json`.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct Campaign {
    pub rounds: usize,
    pub hurry_rounds: Vec<usize>,
    pub reader_checkpoints: Vec<usize>,
    pub integrity_audit_rounds: Vec<usize>,
    pub trials: usize,
    pub writer_allowance_full_tokens: usize,
    pub writer_allowance_hurry_tokens: usize,
    pub reader_budget_tokens: usize,
    pub contamination_threshold: f64,
    pub cost_cap_tokens: u64,
}

/// The frozen model pins, from `models.json`.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct Models {
    pub writer: String,
    pub reader: String,
    pub judge: String,
    pub auditor: String,
}

/// Both arms' tell lists, from `tell-lists.json`, flattened for blinding.
///
/// The reader-path blinder strips **both** lists from every answer regardless of
/// which arm produced it — the judge must not infer the arm from either Arm B's
/// mem/tool vocabulary or Arm A's substrate vocabulary. This is the per-arm
/// extension of the hardcoded [`super::grade::strip_tells`].
#[derive(Clone, Debug, Default)]
pub struct TellLists {
    pub arm_a: Vec<String>,
    pub arm_b: Vec<String>,
}

impl TellLists {
    /// Every tell from both arms, for the reader-path blinder that strips both
    /// directions. Pass to [`super::grade::strip_tells_with`].
    pub fn combined(&self) -> Vec<String> {
        let mut all = self.arm_a.clone();
        all.extend(self.arm_b.iter().cloned());
        all
    }

    fn from_json(bytes: &[u8]) -> Result<Self> {
        #[derive(serde::Deserialize)]
        struct File {
            arm_a_tells: Arm,
            arm_b_tells: Arm,
        }
        #[derive(serde::Deserialize)]
        struct Arm {
            #[serde(default)]
            tokens: Vec<String>,
            #[serde(default)]
            phrases: Vec<String>,
        }
        let file: File = serde_json::from_slice(bytes).context("parsing tell-lists.json")?;
        let flatten = |a: Arm| {
            let mut v = a.tokens;
            v.extend(a.phrases);
            v
        };
        Ok(Self {
            arm_a: flatten(file.arm_a_tells),
            arm_b: flatten(file.arm_b_tells),
        })
    }
}

/// The loaded package: its parsed configuration plus the content hash that pins
/// it for the campaign's lifetime.
#[derive(Clone, Debug)]
pub struct Package {
    pub campaign: Campaign,
    pub models: Models,
    pub tell_lists: TellLists,
    /// Hex SHA-256 over every file in the package directory (see
    /// [`hash_package_dir`]). Recorded at campaign start and re-checked on resume.
    pub content_hash: String,
}

impl Package {
    /// Load and parse the package at `dir`, computing its content hash.
    pub fn load(dir: &Path) -> Result<Self> {
        let campaign: Campaign = read_json(dir, "campaign.json")?;
        let models: Models = read_json(dir, "models.json")?;
        let tell_lists = TellLists::from_json(&read_file(dir, "tell-lists.json")?)?;
        let content_hash = hash_package_dir(dir)?;
        Ok(Self {
            campaign,
            models,
            tell_lists,
            content_hash,
        })
    }

    /// The single model both arms' writers and readers run on — the parity the
    /// experiment rests on. Refuses if the writer and reader pins disagree, so a
    /// confounded pair never starts.
    pub fn single_model(&self) -> Result<&str> {
        if self.models.writer != self.models.reader {
            bail!(
                "package model pins are not single-valued: writer={} reader={} — writers and readers must share one model",
                self.models.writer,
                self.models.reader
            );
        }
        Ok(&self.models.writer)
    }

    /// Refuse if the package's current content hash does not match the hash
    /// pinned at campaign start — a run or resume against an edited package must
    /// not silently mix two designs.
    pub fn verify_pin(&self, pinned_hash: &str) -> Result<()> {
        if self.content_hash != pinned_hash {
            bail!(
                "package content hash changed since the campaign was pinned (pinned {}, now {}) — refusing to run against an edited pre-registration package",
                pinned_hash,
                self.content_hash
            );
        }
        Ok(())
    }
}

/// SHA-256 (hex) over every file directly in `dir`, folded in sorted filename
/// order so the hash is deterministic and independent of directory iteration
/// order. Each file contributes its name, a NUL separator, its byte length, and
/// its bytes, so neither a rename nor a content edit can collide. Subdirectories
/// are ignored (the package is flat).
pub fn hash_package_dir(dir: &Path) -> Result<String> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            let name = entry.file_name().to_string_lossy().into_owned();
            files.push((name, std::fs::read(entry.path())?));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = Sha256::new();
    for (name, bytes) in &files {
        hasher.update(name.as_bytes());
        hasher.update([0u8]);
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    Ok(hex(&hasher.finalize()))
}

fn read_file(dir: &Path, name: &str) -> Result<Vec<u8>> {
    let path = dir.join(name);
    std::fs::read(&path).with_context(|| format!("reading {}", path.display()))
}

fn read_json<T: serde::de::DeserializeOwned>(dir: &Path, name: &str) -> Result<T> {
    let bytes = read_file(dir, name)?;
    serde_json::from_slice(&bytes).with_context(|| format!("parsing {name}"))
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn write_fixture_package(dir: &Path) {
        fs::write(
            dir.join("campaign.json"),
            r#"{ "rounds": 10, "hurry_rounds": [3,6,9], "reader_checkpoints": [1,3,5,10],
                 "integrity_audit_rounds": [5,10], "trials": 3,
                 "writer_allowance_full_tokens": 8000, "writer_allowance_hurry_tokens": 4000,
                 "reader_budget_tokens": 8000, "contamination_threshold": 0.5,
                 "cost_cap_tokens": 20000000 }"#,
        )
        .unwrap();
        fs::write(
            dir.join("models.json"),
            r#"{ "writer": "claude-opus-4-8", "reader": "claude-opus-4-8",
                 "judge": "claude-opus-4-8", "auditor": "claude-opus-4-8" }"#,
        )
        .unwrap();
        fs::write(
            dir.join("tell-lists.json"),
            r#"{ "arm_a_tells": { "tokens": ["wikilink"], "phrases": ["the notes directory"] },
                 "arm_b_tells": { "tokens": ["memstead"], "phrases": ["the mounted mem"] } }"#,
        )
        .unwrap();
    }

    fn tmp() -> PathBuf {
        tempfile::tempdir().unwrap().keep()
    }

    #[test]
    fn loads_campaign_models_and_both_tell_lists() {
        let dir = tmp();
        write_fixture_package(&dir);
        let pkg = Package::load(&dir).unwrap();
        assert_eq!(pkg.campaign.rounds, 10);
        assert_eq!(pkg.campaign.hurry_rounds, vec![3, 6, 9]);
        assert_eq!(pkg.campaign.trials, 3);
        assert_eq!(pkg.campaign.cost_cap_tokens, 20_000_000);
        assert_eq!(pkg.single_model().unwrap(), "claude-opus-4-8");
        let all = pkg.tell_lists.combined();
        assert!(all.iter().any(|t| t == "the mounted mem"));
        assert!(all.iter().any(|t| t == "wikilink"));
    }

    #[test]
    fn content_hash_is_stable_and_edit_sensitive() {
        let dir = tmp();
        write_fixture_package(&dir);
        let h1 = hash_package_dir(&dir).unwrap();
        // Re-hashing the untouched directory yields the same hash.
        assert_eq!(h1, hash_package_dir(&dir).unwrap());
        assert_eq!(h1.len(), 64, "sha-256 hex is 64 chars");
        // Editing any file changes the hash.
        fs::write(dir.join("campaign.json"), r#"{ "rounds": 11 }"#).unwrap();
        assert_ne!(h1, hash_package_dir(&dir).unwrap());
    }

    #[test]
    fn verify_pin_refuses_an_edited_package() {
        let dir = tmp();
        write_fixture_package(&dir);
        let pkg = Package::load(&dir).unwrap();
        assert!(pkg.verify_pin(&pkg.content_hash).is_ok());
        assert!(
            pkg.verify_pin("0000000000000000000000000000000000000000000000000000000000000000")
                .is_err()
        );
    }

    #[test]
    fn single_model_refuses_a_confounded_pair() {
        let dir = tmp();
        write_fixture_package(&dir);
        fs::write(
            dir.join("models.json"),
            r#"{ "writer": "model-a", "reader": "model-b", "judge": "j", "auditor": "x" }"#,
        )
        .unwrap();
        let pkg = Package::load(&dir).unwrap();
        assert!(pkg.single_model().is_err());
    }
}
