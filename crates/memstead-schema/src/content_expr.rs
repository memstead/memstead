//! Content expressions — the compiled half of the section-format
//! vocabulary (agent-toolbox plan 08).
//!
//! A section can declare its markdown shape as a flat expression over
//! the mdast block-node vocabulary, verbatim: `paragraph`, `list`,
//! `table`, `code`, `blockquote`, `heading`, `thematicBreak`, `html`.
//! Operators: sequence (space), alternation of names
//! (`(paragraph | list)`), repetition `+` `*` `?` on names and on
//! parenthesized groups. The grammar is deliberately **regular** — no
//! nesting, no recursion — the ProseMirror precedent: a deterministic
//! content model is what lets a refusal say "expected X at position N"
//! instead of "the structure didn't match".
//!
//! This module owns parsing, validation, and matching. It knows
//! nothing about markdown itself — the consumer (the engine's
//! section-format evaluator) reduces a section body to a sequence of
//! [`ObservedBlock`]s with a real CommonMark parser and hands it to
//! [`ContentExpr::match_blocks`].

use serde::Serialize;

/// The mdast block-node names the vocabulary admits, verbatim
/// (including `thematicBreak`'s camelCase — mdast names are used
/// unchanged because they are the vocabulary agents already know from
/// the remark/MDX ecosystem).
pub const BLOCK_NAMES: &[&str] = &[
    "paragraph",
    "list",
    "table",
    "code",
    "blockquote",
    "heading",
    "thematicBreak",
    "html",
];

/// One observed top-level block of a section body, as reduced by the
/// consumer's markdown parser. Carries exactly the attributes the
/// expression vocabulary can constrain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservedBlock {
    Paragraph,
    /// `ordered: false` is a bullet list.
    List {
        ordered: bool,
    },
    Table,
    /// `lang` is the fenced-code info string's first word, empty for
    /// none (and for indented code blocks).
    Code {
        lang: String,
    },
    Blockquote,
    /// ATX or setext heading depth (1–6).
    Heading {
        depth: u8,
    },
    ThematicBreak,
    Html,
}

impl ObservedBlock {
    /// The mdast name of this block — the `found` vocabulary in
    /// mismatch payloads.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Paragraph => "paragraph",
            Self::List { .. } => "list",
            Self::Table => "table",
            Self::Code { .. } => "code",
            Self::Blockquote => "blockquote",
            Self::Heading { .. } => "heading",
            Self::ThematicBreak => "thematicBreak",
            Self::Html => "html",
        }
    }

    /// Rendered with its attribute where one exists — the display
    /// form used in `found` sequences (`list(bullet)`, `heading(3)`).
    pub fn display(&self) -> String {
        match self {
            Self::List { ordered: false } => "list(bullet)".to_string(),
            Self::List { ordered: true } => "list(ordered)".to_string(),
            Self::Heading { depth } => format!("heading({depth})"),
            Self::Code { lang } if !lang.is_empty() => format!("code(lang={lang})"),
            other => other.name().to_string(),
        }
    }
}

/// One terminal of a compiled expression: a block name plus its
/// optional attribute constraint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Terminal {
    pub name: String,
    /// `bullet` / `ordered` for `list`, `3`..`6` for `heading`,
    /// `lang=<tag>` for `code`. `None` admits any attribute.
    pub attr: Option<String>,
}

impl Terminal {
    /// Display form (`list(bullet)`, `heading(3)`, `paragraph`) —
    /// used for `expected_next` payloads.
    pub fn display(&self) -> String {
        match &self.attr {
            Some(a) => format!("{}({})", self.name, a),
            None => self.name.clone(),
        }
    }

