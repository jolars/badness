//! badness-wasm — the wasm-bindgen shim behind the docs playground
//! (<https://badness.dev/playground.html>).
//!
//! A thin, non-published wrapper over [`badness_formatter`] and
//! [`badness_parser`]: [`format`] formats a document, [`check`] reports parse
//! diagnostics with positions already converted to UTF-16 code units (what
//! JavaScript string indexing and CodeMirror positions use), so the frontend
//! never touches byte offsets. All real logic lives in plain-Rust helpers unit
//! tested on the host target; the exported functions are glue.

use badness_formatter::{FormatStyle, MathWrap, WrapMode};
use badness_parser::parser::{LatexFlavor, LexConfig};
use wasm_bindgen::prelude::*;

/// A parse diagnostic with positions in UTF-16 code units.
#[wasm_bindgen(getter_with_clone)]
pub struct Diagnostic {
    pub message: String,
    /// Start offset into the input, in UTF-16 code units.
    pub start: u32,
    /// End offset (exclusive), in UTF-16 code units.
    pub end: u32,
    /// 1-based line of the start offset.
    pub line: u32,
    /// 1-based UTF-16 column of the start offset.
    pub column: u32,
}

/// Parse-check `input` and return its diagnostics (empty when clean).
///
/// `file_type` is one of `"tex"`, `"sty-cls"`, `"dtx"`, `"bib"`.
#[wasm_bindgen]
pub fn check(input: &str, file_type: &str) -> Result<Vec<Diagnostic>, JsError> {
    check_impl(input, file_type).map_err(|msg| JsError::new(&msg))
}

/// Format `input` and return the formatted text.
///
/// `file_type` is one of `"tex"`, `"sty-cls"`, `"dtx"`, `"bib"`. `wrap`
/// (`"reflow" | "stable" | "sentence" | "semantic" | "preserve"`) and
/// `math_wrap` (`"auto" | "preserve" | "single-line" | "break"`) apply to the
/// LaTeX kinds only and are ignored for `"bib"`. An omitted `wrap` uses the
/// file type's default, matching the CLI: `reflow` for a document, `preserve`
/// for package/class and `.dtx` sources.
#[wasm_bindgen]
pub fn format(
    input: &str,
    file_type: &str,
    line_width: Option<usize>,
    indent_width: Option<usize>,
    wrap: Option<String>,
    math_wrap: Option<String>,
) -> Result<String, JsError> {
    format_impl(
        input,
        file_type,
        line_width,
        indent_width,
        wrap.as_deref(),
        math_wrap.as_deref(),
    )
    .map_err(|msg| JsError::new(&msg))
}

/// The playground's file-type selector, mirroring the CLI's extension-driven
/// [`FileKind`](https://docs.rs/badness) dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileType {
    Tex,
    StyCls,
    Dtx,
    Bib,
}

fn parse_file_type(s: &str) -> Result<FileType, String> {
    match s {
        "tex" => Ok(FileType::Tex),
        "sty-cls" => Ok(FileType::StyCls),
        "dtx" => Ok(FileType::Dtx),
        "bib" => Ok(FileType::Bib),
        other => Err(format!(
            "unsupported file type: {other:?} (expected \"tex\", \"sty-cls\", \"dtx\", or \"bib\")"
        )),
    }
}

fn parse_wrap(s: &str) -> Result<WrapMode, String> {
    match s {
        "reflow" => Ok(WrapMode::Reflow),
        "stable" => Ok(WrapMode::Stable),
        "sentence" => Ok(WrapMode::Sentence),
        "semantic" => Ok(WrapMode::Semantic),
        "preserve" => Ok(WrapMode::Preserve),
        other => Err(format!("unsupported wrap mode: {other:?}")),
    }
}

fn parse_math_wrap(s: &str) -> Result<MathWrap, String> {
    match s {
        "auto" => Ok(MathWrap::Auto),
        "preserve" => Ok(MathWrap::Preserve),
        "single-line" => Ok(MathWrap::SingleLine),
        "break" => Ok(MathWrap::Break),
        other => Err(format!("unsupported math wrap mode: {other:?}")),
    }
}

/// The [`LexConfig`] for a LaTeX file type, mirroring `FileKind::lex_config`
/// in the CLI: a package/class starts with `@` as a letter, a `.dtx` runs the
/// docstrip lexer mode over a `Document`-flavored base.
fn lex_config(file_type: FileType) -> LexConfig {
    LexConfig {
        flavor: match file_type {
            FileType::StyCls => LatexFlavor::Package,
            _ => LatexFlavor::Document,
        },
        dtx: file_type == FileType::Dtx,
    }
}

/// The default [`WrapMode`] when the caller gives none, mirroring
/// `FileKind::default_wrap` in the CLI: package/class and `.dtx` bodies are
/// code, not prose, so they preserve authored breaks.
fn default_wrap(file_type: FileType) -> WrapMode {
    match file_type {
        FileType::StyCls | FileType::Dtx => WrapMode::Preserve,
        FileType::Tex | FileType::Bib => WrapMode::Reflow,
    }
}

