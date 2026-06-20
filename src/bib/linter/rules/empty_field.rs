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
//! ## Autofix (format-clean by construction, Tenet 5)
//!
//! The fix is a single contiguous deletion computed from real CST byte ranges, so
//! it works on messy input too, and on already-*formatted* input it yields
//! formatted output (so `format → lint --fix → format --check` stays green):
//! - **only field:** delete from the key's end to the closing delimiter, collapsing
//!   `@misc{k,\n  note = {}\n}` to the formatter's fieldless form `@misc{k}`;
//! - **last field:** delete from the previous field's end through this field
//!   (dropping the previous field's now-trailing comma);
//! - **otherwise:** delete from this field's start to the next field's start
//!   (dropping this field's comma and the line break before the next field).
//!
//! It is **withheld** (finding still reported) when removing the field would change
//! the entry's `=` alignment — i.e. this field's name is the strict-unique longest —
//! since the formatter re-pads every sibling then, which a single contiguous edit
//! cannot express.
//!
//! [`LITERAL`]: crate::bib::syntax::SyntaxKind::LITERAL

use std::path::PathBuf;

use rowan::NodeOrToken;

use crate::bib::ast::{cite_key, field_name, field_value, fields};
use crate::bib::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};
use crate::linter::diagnostic::{Diagnostic, Fix, Severity};

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
            fix: deletion_fix(field, &name),
        });
    }
}

/// Build the safe deletion [`Fix`] for an empty `field`, or `None` when it should be
/// withheld (see the module docs). `name` is the field's name, for the description.
fn deletion_fix(field: &SyntaxNode, name: &str) -> Option<Fix> {
    let entry = field.parent()?;
    if entry.kind() != SyntaxKind::ENTRY {
        // `@string`'s `name = value` lives in STRING_ENTRY; not an entry field.
        return None;
    }
    let siblings: Vec<SyntaxNode> = fields(&entry).collect();
    let index = siblings.iter().position(|f| f == field)?;

    // Withhold when removing this field would change the `=` alignment: if its name
    // is the strict-unique longest, every sibling's padding (and any wrapped value's
    // hanging indent) shifts, which a single contiguous edit cannot express.
    let this_len = name.chars().count();
    let others_max = siblings
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != index)
        .filter_map(|(_, f)| field_name(f))
        .map(|n| n.to_lowercase().chars().count())
        .max();
    if let Some(max) = others_max
        && this_len > max
    {
        return None;
    }

    let description = format!("remove empty field `{name}`");
    let (start, end) = if siblings.len() == 1 {
        // Only field: collapse to the fieldless single-line form `@type{key}`. Delete
        // from the key's end (dropping the post-key comma) to the closing delimiter.
        let (_, key_range) = cite_key(&entry)?;
        let close = entry
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| matches!(t.kind(), SyntaxKind::R_BRACE | SyntaxKind::R_PAREN))
            .last()?;
        (
            usize::from(key_range.end()),
            usize::from(close.text_range().start()),
        )
    } else if index + 1 == siblings.len() {
        // Last field: delete from the previous field's end through this field, which
        // drops the previous field's now-trailing comma. Stop at the field's last
        // non-trivia token: a last field's value absorbs the trailing newline before
        // the closer, which must survive so the `}` stays on its own line.
        let prev = &siblings[index - 1];
        (usize::from(prev.text_range().end()), content_end(field))
    } else {
        // Otherwise: delete from this field's start to the next field's start, which
        // drops this field's comma and the line break before the next field.
        let next = &siblings[index + 1];
        (
            usize::from(field.text_range().start()),
            usize::from(next.text_range().start()),
        )
    };
    Some(Fix::safe(start, end, "", description))
}

/// The byte offset just past the field's last non-trivia token, excluding any
/// trailing whitespace/newline the value absorbed.
fn content_end(field: &SyntaxNode) -> usize {
    field
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !matches!(t.kind(), SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE))
        .map(|t| usize::from(t.text_range().end()))
        .max()
        .unwrap_or_else(|| usize::from(field.text_range().end()))
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
    fn fix_withheld_when_field_is_unique_longest() {
        // `annotation` is strictly the longest name; deleting it would re-pad the
        // others, so the fix is withheld (the finding still stands).
        let out = findings("@misc{k,\n  a          = {x},\n  annotation = {}\n}\n");
        assert_eq!(out.len(), 1);
        assert!(out[0].fix.is_none());
    }

    #[test]
    fn fix_offered_when_tied_for_longest() {
        // Two names of equal length: deleting one leaves the width unchanged.
        let src = "@misc{k,\n  aaa = {x},\n  bbb = {}\n}\n";
        assert_eq!(fixed(src), "@misc{k,\n  aaa = {x}\n}\n");
    }
}
