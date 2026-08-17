//! What every leaf-splicing tier has to prove, and the survey that keeps it honest.
//!
//! A leaf-splicing tier changes one leaf's text and shares every green node off its
//! leaf-to-root path. Tiers differ in *how* they establish that the file's token
//! sequence is unchanged but for that leaf — the [token tier](super::token) relexes
//! the leaf alone. Everything downstream of that fact is the same question whoever
//! asks it, and lives here:
//!
//! - [`context_admits`] — the position bans, so a read that reaches a leaf through
//!   the tree, through the raw source, or through adjacency cannot fire;
//! - [`text_reads_are_inert`] — the text guards, per leaf kind;
//! - [`shifted_errors`] — the diagnostic splice;
//! - [`tests::TEXT_READS`] — the survey pinning that the enumeration above is still
//!   the whole set of places the grammar branches on a token's text.
//!
//! The survey lives here rather than inside a tier because it is load-bearing for
//! every one of them, and a table inside a single tier's module reads as that tier's
//! private business.

use rowan::TextRange;

use crate::parser::core::SyntaxError;
use crate::parser::grammar::is_def_prefix_command;
use crate::parser::grammar::{BEGIN_CMD, END_CMD, reads_definition_body, split_math_word};
use crate::parser::lexer::reads_following_text;
use crate::semantic::define::is_definition_command;
use crate::syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

use super::Edit;

/// What the leaf's surroundings say about which text reads can reach it.
#[derive(Clone, Copy)]
pub(super) struct Context {
    /// The leaf sits in a math body, where `math_atom` splits a `WORD` into
    /// operator atoms. Exact rather than approximate: every math body — `$…$`,
    /// `\[…\]`, `\left…\right`, and a math environment's — opens a `MATH` node.
    pub(super) in_math: bool,
}

/// Whether the leaf sits somewhere a text change may be spliced at all, and if so
/// what its surroundings imply.
///
/// Three complementary bans, because a construct that reads a token's text can
/// reach it through the tree, through the raw source, or through simple adjacency:
///
/// - **By ancestry**, which is how greedy attachment records an argument that *was*
///   claimed: a `NAME_GROUP`, a `BEGIN`/`END`, any `COMMAND` whose head reads the
///   text after it (`\begin`, `\documentclass`, every definition family), and any
///   `COMMAND` whose head is an expl3 name, whose argspec slots read a `WORD`'s
///   length. Attachment is greedy, so a slot the expl3 scan *declined* is attached
///   to the same command anyway and lands under the same ban.
/// - **By the preceding tokens on the line**, which covers the residue where
///   attachment declined outright — a shape gate demoting a `[`, a `\documentclass`
///   stranded at the end of a group. The lexer's forward scans read raw text and do
///   not care what the tree made of it.
/// - **By immediate adjacency**, for the reads that only ever fire on the token
///   right after another: a parameter digit after a `#`.
pub(super) fn context_admits(leaf: &SyntaxToken) -> Option<Context> {
    let mut in_math = false;
    for node in leaf.parent_ancestors() {
        match node.kind() {
            // An environment name decides routing, verbatim-ness, and pairing; a
            // relex of the same kind proves nothing about any of it.
            SyntaxKind::NAME_GROUP | SyntaxKind::BEGIN | SyntaxKind::END => return None,
            SyntaxKind::MATH => in_math = true,
            SyntaxKind::COMMAND => {
                let head = command_head(&node)?;
                if head_reads_following_text(&head) || is_expl3_name(&head) {
                    return None;
                }
            }
            _ => {}
        }
    }

    let mut prev = leaf.prev_token();
    while let Some(token) = prev {
        if token.kind() == SyntaxKind::NEWLINE {
            break;
        }
        if token.kind() == SyntaxKind::CONTROL_WORD && head_reads_following_text(token.text()) {
            return None;
        }
        prev = token.prev_token();
    }

    // `#1`: the expl3 plan reads whether the token after a `#` is a parameter
    // digit. That read fires nowhere else, so adjacency is the whole guard.
    if leaf
        .prev_token()
        .is_some_and(|t| t.kind() == SyntaxKind::HASH)
    {
        return None;
    }

    Some(Context { in_math })
}

