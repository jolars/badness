//! Opt-in explanations for suppression directives. The shared directive parser
//! retains reason presence; this rule does not re-parse comment text. No fix can
//! supply the author's reason for suppressing a check or preserving layout.

use std::path::PathBuf;

use crate::directives::{Directive, Verb};
use crate::linter::diagnostic::{Diagnostic, Severity};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[Example {
    caption: "Explain why a check is suppressed with `: <reason>`:",
    source: "% badness-lint skip deprecated-command\n{\\bf text}\n",
}];

pub struct UnexplainedSuppression;

impl Rule for UnexplainedSuppression {
    fn id(&self) -> &'static str {
        "unexplained-suppression"
    }

    fn default_enabled(&self) -> bool {
        false
    }

    fn description(&self) -> &'static str {
        "Require a nonempty `: <reason>` on `skip`, `off`, and `skip-file` \
         suppression directives, including format-only, combined, and retired \
         spellings. Closing `on` directives need no explanation. This rule is \
         opt-in because requiring explanations is a project convention. Enable \
         it with `extend-select = [\"unexplained-suppression\"]` under `[lint]`. \
         No autofix is offered because the explanation must come from the \
         author. Inline suppressions cannot hide this meta diagnostic; use \
         `[lint].ignore` to disable the rule deliberately."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn check_file(&self, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        for located in ctx.suppressions.directives() {
            if let Some(diagnostic) = unexplained_diagnostic(&located.directive, located.range) {
                sink.push(diagnostic);
            }
        }
    }
}

/// Shared policy and diagnostic for the LaTeX and BibTeX directive carriers.
pub(crate) fn unexplained_diagnostic(
    directive: &Directive,
    range: rowan::TextRange,
) -> Option<Diagnostic> {
    if directive.verb == Verb::On || directive.has_reason {
        return None;
    }
    Some(Diagnostic {
        rule: "unexplained-suppression",
        severity: Severity::Warning,
        path: PathBuf::new(),
        start: usize::from(range.start()),
        end: usize::from(range.end()),
        message: "suppression directive has no explanation; add `: <reason>`".to_owned(),
        fix: None,
        related: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use crate::semantic::SemanticModel;

    fn findings(src: &str) -> Vec<Diagnostic> {
        let root = parse(src).syntax();
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
        UnexplainedSuppression.check_file(&ctx, &mut out);
        out
    }

    #[test]
    fn reports_missing_and_empty_reasons_on_every_suppression_family() {
        for head in [
            "badness-lint skip deprecated-command",
            "badness-lint off deprecated-command",
            "badness-lint skip-file",
            "badness-format skip",
            "badness-format off",
            "badness-format skip-file",
            "badness skip",
            "badness off",
            "badness skip-file",
            "badness-ignore deprecated-command",
            "badness-ignore-file",
        ] {
            for tail in ["", ":", ": \t", ":\u{2003}"] {
                let comment = format!("%%% {head}{tail}");
                let src = format!("Text.\n{comment}\n{{\\bf text}}\n");
                let out = findings(&src);
                assert_eq!(out.len(), 1, "{src:?}: {out:?}");
                assert_eq!(out[0].rule, "unexplained-suppression");
                assert_eq!(&src[out[0].start..out[0].end], comment);
                assert!(out[0].fix.is_none());
            }
            let src = format!("% {head}: required by the publisher\n{{\\bf text}}\n");
            assert!(findings(&src).is_empty(), "{src:?}");
        }
    }

    #[test]
    fn accepts_closers_ordinary_comments_and_protected_text() {
        for src in [
            "% badness-lint on\n",
            "% badness-format on\n",
            "% badness on\n",
            "% ordinary comment\n",
            "\\begin{verbatim}\n% badness-lint skip\n\\end{verbatim}\n",
            "\\verb|% badness-lint skip|\n",
        ] {
            assert!(findings(src).is_empty(), "{src:?}");
        }
    }
}