    /// Whether this terminal admits the observed block.
    pub fn admits(&self, block: &ObservedBlock) -> bool {
        if self.name != block.name() {
            return false;
        }
        let Some(attr) = &self.attr else {
            return true;
        };
        match block {
            ObservedBlock::List { ordered } => {
                (attr == "bullet" && !ordered) || (attr == "ordered" && *ordered)
            }
            ObservedBlock::Heading { depth } => attr.parse::<u8>() == Ok(*depth),
            ObservedBlock::Code { lang } => attr
                .strip_prefix("lang=")
                .is_some_and(|expected| expected == lang),
            // The remaining kinds admit no attributes; the parser
            // refuses attributes on them, so this arm is unreachable
            // for a parsed expression.
            _ => false,
        }
    }
}

/// Typed parse/validation failure for a content expression. `offender`
/// carries the offending token so loader errors can name it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContentExprError {
    #[error(
        "unknown block name '{0}' — the vocabulary is paragraph, list, table, code, blockquote, heading, thematicBreak, html"
    )]
    UnknownBlockName(String),
    #[error("invalid attribute '{attr}' on '{name}'")]
    InvalidAttribute { name: String, attr: String },
    #[error(
        "heading depth {0} is outside 3–6 — h1/h2 are the entity's own levels (title, section delimiters)"
    )]
    HeadingDepthReserved(u8),
    #[error(
        "nested groups are not allowed — the expression grammar is regular (no nesting, no recursion)"
    )]
    NestedGroup,
    #[error(
        "a group must be either an alternation of names or a sequence — mixing '|' and sequence inside one group is not allowed"
    )]
    MixedGroupOperators,
    #[error("unbalanced parentheses")]
    UnbalancedParens,
    #[error("repetition operator '{0}' has nothing to apply to")]
    DanglingRepetition(char),
    #[error("empty expression")]
    Empty,
    #[error("empty group")]
    EmptyGroup,
    #[error("unexpected token '{0}'")]
    UnexpectedToken(String),
}

/// How often a term repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Repeat {
    One,
    OneOrMore,
    ZeroOrMore,
    ZeroOrOne,
}

/// One term of the (flat) expression: a single terminal, an
/// alternation of terminals, or a sequence group — each with a
/// repetition.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Term {
    Atom(Terminal, Repeat),
    /// `(a | b | c)` — names only.
    Alternation(Vec<Terminal>, Repeat),
    /// `(a b c)` — a repeated sequence group.
    Sequence(Vec<Terminal>, Repeat),
}

/// A parsed, validated content expression. Matching runs an NFA
/// simulation over the observed block sequence so a failure can
/// report the exact position and the terminals that would have been
/// legal there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentExpr {
    source: String,
    terms: Vec<Term>,
    nfa: Nfa,
}

/// Match failure: the observed sequence does not satisfy the
/// expression. `failed_at` is the index into the observed sequence
/// (== its length when the body ended too early); `expected_next`
/// lists the display forms of the terminals legal at that position;
/// `found` is the display form of the offending block (`None` at
/// end-of-body).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MatchFailure {
    pub failed_at: usize,
    pub expected_next: Vec<String>,
    pub found: Option<String>,
}

impl ContentExpr {
    /// Parse and validate an expression string. The compiled
    /// expression is cached on the schema at load time — parse once,
    /// match per write.
    pub fn parse(source: &str) -> Result<Self, ContentExprError> {
        let terms = parse_terms(source)?;
        if terms.is_empty() {
            return Err(ContentExprError::Empty);
        }
        let nfa = Nfa::compile(&terms);
        Ok(Self {
            source: source.to_string(),
            terms,
            nfa,
        })
    }

    /// The verbatim declaration text.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The distinct block names the expression mentions — the loader
    /// uses this for `item_pattern` legality ("exactly one of `list`
    /// / `paragraph`", counted by name, repeated occurrences of the
    /// same kind are fine).
    pub fn mentioned_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .all_terminals()
            .map(|t| t.name.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        names.sort_unstable();
        names
    }

    fn all_terminals(&self) -> impl Iterator<Item = &Terminal> {
        self.terms.iter().flat_map(|t| match t {
            Term::Atom(a, _) => std::slice::from_ref(a).iter(),
            Term::Alternation(v, _) | Term::Sequence(v, _) => v.iter(),
        })
    }

