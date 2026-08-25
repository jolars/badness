//! `deprecated-suppression-syntax`: retired suppression directives carried by
//! structured BibTeX `@comment{…}` entries.
//!
//! The suppression pass retains the parsed directive and its family-name range.
//! This whole-file rule reports that fact and safely replaces only the retired
//! family name and verb, leaving the enclosing entry, selector, and reason
//! untouched.

use std::path::PathBuf;

use crate::directives::Verb;
use crate::linter::diagnostic::{Diagnostic, Fix, Severity};

use super::{BibRule, BibRuleContext, Example};

const EXAMPLES: &[Example] = &[Example {
    caption: "A retired suppression directive in a structured comment:",
    source: "@comment{badness-ignore unused-string: intentional}\n@string{x = {X}}\n",
}];

pub struct DeprecatedSuppressionSyntax;

impl BibRule for DeprecatedSuppressionSyntax {
    fn id(&self) -> &'static str {
        "deprecated-suppression-syntax"
    }

    fn description(&self) -> &'static str {
        "Flag the retired `@comment{badness-ignore <rule>}` and \
         `@comment{badness-ignore-file [<rule>]}` suppression spellings, which \
         remain accepted for compatibility but are no longer documented. The \
         Safe autofix rewrites only the family and verb to `badness-lint skip` or \
         `badness-lint skip-file`; the selector, reason, delimiters, and remaining \
         entry text stay byte-for-byte unchanged. This meta diagnostic is not \
         silenced by the retired directive it reports; use `[lint].ignore` to \
         disable the rule deliberately."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn check_file(&self, ctx: &BibRuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        for located in ctx
            .suppressions
            .directives()
            .iter()
            .filter(|located| located.directive.deprecated)
        {
            let replacement = match located.directive.verb {
                Verb::Skip => "badness-lint skip",
                Verb::SkipFile => "badness-lint skip-file",
                Verb::Off | Verb::On => continue,
            };
            let start = usize::from(located.family_range.start());
            let end = usize::from(located.family_range.end());
            sink.push(Diagnostic {
                rule: self.id(),
                severity: Severity::Warning,
                path: PathBuf::new(),
                start,
                end,
                message: format!(
                    "retired suppression syntax; use `@comment{{{replacement} …}}` instead"
                ),
                fix: Some(Fix::safe(
                    start,
                    end,
                    replacement,
                    "Rewrite the retired suppression directive",
                )),
                related: Vec::new(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bib::linter::suppression::BibSuppressionMap;
    use crate::bib::parse;
    use crate::bib::semantic::Model;
    use crate::linter::diagnostic::Applicability;
    use crate::linter::fix::apply_fixes;

    fn findings(src: &str) -> Vec<Diagnostic> {
        let root = parse(src).syntax();
        let model = Model::build(&root);
        let suppressions = BibSuppressionMap::build(&root);
        let ctx = BibRuleContext {
            path: std::path::Path::new("x.bib"),
            root: &root,
            model: &model,
            db: crate::bib::semantic::builtin(),
            suppressions: &suppressions,
        };
        let mut out = Vec::new();
        DeprecatedSuppressionSyntax.check_file(&ctx, &mut out);
        out
    }

    #[test]
    fn rewrites_node_suppression_safely() {
        let src = "@comment{badness-ignore unused-string: intentional}\n@string{x = {X}}\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(&src[out[0].start..out[0].end], "badness-ignore");
        let fix = out[0].fix.as_ref().expect("has fix");
        assert_eq!(fix.applicability, Applicability::Safe);
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), false).output,
            "@comment{badness-lint skip unused-string: intentional}\n@string{x = {X}}\n"
        );
    }

    #[test]
    fn rewrites_parenthesized_file_suppression() {
        let src = "@comment(badness-ignore-file: imported)\n@misc{k}\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        let fix = out[0].fix.as_ref().expect("has fix");
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), false).output,
            "@comment(badness-lint skip-file: imported)\n@misc{k}\n"
        );
    }

    #[test]
    fn modern_and_ordinary_comment_entries_are_fine() {
        assert!(findings("@comment{badness-lint skip unused-string}\n").is_empty());
        assert!(findings("@comment{ordinary note}\n").is_empty());
    }
}
