//! `duplicate-field`: the same field name appearing more than once on a single
//! entry (e.g. two `author = {…}` fields).
//!
//! BibTeX and Biber keep only one occurrence of a repeated field and silently
//! discard the rest, so a duplicate is almost always a mistake — merged entries,
//! a copy-paste, or an edit that meant to replace a value but appended one. Field
//! names are matched case-insensitively (BibTeX folds case), and we flag every
//! occurrence *after* the first, pointing the caret at the repeated name.
//!
//! A [`Severity::Warning`]: it is not a parse error, and the file still loads.
//!
//! ## Autofix (only when the values are identical)
//!
//! When a duplicate's value is **byte-identical** to the kept (first) occurrence's,
//! the result is the same field regardless of which copy the engine keeps, so
//! deleting the redundant one is meaning-preserving — we offer a safe deletion via
//! [`super::edits::field_deletion_fix`]. When the values **differ**,
//! which one wins is engine/style-dependent and dropping either changes meaning,
//! so the finding is report-only.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::bib::ast::{entry_type, field_name, field_value, fields};
use crate::bib::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};
use crate::linter::diagnostic::{Diagnostic, Severity};

use super::{BibRule, BibRuleContext};

pub struct DuplicateField;

impl BibRule for DuplicateField {
    fn id(&self) -> &'static str {
        "duplicate-field"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        // Regular entries only. `@string`/`@preamble`/`@comment` are distinct kinds.
        &[SyntaxKind::ENTRY]
    }

    fn check(&self, el: &SyntaxElement, _ctx: &BibRuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(entry) = el.as_node() else {
            return;
        };
        // Name the entry type in the message when we know it; not required.
        let ty = entry_type(entry);

        // Walk fields in source order. The map keeps each name's *first* value text
        // (lowercased name → first `VALUE` text), so a later occurrence is flagged and
        // compared against the one that is kept. We never overwrite the first entry,
        // so all repeats compare against the same retained value.
        let mut first_value: HashMap<String, Option<String>> = HashMap::new();
        for field in fields(entry) {
            let Some(name) = field_name(&field) else {
                continue;
            };
            let lower = name.to_lowercase();
            // The value's content, trimmed of the surrounding trivia the CST absorbs
            // (notably the last field's value swallows the newline before the closer),
            // so identity compares the actual `{…}`/`"…"` text, not stray whitespace.
            let value = field_value(&field).map(|v| v.to_string().trim().to_string());
            let Some(kept) = first_value.get(&lower) else {
                first_value.insert(lower, value);
                continue; // First occurrence: retained.
            };

            // Identical value → deleting this redundant copy is meaning-preserving.
            let identical = matches!((kept, &value), (Some(a), Some(b)) if a == b);
            let fix = identical
                .then(|| {
                    super::edits::field_deletion_fix(
                        &field,
                        format!("remove duplicate field `{name}`"),
                    )
                })
                .flatten();

            let range = field_name_range(&field).unwrap_or_else(|| field.text_range());
            let message = match &ty {
                Some(ty) => format!("duplicate field `{name}` on `{ty}` entry"),
                None => format!("duplicate field `{name}`"),
            };
            sink.push(Diagnostic {
                rule: self.id(),
                severity: self.default_severity(),
                path: PathBuf::new(),
                start: usize::from(range.start()),
                end: usize::from(range.end()),
                message,
                fix,
                related: Vec::new(),
            });
        }
    }
}

/// The range of a `FIELD`'s `FIELD_NAME` child, for a tight caret on the name.
fn field_name_range(field: &SyntaxNode) -> Option<rowan::TextRange> {
    field
        .children()
        .find(|c| c.kind() == SyntaxKind::FIELD_NAME)
        .map(|n| n.text_range())
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
            if DuplicateField.interests().contains(&el.kind()) {
                DuplicateField.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_repeated_field() {
        let out = findings("@article{k, author = {A}, author = {B}}\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "duplicate-field");
        assert!(
            out[0].message.contains("`author`"),
            "got: {}",
            out[0].message
        );
    }

    #[test]
    fn flags_each_extra_occurrence() {
        // Three `note` fields → two findings (every occurrence after the first).
        let out = findings("@misc{k, note = {a}, note = {b}, note = {c}}\n");
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|d| d.message.contains("`note`")));
    }

    #[test]
    fn case_insensitive() {
        let out = findings("@article{k, Author = {A}, author = {B}}\n");
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn distinct_fields_are_fine() {
        assert!(findings("@article{k, author = {A}, title = {T}}\n").is_empty());
    }

    #[test]
    fn underlines_the_second_occurrence() {
        let src = "@article{k, author = {A}, author = {B}}\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        let start = src.find("author = {B}").unwrap();
        let end = start + "author".len();
        assert_eq!((out[0].start, out[0].end), (start, end));
    }

    #[test]
    fn differing_values_are_report_only() {
        // The two values disagree: which one wins is engine-dependent, so no fix.
        let out = findings("@article{k, author = {A}, author = {B}}\n");
        assert_eq!(out.len(), 1);
        assert!(out[0].fix.is_none());
    }

    #[test]
    fn identical_values_offer_a_deletion_fix() {
        let out = findings("@misc{k, note = {x}, note = {x}}\n");
        assert_eq!(out.len(), 1);
        assert!(out[0].fix.is_some());
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
    fn fix_removes_redundant_last_field() {
        let src = "@misc{k,\n  note = {x},\n  note = {x}\n}\n";
        assert_eq!(fixed(src), "@misc{k,\n  note = {x}\n}\n");
    }

    #[test]
    fn fix_removes_redundant_middle_field() {
        let src = "@misc{k,\n  note = {x},\n  note = {x},\n  year = 2020\n}\n";
        assert_eq!(fixed(src), "@misc{k,\n  note = {x},\n  year = 2020\n}\n");
    }

    #[test]
    fn differing_delimiters_are_not_identical() {
        // Same content, different delimiters: byte-different value text, so no fix.
        let out = findings("@misc{k, note = {x}, note = \"x\"}\n");
        assert_eq!(out.len(), 1);
        assert!(out[0].fix.is_none());
    }
}
