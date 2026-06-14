//! The `badness` command-line interface.
//!
//! Phase 2 MVP: a `format` subcommand that formats `.tex` files in place (or
//! stdin → stdout), plus `--check` to report whether files are already
//! formatted. The formatter itself is an identity lowering for now (see
//! `formatter::core`), so formatting is byte-for-byte stable.
//!
//! Deferred (later Phase 2): `build.rs` man pages / shell completions /
//! markdown via `clap_mangen` / `clap_complete` / `clap-markdown`, and
//! directory-walking file discovery.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use badness::file_discovery::{FileDiscoveryError, collect_tex_files};
use badness::formatter::{FormatStyle, WrapMode, check_paths_with_style, format_with_style};
use badness::linter::{Diagnostic, OutputMode, lint_document, render_findings};
use badness::parser::parse;
use badness::semantic::SemanticModel;
use badness::syntax::SyntaxNode;
use clap::{Parser, Subcommand, ValueEnum};
use rowan::NodeOrToken;

/// CLI surface for [`WrapMode`]. Kept here (not in the formatter) so the
/// formatter API stays clap-free, mirroring arity's `cli.rs` convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum WrapArg {
    /// Greedy fill: wrap words to the line width (default).
    Reflow,
    /// One sentence per line. (Not yet implemented — behaves like `preserve`.)
    Sentence,
    /// Semantic line breaks (sembr.org). (Not yet implemented — like `preserve`.)
    Semantic,
    /// Leave authored line breaks untouched.
    Preserve,
}

impl From<WrapArg> for WrapMode {
    fn from(arg: WrapArg) -> Self {
        match arg {
            WrapArg::Reflow => WrapMode::Reflow,
            WrapArg::Sentence => WrapMode::Sentence,
            WrapArg::Semantic => WrapMode::Semantic,
            WrapArg::Preserve => WrapMode::Preserve,
        }
    }
}

#[derive(Parser)]
#[command(
    name = "badness",
    version,
    about = "A formatter, linter, and language server for LaTeX"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Format LaTeX source.
    ///
    /// With paths, formats each file in place. With no paths, reads stdin and
    /// writes the formatted result to stdout.
    Format {
        /// Files to format. Omit to read from stdin.
        paths: Vec<PathBuf>,
        /// Report which files would change without writing them. Exits non-zero
        /// if any file is not already formatted.
        #[arg(long)]
        check: bool,
        /// Maximum line width before the formatter breaks a line.
        #[arg(long)]
        line_width: Option<usize>,
        /// Number of spaces per indent step.
        #[arg(long)]
        indent_width: Option<usize>,
        /// How to lay out line breaks inside a paragraph.
        #[arg(long, value_enum)]
        wrap: Option<WrapArg>,
    },
    /// Lint LaTeX source, reporting parse diagnostics.
    ///
    /// With paths, lints each file. With no paths, reads stdin. Exits non-zero
    /// if any diagnostics are reported.
    Lint {
        /// Files to lint. Omit to read from stdin.
        paths: Vec<PathBuf>,
    },
    /// Parse LaTeX source and print its concrete syntax tree (CST).
    ///
    /// A debugging aid: prints the lossless parse tree as an indented
    /// `KIND@range` listing, with token text, followed by any parse errors.
    /// With a path, parses that file. With no path, reads stdin.
    Parse {
        /// File to parse. Omit to read from stdin.
        path: Option<PathBuf>,
    },
    /// Run the language server over stdio.
    Lsp,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Format {
            paths,
            check,
            line_width,
            indent_width,
            wrap,
        } => {
            let mut style = FormatStyle::default();
            if let Some(w) = line_width {
                style.line_width = w;
            }
            if let Some(w) = indent_width {
                style.indent_width = w;
            }
            if let Some(w) = wrap {
                style.wrap = w.into();
            }
            run_format(&paths, check, style)
        }
        Command::Lint { paths } => run_lint(&paths),
        Command::Parse { path } => run_parse(path.as_deref()),
        Command::Lsp => run_lsp(),
    }
}

