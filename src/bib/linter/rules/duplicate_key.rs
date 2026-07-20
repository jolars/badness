//! `duplicate-key`: a cite key used by more than one entry in the same `.bib`
//! file (the 2nd+ occurrence).
//!
//! BibTeX compares keys case-insensitively and silently keeps one of the colliding
//! entries, so this is a [`Severity::Warning`], not an error — the bib analog of
//! [`duplicate-label`](crate::linter::rules::duplicate_label). The duplicate flag is
//! already computed by the semantic model's resolve pass
//! ([`Model::duplicate_keys`](crate::bib::semantic::Model::duplicate_keys)); this
//! rule only turns the fact into a diagnostic. A cross-file branch ("also defined in
//! other.bib") is deferred to Phase 4, mirroring the LaTeX side.

use std::path::PathBuf;

use crate::linter::diagnostic::{Diagnostic, Severity};

use super::{BibRule, BibRuleContext, Example};

const EXAMPLES: &[Example] = &[Example {
    caption: "The same cite key defined by two entries:",
    source: "@misc{knuth84, title = {Draft}}\n@book{knuth84, title = {Book}}\n",
}];

pub struct DuplicateKey;

impl BibRule for DuplicateKey {
    fn id(&self) -> &'static str {
        "duplicate-key"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag a cite key defined by more than one entry in the same `.bib` \
         file. Keys are compared case-insensitively, matching BibTeX, which \
         silently keeps only one of the colliding entries; every definition \
         after the first is flagged. No autofix: resolving the collision \
         (rename vs delete) is the author's call."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn check_file(&self, ctx: &BibRuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        for entry in ctx.model.duplicate_keys() {
            sink.push(Diagnostic {
                rule: self.id(),
                severity: self.default_severity(),
                path: PathBuf::new(),
                start: usize::from(entry.key_range.start()),
                end: usize::from(entry.key_range.end()),
                message: format!("cite key `{}` is defined more than once", entry.key),
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
        DuplicateKey.check_file(&ctx, &mut out);
        out
    }

    #[test]
    fn flags_only_the_second_definition() {
        let out = findings("@misc{dup, t = {a}}\n@book{dup, t = {b}}\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "duplicate-key");
        assert!(out[0].message.contains("dup"));
    }

    #[test]
    fn flags_case_insensitively() {
        let out = findings("@misc{Key, t = {a}}\n@book{key, t = {b}}\n");
        assert_eq!(out.len(), 1);
        // Points at the second key node.
        assert_eq!(out[0].start, "@misc{Key, t = {a}}\n@book{".len());
    }

    #[test]
    fn distinct_keys_are_fine() {
        assert!(findings("@misc{a, t = {x}}\n@book{b, t = {y}}\n").is_empty());
    }

    #[test]
    fn three_definitions_flag_two() {
        assert_eq!(
            findings("@misc{k, t = {a}}\n@book{k, t = {b}}\n@misc{k, t = {c}}\n").len(),
            2
        );
    }
}
