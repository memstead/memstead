//! Where a ref given to a read verb may point, and how it may be spelled.
//!
//! Several mems lie as branches in one git directory, and a workspace
//! may mount some of them and not others. A read verb that takes a ref
//! or a cursor (`diff`, `changes_since`, the commit-note reader) names a
//! mem, which selects the git directory, and a general revision parser
//! would then resolve anything that directory holds: a branch of a mem
//! the workspace does not mount, a hash out of its history, a form that
//! walks into it. Its entity text and its titles would be read through
//! the mem the caller named.
//!
//! So a ref is resolved here, by this module's own small grammar,
//! against the histories the caller may read, and by nothing else. No
//! general revision parser sees a caller's ref. Two things must hold.
//!
//! **The anchor is in scope.** A ref starts at one of: the branch of a
//! mem in scope, by its short or its fully qualified name; `HEAD`,
//! where the scope gives it a meaning; or a commit hash, full or
//! abbreviated, that is looked up among the commits of the histories in
//! scope and nowhere else. A ref anchored anywhere else is no ref, also
//! when walking from it would arrive inside a history in scope: a fork
//! shares its early commits with the mem it was forked from, and
//! serving `<fork>~3` to a workspace that does not mount the fork would
//! tell it that the fork exists and how many commits it has of its own.
//!
//! **The walk stays in scope.** From the anchor a ref may step to
//! parents with `~n` and `^n`. A parent of a commit in scope is in
//! scope, so the commit a ref arrives at always is.
//!
//! Everything else is `None`, with no word on why: a name that is no
//! branch in scope, a hash no commit in scope carries, an abbreviation
//! two of them share, a step past a root, and every other revision
//! syntax (tags, `@{…}`, `^{…}`, ranges, `<rev>:<path>`, `:/<text>`).
//! The answers are one answer, so that what lies outside the mounted
//! mems does not exist for the caller, not even by name. And because
//! lookups run over the histories in scope alone, no object outside
//! them is read at all: no commit, no tree, no blob, no header.

use gix::ObjectId;

/// The histories a ref may be anchored in and walk through.
pub(crate) struct RefScope {
    /// Branches in scope by their fully qualified name, with their tips.
    named: Vec<(String, ObjectId)>,
    /// Every root of the histories in scope.
    tips: Vec<ObjectId>,
    /// What a bare `HEAD` anchors at, where it has a meaning.
    head: Option<ObjectId>,
}

/// One step of a ref's walk.
enum Step {
    /// `~n`: the n-th ancestor along first parents.
    Ancestor(usize),
    /// `^n`: the n-th parent, `^0` the commit itself.
    Parent(usize),
}

impl RefScope {
    /// The histories of `branches`, each given as a mount declares it
    /// (short or fully qualified). A branch that does not exist
    /// contributes nothing. A caller that gives `HEAD` a meaning
    /// re-anchors it onto a branch before it resolves here.
    pub(crate) fn of_branches(repo: &gix::Repository, branches: &[String]) -> Self {
        let mut scope = RefScope {
            named: Vec::new(),
            tips: Vec::new(),
            head: None,
        };
        for branch in branches {
            let full = memstead_base::branch_full_ref(branch);
            let Ok(Some(mut reference)) = repo.try_find_reference(full.as_str()) else {
                continue;
            };
            let Ok(id) = reference.peel_to_id() else {
                continue;
            };
            scope.add(Some(full), id.detach());
        }
        scope
    }

    /// One history: the commits `tip` reaches. `name`, when the history
    /// is a branch, is the name a ref may anchor at; `HEAD` anchors at
    /// the tip either way.
    pub(crate) fn of_history(name: Option<&str>, tip: ObjectId) -> Self {
        let mut scope = RefScope {
            named: Vec::new(),
            tips: Vec::new(),
            head: Some(tip),
        };
        scope.add(name.map(memstead_base::branch_full_ref), tip);
        scope
    }

    fn add(&mut self, name: Option<String>, tip: ObjectId) {
        if let Some(name) = name
            && !self.named.iter().any(|(known, _)| *known == name)
        {
            self.named.push((name, tip));
        }
        if !self.tips.contains(&tip) {
            self.tips.push(tip);
        }
    }

