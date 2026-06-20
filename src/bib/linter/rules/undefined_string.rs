//! `undefined-string`: an `@string` *use* that matches no definition in the file
//! (nor a predefined month macro `jan`..`dec`).
//!
//! Built on [`Model::undefined_string_uses`](crate::bib::semantic::Model::undefined_string_uses),
//! which the resolve pass already computes against in-file `@string` defs plus the
//! twelve month macros. A [`Severity::Warning`]; report-only (the fix is to define
//! the macro or correct the name, a meaning-level edit left to the author).
//!
//! **Single-file caveat:** in a multi-file bibliography the macro may be defined in
//! another `.bib`, so this can false-positive across files — the same trade-off
//! [`unused-string`](super::unused_string) carries. It is sound for the common
//! self-contained `.bib`, which is where a typo'd or undefined macro is worth
//! surfacing. A cross-file `@string` resolver is out of scope for this slice.

use std::path::PathBuf;

use crate::linter::diagnostic::{Diagnostic, Severity};

use super::{BibRule, BibRuleContext};

pub struct UndefinedString;

impl BibRule for UndefinedString {
    fn id(&self) -> &'static str {
        "undefined-string"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn check_file(&self, ctx: &BibRuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        for use_ in ctx.model.undefined_string_uses() {
            sink.push(Diagnostic {
                rule: self.id(),
                severity: self.default_severity(),
                path: PathBuf::new(),
                start: usize::from(use_.range.start()),
                end: usize::from(use_.range.end()),
                message: format!("`@string` macro `{}` is used but never defined", use_.name),
                fix: None,
            });
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
        UndefinedString.check_file(&ctx, &mut out);
        out
    }

    #[test]
    fn flags_undefined_use() {
        let out = findings("@book{k, publisher = nope}\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "undefined-string");
        assert!(out[0].message.contains("nope"));
    }

    #[test]
    fn in_file_def_is_fine() {
        assert!(findings("@string{cup = {C}}\n@book{k, publisher = cup}\n").is_empty());
    }

    #[test]
    fn month_macro_is_fine() {
        assert!(findings("@article{k, month = jan}\n").is_empty());
    }

    #[test]
    fn number_value_is_not_a_use() {
        assert!(findings("@article{k, year = 2020}\n").is_empty());
    }

    #[test]
    fn underlines_the_use() {
        let src = "@book{k, publisher = nope}\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(&src[out[0].start..out[0].end], "nope");
    }
}
