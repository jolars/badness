//! Flags rows in built-in table environments that consume more columns than
//! their preamble declares. Such a row cannot be placed without TeX's
//! extra-alignment-tab recovery. The rule declines custom column types and
//! dynamic `\multicolumn` spans, and it offers no fix because either side of the
//! mismatch may be the author's mistake.

use std::path::PathBuf;

use rowan::TextRange;

use super::{Example, Rule, RuleContext};
use crate::ast::{AstNode, Command, Environment, Group, children};
use crate::linter::diagnostic::{Diagnostic, RelatedInfo, Severity};
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};

pub struct ExtraAlignmentTab;

const EXAMPLES: &[Example] = &[Example {
    caption: "A row that exceeds the declared table width:",
    source: "\\begin{tabular}{ll}\n  a & b & c \\\\\n\\end{tabular}\n",
}];

impl Rule for ExtraAlignmentTab {
    fn id(&self) -> &'static str {
        "extra-alignment-tab"
    }

    fn description(&self) -> &'static str {
        "Flags a row in a built-in `tabular`, `tabular*`, or `array` environment \
         that consumes more columns than its column preamble declares. LaTeX \
         cannot place the overflowing cell and reports an extra alignment tab. \
         Short rows are valid and are not flagged. Custom column types and \
         dynamic `\\multicolumn` spans are left alone when their width cannot be \
         established statically. No autofix is offered because either the row or \
         the preamble may be wrong."
    }

    fn default_severity(&self) -> Severity {
        Severity::Error
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::ENVIRONMENT]
    }

    fn check(&self, el: &SyntaxElement, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(node) = el.as_node() else { return };
        let Some(env) = Environment::cast(node.clone()) else {
            return;
        };
        let Some(begin) = env.begin() else { return };
        let Some(name) = begin.name() else { return };
        if !matches!(name.as_str(), "tabular" | "tabular*" | "array")
            || ctx.user_definitions().environment(&name).is_some()
        {
            return;
        }

        let Some(spec) = children::<Group>(begin.syntax()).last() else {
            return;
        };
        let Some(columns) = crate::formatter::column_count(&spec.inner_source()).filter(|&n| n > 0)
        else {
            return;
        };
        let Some(spec_range) = inner_range(spec.syntax()) else {
            return;
        };

        let mut body = Vec::new();
        for element in node.children_with_tokens() {
            match element {
                SyntaxElement::Node(child)
                    if matches!(child.kind(), SyntaxKind::BEGIN | SyntaxKind::END) => {}
                SyntaxElement::Node(child)
                    if matches!(child.kind(), SyntaxKind::PARAGRAPH | SyntaxKind::MATH) =>
                {
                    body.extend(child.children_with_tokens());
                }
                other => body.push(other),
            }
        }
        check_rows(self, &body, columns, spec_range, ctx, sink);
    }
}

fn inner_range(group: &SyntaxNode) -> Option<TextRange> {
    let open = group.first_token()?;
    let close = group.last_token()?;
    (open.kind() == SyntaxKind::L_BRACE && close.kind() == SyntaxKind::R_BRACE)
        .then(|| TextRange::new(open.text_range().end(), close.text_range().start()))
}

fn check_rows(
    rule: &ExtraAlignmentTab,
    body: &[SyntaxElement],
    columns: usize,
    spec_range: TextRange,
    ctx: &RuleContext<'_>,
    sink: &mut Vec<Diagnostic>,
) {
    let mut cell = Vec::new();
    let mut used = 0usize;
    let mut declined = false;
    let mut reported = false;

    for element in body {
        match element {
            SyntaxElement::Token(token) if token.kind() == SyntaxKind::AMPERSAND => {
                if !declined && !reported {
                    match cell_span(&cell) {
                        Some((span, culprit)) => {
                            used = used.saturating_add(span);
                            if used > columns {
                                emit(
                                    rule,
                                    culprit.unwrap_or_else(|| token.text_range()),
                                    used,
                                    columns,
                                    spec_range,
                                    ctx,
                                    sink,
                                );
                                reported = true;
                            } else if used == columns {
                                emit(
                                    rule,
                                    token.text_range(),
                                    used + 1,
                                    columns,
                                    spec_range,
                                    ctx,
                                    sink,
                                );
                                reported = true;
                            }
                        }
                        None => declined = true,
                    }
                }
                cell.clear();
            }
            SyntaxElement::Node(child) if child.kind() == SyntaxKind::LINE_BREAK => {
                finish_row(
                    rule,
                    &cell,
                    used,
                    columns,
                    spec_range,
                    ctx,
                    declined || reported,
                    sink,
                );
                cell.clear();
                used = 0;
                declined = false;
                reported = false;
            }
            _ => cell.push(element.clone()),
        }
    }
    finish_row(
        rule,
        &cell,
        used,
        columns,
        spec_range,
        ctx,
        declined || reported,
        sink,
    );
}

#[allow(clippy::too_many_arguments)]
fn finish_row(
    rule: &ExtraAlignmentTab,
    cell: &[SyntaxElement],
    used: usize,
    columns: usize,
    spec_range: TextRange,
    ctx: &RuleContext<'_>,
    skip: bool,
    sink: &mut Vec<Diagnostic>,
) {
    if skip || cell.iter().all(is_trivia) {
        return;
    }
    let Some((span, culprit)) = cell_span(cell) else {
        return;
    };
    let total = used.saturating_add(span);
    if total > columns
        && let Some(range) = culprit
    {
        emit(rule, range, total, columns, spec_range, ctx, sink);
    }
}