    /// The commit `raw` names: anchored in scope, walked by parent
    /// steps. `None` for everything else, alike.
    pub(crate) fn resolve<'r>(
        &self,
        repo: &'r gix::Repository,
        raw: &str,
    ) -> Option<gix::Commit<'r>> {
        let (anchor, steps) = split(raw)?;
        let mut id = self.anchor(repo, anchor)?;
        for step in steps {
            id = match step {
                Step::Parent(0) => id,
                Step::Parent(n) => parent(repo, id, n)?,
                Step::Ancestor(n) => {
                    let mut at = id;
                    for _ in 0..n {
                        at = parent(repo, at, 1)?;
                    }
                    at
                }
            };
        }
        repo.find_object(id).ok()?.try_into_commit().ok()
    }

    fn anchor(&self, repo: &gix::Repository, anchor: &str) -> Option<ObjectId> {
        if anchor == "HEAD" {
            return self.head;
        }
        let full = memstead_base::branch_full_ref(anchor);
        if let Some((_, tip)) = self.named.iter().find(|(name, _)| *name == full) {
            return Some(*tip);
        }
        self.commit_by_hash(repo, anchor)
    }

    /// The one commit of the histories in scope whose hash is `hex` or
    /// begins with it. The lookup walks those histories and nothing
    /// else, so a hash that names an object outside them is as unknown
    /// as one that names nothing.
    fn commit_by_hash(&self, repo: &gix::Repository, hex: &str) -> Option<ObjectId> {
        if !(4..=40).contains(&hex.len()) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        if self.tips.is_empty() {
            return None;
        }
        let hex = hex.to_ascii_lowercase();
        let walk = repo.rev_walk(self.tips.iter().copied()).all().ok()?;
        let mut found: Option<ObjectId> = None;
        for info in walk {
            let id = info.ok()?.id;
            if id.to_string().starts_with(hex.as_str()) {
                if hex.len() == 40 {
                    return Some(id);
                }
                if found.is_some() {
                    return None;
                }
                found = Some(id);
            }
        }
        found
    }
}

/// The n-th parent of a commit, counted from one.
fn parent(repo: &gix::Repository, id: ObjectId, n: usize) -> Option<ObjectId> {
    let commit = repo.find_object(id).ok()?.try_into_commit().ok()?;
    let parent = commit.parent_ids().nth(n.checked_sub(1)?)?;
    Some(parent.detach())
}

/// A ref's anchor and its parent steps. `None` when what follows the
/// anchor is anything but `~n` and `^n` steps.
fn split(raw: &str) -> Option<(&str, Vec<Step>)> {
    let cut = raw.find(['~', '^']).unwrap_or(raw.len());
    let (anchor, mut rest) = raw.split_at(cut);
    if anchor.is_empty() {
        return None;
    }
    let mut steps = Vec::new();
    while let Some(kind) = rest.chars().next() {
        // Judged before anything is cut off: the two step characters
        // are one byte each, and whatever else stands here, of any
        // width, ends the ref.
        if kind != '~' && kind != '^' {
            return None;
        }
        rest = &rest[1..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        let count = match &rest[..digits] {
            "" => 1,
            n => n.parse::<usize>().ok()?,
        };
        rest = &rest[digits..];
        steps.push(if kind == '~' {
            Step::Ancestor(count)
        } else {
            Step::Parent(count)
        });
    }
    Some((anchor, steps))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(raw: &str) -> Option<(String, Vec<String>)> {
        let (anchor, steps) = split(raw)?;
        Some((
            anchor.to_string(),
            steps
                .iter()
                .map(|s| match s {
                    Step::Ancestor(n) => format!("~{n}"),
                    Step::Parent(n) => format!("^{n}"),
                })
                .collect(),
        ))
    }

    #[test]
    fn a_ref_is_an_anchor_and_parent_steps() {
        assert_eq!(shape("alpha"), Some(("alpha".into(), vec![])));
        assert_eq!(
            shape("refs/heads/team/alpha~3"),
            Some(("refs/heads/team/alpha".into(), vec!["~3".into()]))
        );
        assert_eq!(
            shape("HEAD^^2~"),
            Some(("HEAD".into(), vec!["^1".into(), "^2".into(), "~1".into()]))
        );
        assert_eq!(
            shape("abc123^0"),
            Some(("abc123".into(), vec!["^0".into()]))
        );
    }

    #[test]
    fn every_other_revision_syntax_is_no_ref() {
        for raw in [
            "",
            "~1",
            "^",
            "alpha^{tree}",
            "alpha^{/text}",
            "alpha^{}",
            "alpha^@",
            "alpha^!",
            "alpha^-",
            "alpha~x",
            "alpha~1x",
            // A character of more than one byte after a step.
            "alpha~1é",
            "HEAD~é",
            "HEAD^é",
            "alpha~1€",
            "alpha~1~é",
            "HEAD~١",
            "HEAD~１",
            "HEAD~1\u{0}",
            // A count no step can have.
            "HEAD~99999999999999999999999999999",
        ] {
            assert!(shape(raw).is_none(), "{raw}");
        }
        // These split, and their anchor is no branch and no hash.
        for raw in [
            "alpha@{1}",
            "alpha:path.md",
            ":/text",
            "alpha..gamma",
            "@",
            "é",
            "HEADé",
            " alpha",
        ] {
            let (anchor, _) = shape(raw).expect("splits");
            assert_eq!(anchor, raw, "{raw}: the whole spec is the anchor");
        }
    }
}
