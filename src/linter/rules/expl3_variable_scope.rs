//! Report local assignments to global variables and global assignments to local variables in recognized nested expl3 code.

use crate::linter::diagnostic::Diagnostic;
use crate::linter::expl3::VariableFinding;
use crate::syntax::{SyntaxElement, SyntaxKind};

use super::{Example, Rule, RuleContext};

pub struct Expl3VariableScope;

impl Rule for Expl3VariableScope {
    fn id(&self) -> &'static str {
        "expl3-variable-scope"
    }

    fn description(&self) -> &'static str {
        "Report local assignments to global variables and global assignments to local variables in recognized nested expl3 code. Top-level assignments are exempt."
    }

    fn examples(&self) -> &'static [Example] {
        &[Example {
            caption: "A function assigns locally to a global variable:",
            source: "\\ExplSyntaxOn\n\\cs_new_protected:Nn \\demo: { \\tl_set:Nn \\g_demo_tl { value } }\n\\ExplSyntaxOff\n",
        }]
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::COMMAND]
    }

    fn check(&self, el: &SyntaxElement, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(node) = el.as_node() else { return };
        super::super::expl3::report_variable_findings(
            self,
            ctx,
            node,
            VariableFinding::Scope,
            sink,
        );
    }
}
