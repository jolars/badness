//! `unused-string`: an `@string{ name = … }` macro never referenced by any field
//! value in the file.
//!
//! Built on [`Model::unused_string_defs`](crate::bib::semantic::Model::unused_string_defs).
//! A [`Severity::Warning`]; report-only (deleting the definition is a meaning-level
//! edit we leave to the author).
//!
//! **Single-file caveat:** in a multi-file bibliography a `@string` defined here may
//! be referenced from another `.bib`, so this can false-positive across files. The
//! cross-file gate (like `undefined-ref`) lands with project-level bib resolution in
//! Phase 4; until then the rule is sound for the common self-contained `.bib`, which
//! is exactly where an unused macro is worth surfacing.

use std::path::PathBuf;

use crate::linter::diagnostic::{Diagnostic, Severity};

use super::{BibRule, BibRuleContext};

pub struct UnusedString;

impl BibRule for UnusedString {
    fn id(&self) -> &'static str {
        "unused-string"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn check_file(&self, ctx: &BibRuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        for def in ctx.model.unused_string_defs() {
            sink.push(Diagnostic {
                rule: self.id(),
                severity: self.default_severity(),
                path: PathBuf::new(),
                start: usize::from(def.range.start()),
                end: usize::from(def.range.end()),
                message: format!("`@string` macro `{}` is defined but never used", def.name),
                fix: None,
                related: Vec::new(),
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
        UnusedString.check_file(&ctx, &mut out);
        out
    }

    #[test]
    fn flags_unused_macro() {
        let out = findings("@string{cup = {C}}\n@book{k, publisher = {Other}}\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "unused-string");
        assert!(out[0].message.contains("cup"));
    }

    #[test]
    fn used_macro_is_fine() {
        assert!(findings("@string{cup = {C}}\n@book{k, publisher = cup}\n").is_empty());
    }

    #[test]
    fn underlines_the_definition_name() {
        let out = findings("@string{cup = {C}}\n");
        assert_eq!(out.len(), 1);
        let start = "@string{".len();
        let end = start + "cup".len();
        assert_eq!((out[0].start, out[0].end), (start, end));
    }

    #[test]
    fn flags_each_unused_macro() {
        let out = findings("@string{a = {A}}\n@string{b = {B}}\n");
        assert_eq!(out.len(), 2);
    }
}
