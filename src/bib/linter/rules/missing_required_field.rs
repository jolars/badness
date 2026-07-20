//! `missing-required-field`: a regular entry lacking a field its type requires,
//! per the built-in signature DB (`data/bib_fields.json`).
//!
//! Only entry types the DB knows are checked — an unknown type carries no
//! signature, so we make no claim (no false positives on custom `@`-types). A
//! [`RequiredField::OneOf`] alternation (e.g. `date` *or* `year`) is satisfied by
//! any one alternative. This is a [`Severity::Warning`]: a missing field degrades
//! the bibliography but is not a parse error, and styles vary in what they enforce.
//! Report-only — we cannot invent field content.

use std::collections::HashSet;
use std::path::PathBuf;

use smol_str::SmolStr;

use crate::bib::ast::{cite_key, entry_type, field_name, fields};
use crate::bib::semantic::RequiredField;
use crate::bib::syntax::{SyntaxElement, SyntaxKind};
use crate::linter::diagnostic::{Diagnostic, Severity};

use super::{BibRule, BibRuleContext, Example};

const EXAMPLES: &[Example] = &[Example {
    caption: "An `@article` without its required `journaltitle`:",
    source: "@article{doe2020,\n  author = {Doe, Jane},\n  title  = {A study},\n  \
             year   = 2020\n}\n",
}];

pub struct MissingRequiredField;

impl BibRule for MissingRequiredField {
    fn id(&self) -> &'static str {
        "missing-required-field"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag a regular entry lacking a field its type requires, per the \
         biblatex data model. An alternation like `date` *or* `year` is \
         satisfied by either, and classic-BibTeX aliases count (`journal` \
         satisfies `journaltitle`). An entry type the built-in database does \
         not know carries no signature and is never flagged. Report-only -- \
         field content cannot be invented."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        // Regular entries only. `@string`/`@preamble`/`@comment` are distinct kinds,
        // so they are excluded without a guard.
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

        // The entry's field names, canonicalized (lowercased, classic-BibTeX aliases
        // resolved to their BibLaTeX field, e.g. `journal` -> `journaltitle`), so a
        // required field is satisfied regardless of which spelling the entry uses.
        let present: HashSet<SmolStr> = fields(entry)
            .filter_map(|f| field_name(&f))
            .map(|n| ctx.db.canonical(&n))
            .collect();

        // Underline the cite key; fall back to the whole entry on a keyless recovery.
        let range = cite_key(entry)
            .map(|(_, r)| r)
            .unwrap_or_else(|| entry.text_range());

        for req in &sig.required {
            let missing_message = match req {
                RequiredField::One(name) => (!present.contains(&ctx.db.canonical(name)))
                    .then(|| format!("entry `{ty}` is missing required field `{name}`")),
                RequiredField::OneOf(alts) => {
                    let satisfied = alts.iter().any(|a| present.contains(&ctx.db.canonical(a)));
                    (!satisfied).then(|| {
                        let names = alts
                            .iter()
                            .map(|a| format!("`{a}`"))
                            .collect::<Vec<_>>()
                            .join(" or ");
                        format!("entry `{ty}` is missing a required field ({names})")
                    })
                }
            };
            if let Some(message) = missing_message {
                sink.push(Diagnostic {
                    rule: self.id(),
                    severity: self.default_severity(),
                    path: PathBuf::new(),
                    start: usize::from(range.start()),
                    end: usize::from(range.end()),
                    message,
                    fix: None,
                    related: Vec::new(),
                });
            }
        }
    }
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
            if MissingRequiredField.interests().contains(&el.kind()) {
                MissingRequiredField.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_missing_single_required_field() {
        // `@article` requires author, title, journaltitle, and date-or-year. Here
        // only title is present.
        let out = findings("@article{k, title = {T}}\n");
        let msgs: Vec<&str> = out.iter().map(|d| d.message.as_str()).collect();
        assert!(msgs.iter().any(|m| m.contains("`author`")), "got: {msgs:?}");
        assert!(out.iter().all(|d| d.rule == "missing-required-field"));
    }

    #[test]
    fn oneof_satisfied_by_year() {
        // `date` OR `year`: providing `year` satisfies the alternation, so no
        // date/year finding (other required fields may still be flagged).
        let out =
            findings("@article{k, author = {A}, title = {T}, journaltitle = {J}, year = 2020}\n");
        assert!(
            out.is_empty(),
            "fully-specified article should be clean, got: {:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn oneof_missing_is_one_finding() {
        // author, title, journaltitle present; neither date nor year.
        let out = findings("@article{k, author = {A}, title = {T}, journaltitle = {J}}\n");
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains("date") && out[0].message.contains("year"));
    }

    #[test]
    fn classic_bibtex_journal_alias_satisfies_journaltitle() {
        // `@article` requires the data-model field `journaltitle`; classic BibTeX
        // spells it `journal` (a biber-resolved alias). The alias must satisfy the
        // requirement rather than triggering a false "missing journaltitle".
        let out = findings("@article{k, author = {A}, title = {T}, journal = {J}, year = 2020}\n");
        assert!(
            out.is_empty(),
            "journal alias should satisfy journaltitle, got: {:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn canonical_field_satisfies_classic_required_alias() {
        // The reverse direction: `@mastersthesis` keeps the classic required field
        // `school`; supplying its canonical BibLaTeX form `institution` must satisfy
        // it, since the two are aliases.
        let out = findings(
            "@mastersthesis{k, author = {A}, title = {T}, institution = {U}, year = 2020}\n",
        );
        assert!(
            out.iter().all(|d| !d.message.contains("school")),
            "institution should satisfy the school requirement, got: {:?}",
            out.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn unknown_entry_type_is_skipped() {
        assert!(findings("@frobnicate{k, wat = {x}}\n").is_empty());
    }

    #[test]
    fn underlines_the_cite_key() {
        let out = findings("@article{mykey, title = {T}}\n");
        let start = "@article{".len();
        let end = start + "mykey".len();
        assert!(
            out.iter().all(|d| (d.start, d.end) == (start, end)),
            "expected all findings at the key range, got: {:?}",
            out.iter().map(|d| (d.start, d.end)).collect::<Vec<_>>()
        );
    }
}
