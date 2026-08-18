//! The protected-body tier: relex a raw capture with its delimiters and splice it.
//!
//! # What it does
//!
//! An edit inside a `VERBATIM_BODY` (an `lstlisting`, a `minted`, a `verbatim`) or a
//! `VERB` (`\verb|…|`, `\url{…}`, `\lstinline|…|`) changes that leaf's text and
//! nothing else, exactly as a prose keystroke does for the [token
//! tier](super::token). The splice is the same one-leaf
//! [`SyntaxToken::replace_with`], `O(depth)` in the tree.
//!
//! What differs is the *proof*. The token tier relexes its leaf **alone** and demands
//! one token of the same kind back. That is unavailable here: a raw capture is a kind
//! the lexer only ever emits once it has already seen an opener, so a body lexed on
//! its own comes back as ordinary prose. Instead this tier relexes the leaf's whole
//! enclosing node — `\begin{verbatim}` … `\end{verbatim}`, or `\url` … `}` — which
//! puts the isolated lexer into the right mode for free, rather than hand-writing a
//! catcode table that would have to be kept in step with [`lex_with`] forever.
//!
//! Newlines are allowed, unlike on the token tier. That is the point: pressing Enter
//! inside a listing is the workload, and inside a raw body a line break restructures
//! nothing, because the grammar sees one opaque token either way.
//!
//! # Why it is sound
//!
//! A parse is a function of the token vector and the [`ParseCtx`](crate::parser::lexer::ParseCtx),
//! so this splice reproduces a full parse when the file's token sequence is unchanged
//! but for the one leaf's text. Four legs, each a guard below.
//!
//! **(1) Faithfulness — the induction base.** [`lex_with`] over the *unedited*
//! fragment, under the base's own `ParseCtx` and flavor, must reproduce the fragment
//! node's own leaf tokens, kind and text, in order. That is the evidence that over
//! these bytes the isolated lexer and the file lexer agree — that the fragment's
//! lexing does not depend on the state the file arrived in. Every way it could is
//! caught by this one check rather than enumerated: a short-verb `VERB` (isolated,
//! `short_verbs` is empty), an `@`-bearing name under `\makeatletter`, a name that
//! only lexes whole inside an expl3 region, a capture the file suppressed at a brace
//! depth the fragment does not have.
//!
//! **(2) Locality — the step.** Old and new fragments differ only inside a raw
//! capture. The lexer pushes those straight to its output, so their bytes never reach
//! `apply_toggles`, `next_pending`, or the brace-depth fold: it therefore leaves the
//! fragment in the same state on both texts, and everything after the fragment lexes
//! identically. That is a claim about lexer code, so it is pinned by a test —
//! `lexer::tests::raw_capture_content_does_not_change_later_lexing`, together with
//! its counterexample, a body that *breaks* its capture and does move later lexing.
//!
//! **(3) Termination.** The capture's terminator must lie *inside* the fragment, or
//! the isolated scan stops at the fragment's edge while the file's runs on past it.
//! A `VERB` carries its own closer by construction — `delimited_len` and
//! `braced_verb_content_len` yield nothing when unterminated, so an unclosed
//! `\verb|…` is not a `VERB` token at all. A `VERBATIM_BODY` does not: its
//! `\end{name}` is a sibling, and an unclosed body simply runs to EOF. So the tier
//! requires a token to follow the body *within* the fragment, which is that `\end`.
//!
//! **(4) The sequence check.** [`lex_with`] over the *edited* fragment must yield the
//! same sequence as leg 1 with exactly one token differing: the leaf, same kind, new
//! text. This is the mechanical half, and it is what catches an `\end{verbatim}` typed
//! into a body (the sequence grows), an emptied body (it shrinks), a `|` that closes a
//! `\verb` early, and a brace that unbalances a `\url`.
//!
//! Boundary joins need no probe, unlike the token tier's two: the fragment's first and
//! last tokens are byte-identical between old and new, so whatever the file lexer did
//! at those seams it does again. The one exception is a self-delimited `VERB` that
//! *is* the whole fragment, whose own text changes — there the leading `\` is what
//! stops a preceding word or control word from merging into it, and it is guarded by
//! [`text_reads_are_inert`], which needs it for the grammar's own read anyway.
//!
//! # What it refuses, and why that is free
//!
//! Every guard returns [`None`] and the caller full-parses. The deliberate refusals:
//!
//! - **A `.dtx` edit that changes the implicit-expl signal.** The tier carries the
//!   base parse's full-file `implicit_expl` fact and relexes fragments under it, but
//!   if an edit changes the signal (`%<@@=...>` / `\ProvidesExpl*`) the base regime
//!   is stale by construction and the tier refuses.
//! - **A leaf whose parent is not the construct that captured it** — a short-verb span
//!   sitting loose in a `PARAGRAPH`, a body under a `\begin` some shape gate demoted.
//!   The check is `O(1)` and it is what keeps the tier from relexing a whole paragraph
//!   only to refuse.
//! - **Everything [`context_admits`] bans**, which for this tier chiefly means the
//!   `VERB` holding an l3doc `\begin{macro}{\foo}` name: that sits under a `BEGIN`,
//!   where the environment-name reads live.
//!
//! There is no cap on the fragment's size, unlike the token tier's neighbour probe. A
//! fragment is bounded by the file, a raw-body relex is a `find("\\end{")` scan plus
//! one string copy, and the thing it competes with is a full parse *and* tree build of
//! that same file — so the ceiling is well under the fallback it would be paid on top
//! of. The tier exists for the 100 KB listing; capping it would refuse exactly that.

