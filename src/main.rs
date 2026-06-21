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

use badness::file_discovery::{FileDiscoveryError, FileKind, collect_lint_files, file_kind_or_tex};
use badness::formatter::{
    FormatStyle, WrapMode, check_paths_with_style, format_with_style_flavored,
};
use badness::linter::{
    Diagnostic, OutputMode, apply_fixes, check_document, lint_document, render_findings,
};
use std::collections::HashMap;

use badness::parser::{LexConfig, parse_with_flavor};
use badness::project::labels::{document_label_names, is_document_root};
use badness::project::{
    CiteFileFacts, FileFacts, IncludeGraph, ResolvedCitations, ResolvedLabels,
    collect_bib_resource_targets, collect_include_edge_keys,
};
use badness::semantic::SemanticModel;
use badness::syntax::SyntaxNode;
use clap::{Parser, Subcommand, ValueEnum};
use rowan::NodeOrToken;
use smol_str::SmolStr;

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
        /// Name the stdin buffer so its language is dispatched by extension
        /// (`.bib` → BibTeX, anything else → LaTeX). No file is read or written;
        /// only the extension is used. Ignored when paths are given.
        #[arg(long, value_name = "PATH")]
        stdin_filepath: Option<PathBuf>,
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
        /// Apply safe autofixes in place, then report what remains. Requires
        /// path arguments; has no effect on stdin (there is nothing to write).
        #[arg(long)]
        fix: bool,
        /// Also apply fixes that may change typeset output (requires `--fix`).
        #[arg(long)]
        unsafe_fixes: bool,
        /// Name the stdin buffer so its language is dispatched by extension
        /// (`.bib` → BibTeX, anything else → LaTeX). No file is read or written;
        /// only the extension is used. Ignored when paths are given.
        #[arg(long, value_name = "PATH")]
        stdin_filepath: Option<PathBuf>,
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
            stdin_filepath,
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
            // `--wrap` is a *global* override; without it each file falls back to
            // its kind's default wrap (`.sty`/`.cls` → Preserve, `.tex` → Reflow),
            // resolved per file at dispatch.
            let wrap_override: Option<WrapMode> = wrap.map(Into::into);
            run_format(
                &paths,
                check,
                stdin_filepath.as_deref(),
                style,
                wrap_override,
            )
        }
        Command::Lint {
            paths,
            fix,
            unsafe_fixes,
            stdin_filepath,
        } => run_lint(&paths, fix, unsafe_fixes, stdin_filepath.as_deref()),
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

/// Cap on fixpoint iterations per file, guarding against a fix that fails to
/// clear its own diagnostic.
const MAX_FIX_ITERATIONS: usize = 10;