/// Whether a control word is an expl3 name — the heads whose argspec suffix directs
/// attachment, and so the only place a `WORD`'s *length* is read.
///
/// A colon-carrying `CONTROL_WORD` only lexes inside an expl3 region, so the colon
/// is both the name test and the region test.
fn is_expl3_name(text: &str) -> bool {
    text.contains(':')
}

/// The control word a `COMMAND` node leads with, if any.
fn command_head(command: &SyntaxNode) -> Option<String> {
    command
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| {
            matches!(
                t.kind(),
                SyntaxKind::CONTROL_WORD | SyntaxKind::CONTROL_SYMBOL
            )
        })
        .map(|t| t.text().to_owned())
}

/// Whether a control word's *following* text is read — by the grammar, by the
/// definition scan, or by the lexer.
///
/// Each disjunct names the reader rather than restating its set, so a family that
/// grows grows here too.
fn head_reads_following_text(text: &str) -> bool {
    if text == BEGIN_CMD || text == END_CMD {
        // The environment name drives math routing, verbatim capture, pairing, and
        // the statement-body flag.
        return true;
    }
    if reads_definition_body(text) || is_def_prefix_command(text) {
        return true;
    }
    // `semantic::define`'s own view of what a definition is — the scan that builds
    // the `ParseCtx` this splice reuses.
    if text.strip_prefix('\\').is_some_and(is_definition_command) {
        return true;
    }
    // The lexer's raw forward scans: the short-verb and document-class toggles, and
    // the one-shot lookahead.
    reads_following_text(text)
}

/// Whether every text-reading decision in the grammar answers the same for `old`
/// and `new`.
///
/// The enumeration is the whole point, and `tests::the_text_read_survey_is_complete`
/// pins that it is still exhaustive. Each arm names the site it stands for.
pub(super) fn text_reads_are_inert(kind: SyntaxKind, old: &str, new: &str, ctx: Context) -> bool {
    // `COMMENT` and `WHITESPACE` are trivia. Every text comparison in the grammar
    // runs on a token it has already established is non-trivia — `peek_meaningful`
    // and friends skip trivia before reading — so a trivia leaf's text reaches no
    // decision at all.
    if kind != SyntaxKind::WORD {
        return true;
    }

    // `parse_block`'s statement run loop: a top-level `;`-carrying `WORD` ends a
    // picture-body statement. Only *whether* the token carries one is read, so a
    // word that gains or loses one is what matters, not where.
    if old.contains(';') != new.contains(';') {
        return false;
    }

    // `at_star_variant_marker`: a lone `*` folds into the invocation before it.
    if old == "*" || new == "*" {
        return false;
    }

    // The math operator split (`math_atom` → `split_math_word`), which cuts a word
    // into `SubTok` atoms. Equal splits are not enough: the pieces are offsets into
    // *this* text, so two words split the same way still carry different atoms.
    // Both sides must therefore split into nothing.
    //
    // Gated on being in math because that is the only place the call happens, and
    // an ungated version would refuse every hyphenated word in prose — `-` is a
    // sign, so `well-known` splits — which is most of the workload this tier is
    // for.
    if ctx.in_math && (split_math_word(old).is_some() || split_math_word(new).is_some()) {
        return false;
    }

    // `peek_meaningful_text`'s one caller asks whether the next non-trivia token
    // starts with a backslash. A `WORD` in a tree never does — a leading backslash
    // lexes as a control sequence — and a *proposed* one that does is refused by
    // the isolated relex a few lines later anyway. Checked here rather than left to
    // that: a guard whose necessity rests on the order of two other guards is one
    // refactor away from being no guard at all.
    if old.starts_with('\\') || new.starts_with('\\') {
        return false;
    }

    true
}

