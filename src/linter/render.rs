//! Diagnostic rendering for the CLI: pretty (annotate-snippets) and concise.
//!
//! Only the two text modes that matter today are implemented (JSON is
//! deferred). Diagnostics are grouped by file so each file's source is fetched
//! at most once.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use annotate_snippets::{AnnotationKind, Level, Renderer, Snippet};

use crate::text::LineIndex;

use super::diagnostic::{Diagnostic, RelatedInfo, Severity};

/// How diagnostics are rendered to the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputMode {
    /// Source-snippet output with a caret span, via `annotate-snippets`.
    #[default]
    Pretty,
    /// One `path:line:col: severity [rule] message` line per finding.
    Concise,
}

/// Render `diagnostics` to a string. `source_for` supplies the source text of a
/// file (used for snippets and line/column lookup); returning `None` falls back
/// to a concise, location-only line for that file.
pub fn render_findings(
    diagnostics: &[Diagnostic],
    mode: OutputMode,
    source_for: &dyn Fn(&Path) -> Option<String>,
) -> String {
    match mode {
        OutputMode::Pretty => render_pretty(diagnostics, source_for),
        OutputMode::Concise => render_concise(diagnostics, source_for),
    }
}

/// Group diagnostics by path, preserving their original order within each file.
fn group_by_path(diagnostics: &[Diagnostic]) -> BTreeMap<&PathBuf, Vec<&Diagnostic>> {
    let mut by_path: BTreeMap<&PathBuf, Vec<&Diagnostic>> = BTreeMap::new();
    for d in diagnostics {
        by_path.entry(&d.path).or_default().push(d);
    }
    by_path
}

fn render_pretty(
    diagnostics: &[Diagnostic],
    source_for: &dyn Fn(&Path) -> Option<String>,
) -> String {
    let renderer = Renderer::plain();
    let mut out = String::new();
    for (path, diags) in group_by_path(diagnostics) {
        let Some(source) = source_for(path) else {
            // No source: fall back to concise, location-less lines.
            for d in &diags {
                let _ = writeln!(out, "{}", concise_line(path, None, d));
            }
            continue;
        };
        let origin = path.display().to_string();
        for d in &diags {
            let level = severity_level(d.severity);
            let span = clamp_span(&source, d.start, d.end);
            // A secondary in *this* file rides the primary snippet as a context
            // annotation; one in another file needs that file's source loaded
            // (once each), kept alive in `extra` through the render call.
            let (same_file, cross): (Vec<&RelatedInfo>, Vec<&RelatedInfo>) = d
                .related
                .iter()
                .partition(|ri| ri.path.as_path() == path.as_path());
            let extra: Vec<(String, String, &RelatedInfo)> = cross
                .iter()
                .filter_map(|ri| {
                    let src = source_for(&ri.path)?;
                    Some((ri.path.display().to_string(), src, *ri))
                })
                .collect();

            let mut snippet = Snippet::source(&source)
                .path(&origin)
                .annotation(AnnotationKind::Primary.span(span).label(&d.message));
            for ri in &same_file {
                let s = clamp_span(&source, ri.start, ri.end);
                snippet = snippet.annotation(AnnotationKind::Context.span(s).label(&ri.message));
            }
            let mut group = level.primary_title(d.rule).element(snippet);
            for (origin2, src2, ri) in &extra {
                let s = clamp_span(src2, ri.start, ri.end);
                let secondary = Snippet::source(src2)
                    .path(origin2.as_str())
                    .annotation(AnnotationKind::Context.span(s).label(ri.message.as_str()));
                group = group.element(secondary);
            }
            let _ = writeln!(out, "{}", renderer.render(&[group]));
        }
    }
    out
}

fn render_concise(
    diagnostics: &[Diagnostic],
    source_for: &dyn Fn(&Path) -> Option<String>,
) -> String {
    let mut out = String::new();
    for (path, diags) in group_by_path(diagnostics) {
        let source = source_for(path);
        let index = source.as_deref().map(|s| (s, LineIndex::new(s)));
        for d in &diags {
            let resolved = index.as_ref().map(|(s, idx)| (*s, idx));
            let _ = writeln!(out, "{}", concise_line(path, resolved, d));
        }
    }
    out
}

