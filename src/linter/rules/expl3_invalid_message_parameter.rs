//! Expl3 messages accept four parameters. Higher parameter numbers have no
//! corresponding message argument; the author's intended replacement is unknown.

use crate::ast::command_name;
use crate::linter::diagnostic::Diagnostic;
use crate::linter::expl3::is_message_definition;
use crate::syntax::{SyntaxElement, SyntaxKind};

use super::{Example, Rule, RuleContext};

pub struct Expl3InvalidMessageParameter;

const EXAMPLES: &[Example] = &[Example {
    caption: "A message refers to a fifth parameter:",
    source: "\\ExplSyntaxOn\n\\msg_new:nnn { demo } { bad-value } { Invalid~value:~#5 }\n\\ExplSyntaxOff\n",
}];

impl Rule for Expl3InvalidMessageParameter {
    fn id(&self) -> &'static str {
        "expl3-invalid-message-parameter"
    }

    fn description(&self) -> &'static str {
        "Flag `#5` through `#9` in either text argument of a literal expl3 \
         `msg_new`, `msg_set`, or `msg_gset` definition. Messages accept only \
         `#1` through `#4`. Escaped hashes and parameters belonging to an \
         enclosing function definition are distinguished from message parameters. \
         Checks cover recognized executable calls and unexpanded function bodies; \
         stored token lists, expanded text arguments, and unresolved calls stay \
         silent. Report-only: the intended message argument is unknown."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }
    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::COMMAND]
    }

    fn check(&self, el: &SyntaxElement, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(node) = el.as_node() else { return };
        if !command_name(node).is_some_and(|name| is_message_definition(&name)) {
            return;
        }
        let Some(call) = ctx.expl3_call(node) else {
            return;
        };
        for &(range, number) in &call.message_parameters {
            sink.push(Diagnostic {
                rule: self.id(),
                severity: self.default_severity(),
                path: Default::default(),
                start: range.start().into(),
                end: range.end().into(),
                message: format!("invalid expl3 message parameter `#{number}`; messages accept only `#1` through `#4`"),
                fix: None,
                related: Vec::new(),
            });
        }
    }
}