fn cell_span(cell: &[SyntaxElement]) -> Option<(usize, Option<TextRange>)> {
    let content: Vec<&SyntaxElement> = cell.iter().filter(|element| !is_trivia(element)).collect();
    let multicolumns: Vec<Command> = content
        .iter()
        .filter_map(|element| element.as_node())
        .filter_map(|node| Command::cast(node.clone()))
        .filter(|command| command.name().as_deref() == Some("multicolumn"))
        .collect();
    if multicolumns.is_empty() {
        return Some((1, None));
    }
    if content.len() != 1 || multicolumns.len() != 1 {
        return None;
    }

    let command = &multicolumns[0];
    let span = command.nth_group_text(0)?.trim().parse::<usize>().ok()?;
    if span == 0 {
        return None;
    }
    let third = command.nth_group(2)?;
    let range = TextRange::new(
        command.syntax().text_range().start(),
        third.syntax().text_range().end(),
    );
    Some((span, Some(range)))
}

fn is_trivia(element: &SyntaxElement) -> bool {
    element.as_token().is_some_and(|token| {
        matches!(
            token.kind(),
            SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
        )
    })
}

fn emit(
    rule: &ExtraAlignmentTab,
    range: TextRange,
    used: usize,
    columns: usize,
    spec_range: TextRange,
    ctx: &RuleContext<'_>,
    sink: &mut Vec<Diagnostic>,
) {
    sink.push(Diagnostic {
        rule: rule.id(),
        severity: rule.default_severity(),
        path: PathBuf::new(),
        start: usize::from(range.start()),
        end: usize::from(range.end()),
        message: format!(
            "row uses at least {used} columns, but the table preamble declares {columns}"
        ),
        fix: None,
        related: vec![RelatedInfo {
            path: ctx.path.to_path_buf(),
            start: usize::from(spec_range.start()),
            end: usize::from(spec_range.end()),
            message: format!("table preamble declares {columns} columns"),
        }],
    });
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
        let ctx = RuleContext::new(
            std::path::Path::new("x.tex"),
            &root,
            &model,
            None,
            None,
            None,
        );
        let mut out = Vec::new();
        for el in root.descendants_with_tokens() {
            if ExtraAlignmentTab.interests().contains(&el.kind()) {
                ExtraAlignmentTab.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_first_separator_beyond_the_preamble() {
        let src = "\\begin{tabular}{ll}\n  a & b & c \\\\\n\\end{tabular}\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "extra-alignment-tab");
        assert_eq!(out[0].severity, crate::linter::diagnostic::Severity::Error);
        assert_eq!(&src[out[0].start..out[0].end], "&");
        assert_eq!(
            out[0].message,
            "row uses at least 3 columns, but the table preamble declares 2"
        );
        assert!(out[0].fix.is_none());
        assert_eq!(out[0].related.len(), 1);
        let preamble = &out[0].related[0];
        assert_eq!(preamble.path, std::path::Path::new("x.tex"));
        assert_eq!(&src[preamble.start..preamble.end], "ll");
    }

    #[test]
    fn permits_omitted_trailing_cells() {
        assert!(findings("\\begin{tabular}{lll}\n  a & b \\\\\n\\end{tabular}\n").is_empty());
    }

    #[test]
    fn counts_literal_multicolumn_spans() {
        let bad = "\\begin{tabular}{ll}\n  \\multicolumn{2}{c}{a} & b \\\\\n\\end{tabular}\n";
        let out = findings(bad);
        assert_eq!(out.len(), 1);
        assert_eq!(&bad[out[0].start..out[0].end], "&");

        assert!(
            findings("\\begin{tabular}{lll}\n  \\multicolumn{2}{c}{a} & b \\\\\n\\end{tabular}\n")
                .is_empty()
        );
    }

    #[test]
    fn flags_a_final_multicolumn_that_overflows() {
        let src = "\\begin{tabular}{ll}\n  a & \\multicolumn{2}{c}{b} \\\\\n\\end{tabular}\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(&src[out[0].start..out[0].end], "\\multicolumn{2}{c}{b}");
    }

    #[test]
    fn understands_repeated_and_decorated_preambles() {
        assert!(
            findings(
                "\\begin{tabular}{@{}*{2}{>{\\bfseries}c}@{}}\n  a & b \\\\\n\\end{tabular}\n"
            )
            .is_empty()
        );
        assert_eq!(
            findings(
                "\\begin{tabular}{@{}*{2}{>{\\bfseries}c}@{}}\n  a & b & c \\\\\n\\end{tabular}\n"
            )
            .len(),
            1
        );
    }

    #[test]
    fn declines_unknown_preambles_and_dynamic_spans() {
        assert!(findings("\\begin{tabular}{XX}\n  a & b & c \\\\\n\\end{tabular}\n").is_empty());
        assert!(
            findings("\\begin{tabular}{ll}\n  \\multicolumn{\\n}{c}{a} & b \\\\\n\\end{tabular}\n")
                .is_empty()
        );
    }

    #[test]
    fn ignores_nested_ampersands_and_non_table_environments() {
        assert!(findings("\\begin{tabular}{l}\n  {a & b} \\\\\n\\end{tabular}\n").is_empty());
        assert!(findings("\\begin{align}\n  a & b & c \\\\\n\\end{align}\n").is_empty());
    }

    #[test]
    fn covers_array_and_tabular_star() {
        assert_eq!(
            findings("\\begin{array}{c}\n  a & b \\\\\n\\end{array}\n").len(),
            1
        );
        assert_eq!(
            findings("\\begin{tabular*}{4cm}{c}\n  a & b \\\\\n\\end{tabular*}\n").len(),
            1
        );
    }

    #[test]
    fn ignores_a_redefined_builtin_environment() {
        assert!(
            findings(
                "\\renewenvironment{tabular}[1]{}{}\n\\begin{tabular}{l}\n  a & b \\\\\n\\end{tabular}\n"
            )
            .is_empty()
        );
    }
}
