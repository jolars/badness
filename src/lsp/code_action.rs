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
use std::path::PathBuf;

use super::*;
use crate::linter::diagnostic::Applicability;
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
    text: &str,
    uri: &Uri,
    self_path: &Path,
    request_range: Range,
    enc: PositionEncoding,
    link_docs: bool,
    resolve: &dyn Fn(&Path) -> Option<(Uri, String)>,
) -> CodeActionResponse {
    let idx = LineIndex::with_encoding(text, enc);
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
    // Per-request cache of resolved foreign files: path -> (uri, line index).
    let mut foreign: HashMap<PathBuf, (Uri, LineIndex)> = HashMap::new();
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
                    foreign.insert(p.clone(), (u, LineIndex::with_encoding(&txt, enc)));
                }
                let (u, fidx) = &foreign[p];
                (
                    u.clone(),
                    TextEdit {
                        range: byte_range_to_lsp(fidx, e.start, e.end),
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
        check_document(std::path::Path::new("x.tex"), src, LexConfig::default())
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
            src,
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
            &no_resolve,
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
            src,
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
            src,
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
}
