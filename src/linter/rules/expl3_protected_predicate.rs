//! A predicate must be expandable, so a protected conditional cannot provide
//! a `p` form. Removing protection or the predicate changes the API or meaning.

use crate::ast::command_name;
use crate::linter::diagnostic::Diagnostic;
use crate::linter::expl3::is_protected_conditional;
use crate::syntax::{SyntaxElement, SyntaxKind};

use super::{Example, Rule, RuleContext};

pub struct Expl3ProtectedPredicate;

const EXAMPLES: &[Example] = &[Example {
    caption: "A protected conditional requests a predicate:",
    source: "\\ExplSyntaxOn\n\\prg_new_protected_conditional:Nnn \\demo_ready: { p, TF }\n  { \\prg_return_true: }\n\\ExplSyntaxOff\n",
}];

impl Rule for Expl3ProtectedPredicate {
    fn id(&self) -> &'static str {
        "expl3-protected-predicate"
    }

    fn description(&self) -> &'static str {
        "Flag a protected expl3 conditional definition whose literal condition \
         list requests a `p` predicate. Predicates must be expandable, which \
         protection prevents. The `new`, `set`, and `gset` families are checked \
         in recognized executable code, including unexpanded function bodies. \
         Computed condition lists and unresolved calls stay silent. Report-only: \
         choosing between protection and the predicate changes the function's API \
         or meaning."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }
    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::COMMAND]
    }

    fn check(&self, el: &SyntaxElement, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(node) = el.as_node() else { return };
        if !command_name(node).is_some_and(|name| is_protected_conditional(&name)) {
            return;
        }
        let Some(call) = ctx.expl3_call(node) else {
            return;
        };
        if call.arguments[0].name().is_none() {
            return;
        }
        let Some(conditions) = call.arguments[call.arguments.len() - 2].list() else {
            return;
        };
        if !conditions
            .iter()
            .all(|c| matches!(c.text.as_str(), "p" | "T" | "F" | "TF"))
        {
            return;
        }
        for condition in conditions.into_iter().filter(|c| c.text == "p") {
            sink.push(Diagnostic {
                rule: self.id(),
                severity: self.default_severity(),
                path: Default::default(),
                start: condition.range.start().into(),
                end: condition.range.end().into(),
                message: "a protected expl3 conditional cannot define an expandable `p` predicate"
                    .to_owned(),
                fix: None,
                related: Vec::new(),
            });
        }
    }
}
