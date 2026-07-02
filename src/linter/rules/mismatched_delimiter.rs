//! `mismatched-delimiter`: a `\left … \right` pair whose delimiter glyphs point
//! the wrong way — an opening glyph after `\right`, or a closing glyph after
//! `\left`.
//!
//! The parser already reports *structural* delimiter faults (a missing `\right`,
//! a stray `\right`, a missing delimiter token) on its error channel; what it
//! deliberately tolerates is a *balanced but mismatched* pair, because that is
//! exactly how TeX counts them (`grammar.rs`, `left_right`). Most such pairs are
//! intentional — half-open intervals `\left( … \right]`, a null `\left. …
//! \right)` — so this rule is intentionally conservative: it flags only an
//! *orientation* error (a `Closer` glyph opening the pair, or an `Opener` glyph
//! closing it), never a mere opener/closer mismatch. That catches genuine
//! copy-paste slips like `\left) … \right(` with near-zero false positives.
//!
//! Reads only the static glyph text the lexer has already isolated into a single
//! token (AGENTS.md decision #1); no math meaning is resolved.

use std::path::PathBuf;

use rowan::NodeOrToken;

use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken};

use crate::linter::diagnostic::{Diagnostic, Severity};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[Example {
    caption: "A `\\left`/`\\right` pair whose glyphs point the wrong way:",
    source: "$\\left) x \\right($\n",
}];

/// Glyphs that conventionally open a delimited pair.
const OPENERS: &[&str] = &[
    "(", "[", "\\{", "\\lbrace", "\\lbrack", "\\langle", "\\lceil", "\\lfloor", "\\lgroup",
    "\\lvert", "\\lVert",
];

/// Glyphs that conventionally close a delimited pair.
const CLOSERS: &[&str] = &[
    ")", "]", "\\}", "\\rbrace", "\\rbrack", "\\rangle", "\\rceil", "\\rfloor", "\\rgroup",
    "\\rvert", "\\rVert",
];

pub struct MismatchedDelimiter;

impl Rule for MismatchedDelimiter {
    fn id(&self) -> &'static str {
        "mismatched-delimiter"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag a `\\left ... \\right` pair whose delimiter glyphs point the wrong \
         way -- a closing glyph opening the pair, or an opening glyph closing it \
         (`\\left) ... \\right(`). Deliberately conservative: only an \
         *orientation* error is flagged, never a mere opener/closer mismatch, \
         since half-open intervals like `\\left( ... \\right]` are legitimate. \
         Structural faults (a missing `\\right`) are reported by the parser, not \
         this rule. No autofix: the intended glyphs are ambiguous."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::LEFT_RIGHT]
    }

    fn check(&self, el: &SyntaxElement, _ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(pair) = el.as_node() else {
            return;
        };
        let (open, close) = delimiters(pair);
        // Only judge a fully-formed pair: a missing delimiter is already a parser
        // error, so don't pile on.
        let (Some(open), Some(close)) = (open, close) else {
            return;
        };

        if CLOSERS.contains(&open.text()) {
            sink.push(orientation_diag(
                self,
                &open,
                format!(
                    "`\\left{}` uses a closing delimiter where an opening one is expected",
                    open.text()
                ),
            ));
        }
        if OPENERS.contains(&close.text()) {
            sink.push(orientation_diag(
                self,
                &close,
                format!(
                    "`\\right{}` uses an opening delimiter where a closing one is expected",
                    close.text()
                ),
            ));
        }
    }
}

fn orientation_diag(
    rule: &MismatchedDelimiter,
    delim: &SyntaxToken,
    message: String,
) -> Diagnostic {
    let range = delim.text_range();
    Diagnostic {
        rule: rule.id(),
        severity: rule.default_severity(),
        path: PathBuf::new(),
        start: usize::from(range.start()),
        end: usize::from(range.end()),
        message,
        fix: None,
    }
}

/// The opening and closing delimiter tokens of a `LEFT_RIGHT` node: the first
/// non-trivia token after the `\left` marker, and likewise after `\right`. Either
/// is `None` when the parser found the delimiter missing (the next element is the
/// `MATH` body or the closing marker, a node or marker rather than a glyph).
fn delimiters(pair: &SyntaxNode) -> (Option<SyntaxToken>, Option<SyntaxToken>) {
    let mut open = None;
    let mut close = None;
    let mut pending: Option<bool> = None; // Some(true) after `\left`, Some(false) after `\right`
    for element in pair.children_with_tokens() {
        match element {
            NodeOrToken::Token(token) => {
                if is_trivia(token.kind()) {
                    continue;
                }
                match token.text() {
                    "\\left" => pending = Some(true),
                    "\\right" => pending = Some(false),
                    _ => match pending.take() {
                        Some(true) => open = Some(token),
                        Some(false) => close = Some(token),
                        None => {}
                    },
                }
            }
            // The MATH body (or any node) ends the run after a marker: the
            // delimiter token was missing.
            NodeOrToken::Node(_) => pending = None,
        }
    }
    (open, close)
}

fn is_trivia(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use crate::semantic::SemanticModel;

    fn findings(src: &str) -> Vec<Diagnostic> {
        let root = SyntaxNode::new_root(parse(src).green);
        let model = SemanticModel::build(&root);
        let ctx = RuleContext {
            path: std::path::Path::new("x.tex"),
            root: &root,
            model: &model,
            resolution: None,
            citations: None,
        };
        let mut out = Vec::new();
        for el in root.descendants_with_tokens() {
            if MismatchedDelimiter.interests().contains(&el.kind()) {
                MismatchedDelimiter.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_closer_opening_the_pair() {
        let out = findings("$\\left) a \\right| $\n");
        assert_eq!(out.len(), 1, "got: {out:?}");
        assert_eq!(out[0].rule, "mismatched-delimiter");
        assert!(
            out[0].message.contains("\\left)"),
            "got: {}",
            out[0].message
        );
    }

    #[test]
    fn flags_opener_closing_the_pair() {
        let out = findings("$\\left| a \\right( $\n");
        assert_eq!(out.len(), 1, "got: {out:?}");
        assert!(
            out[0].message.contains("\\right("),
            "got: {}",
            out[0].message
        );
    }

    #[test]
    fn flags_both_ends_when_both_reversed() {
        let out = findings("$\\left) a \\right( $\n");
        assert_eq!(out.len(), 2, "got: {out:?}");
    }

    #[test]
    fn matched_pair_is_fine() {
        assert!(findings("$\\left( a \\right) $\n").is_empty());
    }

    #[test]
    fn half_open_interval_is_fine() {
        // `\left( … \right]` is a legitimate half-open interval, not an
        // orientation error.
        assert!(findings("$\\left( a \\right] $\n").is_empty());
    }

    #[test]
    fn null_delimiter_is_fine() {
        assert!(findings("$\\left. a \\right) $\n").is_empty());
    }
}
