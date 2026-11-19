//! Report incompatible types on literal variable operands of recognized expl3 kernel operations.

use crate::linter::diagnostic::Diagnostic;
use crate::linter::expl3::VariableFinding;
use crate::syntax::{SyntaxElement, SyntaxKind};

use super::{Example, Rule, RuleContext};

pub struct Expl3VariableType;

impl Rule for Expl3VariableType {
    fn id(&self) -> &'static str {
        "expl3-variable-type"
    }

    fn description(&self) -> &'static str {
        "Report incompatible types on literal variable operands of recognized expl3 kernel operations. Checks use the variable suffix and the operand role."
    }

    fn examples(&self) -> &'static [Example] {
        &[Example {
            caption: "A sequence operation receives an integer variable:",
            source: "\\ExplSyntaxOn\n\\seq_count:N \\l_demo_int\n\\ExplSyntaxOff\n",
        }]
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::COMMAND]
    }

    fn check(&self, el: &SyntaxElement, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(node) = el.as_node() else { return };
        super::super::expl3::report_variable_findings(self, ctx, node, VariableFinding::Type, sink);
    }
}