/// Lint each path (or stdin), rendering parse diagnostics. Exits non-zero if
/// any diagnostics are reported or any file fails to read. With `fix`, safe
/// autofixes (plus unsafe ones when `unsafe_fixes` is set) are applied in place
/// first; the reporting pass below then shows whatever findings remain.
fn run_lint(
    paths: &[PathBuf],
    fix: bool,
    unsafe_fixes: bool,
    stdin_filepath: Option<&Path>,
) -> ExitCode {
    // Apply fixes in place first; the reporting pass below then re-reads from
    // disk and shows whatever findings remain. Mirrors arity's two-pass flow.
    // Stdin (no paths) has nowhere to write back, so `--fix` only acts on files.
    if fix
        && !paths.is_empty()
        && let Some(code) = apply_fixes_to_paths(paths, unsafe_fixes)
    {
        return code;
    }

    // Hold each file's text (and which pipeline it feeds) in memory keyed by the
    // label we report it under, so the renderer can fetch source for snippets
    // without re-reading from disk (and so stdin, which has no path, still gets a
    // source). Stdin has no extension to dispatch on, so it is LaTeX unless
    // `--stdin-filepath` names the buffer (`.bib` → BibTeX); the label stays
    // `<stdin>` regardless, so the named path never reaches the report or disk.
    let mut sources: Vec<(PathBuf, String, FileKind)> = Vec::new();
    let mut failed = false;

    if paths.is_empty() {
        let mut input = String::new();
        if let Err(err) = std::io::stdin().read_to_string(&mut input) {
            eprintln!("badness: cannot read stdin: {err}");
            return ExitCode::FAILURE;
        }
        let kind = stdin_filepath.map_or(FileKind::Tex, file_kind_or_tex);
        sources.push((PathBuf::from("<stdin>"), input, kind));
    } else {
        let files = match collect_lint_files(paths) {
            Ok(files) => files,
            Err(err) => {
                report_discovery_error(&err);
                return ExitCode::FAILURE;
            }
        };
        if files.is_empty() {
            eprintln!(
                "badness: no .tex, .sty, .cls, or .bib files found under the provided input paths"
            );
            return ExitCode::FAILURE;
        }
        for (path, kind) in files {
            match std::fs::read_to_string(&path) {
                Ok(content) => sources.push((path, content, kind)),
                Err(err) => {
                    eprintln!("badness: cannot read {}: {err}", path.display());
                    failed = true;
                }
            }
        }
    }

    // Parse and build the per-file model for every LaTeX source first: cross-file
    // label resolution needs the whole analyzed set before any one file can be
    // linted. `.bib` files have no cross-file resolution yet (Phase 4), so each is
    // linted standalone via the bib driver and its findings folded straight in.
    // Lint rules run off these parses — no salsa needed on the CLI path (the salsa
    // firewall is an editor-incrementality concern; mirrors arity's
    // `check_document`). The resolver reuses the *same* pure helpers the salsa
    // queries do (`document_label_names`, `is_document_root`,
    // `collect_include_edge_keys`, `ResolvedLabels::build`), so CLI and LSP agree.
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut analyzed: Vec<(&PathBuf, SyntaxNode, SemanticModel)> = Vec::new();
    let mut facts: Vec<FileFacts> = Vec::new();
    let mut label_inputs = Vec::new();
    let mut cite_facts: Vec<CiteFileFacts> = Vec::new();
    // Cite keys per analyzed `.bib` path, feeding the cross-file citation resolver.
    let mut bib_keys: HashMap<PathBuf, Vec<SmolStr>> = HashMap::new();
    for (path, content, kind) in &sources {
        match kind {
            FileKind::Bib => {
                // Build the model once: it yields both the lint diagnostics and the
                // cite keys this `.bib` contributes to the citation resolver.
                let parsed = badness::bib::parse(content);
                diagnostics.extend(parsed.errors.iter().map(|err| Diagnostic {
                    rule: "parse",
                    severity: badness::linter::Severity::Error,
                    path: path.clone(),
                    start: err.start,
                    end: err.end,
                    message: err.message.clone(),
                    fix: None,
                }));
                let root = parsed.syntax();
                let model = badness::bib::semantic::Model::build(&root);
                bib_keys.insert(
                    path.clone(),
                    model.entries().iter().map(|e| e.key.clone()).collect(),
                );
                diagnostics.extend(badness::bib::linter::lint_document(path, &root, &model));
            }
            FileKind::Tex | FileKind::Sty | FileKind::Cls | FileKind::Dtx => {
                let parsed = parse_with_flavor(content, kind.lex_config());
                diagnostics.extend(
                    parsed
                        .errors
                        .iter()
                        .map(|err| Diagnostic::from_parse(path.clone(), err)),
                );
                let root = SyntaxNode::new_root(parsed.green);
                let model = SemanticModel::build(&root);
                facts.push(FileFacts {
                    path: path.clone(),
                    include_edges: collect_include_edge_keys(&root, path.parent()),
                });
                label_inputs.push((
                    path.clone(),
                    document_label_names(&model),
                    is_document_root(&root),
                ));
                cite_facts.push(CiteFileFacts {
                    path: path.clone(),
                    bib_targets: collect_bib_resource_targets(&root, path.parent()),
                    nocite_all: model.has_wildcard_nocite(),
                    is_document_root: is_document_root(&root),
                });
                analyzed.push((path, root, model));
            }
        }
    }

    let graph = IncludeGraph::build(&facts, None);
    let resolved = ResolvedLabels::build(&label_inputs, &graph);
    let resolved_citations = ResolvedCitations::build(&cite_facts, &graph, &bib_keys);
    for (path, root, model) in &analyzed {
        diagnostics.extend(lint_document(
            path,
            root,
            model,
            Some(&resolved),
            Some(&resolved_citations),
        ));
    }

    // Findings from the two pipelines arrive interleaved by file; sort so the
    // renderer presents them deterministically (by path, then position).
    diagnostics.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then(a.start.cmp(&b.start))
            .then(a.end.cmp(&b.end))
            .then(a.rule.cmp(b.rule))
    });

    if !diagnostics.is_empty() {
        let source_for = |path: &Path| {
            sources
                .iter()
                .find(|(p, _, _)| p == path)
                .map(|(_, text, _)| text.clone())
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

/// Discover lintable files under `paths` and apply autofixes in place. Returns
/// `Some(exit_code)` only on a hard error (discovery / IO); on success returns
/// `None` so the caller falls through to the normal reporting pass. Mirrors
/// arity's `apply_fixes_to_paths`.
///
/// Both `.tex` and `.bib` files are fixed, each through its own linter; rules that
/// emit no autofix (the report-only majority) leave their findings for the
/// reporting pass that follows.
fn apply_fixes_to_paths(paths: &[PathBuf], include_unsafe: bool) -> Option<ExitCode> {
    let files = match collect_lint_files(paths) {
        Ok(files) => files,
        Err(err) => {
            report_discovery_error(&err);
            return Some(ExitCode::FAILURE);
        }
    };
    if files.is_empty() {
        eprintln!("badness: no .tex or .bib files found under the provided input paths");
        return Some(ExitCode::FAILURE);
    }
    for (path, kind) in files {
        match fix_file(&path, kind, include_unsafe) {
            Ok(0) => {}
            Ok(n) => eprintln!("{}: {n} fix{} applied", path.display(), plural(n)),
            Err(err) => {
                eprintln!("badness: cannot fix {}: {err}", path.display());
                return Some(ExitCode::FAILURE);
            }
        }
    }
    None
}

/// Run the fixpoint loop on a single file and write it back if anything changed.
/// Returns the number of individual fixes applied. Re-lints after each round so
/// fixes can cascade; bounded by [`MAX_FIX_ITERATIONS`]. Mirrors arity's
/// `fix_file`. Routes to the LaTeX or BibTeX linter by [`FileKind`].
fn fix_file(path: &Path, kind: FileKind, include_unsafe: bool) -> std::io::Result<usize> {
    let mut content = std::fs::read_to_string(path)?;
    let mut total = 0usize;
    for _ in 0..MAX_FIX_ITERATIONS {
        let diagnostics = match kind {
            FileKind::Tex | FileKind::Sty | FileKind::Cls | FileKind::Dtx => {
                check_document(path, &content, kind.lex_config())
            }
            FileKind::Bib => badness::bib::linter::check_document(path, &content),
        };
        let fixes: Vec<_> = diagnostics.into_iter().filter_map(|d| d.fix).collect();
        if fixes.is_empty() {
            break;
        }
        let outcome = apply_fixes(&content, &fixes, include_unsafe);
        if outcome.applied == 0 {
            break;
        }
        total += outcome.applied;
        content = outcome.output;
    }
    if total > 0 {
        std::fs::write(path, &content)?;
    }
    Ok(total)
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "es" }
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

    let config = path.map_or(LexConfig::default(), |p| file_kind_or_tex(p).lex_config());
    let parsed = parse_with_flavor(&input, config);
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

fn run_format(
    paths: &[PathBuf],
    check: bool,
    stdin_filepath: Option<&Path>,
    style: FormatStyle,
    wrap_override: Option<WrapMode>,
) -> ExitCode {
    if check {
        return run_check(paths, style, wrap_override);
    }
    if paths.is_empty() {
        run_format_stdin(stdin_filepath, style, wrap_override)
    } else {
        run_format_paths(paths, style, wrap_override)
    }
}

/// `--check`: report unformatted files, exit code 1 if any.
fn run_check(paths: &[PathBuf], style: FormatStyle, wrap_override: Option<WrapMode>) -> ExitCode {
    match check_paths_with_style(paths, style, wrap_override) {
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

/// No paths: read stdin, format, write to stdout. The pipeline is chosen from
/// `stdin_filepath`'s extension (`.bib` → BibTeX, else LaTeX); with no name given,
/// stdin stays LaTeX, the long-standing conservative default.
fn run_format_stdin(
    stdin_filepath: Option<&Path>,
    mut style: FormatStyle,
    wrap_override: Option<WrapMode>,
) -> ExitCode {
    let mut input = String::new();
    if let Err(err) = std::io::stdin().read_to_string(&mut input) {
        eprintln!("badness: cannot read stdin: {err}");
        return ExitCode::FAILURE;
    }
    let kind = stdin_filepath.map_or(FileKind::Tex, file_kind_or_tex);
    style.wrap = wrap_override.unwrap_or(kind.default_wrap());
    let formatted = match kind {
        FileKind::Tex | FileKind::Sty | FileKind::Cls | FileKind::Dtx => {
            format_with_style_flavored(&input, style, kind.lex_config()).map_err(|e| e.to_string())
        }
        FileKind::Bib => badness::bib::format_with_style(&input, style).map_err(|e| e.to_string()),
    };
    match formatted {
        Ok(formatted) => {
            if let Err(err) = std::io::stdout().write_all(formatted.as_bytes()) {
                eprintln!("badness: cannot write stdout: {err}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(msg) => {
            eprintln!("badness: {msg}");
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
        FileDiscoveryError::UnsupportedLintFilePath { path } => {
            eprintln!(
                "badness: input file {} is not a .tex, .sty, .cls, or .bib file",
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

/// Resolve the input paths to `.tex`/`.bib` files and format each in place,
/// writing only files whose content changes. Each file is routed to its own
/// formatter by [`FileKind`].
fn run_format_paths(
    paths: &[PathBuf],
    mut style: FormatStyle,
    wrap_override: Option<WrapMode>,
) -> ExitCode {
    let files = match collect_lint_files(paths) {
        Ok(files) => files,
        Err(err) => {
            report_discovery_error(&err);
            return ExitCode::FAILURE;
        }
    };
    if files.is_empty() {
        eprintln!(
            "badness: no .tex, .sty, .cls, or .bib files found under the provided input paths"
        );
        return ExitCode::FAILURE;
    }

    let mut failed = false;
    for (path, kind) in &files {
        let content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(err) => {
                eprintln!("badness: cannot read {}: {err}", path.display());
                failed = true;
                continue;
            }
        };
        style.wrap = wrap_override.unwrap_or(kind.default_wrap());
        let formatted = match kind {
            FileKind::Tex | FileKind::Sty | FileKind::Cls | FileKind::Dtx => {
                format_with_style_flavored(&content, style, kind.lex_config())
                    .map_err(|e| e.to_string())
            }
            FileKind::Bib => {
                badness::bib::format_with_style(&content, style).map_err(|e| e.to_string())
            }
        };
        match formatted {
            Ok(formatted) => {
                if formatted != *content
                    && let Err(err) = std::fs::write(path, formatted)
                {
                    eprintln!("badness: cannot write {}: {err}", path.display());
                    failed = true;
                }
            }
            Err(msg) => {
                eprintln!("badness: cannot format {}: {msg}", path.display());
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
