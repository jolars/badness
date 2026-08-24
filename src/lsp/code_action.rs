//! Pure code-action logic: turn fix-carrying linter findings into LSP quick-fixes
//! and build conservative syntax-aware refactorings.
//!
//! The threading side ([`super::run_code_action`]) re-lints the buffer off a fresh
//! snapshot (like the pull-diagnostics path) and hands the raw findings here. This
//! module is rule-agnostic: any finding whose [`crate::linter::Diagnostic::fix`] is
//! populated and whose caret overlaps the requested range becomes a `QUICKFIX`.
//! Independently, a cursor inside a statically understood table can receive an
//! atomic `REFACTOR_REWRITE` that appends a column to its preamble and rows.
//!
//! Fully-built actions are returned (no
//! `codeAction/resolve` step), and a fix's byte span maps straight to a `TextEdit`
//! via the shared [`super::byte_range_to_lsp`] — the fix owns *what* to rewrite,
//! never *how* to lay it out (tenet 1).

use std::collections::HashMap;
use std::path::PathBuf;

use super::*;
use crate::ast::{AstNode, Command, Environment, Group, children};
use crate::linter::diagnostic::Applicability;
use crate::syntax::SyntaxElement;
use lsp_types::{CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionResponse};

/// Build the quick-fix actions for the findings overlapping `request_range`.
///
/// A finding is offered when it carries a `fix` and its diagnostic span overlaps the
/// request range (inclusive at the edges, so a zero-width cursor sitting on `\bf`
/// matches). The edit replaces the fix's byte span verbatim; `Safe` fixes are marked
/// `is_preferred`.
///
/// A fix's edits are grouped into a `WorkspaceEdit` keyed by URI. Most edits target
/// the buffer under `uri` (the finding's own file, [`Edit::path`] `None`); an edit
/// with `path: Some(p)` targets file `p`, resolved to its `(Uri, text)` via
/// `resolve`. If any foreign target can't be resolved (its text is not in the
/// snapshot), the whole action is dropped rather than emit a partial cross-file
/// edit.
#[allow(clippy::too_many_arguments)]
pub(crate) fn code_actions_for_range(
    findings: &[crate::linter::Diagnostic],
    text: &TextBuffer,
    uri: &Uri,
    self_path: &Path,
    request_range: Range,
    enc: PositionEncoding,
    link_docs: bool,
    resolve: &dyn Fn(&Path) -> Option<(Uri, String)>,
) -> CodeActionResponse {
    let idx = text.line_index();
    let req_start = idx.offset_at(request_range.start.line, request_range.start.character);
    let req_end = idx.offset_at(request_range.end.line, request_range.end.character);

    findings
        .iter()
        .filter_map(|d| {
            let fix = d.fix.as_ref()?;
            // Offer the action only when the finding's caret touches the request.
            if !byte_ranges_overlap(d.start, d.end, req_start, req_end) {
                return None;
            }
            let changes = workspace_changes(fix, uri, &idx, enc, resolve)?;
            Some(CodeActionOrCommand::CodeAction(CodeAction {
                title: fix.description.clone(),
                kind: Some(CodeActionKind::QUICKFIX),
                // Link the action to the finding it fixes, so the client can dim it
                // once the diagnostic clears.
                diagnostics: Some(vec![lint_to_lsp(&idx, d.clone(), link_docs, self_path)]),
                edit: Some(WorkspaceEdit {
                    changes: Some(changes),
                    ..Default::default()
                }),
                is_preferred: Some(fix.applicability == Applicability::Safe),
                ..Default::default()
            }))
        })
        .collect()
}