/// Diagnostics for the spliced tree: keep the prefix, shift the suffix, refuse any
/// error that touches the leaf.
///
/// Refusing an overlap rather than regenerating it is the refusal-first contract:
/// an error *about* the leaf may change its message, its extent, or disappear, and
/// none of that is derivable from a relex of one token. Since a full parse emits
/// errors in offset order (pinned by the harness), shifting a suffix keeps them
/// sorted.
pub(super) fn shifted_errors(
    errors: &[SyntaxError],
    leaf: TextRange,
    edit: &Edit,
) -> Option<Vec<SyntaxError>> {
    let start = usize::from(leaf.start());
    let end = usize::from(leaf.end());
    let delta = edit.delta();

    let mut out = Vec::with_capacity(errors.len());
    for error in errors {
        if error.end <= start {
            out.push(error.clone());
        } else if error.start >= end {
            out.push(SyntaxError {
                message: error.message.clone(),
                start: error.start.checked_add_signed(delta)?,
                end: error.end.checked_add_signed(delta)?,
            });
        } else {
            return None;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    /// Why a text-reading site in the grammar cannot see a leaf a tier splices.
    ///
    /// The verdicts are the point of the survey below: a bare list of matched lines
    /// would prove the scan still runs, but not that anyone read what it found.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Verdict {
        /// Kind-gated to `CONTROL_WORD`/`CONTROL_SYMBOL`, or reached only through a
        /// `strip_prefix('\\')` that a `WORD`/`WHITESPACE`/`COMMENT` never survives.
        /// No tier splices those kinds.
        ControlSequence,
        /// The definition of a text accessor, not a decision that branches on one.
        Accessor,
        /// Reads a length to advance an offset; the *content* reaches no decision.
        Offsets,
        /// A real read of a spliceable leaf's text, neutralized by a named guard in
        /// [`text_reads_are_inert`].
        Guarded(&'static str),
        /// A real read, neutralized by a named ban in [`context_admits`] — the leaf
        /// can never be in a position where the read happens.
        Context(&'static str),
    }
    use Verdict::*;

    /// Every place the grammar branches on a token's text, and why each is safe.
    ///
    /// `the_text_read_survey_is_complete` scans the grammar sources and asserts the
    /// set of matched lines is exactly the set of keys here. A new text read is
    /// therefore a *failing test* naming the line nobody classified, rather than a
    /// silent hole in a tier's soundness argument.
    const TEXT_READS: &[(&str, Verdict)] = &[
        (
            "&& P::MATH_ANCHOR.anchors(t.text.as_str()) =>",
            ControlSequence,
        ),
        (
            "&& let Some(pieces) = split_math_word(self.text())",
            Guarded("the math operator split, gated on `Context::in_math`"),
        ),
        ("&& self.tokens[i].text == END_CMD", ControlSequence),
        (".map(|t| t.text.as_str())", Accessor),
        (".text", ControlSequence),
        (r".then(|| t.text.strip_prefix('\\'))", ControlSequence),
        ("0 => definition_name_slots(&t.text),", ControlSequence),
        (
            r#"Some(SyntaxKind::CONTROL_SYMBOL) => matches!(self.text(), "\\]" | "\\)"),"#,
            ControlSequence,
        ),
        (
            r#"Some(SyntaxKind::CONTROL_SYMBOL) if matches!(self.text(), "\\]" | "\\)") => {"#,
            ControlSequence,
        ),
        (
            r#"Some(SyntaxKind::CONTROL_SYMBOL) if self.text() == "\\\\" => self.line_break(),"#,
            ControlSequence,
        ),
        (
            "Some(SyntaxKind::CONTROL_SYMBOL) if self.text() == closer => {",
            ControlSequence,
        ),
        (
            "SyntaxKind::CONTROL_SYMBOL => match t.text.as_str() {",
            ControlSequence,
        ),
        (
            "SyntaxKind::WORD if is_param_digit_text(&t.text) => {",
            Context("a `#` immediately before the leaf"),
        ),
        (
            "SyntaxKind::WORD if t.text.chars().count() == 1 => {",
            Context("an expl3 (colon-carrying) head on an ancestor `COMMAND`"),
        ),
        (
            "[t] => Cow::Borrowed(t.text.trim()),",
            Context("a `NAME_GROUP`/`BEGIN`/`END` ancestor, or a `\\begin` head"),
        ),
        (
            "if let Some(toggle) = expl_toggle(&t.text) {",
            ControlSequence,
        ),
        (
            "if self.kind() == Some(SyntaxKind::CONTROL_SYMBOL) && self.text() == closer {",
            ControlSequence,
        ),
        (
            r#"if self.kind() == Some(SyntaxKind::WORD) && self.text() == "*" {"#,
            Guarded("the lone-`*` ban"),
        ),
        (
            "if self.tokens.get(idx).is_some_and(|t| t.text == BEGIN_CMD) {",
            ControlSequence,
        ),
        (
            "if self.tokens.get(self.pos).map(|t| (t.kind, t.text.as_str()))",
            Guarded("the lone-`*` ban (`at_star_variant_marker`)"),
        ),
        ("if t.text.as_str() == RIGHT_CMD {", ControlSequence),
        (
            "let bracket = if is_big_delimiter_command(self.text()) {",
            ControlSequence,
        ),
        (
            "let def_prefix = is_def_prefix_command(self.text());",
            ControlSequence,
        ),
        (
            "let kind = match self.tokens[s.next].text.as_str() {",
            ControlSequence,
        ),
        (r"let name = t.text.strip_prefix('\\')?;", ControlSequence),
        ("let sym = self.text().to_owned();", ControlSequence),
        (
            "let text = self.p.tokens[idx].text.as_str();",
            ControlSequence,
        ),
        ("match self.p.tokens[idx].text.as_str() {", ControlSequence),
        (
            "name.push_str(&t.text);",
            Context("a `NAME_GROUP`/`BEGIN`/`END` ancestor, or a `\\begin` head"),
        ),
        (
            "name.push_str(self.text());",
            Context("a `NAME_GROUP`/`BEGIN`/`END` ancestor, or a `\\begin` head"),
        ),
        ("off += t.text.len();", Offsets),
        (
            "return Some(t.text.as_str());",
            Guarded("the leading-backslash ban (`peek_meaningful_text`)"),
        ),
        (
            "self.in_def_body = saved || is_definition_body_command(self.text());",
            ControlSequence,
        ),
        (
            "self.kind() == Some(SyntaxKind::CONTROL_WORD) && self.text() == name",
            ControlSequence,
        ),
        (
            r#"self.kind() == Some(SyntaxKind::WORD) && self.text().contains(';');"#,
            Guarded("the `;` presence ban"),
        ),
        (
            "self.tokens[idx].text == BEGIN_CMD && self.env_name_follows(idx)",
            ControlSequence,
        ),
        (
            "self.tokens[idx].text == END_CMD && self.env_name_follows(idx)",
            ControlSequence,
        ),
        (
            "t.kind == SyntaxKind::CONTROL_SYMBOL && t.text.as_str() == self.closer",
            ControlSequence,
        ),
        (
            "t.kind == SyntaxKind::CONTROL_WORD && t.text.as_str() == LEFT_CMD",
            ControlSequence,
        ),
        (
            "t.kind == SyntaxKind::CONTROL_WORD && t.text.as_str() == RIGHT_CMD",
            ControlSequence,
        ),
        (
            r"t.text.strip_prefix('\\').and_then(conditional::flow_word)",
            ControlSequence,
        ),
        (
            r"} else if t.text.strip_prefix('\\').and_then(conditional::flow_word)",
            ControlSequence,
        ),
    ];

    /// The grammar sources the survey reads. Compiled in, so the scan cannot go
    /// looking at a stale checkout or silently find no files.
    const GRAMMAR_SOURCES: &[(&str, &str)] = &[
        ("grammar.rs", include_str!("../grammar.rs")),
        ("grammar/prescan.rs", include_str!("../grammar/prescan.rs")),
        ("grammar/expl3.rs", include_str!("../grammar/expl3.rs")),
        ("grammar/trivia.rs", include_str!("../grammar/trivia.rs")),
        ("grammar/facts.rs", include_str!("../grammar/facts.rs")),
        ("conditional.rs", include_str!("../conditional.rs")),
    ];

    /// Every line of `src` that reads a token's text.
    ///
    /// Deliberately crude: `.text` not followed by an identifier character, which
    /// catches `.text()`, `.text.as_str()`, and `.text.len()` while skipping
    /// `.text_range()` (an offset API) and identifiers that merely contain the word.
    /// Over-matching costs a line in the table; under-matching costs the guarantee,
    /// so the bias is deliberate.
    fn text_reads(src: &str) -> Vec<&str> {
        // The `#[cfg(test)]` tail is test code, which decides nothing at parse time.
        let body = src.split("\n#[cfg(test)]").next().unwrap_or(src);
        body.lines()
            .map(str::trim)
            .filter(|line| !line.starts_with("//"))
            .filter(|line| {
                line.match_indices(".text").any(|(i, _)| {
                    !line[i + 5..]
                        .chars()
                        .next()
                        .is_some_and(|c| c == '_' || c.is_alphanumeric())
                })
            })
            .collect()
    }

    /// Every tier's soundness rests on [`TEXT_READS`] being the *whole* set of
    /// places the grammar branches on a token's text. Nothing but this test keeps
    /// that true: the guards have no compile-time link to the sites they neutralize,
    /// so a new comparison would otherwise land silently and the oracle would only
    /// catch it if a fuzz seed happened to spell it.
    #[test]
    fn the_text_read_survey_is_complete() {
        let mut found: Vec<&str> = GRAMMAR_SOURCES
            .iter()
            .flat_map(|(_, src)| text_reads(src))
            .collect();
        found.sort_unstable();
        found.dedup();

        let mut classified: Vec<&str> = TEXT_READS.iter().map(|(line, _)| *line).collect();
        classified.sort_unstable();
        classified.dedup();
        assert_eq!(
            classified.len(),
            TEXT_READS.len(),
            "the survey table has duplicate entries",
        );

        let unclassified: Vec<&&str> = found.iter().filter(|l| !classified.contains(l)).collect();
        assert!(
            unclassified.is_empty(),
            "the grammar grew text reads nobody classified. Each one is a decision \
             that could differ between a splice and a full parse, so it must be \
             added to `TEXT_READS` with a verdict — and, if it can see a leaf a tier \
             splices, a guard in `text_reads_are_inert` or `context_admits` \
             besides:\n{unclassified:#?}",
        );

        let stale: Vec<&&str> = classified.iter().filter(|l| !found.contains(l)).collect();
        assert!(
            stale.is_empty(),
            "the survey table names lines the grammar no longer has. Drop them — a \
             table that outlives its sites stops describing anything:\n{stale:#?}",
        );
    }

    /// The scanner must be able to find something, or the test above passes by
    /// looking at nothing — panache lost two thirds of its fuzz coverage exactly
    /// that way, with every assertion still green.
    #[test]
    fn the_text_read_scanner_finds_the_sites_it_claims_to() {
        for (name, src) in GRAMMAR_SOURCES {
            assert!(
                !src.is_empty(),
                "{name} is empty: include_str! resolved wrong"
            );
        }
        let found = GRAMMAR_SOURCES
            .iter()
            .flat_map(|(_, src)| text_reads(src))
            .count();
        assert!(
            found >= 40,
            "the scanner found only {found} text reads; it has stopped matching",
        );
        // And it must not match things that are not reads.
        assert!(text_reads("let x = node.text_range().start();").is_empty());
        assert!(text_reads("self.text_bracket_batch.clear();").is_empty());
        assert!(text_reads("// t.text == BEGIN_CMD").is_empty());
        assert_eq!(text_reads("if t.text == BEGIN_CMD {").len(), 1);
    }
}
