//! Diagnostic rendering for the CLI: pretty (annotate-snippets), concise, and
//! machine-readable JSON.
//!
//! For the text modes, diagnostics are grouped by file so each file's source is
//! fetched at most once. JSON is a faithful serialization of the diagnostic
//! model (byte offsets, no line/column resolution), so it needs no source.

use std::collections::{BTreeMap, HashMap};
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
    /// A JSON array of findings with byte-offset ranges and fix data.
    Json,
}

/// Render `diagnostics` to a string. `source_for` supplies the source text of a
/// file (used for snippets and line/column lookup); returning `None` falls back
/// to a concise, location-only line for that file.
///
/// `use_color` bears on `Pretty` only: `Concise` is the compact one-liner that
/// callers grep and cut, and `Json` is machine-readable, so neither ever carries
/// ANSI (matching arity and fatou). The caller resolves it against the
/// destination stream — for the CLI that is *stderr*, where the text modes go.
pub fn render_findings(
    diagnostics: &[Diagnostic],
    mode: OutputMode,
    use_color: bool,
    source_for: &dyn Fn(&Path) -> Option<String>,
) -> String {
    match mode {
        OutputMode::Pretty => render_pretty(diagnostics, use_color, source_for),
        OutputMode::Concise => render_concise(diagnostics, source_for),
        OutputMode::Json => render_json(diagnostics),
    }
}

/// Serialize the findings as a pretty-printed JSON array (no trailing newline).
/// An empty slice renders as `[]`, so consumers always receive valid JSON.
fn render_json(diagnostics: &[Diagnostic]) -> String {
    serde_json::to_string_pretty(diagnostics).unwrap_or_else(|_| "[]".to_string())
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
    use_color: bool,
    source_for: &dyn Fn(&Path) -> Option<String>,
) -> String {
    let renderer = if use_color {
        Renderer::styled()
    } else {
        Renderer::plain()
    };
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
        let index = LineIndex::new(&source);
        // Sources of *other* files, pulled in only by a cross-file secondary and
        // kept for the rest of this file's diagnostics: `source_for` clones the
        // whole text on every call, and a rule like `duplicate-label` reports the
        // same partner file once per finding.
        let mut cross_sources: HashMap<PathBuf, (String, LineIndex)> = HashMap::new();
        for d in &diags {
            let level = severity_level(d.severity);
            // A secondary in *this* file rides the primary snippet as a context
            // annotation; one in another file needs that file's source loaded
            // (once each), kept alive in `extra` through the render call.
            let (same_file, cross): (Vec<&RelatedInfo>, Vec<&RelatedInfo>) = d
                .related
                .iter()
                .partition(|ri| ri.path.as_path() == path.as_path());
            for ri in &cross {
                if !cross_sources.contains_key(&ri.path)
                    && let Some(src) = source_for(&ri.path)
                {
                    let idx = LineIndex::new(&src);
                    cross_sources.insert(ri.path.clone(), (src, idx));
                }
            }
            let extra: Vec<(String, &str, Window, &RelatedInfo)> = cross
                .iter()
                .filter_map(|ri| {
                    let (src, idx) = cross_sources.get(&ri.path)?;
                    let win = Window::around(src, idx, [(ri.start, ri.end)]);
                    Some((ri.path.display().to_string(), src.as_str(), win, *ri))
                })
                .collect();

            // The window must cover the primary span *and* every same-file
            // secondary, since they share one snippet.
            let win = Window::around(
                &source,
                &index,
                std::iter::once((d.start, d.end))
                    .chain(same_file.iter().map(|ri| (ri.start, ri.end))),
            );
            let mut snippet = Snippet::source(win.slice(&source))
                .line_start(win.line_start)
                .path(&origin)
                .annotation(
                    AnnotationKind::Primary
                        .span(win.span(d.start, d.end))
                        .label(&d.message),
                );
            for ri in &same_file {
                snippet = snippet.annotation(
                    AnnotationKind::Context
                        .span(win.span(ri.start, ri.end))
                        .label(&ri.message),
                );
            }
            let mut group = level.primary_title(d.rule).element(snippet);
            for (origin2, src2, win2, ri) in &extra {
                let secondary = Snippet::source(win2.slice(src2))
                    .line_start(win2.line_start)
                    .path(origin2.as_str())
                    .annotation(
                        AnnotationKind::Context
                            .span(win2.span(ri.start, ri.end))
                            .label(ri.message.as_str()),
                    );
                group = group.element(secondary);
            }
            let _ = writeln!(out, "{}", renderer.render(&[group]));
        }
    }
    out
}

