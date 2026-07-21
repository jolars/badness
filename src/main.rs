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

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use badness::config::Config;
use badness::file_discovery::{
    ExcludeFilter, FileDiscoveryError, FileKind, collect_lint_files, file_kind_or_tex,
};
use badness::formatter::{
    FormatStyle, SentenceOptions, WrapMode, check_paths_with_style,
    format_file_with_packages_sentence, format_with_style_flavored_sentence,
};
use badness::linter::{
    Diagnostic, OutputMode, RuleSelection, apply_fixes, check_document_fixable, lint_document,
    render_findings,
};
use std::collections::HashMap;

use badness::cli::{Cli, Command, WrapArg};
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
use smol_str::SmolStr;

/// Lower the CLI [`WrapArg`] to the formatter's [`WrapMode`]. Kept as a free
/// function (not a `From` impl) because the orphan rule forbids implementing a
/// foreign trait for a foreign type in the binary crate, now that both types
/// live in the library.
fn wrap_mode(arg: WrapArg) -> WrapMode {
    match arg {
        WrapArg::Reflow => WrapMode::Reflow,
        WrapArg::Minimal => WrapMode::Minimal,
        WrapArg::Sentence => WrapMode::Sentence,
        WrapArg::Semantic => WrapMode::Semantic,
        WrapArg::Preserve => WrapMode::Preserve,
    }
}

fn main() -> ExitCode {
    let Cli {
        command,
        config: config_arg,
        no_config,
    } = Cli::parse();
    match command {
        Command::Format {
            paths,
            check,
            stdin_filepath,
            line_width,
            wrap_target,
            indent_width,
            wrap,
            exclude,
            force_exclude,
        } => {
            // Discover/load `badness.toml` from the working directory (one config
            // per invocation). The exclude filter is rooted at
            // the config's directory so its patterns resolve relative to it.
            let anchor = match cwd_anchor() {
                Ok(anchor) => anchor,
                Err(code) => return code,
            };
            let (config, config_path) =
                match resolve_config(config_arg.as_deref(), no_config, &anchor) {
                    Ok(resolved) => resolved,
                    Err(code) => return code,
                };
            let exclude_filter =
                match build_exclude_filter(&config, config_path.as_deref(), &anchor, &exclude) {
                    Ok(filter) => filter.with_force_exclude(force_exclude),
                    Err(code) => return code,
                };

            let mut style = FormatStyle::from(&config.format);
            if let Some(w) = line_width {
                style.line_width = w;
            }
            if let Some(w) = wrap_target {
                style.wrap_target = Some(w);
            }
            if let Some(w) = indent_width {
                style.indent_width = w;
            }
            // Wrap precedence: `--wrap` > config `wrap` > file-kind default. The
            // override is `None` only when neither is set, leaving each file on its
            // kind's default wrap (`.sty`/`.cls`/`.dtx`/`.ins` → Preserve, `.tex` →
            // Reflow), resolved per file at dispatch.
            let wrap_override: Option<WrapMode> =
                wrap.map(wrap_mode).or(config.format.wrap.map(Into::into));
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
        } => {
            if let Some(rule) = explain {
                return run_explain(&rule);
            }
            let anchor = match cwd_anchor() {
                Ok(anchor) => anchor,
                Err(code) => return code,
            };
            let (mut config, config_path) =
                match resolve_config(config_arg.as_deref(), no_config, &anchor) {
                    Ok(resolved) => resolved,
                    Err(code) => return code,
                };
            let exclude_filter =
                match build_exclude_filter(&config, config_path.as_deref(), &anchor, &exclude) {
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
            )
        }
        Command::Parse { path } => run_parse(path.as_deref()),
        Command::Lsp => run_lsp(),
        Command::Init { force } => run_init(force),
    }
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
) -> Result<(Config, Option<PathBuf>), ExitCode> {
    Config::resolve(explicit, no_config, anchor).map_err(|err| {
        eprintln!("badness: {err}");
        ExitCode::from(2)
    })
}