    /// Match the observed block sequence against the expression.
    pub fn match_blocks(&self, blocks: &[ObservedBlock]) -> Result<(), MatchFailure> {
        self.nfa.run(blocks)
    }
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Name(Terminal),
    Open,
    Close,
    Pipe,
    Rep(char),
}

fn tokenize(source: &str) -> Result<Vec<Token>, ContentExprError> {
    let mut out = Vec::new();
    let chars: Vec<char> = source.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\n' => i += 1,
            '(' => {
                // Disambiguate group-open from attribute-open: an
                // attribute paren directly follows a name and is
                // consumed by the name lexer below, so a bare '(' here
                // is always a group.
                out.push(Token::Open);
                i += 1;
            }
            ')' => {
                out.push(Token::Close);
                i += 1;
            }
            '|' => {
                out.push(Token::Pipe);
                i += 1;
            }
            '+' | '*' | '?' => {
                out.push(Token::Rep(c));
                i += 1;
            }
            c if c.is_ascii_alphabetic() => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric()) {
                    i += 1;
                }
                let name: String = chars[start..i].iter().collect();
                let mut attr = None;
                if i < chars.len() && chars[i] == '(' {
                    // Attribute parens bind tighter than group parens
                    // — only when the content is attribute-shaped
                    // (no spaces / pipes before the close).
                    let close = chars[i + 1..]
                        .iter()
                        .position(|&c| c == ')')
                        .map(|p| i + 1 + p);
                    if let Some(close_idx) = close {
                        let inner: String = chars[i + 1..close_idx].iter().collect();
                        if !inner.contains(' ') && !inner.contains('|') && !inner.is_empty() {
                            attr = Some(inner);
                            i = close_idx + 1;
                        }
                    }
                }
                out.push(Token::Name(validate_terminal(name, attr)?));
            }
            other => return Err(ContentExprError::UnexpectedToken(other.to_string())),
        }
    }
    Ok(out)
}

fn validate_terminal(name: String, attr: Option<String>) -> Result<Terminal, ContentExprError> {
    if !BLOCK_NAMES.contains(&name.as_str()) {
        return Err(ContentExprError::UnknownBlockName(name));
    }
    if let Some(a) = &attr {
        let valid = match name.as_str() {
            "list" => a == "bullet" || a == "ordered",
            "heading" => match a.parse::<u8>() {
                Ok(d @ 3..=6) => {
                    let _ = d;
                    true
                }
                Ok(d @ (1 | 2)) => return Err(ContentExprError::HeadingDepthReserved(d)),
                _ => false,
            },
            "code" => a.starts_with("lang=") && a.len() > "lang=".len(),
            _ => false,
        };
        if !valid {
            return Err(ContentExprError::InvalidAttribute {
                name,
                attr: a.clone(),
            });
        }
    }
    Ok(Terminal { name, attr })
}

fn parse_terms(source: &str) -> Result<Vec<Term>, ContentExprError> {
    let tokens = tokenize(source)?;
    let mut terms = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            Token::Name(t) => {
                let rep = take_rep(&tokens, i + 1);
                let consumed = 1 + usize::from(rep != Repeat::One);
                terms.push(Term::Atom(t.clone(), rep));
                i += consumed;
            }
            Token::Open => {
                // Collect group members up to the matching close —
                // flat only, a nested Open refuses.
                let mut members: Vec<Terminal> = Vec::new();
                let mut saw_pipe = false;
                let mut saw_adjacent_names = false;
                let mut j = i + 1;
                let mut prev_was_name = false;
                loop {
                    match tokens.get(j) {
                        None => return Err(ContentExprError::UnbalancedParens),
                        Some(Token::Close) => break,
                        Some(Token::Open) => return Err(ContentExprError::NestedGroup),
                        Some(Token::Pipe) => {
                            saw_pipe = true;
                            prev_was_name = false;
                            j += 1;
                        }
                        Some(Token::Name(t)) => {
                            if prev_was_name {
                                saw_adjacent_names = true;
                            }
                            members.push(t.clone());
                            prev_was_name = true;
                            j += 1;
                        }
                        Some(Token::Rep(c)) => {
                            // Repetition inside a group would nest.
                            return Err(ContentExprError::DanglingRepetition(*c));
                        }
                    }
                }
                if members.is_empty() {
                    return Err(ContentExprError::EmptyGroup);
                }
                if saw_pipe && saw_adjacent_names {
                    return Err(ContentExprError::MixedGroupOperators);
                }
                let rep = take_rep(&tokens, j + 1);
                let consumed = j + 1 - i + usize::from(rep != Repeat::One);
                if saw_pipe {
                    terms.push(Term::Alternation(members, rep));
                } else {
                    terms.push(Term::Sequence(members, rep));
                }
                i += consumed;
            }
            Token::Close => return Err(ContentExprError::UnbalancedParens),
            Token::Pipe => {
                return Err(ContentExprError::UnexpectedToken("|".to_string()));
            }
            Token::Rep(c) => return Err(ContentExprError::DanglingRepetition(*c)),
        }
    }
    Ok(terms)
}