use rowan::{GreenToken, NodeOrToken, TextRange, TextSize};

use crate::parser::lexer::{Token, dtx_has_expl_signal, lex_with, lex_with_implicit_expl};
use crate::syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

use super::leaf::{context_admits, shifted_errors, text_reads_are_inert};
use super::token::candidates;
use super::{Edit, ReparseBase, ReparseTier, Reparsed, finish};

/// Splice `edit` into the raw capture that contains it, or [`None`].
pub(super) fn reparse_protected(
    base: &ReparseBase<'_>,
    edit: &Edit,
    new_text: &str,
) -> Option<Reparsed> {
    // Cheapest first: a rejected attempt is paid *on top of* the full parse it falls
    // back to, and this tier's relex is the most expensive thing in the ladder.
    if base.config.dtx && dtx_has_expl_signal(new_text) != base.implicit_expl {
        return None;
    }

    let root = base.syntax();
    let range = TextRange::new(
        TextSize::try_from(edit.range.start).ok()?,
        TextSize::try_from(edit.range.end).ok()?,
    );

    candidates(&root, range)
        .into_iter()
        .find_map(|leaf| try_capture(base, edit, new_text, &leaf, range))
}

fn try_capture(
    base: &ReparseBase<'_>,
    edit: &Edit,
    new_text: &str,
    leaf: &SyntaxToken,
    range: TextRange,
) -> Option<Reparsed> {
    if !leaf.text_range().contains_range(range) {
        return None;
    }
    if !matches!(leaf.kind(), SyntaxKind::VERB | SyntaxKind::VERBATIM_BODY) {
        return None;
    }
    let fragment = Fragment::around(leaf)?;
    let tokens = fragment.tokens();
    let ctx = context_admits(leaf, tokens.first()?)?;
    let at = tokens
        .iter()
        .position(|t| t.text_range() == leaf.text_range())?;

    // Leg 3. A `VERBATIM_BODY`'s terminator is the sibling `\end{name}`, so it has to
    // be in the fragment; a `VERB`'s is inside its own text by construction.
    if leaf.kind() == SyntaxKind::VERBATIM_BODY && at + 1 == tokens.len() {
        return None;
    }

    let old_leaf = leaf.text();
    let new_leaf = edited(old_leaf, usize::from(leaf.text_range().start()), edit)?;
    // An emptied leaf is a token *removed*, a change to the kind sequence and so a
    // different question than this tier answers. Leg 4 would catch it; refusing here
    // keeps the reason legible.
    if new_leaf.is_empty() {
        return None;
    }
    if !text_reads_are_inert(leaf.kind(), old_leaf, &new_leaf, ctx) {
        return None;
    }

    // Leg 1: the unedited fragment must lex, in isolation, to exactly the tokens the
    // tree holds for it.
    let old_fragment = fragment.text();
    if !relexes_to(base, &old_fragment, &tokens, None) {
        return None;
    }

    // Leg 4: the edited fragment must lex to the same sequence with only this leaf's
    // text changed.
    let new_fragment = edited(&old_fragment, usize::from(fragment.range().start()), edit)?;
    if !relexes_to(base, &new_fragment, &tokens, Some((at, &new_leaf))) {
        return None;
    }

    let errors = shifted_errors(base.errors, leaf.text_range(), edit)?;
    let green = leaf.replace_with(GreenToken::new(leaf.kind().into(), &new_leaf));
    finish(green, errors, ReparseTier::Verbatim, base, new_text)
}