/// Offer a structural refactoring that appends one centered column to the table
/// enclosing the request's start position.
///
/// The edit is deliberately conservative: the environment must be a non-redefined
/// built-in table, its preamble and every data row must have a statically known,
/// matching width, and comments inside the body cause the action to be withheld.
/// These gates make the workspace edit atomic—either the preamble and every row
/// acquire the same trailing column, or no action is offered.
pub(crate) fn table_column_actions(
    root: &SyntaxNode,
    text: &TextBuffer,
    uri: &Uri,
    request_range: Range,
) -> CodeActionResponse {
    let idx = text.line_index();
    let offset = idx.offset_at(request_range.start.line, request_range.start.character);
    let Some((spec_insert, row_inserts)) = table_column_insertions(root, text, offset) else {
        return Vec::new();
    };

    let mut edits = Vec::with_capacity(row_inserts.len() + 1);
    edits.push(TextEdit {
        range: byte_range_to_lsp(&idx, spec_insert, spec_insert),
        new_text: "c".to_string(),
    });
    edits.extend(row_inserts.into_iter().map(|(at, content)| TextEdit {
        range: byte_range_to_lsp(&idx, at, at),
        new_text: content,
    }));

    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: "Add column at end".to_string(),
        kind: Some(CodeActionKind::REFACTOR_REWRITE),
        edit: Some(WorkspaceEdit {
            changes: Some(HashMap::from([(uri.clone(), edits)])),
            ..Default::default()
        }),
        ..Default::default()
    }))
    .into_iter()
    .collect()
}

fn table_column_insertions(
    root: &SyntaxNode,
    text: &str,
    offset: usize,
) -> Option<(usize, Vec<(usize, String)>)> {
    let offset = TextSize::try_from(offset).ok()?;
    let definitions = crate::semantic::scan_definitions(root);
    let env = root
        .descendants()
        .filter_map(Environment::cast)
        .filter(|env| env.syntax().text_range().contains_inclusive(offset))
        .filter(|env| {
            env.name().is_some_and(|name| {
                matches!(name.as_str(), "tabular" | "tabular*" | "array")
                    && definitions.environment(&name).is_none()
            })
        })
        .min_by_key(|env| env.syntax().text_range().len())?;
    let begin = env.begin()?;
    let spec = children::<Group>(begin.syntax()).last()?;
    let columns = crate::formatter::column_count(&spec.inner_source()).filter(|&n| n > 0)?;
    let close = spec.syntax().last_token()?;
    if close.kind() != SyntaxKind::R_BRACE {
        return None;
    }

    let body = table_body(env.syntax())?;
    if body.iter().any(|element| {
        element
            .as_token()
            .is_some_and(|token| token.kind() == SyntaxKind::COMMENT)
    }) {
        return None;
    }

    let mut row = Vec::new();
    let mut inserts = Vec::new();
    for element in body {
        if let SyntaxElement::Node(node) = &element
            && node.kind() == SyntaxKind::LINE_BREAK
        {
            if let Some(insert) =
                row_insertion(&row, columns, node.text_range().start(), text, &definitions)?
            {
                inserts.push(insert);
            }
            row.clear();
        } else {
            row.push(element);
        }
    }
    let end = env.end()?.syntax().text_range().start();
    if let Some(insert) = row_insertion(&row, columns, end, text, &definitions)? {
        inserts.push(insert);
    }

    Some((usize::from(close.text_range().start()), inserts))
}

/// Flatten the single paragraph/math wrapper in a table body. Multiple wrappers
/// mean a blank-line boundary, whose row semantics are not safe to rewrite.
fn table_body(env: &SyntaxNode) -> Option<Vec<SyntaxElement>> {
    let mut body = Vec::new();
    let mut wrappers = 0usize;
    for element in env.children_with_tokens() {
        match element {
            SyntaxElement::Node(node)
                if matches!(node.kind(), SyntaxKind::BEGIN | SyntaxKind::END) => {}
            SyntaxElement::Node(node)
                if matches!(node.kind(), SyntaxKind::PARAGRAPH | SyntaxKind::MATH) =>
            {
                wrappers += 1;
                if wrappers > 1 {
                    return None;
                }
                body.extend(node.children_with_tokens());
            }
            other => body.push(other),
        }
    }
    Some(body)
}

fn row_insertion(
    row: &[SyntaxElement],
    columns: usize,
    boundary: TextSize,
    text: &str,
    definitions: &crate::semantic::SignatureDb,
) -> Option<Option<(usize, String)>> {
    let structural: Vec<&SyntaxElement> = row
        .iter()
        .filter(|element| !is_table_trivia(element) && !is_rule_command(element, definitions))
        .collect();
    if structural.is_empty() {
        return Some(None);
    }

    let mut cell = Vec::new();
    let mut used = 0usize;
    for element in &structural {
        if element.kind() == SyntaxKind::AMPERSAND {
            used = used.checked_add(table_cell_span(&cell)?)?;
            cell.clear();
        } else {
            cell.push(*element);
        }
    }
    used = used.checked_add(table_cell_span(&cell)?)?;
    if used != columns {
        return None;
    }

    let anchor = structural.last()?.text_range().end();
    let gap = text.get(usize::from(anchor)..usize::from(boundary))?;
    if !gap.chars().all(char::is_whitespace) {
        return None;
    }
    let content = if gap.is_empty() { " & " } else { " &" };
    Some(Some((usize::from(anchor), content.to_string())))
}