fn take_rep(tokens: &[Token], at: usize) -> Repeat {
    match tokens.get(at) {
        Some(Token::Rep('+')) => Repeat::OneOrMore,
        Some(Token::Rep('*')) => Repeat::ZeroOrMore,
        Some(Token::Rep('?')) => Repeat::ZeroOrOne,
        _ => Repeat::One,
    }
}

// ---------------------------------------------------------------------------
// NFA
// ---------------------------------------------------------------------------

/// Thompson-style NFA over [`Terminal`] transitions. Small by
/// construction (the grammar is regular and flat), simulated with a
/// state-set walk so failures report position + legal-next terminals.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Nfa {
    /// `transitions[state]` = list of `(terminal, next_state)`.
    transitions: Vec<Vec<(Terminal, usize)>>,
    /// `epsilons[state]` = ε-reachable next states.
    epsilons: Vec<Vec<usize>>,
    accept: usize,
}

impl Nfa {
    fn compile(terms: &[Term]) -> Self {
        let mut nfa = Nfa {
            transitions: vec![Vec::new()],
            epsilons: vec![Vec::new()],
            accept: 0,
        };
        let mut current = 0;
        for term in terms {
            current = nfa.append_term(current, term);
        }
        nfa.accept = current;
        nfa
    }

    fn new_state(&mut self) -> usize {
        self.transitions.push(Vec::new());
        self.epsilons.push(Vec::new());
        self.transitions.len() - 1
    }

    /// Append one term after `from`; returns the term's exit state.
    fn append_term(&mut self, from: usize, term: &Term) -> usize {
        type UnitBuilder = Box<dyn Fn(&mut Nfa, usize) -> usize>;
        let (unit_entry_build, rep): (UnitBuilder, Repeat) = match term {
            Term::Atom(t, rep) => {
                let t = t.clone();
                (
                    Box::new(move |nfa: &mut Nfa, from: usize| {
                        let next = nfa.new_state();
                        nfa.transitions[from].push((t.clone(), next));
                        next
                    }),
                    *rep,
                )
            }
            Term::Alternation(alts, rep) => {
                let alts = alts.clone();
                (
                    Box::new(move |nfa: &mut Nfa, from: usize| {
                        let next = nfa.new_state();
                        for t in &alts {
                            nfa.transitions[from].push((t.clone(), next));
                        }
                        next
                    }),
                    *rep,
                )
            }
            Term::Sequence(seq, rep) => {
                let seq = seq.clone();
                (
                    Box::new(move |nfa: &mut Nfa, from: usize| {
                        let mut cur = from;
                        for t in &seq {
                            let next = nfa.new_state();
                            nfa.transitions[cur].push((t.clone(), next));
                            cur = next;
                        }
                        cur
                    }),
                    *rep,
                )
            }
        };

        match rep {
            Repeat::One => unit_entry_build(self, from),
            Repeat::ZeroOrOne => {
                let exit = unit_entry_build(self, from);
                self.epsilons[from].push(exit);
                exit
            }
            Repeat::OneOrMore => {
                let exit = unit_entry_build(self, from);
                // Loop back: from the exit, the unit can run again.
                let exit2 = unit_entry_build(self, exit);
                self.epsilons[exit2].push(exit);
                self.epsilons[exit].push(exit2);
                // Collapse: use `exit` as the canonical exit; the
                // second copy shares it via the ε-cycle above.
                exit
            }
            Repeat::ZeroOrMore => {
                let exit = unit_entry_build(self, from);
                self.epsilons[exit].push(from);
                self.epsilons[from].push(exit);
                exit
            }
        }
    }

