//! `inert-suppression`: a suppression directive that cannot take effect, or an
//! `off` region that reaches EOF without its intended closer.
//!
//! Placement and region matching come from the shared parser-side resolver. The
//! rule never re-parses comments or repeats the CST attachment walk. It is
//! report-only: repairing a dangling `skip`, unmatched `on`, unclosed `off`, or
//! directive-shaped `.dtx` prose requires knowing which construct or boundary
//! the author intended.

use std::path::PathBuf;

use crate::directives::DirectiveOutcome;
use crate::linter::diagnostic::Diagnostic;

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[Example {
    caption: "An `on` directive with no matching open region does nothing:",
    source: "% badness-lint on deprecated-command\n{\\bf text}\n",
}];

pub struct InertSuppression;

impl Rule for InertSuppression {
    fn id(&self) -> &'static str {
        "inert-suppression"
    }

    fn description(&self) -> &'static str {
        "Flag a suppression directive that cannot take effect: `skip` with no \
         following construct, `on` with no matching `off`, or a directive written \
         on a `.dtx` documentation-margin line, where `%` is typeset prose rather \
         than a comment. Also flag an `off` region left open at EOF; it currently \
         suppresses through the end of the file, but the missing closer is usually \
         accidental. Report-only: moving, deleting, or closing the directive \
         requires knowing the boundary the author intended. Inline suppressions \
         cannot hide this meta diagnostic; use `[lint].ignore` to disable the rule \
         deliberately."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn check_file(&self, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        for located in ctx
            .suppressions
            .directives()
            .iter()
            .filter(|located| located.outcome != DirectiveOutcome::Honored)
        {
            let message = match located.outcome {
                DirectiveOutcome::Honored => continue,
                DirectiveOutcome::DanglingSkip => {
                    "`skip` has no following construct, so this directive does nothing"
                }
                DirectiveOutcome::UnmatchedOn => {
                    "`on` has no matching `off`, so this directive closes no region"
                }
                DirectiveOutcome::UnclosedOff => {
                    "`off` reaches the end of the file without a matching `on`"
                }
                DirectiveOutcome::Unsupported => {
                    "this directive is on a `.dtx` documentation line, where `%` is \
                     typeset prose rather than a comment"
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
    use crate::linter::rules::RuleContext;
    use crate::parser::{LatexFlavor, LexConfig, parse_with_flavor};
    use crate::semantic::SemanticModel;

    fn findings_with(src: &str, config: LexConfig) -> Vec<Diagnostic> {
        let root = parse_with_flavor(src, config).syntax();
        assert_eq!(root.to_string(), src);
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
        InertSuppression.check_file(&ctx, &mut out);
        out
    }

    fn findings(src: &str) -> Vec<Diagnostic> {
        findings_with(src, LexConfig::default())
    }

    #[test]
    fn reports_each_inert_or_incomplete_shape() {
        for (src, message) in [
            (
                "% badness-lint skip deprecated-command\n",
                "no following construct",
            ),
            (
                "% badness-lint on deprecated-command\n",
                "no matching `off`",
            ),
            (
                "% badness-lint off deprecated-command\n\\bf\n",
                "without a matching `on`",
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
    fn reports_a_directive_on_a_dtx_documentation_line() {
        let src = "% badness-lint skip deprecated-command\nDocumentation.\n";
        let out = findings_with(
            src,
            LexConfig {
                flavor: LatexFlavor::Document,
                dtx: true,
            },
        );
        assert_eq!(out.len(), 1);
        assert_eq!(
            &src[out[0].start..out[0].end],
            "% badness-lint skip deprecated-command"
        );
        assert!(out[0].message.contains("documentation line"));
    }

    #[test]
    fn accepts_honored_directives() {
        for src in [
            "% badness-lint skip deprecated-command\n\\bf\n",
            "% badness-lint off deprecated-command\n\\bf\n% badness-lint on deprecated-command\n",
            "% badness-lint skip-file deprecated-command\n",
        ] {
            assert!(findings(src).is_empty(), "{src:?}");
        }
    }
}
