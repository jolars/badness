//! `unknown-field`: a field on a regular entry that is neither required nor
//! optional for that entry type, and carries no global field metadata.
//!
//! Checked only for entry types the DB knows (an unknown type makes no claim about
//! its fields). "Acceptable" is the union of the type's required-field names (both
//! single and `OneOf` alternatives) and its optional fields, widened by any field
//! the DB categorizes globally ([`BibFieldDb::field`]) — so a common cross-cutting
//! field (e.g. a `Verbatim` `url`) is not flagged merely because a given entry
//! type's optional list omits it. A [`Severity::Warning`]: BibLaTeX silently
//! ignores unknown fields, but they are usually typos or misplaced data.
//! Report-only — deleting a field is meaning-changing.
//!
//! [`BibFieldDb::field`]: crate::bib::semantic::BibFieldDb::field

use std::collections::HashSet;
use std::path::PathBuf;

use crate::bib::ast::{entry_type, field_name, fields};
use crate::bib::semantic::RequiredField;
use crate::bib::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};
use crate::linter::diagnostic::{Diagnostic, Severity};

use super::{BibRule, BibRuleContext};

pub struct UnknownField;

impl BibRule for UnknownField {
    fn id(&self) -> &'static str {
        "unknown-field"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::ENTRY]
    }

    fn check(&self, el: &SyntaxElement, ctx: &BibRuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(entry) = el.as_node() else {
            return;
        };
        let Some(ty) = entry_type(entry) else {
            return;
        };
        let Some(sig) = ctx.db.entry(&ty) else {
            return; // Unknown entry type: no signature, no claim.
        };

        // Names this entry type accepts: required (single + alternatives) ∪ optional.
        let mut accepted: HashSet<&str> = HashSet::new();
        for req in &sig.required {
            match req {
                RequiredField::One(name) => {
                    accepted.insert(name.as_str());
                }
                RequiredField::OneOf(alts) => accepted.extend(alts.iter().map(|a| a.as_str())),
            }
        }
        accepted.extend(sig.optional.iter().map(|o| o.as_str()));

        for field in fields(entry) {
            let Some(name) = field_name(&field) else {
                continue;
            };
            let lower = name.to_lowercase();
            // Accepted by the entry type, or carrying global field metadata.
            if accepted.contains(lower.as_str()) || ctx.db.field(&lower).is_some() {
                continue;
            }
            let range = field_name_range(&field).unwrap_or_else(|| field.text_range());
            sink.push(Diagnostic {
                rule: self.id(),
                severity: self.default_severity(),
                path: PathBuf::new(),
                start: usize::from(range.start()),
                end: usize::from(range.end()),
                message: format!("unknown field `{name}` on `{ty}` entry"),
                fix: None,
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
            if UnknownField.interests().contains(&el.kind()) {
                UnknownField.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_unknown_field() {
        let out = findings("@article{k, title = {T}, frobnozzle = {x}}\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "unknown-field");
        assert!(
            out[0].message.contains("frobnozzle"),
            "got: {}",
            out[0].message
        );
    }

    #[test]
    fn known_optional_field_is_fine() {
        // `note` is an accepted optional field on most entry types.
        assert!(findings("@article{k, title = {T}, note = {n}}\n").is_empty());
    }

    #[test]
    fn globally_categorized_field_is_fine() {
        // `url` carries global field metadata, so it is not flagged even if absent
        // from a given type's optional list.
        assert!(findings("@misc{k, title = {T}, url = {http://x}}\n").is_empty());
    }

    #[test]
    fn standard_biblatex_fields_on_article_are_fine() {
        // Standard BibLaTeX fields absent from `article`'s optional list but valid
        // everywhere (manual §2.2): they carry global metadata and must not flag.
        let src = "@article{k, title = {T}, langid = {english}, \
                   publisher = {P}, shortjournal = {SJ}, shorttitle = {ST}}\n";
        assert!(findings(src).is_empty(), "got: {:?}", findings(src));
    }

    #[test]
    fn unknown_entry_type_is_skipped() {
        assert!(findings("@frobnicate{k, wat = {x}}\n").is_empty());
    }

    #[test]
    fn underlines_the_field_name() {
        let out = findings("@article{k, frobnozzle = {x}}\n");
        assert_eq!(out.len(), 1);
        let start = "@article{k, ".len();
        let end = start + "frobnozzle".len();
        assert_eq!((out[0].start, out[0].end), (start, end));
    }
}
