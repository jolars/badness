//! `deprecated-suppression-syntax`: retired `% badness-ignore` suppression
//! directives that remain accepted for compatibility.
//!
//! The parser-side suppression pass already identifies directives and retains
//! their carrier and family-name ranges. This whole-file rule turns that fact
//! into a warning and a `Safe` fix over the family name alone. Replacing
//! `badness-ignore` with `badness-lint skip`, or `badness-ignore-file` with
//! `badness-lint skip-file`, preserves the selector and reason verbatim and
//! changes no non-comment token.

use std::path::PathBuf;

use crate::directives::Verb;
use crate::linter::diagnostic::{Diagnostic, Fix, Severity};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[Example {
    caption: "A retired suppression directive:",
    source: "% badness-ignore deprecated-command: legacy source\n{\\bf text}\n",
}];

pub struct DeprecatedSuppressionSyntax;

impl Rule for DeprecatedSuppressionSyntax {
    fn id(&self) -> &'static str {
        "deprecated-suppression-syntax"
    }

    fn description(&self) -> &'static str {
        "Flag the retired `% badness-ignore <rule>` and `% badness-ignore-file \
         [<rule>]` suppression spellings, which remain accepted for compatibility \
         but are no longer documented. The Safe autofix rewrites only the family \
         and verb to `% badness-lint skip <rule>` or `% badness-lint skip-file \
         [<rule>]`; the selector and reason remain byte-for-byte unchanged, and \
         the edit stays entirely inside a comment. This meta diagnostic is not \
         silenced by the retired directive it reports; use `[lint].ignore` to \
         disable the rule deliberately."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn emits_fix(&self) -> bool {
        true
    }

    fn check_file(&self, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
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
                message: format!("retired suppression syntax; use `% {replacement}` instead"),
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
    use crate::linter::diagnostic::Applicability;
    use crate::linter::fix::apply_fixes;
    use crate::parser::parse;
    use crate::semantic::SemanticModel;
    use crate::syntax::SyntaxNode;

    fn findings(src: &str) -> Vec<Diagnostic> {
        let root = SyntaxNode::new_root(parse(src).green);
        let model = SemanticModel::build(&root);
        let ctx = RuleContext::new(
            std::path::Path::new("x.tex"),
            &root,
            &model,
            None,
            None,
            None,
        );
        let mut out = Vec::new();
        DeprecatedSuppressionSyntax.check_file(&ctx, &mut out);
        out
    }

    #[test]
    fn rewrites_node_suppression_safely() {
        let src = "% badness-ignore deprecated-command: legacy\n{\\bf text}\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "deprecated-suppression-syntax");
        assert_eq!(&src[out[0].start..out[0].end], "badness-ignore");
        let fix = out[0].fix.as_ref().expect("has fix");
        assert_eq!(fix.applicability, Applicability::Safe);
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), false).output,
            "% badness-lint skip deprecated-command: legacy\n{\\bf text}\n"
        );
    }

    #[test]
    fn rewrites_file_suppression_and_preserves_reason() {
        let src = "%%% badness-ignore-file: generated file\ntext\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        let fix = out[0].fix.as_ref().expect("has fix");
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), false).output,
            "%%% badness-lint skip-file: generated file\ntext\n"
        );
    }

    #[test]
    fn modern_and_ordinary_comments_are_fine() {
        assert!(findings("% badness-lint skip deprecated-command\n\\bf\n").is_empty());
        assert!(findings("% badness is good\n").is_empty());
    }
}