/// The slice of a source a single rendered snippet is built from: the lines its
/// annotations touch, plus one line of padding each side.
///
/// Handing `annotate-snippets` the whole file rebuilds an O(file) source map on
/// *every* `render()` call, so a file's worth of findings costs O(findings x
/// file length). Slicing to the annotated lines and anchoring the gutter with
/// `Snippet::line_start` makes each render O(window) instead. Output is
/// unchanged: snippets fold by default, and folding retains only lines carrying
/// an annotation, so the padding is never printed and the gutter width still
/// comes from the absolute line numbers.
struct Window {
    /// Byte offset the slice starts at, and the base every span rebases against.
    start: usize,
    /// Byte offset the slice ends at.
    end: usize,
    /// 1-indexed absolute line number of the slice's first line.
    line_start: usize,
}

impl Window {
    /// The window covering every span in `spans` (which must be non-empty).
    fn around(
        source: &str,
        index: &LineIndex,
        spans: impl IntoIterator<Item = (usize, usize)>,
    ) -> Self {
        let len = source.len();
        let mut lo = len;
        let mut hi = 0;
        for (start, end) in spans {
            lo = lo.min(start.min(len));
            hi = hi.max(end.min(len));
        }
        let hi = hi.max(lo);
        // 0-indexed lines, one line of padding each side; folding drops it.
        let first = index.line_col(lo).line.saturating_sub(2);
        let last = index.line_col(hi).line;
        Self {
            start: index.line_start(first),
            end: index.line_start(last + 1),
            line_start: first + 1,
        }
    }

    fn slice<'a>(&self, source: &'a str) -> &'a str {
        &source[self.start..self.end]
    }

    /// `start..end`, rebased into the slice and clamped to it —
    /// `annotate-snippets` panics on an out-of-range or inverted span.
    fn span(&self, start: usize, end: usize) -> std::ops::Range<usize> {
        let start = start.clamp(self.start, self.end) - self.start;
        let end = end.clamp(self.start, self.end) - self.start;
        start..end.max(start)
    }
}

fn render_concise(
    diagnostics: &[Diagnostic],
    source_for: &dyn Fn(&Path) -> Option<String>,
) -> String {
    let mut out = String::new();
    for (path, diags) in group_by_path(diagnostics) {
        let source = source_for(path);
        let index = source.as_deref().map(LineIndex::new);
        for d in &diags {
            let _ = writeln!(out, "{}", concise_line(path, index.as_ref(), d));
        }
    }
    out
}