/// Run the language server, mapping a startup failure to a non-zero exit.
fn run_lsp() -> ExitCode {
    match badness::lsp::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("badness: language server error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Lint each path (or stdin), rendering parse diagnostics. Exits non-zero if
/// any diagnostics are reported or any file fails to read.
fn run_lint(paths: &[PathBuf]) -> ExitCode {
    // Hold each file's text in memory keyed by the label we report it under, so
    // the renderer can fetch source for snippets without re-reading from disk
    // (and so stdin, which has no path, still gets a source).
    let mut sources: Vec<(PathBuf, String)> = Vec::new();
    let mut failed = false;

    if paths.is_empty() {
        let mut input = String::new();
        if let Err(err) = std::io::stdin().read_to_string(&mut input) {
            eprintln!("badness: cannot read stdin: {err}");
            return ExitCode::FAILURE;
        }
        sources.push((PathBuf::from("<stdin>"), input));
    } else {
        let files = match collect_tex_files(paths) {
            Ok(files) => files,
            Err(err) => {
                report_discovery_error(&err);
                return ExitCode::FAILURE;
            }
        };
        if files.is_empty() {
            eprintln!("badness: no .tex files found under the provided input paths");
            return ExitCode::FAILURE;
        }
        for path in files {
            match std::fs::read_to_string(&path) {
                Ok(content) => sources.push((path, content)),
                Err(err) => {
                    eprintln!("badness: cannot read {}: {err}", path.display());
                    failed = true;
                }
            }
        }
    }

    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    for (path, content) in &sources {
        let parsed = parse(content);
        diagnostics.extend(
            parsed
                .errors
                .iter()
                .map(|err| Diagnostic::from_parse(path.clone(), err)),
        );
        // Lint rules run off the same parse — no salsa needed on the CLI path
        // (mirrors arity's `check_document`).
        let root = SyntaxNode::new_root(parsed.green);
        let model = SemanticModel::build(&root);
        diagnostics.extend(lint_document(path, &root, &model));
    }

    if !diagnostics.is_empty() {
        let source_for = |path: &Path| {
            sources
                .iter()
                .find(|(p, _)| p == path)
                .map(|(_, text)| text.clone())
        };
        eprint!(
            "{}",
            render_findings(&diagnostics, OutputMode::Pretty, &source_for)
        );
    }

    if failed || !diagnostics.is_empty() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Parse a single file (or stdin) and print its CST to stdout. Parse errors are
/// printed after the tree; the command exits non-zero if any are reported.
fn run_parse(path: Option<&Path>) -> ExitCode {
    let input = match path {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(err) => {
                eprintln!("badness: cannot read {}: {err}", path.display());
                return ExitCode::FAILURE;
            }
        },
        None => {
            let mut input = String::new();
            if let Err(err) = std::io::stdin().read_to_string(&mut input) {
                eprintln!("badness: cannot read stdin: {err}");
                return ExitCode::FAILURE;
            }
            input
        }
    };

    let parsed = parse(&input);
    let mut out = String::new();
    render_cst(&parsed.syntax(), 0, &mut out);
    if let Err(err) = std::io::stdout().write_all(out.as_bytes()) {
        eprintln!("badness: cannot write stdout: {err}");
        return ExitCode::FAILURE;
    }

    if parsed.errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        for err in &parsed.errors {
            eprintln!("error @{}..{}: {}", err.start, err.end, err.message);
        }
        ExitCode::FAILURE
    }
}

/// Render a CST as an indented `KIND@range` tree, with token text. Kept in sync
/// with the test renderer in `tests/parser.rs`.
fn render_cst(node: &SyntaxNode, depth: usize, out: &mut String) {
    out.push_str(&format!(
        "{:indent$}{:?}@{:?}\n",
        "",
        node.kind(),
        node.text_range(),
        indent = depth * 2
    ));
    for child in node.children_with_tokens() {
        match child {
            NodeOrToken::Node(n) => render_cst(&n, depth + 1, out),
            NodeOrToken::Token(t) => out.push_str(&format!(
                "{:indent$}{:?}@{:?} {:?}\n",
                "",
                t.kind(),
                t.text_range(),
                t.text(),
                indent = (depth + 1) * 2
            )),
        }
    }
}

fn run_format(paths: &[PathBuf], check: bool, style: FormatStyle) -> ExitCode {
    if check {
        return run_check(paths, style);
    }
    if paths.is_empty() {
        run_format_stdin(style)
    } else {
        run_format_paths(paths, style)
    }
}

/// `--check`: report unformatted files, exit code 1 if any.
fn run_check(paths: &[PathBuf], style: FormatStyle) -> ExitCode {
    match check_paths_with_style(paths, style) {
        Ok(result) => {
            if result.changed_files.is_empty() {
                ExitCode::SUCCESS
            } else {
                for path in &result.changed_files {
                    eprintln!("would reformat {}", path.display());
                }
                eprintln!(
                    "{} of {} file(s) would be reformatted",
                    result.changed_files.len(),
                    result.checked_files
                );
                ExitCode::FAILURE
            }
        }
        Err(err) => {
            eprintln!("badness: {err}");
            ExitCode::FAILURE
        }
    }
}

/// No paths: read stdin, format, write to stdout.
fn run_format_stdin(style: FormatStyle) -> ExitCode {
    let mut input = String::new();
    if let Err(err) = std::io::stdin().read_to_string(&mut input) {
        eprintln!("badness: cannot read stdin: {err}");
        return ExitCode::FAILURE;
    }
    match format_with_style(&input, style) {
        Ok(formatted) => {
            if let Err(err) = std::io::stdout().write_all(formatted.as_bytes()) {
                eprintln!("badness: cannot write stdout: {err}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("badness: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Print a file-discovery error to stderr, prefixed like the other CLI errors.
fn report_discovery_error(err: &FileDiscoveryError) {
    match err {
        FileDiscoveryError::NonTexFilePath { path } => {
            eprintln!(
                "badness: input file {} is not a .tex file; only .tex files are supported",
                path.display()
            );
        }
        FileDiscoveryError::WalkError { path, message } => {
            eprintln!(
                "badness: failed while scanning {}: {message}",
                path.display()
            );
        }
    }
}

/// Resolve the input paths to `.tex` files in place, writing only files whose
/// content changes.
fn run_format_paths(paths: &[PathBuf], style: FormatStyle) -> ExitCode {
    let files = match collect_tex_files(paths) {
        Ok(files) => files,
        Err(err) => {
            report_discovery_error(&err);
            return ExitCode::FAILURE;
        }
    };
    if files.is_empty() {
        eprintln!("badness: no .tex files found under the provided input paths");
        return ExitCode::FAILURE;
    }

    let mut failed = false;
    for path in &files {
        let content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(err) => {
                eprintln!("badness: cannot read {}: {err}", path.display());
                failed = true;
                continue;
            }
        };
        match format_with_style(&content, style) {
            Ok(formatted) => {
                if formatted != content
                    && let Err(err) = std::fs::write(path, formatted)
                {
                    eprintln!("badness: cannot write {}: {err}", path.display());
                    failed = true;
                }
            }
            Err(err) => {
                eprintln!("badness: cannot format {}: {err}", path.display());
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
