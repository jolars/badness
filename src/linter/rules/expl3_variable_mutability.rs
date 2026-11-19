//! Report constant declarations using a variable name and assignments to a constant name..

use crate::linter::diagnostic::Diagnostic;
use crate::linter::expl3::VariableFinding;
use crate::syntax::{SyntaxElement, SyntaxKind};

use super::{Example, Rule, RuleContext};

pub struct Expl3VariableMutability;

impl Rule for Expl3VariableMutability {
    fn id(&self) -> &'static str {
        "expl3-variable-mutability"
    }

    fn description(&self) -> &'static str {
        "Report constant declarations using a variable name and assignments to a constant name."
    }

    fn examples(&self) -> &'static [Example] {
        &[Example {
            caption: "A variable is declared as a constant:",
            source: "\\ExplSyntaxOn\n\\tl_const:Nn \\g_demo_tl { value }\n\\ExplSyntaxOff\n",
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
            VariableFinding::Mutability,
            sink,
        );
    }
}
