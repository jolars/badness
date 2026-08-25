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
use crate::parser::grammar::{BEGIN_CMD, END_CMD, reads_definition_body};
use crate::parser::lexer::reads_following_text;
use crate::semantic::define::is_definition_command;
use crate::syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

use super::Edit;

/// What the leaf's surroundings say about which text reads can reach it.
#[derive(Clone, Copy)]
pub(super) struct Context {
    /// The leaf sits under an explicit `MATH` body, where script adjacency can
    /// split a `WORD`. Positional math arguments need no semantic lookup here:
    /// the token tier recognizes their actual `SCRIPTED`/adjacent-word shape.
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
///
/// The line scan runs back from `relex_from`, the first token of the span the tier
/// actually relexes, rather than from the leaf. For the token tier the two are the
/// same token. For the protected-body tier they are not, and the difference is the
/// whole reason that tier can splice at all: it relexes from the construct's opener,
/// so a reader *inside* the fragment — the `\begin` whose forward scan finds the raw
/// body's `\end` — is reproduced by the relex rather than merely assumed inert. Only
/// a reader that starts *before* the relexed span is unaccounted for, and that is
/// exactly what the scan still covers.
pub(super) fn context_admits(leaf: &SyntaxToken, relex_from: &SyntaxToken) -> Option<Context> {
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
    let mut prev = relex_from.prev_token();
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
///
/// Dispatched on the leaf's **kind**, not on the tier that spliced it: what the
/// grammar may read is a property of the token. Each kind gets the argument that
/// actually covers it, and a kind nobody has classified is refused rather than
/// waved through — the alternative is a future tier inheriting a pass it never
/// asked for.
pub(super) fn text_reads_are_inert(kind: SyntaxKind, old: &str, new: &str, ctx: Context) -> bool {
    match kind {
        // Trivia. Every text comparison in the grammar runs on a token it has
        // already established is non-trivia — `peek_meaningful` and friends skip
        // trivia before reading — so a trivia leaf's text reaches no decision at
        // all.
        SyntaxKind::WHITESPACE | SyntaxKind::COMMENT => true,
        SyntaxKind::WORD => word_reads_are_inert(old, new, ctx),
        SyntaxKind::VERB | SyntaxKind::VERBATIM_BODY => raw_capture_reads_are_inert(old, new),
        _ => false,
    }
}

/// [`text_reads_are_inert`] for a `WORD`, which the grammar branches on in three
/// places.
fn word_reads_are_inert(old: &str, new: &str, ctx: Context) -> bool {
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

    // Math parsing can split any `WORD`: a script next to the token isolates a
    // one-character base or argument. A leaf
    // splice cannot prove that those boundaries remain unchanged from the leaf
    // text alone because script adjacency lives outside it. Decline every math
    // `WORD`; the shared reparse oracle routes the edit to a wider tier or a full
    // parse. Prose words retain the token-tier fast path.
    if ctx.in_math {
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

/// [`text_reads_are_inert`] for the raw-capture kinds — a `VERB` or a
/// `VERBATIM_BODY`, which the protected-body tier splices.
///
/// A raw capture is opaque to the grammar in every way but one. `attach_arguments`
/// asks, through [`peek_meaningful_text`], whether an abutting `VERB`'s text starts
/// with a backslash: a *standalone* `\verb|…|` is self-contained and must not become
/// the argument of the command before it, while a `\lstinline`'s bare `|…|` must.
/// So the answer to that question has to survive the splice.
///
/// The read is worth spelling out because it is the one place a raw capture is
/// visible where a `WORD` is not. `peek_meaningful_text` skips *trivia* and returns
/// whatever it lands on; the `WORD` arm above can dismiss it (a `WORD` in a tree
/// never starts with a backslash — that lexes as a control sequence), and a raw
/// capture cannot.
///
/// Everything else the grammar does with these tokens reads their *kind*. The
/// remaining text reads are gated to a control sequence, gated to a `WORD`, or
/// banned by position in [`context_admits`]; the survey below carries the per-site
/// verdicts.
///
/// [`peek_meaningful_text`]: crate::parser::grammar
fn raw_capture_reads_are_inert(old: &str, new: &str) -> bool {
    old.starts_with('\\') == new.starts_with('\\')
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
    use crate::syntax::SyntaxKind;

    use super::text_reads_are_inert;

    fn ctx() -> super::Context {
        super::Context { in_math: false }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Verdict {
        ControlSequence,
        Accessor,
        Offsets,
        Guarded(&'static str),
        Context(&'static str),
    }
    use Verdict::*;

    const TEXT_READS: &[(&str, Verdict)] = &[
        (
            "&& P::MATH_ANCHOR.anchors(t.text.as_str()) =>",
            ControlSequence,
        ),
        ("&& super::is_def_prefix_command(&t.text)", ControlSequence),
        (
            "let end = self.tokens[idx].text.len();",
            Guarded("math WORD slicing, guarded by the token tier's structural proof"),
        ),
        (
            "let last = self.tokens[idx].text[start..end]",
            Guarded("math WORD slicing, guarded by the token tier's structural proof"),
        ),
        (
            "let text = &self.tokens[idx].text;",
            Guarded("math WORD slicing, guarded by the token tier's structural proof"),
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
            Guarded("the lone-`*` ban, itself gated on `WORD`"),
        ),
        (
            "if self.tokens.get(idx).is_some_and(|t| t.text == BEGIN_CMD) {",
            ControlSequence,
        ),
        (
            "if self.tokens.get(self.pos).map(|t| (t.kind, t.text.as_str()))",
            Guarded("the lone-`*` ban (`at_star_variant_marker`), gated on `WORD`"),
        ),
        ("if t.text.as_str() == RIGHT_CMD {", ControlSequence),
        (
            "let bracket = if is_big_delimiter_command(self.text()) {",
            ControlSequence,
        ),
        (
            "let builtin_args = builtin_command_args(self.text());",
            ControlSequence,
        ),
        (
            "is_def_prefix_command(self.text()) || is_command_definition_command(self.text());",
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
            Guarded(
                "the leading-backslash ban (`peek_meaningful_text`) — a `WORD` may not \
             gain one, and a raw capture may not gain or lose one",
            ),
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
            Guarded("the `;` presence ban, gated on `WORD`"),
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

    const GRAMMAR_SOURCES: &[(&str, &str)] = &[
        ("grammar.rs", include_str!("../grammar.rs")),
        ("grammar/prescan.rs", include_str!("../grammar/prescan.rs")),
        ("grammar/expl3.rs", include_str!("../grammar/expl3.rs")),
        ("grammar/trivia.rs", include_str!("../grammar/trivia.rs")),
        ("grammar/facts.rs", include_str!("../grammar/facts.rs")),
        ("conditional.rs", include_str!("../conditional.rs")),
    ];

    fn text_reads(src: &str) -> Vec<&str> {
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
        assert!(text_reads("let x = node.text_range().start();").is_empty());
        assert!(text_reads("self.text_bracket_batch.clear();").is_empty());
        assert!(text_reads("// t.text == BEGIN_CMD").is_empty());
        assert_eq!(text_reads("if t.text == BEGIN_CMD {").len(), 1);
    }

    #[test]
    fn an_unclassified_kind_is_refused() {
        let ctx = ctx();
        assert!(!text_reads_are_inert(
            SyntaxKind::CONTROL_WORD,
            "\\a",
            "\\ab",
            ctx
        ));
        assert!(!text_reads_are_inert(SyntaxKind::L_BRACE, "{", "{", ctx));
    }

    #[test]
    fn a_raw_capture_may_not_gain_or_lose_its_leading_backslash() {
        let ctx = ctx();
        for kind in [SyntaxKind::VERB, SyntaxKind::VERBATIM_BODY] {
            assert!(text_reads_are_inert(kind, "|a|", "|ab|", ctx));
            assert!(text_reads_are_inert(kind, "\\verb|a|", "\\verb|ab|", ctx));
            assert!(!text_reads_are_inert(kind, "|a|", "\\a|", ctx));
            assert!(!text_reads_are_inert(kind, "\\verb|a|", "verb|a|", ctx));
        }
    }

    #[test]
    fn the_word_guards_did_not_follow_the_raw_captures() {
        let ctx = ctx();
        assert!(!text_reads_are_inert(SyntaxKind::WORD, "a", "a;", ctx));
        assert!(!text_reads_are_inert(SyntaxKind::WORD, "*", "**", ctx));
        assert!(text_reads_are_inert(
            SyntaxKind::VERBATIM_BODY,
            "a",
            "a; *",
            ctx
        ));
    }
}
