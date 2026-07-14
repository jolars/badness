//! Pure code-action logic: turn fix-carrying linter findings into LSP quick-fixes.
//!
//! The threading side ([`super::run_code_action`]) re-lints the buffer off a fresh
//! snapshot (like the pull-diagnostics path) and hands the raw findings here. This
//! module is rule-agnostic: any finding whose [`crate::linter::Diagnostic::fix`] is
//! populated and whose caret overlaps the requested range becomes a `QUICKFIX`.
//!
//! Fully-built actions are returned (no
//! `codeAction/resolve` step), and a fix's byte span maps straight to a `TextEdit`
//! via the shared [`super::byte_range_to_lsp`] — the fix owns *what* to rewrite,
//! never *how* to lay it out (tenet 1).

use std::collections::HashMap;

use super::*;
use crate::linter::diagnostic::Applicability;
use lsp_types::{CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionResponse};

/// Build the quick-fix actions for the findings overlapping `request_range`.
///
/// A finding is offered when it carries a `fix` and its diagnostic span overlaps the
/// request range (inclusive at the edges, so a zero-width cursor sitting on `\bf`
/// matches). The edit replaces the fix's byte span verbatim; `Safe` fixes are marked
/// `is_preferred`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn code_actions_for_range(
    findings: &[crate::linter::Diagnostic],
    text: &str,
    uri: &Uri,
    self_path: &Path,
    request_range: Range,
    enc: PositionEncoding,
    link_docs: bool,
) -> CodeActionResponse {
    let idx = LineIndex::with_encoding(text, enc);
    let req_start = idx.offset_at(
        text,
        request_range.start.line,
        request_range.start.character,
    );
    let req_end = idx.offset_at(text, request_range.end.line, request_range.end.character);

    findings
        .iter()
        .filter_map(|d| {
            let fix = d.fix.as_ref()?;
            // Offer the action only when the finding's caret touches the request.
            if !byte_ranges_overlap(d.start, d.end, req_start, req_end) {
                return None;
            }
            let edits: Vec<TextEdit> = fix
                .edits
                .iter()
                .map(|e| TextEdit {
                    range: byte_range_to_lsp(&idx, text, e.start, e.end),
                    new_text: e.content.clone(),
                })
                .collect();
            let changes = HashMap::from([(uri.clone(), edits)]);
            Some(CodeActionOrCommand::CodeAction(CodeAction {
                title: fix.description.clone(),
                kind: Some(CodeActionKind::QUICKFIX),
                // Link the action to the finding it fixes, so the client can dim it
                // once the diagnostic clears.
                diagnostics: Some(vec![lint_to_lsp(
                    &idx,
                    text,
                    d.clone(),
                    link_docs,
                    self_path,
                )]),
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
        let (el, ec) = idx.position(text, text.len());
        Range {
            start: Position::new(0, 0),
            end: Position::new(el, ec),
        }
    }

    /// Lint `src` as a `.tex` document and return the raw findings.
    fn findings(src: &str) -> Vec<crate::linter::Diagnostic> {
        check_document(std::path::Path::new("x.tex"), src, LexConfig::default())
    }

    #[test]
    fn offers_quickfix_for_deprecated_command_in_range() {
        let src = "{\\bf hi}\n";
        let actions = code_actions_for_range(
            &findings(src),
            src,
            &uri(),
            std::path::Path::new("x.tex"),
            full_range(src),
            PositionEncoding::Utf16,
            true,
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
    fn empty_when_range_misses_the_finding() {
        let src = "ok\n{\\bf hi}\n";
        // A zero-width cursor on line 0 (the `ok` prose), nowhere near `\bf`.
        let cursor = Range {
            start: Position::new(0, 0),
            end: Position::new(0, 0),
        };
        let actions = code_actions_for_range(
            &findings(src),
            src,
            &uri(),
            std::path::Path::new("x.tex"),
            cursor,
            PositionEncoding::Utf16,
            true,
        );
        assert!(actions.is_empty());
    }

    #[test]
    fn surfaces_dollar_display_math_fix() {
        let src = "$$x = y$$\n";
        let actions = code_actions_for_range(
            &findings(src),
            src,
            &uri(),
            std::path::Path::new("x.tex"),
            full_range(src),
            PositionEncoding::Utf16,
            true,
        );
        assert!(actions.iter().any(|a| matches!(
            a,
            CodeActionOrCommand::CodeAction(a) if a.title.contains("\\[")
        )));
    }
}