fn table_cell_span(cell: &[&SyntaxElement]) -> Option<usize> {
    let commands: Vec<Command> = cell
        .iter()
        .filter_map(|element| element.as_node())
        .filter_map(|node| Command::cast(node.clone()))
        .filter(|command| command.name().as_deref() == Some("multicolumn"))
        .collect();
    if commands.is_empty() {
        return Some(1);
    }
    if cell.len() != 1 || commands.len() != 1 {
        return None;
    }
    commands[0]
        .nth_group_text(0)?
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|&span| span > 0)
}

fn is_table_trivia(element: &SyntaxElement) -> bool {
    element
        .as_token()
        .is_some_and(|token| matches!(token.kind(), SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE))
}

fn is_rule_command(element: &SyntaxElement, definitions: &crate::semantic::SignatureDb) -> bool {
    let Some(command) = element
        .as_node()
        .and_then(|node| Command::cast(node.clone()))
    else {
        return false;
    };
    command.name().is_some_and(|name| {
        definitions.command(&name).is_none()
            && crate::semantic::signature::builtin()
                .command(&name)
                .is_some_and(|signature| signature.rule)
    })
}

/// Group a fix's edits into a `WorkspaceEdit`'s per-URI `TextEdit` lists.
///
/// `None` edits map through `self_idx` onto `self_uri` (the finding's own file);
/// `Some(p)` edits are converted through a `LineIndex` built from `p`'s resolved
/// text. Returns `None` — dropping the whole action — if any foreign file fails to
/// resolve, so a cross-file fix is never offered half-applied.
fn workspace_changes(
    fix: &crate::linter::Fix,
    self_uri: &Uri,
    self_idx: &LineIndex,
    enc: PositionEncoding,
    resolve: &dyn Fn(&Path) -> Option<(Uri, String)>,
) -> Option<HashMap<Uri, Vec<TextEdit>>> {
    // Per-request cache of resolved foreign files: path -> (uri, text, table).
    // The text is kept beside its table because an index borrows what it
    // indexes, and here that is the entry's own field.
    let mut foreign: HashMap<PathBuf, (Uri, String, LineTable)> = HashMap::new();
    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    for e in &fix.edits {
        let (target_uri, edit) = match &e.path {
            None => (
                self_uri.clone(),
                TextEdit {
                    range: byte_range_to_lsp(self_idx, e.start, e.end),
                    new_text: e.content.clone(),
                },
            ),
            Some(p) => {
                if !foreign.contains_key(p) {
                    let (u, txt) = resolve(p)?;
                    let table = LineTable::new(&txt);
                    foreign.insert(p.clone(), (u, txt, table));
                }
                let (u, txt, table) = &foreign[p];
                let fidx = LineIndex::with_table(txt, table, enc);
                (
                    u.clone(),
                    TextEdit {
                        range: byte_range_to_lsp(&fidx, e.start, e.end),
                        new_text: e.content.clone(),
                    },
                )
            }
        };
        changes.entry(target_uri).or_default().push(edit);
    }
    Some(changes)
}

