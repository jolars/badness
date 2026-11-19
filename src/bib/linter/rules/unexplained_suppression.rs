//! Opt-in explanations for BibTeX suppression directives. Reuse the
//! shared reason-presence fact and the LaTeX rule's diagnostic policy.

use crate::linter::diagnostic::Diagnostic;
use crate::linter::rules::unexplained_suppression::unexplained_diagnostic;

use super::{BibRule, BibRuleContext, Example};

const EXAMPLES: &[Example] = &[Example {
    caption: "Explain why an entry's diagnostics are suppressed:",
    source: "@comment{badness-lint skip missing-required-field}\n@book{key}\n",
}];

pub struct UnexplainedSuppression;

impl BibRule for UnexplainedSuppression {
    fn id(&self) -> &'static str {
        "unexplained-suppression"
    }

    fn default_enabled(&self) -> bool {
        false
    }

    fn description(&self) -> &'static str {
        "Require a nonempty `: <reason>` on `skip`, `off`, and `skip-file` \
         suppression directives in `@comment` entries and `%` comments, including \
         format-only, combined, and retired spellings. Closing `on` \
         directives need no explanation. This rule is opt-in because requiring \
         explanations is a project convention. Enable it with \
         `extend-select = [\"unexplained-suppression\"]` under `[lint]`. No autofix \
         is offered because the explanation must come from the author. Inline \
         suppressions cannot hide this diagnostic. Use `[lint].ignore` to disable it."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn check_file(&self, ctx: &BibRuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        for located in ctx.suppressions.directives() {
            if let Some(diagnostic) = unexplained_diagnostic(&located.directive, located.range) {
                sink.push(diagnostic);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bib::linter::suppression::BibSuppressionMap;
    use crate::bib::{parse, semantic::Model};

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
        UnexplainedSuppression.check_file(&ctx, &mut out);
        out
    }

    #[test]
    fn checks_both_carriers_and_empty_reasons() {
        for (open, close) in [('(', ')'), ('{', '}')] {
            for head in [
                "badness-lint skip missing-required-field",
                "badness-lint off missing-required-field",
                "badness-lint skip-file",
                "badness skip",
                "badness-format skip-file",
                "badness-ignore missing-required-field",
                "badness-ignore-file",
            ] {
                for tail in ["", ":", ": \t", ":\u{2003}"] {
                    let comment = format!("@comment{open}{head}{tail}{close}");
                    let src = format!("\n{comment}\n@book{{key}}\n");
                    let out = findings(&src);
                    assert_eq!(out.len(), 1, "{src:?}: {out:?}");
                    assert_eq!(&src[out[0].start..out[0].end], comment);
                    assert!(out[0].fix.is_none());
                }
                let src = format!("@comment{open}{head}: imported data{close}\n@book{{key}}\n");
                assert!(findings(&src).is_empty(), "{src:?}");
            }
        }
    }

    #[test]
    fn accepts_closers_and_ignores_other_text() {
        for src in [
            "@comment{badness-lint on}\n",
            "@comment{badness-format on}\n",
            "@comment{badness on}\n",
            "@comment{ordinary comment}\n",
            "@book{k, note = {badness-lint skip}}\n",
        ] {
            assert!(findings(src).is_empty(), "{src:?}");
        }
    }

    #[test]
    fn field_suppressions_need_a_reason_when_enabled() {
        let missing = "% badness-lint skip unknown-field";
        let src = format!("@misc{{key,\n  {missing}\n  localid = {{123}}\n}}\n");
        let out = findings(&src);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(&src[out[0].start..out[0].end], missing);
        assert!(findings(&src.replace(missing, &format!("{missing}: custom field"))).is_empty());
    }
}
