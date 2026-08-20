//! The token tier: relex one leaf in isolation and splice it in place.
//!
//! # What it does
//!
//! An edit that lands strictly inside a single `WORD` / `WHITESPACE` / `COMMENT`
//! leaf changes that leaf's *text* and nothing else. Rowan's
//! [`SyntaxToken::replace_with`] rebuilds only the leaf-to-root spine and shares
//! every green node off it, so the splice is `O(depth)` rather than `O(file)`.
//! Diagnostics keep their prefix, shift their suffix, and refuse anything that
//! touches the leaf.
//!
//! # Why it is sound
//!
//! This is the argument a reviewer has to check, so it is written out rather than
//! implied.
//!
//! Everything the parser does is a function of two things: the **token vector**
//! and the [`ParseCtx`](crate::parser::lexer::ParseCtx). Fix those two and the
//! grammar is deterministic — the shape gates, the prescan indices, the trivia
//! binding, and the attachment walk all read tokens, never source offsets. So a
//! splice reproduces a full parse exactly when it can show that
//!
//! 1. the token **kind** sequence is unchanged, and only the one leaf's text moved;
//! 2. the `ParseCtx` is unchanged;
//! 3. no decision that reads a token's **text** can flip.
//!
//! Each is a guard below.
//!
//! **(1) The kind sequence.** [`lex_with`] over the new leaf text alone must yield
//! exactly one token of the leaf's own kind, and the two join probes must show it
//! still separates from its neighbours. The isolated relex is faithful because the
//! lexer's modes cannot be entered or left by a token of these three kinds: every
//! mode is armed by a control word, a brace, or a `\begin{…}` name — and a leaf
//! that relexes to a single `WORD`/`WHITESPACE`/`COMMENT` spells none of them.
//! Conversely the leaf's *presence* in the tree as one of these kinds is what
//! proves the lexer was in the ordinary regime there: inside `\verb` or a verbatim
//! body the same bytes would be a `VERB` or `VERBATIM_BODY`. The join probes are
//! not decoration — `\foo` followed by `WORD("1ab")` is two tokens only because the
//! word starts with a non-letter, and editing it to `aab` would merge the pair into
//! one control word.
//!
//! **(2) The context.** [`scan_definitions`](crate::semantic::define::scan_definitions)
//! walks only `COMMAND` nodes whose head names a definition family, so a leaf that
//! sits under none of them cannot change what the scan found.
//! [`context_admits`](super::leaf::context_admits) bans those, plus the
//! environment-name positions and the commands whose *lexing* reads the raw text
//! after them.
//!
//! **(3) The text reads.** [`text_reads_are_inert`](super::leaf::text_reads_are_inert)
//! enumerates every place the grammar branches on a `WORD`'s text, and the survey in
//! [`super::leaf`] reads the grammar sources to pin that the enumeration is still the
//! whole set. A new one appears as a failing test, not as a silent divergence.
//!
//! Both live in [`super::leaf`], because they are the same question for any tier
//! that splices one leaf.
//!
//! # What it refuses, and why that is free
//!
//! Every guard returns [`None`] and the caller full-parses, so the cost of being
//! wrong about a guard's *necessity* is speed. The deliberate refusals worth
//! knowing about: any edit carrying a line terminator, a leaf whose neighbour is
//! too large to probe cheaply, and anything in math whose word splits into
//! operator atoms.

use rowan::{GreenToken, NodeOrToken, TextRange, TextSize};

use crate::parser::lexer::lex_with;
use crate::syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

use super::leaf::{context_admits, shifted_errors, text_reads_are_inert};
use super::{Edit, ReparseBase, ReparseTier, Reparsed, finish};

/// How much neighbour text a join probe will relex.
///
/// A probe is `O(neighbour)`, so an unbounded one would make the tier `O(file)` the
/// moment a leaf sits beside a 100 KB `VERBATIM_BODY` — the exact shape this tier
/// exists to be cheaper than. Over the cap the tier refuses; a real neighbour is a
/// newline, a space, or a word.
const MAX_PROBE_BYTES: usize = 1024;

