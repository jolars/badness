//! Report braced operands in literal expl3 N slots.

use crate::ast::{AstNode, Command};
use crate::linter::diagnostic::Diagnostic;
use crate::syntax::{SyntaxElement, SyntaxKind};

use super::{Example, Rule, RuleContext};

pub struct Expl3BracedNArgument;

impl Rule for Expl3BracedNArgument {
    fn id(&self) -> &'static str {
        "expl3-braced-n-argument"
    }

    fn default_enabled(&self) -> bool {
        false
    }

    fn description(&self) -> &'static str {
        "Flag braced `N` arguments in recognized expl3 calls. The `N` convention calls for a bare token. TeX can accept a braced group here, and removing its braces can change which tokens the function receives."
    }

    fn examples(&self) -> &'static [Example] {
        &[Example {
            caption: "Braces around an expl3 token-list variable",
            source: "\\ExplSyntaxOn\n\\tl_clear:N { \\l_tmpa_tl }\n\\ExplSyntaxOff\n",
        }]
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::COMMAND]
    }

    fn check(&self, el: &SyntaxElement, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(command) = el.as_node().cloned().and_then(Command::cast) else {
            return;
        };
        if !command.control_word().is_some_and(|head| {
            head.name()
                .rsplit_once(':')
                .is_some_and(|(_, spec)| spec.contains('N'))
        }) {
            return;
        }
        let Some(call) = ctx.expl3_call(command.syntax()) else {
            return;
        };
        for argument in &call.arguments {
            if argument.specifier == b'N'
                && let Some(range) = argument.group_range()
            {
                sink.push(Diagnostic {
                    rule: self.id(),
                    severity: self.default_severity(),
                    path: Default::default(),
                    start: range.start().into(),
                    end: range.end().into(),
                    message: format!("braced `N` argument to `\\{}`", call.name),
                    fix: None,
                    related: Vec::new(),
                });
            }
        }
    }
}