/// Build the directory-discovery exclude filter from the resolved config plus any
/// `--exclude` CLI patterns. Patterns resolve relative to the directory holding
/// `badness.toml` (or `anchor` when there is no config file).
fn build_exclude_filter(
    config: &Config,
    config_path: Option<&Path>,
    anchor: &Path,
    cli_excludes: &[String],
) -> Result<ExcludeFilter, ExitCode> {
    let root = config_path
        .and_then(Path::parent)
        .unwrap_or(anchor)
        .to_path_buf();
    let patterns = config.exclude_patterns(cli_excludes);
    ExcludeFilter::new(&root, &patterns).map_err(|err| {
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
# wrap-target = 70  # soft target used only by wrap = \"minimal\"
# indent-width = 2
# wrap = \"reflow\"  # reflow | minimal | sentence | semantic | preserve
                     # omit to use each file kind's default
                     # (.tex -> reflow, .sty/.cls/.dtx/.ins -> preserve)

[lint]
# select = [\"...\"]  # if set, only these rules run
# ignore = []        # rules to disable
";

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
        FileKind::Tex | FileKind::Sty | FileKind::Cls | FileKind::Dtx | FileKind::Ins => {
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
    // Phase 1 — parse + analyze every source in parallel. Each task is pure and
    // returns only `Send` data (`analyze_source`); rayon preserves input order in
    // the collected Vec, so folding it below is deterministic.
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
    // (red trees are not `Send`). Order-preserving collect keeps output stable;
    // the final sort below makes it fully deterministic regardless.
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

    if !diagnostics.is_empty() {
        // Index sources by path so the renderer's per-file source lookup is O(1),
        // not a linear scan of every source (quadratic over a large project).
        let source_index: HashMap<&Path, &str> = sources
            .iter()
            .map(|(p, text, _)| (p.as_path(), text.as_str()))
            .collect();
        let source_for = |path: &Path| source_index.get(path).map(|s| s.to_string());
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
            FileKind::Tex | FileKind::Sty | FileKind::Cls | FileKind::Dtx | FileKind::Ins => {
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

fn run_format(
    paths: &[PathBuf],
    check: bool,
    stdin_filepath: Option<&Path>,
    style: FormatStyle,
    wrap_override: Option<WrapMode>,
    sentence: SentenceOptions<'_>,
    exclude: &ExcludeFilter,
) -> ExitCode {
    if check {
        return run_check(paths, style, wrap_override, sentence, exclude);
    }
    if paths.is_empty() {
        // Stdin has no directory to walk, so the exclude filter never applies.
        run_format_stdin(stdin_filepath, style, wrap_override, sentence)
    } else {
        run_format_paths(paths, style, wrap_override, sentence, exclude)
    }
}

/// `--check`: report unformatted files, exit code 1 if any.
fn run_check(
    paths: &[PathBuf],
    style: FormatStyle,
    wrap_override: Option<WrapMode>,
    sentence: SentenceOptions<'_>,
    exclude: &ExcludeFilter,
) -> ExitCode {
    match check_paths_with_style(paths, style, wrap_override, sentence, exclude) {
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
    sentence: SentenceOptions<'_>,
) -> ExitCode {
    let mut input = String::new();
    if let Err(err) = std::io::stdin().read_to_string(&mut input) {
        eprintln!("badness: cannot read stdin: {err}");
        return ExitCode::FAILURE;
    }
    let kind = stdin_filepath.map_or(FileKind::Tex, file_kind_or_tex);
    style.wrap = wrap_override.unwrap_or(kind.default_wrap());
    let formatted = match kind {
        FileKind::Tex | FileKind::Sty | FileKind::Cls | FileKind::Dtx | FileKind::Ins => {
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
            style.wrap = wrap_override.unwrap_or(kind.default_wrap());
            let formatted = match kind {
                FileKind::Tex | FileKind::Sty | FileKind::Cls | FileKind::Dtx | FileKind::Ins => {
                    format_file_with_packages_sentence(
                        &content,
                        path,
                        style,
                        kind.lex_config(),
                        sentence,
                    )
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