/// Splice `edit` into the single leaf that contains it, or [`None`].
pub(super) fn reparse_token(
    base: &ReparseBase<'_>,
    edit: &Edit,
    new_text: &str,
) -> Option<Reparsed> {
    // Cheapest first, and a guard must bail on cheap evidence: a rejected attempt
    // is paid *on top of* the full parse it falls back to.

    // A line terminator restructures paragraphs, comment extents, and `.dtx` lines,
    // none of which a single-leaf splice can account for. Checked on both sides of
    // the edit: a `WORD` cannot contain one, but proving that here beats assuming it.
    if edit.insert.contains(['\n', '\r']) || base.text[edit.range.clone()].contains(['\n', '\r']) {
        return None;
    }

    let root = base.syntax();
    let range = TextRange::new(
        TextSize::try_from(edit.range.start).ok()?,
        TextSize::try_from(edit.range.end).ok()?,
    );

    candidates(&root, range)
        .into_iter()
        .find_map(|leaf| try_leaf(base, edit, new_text, &leaf, range))
}

/// The leaves an edit of `range` could be inside, in preference order.
///
/// An insertion at a token boundary belongs to *either* neighbour, and which one
/// works is not knowable up front: typing a letter after a space extends the word
/// to its right, while typing one after a word extends the word to its left. Both
/// are offered and the guards decide. A non-empty range has at most one covering
/// token, and a range that straddles two lands on their parent node instead.
pub(super) fn candidates(root: &SyntaxNode, range: TextRange) -> Vec<SyntaxToken> {
    if range.is_empty() {
        root.token_at_offset(range.start()).collect()
    } else {
        match root.covering_element(range) {
            NodeOrToken::Token(t) => vec![t],
            NodeOrToken::Node(_) => Vec::new(),
        }
    }
}

fn try_leaf(
    base: &ReparseBase<'_>,
    edit: &Edit,
    new_text: &str,
    leaf: &SyntaxToken,
    range: TextRange,
) -> Option<Reparsed> {
    if !leaf.text_range().contains_range(range) {
        return None;
    }
    if !matches!(
        leaf.kind(),
        SyntaxKind::WORD | SyntaxKind::WHITESPACE | SyntaxKind::COMMENT
    ) {
        return None;
    }
    let ctx = context_admits(leaf, leaf)?;

    let leaf_start = usize::from(leaf.text_range().start());
    let old = leaf.text();
    let cut = edit.range.start.checked_sub(leaf_start)?..edit.range.end.checked_sub(leaf_start)?;
    let mut new_leaf = String::with_capacity(old.len() + edit.insert.len());
    new_leaf.push_str(old.get(..cut.start)?);
    new_leaf.push_str(&edit.insert);
    new_leaf.push_str(old.get(cut.end..)?);

    // An emptied leaf is a token *removed*, which is a change to the kind sequence
    // and so a different question than this tier answers.
    if new_leaf.is_empty() {
        return None;
    }

    if !text_reads_are_inert(leaf.kind(), old, &new_leaf, ctx) {
        return None;
    }

    // The isolated relex, under the base's own context and flavor — a `\newcommand`
    // the definition scan found must lex the fragment the way it lexed the tree.
    let relexed = lex_with(&new_leaf, base.ctx, base.config);
    if relexed.len() != 1 || relexed[0].kind != leaf.kind() {
        return None;
    }

    if !joins(base, leaf.prev_token().as_ref(), &new_leaf, Side::Before)
        || !joins(base, leaf.next_token().as_ref(), &new_leaf, Side::After)
    {
        return None;
    }

    let errors = shifted_errors(base.errors, leaf.text_range(), edit)?;
    let green = leaf.replace_with(GreenToken::new(leaf.kind().into(), &new_leaf));
    finish(green, errors, ReparseTier::Token, base, new_text)
}

