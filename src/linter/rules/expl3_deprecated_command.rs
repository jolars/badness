//! Report literal references to deprecated LaTeX3 kernel control sequences.

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::linter::diagnostic::Diagnostic;
use crate::syntax::{SyntaxElement, SyntaxKind};

use super::{Example, Rule, RuleContext};

pub struct Expl3DeprecatedCommand;

static DEPRECATIONS: LazyLock<HashMap<&str, &str>> = LazyLock::new(|| {
    include_str!("../expl3/deprecations.txt")
        .lines()
        .skip(1)
        .map(|line| line.split_once(' ').expect("generated deprecation row"))
        .collect()
});

impl Rule for Expl3DeprecatedCommand {
    fn id(&self) -> &'static str {
        "expl3-deprecated-command"
    }

    fn description(&self) -> &'static str {
        "Report literal references to deprecated control sequences in expl3 code, using the LaTeX3 kernel deprecation list pinned to the 2026-01-19 release. The warning includes the deprecation date."
    }

    fn examples(&self) -> &'static [Example] {
        &[Example {
            caption: "A deprecated expl3 function:",
            source: "\\ExplSyntaxOn\n\\str_lower_case:n { ABC }\n\\ExplSyntaxOff\n",
        }]
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::CONTROL_WORD]
    }

    fn check(&self, el: &SyntaxElement, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(token) = el.as_token() else { return };
        let Some(date) = token
            .text()
            .strip_prefix('\\')
            .and_then(|name| DEPRECATIONS.get(name))
        else {
            return;
        };
        let range = token.text_range();
        if ctx.in_expl3_lexical_mode(range.start()) {
            sink.push(Diagnostic {
                rule: self.id(),
                severity: self.default_severity(),
                path: Default::default(),
                start: range.start().into(),
                end: range.end().into(),
                message: format!("`{}` was deprecated in LaTeX3 on {date}", token.text()),
                fix: None,
                related: Vec::new(),
            });
        }
    }
}