/// `path:line:col: severity [rule] message`, or `path: …` when no source is
/// available to resolve line/column.
fn concise_line(path: &Path, source: Option<(&str, &LineIndex)>, d: &Diagnostic) -> String {
    let severity = severity_word(d.severity);
    match source {
        Some((text, index)) => {
            let lc = index.line_col(text, d.start);
            format!(
                "{}:{}:{}: {severity} [{}] {}",
                path.display(),
                lc.line,
                lc.column,
                d.rule,
                d.message,
            )
        }
        None => format!("{}: {severity} [{}] {}", path.display(), d.rule, d.message),
    }
}

/// Keep the annotation span within the source bounds; `annotate-snippets`
/// panics on out-of-range or inverted spans.
fn clamp_span(source: &str, start: usize, end: usize) -> std::ops::Range<usize> {
    let len = source.len();
    let start = start.min(len);
    let end = end.clamp(start, len);
    start..end
}

fn severity_level(s: Severity) -> Level<'static> {
    match s {
        Severity::Error => Level::ERROR,
        Severity::Warning => Level::WARNING,
        Severity::Info => Level::INFO,
        Severity::Hint => Level::HELP,
    }
}

fn severity_word(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
        Severity::Hint => "hint",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diag(start: usize, end: usize, message: &str) -> Diagnostic {
        Diagnostic {
            rule: "parse",
            severity: Severity::Error,
            path: PathBuf::from("x.tex"),
            start,
            end,
            message: message.to_owned(),
            fix: None,
            related: Vec::new(),
        }
    }

    #[test]
    fn concise_resolves_line_and_column() {
        let source = "\\foo\n\\bar{".to_owned();
        let diags = [diag(9, 10, "expected '}'")];
        let rendered = render_findings(&diags, OutputMode::Concise, &|_| Some(source.clone()));
        assert_eq!(rendered, "x.tex:2:5: error [parse] expected '}'\n");
    }

    #[test]
    fn concise_without_source_omits_location() {
        let diags = [diag(0, 1, "boom")];
        let rendered = render_findings(&diags, OutputMode::Concise, &|_| None);
        assert_eq!(rendered, "x.tex: error [parse] boom\n");
    }

    #[test]
    fn pretty_includes_message_and_origin() {
        let source = "\\foo{bar\n".to_owned();
        let diags = [diag(4, 5, "unclosed group")];
        let rendered = render_findings(&diags, OutputMode::Pretty, &|_| Some(source.clone()));
        assert!(rendered.contains("unclosed group"), "got: {rendered}");
        assert!(rendered.contains("x.tex"), "got: {rendered}");
    }

    #[test]
    fn pretty_renders_same_file_related_as_context() {
        // A related location in the same file rides the primary snippet as a
        // second (context) annotation.
        let source = "\\label{a}\\label{a}\n".to_owned();
        let mut d = diag(9, 18, "label `a` is defined more than once");
        d.related.push(RelatedInfo {
            path: PathBuf::from("x.tex"),
            start: 7,
            end: 8,
            message: "first definition of `a`".to_owned(),
        });
        let rendered = render_findings(&[d], OutputMode::Pretty, &|_| Some(source.clone()));
        assert!(
            rendered.contains("defined more than once"),
            "got: {rendered}"
        );
        assert!(
            rendered.contains("first definition of `a`"),
            "got: {rendered}"
        );
    }

    #[test]
    fn pretty_renders_cross_file_related_as_second_snippet() {
        // A related location in another file becomes a secondary snippet, whose
        // source is fetched through `source_for`.
        let main = "\\label{dup}\\ref{dup}\n".to_owned();
        let chap = "\\label{dup}\n".to_owned();
        let mut d = diag(0, 11, "label `dup` is also defined in `chap.tex`");
        d.path = PathBuf::from("main.tex");
        d.related.push(RelatedInfo {
            path: PathBuf::from("chap.tex"),
            start: 0,
            end: 0,
            message: "other definition of `dup`".to_owned(),
        });
        let rendered = render_findings(&[d], OutputMode::Pretty, &|p| match p.to_str() {
            Some("main.tex") => Some(main.clone()),
            Some("chap.tex") => Some(chap.clone()),
            _ => None,
        });
        assert!(rendered.contains("main.tex"), "got: {rendered}");
        assert!(rendered.contains("chap.tex"), "got: {rendered}");
        assert!(
            rendered.contains("other definition of `dup`"),
            "got: {rendered}"
        );
    }
}