/// Which side of the leaf a join probe is testing.
#[derive(Clone, Copy)]
enum Side {
    Before,
    After,
}

/// Whether the new leaf text still lexes apart from its neighbour.
///
/// The probe relexes just the pair and demands the same two tokens back. A missing
/// neighbour is the file edge, where there is nothing to merge with. A neighbour
/// that does not reproduce itself in isolation — a `VERB`, a `VERBATIM_BODY`, a
/// `WORD` that is really a sub-slice of one the math split cut up — fails the probe
/// and the tier refuses, which is the conservative answer in every one of those
/// cases.
fn joins(
    base: &ReparseBase<'_>,
    neighbour: Option<&SyntaxToken>,
    leaf_text: &str,
    side: Side,
) -> bool {
    let Some(neighbour) = neighbour else {
        return true;
    };
    let n = neighbour.text();
    if n.len() > MAX_PROBE_BYTES {
        return false;
    }
    let (first, second) = match side {
        Side::Before => (n, leaf_text),
        Side::After => (leaf_text, n),
    };
    let mut probe = String::with_capacity(first.len() + second.len());
    probe.push_str(first);
    probe.push_str(second);

    let toks = lex_with(&probe, base.ctx, base.config);
    if toks.len() != 2 || toks[0].text != first || toks[1].text != second {
        return false;
    }
    // The neighbour must also come back as *itself*. A token that lexes to a
    // different kind in isolation than it holds in the tree means the probe was run
    // in a regime the tree was not parsed in, so its verdict says nothing.
    match side {
        Side::Before => toks[0].kind == neighbour.kind(),
        Side::After => toks[1].kind == neighbour.kind(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::declarations::ResolvedDeclarations;
    use crate::parser::core::parse_with_declarations_resolved;
    use crate::parser::lexer::{LatexFlavor, LexConfig, lex_with};
    use crate::parser::reparse::{ReparseBase, reparse};

    fn with_base<R>(text: &str, f: impl FnOnce(&ReparseBase<'_>) -> R) -> R {
        let declared = ResolvedDeclarations::default();
        let (parse, ctx) = parse_with_declarations_resolved(text, LatexFlavor::Document, &declared);
        f(&ReparseBase::from_parts(
            text,
            &parse.green,
            &parse.errors,
            &ctx,
            LatexFlavor::Document.into(),
            &declared,
        ))
    }

    fn with_dtx_base<R>(text: &str, f: impl FnOnce(&ReparseBase<'_>) -> R) -> R {
        let declared = ResolvedDeclarations::default();
        let config = LexConfig {
            flavor: LatexFlavor::Document,
            dtx: true,
        };
        let (parse, ctx) = parse_with_declarations_resolved(text, config, &declared);
        f(&ReparseBase::from_parts(
            text,
            &parse.green,
            &parse.errors,
            &ctx,
            config,
            &declared,
        ))
    }

    fn edit(range: std::ops::Range<usize>, insert: &str) -> Edit {
        Edit {
            range,
            insert: insert.to_string(),
        }
    }

    fn edit_at(text: &str, needle: &str, offset: usize, insert: &str) -> Edit {
        let start = text.find(needle).expect("fixture") + offset;
        edit(start..start, insert)
    }

    fn replace_needle(text: &str, needle: &str, insert: &str) -> Edit {
        let start = text.find(needle).expect("fixture");
        edit(start..start + needle.len(), insert)
    }

    fn as_text_range(range: &std::ops::Range<usize>) -> TextRange {
        TextRange::new(
            TextSize::try_from(range.start).expect("range start"),
            TextSize::try_from(range.end).expect("range end"),
        )
    }

    fn candidate_leaf(base: &ReparseBase<'_>, e: &Edit) -> SyntaxToken {
        let range = as_text_range(&e.range);
        candidates(&base.syntax(), range)
            .into_iter()
            .find(|leaf| leaf.text_range().contains_range(range))
            .expect("expected a covering token candidate")
    }

    fn try_leaf_without_dtx_bail(base: &ReparseBase<'_>, e: &Edit, leaf: &SyntaxToken) -> bool {
        let next = e.apply(base.text);
        let range = as_text_range(&e.range);
        try_leaf(base, e, &next, leaf, range).is_some()
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum DtxLeafRefusal {
        LeafKindAllowlist,
        RelexNotSingleOrSameKind,
    }

    fn classify_dtx_leaf_refusal(
        base: &ReparseBase<'_>,
        e: &Edit,
        leaf: &SyntaxToken,
    ) -> DtxLeafRefusal {
        if !matches!(
            leaf.kind(),
            SyntaxKind::WORD | SyntaxKind::WHITESPACE | SyntaxKind::COMMENT
        ) {
            return DtxLeafRefusal::LeafKindAllowlist;
        }

        let leaf_start = usize::from(leaf.text_range().start());
        let old = leaf.text();
        let cut = e.range.start - leaf_start..e.range.end - leaf_start;
        let mut new_leaf = String::with_capacity(old.len() + e.insert.len());
        new_leaf.push_str(&old[..cut.start]);
        new_leaf.push_str(&e.insert);
        new_leaf.push_str(&old[cut.end..]);

        let relexed = lex_with(&new_leaf, base.ctx, base.config);
        if relexed.len() != 1 || relexed[0].kind != leaf.kind() {
            return DtxLeafRefusal::RelexNotSingleOrSameKind;
        }

        panic!("fixture no longer trips the expected guard: {e:?}");
    }

    /// Splices, with the tier they must reach. The oracle inside `finish` is what
    /// checks the *result*; these pin that the guards let the case through at all.
    #[track_caller]
    fn assert_splices(text: &str, e: Edit) {
        with_base(text, |base| {
            let out = reparse(base, &e, &e.apply(text));
            let out = out.unwrap_or_else(|| panic!("expected a token-tier splice for {e:?}"));
            assert_eq!(out.tier, ReparseTier::Token);
        });
    }

    #[track_caller]
    fn assert_refuses(text: &str, e: Edit) {
        with_base(text, |base| {
            assert!(
                reparse(base, &e, &e.apply(text)).is_none(),
                "expected a refusal for {e:?}",
            );
        });
    }

    #[test]
    fn splices_a_letter_typed_into_a_prose_word() {
        assert_splices("Some ordinary prose.\n", edit(5..5, "x"));
        assert_splices("Some ordinary prose.\n", edit(5..8, "sensible"));
    }

    #[test]
    fn splices_inside_a_comment_and_inside_whitespace() {
        assert_splices("text % a trailing note\nmore\n", edit(10..10, "z"));
        assert_splices("a   b\n", edit(2..2, " "));
    }

    /// Math's `WORD` slicing never runs in prose, so hyphenated words retain the
    /// ordinary token-tier fast path.
    #[test]
    fn splices_a_hyphenated_word_outside_math() {
        assert_splices("a well-known result\n", edit(6..6, "l"));
    }

    #[test]
    fn refuses_an_edit_that_carries_a_newline() {
        assert_refuses("Some ordinary prose.\n", edit(5..5, "\n"));
        assert_refuses("Some ordinary prose.\n", edit(5..5, "\r\n"));
    }

    /// The environment name decides routing, verbatim capture, and pairing. A relex
    /// to the same kind proves nothing about any of it, so the position is banned
    /// outright — including for the `\begin` a shape gate demoted to a plain
    /// command, where the name sits in a `GROUP` rather than a `NAME_GROUP`.
    #[test]
    fn refuses_an_environment_name() {
        assert_refuses(
            "\\begin{itemize}\n\\item x\n\\end{itemize}\n",
            edit(8..8, "z"),
        );
        assert_refuses("{\\begin{itemize}\\item x}\n", edit(9..9, "z"));
    }

    #[test]
    fn refuses_a_definition_body_and_a_document_class() {
        // The definition scan builds the `ParseCtx` the splice reuses.
        assert_refuses("\\newcommand{\\bea}{\\begin{align}}\n", edit(26..26, "z"));
        // The lexer reads this name to decide whether `|` is a short verb.
        assert_refuses("\\documentclass{ltxdoc}\n", edit(16..16, "z"));
    }

    /// In math every word may be cut into operator or script-boundary atoms, so
    /// its text and adjacency are structural. Refuse even an ordinary letter edit;
    /// the shared driver will try a wider tier or a full parse.
    #[test]
    fn refuses_every_math_word() {
        assert_refuses("$ab$\n", edit(2..2, "c"));
        assert_refuses("$a b$\n", edit(3..3, "+"));
        assert_refuses("\\begin{align}\n  a b\n\\end{align}\n", edit(17..17, "+"));
    }

    /// A `;` ends a picture-body statement, so gaining or losing one restructures
    /// the tree even though the token's kind is unchanged.
    #[test]
    fn refuses_a_word_that_gains_a_statement_terminator() {
        let text = "\\begin{tikzpicture}\n  \\draw (0,0) -- (1,1);\n\\end{tikzpicture}\n";
        // The end of `(0,0)`, which carries no `;` yet.
        let at = text.find("(0,0)").expect("fixture") + 5;
        assert_refuses(text, edit(at..at, ";"));
    }

    /// The backward join probe. `\foo` and `1ab` are two tokens only because the
    /// word starts with a non-letter; editing it to `aab` would merge the pair into
    /// a single control word, which is a change to the token *kind* sequence.
    #[test]
    fn refuses_an_edit_that_would_merge_with_the_previous_token() {
        assert_refuses("\\foo1ab\n", edit(4..5, "a"));
    }

    /// Phase 6.5's `.dtx` argument, stated as a guard enumeration instead of as
    /// "the sweep found nothing": each lexer state bit that differs from a
    /// fragment-at-offset-0 relex has a counterexample and the guard that refuses it.
    #[test]
    fn dtx_state_bit_survey_is_complete_for_the_token_tier() {
        use DtxLeafRefusal::{LeafKindAllowlist, RelexNotSingleOrSameKind};

        struct Case {
            state_bit: &'static str,
            text: &'static str,
            edit: Edit,
            expected: DtxLeafRefusal,
        }

        let cases = [
            Case {
                state_bit: "at_line_start",
                text: "% alpha\n",
                // A `%` at fragment column 0 lexes as DOC_MARGIN, not WORD.
                edit: edit_at("% alpha\n", "alpha", 0, "%"),
                expected: RelexNotSingleOrSameKind,
            },
            Case {
                state_bit: "in_doc_line",
                text: "% alpha\n",
                // `^^A` in a doc line is a comment in-file, but a fragment has no
                // doc-line context and does not relex to one WORD token.
                edit: edit_at("% alpha\n", "alpha", 0, "^^A"),
                expected: RelexNotSingleOrSameKind,
            },
            Case {
                state_bit: "at_letter",
                text: "%    \\begin{macrocode}\n\\foo@bar\n%    \\end{macrocode}\n",
                // `@`-bearing command names are CONTROL_WORDs in macrocode.
                edit: edit_at(
                    "%    \\begin{macrocode}\n\\foo@bar\n%    \\end{macrocode}\n",
                    "foo@bar",
                    4,
                    "z",
                ),
                expected: LeafKindAllowlist,
            },
            Case {
                state_bit: "expl_syntax",
                text: "%    \\begin{macrocode}\n\\ExplSyntaxOn\n\\foo_bar:n\n%    \\end{macrocode}\n",
                // Colon/underscore expl3 names are CONTROL_WORDs under ExplSyntaxOn.
                edit: edit_at(
                    "%    \\begin{macrocode}\n\\ExplSyntaxOn\n\\foo_bar:n\n%    \\end{macrocode}\n",
                    "foo_bar:n",
                    3,
                    "z",
                ),
                expected: LeafKindAllowlist,
            },
            Case {
                state_bit: "macrocode",
                text: "%    \\begin{macrocode}\n% comment\n%    \\end{macrocode}\n",
                // In macrocode, line-leading `%` is COMMENT, not DOC_MARGIN.
                edit: edit_at(
                    "%    \\begin{macrocode}\n% comment\n%    \\end{macrocode}\n",
                    "comment",
                    3,
                    "x",
                ),
                expected: RelexNotSingleOrSameKind,
            },
            Case {
                state_bit: "implicit_expl",
                text: "%<@@=demo>\n%    \\begin{macrocode}\n\\foo_bar:n\n%    \\end{macrocode}\n",
                // `%<@@=...>` turns on implicit expl3 in macrocode; affected names
                // are CONTROL_WORDs and therefore outside the leaf allowlist.
                edit: edit_at(
                    "%<@@=demo>\n%    \\begin{macrocode}\n\\foo_bar:n\n%    \\end{macrocode}\n",
                    "foo_bar:n",
                    3,
                    "z",
                ),
                expected: LeafKindAllowlist,
            },
            Case {
                state_bit: "short_verbs",
                text: "% alpha\n",
                // `.dtx` docs start with `|` as a short-verb delimiter.
                edit: replace_needle("% alpha\n", "alpha", "|a|"),
                expected: RelexNotSingleOrSameKind,
            },
        ];
        assert_eq!(cases.len(), 7, "enumerate every dtx state bit exactly once");

        for case in cases {
            with_dtx_base(case.text, |base| {
                let leaf = candidate_leaf(base, &case.edit);
                assert!(
                    !try_leaf_without_dtx_bail(base, &case.edit, &leaf),
                    "fixture for `{}` unexpectedly spliced",
                    case.state_bit
                );
                let got = classify_dtx_leaf_refusal(base, &case.edit, &leaf);
                assert_eq!(got, case.expected, "state bit `{}`", case.state_bit);
            });
        }
    }

    /// `.dtx` is no longer refused wholesale: ordinary doc-line words can splice.
    #[test]
    fn splices_a_doc_line_word_in_a_dtx_parse() {
        let text = "% alpha beta\n";
        with_dtx_base(text, |base| {
            let e = edit_at(text, "alpha", 3, "z");
            let out = reparse(base, &e, &e.apply(text)).expect("expected dtx splice");
            assert_eq!(out.tier, ReparseTier::Token);
        });
    }

    /// Deleting a leaf outright removes a token, which this tier does not model.
    #[test]
    fn refuses_an_edit_that_empties_the_leaf() {
        assert_refuses("a bb c\n", edit(2..4, ""));
    }

    /// A diagnostic that *touches* the leaf may change its message or extent, and
    /// neither is derivable from a relex of one token. One that sits after it just
    /// shifts.
    #[test]
    fn shifts_errors_after_the_leaf_and_refuses_ones_that_touch_it() {
        let text = "word\n\n\\begin{itemize}\n";
        with_base(text, |base| {
            assert!(
                !base.errors.is_empty(),
                "this fixture exists to carry an error"
            );
            let e = edit(2..2, "z");
            let out = reparse(base, &e, &e.apply(text)).expect("a splice before the error");
            assert_eq!(out.errors.len(), base.errors.len());
            assert_eq!(out.errors[0].start, base.errors[0].start + 1);
        });
    }

    /// The neighbour cap keeps the join probe from making the tier `O(file)`.
    ///
    /// Paired with the same edit beside a short neighbour, because a refusal on its
    /// own proves nothing about *which* guard refused — the first draft of this
    /// test was tripping the `\\end` scan instead and looked just as green.
    #[test]
    fn refuses_a_leaf_beside_an_oversized_neighbour() {
        let long = "a".repeat(MAX_PROBE_BYTES + 10);
        // The leaf is the space; its previous token is the word beside it.
        assert_refuses(&format!("{long} b\n"), edit(long.len()..long.len(), " "));

        let short = "a".repeat(8);
        assert_splices(&format!("{short} b\n"), edit(short.len()..short.len(), " "));
    }
}
