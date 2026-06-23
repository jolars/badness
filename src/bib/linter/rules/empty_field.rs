//! `empty-field`: a field whose value is empty or only whitespace — `title = {}`,
//! `note = ""`, `author = {  }`.
//!
//! An empty field is dead weight: it carries no data and some styles still emit
//! punctuation for it. A [`Severity::Warning`] with a **safe deletion autofix** that
//! removes the field and its one separating comma.
//!
//! "Empty" means the `VALUE` holds no macro/number piece ([`LITERAL`]) and every
//! brace/quote group is content-empty (only its delimiters and trivia). A nested
//! group or any value token counts as content, so the check never flags a field
//! that carries data.
//!
//! ## Autofix
//!
//! A safe deletion via [`super::edits::field_deletion_fix`], which removes the field
//! and its one separating comma from real CST byte ranges (correct by
//! construction: parses + lossless). It does not withhold to preserve `=`
//! alignment — re-padding is the formatter's job (tenet 1, fix-then-format).
//!
//! [`LITERAL`]: crate::bib::syntax::SyntaxKind::LITERAL

use std::path::PathBuf;

use rowan::NodeOrToken;

use crate::bib::ast::{field_name, field_value};
use crate::bib::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};
use crate::linter::diagnostic::{Diagnostic, Severity};

use super::{BibRule, BibRuleContext};

pub struct EmptyField;

impl BibRule for EmptyField {
    fn id(&self) -> &'static str {
        "empty-field"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::FIELD]
    }

    fn check(&self, el: &SyntaxElement, _ctx: &BibRuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(field) = el.as_node() else {
            return;
        };
        // A field carries a name; without one this is a recovery artifact, not an
        // empty field. (`@string` lives in STRING_ENTRY, a distinct kind, so its
        // `name = value` FIELD is not visited here.)
        let Some(name) = field_name(field) else {
            return;
        };

        let empty = match field_value(field) {
            Some(value) => value_is_empty(&value),
            // No value node: only an `author =` with a present `=` and nothing after
            // counts as an (empty) field rather than a stray name.
            None => has_eq(field),
        };
        if !empty {
            return;
        }

        let range = field.text_range();
        sink.push(Diagnostic {
            rule: self.id(),
            severity: self.default_severity(),
            path: PathBuf::new(),
            start: usize::from(range.start()),
            end: usize::from(range.end()),
            message: format!("field `{name}` is empty"),
            fix: super::edits::field_deletion_fix(field, format!("remove empty field `{name}`")),
        });
    }
}

/// Whether `field` contains an `=` token (a real assignment).
fn has_eq(field: &SyntaxNode) -> bool {
    field
        .children_with_tokens()
        .any(|c| c.kind() == SyntaxKind::EQ)
}

/// Whether a `VALUE` node carries no content: no `LITERAL` piece, and every
/// `BRACE_GROUP`/`QUOTED` is content-empty.
fn value_is_empty(value: &SyntaxNode) -> bool {
    for child in value.children() {
        match child.kind() {
            SyntaxKind::LITERAL => return false, // a macro or number is content
            SyntaxKind::BRACE_GROUP | SyntaxKind::QUOTED if !delimited_is_empty(&child) => {
                return false;
            }
            _ => {}
        }
    }
    true
}

/// Whether a `BRACE_GROUP`/`QUOTED` holds only its delimiters and trivia — no value
/// tokens and no nested groups.
fn delimited_is_empty(node: &SyntaxNode) -> bool {
    node.children_with_tokens().all(|child| match child {
        NodeOrToken::Node(_) => false, // a nested group is content
        NodeOrToken::Token(t) => matches!(
            t.kind(),
            SyntaxKind::L_BRACE
                | SyntaxKind::R_BRACE
                | SyntaxKind::QUOTE
                | SyntaxKind::WHITESPACE
                | SyntaxKind::NEWLINE
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bib::parse;
    use crate::bib::semantic::Model;

    fn findings(src: &str) -> Vec<Diagnostic> {
        let root = parse(src).syntax();
        let model = Model::build(&root);
        let ctx = BibRuleContext {
            path: std::path::Path::new("x.bib"),
            root: &root,
            model: &model,
            db: crate::bib::semantic::builtin(),
        };
        let mut out = Vec::new();
        for el in root.descendants_with_tokens() {
            if EmptyField.interests().contains(&el.kind()) {
                EmptyField.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_empty_braces() {
        let out = findings("@misc{k, title = {}}\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "empty-field");
        assert!(out[0].message.contains("title"));
    }

    #[test]
    fn flags_empty_quotes() {
        assert_eq!(findings("@misc{k, note = \"\"}\n").len(), 1);
    }

    #[test]
    fn flags_whitespace_only_braces() {
        assert_eq!(findings("@misc{k, author = {   }}\n").len(), 1);
    }

    #[test]
    fn non_empty_brace_is_fine() {
        assert!(findings("@misc{k, title = {Hi}}\n").is_empty());
    }

    #[test]
    fn macro_value_is_not_empty() {
        assert!(findings("@misc{k, title = sometext}\n").is_empty());
    }

    #[test]
    fn number_value_is_not_empty() {
        assert!(findings("@misc{k, year = 2020}\n").is_empty());
    }

    #[test]
    fn underlines_the_field() {
        let out = findings("@misc{k, title = {}}\n");
        assert_eq!(out.len(), 1);
        let start = "@misc{k, ".len();
        let end = start + "title = {}".len();
        assert_eq!((out[0].start, out[0].end), (start, end));
    }

    /// Apply the single finding's fix to `src` and return the result.
    fn fixed(src: &str) -> String {
        let out = findings(src);
        assert_eq!(out.len(), 1, "expected exactly one finding");
        let fix = out[0].fix.as_ref().expect("a fix");
        let mut s = src.to_string();
        s.replace_range(fix.start..fix.end, &fix.content);
        s
    }

    #[test]
    fn fix_deletes_only_field_to_fieldless_form() {
        assert_eq!(fixed("@misc{k,\n  note = {}\n}\n"), "@misc{k}\n");
    }

    #[test]
    fn fix_deletes_last_field_and_prior_comma() {
        let src = "@misc{k,\n  title = {T},\n  note  = {}\n}\n";
        assert_eq!(fixed(src), "@misc{k,\n  title = {T}\n}\n");
    }

    #[test]
    fn fix_deletes_middle_field_and_its_comma() {
        let src = "@misc{k,\n  title = {T},\n  note  = {},\n  year  = 2020\n}\n";
        assert_eq!(fixed(src), "@misc{k,\n  title = {T},\n  year  = 2020\n}\n");
    }

    #[test]
    fn fix_offered_when_field_is_unique_longest() {
        // `annotation` is strictly the longest name, so deleting it leaves the kept
        // field over-padded. The fix is still offered: the deletion is a correct
        // byte-range edit (parses + lossless), and re-padding `=` is the formatter's
        // job, not the fixer's (tenet 1, fix-then-format).
        let src = "@misc{k,\n  a          = {x},\n  annotation = {}\n}\n";
        assert_eq!(fixed(src), "@misc{k,\n  a          = {x}\n}\n");
    }

    #[test]
    fn fix_offered_when_tied_for_longest() {
        // Two names of equal length: deleting one leaves the width unchanged.
        let src = "@misc{k,\n  aaa = {x},\n  bbb = {}\n}\n";
        assert_eq!(fixed(src), "@misc{k,\n  aaa = {x}\n}\n");
    }
}
