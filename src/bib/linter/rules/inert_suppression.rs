//! `inert-suppression`: a BibTeX suppression directive that cannot
//! take effect, or an `off` region that reaches EOF without its closer.
//!
//! The BibTeX suppression map retains the placement outcome while resolving
//! ranges. This whole-file rule reports that fact without re-parsing comments.

use std::path::PathBuf;

use crate::directives::DirectiveOutcome;
use crate::linter::diagnostic::Diagnostic;

use super::{BibRule, BibRuleContext, Example};

const EXAMPLES: &[Example] = &[Example {
    caption: "A formatting region needs a matching opener:",
    source: "@comment{badness-format on}\n@book{key}\n",
}];

pub struct InertSuppression;

impl BibRule for InertSuppression {
    fn id(&self) -> &'static str {
        "inert-suppression"
    }

    fn description(&self) -> &'static str {
        "Flag a BibTeX suppression directive that cannot take effect, such as \
         `skip` without a target or `on` without a matching `off`. Within a regular \
         entry, use `% badness-lint skip` to suppress lint on one field. Use an \
         `@comment` entry for region, file-wide, and formatting directives. An `off` \
         left open through the end of the file still suppresses that span and \
         triggers this diagnostic. Inline suppressions cannot hide this diagnostic. \
         Use `[lint].ignore` to disable it."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn check_file(&self, ctx: &BibRuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        for located in ctx
            .suppressions
            .directives()
            .iter()
            .filter(|located| located.outcome != DirectiveOutcome::Honored)
        {
            let message = match located.outcome {
                DirectiveOutcome::Honored => continue,
                DirectiveOutcome::DanglingSkip if located.is_line_comment => {
                    "`skip` has no field to suppress"
                }
                DirectiveOutcome::DanglingSkip => {
                    "`skip` has no following entry, so this directive does nothing"
                }
                DirectiveOutcome::UnmatchedOn => {
                    "`on` has no matching `off`, so this directive closes no region"
                }
                DirectiveOutcome::UnclosedOff => {
                    "`off` reaches the end of the file without a matching `on`"
                }
                DirectiveOutcome::Unsupported => {
                    "use `% badness-lint skip` for a field or `@comment{…}` for other BibTeX directives"
                }
            };
            sink.push(Diagnostic {
                rule: self.id(),
                severity: self.default_severity(),
                path: PathBuf::new(),
                start: usize::from(located.range.start()),
                end: usize::from(located.range.end()),
                message: message.to_owned(),
                fix: None,
                related: Vec::new(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bib::linter::rules::BibRuleContext;
    use crate::bib::linter::suppression::BibSuppressionMap;
    use crate::bib::parse;
    use crate::bib::semantic::Model;
    use crate::linter::diagnostic::Diagnostic;

    fn findings(src: &str) -> Vec<Diagnostic> {
        let root = parse(src).syntax();
        assert_eq!(root.to_string(), src);
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
        InertSuppression.check_file(&ctx, &mut out);
        out
    }

    #[test]
    fn reports_each_inert_or_incomplete_shape() {
        for (src, message) in [
            (
                "@comment{badness-lint skip missing-required-field}\n",
                "no following entry",
            ),
            (
                "@comment{badness-lint on missing-required-field}\n",
                "no matching `off`",
            ),
            (
                "@comment{badness-lint off missing-required-field}\n@book{a}\n",
                "without a matching `on`",
            ),
            (
                "@comment{badness-format on}\n@book{a}\n",
                "no matching `off`",
            ),
        ] {
            let out = findings(src);
            assert_eq!(out.len(), 1, "{src:?}: {out:?}");
            assert_eq!(out[0].rule, "inert-suppression");
            assert_eq!(&src[out[0].start..out[0].end], src.lines().next().unwrap());
            assert!(out[0].message.contains(message), "{:?}", out[0].message);
            assert!(out[0].fix.is_none());
        }
    }

    #[test]
    fn accepts_honored_lint_and_combined_directives() {
        for src in [
            "@comment{badness-lint skip missing-required-field}\n@book{a}\n",
            "@comment{badness-lint off missing-required-field}\n@book{a}\n@comment{badness-lint on missing-required-field}\n",
            "@comment{badness-lint skip-file missing-required-field}\n",
            "@comment{badness skip}\n@book{a}\n",
        ] {
            assert!(findings(src).is_empty(), "{src:?}");
        }
    }
}