/// The text `edit` produces from `source`, which starts at byte `origin` in the
/// document. [`None`] when the edit does not land inside `source` on char boundaries.
fn edited(source: &str, origin: usize, edit: &Edit) -> Option<String> {
    let cut = edit.range.start.checked_sub(origin)?..edit.range.end.checked_sub(origin)?;
    let mut out = String::with_capacity(source.len() + edit.insert.len());
    out.push_str(source.get(..cut.start)?);
    out.push_str(&edit.insert);
    out.push_str(source.get(cut.end..)?);
    Some(out)
}

/// Whether `text` lexes, in isolation under the base's own inputs, to exactly
/// `expected` — optionally with the token at one index carrying replacement text.
///
/// Both legs that relex ask this same question, differing only in that argument, so
/// they share it: a check written twice is a check that can be weakened once.
fn relexes_to(
    base: &ReparseBase<'_>,
    text: &str,
    expected: &[SyntaxToken],
    replaced: Option<(usize, &str)>,
) -> bool {
    let got: Vec<Token> = if base.config.dtx {
        lex_with_implicit_expl(text, base.ctx, base.config, base.implicit_expl)
    } else {
        lex_with(text, base.ctx, base.config)
    };
    if got.len() != expected.len() {
        return false;
    }
    got.iter()
        .zip(expected)
        .enumerate()
        .all(|(i, (got, want))| {
            let want_text = match replaced {
                Some((at, replacement)) if at == i => replacement,
                _ => want.text(),
            };
            got.kind == want.kind() && got.text == want_text
        })
}

/// The span relexed to prove a capture: the leaf's enclosing construct, delimiters
/// included, because that is what puts the isolated lexer into the capturing mode.
enum Fragment {
    /// A `VERB` that carries its own opener — a standalone `\verb|…|`/`\verb*|…|`,
    /// whose text starts with the control word. Nothing around it is needed.
    Token(SyntaxToken),
    /// The node whose opener armed the capture: the `ENVIRONMENT` of a
    /// `VERBATIM_BODY`, or the `COMMAND` of an attached `VERB`.
    Node(SyntaxNode),
}

impl Fragment {
    /// The fragment for `leaf`, or [`None`] when its surroundings are not the
    /// construct that captured it.
    ///
    /// This is the tier's one `O(1)` cost guard as well as a correctness one. A
    /// short-verb span (`|…|` under `\MakeShortVerb`) and a body under a demoted
    /// `\begin` both sit loose in a `PARAGRAPH`, where the relex would have to cover
    /// the whole paragraph — and would then fail leg 1 anyway, since neither
    /// `short_verbs` nor the demotion survives isolation.
    fn around(leaf: &SyntaxToken) -> Option<Self> {
        if leaf.kind() == SyntaxKind::VERB && leaf.text().starts_with('\\') {
            return Some(Self::Token(leaf.clone()));
        }
        let parent = leaf.parent()?;
        match (leaf.kind(), parent.kind()) {
            (SyntaxKind::VERBATIM_BODY, SyntaxKind::ENVIRONMENT)
            | (SyntaxKind::VERB, SyntaxKind::COMMAND) => Some(Self::Node(parent)),
            _ => None,
        }
    }

    fn range(&self) -> TextRange {
        match self {
            Self::Token(t) => t.text_range(),
            Self::Node(n) => n.text_range(),
        }
    }

    fn text(&self) -> String {
        match self {
            Self::Token(t) => t.text().to_owned(),
            Self::Node(n) => n.text().to_string(),
        }
    }