/// `path:line:col: severity [rule] message`, or `path: …` when no source is
/// available to resolve line/column.
fn concise_line(path: &Path, index: Option<&LineIndex>, d: &Diagnostic) -> String {
    let severity = severity_word(d.severity);
    match index {
        Some(index) => {
            let lc = index.line_col(d.start);
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
        let rendered = render_findings(&diags, OutputMode::Concise, false, &|_| {
            Some(source.clone())
        });
        assert_eq!(rendered, "x.tex:2:5: error [parse] expected '}'\n");
    }

    #[test]
    fn concise_without_source_omits_location() {
        let diags = [diag(0, 1, "boom")];
        let rendered = render_findings(&diags, OutputMode::Concise, false, &|_| None);
        assert_eq!(rendered, "x.tex: error [parse] boom\n");
    }

    #[test]
    fn pretty_includes_message_and_origin() {
        let source = "\\foo{bar\n".to_owned();
        let diags = [diag(4, 5, "unclosed group")];
        let rendered =
            render_findings(&diags, OutputMode::Pretty, false, &|_| Some(source.clone()));
        assert!(rendered.contains("unclosed group"), "got: {rendered}");
        assert!(rendered.contains("x.tex"), "got: {rendered}");
    }

    #[test]
    fn pretty_styles_only_when_color_is_asked_for() {
        let source = "\\foo{bar\n".to_owned();
        let diags = [diag(4, 5, "unclosed group")];
        let plain = render_findings(&diags, OutputMode::Pretty, false, &|_| Some(source.clone()));
        let styled = render_findings(&diags, OutputMode::Pretty, true, &|_| Some(source.clone()));
        assert!(!plain.contains('\x1b'), "got: {plain:?}");
        assert!(styled.contains('\x1b'), "got: {styled:?}");
    }

    #[test]
    fn concise_and_json_stay_plain_under_color() {
        // Both are consumed by tools, not read in a terminal, so `use_color` is
        // inert for them.
        let source = "\\foo\n\\bar{".to_owned();
        let diags = [diag(9, 10, "expected '}'")];
        let concise = render_findings(&diags, OutputMode::Concise, true, &|_| Some(source.clone()));
        assert_eq!(concise, "x.tex:2:5: error [parse] expected '}'\n");
        let json = render_findings(&diags, OutputMode::Json, true, &|_| None);
        assert!(!json.contains('\x1b'), "got: {json:?}");
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
        let rendered = render_findings(&[d], OutputMode::Pretty, false, &|_| Some(source.clone()));
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
    fn pretty_window_keeps_absolute_line_numbers() {
        // The snippet is sliced to the span's lines, so the gutter has to be
        // anchored back to the file's own numbering — a window that forgot
        // `line_start` would report line 1 here.
        let source = "line one\nline two\nline three\nline four\n".to_owned();
        let at = source.find("three").unwrap();
        let diags = [diag(at, at + 5, "the third line")];
        let rendered =
            render_findings(&diags, OutputMode::Pretty, false, &|_| Some(source.clone()));
        assert!(rendered.contains("x.tex:3:6"), "got: {rendered}");
        assert!(rendered.contains("3 | line three"), "got: {rendered}");
        // Folding drops the padding lines the window carries, so no neighbour
        // leaks into the output.
        assert!(!rendered.contains("line two"), "got: {rendered}");
        assert!(!rendered.contains("line four"), "got: {rendered}");
    }

    #[test]
    fn pretty_window_covers_a_distant_same_file_related() {
        // Primary and secondary share one snippet, so the window must span both
        // — a window sized to the primary alone would put the secondary's span
        // out of range and `annotate-snippets` would panic.
        let mut source = "\\label{a}\n".to_owned();
        source.push_str(&"filler\n".repeat(50));
        source.push_str("\\label{a}\n");
        let second = source.rfind("\\label{a}").unwrap();
        let mut d = diag(second, second + 9, "label `a` is defined more than once");
        d.related.push(RelatedInfo {
            path: PathBuf::from("x.tex"),
            start: 0,
            end: 9,
            message: "first definition of `a`".to_owned(),
        });
        let rendered = render_findings(&[d], OutputMode::Pretty, false, &|_| Some(source.clone()));
        assert!(rendered.contains("1 | \\label{a}"), "got: {rendered}");
        assert!(rendered.contains("52 | \\label{a}"), "got: {rendered}");
        assert!(
            rendered.contains("first definition of `a`"),
            "got: {rendered}"
        );
    }

    #[test]
    fn pretty_output_is_invariant_to_trailing_file_size() {
        // The property the window exists for: rendering a finding must not
        // depend on how much source sits beyond it. Before the window this held
        // by accident (the renderer read the whole file and folded it away) at
        // O(file) cost per finding; now it holds by construction, and this
        // pins it.
        let short = "\\foo{bar\n".to_owned();
        let long = format!("{short}{}", "padding line\n".repeat(2000));
        let diags = [diag(4, 5, "unclosed group")];
        let a = render_findings(&diags, OutputMode::Pretty, false, &|_| Some(short.clone()));
        let b = render_findings(&diags, OutputMode::Pretty, false, &|_| Some(long.clone()));
        assert_eq!(a, b);
    }

    #[test]
    fn json_serializes_diagnostic_with_fix_and_related() {
        use super::super::diagnostic::{Edit, Fix};

        let mut d = diag(12, 20, "label `x` is defined more than once");
        d.severity = Severity::Warning;
        d.fix = Some(Fix::safe_edits(
            vec![
                Edit::new(5, 9, "abcd"),
                Edit::in_file(PathBuf::from("other.tex"), 0, 4, "efgh"),
            ],
            "rename the second label",
        ));
        d.related.push(RelatedInfo {
            path: PathBuf::from("other.tex"),
            start: 0,
            end: 0,
            message: "first definition of `x`".to_owned(),
        });

        let rendered = render_findings(&[d], OutputMode::Json, false, &|_| None);
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        let diag = &value[0];
        assert_eq!(diag["rule"], "parse");
        assert_eq!(diag["severity"], "warning");
        assert_eq!(diag["path"], "x.tex");
        assert_eq!(diag["start"], 12);
        assert_eq!(diag["end"], 20);
        assert_eq!(diag["fix"]["applicability"], "safe");
        assert_eq!(diag["fix"]["description"], "rename the second label");
        assert_eq!(diag["fix"]["edits"][0]["content"], "abcd");
        // An own-file edit omits `path`; a cross-file edit carries it.
        assert!(diag["fix"]["edits"][0].get("path").is_none());
        assert_eq!(diag["fix"]["edits"][1]["path"], "other.tex");
        assert_eq!(diag["related"][0]["message"], "first definition of `x`");
    }

    #[test]
    fn json_omits_fix_when_none() {
        let d = diag(0, 1, "boom");
        let rendered = render_findings(&[d], OutputMode::Json, false, &|_| None);
        let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert!(value[0].get("fix").is_none());
        assert_eq!(value[0]["related"], serde_json::json!([]));
    }

    #[test]
    fn json_empty_input_is_empty_array() {
        let rendered = render_findings(&[], OutputMode::Json, false, &|_| None);
        assert_eq!(rendered, "[]");
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
        let rendered = render_findings(&[d], OutputMode::Pretty, false, &|p| match p.to_str() {
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