fn build_style(
    file_type: FileType,
    line_width: Option<usize>,
    indent_width: Option<usize>,
    wrap: Option<&str>,
    math_wrap: Option<&str>,
) -> Result<FormatStyle, String> {
    let default = FormatStyle::default();
    Ok(FormatStyle {
        line_width: line_width.unwrap_or(default.line_width),
        indent_width: indent_width.unwrap_or(default.indent_width),
        wrap: match wrap {
            Some(s) => parse_wrap(s)?,
            None => default_wrap(file_type),
        },
        math_wrap: match math_wrap {
            Some(s) => parse_math_wrap(s)?,
            None => MathWrap::Auto,
        },
    })
}

fn check_impl(input: &str, file_type: &str) -> Result<Vec<Diagnostic>, String> {
    let file_type = parse_file_type(file_type)?;
    let errors: Vec<(String, usize, usize)> = match file_type {
        FileType::Bib => badness_parser::bib::parse(input)
            .errors
            .into_iter()
            .map(|e| (e.message, e.start, e.end))
            .collect(),
        _ => badness_parser::parser::parse_with_flavor(input, lex_config(file_type))
            .errors
            .into_iter()
            .map(|e| (e.message, e.start, e.end))
            .collect(),
    };
    Ok(to_diagnostics(input, errors))
}

fn format_impl(
    input: &str,
    file_type: &str,
    line_width: Option<usize>,
    indent_width: Option<usize>,
    wrap: Option<&str>,
    math_wrap: Option<&str>,
) -> Result<String, String> {
    let file_type = parse_file_type(file_type)?;
    let style = build_style(file_type, line_width, indent_width, wrap, math_wrap)?;
    match file_type {
        FileType::Bib => {
            badness_formatter::bib::format_with_style(input, style).map_err(|e| e.to_string())
        }
        _ => badness_formatter::formatter::format_with_style_flavored(
            input,
            style,
            lex_config(file_type),
        )
        .map_err(|e| e.to_string()),
    }
}

