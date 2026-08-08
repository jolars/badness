//! The `badness` command-line interface.
//!
//! Phase 2 MVP: a `format` subcommand that formats `.tex` files in place (or
//! stdin → stdout), plus `--check` to report whether files are already
//! formatted. The formatter itself is an identity lowering for now (see
//! `formatter::core`), so formatting is byte-for-byte stable.
//!
//! Deferred (later Phase 2): directory-walking file discovery.
//!
//! Man pages, shell completions, and the markdown CLI reference are generated
//! from the [`badness::cli`] definitions by `build.rs` (via `clap_mangen` /
//! `clap_complete` / `clapdown`).

use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use badness::config::{Config, ConfigSource};
use badness::file_discovery::{
    ExcludeFilter, FileDiscoveryError, FileKind, collect_lint_files, file_kind_or_tex,
};
use badness::formatter::perturb::{
    ConvergenceError, DEFAULT_SINGLE_FLIP_SAMPLES, check_trivia_convergence,
};
use badness::formatter::{
    ChangedFile, FormatStyle, LineEnding, MathWrap, SentenceOptions, WrapMode,
    check_paths_with_style, format_file_with_packages_sentence,
    format_with_style_flavored_sentence,
};
use badness::linter::{
    Diagnostic, Fix, OutputMode, RuleSelection, apply_fixes, apply_fixes_multi,
    check_document_fixable, lint_document, render_findings,
};
use std::collections::{BTreeSet, HashMap};

use badness::cli::{
    Cli, ColorChoice, Command, DebugChecksArg, DebugCommand, LineEndingArg, LintOutput,
    MathWrapArg, WrapArg,
};
use badness::parser::{LexConfig, parse_with_flavor};
use badness::project::labels::{document_label_names, document_ref_names, is_document_root};
use badness::project::{
    CiteFileFacts, FileFacts, IncludeGraph, PackageOptionFacts, ResolvedCitations, ResolvedLabels,
    ResolvedPackageOptions, collect_bib_resource_targets, collect_include_edge_keys,
    package_option_facts,
};
use badness::semantic::SemanticModel;
use badness::syntax::SyntaxNode;
use clap::Parser;
use rayon::prelude::*;
use rowan::{GreenNode, NodeOrToken};
use similar::{ChangeTag, TextDiff};
use smol_str::SmolStr;

/// Lower the CLI [`WrapArg`] to the formatter's [`WrapMode`]. Kept as a free
/// function (not a `From` impl) because the orphan rule forbids implementing a
/// foreign trait for a foreign type in the binary crate, now that both types
/// live in the library.
fn wrap_mode(arg: WrapArg) -> WrapMode {
    match arg {
        WrapArg::Reflow => WrapMode::Reflow,
        WrapArg::Stable => WrapMode::Stable,
        WrapArg::Sentence => WrapMode::Sentence,
        WrapArg::Semantic => WrapMode::Semantic,
        WrapArg::Preserve => WrapMode::Preserve,
    }
}

/// Lower the CLI [`LintOutput`] to the renderer's [`OutputMode`] (same
/// orphan-rule story as [`wrap_mode`]).
fn lint_output_mode(arg: LintOutput) -> OutputMode {
    match arg {
        LintOutput::Pretty => OutputMode::Pretty,
        LintOutput::Concise => OutputMode::Concise,
        LintOutput::Json => OutputMode::Json,
    }
}

/// Lower the CLI [`MathWrapArg`] to the formatter's [`MathWrap`] (same orphan-rule
/// story as [`wrap_mode`]).
fn math_wrap_mode(arg: MathWrapArg) -> MathWrap {
    match arg {
        MathWrapArg::Auto => MathWrap::Auto,
        MathWrapArg::Preserve => MathWrap::Preserve,
        MathWrapArg::SingleLine => MathWrap::SingleLine,
        MathWrapArg::Break => MathWrap::Break,
    }
}

/// Lower the CLI [`LineEndingArg`] to the formatter's [`LineEnding`] (same
/// orphan-rule story as [`wrap_mode`]).
fn line_ending_mode(arg: LineEndingArg) -> LineEnding {
    match arg {
        LineEndingArg::Auto => LineEnding::Auto,
        LineEndingArg::Lf => LineEnding::Lf,
        LineEndingArg::Crlf => LineEnding::Crlf,
        LineEndingArg::Native => LineEnding::Native,
    }
}

fn main() -> ExitCode {
    let Cli {
        command,
        config: config_arg,
        no_config,
        color,
        quiet,
    } = Cli::parse();
    let out = OutputOptions { color, quiet };
    match command {
        Command::Format {
            paths,
            check,
            stdin_filepath,
            line_width,
            indent_width,
            wrap,
            math_wrap,
            line_ending,
            exclude,
            force_exclude,
        } => {
            // Discover/load `badness.toml` from the working directory (one config
            // per invocation), falling back to the global user config. The exclude
            // filter is rooted at the config's directory so its patterns resolve
            // relative to it.
            let anchor = match cwd_anchor() {
                Ok(anchor) => anchor,
                Err(code) => return code,
            };
            let (config, config_source) =
                match resolve_config(config_arg.as_deref(), no_config, &anchor) {
                    Ok(resolved) => resolved,
                    Err(code) => return code,
                };
            let exclude_filter =
                match build_exclude_filter(&config, &config_source, &anchor, &exclude) {
                    Ok(filter) => filter.with_force_exclude(force_exclude),
                    Err(code) => return code,
                };

            let (style, wrap_override) = resolve_style(
                &config,
                line_width,
                indent_width,
                wrap,
                math_wrap,
                line_ending,
            );
            // The `sentence`/`semantic` language profile, resolved once from
            // `[format] lang` + `[format.no-break-abbreviations]`; `scratch` owns the
            // merged entries for the whole format run. Ignored by other wrap modes.
            let mut abbrev_scratch = Vec::new();
            let sentence = SentenceOptions::resolve(
                config.format.lang.as_deref(),
                &config.format.no_break_abbreviations,
                &mut abbrev_scratch,
            );
            run_format(
                &paths,
                check,
                stdin_filepath.as_deref(),
                style,
                wrap_override,
                sentence,
                &exclude_filter,
                out,
            )
        }
        Command::Lint {
            paths,
            fix,
            unsafe_fixes,
            stdin_filepath,
            exclude,
            force_exclude,
            select,
            ignore,
            explain,
            output,
        } => {
            if let Some(rule) = explain {
                return run_explain(&rule);
            }
            let anchor = match cwd_anchor() {
                Ok(anchor) => anchor,
                Err(code) => return code,
            };
            let (mut config, config_source) =
                match resolve_config(config_arg.as_deref(), no_config, &anchor) {
                    Ok(resolved) => resolved,
                    Err(code) => return code,
                };
            let exclude_filter =
                match build_exclude_filter(&config, &config_source, &anchor, &exclude) {
                    Ok(filter) => filter.with_force_exclude(force_exclude),
                    Err(code) => return code,
                };
            // CLI `--select`/`--ignore` override the configured selection when given.
            if !select.is_empty() {
                config.lint.select = Some(select);
            }
            if !ignore.is_empty() {
                config.lint.ignore = ignore;
            }
            let (rules, unknown) =
                RuleSelection::resolve(config.lint.select.as_deref(), &config.lint.ignore);
            for id in &unknown {
                eprintln!("badness: warning: unknown lint rule `{id}`");
            }
            run_lint(
                &paths,
                fix,
                unsafe_fixes,
                stdin_filepath.as_deref(),
                &exclude_filter,
                &rules,
                lint_output_mode(output),
            )
        }
        Command::Parse { path } => run_parse(path.as_deref()),
        Command::Lsp => run_lsp(),
        Command::InverseSearch {
            input,
            line,
            line0,
            character,
            ipc_dir,
        } => run_inverse_search(&input, line, line0, character, ipc_dir.as_deref()),
        Command::Init { force } => run_init(force),
        Command::Debug { command } => match command {
            DebugCommand::Format {
                paths,
                checks,
                line_width,
                wrap,
                report,
                dump_dir,
                dump_passes,
                exclude,
                force_exclude,
            } => {
                let anchor = match cwd_anchor() {
                    Ok(anchor) => anchor,
                    Err(code) => return code,
                };
                let (config, config_source) =
                    match resolve_config(config_arg.as_deref(), no_config, &anchor) {
                        Ok(resolved) => resolved,
                        Err(code) => return code,
                    };
                let exclude_filter =
                    match build_exclude_filter(&config, &config_source, &anchor, &exclude) {
                        Ok(filter) => filter.with_force_exclude(force_exclude),
                        Err(code) => return code,
                    };
                let (style, wrap_override) =
                    resolve_style(&config, line_width, None, wrap, None, None);
                let mut abbrev_scratch = Vec::new();
                let sentence = SentenceOptions::resolve(
                    config.format.lang.as_deref(),
                    &config.format.no_break_abbreviations,
                    &mut abbrev_scratch,
                );
                run_debug_format(
                    &paths,
                    checks,
                    report,
                    dump_dir.as_deref(),
                    dump_passes,
                    style,
                    wrap_override,
                    sentence,
                    &exclude_filter,
                )
            }
            DebugCommand::EchoArgs { out, args } => run_debug_echo_args(&out, &args),
        },
    }
}

