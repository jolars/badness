//! Shared expl3 semantic call recognition.

pub(crate) use badness_parser::semantic::expl3::calls::*;

mod variables;
pub(crate) use variables::{Finding, VariableFinding, VariableIndex};

pub(crate) fn report_variable_findings(
    rule: &dyn super::rules::Rule,
    ctx: &super::rules::RuleContext<'_>,
    node: &crate::syntax::SyntaxNode,
    kind: VariableFinding,
    sink: &mut Vec<super::diagnostic::Diagnostic>,
) {
    sink.extend(
        ctx.expl3_variable_findings(node)
            .iter()
            .filter(|finding| finding.kind == kind)
            .map(|finding| super::diagnostic::Diagnostic {
                rule: rule.id(),
                severity: rule.default_severity(),
                path: Default::default(),
                start: finding.range.start().into(),
                end: finding.range.end().into(),
                message: finding.message.clone(),
                fix: None,
                related: Vec::new(),
            }),
    );
}
