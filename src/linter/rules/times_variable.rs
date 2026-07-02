//! `times-variable`: a literal `x` used as a multiplication sign between two
//! numbers (`640x200`). Mirrors ChkTeX rule 29 ("`$\times$` may look prettier
//! here").
//!
//! Authors routinely write dimensions and products as `640x200` or `3x3`, where
//! the `x` stands in for a multiplication cross. TeX sets that `x` as an italic
//! letter, not the `\times` symbol, so it reads wrong. This rule flags such an
//! `x` and offers a fix to `\times`.
//!
//! The shape is deliberately narrow to stay out of false positives: the whole
//! `WORD` must be `digits x digits` -- exactly one lowercase `x` with a run of
//! ASCII digits on each side and nothing else. That excludes ordinary words with
//! an `x` (`matrix`, `box`), spaced products (`n x m`, three separate tokens),
//! and hex literals (`0xFF` carries a non-digit; `0x12` is excluded by the extra
//! guard that the leading number is not a lone `0`, the `0x` hex marker). The
//! rule reads only `WORD` tokens, so comments, `\verb`, and verbatim (which never
//! lex as `WORD`) are untouched.
//!
//! The fix is `Unsafe` (an intent heuristic -- a bare `x` between numbers is
//! *usually* a cross, but occasionally a real variable) and its content depends
//! on context, so it stays correct by construction (tenet 1) either way:
//!
//! - **In math mode** (`$640x200$`) the `x` becomes `\times`, valid where the
//!   letter stood.
//! - **In text mode** (`640x200`) the `x` becomes `$\times$`, wrapping the symbol
//!   in inline math so the result compiles -- exactly what ChkTeX suggests.
//!
//! Both edits are a single contiguous splice that re-parses and stays lossless.
//! Because it changes the typeset glyph, `--fix` leaves it alone; `--unsafe-fixes`
//! and the editor code action apply it.

use std::path::PathBuf;

use crate::linter::diagnostic::{Diagnostic, Fix, Severity};
use crate::syntax::{SyntaxElement, SyntaxKind};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[
    Example {
        caption: "A literal `x` as a multiplication sign in text (fixed to `$\\times$`):",
        source: "A 640x200 pixel image.\n",
    },
    Example {
        caption: "The same inside math mode (fixed to `\\times`):",
        source: "The grid is $640x200$ cells.\n",
    },
];

pub struct TimesVariable;

impl Rule for TimesVariable {
    fn id(&self) -> &'static str {
        "times-variable"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag a literal `x` used as a multiplication sign between two numbers, such \
         as `640x200` or `3x3` (ChkTeX 29). TeX sets that `x` as an italic letter \
         rather than the `\\times` cross, so it reads wrong. The rule only fires \
         when the whole word is `digits x digits` -- one lowercase `x` with ASCII \
         digits on both sides and nothing else -- so ordinary words (`matrix`), \
         spaced products (`n x m`), and hex literals (`0xFF`, `0x12`) are left \
         alone. The fix is **unsafe** (a bare `x` between numbers is usually a \
         cross but occasionally a real variable): inside math it rewrites the `x` \
         to `\\times`, and in text it wraps it as `$\\times$` so the result still \
         compiles. So `--fix` leaves it alone; `--unsafe-fixes` and the editor code \
         action apply it."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::WORD]
    }

    fn check(&self, el: &SyntaxElement, _ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(tok) = el.as_token() else {
            return;
        };
        let text = tok.text();
        let Some(xi) = times_x_position(text) else {
            return;
        };
        let base = usize::from(tok.text_range().start());
        let start = base + xi;
        let end = start + 1;

        // `\times` is only valid in math mode; wrap it in inline math otherwise so
        // the fixed text still compiles. Both are a single contiguous splice, so
        // the result re-parses and stays lossless.
        let in_math = tok
            .parent_ancestors()
            .any(|node| node.kind() == SyntaxKind::MATH);
        let content = if in_math { "\\times" } else { "$\\times$" };

        sink.push(Diagnostic {
            rule: self.id(),
            severity: self.default_severity(),
            path: PathBuf::new(),
            start,
            end,
            message:
                "literal `x` as a multiplication sign between numbers; use `\\times` for a cross"
                    .to_owned(),
            fix: Some(Fix::unsafe_(
                start,
                end,
                content,
                format!("Replace `x` with `{content}`"),
            )),
        });
    }
}

/// Byte offset of the offending `x` when `text` is exactly `digits x digits`:
/// one lowercase `x` flanked by non-empty ASCII-digit runs, with the leading run
/// not a lone `0` (the `0x` hex marker). Any other shape returns `None`, keeping
/// ordinary words, spaced products, and hex literals out of scope.
fn times_x_position(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    // Exactly one `x`, and it is the only non-digit in the word.
    let xi = bytes.iter().position(|&b| b == b'x')?;
    if bytes[xi + 1..].contains(&b'x') {
        return None;
    }
    let (before, after) = (&bytes[..xi], &bytes[xi + 1..]);
    if before.is_empty() || after.is_empty() {
        return None;
    }
    if !before.iter().all(u8::is_ascii_digit) || !after.iter().all(u8::is_ascii_digit) {
        return None;
    }
    // A lone leading `0` is the hex prefix (`0x...`), not a product.
    if before == b"0" {
        return None;
    }
    Some(xi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linter::diagnostic::Applicability;
    use crate::linter::fix::apply_fixes;
    use crate::parser::parse;
    use crate::semantic::SemanticModel;
    use crate::syntax::SyntaxNode;

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
            if TimesVariable.interests().contains(&el.kind()) {
                TimesVariable.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_times_in_text_with_unsafe_math_wrapping_fix() {
        let src = "A 640x200 image.\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "times-variable");
        // Caret on just the `x` (byte 5).
        assert_eq!((out[0].start, out[0].end), (5, 6));
        let fix = out[0].fix.as_ref().expect("a fix");
        assert_eq!(fix.applicability, Applicability::Unsafe);
        assert_eq!(fix.content, "$\\times$");
        // Unsafe: skipped without the opt-in, applied with it.
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), false).applied,
            0
        );
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), true).output,
            "A 640$\\times$200 image.\n"
        );
    }

    #[test]
    fn flags_times_in_math_with_bare_times_fix() {
        let src = "$640x200$\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        let fix = out[0].fix.as_ref().expect("a fix");
        assert_eq!(fix.content, "\\times");
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), true).output,
            "$640\\times200$\n"
        );
    }

    #[test]
    fn flags_small_product() {
        assert_eq!(findings("a 3x3 matrix\n").len(), 1);
    }

    #[test]
    fn ordinary_word_is_left_alone() {
        assert!(findings("a matrix and a box\n").is_empty());
    }

    #[test]
    fn spaced_product_is_left_alone() {
        // `n x m` is three tokens; no single `digits x digits` word.
        assert!(findings("an n x m grid\n").is_empty());
    }

    #[test]
    fn hex_literal_is_left_alone() {
        // `0xFF` carries a non-digit; `0x12` is the lone-zero hex marker.
        assert!(findings("mask 0xFF here\n").is_empty());
        assert!(findings("mask 0x12 here\n").is_empty());
    }

    #[test]
    fn uppercase_x_is_left_alone() {
        assert!(findings("a 640X200 image\n").is_empty());
    }

    #[test]
    fn missing_operand_is_left_alone() {
        assert!(findings("2x and x3 alone\n").is_empty());
    }

    #[test]
    fn trailing_punctuation_is_left_alone() {
        // `640x200.` is one WORD with a non-digit tail; conservatively skipped.
        assert!(findings("sized 640x200.\n").is_empty());
    }
}