/// `badness debug echo-args`: record the trailing arguments, one per line.
///
/// The forward-search tests configure this as their PDF viewer, so what lands in
/// `out` is exactly the argument vector the viewer would have been launched with.
fn run_debug_echo_args(out: &Path, args: &[String]) -> ExitCode {
    let mut body = args.join("\n");
    if !args.is_empty() {
        body.push('\n');
    }
    match std::fs::write(out, body) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("badness: failed to write {}: {err}", out.display());
            ExitCode::from(2)
        }
    }
}

/// Resolve the effective [`FormatStyle`] and wrap override from the config plus
/// the CLI flags (each `None` when not given).
///
/// Wrap precedence: `--wrap` > config `wrap` > file-kind default. The override
/// is `None` only when neither is set, leaving each file on its kind's default
/// wrap (`.sty`/`.cls`/`.dtx`/`.ins` → Preserve, `.tex` → Reflow), resolved per
/// file at dispatch. Math-wrap precedence: `--math-wrap` > config `math-wrap` >
/// `auto`; the style already carries the config value (or `Auto`), the flag
/// just overwrites it, and `Auto` resolves against the effective wrap inside
/// the formatter, so no per-file dispatch is needed here.
fn resolve_style(
    config: &Config,
    line_width: Option<usize>,
    indent_width: Option<usize>,
    wrap: Option<WrapArg>,
    math_wrap: Option<MathWrapArg>,
    line_ending: Option<LineEndingArg>,
) -> (FormatStyle, Option<WrapMode>) {
    let mut style = FormatStyle::from(&config.format);
    if let Some(w) = line_width {
        style.line_width = w;
    }
    if let Some(w) = indent_width {
        style.indent_width = w;
    }
    let wrap_override: Option<WrapMode> =
        wrap.map(wrap_mode).or(config.format.wrap.map(Into::into));
    if let Some(mw) = math_wrap {
        style.math_wrap = math_wrap_mode(mw);
    }
    // Same precedence story as `math-wrap`: the style already carries the config
    // value (or `Auto`), and `Auto` resolves per document inside the formatter.
    if let Some(le) = line_ending {
        style.line_ending = line_ending_mode(le);
    }
    (style, wrap_override)
}

/// The directory to anchor config discovery and exclude-pattern roots at: the
/// current working directory.
fn cwd_anchor() -> Result<PathBuf, ExitCode> {
    std::env::current_dir().map_err(|err| {
        eprintln!("badness: cannot determine the current directory: {err}");
        ExitCode::from(2)
    })
}

/// Resolve the effective config, mapping any [`ConfigError`] to a stderr message
/// and exit code 2.
fn resolve_config(
    explicit: Option<&Path>,
    no_config: bool,
    anchor: &Path,
) -> Result<(Config, ConfigSource), ExitCode> {
    Config::resolve(explicit, no_config, anchor).map_err(|err| {
        eprintln!("badness: {err}");
        ExitCode::from(2)
    })
}

/// Build the directory-discovery exclude filter from the resolved config plus any
/// `--exclude` CLI patterns. Patterns resolve relative to the directory holding
/// `badness.toml`, or relative to `anchor` for the env and global user configs
/// and the no-config case ([`ConfigSource::exclude_root`]).
fn build_exclude_filter(
    config: &Config,
    source: &ConfigSource,
    anchor: &Path,
    cli_excludes: &[String],
) -> Result<ExcludeFilter, ExitCode> {
    let root = source.exclude_root(anchor);
    let patterns = config.exclude_patterns(cli_excludes);
    ExcludeFilter::new(root, &patterns).map_err(|err| {
        eprintln!("badness: {err}");
        ExitCode::from(2)
    })
}

/// A commented starter `badness.toml` showing every key at its default.
const STARTER_CONFIG: &str = "\
# badness configuration. All keys are optional; values shown are the defaults.

# Gitignore-style patterns to skip during directory discovery. `exclude` replaces
# the built-in default set (`.git/`); `extend-exclude` adds on top of it. Both
# apply to `format` and `lint`.
# exclude = [\".git/\"]
# extend-exclude = []

[format]
# line-width = 80
# indent-width = 2
# wrap = \"reflow\"  # reflow | stable | sentence | semantic | preserve
                     # omit to use each file kind's default
                     # (.tex -> reflow, .sty/.cls/.dtx/.ins -> preserve)
# math-wrap = \"auto\"  # auto | preserve | single-line | break
                        # display-math line breaking; auto derives from wrap
                        # (preserve -> preserve, else break)
# line-ending = \"auto\"  # auto | lf | crlf | native
                          # auto keeps the endings each file was written with

[lint]
# select = [\"...\"]  # if set, only these rules run
# ignore = []        # rules to disable
";

/// `badness inverse-search`: hand a viewer's source position to a running
/// language server.
///
/// The path is canonicalized first: a viewer's `%f` is often relative to the
/// compile directory, or reached through a symlink, and the server matches
/// against the paths its editor opened.
///
/// Exits `2` — the CLI's usage/environment code — with a message naming the
/// likely cause, rather than texlab's `-1` (which reaches the shell as 255 and
/// says nothing). The viewer shows this to the user, so it has to be readable.
fn run_inverse_search(
    input: &Path,
    line: Option<u32>,
    line0: Option<u32>,
    character: u32,
    ipc_dir: Option<&Path>,
) -> ExitCode {
    let Some(line) = line.or_else(|| line0.map(|l| l + 1)) else {
        eprintln!(
            "badness: pass --line (counting from 1, what most viewers emit) \
             or --line0 (counting from 0)"
        );
        return ExitCode::from(2);
    };
    let path = input.canonicalize().unwrap_or_else(|_| input.to_path_buf());
    let dir = ipc_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(badness::ipc::ipc_dir);
    match badness::ipc::send_inverse_search_in(&dir, &path, line, character) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("badness: {err}");
            ExitCode::from(2)
        }
    }
}