    fn closure(&self, states: &mut std::collections::BTreeSet<usize>) {
        let mut stack: Vec<usize> = states.iter().copied().collect();
        while let Some(s) = stack.pop() {
            for &e in &self.epsilons[s] {
                if states.insert(e) {
                    stack.push(e);
                }
            }
        }
    }

    fn run(&self, blocks: &[ObservedBlock]) -> Result<(), MatchFailure> {
        let mut current: std::collections::BTreeSet<usize> = std::iter::once(0).collect();
        self.closure(&mut current);
        for (i, block) in blocks.iter().enumerate() {
            let mut next: std::collections::BTreeSet<usize> = Default::default();
            for &s in &current {
                for (terminal, to) in &self.transitions[s] {
                    if terminal.admits(block) {
                        next.insert(*to);
                    }
                }
            }
            if next.is_empty() {
                return Err(MatchFailure {
                    failed_at: i,
                    expected_next: self.expected_from(&current),
                    found: Some(block.display()),
                });
            }
            self.closure(&mut next);
            current = next;
        }
        if current.contains(&self.accept) {
            Ok(())
        } else {
            Err(MatchFailure {
                failed_at: blocks.len(),
                expected_next: self.expected_from(&current),
                found: None,
            })
        }
    }

    /// The display forms of every terminal legal from the state set —
    /// deduplicated, deterministic order.
    fn expected_from(&self, states: &std::collections::BTreeSet<usize>) -> Vec<String> {
        let mut out: std::collections::BTreeSet<String> = Default::default();
        for &s in states {
            for (terminal, _) in &self.transitions[s] {
                out.insert(terminal.display());
            }
        }
        out.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bullet() -> ObservedBlock {
        ObservedBlock::List { ordered: false }
    }
    fn h(depth: u8) -> ObservedBlock {
        ObservedBlock::Heading { depth }
    }
    fn para() -> ObservedBlock {
        ObservedBlock::Paragraph
    }

    #[test]
    fn single_name_matches_exactly_one_block() {
        let e = ContentExpr::parse("list(bullet)").unwrap();
        assert!(e.match_blocks(&[bullet()]).is_ok());
        assert!(e.match_blocks(&[]).is_err());
        assert!(e.match_blocks(&[bullet(), bullet()]).is_err());
        assert!(
            e.match_blocks(&[ObservedBlock::List { ordered: true }])
                .is_err()
        );
    }

    #[test]
    fn plan_example_repeated_group() {
        // The plan's own example: "(heading(3) list(bullet))+"
        let e = ContentExpr::parse("(heading(3) list(bullet))+").unwrap();
        assert!(e.match_blocks(&[h(3), bullet()]).is_ok());
        assert!(e.match_blocks(&[h(3), bullet(), h(3), bullet()]).is_ok());
        assert!(e.match_blocks(&[h(3)]).is_err(), "group half-done");
        assert!(e.match_blocks(&[]).is_err(), "+ needs at least one");
        assert!(e.match_blocks(&[bullet(), h(3)]).is_err(), "order matters");
        assert!(e.match_blocks(&[h(4), bullet()]).is_err(), "depth pinned");
    }

    #[test]
    fn alternation_and_optional() {
        let e = ContentExpr::parse("(paragraph | list) table?").unwrap();
        assert!(e.match_blocks(&[para()]).is_ok());
        assert!(e.match_blocks(&[bullet()]).is_ok());
        assert!(e.match_blocks(&[para(), ObservedBlock::Table]).is_ok());
        assert!(e.match_blocks(&[ObservedBlock::Table]).is_err());
    }

    #[test]
    fn star_admits_empty() {
        let e = ContentExpr::parse("paragraph*").unwrap();
        assert!(e.match_blocks(&[]).is_ok());
        assert!(e.match_blocks(&[para(), para(), para()]).is_ok());
        assert!(e.match_blocks(&[bullet()]).is_err());
    }

    #[test]
    fn sequence_of_names() {
        let e = ContentExpr::parse("paragraph list(bullet) paragraph?").unwrap();
        assert!(e.match_blocks(&[para(), bullet()]).is_ok());
        assert!(e.match_blocks(&[para(), bullet(), para()]).is_ok());
        assert!(e.match_blocks(&[bullet(), para()]).is_err());
    }

    #[test]
    fn failure_reports_position_and_expected_next() {
        let e = ContentExpr::parse("heading(3) list(bullet)+").unwrap();
        let err = e.match_blocks(&[h(3), para()]).unwrap_err();
        assert_eq!(err.failed_at, 1);
        assert_eq!(err.expected_next, vec!["list(bullet)".to_string()]);
        assert_eq!(err.found.as_deref(), Some("paragraph"));

        let err = e.match_blocks(&[h(3)]).unwrap_err();
        assert_eq!(err.failed_at, 1);
        assert_eq!(err.found, None, "body ended too early");
        assert_eq!(err.expected_next, vec!["list(bullet)".to_string()]);
    }

    #[test]
    fn code_lang_attribute() {
        let e = ContentExpr::parse("code(lang=rust)").unwrap();
        assert!(
            e.match_blocks(&[ObservedBlock::Code {
                lang: "rust".into()
            }])
            .is_ok()
        );
        assert!(
            e.match_blocks(&[ObservedBlock::Code {
                lang: "python".into()
            }])
            .is_err()
        );
        let bare = ContentExpr::parse("code").unwrap();
        assert!(
            bare.match_blocks(&[ObservedBlock::Code { lang: "".into() }])
                .is_ok()
        );
    }

    #[test]
    fn validation_refusals() {
        assert!(matches!(
            ContentExpr::parse("bulletList"),
            Err(ContentExprError::UnknownBlockName(n)) if n == "bulletList"
        ));
        assert!(matches!(
            ContentExpr::parse("heading(2)"),
            Err(ContentExprError::HeadingDepthReserved(2))
        ));
        assert!(matches!(
            ContentExpr::parse("heading(1) list"),
            Err(ContentExprError::HeadingDepthReserved(1))
        ));
        assert!(matches!(
            ContentExpr::parse("list(numbered)"),
            Err(ContentExprError::InvalidAttribute { .. })
        ));
        assert!(matches!(
            ContentExpr::parse("paragraph(x)"),
            Err(ContentExprError::InvalidAttribute { .. })
        ));
        assert!(matches!(
            ContentExpr::parse("((list))"),
            Err(ContentExprError::NestedGroup)
        ));
        assert!(matches!(
            ContentExpr::parse("(paragraph | list table)"),
            Err(ContentExprError::MixedGroupOperators)
        ));
        assert!(matches!(
            ContentExpr::parse("(list"),
            Err(ContentExprError::UnbalancedParens)
        ));
        assert!(matches!(
            ContentExpr::parse("+list"),
            Err(ContentExprError::DanglingRepetition('+'))
        ));
        assert!(matches!(
            ContentExpr::parse(""),
            Err(ContentExprError::Empty)
        ));
        assert!(matches!(
            ContentExpr::parse("()"),
            Err(ContentExprError::EmptyGroup)
        ));
    }

    #[test]
    fn mentioned_names_deduplicate() {
        let e = ContentExpr::parse("(heading(3) list(bullet))+ list(bullet)?").unwrap();
        assert_eq!(e.mentioned_names(), vec!["heading", "list"]);
    }
}