/// Convert the parser's byte-span errors into [`Diagnostic`]s with UTF-16
/// offsets and 1-based line/column, in one `char_indices` walk over `input`.
/// Spans may arrive in any order; offsets past the end or inside a multi-byte
/// character snap to the next character boundary.
fn to_diagnostics(input: &str, errors: Vec<(String, usize, usize)>) -> Vec<Diagnostic> {
    // Every byte offset we need to translate, tagged with (error index, is_end).
    let mut wanted: Vec<(usize, usize, bool)> = Vec::with_capacity(errors.len() * 2);
    for (i, (_, start, end)) in errors.iter().enumerate() {
        wanted.push((*start, i, false));
        wanted.push((*end, i, true));
    }
    wanted.sort_unstable_by_key(|&(byte, ..)| byte);

    struct Pos {
        offset: u32,
        line: u32,
        column: u32,
    }
    let mut resolved: Vec<(Option<Pos>, Option<u32>)> =
        errors.iter().map(|_| (None, None)).collect();

    let mut utf16: u32 = 0;
    let mut line: u32 = 1;
    let mut column: u32 = 1;
    let mut next = wanted.iter().peekable();
    for (byte, ch) in input.char_indices() {
        while let Some(&&(want, idx, is_end)) = next.peek() {
            if want > byte {
                break;
            }
            record(&mut resolved[idx], is_end, utf16, line, column);
            next.next();
        }
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += u32::try_from(ch.len_utf16()).unwrap_or(1);
        }
        utf16 += u32::try_from(ch.len_utf16()).unwrap_or(1);
    }
    for &(_, idx, is_end) in next {
        record(&mut resolved[idx], is_end, utf16, line, column);
    }

    fn record(
        slot: &mut (Option<Pos>, Option<u32>),
        is_end: bool,
        utf16: u32,
        line: u32,
        column: u32,
    ) {
        if is_end {
            slot.1 = Some(utf16);
        } else {
            slot.0 = Some(Pos {
                offset: utf16,
                line,
                column,
            });
        }
    }

    errors
        .into_iter()
        .zip(resolved)
        .map(|((message, ..), (start, end))| {
            let start = start.expect("every span start was queued");
            Diagnostic {
                message,
                start: start.offset,
                end: end.expect("every span end was queued"),
                line: start.line,
                column: start.column,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_types_parse_and_reject() {
        assert_eq!(parse_file_type("tex"), Ok(FileType::Tex));
        assert_eq!(parse_file_type("sty-cls"), Ok(FileType::StyCls));
        assert_eq!(parse_file_type("dtx"), Ok(FileType::Dtx));
        assert_eq!(parse_file_type("bib"), Ok(FileType::Bib));
        assert!(parse_file_type("sty").is_err());
        assert!(parse_file_type("").is_err());
    }

    #[test]
    fn wrap_modes_parse_and_reject() {
        assert_eq!(parse_wrap("reflow"), Ok(WrapMode::Reflow));
        assert_eq!(parse_wrap("stable"), Ok(WrapMode::Stable));
        assert_eq!(parse_wrap("sentence"), Ok(WrapMode::Sentence));
        assert_eq!(parse_wrap("semantic"), Ok(WrapMode::Semantic));
        assert_eq!(parse_wrap("preserve"), Ok(WrapMode::Preserve));
        assert!(parse_wrap("Reflow").is_err());
        assert_eq!(parse_math_wrap("auto"), Ok(MathWrap::Auto));
        assert_eq!(parse_math_wrap("preserve"), Ok(MathWrap::Preserve));
        assert_eq!(parse_math_wrap("single-line"), Ok(MathWrap::SingleLine));
        assert_eq!(parse_math_wrap("break"), Ok(MathWrap::Break));
        assert!(parse_math_wrap("single_line").is_err());
    }

    #[test]
    fn lex_config_mirrors_file_kind() {
        assert_eq!(
            lex_config(FileType::Tex),
            LexConfig {
                flavor: LatexFlavor::Document,
                dtx: false
            }
        );
        assert_eq!(
            lex_config(FileType::StyCls),
            LexConfig {
                flavor: LatexFlavor::Package,
                dtx: false
            }
        );
        assert_eq!(
            lex_config(FileType::Dtx),
            LexConfig {
                flavor: LatexFlavor::Document,
                dtx: true
            }
        );
    }

    #[test]
    fn default_wrap_mirrors_file_kind() {
        assert_eq!(default_wrap(FileType::Tex), WrapMode::Reflow);
        assert_eq!(default_wrap(FileType::StyCls), WrapMode::Preserve);
        assert_eq!(default_wrap(FileType::Dtx), WrapMode::Preserve);
    }

    #[test]
    fn build_style_defaults_and_overrides() {
        let style = build_style(FileType::Tex, None, None, None, None).unwrap();
        assert_eq!(style, FormatStyle::default());

        let style = build_style(FileType::StyCls, None, None, None, None).unwrap();
        assert_eq!(style.wrap, WrapMode::Preserve);

        let style = build_style(
            FileType::StyCls,
            Some(100),
            Some(4),
            Some("reflow"),
            Some("break"),
        )
        .unwrap();
        assert_eq!(style.line_width, 100);
        assert_eq!(style.indent_width, 4);
        assert_eq!(style.wrap, WrapMode::Reflow);
        assert_eq!(style.math_wrap, MathWrap::Break);

        assert!(build_style(FileType::Tex, None, None, Some("bogus"), None).is_err());
    }

    #[test]
    fn to_diagnostics_converts_bytes_to_utf16() {
        // "é" is 2 bytes / 1 UTF-16 unit; "𝛼" is 4 bytes / 2 UTF-16 units.
        let input = "é𝛼\nab";
        // Span over "ab": bytes 7..9.
        let diags = to_diagnostics(input, vec![("x".into(), 7, 9)]);
        assert_eq!(diags[0].start, 4);
        assert_eq!(diags[0].end, 6);
        assert_eq!(diags[0].line, 2);
        assert_eq!(diags[0].column, 1);
    }

    #[test]
    fn to_diagnostics_handles_out_of_order_and_eof_spans() {
        let input = "abc";
        let diags = to_diagnostics(
            input,
            vec![
                ("late".into(), 2, 3),
                ("early".into(), 0, 1),
                ("eof".into(), 3, 3),
            ],
        );
        assert_eq!(diags[0].message, "late");
        assert_eq!((diags[0].start, diags[0].end), (2, 3));
        assert_eq!((diags[1].start, diags[1].end), (0, 1));
        assert_eq!((diags[2].start, diags[2].end), (3, 3));
        assert_eq!((diags[2].line, diags[2].column), (1, 4));
    }

    #[test]
    fn check_reports_and_clears() {
        assert!(
            !check_impl("\\begin{itemize} \\item x", "tex")
                .unwrap()
                .is_empty()
        );
        assert!(check_impl("\\emph{fine}\n", "tex").unwrap().is_empty());
        assert!(
            check_impl("@article{key,\n  title = {T},\n}\n", "bib")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn format_smoke_per_file_type() {
        assert!(format_impl("Hello \\emph{world}.\n", "tex", None, None, None, None).is_ok());
        assert!(format_impl("\\def\\x@y{z}\n", "sty-cls", None, None, None, None).is_ok());
        assert!(
            format_impl(
                "@article{key,\n  title = {T},\n}\n",
                "bib",
                None,
                None,
                None,
                None
            )
            .is_ok()
        );
        // Unbalanced group: the formatter refuses input with parse errors.
        assert!(format_impl("{\n", "tex", None, None, None, None).is_err());
        // Junk options are rejected up front.
        assert!(format_impl("x\n", "tex", None, None, Some("bogus"), None).is_err());
        assert!(format_impl("x\n", "nope", None, None, None, None).is_err());
    }
}