/// `badness init`: write a commented starter config to `<cwd>/badness.toml`.
fn run_init(force: bool) -> ExitCode {
    let anchor = match cwd_anchor() {
        Ok(anchor) => anchor,
        Err(code) => return code,
    };
    let path = anchor.join(badness::config::CONFIG_FILE_NAME);
    if path.exists() && !force {
        eprintln!(
            "badness: {} already exists; pass --force to overwrite",
            path.display()
        );
        return ExitCode::from(2);
    }
    match std::fs::write(&path, STARTER_CONFIG) {
        Ok(()) => {
            println!("Wrote {}", path.display());
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("badness: failed to write {}: {err}", path.display());
            ExitCode::from(2)
        }
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

/// Print a rule's description and examples (`lint --explain <rule>`), then exit.
/// The id is looked up in the LaTeX registry first, then the bib registry (the
/// two share one namespace, so at most one matches). Unknown ids exit `2` after
/// listing every known built-in rule id across both linters.
fn run_explain(id: &str) -> ExitCode {
    let doc = badness::linter::docs::explain_rule(id)
        .or_else(|| badness::bib::linter::docs::explain_rule(id));
    match doc {
        Some(doc) => {
            print!("{doc}");
            ExitCode::SUCCESS
        }
        None => {
            eprintln!("badness: unknown lint rule `{id}`");
            eprintln!(
                "known rules: {}",
                badness::linter::rules::all_known_rule_ids()
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            ExitCode::from(2)
        }
    }
}

/// Per-file result of the parallel Phase-1 parse+analyze in [`run_lint`]. Carries
/// only `Send` data across the rayon boundary — a `GreenNode` (Send), never a red
/// `SyntaxNode` (not Send; AGENTS.md decision #7). The red tree is materialized
/// thread-locally to extract facts and dropped before returning; Phase 3
/// re-materializes it from the green node to lint.
/// Result of reading one discovered source in parallel: its `(path, text, kind)`
/// on success, or the `(path, error)` to report on failure.
type ReadResult = Result<(PathBuf, String, FileKind), (PathBuf, std::io::Error)>;

enum FileAnalysis {
    Bib {
        diagnostics: Vec<Diagnostic>,
        path: PathBuf,
        keys: Vec<SmolStr>,
    },
    // Boxed: the `.tex` payload is far larger than the `.bib` one, so an unboxed
    // variant would bloat every `FileAnalysis` to its size.
    Tex(Box<TexAnalysis>),
}

/// The `.tex`/`.sty`/… parse+analyze payload carried by [`FileAnalysis::Tex`].
struct TexAnalysis {
    diagnostics: Vec<Diagnostic>,
    path: PathBuf,
    green: GreenNode,
    model: SemanticModel,
    facts: FileFacts,
    label_input: (PathBuf, Vec<SmolStr>, Vec<SmolStr>, bool),
    cite_fact: CiteFileFacts,
    /// The file's declared-option surface when it is a `.sty`, feeding the
    /// cross-file package-option model (`unknown-option`).
    option_facts: Option<PackageOptionFacts>,
}

/// Parse and analyze one source. Pure and thread-safe (no shared mutable state,
/// no environment access), so [`run_lint`] maps it over all files with rayon. The
/// resolver-feeding facts use the same pure helpers the salsa queries do, so CLI
/// and LSP agree.
fn analyze_source(path: &Path, content: &str, kind: FileKind) -> FileAnalysis {
    match kind {
        FileKind::Bib => {
            // Build the model once: it yields both the lint diagnostics and the
            // cite keys this `.bib` contributes to the citation resolver.
            let parsed = badness::bib::parse(content);
            let mut diagnostics: Vec<Diagnostic> = parsed
                .errors
                .iter()
                .map(|err| Diagnostic {
                    rule: "parse",
                    severity: badness::linter::Severity::Error,
                    path: path.to_path_buf(),
                    start: err.start,
                    end: err.end,
                    message: err.message.clone(),
                    fix: None,
                    related: Vec::new(),
                })
                .collect();
            let root = parsed.syntax();
            let model = badness::bib::semantic::Model::build(&root);
            let keys = model.entries().iter().map(|e| e.key.clone()).collect();
            diagnostics.extend(badness::bib::linter::lint_document(path, &root, &model));
            FileAnalysis::Bib {
                diagnostics,
                path: path.to_path_buf(),
                keys,
            }
        }
        FileKind::Tex
        | FileKind::CodeTex
        | FileKind::Sty
        | FileKind::Cls
        | FileKind::Dtx
        | FileKind::Ins => {
            let parsed = parse_with_flavor(content, kind.lex_config());
            let diagnostics: Vec<Diagnostic> = parsed
                .errors
                .iter()
                .map(|err| Diagnostic::from_parse(path.to_path_buf(), err))
                .collect();
            let green = parsed.green;
            let root = SyntaxNode::new_root(green.clone());
            let model = SemanticModel::build(&root);
            let facts = FileFacts {
                path: path.to_path_buf(),
                include_edges: collect_include_edge_keys(&root, path.parent()),
            };
            let label_input = (
                path.to_path_buf(),
                document_label_names(&model),
                document_ref_names(&model),
                is_document_root(&root),
            );
            let cite_fact = CiteFileFacts {
                path: path.to_path_buf(),
                bib_targets: collect_bib_resource_targets(&root, path.parent()),
                nocite_all: model.has_wildcard_nocite(),
                is_document_root: is_document_root(&root),
            };
            let option_facts = package_option_facts(path, &root, &model);
            FileAnalysis::Tex(Box::new(TexAnalysis {
                diagnostics,
                path: path.to_path_buf(),
                green,
                model,
                facts,
                label_input,
                cite_fact,
                option_facts,
            }))
        }
    }
}

/// Lint each path (or stdin), rendering parse diagnostics. Exits non-zero if
/// any diagnostics are reported or any file fails to read. With `fix`, safe
/// autofixes (plus unsafe ones when `unsafe_fixes` is set) are applied in place
/// first; the reporting pass below then shows whatever findings remain.
fn run_lint(
    paths: &[PathBuf],
    fix: bool,
    unsafe_fixes: bool,
    stdin_filepath: Option<&Path>,
    exclude: &ExcludeFilter,
    rules: &RuleSelection,
    mode: OutputMode,
) -> ExitCode {
    // Apply fixes in place first; the reporting pass below then re-reads from
    // disk and shows whatever findings remain. This is a two-pass flow.
    // Stdin (no paths) has nowhere to write back, so `--fix` only acts on files.
    if fix
        && !paths.is_empty()
        && let Some(code) = apply_fixes_to_paths(paths, unsafe_fixes, exclude, rules)
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
        let files = match collect_lint_files(paths, exclude) {
            Ok(files) => files,
            Err(err) => {
                report_discovery_error(&err);
                return ExitCode::FAILURE;
            }
        };
        if files.is_empty() {
            // Under `--force-exclude` an empty set is expected (a runner like
            // pre-commit may pass only excluded files), so it is a clean no-op.
            if exclude.force() {
                return ExitCode::SUCCESS;
            }
            eprintln!(
                "badness: no .tex, .sty, .cls, .dtx, .ins, or .bib files found under the provided input paths"
            );
            return ExitCode::FAILURE;
        }
        // Read every file in parallel (IO-bound; the OS serves many opens at once).
        // Order-preserving collect keeps `sources` in the discovered (sorted) order,
        // then a serial fold reports read failures deterministically.
        let read_results: Vec<ReadResult> = files
            .par_iter()
            .map(|(path, kind)| match std::fs::read_to_string(path) {
                Ok(content) => Ok((path.clone(), content, *kind)),
                Err(err) => Err((path.clone(), err)),
            })
            .collect();
        for result in read_results {
            match result {
                Ok(source) => sources.push(source),
                Err((path, err)) => {
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
    // firewall is an editor-incrementality concern). The resolver reuses the
    // *same* pure helpers the salsa
    // queries do (`document_label_names`, `is_document_root`,
    // `collect_include_edge_keys`, `ResolvedLabels::build`), so CLI and LSP agree.
    // Phases 1–3 (parse+analyze, cross-file resolution, resolution-aware lint) live
    // in `collect_project_diagnostics` so the `--fix` cross-file pass shares the
    // exact same pipeline — CLI report and CLI fix can never drift.
    let mut diagnostics = collect_project_diagnostics(&sources);

    // Drop findings from rules the config/CLI deselected. Parse diagnostics
    // (`rule == "parse"`) are always kept (see `RuleSelection::is_active`).
    diagnostics.retain(|d| rules.is_active(d.rule));

    // Findings from the two pipelines arrive interleaved by file; sort so the
    // renderer presents them deterministically (by path, then position).
    diagnostics.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then(a.start.cmp(&b.start))
            .then(a.end.cmp(&b.end))
            .then(a.rule.cmp(b.rule))
    });

    if mode == OutputMode::Json {
        // JSON goes to stdout unconditionally (`[]` when clean) so consumers
        // always receive a valid document. It serializes byte offsets and
        // needs no source lookup.
        println!("{}", render_findings(&diagnostics, mode, &|_| None));
    } else if !diagnostics.is_empty() {
        // Index sources by path so the renderer's per-file source lookup is O(1),
        // not a linear scan of every source (quadratic over a large project).
        let source_index: HashMap<&Path, &str> = sources
            .iter()
            .map(|(p, text, _)| (p.as_path(), text.as_str()))
            .collect();
        let source_for = |path: &Path| source_index.get(path).map(|s| s.to_string());
        eprint!("{}", render_findings(&diagnostics, mode, &source_for));
    }

    if failed || !diagnostics.is_empty() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Run the project lint pipeline over in-memory `sources`, returning every
/// finding (parse + lint, unfiltered by rule selection and unsorted).
///
/// This is the CLI's Phases 1–3, factored out of [`run_lint`] so the `--fix`
/// cross-file pass ([`apply_cross_file_fixes`]) lints through the identical
/// path: parse+analyze each source in parallel, build cross-file resolution over
/// the whole set, then lint every LaTeX file with that resolution. `.bib` files
/// are linted standalone (no cross-file resolution yet); their parse+lint
/// findings ride along from the analyze phase.
fn collect_project_diagnostics(sources: &[(PathBuf, String, FileKind)]) -> Vec<Diagnostic> {
    // Phase 1 — parse + analyze every source in parallel. Each task is pure and
    // returns only `Send` data (`analyze_source`); rayon preserves input order.
    let analyses: Vec<FileAnalysis> = sources
        .par_iter()
        .map(|(path, content, kind)| analyze_source(path, content, *kind))
        .collect();

    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    let mut analyzed: Vec<(PathBuf, GreenNode, SemanticModel)> = Vec::new();
    let mut facts: Vec<FileFacts> = Vec::new();
    let mut label_inputs = Vec::new();
    let mut cite_facts: Vec<CiteFileFacts> = Vec::new();
    let mut option_facts: Vec<PackageOptionFacts> = Vec::new();
    // Cite keys per analyzed `.bib` path, feeding the cross-file citation resolver.
    let mut bib_keys: HashMap<PathBuf, Vec<SmolStr>> = HashMap::new();
    for analysis in analyses {
        match analysis {
            FileAnalysis::Bib {
                diagnostics: d,
                path,
                keys,
            } => {
                diagnostics.extend(d);
                bib_keys.insert(path, keys);
            }
            FileAnalysis::Tex(tex) => {
                let TexAnalysis {
                    diagnostics: d,
                    path,
                    green,
                    model,
                    facts: f,
                    label_input,
                    cite_fact,
                    option_facts: o,
                } = *tex;
                diagnostics.extend(d);
                facts.push(f);
                label_inputs.push(label_input);
                cite_facts.push(cite_fact);
                option_facts.extend(o);
                analyzed.push((path, green, model));
            }
        }
    }

    // Phase 2 — cross-file resolution: a serial barrier (needs the whole analyzed
    // set) over the collected facts. Pure graph work, no re-parsing.
    let graph = IncludeGraph::build(&facts, None);
    let resolved = ResolvedLabels::build(&label_inputs, &graph);
    let resolved_citations = ResolvedCitations::build(&cite_facts, &graph, &bib_keys);
    let resolved_packages = ResolvedPackageOptions::build(option_facts);

    // Phase 3 — lint every analyzed file in parallel, sharing the resolution by
    // reference. The red tree is materialized thread-locally from each green node
    // (red trees are not `Send`).
    let lint_results: Vec<Vec<Diagnostic>> = analyzed
        .par_iter()
        .map(|(path, green, model)| {
            let root = SyntaxNode::new_root(green.clone());
            lint_document(
                path,
                &root,
                model,
                Some(&resolved),
                Some(&resolved_citations),
                Some(&resolved_packages),
            )
        })
        .collect();
    for result in lint_results {
        diagnostics.extend(result);
    }
    diagnostics
}

/// Discover lintable files under `paths` and apply autofixes in place. Returns
/// `Some(exit_code)` only on a hard error (discovery / IO); on success returns
/// `None` so the caller falls through to the normal reporting pass.
///
/// Both `.tex` and `.bib` files are fixed, each through its own linter; rules that
/// emit no autofix (the report-only majority) leave their findings for the
/// reporting pass that follows.
fn apply_fixes_to_paths(
    paths: &[PathBuf],
    include_unsafe: bool,
    exclude: &ExcludeFilter,
    rules: &RuleSelection,
) -> Option<ExitCode> {
    let files = match collect_lint_files(paths, exclude) {
        Ok(files) => files,
        Err(err) => {
            report_discovery_error(&err);
            return Some(ExitCode::FAILURE);
        }
    };
    if files.is_empty() {
        if exclude.force() {
            return Some(ExitCode::SUCCESS);
        }
        eprintln!("badness: no .tex or .bib files found under the provided input paths");
        return Some(ExitCode::FAILURE);
    }

    // Fix each file in parallel: `fix_file` is a pure per-file fixpoint (read, lint,
    // apply, write back) with no shared mutable state, and distinct output files
    // never race. The order-preserving collect lets the serial fold below report
    // "n fixes applied" messages and read failures deterministically, in discovered
    // order, mirroring `run_format_paths`.
    let outcomes: Vec<FixOutcome> = files
        .par_iter()
        .map(
            |(path, kind)| match fix_file(path, *kind, include_unsafe, rules) {
                Ok(0) => FixOutcome::Unchanged,
                Ok(n) => FixOutcome::Applied {
                    path: path.clone(),
                    count: n,
                },
                Err(err) => {
                    FixOutcome::Failed(format!("badness: cannot fix {}: {err}", path.display()))
                }
            },
        )
        .collect();

    let mut failed = false;
    for outcome in outcomes {
        match outcome {
            FixOutcome::Unchanged => {}
            FixOutcome::Applied { path, count } => {
                eprintln!("{}: {count} fix{} applied", path.display(), plural(count))
            }
            FixOutcome::Failed(message) => {
                eprintln!("{message}");
                failed = true;
            }
        }
    }

    // Second pass: cross-file fixes, which need whole-project resolution the
    // per-file pass above deliberately lacks. A no-op unless a rule emits a fix
    // that reaches into another file.
    if let Err(err) = apply_cross_file_fixes(&files, include_unsafe, rules) {
        eprintln!("badness: cannot apply cross-file fixes: {err}");
        failed = true;
    }

    failed.then_some(ExitCode::FAILURE)
}

/// Per-file result of the parallel autofix pass in [`apply_fixes_to_paths`],
/// folded serially afterward so messages print in discovered order.
enum FixOutcome {
    /// The file was already clean; nothing to report.
    Unchanged,
    /// `count` fixes were applied to `path`.
    Applied { path: PathBuf, count: usize },
    /// The file could not be fixed; carries the ready-to-print error message.
    Failed(String),
}

/// Run the fixpoint loop on a single file and write it back if anything changed.
/// Returns the number of individual fixes applied. Re-lints after each round so
/// fixes can cascade; bounded by [`MAX_FIX_ITERATIONS`].
/// Routes to the LaTeX or BibTeX linter by [`FileKind`]. `rules` gates
/// which findings contribute fixes, so a deselected rule's autofix never applies.
fn fix_file(
    path: &Path,
    kind: FileKind,
    include_unsafe: bool,
    rules: &RuleSelection,
) -> std::io::Result<usize> {
    let mut content = std::fs::read_to_string(path)?;
    // Tenet #1: a fix owes correctness — the result still parses and is still
    // lossless. Snapshot the pre-fix parse-error count so the debug guard below
    // can assert no fix introduced a *new* syntactic error.
    let errors_before = debug_parse_error_count(&content, kind);
    let mut total = 0usize;
    for _ in 0..MAX_FIX_ITERATIONS {
        let diagnostics = match kind {
            FileKind::Tex
            | FileKind::CodeTex
            | FileKind::Sty
            | FileKind::Cls
            | FileKind::Dtx
            | FileKind::Ins => {
                // Fixpoint loop: only fix-emitting rules can change anything, so run
                // just those each round (report-only rules are surfaced later by the
                // reporting pass).
                check_document_fixable(path, &content, kind.lex_config())
            }
            FileKind::Bib => badness::bib::linter::check_document(path, &content),
        };
        let fixes: Vec<_> = diagnostics
            .into_iter()
            .filter(|d| rules.is_active(d.rule))
            .filter_map(|d| d.fix)
            .collect();
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
        debug_assert_fixes_preserved(path, kind, &content, errors_before);
        std::fs::write(path, &content)?;
    }
    Ok(total)
}

/// Outcome of one [`apply_project_fixes`] call.
struct ProjectFixOutcome {
    /// Fixes fully applied this call.
    applied: usize,
    /// Fixes dropped (malformed, missing target file, or a cross-file conflict).
    skipped_conflicts: usize,
    /// Paths whose text changed, sorted.
    changed: Vec<PathBuf>,
}

/// Apply cross-file `fixes` to the in-memory `files` map, folding each rewritten
/// file back in place. A thin, IO-free wrapper over [`apply_fixes_multi`] so the
/// CLI write-back loop is unit-testable without touching disk. Each fix is paired
/// with its origin path (the finding's own file), which resolves its `None`
/// edits; `Some(_)` edits target other files. Atomicity spans files.
fn apply_project_fixes(
    files: &mut HashMap<PathBuf, String>,
    fixes: &[(PathBuf, Fix)],
    include_unsafe: bool,
) -> ProjectFixOutcome {
    let refs: Vec<(PathBuf, &Fix)> = fixes.iter().map(|(p, f)| (p.clone(), f)).collect();
    let out = apply_fixes_multi(files, &refs, include_unsafe);
    let mut changed: Vec<PathBuf> = out.outputs.keys().cloned().collect();
    changed.sort();
    for (path, text) in out.outputs {
        files.insert(path, text);
    }
    ProjectFixOutcome {
        applied: out.applied,
        skipped_conflicts: out.skipped_conflicts,
        changed,
    }
}

/// Second `--fix` pass: apply **cross-file** fixes (those with an edit touching a
/// file other than the finding's own) atomically across the whole project.
///
/// The per-file pass ([`fix_file`]) runs first and handles every single-file fix;
/// it lints without cross-file resolution, so a rule whose fix spans files is
/// inert there. This pass reads the whole discovered LaTeX set (post per-file
/// pass), lints it through [`collect_project_diagnostics`] — the *same* pipeline
/// the report uses, so resolution matches — keeps only active fixes carrying a
/// cross-file edit, and applies them via [`apply_project_fixes`], writing every
/// changed file back. Bounded by [`MAX_FIX_ITERATIONS`] (resolution is rebuilt
/// each round, since a rename changes the label/ref sets); a rename converges in
/// one round.
///
/// Gated to genuine multi-file sets: a lone file (or none) can host no cross-file
/// edit, so single-file `--fix` skips this pass entirely — no added cost on the
/// dominant path.
fn apply_cross_file_fixes(
    files: &[(PathBuf, FileKind)],
    include_unsafe: bool,
    rules: &RuleSelection,
) -> std::io::Result<()> {
    // Only LaTeX-family files take part in cross-file resolution (`.bib` has none
    // yet); a set of one can't host a cross-file edit.
    let members: Vec<(PathBuf, FileKind)> = files
        .iter()
        .filter(|(_, k)| !matches!(k, FileKind::Bib))
        .cloned()
        .collect();
    if members.len() <= 1 {
        return Ok(());
    }

    // Read the current on-disk text of every member (already carries the per-file
    // pass's edits). Snapshot each file's pre-fix parse-error count for the guard.
    let mut texts: HashMap<PathBuf, String> = HashMap::new();
    let kinds: HashMap<PathBuf, FileKind> = members.iter().cloned().collect();
    let mut errors_before: HashMap<PathBuf, usize> = HashMap::new();
    for (path, kind) in &members {
        let text = std::fs::read_to_string(path)?;
        errors_before.insert(path.clone(), debug_parse_error_count(&text, *kind));
        texts.insert(path.clone(), text);
    }

    let mut changed: BTreeSet<PathBuf> = BTreeSet::new();
    let mut skipped = 0usize;
    for _ in 0..MAX_FIX_ITERATIONS {
        let sources: Vec<(PathBuf, String, FileKind)> = members
            .iter()
            .map(|(p, k)| (p.clone(), texts[p].clone(), *k))
            .collect();
        let fixes: Vec<(PathBuf, Fix)> = collect_project_diagnostics(&sources)
            .into_iter()
            .filter(|d| rules.is_active(d.rule))
            .filter_map(|d| {
                let fix = d.fix?;
                // Local-only fixes were already handled by the per-file pass; this
                // pass owns only the ones that reach into another file.
                fix.edits
                    .iter()
                    .any(|e| e.path.is_some())
                    .then_some((d.path, fix))
            })
            .collect();
        if fixes.is_empty() {
            break;
        }
        let outcome = apply_project_fixes(&mut texts, &fixes, include_unsafe);
        skipped += outcome.skipped_conflicts;
        if outcome.applied == 0 {
            break;
        }
        changed.extend(outcome.changed);
    }
    if skipped > 0 {
        eprintln!(
            "badness: {skipped} cross-file fix{} skipped (conflicting edits)",
            plural(skipped)
        );
    }

    for path in changed {
        let kind = kinds[&path];
        debug_assert_fixes_preserved(&path, kind, &texts[&path], errors_before[&path]);
        std::fs::write(&path, &texts[&path])?;
    }
    Ok(())
}

/// Parse-error count of `content` under `kind`'s flavor, computed only in debug
/// builds (returns `0` in release, where the guard is compiled out). Feeds
/// [`debug_assert_fixes_preserved`].
fn debug_parse_error_count(content: &str, kind: FileKind) -> usize {
    if !cfg!(debug_assertions) {
        return 0;
    }
    match kind {
        FileKind::Bib => badness::bib::parse(content).errors.len(),
        _ => parse_with_flavor(content, kind.lex_config()).errors.len(),
    }
}

/// Debug-only tripwire enforcing tenet #1 on the `--fix` output before it is
/// written back: the fixed text must (1) reconstruct losslessly and (2) carry no
/// *new* parse errors relative to the original (`errors_before`). A fix is a
/// textual edit that owes correctness but never layout, so a mis-built fix span
/// that corrupts structure — deleting a closing brace, splicing at the wrong
/// offset — is exactly what this catches before it reaches disk. Compiled out of
/// release builds (`debug_assert!`), so it costs nothing in shipped binaries.
fn debug_assert_fixes_preserved(path: &Path, kind: FileKind, content: &str, errors_before: usize) {
    if !cfg!(debug_assertions) {
        return;
    }
    let (reconstructed, errors_after) = match kind {
        FileKind::Bib => {
            let parsed = badness::bib::parse(content);
            (parsed.syntax().to_string(), parsed.errors.len())
        }
        _ => {
            let parsed = parse_with_flavor(content, kind.lex_config());
            (
                SyntaxNode::new_root(parsed.green.clone()).to_string(),
                parsed.errors.len(),
            )
        }
    };
    debug_assert_eq!(
        reconstructed,
        content,
        "--fix produced non-lossless output for {}",
        path.display()
    );
    debug_assert!(
        errors_after <= errors_before,
        "--fix introduced {} new parse error(s) in {} ({errors_before} -> {errors_after})",
        errors_after.saturating_sub(errors_before),
        path.display()
    );
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

/// The global output flags (`--color`, `--quiet`), threaded to the commands that
/// write human-facing output.
#[derive(Debug, Clone, Copy)]
struct OutputOptions {
    color: ColorChoice,
    quiet: bool,
}

/// Resolve `--color` against the destination stream. `Auto` honors `NO_COLOR`
/// (any value, per no-color.org) and requires a terminal, so redirected output
/// and CI logs stay plain unless `--color always` is passed.
fn color_enabled(choice: ColorChoice, is_terminal: bool) -> bool {
    match choice {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => std::env::var_os("NO_COLOR").is_none() && is_terminal,
    }
}

#[allow(clippy::too_many_arguments)]
fn run_format(
    paths: &[PathBuf],
    check: bool,
    stdin_filepath: Option<&Path>,
    style: FormatStyle,
    wrap_override: Option<WrapMode>,
    sentence: SentenceOptions<'_>,
    exclude: &ExcludeFilter,
    out: OutputOptions,
) -> ExitCode {
    if check {
        return run_check(paths, style, wrap_override, sentence, exclude, out);
    }
    if paths.is_empty() {
        // Stdin has no directory to walk, so the exclude filter never applies.
        run_format_stdin(stdin_filepath, style, wrap_override, sentence)
    } else {
        run_format_paths(paths, style, wrap_override, sentence, exclude)
    }
}

/// `--check`: report unformatted files, exit code 1 if any.
///
/// The report goes to stdout (only the error path uses stderr) so it can be
/// piped, and each changed file is rendered as a diff — under `--check` nothing
/// is written, so this output is the only account of what would change. That
/// matters most where `--check` is actually used: a CI step log, and a
/// pre-commit hook configured with `args: [--check]`, neither of which has a
/// modified file to inspect afterwards. `--quiet` drops the diffs for callers
/// that want just the file list.
fn run_check(
    paths: &[PathBuf],
    style: FormatStyle,
    wrap_override: Option<WrapMode>,
    sentence: SentenceOptions<'_>,
    exclude: &ExcludeFilter,
    out: OutputOptions,
) -> ExitCode {
    match check_paths_with_style(paths, style, wrap_override, sentence, exclude) {
        Ok(result) => {
            if result.changed_files.is_empty() {
                return ExitCode::SUCCESS;
            }
            if out.quiet {
                for path in result.changed_paths() {
                    println!("would reformat {}", path.display());
                }
            } else {
                let use_color = color_enabled(out.color, std::io::stdout().is_terminal());
                for (idx, file) in result.changed_files.iter().enumerate() {
                    if idx > 0 {
                        println!();
                    }
                    print_diff(file, use_color);
                }
            }
            println!(
                "{} of {} file(s) would be reformatted",
                result.changed_files.len(),
                result.checked_files
            );
            ExitCode::FAILURE
        }
        Err(err) => {
            eprintln!("badness: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Print a unified-style, per-file diff of the formatting change (rustfmt-like:
/// a `Diff in <path>:<line>:` header followed by context-grouped hunks), matching
/// what arity and panache emit.
fn print_diff(file: &ChangedFile, use_color: bool) {
    const RED: &str = "\x1b[31m";
    const GREEN: &str = "\x1b[32m";
    const RESET: &str = "\x1b[0m";

    let diff = TextDiff::from_lines(&file.original, &file.formatted);
    // Three lines of context around each changed region, so a single stray
    // space in a long file does not print the whole file.
    for (idx, group) in diff.grouped_ops(3).iter().enumerate() {
        if idx > 0 {
            println!("---");
        }
        let start = group[0].old_range().start + 1;
        println!("Diff in {}:{}:", file.path.display(), start);
        for op in group {
            for change in diff.iter_changes(op) {
                let (sign, color) = match change.tag() {
                    ChangeTag::Delete => ("-", RED),
                    ChangeTag::Insert => ("+", GREEN),
                    ChangeTag::Equal => (" ", ""),
                };
                // A final line without a trailing newline must not gain one, so
                // the newline is re-emitted only when the change carried it.
                let value = change.value();
                let newline = value.ends_with('\n');
                let line = value.strip_suffix('\n').unwrap_or(value);
                if use_color && !color.is_empty() {
                    print!("{color}{sign}{line}{RESET}");
                } else {
                    print!("{sign}{line}");
                }
                if newline {
                    println!();
                }
            }
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
    sentence: SentenceOptions<'_>,
) -> ExitCode {
    let mut input = String::new();
    if let Err(err) = std::io::stdin().read_to_string(&mut input) {
        eprintln!("badness: cannot read stdin: {err}");
        return ExitCode::FAILURE;
    }
    let kind = stdin_filepath.map_or(FileKind::Tex, file_kind_or_tex);
    style.wrap = wrap_override.unwrap_or_default();
    let formatted = match kind {
        FileKind::Tex
        | FileKind::CodeTex
        | FileKind::Sty
        | FileKind::Cls
        | FileKind::Dtx
        | FileKind::Ins => {
            format_with_style_flavored_sentence(&input, style, kind.lex_config(), sentence)
                .map_err(|e| e.to_string())
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
        FileDiscoveryError::UnsupportedLintFilePath { path } => {
            eprintln!(
                "badness: input file {} is not a .tex, .sty, .cls, .dtx, .ins, or .bib file",
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
    style: FormatStyle,
    wrap_override: Option<WrapMode>,
    sentence: SentenceOptions<'_>,
    exclude: &ExcludeFilter,
) -> ExitCode {
    let files = match collect_lint_files(paths, exclude) {
        Ok(files) => files,
        Err(err) => {
            report_discovery_error(&err);
            return ExitCode::FAILURE;
        }
    };
    if files.is_empty() {
        if exclude.force() {
            return ExitCode::SUCCESS;
        }
        eprintln!(
            "badness: no .tex, .sty, .cls, .dtx, .ins, or .bib files found under the provided input paths"
        );
        return ExitCode::FAILURE;
    }

    // Read, format, and write each file in parallel (formatting is a pure function
    // of input plus shipped data, so it is thread-safe; distinct output files never
    // race). Each task returns `Some(message)` on failure; the order-preserving
    // collect lets the serial fold below report errors deterministically.
    let outcomes: Vec<Option<String>> = files
        .par_iter()
        .map(|(path, kind)| {
            let content = match std::fs::read_to_string(path) {
                Ok(content) => content,
                Err(err) => return Some(format!("badness: cannot read {}: {err}", path.display())),
            };
            let mut style = style;
            style.wrap = wrap_override.unwrap_or_default();
            let formatted = match kind {
                FileKind::Tex
                | FileKind::CodeTex
                | FileKind::Sty
                | FileKind::Cls
                | FileKind::Dtx
                | FileKind::Ins => format_file_with_packages_sentence(
                    &content,
                    path,
                    style,
                    kind.lex_config(),
                    sentence,
                )
                .map_err(|e| e.to_string()),
                FileKind::Bib => {
                    badness::bib::format_with_style(&content, style).map_err(|e| e.to_string())
                }
            };
            match formatted {
                Ok(formatted) => {
                    if formatted != *content
                        && let Err(err) = std::fs::write(path, formatted)
                    {
                        return Some(format!("badness: cannot write {}: {err}", path.display()));
                    }
                    None
                }
                Err(msg) => Some(format!("badness: cannot format {}: {msg}", path.display())),
            }
        })
        .collect();

    let mut failed = false;
    for message in outcomes.into_iter().flatten() {
        eprintln!("{message}");
        failed = true;
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

// --- `debug format`: per-file invariant checks for the CI smoke test --------
//
// The output strings below are load-bearing: the smoke-test workflow
// (`.github/workflows/smoke-test.yml`) classifies failures by grepping logs and
// reports for `idempotency`/`losslessness`/`format-error` and extracts
// `Approx. diff start line: N` from the report. Keep them stable, and keep the
// `format-error` wording free of the substrings `idempot` and `lossless` so a
// formatter refusal is never misclassified as an invariant regression. The
// `trivia` check is deliberately excluded from `--checks all` (the workflow's
// failure classes stay as they are), and its label must likewise stay free of
// the other three substrings.

/// One invariant (or the failure to even run it) checked per file.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CheckKind {
    Losslessness,
    Idempotency,
    Trivia,
    FormatError,
}

impl CheckKind {
    fn label(self) -> &'static str {
        match self {
            CheckKind::Losslessness => "losslessness",
            CheckKind::Idempotency => "idempotency",
            CheckKind::Trivia => "trivia",
            CheckKind::FormatError => "format-error",
        }
    }
}

/// A failed check: the two texts whose divergence is the finding. For
/// `format-error`, `left` is the formatter's error message and `right` is
/// empty (there is nothing to diff).
struct DebugFailure {
    kind: CheckKind,
    left: String,
    right: String,
    /// Extra context shown after the label — the trivia check's offending
    /// variant. `None` for the other kinds.
    detail: Option<String>,
}

/// Everything one file's check run produced: the pass texts (for `--dump-dir`)
/// plus any failures.
#[derive(Default)]
struct DebugArtifacts {
    /// `(input, parsed-reconstruction)` when the losslessness check ran.
    losslessness: Option<(String, String)>,
    /// `(input, once, twice)` when the idempotency check ran to completion.
    idempotency: Option<(String, String, String)>,
    /// The perturbed input (the reproducer) when the trivia check failed; its
    /// two formattings are the failure's `left`/`right`.
    trivia_perturbed: Option<String>,
    failures: Vec<DebugFailure>,
}

/// The `--checks` value as it appears in output (report header and the
/// all-passed line).
fn checks_label(checks: DebugChecksArg) -> &'static str {
    match checks {
        DebugChecksArg::Idempotency => "idempotency",
        DebugChecksArg::Losslessness => "losslessness",
        DebugChecksArg::Trivia => "trivia",
        DebugChecksArg::All => "all",
    }
}

/// Map every character outside `[A-Za-z0-9._-]` to `_`, matching the smoke-test
/// workflow's `sed 's/[^[:alnum:]._-]/_/g'` so it can predict artifact names
/// from a repo-relative path.
fn sanitize_path_for_filename(path: &str) -> String {
    path.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// 1-based line number of the first difference between two texts. When the
/// visible lines all match (the difference is only in trailing newline
/// material) this points just past the common lines; identical texts return 1,
/// which never occurs on a failure.
fn first_diff_line(left: &str, right: &str) -> usize {
    let mut left_lines = left.lines();
    let mut right_lines = right.lines();
    let mut line = 1;
    loop {
        match (left_lines.next(), right_lines.next()) {
            (Some(a), Some(b)) if a == b => line += 1,
            _ => return line,
        }
    }
}

/// Render a minimal diff body: a few common context lines, then the two sides'
/// remaining lines as `-`/`+` runs starting at the first difference, capped so
/// a whole-file divergence stays readable. The smoke-test workflow never
/// parses this block (only `Approx. diff start line`), so first-mismatch
/// granularity is enough — no diff dependency needed.
fn render_window_diff(left: &str, right: &str, out: &mut String) {
    const CONTEXT: usize = 3;
    const MAX_SIDE: usize = 40;
    let start = first_diff_line(left, right) - 1;
    let context_start = start.saturating_sub(CONTEXT);
    for line in left.lines().skip(context_start).take(start - context_start) {
        out.push(' ');
        out.push_str(line);
        out.push('\n');
    }
    for (side, text) in [('-', left), ('+', right)] {
        let mut lines = text.lines().skip(start);
        for line in lines.by_ref().take(MAX_SIDE) {
            out.push(side);
            out.push_str(line);
            out.push('\n');
        }
        if lines.next().is_some() {
            out.push(side);
            out.push_str(" [truncated]\n");
        }
    }
}

/// Build the `--report` Markdown. Contract with the smoke-test workflow: the
/// `### k. \`file\` (kind)` headings carry the parenthesized failure label, and
/// each diffable failure has an `Approx. diff start line: N` bullet.
fn build_debug_report(
    checks: DebugChecksArg,
    files_checked: usize,
    files_skipped: usize,
    failures: &[(String, DebugFailure)],
) -> String {
    let mut out = String::new();
    out.push_str("# Debug-format regression report\n\n");
    out.push_str(&format!(
        "- Checks: `{}`\n- Files checked: {files_checked}\n",
        checks_label(checks)
    ));
    // Only the trivia check skips files today (`.bib` runs nothing under it);
    // parameterize the reason if a second skipping check ever appears.
    if files_skipped > 0 {
        out.push_str(&format!(
            "- Files skipped: {files_skipped} (`.bib` — the trivia oracle is LaTeX-CST-based)\n"
        ));
    }
    out.push_str(&format!("- Failures: {}\n\n", failures.len()));
    if failures.is_empty() {
        out.push_str("All checks passed.\n");
        return out;
    }
    out.push_str("## Failures\n\n");
    for (idx, (file, failure)) in failures.iter().enumerate() {
        out.push_str(&format!(
            "### {}. `{}` ({})\n\n",
            idx + 1,
            file,
            failure.kind.label()
        ));
        if let Some(detail) = &failure.detail {
            out.push_str(&format!("- Variant: `{detail}`\n"));
        }
        if failure.kind == CheckKind::FormatError {
            out.push_str(&format!("- Error: {}\n\n", failure.left));
            continue;
        }
        out.push_str(&format!(
            "- Approx. diff start line: {}\n\n",
            first_diff_line(&failure.left, &failure.right)
        ));
        out.push_str("```diff\n");
        render_window_diff(&failure.left, &failure.right, &mut out);
        out.push_str("```\n\n");
    }
    out
}

/// Write one file's pass texts and failure sides into `dump_dir`. Pass texts
/// are written when their check failed, or always under `--dump-passes`. The
/// `{stem}.idempotency.{input,once,twice}.txt` names are the contract the
/// smoke-test workflow's artifact lookup depends on.
fn write_debug_artifacts(
    dump_dir: &Path,
    stem: &str,
    artifacts: &DebugArtifacts,
    dump_passes: bool,
) -> std::io::Result<()> {
    std::fs::create_dir_all(dump_dir)?;
    let failed = |kind: CheckKind| artifacts.failures.iter().any(|f| f.kind == kind);

    if let Some((input, parsed)) = artifacts.losslessness.as_ref()
        && (dump_passes || failed(CheckKind::Losslessness))
    {
        std::fs::write(
            dump_dir.join(format!("{stem}.losslessness.input.txt")),
            input,
        )?;
        std::fs::write(
            dump_dir.join(format!("{stem}.losslessness.parsed.txt")),
            parsed,
        )?;
    }

    if let Some((input, once, twice)) = artifacts.idempotency.as_ref()
        && (dump_passes || failed(CheckKind::Idempotency))
    {
        std::fs::write(
            dump_dir.join(format!("{stem}.idempotency.input.txt")),
            input,
        )?;
        std::fs::write(dump_dir.join(format!("{stem}.idempotency.once.txt")), once)?;
        std::fs::write(
            dump_dir.join(format!("{stem}.idempotency.twice.txt")),
            twice,
        )?;
    }

    if let Some(perturbed) = artifacts.trivia_perturbed.as_ref()
        && failed(CheckKind::Trivia)
    {
        std::fs::write(
            dump_dir.join(format!("{stem}.trivia.perturbed-input.txt")),
            perturbed,
        )?;
    }

    for failure in &artifacts.failures {
        let kind = failure.kind.label();
        std::fs::write(
            dump_dir.join(format!("{stem}.{kind}.left.txt")),
            &failure.left,
        )?;
        std::fs::write(
            dump_dir.join(format!("{stem}.{kind}.right.txt")),
            &failure.right,
        )?;
    }

    Ok(())
}

/// Run the selected checks over one file's content.
///
/// Losslessness parses under the file's own lex config (like
/// [`debug_assert_fixes_preserved`]) so `.sty`/`.dtx` are checked under their
/// real catcode regime, and compares the CST's text to the input.
///
/// Idempotency formats twice through the same pipeline `badness format` uses.
/// A first-pass [`FormatError`] is a `format-error` finding (the invariant
/// could not be evaluated); a second-pass error on the first pass's own output
/// *is* a fixed-point violation and is reported as `idempotency`.
fn run_debug_checks_for_file(
    path: &Path,
    kind: FileKind,
    content: &str,
    style: FormatStyle,
    wrap_override: Option<WrapMode>,
    sentence: SentenceOptions<'_>,
    checks: DebugChecksArg,
) -> DebugArtifacts {
    let mut artifacts = DebugArtifacts::default();

    if matches!(checks, DebugChecksArg::Losslessness | DebugChecksArg::All) {
        let reconstructed = match kind {
            FileKind::Bib => badness::bib::parse(content).syntax().to_string(),
            _ => parse_with_flavor(content, kind.lex_config())
                .syntax()
                .to_string(),
        };
        artifacts.losslessness = Some((content.to_string(), reconstructed.clone()));
        if reconstructed != content {
            artifacts.failures.push(DebugFailure {
                kind: CheckKind::Losslessness,
                left: content.to_string(),
                right: reconstructed,
                detail: None,
            });
        }
    }

    if matches!(checks, DebugChecksArg::Idempotency | DebugChecksArg::All) {
        let mut style = style;
        style.wrap = wrap_override.unwrap_or_default();
        let fmt = |input: &str| match kind {
            FileKind::Bib => {
                badness::bib::format_with_style(input, style).map_err(|e| e.to_string())
            }
            _ => {
                format_file_with_packages_sentence(input, path, style, kind.lex_config(), sentence)
                    .map_err(|e| e.to_string())
            }
        };
        match fmt(content) {
            Err(msg) => artifacts.failures.push(DebugFailure {
                kind: CheckKind::FormatError,
                left: msg,
                right: String::new(),
                detail: None,
            }),
            Ok(once) => match fmt(&once) {
                Ok(twice) => {
                    artifacts.idempotency =
                        Some((content.to_string(), once.clone(), twice.clone()));
                    if once != twice {
                        artifacts.failures.push(DebugFailure {
                            kind: CheckKind::Idempotency,
                            left: once,
                            right: twice,
                            detail: None,
                        });
                    }
                }
                Err(msg) => {
                    artifacts.idempotency =
                        Some((content.to_string(), once.clone(), String::new()));
                    artifacts.failures.push(DebugFailure {
                        kind: CheckKind::Idempotency,
                        left: once,
                        right: format!("second pass failed to format: {msg}"),
                        detail: None,
                    });
                }
            },
        }
    }

    // The trivia-convergence oracle (opt-in): every TeX-identical
    // newline<->space perturbation must format to a fixed point upholding the
    // invariants — the perturbations synthesize the trivia configurations a
    // hybrid needs, so no corpus file has to land on the right column
    // arithmetic. Wrap is pinned to `reflow` regardless of `--wrap` or the
    // file kind's default (`Preserve` reproduces authored breaks verbatim, so
    // it converges trivially and stresses nothing), and `.bib` files are
    // skipped (the oracle is LaTeX-CST-based). A refusal to format the
    // original is a `format-error` finding, mirroring the idempotency check's
    // first pass.
    if checks == DebugChecksArg::Trivia && kind != FileKind::Bib {
        let mut style = style;
        style.wrap = WrapMode::Reflow;
        let fmt = |input: &str| {
            format_file_with_packages_sentence(input, path, style, kind.lex_config(), sentence)
                .map_err(|e| e.to_string())
        };
        match check_trivia_convergence(content, kind.lex_config(), DEFAULT_SINGLE_FLIP_SAMPLES, fmt)
        {
            Ok(_) => {}
            Err(ConvergenceError::Original(msg)) => artifacts.failures.push(DebugFailure {
                kind: CheckKind::FormatError,
                left: msg,
                right: String::new(),
                detail: None,
            }),
            Err(ConvergenceError::Violation(failure)) => {
                artifacts.trivia_perturbed = Some(failure.perturbed_input);
                artifacts.failures.push(DebugFailure {
                    kind: CheckKind::Trivia,
                    left: failure.once,
                    right: failure.twice,
                    detail: Some(format!("{}, {}", failure.label, failure.reason)),
                });
            }
        }
    }

    artifacts
}

/// `badness debug format`: check invariants over the discovered files, writing
/// nothing back. Exit 0 when everything passes, 1 on any failure or unreadable
/// file (config and discovery errors keep their usual codes upstream).
#[allow(clippy::too_many_arguments)]
fn run_debug_format(
    paths: &[PathBuf],
    checks: DebugChecksArg,
    report: bool,
    dump_dir: Option<&Path>,
    dump_passes: bool,
    style: FormatStyle,
    wrap_override: Option<WrapMode>,
    sentence: SentenceOptions<'_>,
    exclude: &ExcludeFilter,
) -> ExitCode {
    if paths.is_empty() {
        eprintln!("badness: debug format requires at least one file or directory");
        return ExitCode::from(2);
    }
    let files = match collect_lint_files(paths, exclude) {
        Ok(files) => files,
        Err(err) => {
            report_discovery_error(&err);
            return ExitCode::FAILURE;
        }
    };
    if files.is_empty() {
        if exclude.force() {
            return ExitCode::SUCCESS;
        }
        eprintln!(
            "badness: no .tex, .sty, .cls, .dtx, .ins, or .bib files found under the provided input paths"
        );
        return ExitCode::FAILURE;
    }

    // Checks are pure functions of the file content, so they parallelize like
    // `run_format_paths`; the order-preserving collect keeps output and report
    // numbering deterministic.
    let outcomes: Vec<(String, FileKind, Result<DebugArtifacts, String>)> = files
        .par_iter()
        .map(|(path, kind)| {
            let label = path.display().to_string();
            let outcome = match std::fs::read_to_string(path) {
                Ok(content) => Ok(run_debug_checks_for_file(
                    path,
                    *kind,
                    &content,
                    style,
                    wrap_override,
                    sentence,
                    checks,
                )),
                Err(err) => Err(format!("badness: cannot read {label}: {err}")),
            };
            (label, *kind, outcome)
        })
        .collect();

    let mut files_checked = 0usize;
    let mut files_skipped = 0usize;
    let mut io_failed = false;
    let mut collected: Vec<(String, DebugFailure)> = Vec::new();
    for (label, kind, outcome) in outcomes {
        match outcome {
            Err(msg) => {
                eprintln!("{msg}");
                io_failed = true;
            }
            Ok(artifacts) => {
                // A `.bib` file under `--checks trivia` runs nothing (the
                // oracle is LaTeX-CST-based): count it as skipped, not
                // checked, so the summary reports real oracle coverage.
                if checks == DebugChecksArg::Trivia && kind == FileKind::Bib {
                    files_skipped += 1;
                } else {
                    files_checked += 1;
                }
                if let Some(dir) = dump_dir {
                    let stem = sanitize_path_for_filename(&label);
                    if let Err(err) = write_debug_artifacts(dir, &stem, &artifacts, dump_passes) {
                        eprintln!(
                            "badness: cannot write debug artifacts to {}: {err}",
                            dir.display()
                        );
                        io_failed = true;
                    }
                }
                for failure in artifacts.failures {
                    if !report {
                        match &failure.detail {
                            Some(detail) => eprintln!(
                                "Debug check failed ({}: {detail}) in {label}",
                                failure.kind.label()
                            ),
                            None => eprintln!(
                                "Debug check failed ({}) in {label}",
                                failure.kind.label()
                            ),
                        }
                        if failure.kind == CheckKind::FormatError {
                            eprintln!("  {}", failure.left);
                        }
                    }
                    collected.push((label.clone(), failure));
                }
            }
        }
    }

    if report {
        print!(
            "{}",
            build_debug_report(checks, files_checked, files_skipped, &collected)
        );
    } else if collected.is_empty() && !io_failed {
        if files_skipped > 0 {
            println!(
                "All checks passed (checks: {}, files: {files_checked}, skipped: {files_skipped})",
                checks_label(checks)
            );
        } else {
            println!(
                "All checks passed (checks: {}, files: {files_checked})",
                checks_label(checks)
            );
        }
    }
    if collected.is_empty() && !io_failed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use badness::linter::Edit;

    #[test]
    fn color_choice_overrides_ignore_the_terminal() {
        assert!(color_enabled(ColorChoice::Always, false));
        assert!(!color_enabled(ColorChoice::Never, true));
    }

    #[test]
    fn color_auto_is_off_when_not_a_terminal() {
        // Env-independent: a redirected stream (a CI step log, a pipe) is plain
        // whatever `NO_COLOR` says, which is what keeps the `--check` diff
        // readable in captured output.
        assert!(!color_enabled(ColorChoice::Auto, false));
    }

    fn file_map(entries: &[(&str, &str)]) -> HashMap<PathBuf, String> {
        entries
            .iter()
            .map(|(p, t)| (PathBuf::from(p), (*t).to_string()))
            .collect()
    }

    #[test]
    fn project_fix_applies_cross_file_edit_atomically() {
        // Rename `x` -> `y` in a.tex's `\label` and b.tex's `\ref`.
        let mut files = file_map(&[("a.tex", "\\label{x}\n"), ("b.tex", "\\ref{x}\n")]);
        let fix = Fix::safe_edits(
            vec![
                Edit::new(7, 8, "y"),
                Edit::in_file(PathBuf::from("b.tex"), 5, 6, "y"),
            ],
            "rename x to y",
        );
        let out = apply_project_fixes(&mut files, &[(PathBuf::from("a.tex"), fix)], false);
        assert_eq!(out.applied, 1);
        assert_eq!(out.skipped_conflicts, 0);
        assert_eq!(
            out.changed,
            vec![PathBuf::from("a.tex"), PathBuf::from("b.tex")]
        );
        assert_eq!(files[&PathBuf::from("a.tex")], "\\label{y}\n");
        assert_eq!(files[&PathBuf::from("b.tex")], "\\ref{y}\n");
    }

    #[test]
    fn cross_file_pass_is_a_safe_noop_without_a_cross_file_rule() {
        // End-to-end driver check: a real two-file project on disk. No rule emits
        // a cross-file fix today, so the pass must run resolution + lint and write
        // nothing, leaving both files byte-identical.
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main.tex");
        let chap = dir.path().join("chap.tex");
        let main_src = "\\documentclass{article}\n\\input{chap}\n\\ref{a}\n";
        let chap_src = "\\label{a}\n";
        std::fs::write(&main, main_src).unwrap();
        std::fs::write(&chap, chap_src).unwrap();

        let files = vec![(main.clone(), FileKind::Tex), (chap.clone(), FileKind::Tex)];
        apply_cross_file_fixes(&files, false, &RuleSelection::all()).unwrap();

        assert_eq!(std::fs::read_to_string(&main).unwrap(), main_src);
        assert_eq!(std::fs::read_to_string(&chap).unwrap(), chap_src);
    }

    #[test]
    fn cross_file_pass_skips_single_file_sets() {
        // A lone file can host no cross-file edit, so the pass returns immediately
        // (and never reads the path, which need not even exist).
        let files = vec![(PathBuf::from("/nonexistent/only.tex"), FileKind::Tex)];
        apply_cross_file_fixes(&files, false, &RuleSelection::all()).unwrap();
    }

    #[test]
    fn project_fix_drops_whole_fix_on_missing_target() {
        // The cross-file edit names a file not in the map: the whole fix is
        // dropped, so the origin file is left untouched too (atomicity).
        let mut files = file_map(&[("a.tex", "\\label{x}\n")]);
        let fix = Fix::safe_edits(
            vec![
                Edit::new(7, 8, "y"),
                Edit::in_file(PathBuf::from("gone.tex"), 5, 6, "y"),
            ],
            "rename x to y",
        );
        let out = apply_project_fixes(&mut files, &[(PathBuf::from("a.tex"), fix)], false);
        assert_eq!(out.applied, 0);
        assert_eq!(out.skipped_conflicts, 1);
        assert!(out.changed.is_empty());
        assert_eq!(files[&PathBuf::from("a.tex")], "\\label{x}\n");
    }

    #[test]
    fn sanitize_matches_the_workflow_sed_class() {
        assert_eq!(
            sanitize_path_for_filename("sub dir/a b.tex"),
            "sub_dir_a_b.tex"
        );
        assert_eq!(sanitize_path_for_filename("ok-1.2_x.bib"), "ok-1.2_x.bib");
        // Non-ASCII maps per *character* (`å`, `/`, `ü` → three `_`). GNU sed in
        // a UTF-8 locale may instead keep non-ASCII alnums; the workflow's
        // artifact lookups are existence-guarded, so that divergence only
        // drops the artifact link for such paths.
        assert_eq!(sanitize_path_for_filename("å/ü.tex"), "___.tex");
    }

    #[test]
    fn first_diff_line_finds_the_first_mismatch() {
        assert_eq!(first_diff_line("a\nb\nc\n", "a\nB\nc\n"), 2);
        assert_eq!(first_diff_line("a\n", "a\nb\n"), 2);
        assert_eq!(first_diff_line("x", "y"), 1);
        // Difference past the last visible line (trailing newline material).
        assert_eq!(first_diff_line("a\nb", "a\nb\n\n"), 3);
    }

    #[test]
    fn report_carries_the_ci_contract_strings() {
        let failures = vec![(
            "sub/file.tex".to_string(),
            DebugFailure {
                kind: CheckKind::Idempotency,
                left: "a\nb\nc\n".to_string(),
                right: "a\nB\nc\n".to_string(),
                detail: None,
            },
        )];
        let report = build_debug_report(DebugChecksArg::All, 3, 0, &failures);
        assert!(report.contains("# Debug-format regression report"));
        assert!(report.contains("- Files checked: 3"));
        assert!(report.contains("### 1. `sub/file.tex` (idempotency)"));
        assert!(report.contains("- Approx. diff start line: 2"));
        assert!(report.contains("```diff\n a\n-b\n-c\n+B\n+c\n```"));
    }

    #[test]
    fn report_on_all_passing_files_has_no_failure_sections() {
        let report = build_debug_report(DebugChecksArg::All, 2, 0, &[]);
        assert!(report.contains("- Failures: 0"));
        assert!(report.contains("All checks passed."));
        assert!(!report.contains("## Failures"));
    }

    #[test]
    fn format_error_report_entry_avoids_the_invariant_substrings() {
        let failures = vec![(
            "bad.tex".to_string(),
            DebugFailure {
                kind: CheckKind::FormatError,
                left:
                    "input contains 1 parser diagnostic(s); formatter only supports parseable input"
                        .to_string(),
                right: String::new(),
                detail: None,
            },
        )];
        let report = build_debug_report(DebugChecksArg::Idempotency, 1, 0, &failures);
        assert!(report.contains("### 1. `bad.tex` (format-error)"));
        let lower = report.to_lowercase();
        // `- Checks: `idempotency`` is the run configuration, not a failure
        // label; strip the header before asserting the classification
        // guarantee on the failure section.
        let failure_section = &lower[lower.find("## failures").unwrap()..];
        assert!(!failure_section.contains("idempot"));
        assert!(!failure_section.contains("lossless"));
    }
}