/// Whether two byte ranges overlap, inclusive at the edges (a zero-width cursor at a
/// range boundary counts as touching it).
fn byte_ranges_overlap(a_start: usize, a_end: usize, b_start: usize, b_end: usize) -> bool {
    a_start <= b_end && b_start <= a_end
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linter::check_document;
    use crate::parser::LexConfig;

    fn uri() -> Uri {
        "file:///x.tex".parse().unwrap()
    }

    fn full_range(text: &str) -> Range {
        let idx = LineIndex::new(text);
        let (el, ec) = idx.position(text.len());
        Range {
            start: Position::new(0, 0),
            end: Position::new(el, ec),
        }
    }

    /// Lint `src` as a `.tex` document and return the raw findings.
    fn findings(src: &str) -> Vec<crate::linter::Diagnostic> {
        check_document(
            std::path::Path::new("x.tex"),
            &buf(src),
            LexConfig::default(),
            &crate::declarations::ResolvedDeclarations::default(),
        )
    }

    /// The document buffer the entry point takes, over a test source.
    fn buf(src: &str) -> TextBuffer {
        TextBuffer::new(src, PositionEncoding::Utf16)
    }

    /// A resolver that knows no foreign files — the single-file default.
    fn no_resolve(_: &Path) -> Option<(Uri, String)> {
        None
    }

    #[test]
    fn offers_quickfix_for_deprecated_command_in_range() {
        let src = "{\\bf hi}\n";
        let actions = code_actions_for_range(
            &findings(src),
            &buf(src),
            &uri(),
            std::path::Path::new("x.tex"),
            full_range(src),
            PositionEncoding::Utf16,
            true,
            &no_resolve,
        );
        let CodeActionOrCommand::CodeAction(action) = actions
            .iter()
            .find(
                |a| matches!(a, CodeActionOrCommand::CodeAction(a) if a.title.contains("bfseries")),
            )
            .expect("a `\\bf` → `\\bfseries` quick-fix")
        else {
            unreachable!()
        };
        assert_eq!(action.kind, Some(CodeActionKind::QUICKFIX));
        assert_eq!(action.is_preferred, Some(true));
        let edits = action
            .edit
            .as_ref()
            .and_then(|e| e.changes.as_ref())
            .and_then(|c| c.get(&uri()))
            .expect("a single-file edit");
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "\\bfseries");
        // The edit replaces just `\bf` (line 0, chars 1..4).
        assert_eq!(edits[0].range.start, Position::new(0, 1));
        assert_eq!(edits[0].range.end, Position::new(0, 4));
    }

    #[test]
    fn one_quickfix_repairs_a_whole_quotation() {
        // A straight-quote pair is one finding carrying both edits, so the editor
        // offers a single action -- with the caret on either quote -- that rewrites
        // the quotation in one step, instead of one action per quote.
        let src = "He said \"hi\" now\n";
        for caret in [9, 11] {
            let cursor = Range {
                start: Position::new(0, caret),
                end: Position::new(0, caret),
            };
            let actions = code_actions_for_range(
                &findings(src),
                &buf(src),
                &uri(),
                std::path::Path::new("x.tex"),
                cursor,
                PositionEncoding::Utf16,
                true,
                &no_resolve,
            );
            let quotes: Vec<_> = actions
                .iter()
                .filter_map(|a| match a {
                    CodeActionOrCommand::CodeAction(a) if a.title.contains("straight quotes") => {
                        Some(a)
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(quotes.len(), 1, "caret {caret}: {actions:?}");
            let edits = quotes[0]
                .edit
                .as_ref()
                .and_then(|e| e.changes.as_ref())
                .and_then(|c| c.get(&uri()))
                .expect("a single-file edit");
            assert_eq!(edits.len(), 2);
            assert_eq!(edits[0].new_text, "``");
            assert_eq!(edits[0].range.start, Position::new(0, 8));
            assert_eq!(edits[1].new_text, "''");
            assert_eq!(edits[1].range.start, Position::new(0, 11));
            // Direction is inferred, so the fix stays unsafe and never preferred.
            assert_eq!(quotes[0].is_preferred, Some(false));
        }
    }

    #[test]
    fn empty_when_range_misses_the_finding() {
        let src = "ok\n{\\bf hi}\n";
        // A zero-width cursor on line 0 (the `ok` prose), nowhere near `\bf`.
        let cursor = Range {
            start: Position::new(0, 0),
            end: Position::new(0, 0),
        };
        let actions = code_actions_for_range(
            &findings(src),
            &buf(src),
            &uri(),
            std::path::Path::new("x.tex"),
            cursor,
            PositionEncoding::Utf16,
            true,
            &no_resolve,
        );
        assert!(actions.is_empty());
    }

    #[test]
    fn surfaces_dollar_display_math_fix() {
        let src = "$$x = y$$\n";
        let actions = code_actions_for_range(
            &findings(src),
            &buf(src),
            &uri(),
            std::path::Path::new("x.tex"),
            full_range(src),
            PositionEncoding::Utf16,
            true,
            &no_resolve,
        );
        assert!(actions.iter().any(|a| matches!(
            a,
            CodeActionOrCommand::CodeAction(a) if a.title.contains("\\[")
        )));
    }

    /// A synthetic cross-file fix (an `Edit::in_file` into another buffer) becomes
    /// a single `WorkspaceEdit` spanning both URIs when the foreign file resolves.
    #[test]
    fn cross_file_fix_spans_two_uris() {
        use crate::linter::{Diagnostic, Edit, Fix, Severity};

        let other_uri: Uri = "file:///other.tex".parse().unwrap();
        let other_text = "\\ref{x}\n";
        let resolve = |p: &Path| -> Option<(Uri, String)> {
            (p == Path::new("other.tex")).then(|| (other_uri.clone(), other_text.to_string()))
        };

        // Own-file finding on `\label{x}` at 7..8, plus a cross-file edit renaming
        // the `\ref{x}` key (5..6) in other.tex.
        let src = "\\label{x}\n";
        let d = Diagnostic {
            rule: "synthetic",
            severity: Severity::Warning,
            path: std::path::PathBuf::from("x.tex"),
            start: 7,
            end: 8,
            message: "rename x".into(),
            fix: Some(Fix::safe_edits(
                vec![
                    Edit::new(7, 8, "y"),
                    Edit::in_file(PathBuf::from("other.tex"), 5, 6, "y"),
                ],
                "rename x to y",
            )),
            related: Vec::new(),
        };

        let actions = code_actions_for_range(
            std::slice::from_ref(&d),
            &buf(src),
            &uri(),
            std::path::Path::new("x.tex"),
            full_range(src),
            PositionEncoding::Utf16,
            true,
            &resolve,
        );
        let CodeActionOrCommand::CodeAction(action) = actions
            .iter()
            .find(|a| matches!(a, CodeActionOrCommand::CodeAction(a) if a.title == "rename x to y"))
            .expect("the cross-file quick-fix")
        else {
            unreachable!()
        };
        let changes = action
            .edit
            .as_ref()
            .and_then(|e| e.changes.as_ref())
            .expect("workspace changes");
        assert_eq!(changes.len(), 2, "one entry per touched file");
        assert_eq!(changes[&uri()][0].new_text, "y");
        assert_eq!(changes[&other_uri][0].new_text, "y");
        // The foreign edit's range is computed from other.tex, not the buffer.
        assert_eq!(changes[&other_uri][0].range.start, Position::new(0, 5));
    }

    /// If a cross-file fix's foreign target can't be resolved, the whole action is
    /// dropped — never a half-applied edit.
    #[test]
    fn cross_file_fix_dropped_when_target_unresolved() {
        use crate::linter::{Diagnostic, Edit, Fix, Severity};

        let src = "\\label{x}\n";
        let d = Diagnostic {
            rule: "synthetic",
            severity: Severity::Warning,
            path: std::path::PathBuf::from("x.tex"),
            start: 7,
            end: 8,
            message: "rename x".into(),
            fix: Some(Fix::safe_edits(
                vec![
                    Edit::new(7, 8, "y"),
                    Edit::in_file(PathBuf::from("gone.tex"), 5, 6, "y"),
                ],
                "rename x to y",
            )),
            related: Vec::new(),
        };
        let actions = code_actions_for_range(
            std::slice::from_ref(&d),
            &buf(src),
            &uri(),
            std::path::Path::new("x.tex"),
            full_range(src),
            PositionEncoding::Utf16,
            true,
            &no_resolve,
        );
        assert!(
            !actions.iter().any(|a| matches!(
                a,
                CodeActionOrCommand::CodeAction(a) if a.title == "rename x to y"
            )),
            "unresolved cross-file fix must not be offered"
        );
    }

    fn apply_table_action(src: &str, line: u32, character: u32) -> Option<(CodeAction, String)> {
        let root = SyntaxNode::new_root(crate::parser::parse(src).green);
        let buffer = buf(src);
        let cursor = Range::new(
            Position::new(line, character),
            Position::new(line, character),
        );
        let mut actions = table_column_actions(&root, &buffer, &uri(), cursor);
        let CodeActionOrCommand::CodeAction(action) = actions.pop()? else {
            return None;
        };
        let edits = action.edit.as_ref()?.changes.as_ref()?.get(&uri())?;
        let idx = buffer.line_index();
        let mut byte_edits: Vec<_> = edits
            .iter()
            .map(|edit| {
                let start = idx.offset_at(edit.range.start.line, edit.range.start.character);
                let end = idx.offset_at(edit.range.end.line, edit.range.end.character);
                (start, end, edit.new_text.as_str())
            })
            .collect();
        byte_edits.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
        let mut output = src.to_string();
        for (start, end, replacement) in byte_edits {
            output.replace_range(start..end, replacement);
        }
        Some((action, output))
    }

    #[test]
    fn adds_a_trailing_column_to_every_table_row() {
        let src = "\\begin{tabular}{lr}\n  a & b \\\\\n  c & d \\\\\n\\end{tabular}\n";
        let (action, output) = apply_table_action(src, 1, 3).expect("table refactor");
        assert_eq!(action.title, "Add column at end");
        assert_eq!(action.kind, Some(CodeActionKind::REFACTOR_REWRITE));
        assert_eq!(
            output,
            "\\begin{tabular}{lrc}\n  a & b & \\\\\n  c & d & \\\\\n\\end{tabular}\n"
        );
    }

    #[test]
    fn adds_a_column_to_an_unterminated_final_row() {
        let src = "\\begin{tabular}{cc}\n  a & b\n\\end{tabular}\n";
        let (_, output) = apply_table_action(src, 1, 3).expect("table refactor");
        assert_eq!(output, "\\begin{tabular}{ccc}\n  a & b &\n\\end{tabular}\n");
    }

    #[test]
    fn declines_short_rows_and_dynamic_spans() {
        let short = "\\begin{tabular}{ccc}\n  a & b \\\\\n\\end{tabular}\n";
        assert!(apply_table_action(short, 1, 3).is_none());

        let dynamic = "\\begin{tabular}{cc}\n  \\multicolumn{\\n}{c}{a} \\\\\n\\end{tabular}\n";
        assert!(apply_table_action(dynamic, 1, 3).is_none());
    }

    #[test]
    fn handles_static_multicolumn_rows_and_rule_lines() {
        let src = "\\begin{tabular}{cc}\n  \\toprule\n  \\multicolumn{2}{c}{heading} \\\\\n  a & b \\\\\n  \\bottomrule\n\\end{tabular}\n";
        let (_, output) = apply_table_action(src, 3, 3).expect("table refactor");
        assert_eq!(
            output,
            "\\begin{tabular}{ccc}\n  \\toprule\n  \\multicolumn{2}{c}{heading} & \\\\\n  a & b & \\\\\n  \\bottomrule\n\\end{tabular}\n"
        );
    }

    #[test]
    fn declines_custom_preambles_redefinitions_and_outside_cursors() {
        let custom = "\\begin{tabular}{XX}\n  a & b \\\\\n\\end{tabular}\n";
        assert!(apply_table_action(custom, 1, 3).is_none());

        let redefined =
            "\\renewenvironment{tabular}[1]{}{}\n\\begin{tabular}{c}\n  a \\\\\n\\end{tabular}\n";
        assert!(apply_table_action(redefined, 2, 3).is_none());

        let outside = "prose\n\\begin{tabular}{c}\n  a \\\\\n\\end{tabular}\n";
        assert!(apply_table_action(outside, 0, 2).is_none());
    }

    #[test]
    fn covers_tabular_star_and_array_but_declines_commented_tables() {
        let tabular_star = "\\begin{tabular*}{4cm}{c}\n  a \\\\\n\\end{tabular*}\n";
        let (_, output) = apply_table_action(tabular_star, 1, 3).expect("tabular* refactor");
        assert_eq!(
            output,
            "\\begin{tabular*}{4cm}{cc}\n  a & \\\\\n\\end{tabular*}\n"
        );

        let array = "\\begin{array}{c}\n  a \\\\\n\\end{array}\n";
        let (_, output) = apply_table_action(array, 1, 3).expect("array refactor");
        assert_eq!(output, "\\begin{array}{cc}\n  a & \\\\\n\\end{array}\n");

        let commented = "\\begin{tabular}{c}\n  a \\\\ % why\n  b \\\\\n\\end{tabular}\n";
        assert!(apply_table_action(commented, 1, 3).is_none());
    }
}