    /// The fragment's own leaf tokens, in document order — what leg 1 compares an
    /// isolated relex against.
    fn tokens(&self) -> Vec<SyntaxToken> {
        match self {
            Self::Token(t) => vec![t.clone()],
            Self::Node(n) => n
                .descendants_with_tokens()
                .filter_map(NodeOrToken::into_token)
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::declarations::ResolvedDeclarations;
    use crate::parser::core::parse_with_declarations_resolved;
    use crate::parser::lexer::{LatexFlavor, LexConfig};
    use crate::parser::reparse::reparse;

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

    /// An edit at the byte after the first occurrence of `needle`.
    fn after(text: &str, needle: &str, insert: &str) -> Edit {
        let at = text.find(needle).expect("fixture") + needle.len();
        edit(at..at, insert)
    }

    /// The oracle inside `finish` checks the *result*; these pin that the guards let
    /// the case through at all, and on this tier rather than the token one.
    #[track_caller]
    fn assert_splices(text: &str, e: Edit) {
        with_base(text, |base| {
            let out = reparse(base, &e, &e.apply(text));
            let out = out.unwrap_or_else(|| panic!("expected a protected-body splice for {e:?}"));
            assert_eq!(out.tier, ReparseTier::Verbatim);
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
    fn splices_a_character_typed_into_a_verbatim_body() {
        assert_splices(
            "\\begin{verbatim}\n  raw { $ % \\ text\n\\end{verbatim}\n",
            after("\\begin{verbatim}\n  raw", "raw", "x"),
        );
    }

    /// The environment's own arguments are inside the fragment, so they have to relex
    /// too: an `lstlisting`'s leading optional and a `minted`'s leading required
    /// group are both lexed before the body begins.
    #[test]
    fn splices_a_body_behind_environment_arguments() {
        let text = "\\begin{lstlisting}[language=C]\nint main() { return 0; }\n\\end{lstlisting}\n";
        assert_splices(text, after(text, "int", "x"));

        let text = "\\begin{minted}{python}\nif x: pass\n\\end{minted}\n";
        assert_splices(text, after(text, "if x", ":"));
    }

    /// The workload the tier exists for, and the one the token tier bans outright.
    #[test]
    fn splices_a_newline_typed_into_a_body() {
        let text = "\\begin{lstlisting}\nint main() {}\n\\end{lstlisting}\n";
        assert_splices(text, after(text, "int main() {", "\n  "));
        assert_splices(text, after(text, "int main() {", "\r\n"));
    }

    /// The three `VERB` shapes: self-delimited (its own fragment), delimited after a
    /// command, and braced after a command.
    #[test]
    fn splices_inside_each_verb_shape() {
        assert_splices(
            "Inline \\verb|raw $ %| and after.\n",
            after("Inline \\verb|raw", "raw", "x"),
        );
        assert_splices(
            "A \\lstinline|x_$y$| here.\n",
            after("A \\lstinline|x_", "x_", "y"),
        );
        assert_splices(
            "See \\url{https://x/a_b} now.\n",
            after("See \\url{https://x/a_", "a_", "z"),
        );
    }

    /// Leg 4. Typing the closer into the body ends the environment early, which is a
    /// change to the token sequence the fragment relex sees.
    #[test]
    fn refuses_a_body_that_gains_its_own_closer() {
        let text = "\\begin{verbatim}\n  raw\n\\end{verbatim}\n";
        assert_refuses(text, after(text, "  raw", "\n\\end{verbatim}\n"));
    }

    /// Leg 4 again, from the other side: a brace that unbalances a `\url` dissolves
    /// the capture, and the braces become structure the rest of the file can see.
    #[test]
    fn refuses_a_braced_verb_that_loses_its_balance() {
        assert_refuses("See \\url{a_b} now.\n", after("See \\url{a", "a", "{"));
        assert_refuses(
            "A \\lstinline|xy| here.\n",
            after("A \\lstinline|x", "x", "|"),
        );
    }

    /// Leg 3. An unterminated body runs to EOF, so its extent is fixed by the end of
    /// the file rather than by anything inside the fragment.
    #[test]
    fn refuses_an_unterminated_verbatim_body() {
        let text = "\\begin{verbatim}\n  raw text\n";
        assert_refuses(text, after(text, "  raw", "x"));
    }

    /// Leg 1. A short-verb span is a `VERB` only because `\MakeShortVerb` armed the
    /// character earlier in the file; isolated, `short_verbs` is empty. It also sits
    /// loose in a `PARAGRAPH`, so the fragment check declines first — both are
    /// refusals, and the tier must not splice it either way.
    #[test]
    fn refuses_a_short_verb_span() {
        let text = "\\MakeShortVerb{\\|}\nnow |raw| is verbatim\n";
        assert_refuses(text, after(text, "|raw", "x"));
    }

    /// `.dtx` is no longer refused wholesale on this tier when the implicit-expl
    /// signal is stable.
    #[test]
    fn splices_a_protected_body_edit_in_a_dtx_parse_when_signal_is_stable() {
        let text = "% \\begin{macro}{\\foo}\n%    \\begin{macrocode}\n\\url{a_b}\n";
        let at = text.find("a_b").expect("fixture") + 1;
        let e = edit(at..at, "z");
        with_dtx_base(text, |base| {
            let out = reparse(base, &e, &e.apply(text)).expect("expected a protected dtx splice");
            assert_eq!(out.tier, ReparseTier::Verbatim);
        });
    }

    /// A `.dtx` edit that flips `%<@@=...>` / `\ProvidesExpl*` signal state is
    /// refused: the base's carried implicit-expl regime is stale by construction.
    #[test]
    fn refuses_a_dtx_protected_edit_that_changes_implicit_expl_signal_state() {
        let text = "% \\begin{macro}{\\foo}\n%    \\begin{macrocode}\n\\url{a_b}\n";
        // The scanner is intentionally coarse and sees signal spellings anywhere in
        // the file, including inside raw captures.
        let e = after(text, "a_b", "\\ProvidesExplFile");
        with_dtx_base(text, |base| {
            assert!(reparse(base, &e, &e.apply(text)).is_none());
        });
    }

    #[test]
    fn refuses_an_edit_that_empties_the_body() {
        let text = "\\begin{verbatim}\nx\n\\end{verbatim}\n";
        // The whole body, `\nx\n`, from the `}` of the opener to the `\end`.
        let from = text.find('\n').expect("fixture");
        let to = text.find("\\end").expect("fixture");
        assert_refuses(text, edit(from..to, ""));
    }

    /// An edit reaching out of the leaf and into a delimiter is not this tier's
    /// question: the covering element is then the node, not the token.
    #[test]
    fn refuses_an_edit_that_spans_out_of_the_leaf() {
        let text = "\\begin{verbatim}\n  raw\n\\end{verbatim}\n";
        let at = text.find("\\end").expect("fixture");
        assert_refuses(text, edit(at - 1..at + 2, "zz"));
    }

    /// A diagnostic that *touches* the leaf may change its message or extent; one
    /// after it just shifts.
    #[test]
    fn shifts_errors_after_the_body() {
        let text = "\\begin{verbatim}\n  raw\n\\end{verbatim}\n\n\\begin{itemize}\n";
        with_base(text, |base| {
            assert!(
                !base.errors.is_empty(),
                "this fixture exists to carry an error"
            );
            let e = after(text, "  raw", "xx");
            let out = reparse(base, &e, &e.apply(text)).expect("a splice before the error");
            assert_eq!(out.errors.len(), base.errors.len());
            assert_eq!(out.errors[0].start, base.errors[0].start + 2);
        });
    }

    /// A bound `DOC_COMMENT` run lands *inside* the construct it binds to
    /// (decision #9), so it is part of the fragment and has to relex with it. Worth
    /// a case of its own: it is the one way a fragment does not begin with its own
    /// opener.
    #[test]
    fn splices_behind_a_bound_doc_comment() {
        let text = "% a note\n% a second line\n\\begin{verbatim}\n  raw\n\\end{verbatim}\n";
        assert_splices(text, after(text, "  raw", "x"));
    }

    /// Leg 3's other half, as a property of the lexer rather than of this tier: a
    /// `VERB` never exists without its closer, which is why only the body needs the
    /// termination check.
    #[test]
    fn an_unterminated_verb_is_never_a_verb_token() {
        for text in [
            "\\verb|unclosed\n",
            "\\url{unclosed\n",
            "\\lstinline|unclosed\n",
        ] {
            let toks = crate::parser::lexer::lex(text);
            assert!(
                !toks.iter().any(|t| t.kind == SyntaxKind::VERB),
                "an unterminated capture became a VERB: {text:?}",
            );
        }
    }
}
