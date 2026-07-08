//! `deprecated-command`: the obsolete two-letter font *switches* (`\bf`, `\it`,
//! …) superseded by the LaTeX 2e `\…shape`/`\…family`/`\…series` declarations.
//!
//! These are the classic `\bf`-style commands the LaTeX team has discouraged
//! since 1994. The replacement is a plain declaration swap (`\bf` → `\bfseries`),
//! carried as a `Safe` autofix that replaces just the control word — correct by
//! construction (tenet 1), withheld on the rare shape where the control word
//! cannot be isolated. `\em` is intentionally absent: it is still the supported
//! emphasis switch.
//!
//! The table lives here, not in `data/signatures.json`: deprecation is a lint
//! judgment, not the structural arity/verbatim fact the signature DB carries
//! (AGENTS.md core decision #2).

use std::path::PathBuf;

use crate::ast::{command_name, control_word_range};
use crate::linter::diagnostic::{Diagnostic, Fix, Severity};
use crate::syntax::{SyntaxElement, SyntaxKind};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[Example {
    caption: "An obsolete two-letter font switch:",
    source: "{\\bf important}\n",
}];

/// Deprecated control word → its modern replacement.
const DEPRECATED: &[(&str, &str)] = &[
    ("bf", "bfseries"),
    ("it", "itshape"),
    ("rm", "rmfamily"),
    ("sf", "sffamily"),
    ("tt", "ttfamily"),
    ("sc", "scshape"),
    ("sl", "slshape"),
];

pub struct DeprecatedCommand;

impl Rule for DeprecatedCommand {
    fn id(&self) -> &'static str {
        "deprecated-command"
    }

    fn emits_fix(&self) -> bool {
        true
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag the obsolete two-letter font *switches* (`\\bf`, `\\it`, `\\rm`, \
         `\\sf`, `\\tt`, `\\sc`, `\\sl`) that LaTeX 2e superseded with the \
         `\\...series`/`\\...shape`/`\\...family` declarations. `\\em` is not \
         flagged; it is still the supported emphasis switch. The autofix swaps \
         just the control word (`\\bf` -> `\\bfseries`), leaving any following \
         text untouched, so it is correct by construction."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::COMMAND]
    }

    fn check(&self, el: &SyntaxElement, _ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(command) = el.as_node() else {
            return;
        };
        let Some(name) = command_name(command) else {
            return;
        };
        let Some((_, replacement)) = DEPRECATED.iter().find(|(dep, _)| *dep == name) else {
            return;
        };
        // Underline just the control word, not any greedily-attached group, so
        // the caret sits tightly on `\bf`.
        let control_word = control_word_range(command);
        let range = control_word.unwrap_or_else(|| command.text_range());
        // The fix is a tight control-word swap (`\bf` → `\bfseries`): the span
        // covers exactly the `CONTROL_WORD` token (backslash included), the
        // replacement copies in the modern declaration, so it stays correct by
        // construction (tenet 1). Withheld on the fallback span, where the tight
        // control word could not be isolated and a whole-node rewrite might drop
        // a greedily-attached group.
        let fix = control_word.map(|r| {
            Fix::safe(
                usize::from(r.start()),
                usize::from(r.end()),
                format!("\\{replacement}"),
                format!("Replace `\\{name}` with `\\{replacement}`"),
            )
        });
        sink.push(Diagnostic {
            rule: self.id(),
            severity: self.default_severity(),
            path: PathBuf::new(),
            start: usize::from(range.start()),
            end: usize::from(range.end()),
            message: format!("`\\{name}` is deprecated; use `\\{replacement}`"),
            fix,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use crate::semantic::SemanticModel;
    use crate::syntax::SyntaxNode;

    fn findings(src: &str) -> Vec<Diagnostic> {
        let root = SyntaxNode::new_root(parse(src).green);
        let model = SemanticModel::build(&root);
        let ctx = RuleContext::new(std::path::Path::new("x.tex"), &root, &model, None, None);
        let mut out = Vec::new();
        for el in root.descendants_with_tokens() {
            if DeprecatedCommand.interests().contains(&el.kind()) {
                DeprecatedCommand.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_bare_font_switch() {
        let out = findings("{\\bf hi}\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "deprecated-command");
        assert!(
            out[0].message.contains("\\bfseries"),
            "got: {}",
            out[0].message
        );
        // Caret covers just `\bf` (bytes 1..4), not the trailing text.
        assert_eq!((out[0].start, out[0].end), (1, 4));
    }

    #[test]
    fn modern_commands_are_fine() {
        assert!(findings("\\textbf{x}\\emph{y}\n").is_empty());
    }

    #[test]
    fn em_is_not_deprecated() {
        assert!(findings("{\\em hi}\n").is_empty());
    }

    #[test]
    fn flags_each_occurrence() {
        assert_eq!(findings("{\\bf a}{\\it b}\n").len(), 2);
    }

    #[test]
    fn carries_safe_control_word_fix() {
        use crate::linter::diagnostic::Applicability;
        use crate::linter::fix::apply_fixes;

        let src = "{\\bf hi}\n";
        let out = findings(src);
        let fix = out[0].fix.as_ref().expect("should carry a fix");
        assert_eq!(fix.applicability, Applicability::Safe);
        // The fix spans just the `\bf` control word (bytes 1..4), swapping it for
        // the modern declaration while leaving the rest of the group untouched.
        assert_eq!((fix.start, fix.end), (1, 4));
        assert_eq!(fix.content, "\\bfseries");
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), false).output,
            "{\\bfseries hi}\n"
        );
    }
}
