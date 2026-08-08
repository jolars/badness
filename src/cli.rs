//! The `badness` command-line surface.
//!
//! Kept as a self-contained module (referencing only `std` and `clap`) so that
//! `build.rs` can `#[path = "src/cli.rs"]`-include it to generate man pages,
//! shell completions, and the markdown CLI reference, exactly as arity does.
//! Conversions to library types (e.g. [`WrapArg`] → `formatter::WrapMode`) live
//! in `main.rs`, never here, so the file compiles inside the build script too.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// CLI surface for `formatter::WrapMode`. Kept here (not in the formatter) so the
/// formatter API stays clap-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum WrapArg {
    /// Greedy fill: wrap words to the line width (default).
    Reflow,
    /// Preserve acceptable authored breaks and rebalance only nearby text
    /// (revision-stable wrapping).
    Stable,
    /// One sentence per line (line width ignored).
    Sentence,
    /// Semantic line breaks (sembr.org): keep authored breaks and add breaks at
    /// sentence boundaries.
    Semantic,
    /// Leave authored line breaks untouched.
    Preserve,
}

/// When to colorize output. Mirrors arity's global `--color` so the two CLIs
/// agree on the spelling and on `NO_COLOR`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum ColorChoice {
    /// Colorize when writing to a terminal and `NO_COLOR` is unset (default).
    #[default]
    Auto,
    /// Always colorize.
    Always,
    /// Never colorize.
    Never,
}

/// CLI surface for the lint renderer's `OutputMode`. Kept here for the same
/// reason as [`WrapArg`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LintOutput {
    /// Source-snippet output with caret spans, on stderr (default).
    Pretty,
    /// One `path:line:col: severity [rule] message` line per finding, on
    /// stderr.
    Concise,
    /// A machine-readable JSON array of findings on stdout (`[]` when clean),
    /// with byte-offset ranges and fix data.
    Json,
}

/// CLI surface for `formatter::MathWrap` (display-math line breaking). Kept
/// here for the same reason as [`WrapArg`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum MathWrapArg {
    /// Derive from the effective wrap mode: preserve → preserve, else break
    /// (default).
    Auto,
    /// Keep authored line breaks inside display-math bodies.
    Preserve,
    /// Never insert breaks; a long body overflows the line width.
    SingleLine,
    /// Break a too-long body before its top-level operators (amsmath style).
    Break,
}

/// CLI surface for `formatter::LineEnding`. Kept here for the same reason as
/// [`WrapArg`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LineEndingArg {
    /// Keep the endings the file was written with (default).
    Auto,
    /// Always LF (`\n`).
    Lf,
    /// Always CRLF (`\r\n`).
    Crlf,
    /// The platform's convention: CRLF on Windows, LF elsewhere.
    Native,
}

#[derive(Parser)]
#[command(
    name = "badness",
    version,
    about = "A formatter, linter, and language server for LaTeX"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
    /// Path to a `badness.toml` to use instead of discovering one. Applies to
    /// `format` and `lint`; ignored by `parse`, `lsp`, and `init`.
    #[arg(long, value_name = "PATH", global = true, conflicts_with = "no_config")]
    pub config: Option<PathBuf>,
    /// Ignore any `badness.toml` (project, `$BADNESS_CONFIG`, or global) and
    /// use built-in defaults.
    #[arg(long, global = true)]
    pub no_config: bool,
    /// When to use color in output.
    #[arg(long, value_enum, default_value_t = ColorChoice::Auto, global = true, value_name = "WHEN")]
    pub color: ColorChoice,
    /// Suppress non-essential output (errors are still shown). Under
    /// `format --check` this drops the per-file diff, leaving the list of files
    /// that would be reformatted and the summary.
    #[arg(long, short = 'q', global = true)]
    pub quiet: bool,
}

#[derive(Subcommand)]
pub enum Command {
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
        /// How to lay out line breaks inside display math.
        #[arg(long, value_enum)]
        math_wrap: Option<MathWrapArg>,
        /// How to spell the line breaks in the formatted output.
        #[arg(long, value_enum)]
        line_ending: Option<LineEndingArg>,
        /// Gitignore-style pattern to skip during directory discovery (repeatable).
        /// Added on top of any `exclude`/`extend-exclude` from `badness.toml`.
        #[arg(long, value_name = "PATTERN")]
        exclude: Vec<String>,
        /// Apply exclude patterns to files named explicitly on the command line
        /// too (they are normally always processed). For runners like pre-commit
        /// that pass staged files as arguments.
        #[arg(long)]
        force_exclude: bool,
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
        /// Gitignore-style pattern to skip during directory discovery (repeatable).
        /// Added on top of any `exclude`/`extend-exclude` from `badness.toml`.
        #[arg(long, value_name = "PATTERN")]
        exclude: Vec<String>,
        /// Apply exclude patterns to files named explicitly on the command line
        /// too (they are normally always processed). For runners like pre-commit
        /// that pass staged files as arguments.
        #[arg(long)]
        force_exclude: bool,
        /// Run only these rules (repeatable). Overrides `[lint] select` from
        /// `badness.toml` when given.
        #[arg(long, value_name = "RULE")]
        select: Vec<String>,
        /// Disable these rules (repeatable). Overrides `[lint] ignore` from
        /// `badness.toml` when given.
        #[arg(long, value_name = "RULE")]
        ignore: Vec<String>,
        /// Print the description and examples for a rule id, then exit. Ignores
        /// paths, config, and fixes.
        #[arg(long, value_name = "RULE")]
        explain: Option<String>,
        /// Output format for findings. The human modes write to stderr; `json`
        /// writes to stdout.
        #[arg(long, value_enum, default_value_t = LintOutput::Pretty)]
        output: LintOutput,
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
    /// Write a commented starter `badness.toml` to the current directory.
    Init {
        /// Overwrite an existing `badness.toml`.
        #[arg(long)]
        force: bool,
    },
    /// Debug utilities for parser and formatter diagnostics, and test
    /// scaffolding.
    ///
    /// Intended for CI smoke tests and local triage; hidden from help and the
    /// generated docs, and covered by no stability promise.
    #[command(hide = true)]
    Debug {
        #[command(subcommand)]
        command: DebugCommand,
    },
}

/// Subcommands under `badness debug`.
#[derive(Subcommand)]
pub enum DebugCommand {
    /// Check formatter and parser invariants per file, writing nothing back.
    ///
    /// Runs the selected checks (losslessness: `reconstruct(x) == x`;
    /// idempotency: `fmt(fmt(x)) == fmt(x)`; trivia: every perturbed
    /// `fmt(perturb(x))` is a fixed point, opt-in) over each input file.
    /// `--report` emits a Markdown summary to stdout; `--dump-dir` writes
    /// per-pass artifacts for triage.
    Format {
        /// Files or directories to check.
        paths: Vec<PathBuf>,
        /// Which invariant checks to run.
        #[arg(long, value_enum, default_value = "all")]
        checks: DebugChecksArg,
        /// Maximum line width before the formatter breaks a line (overrides
        /// config; the multi-width corpus sweep's knob).
        #[arg(long)]
        line_width: Option<usize>,
        /// How to lay out line breaks inside a paragraph (the trivia check
        /// ignores this and pins `reflow`).
        #[arg(long, value_enum)]
        wrap: Option<WrapArg>,
        /// Emit a Markdown report to stdout instead of log lines.
        #[arg(long)]
        report: bool,
        /// Directory where per-pass artifacts are written on failure.
        #[arg(long, value_name = "DIR")]
        dump_dir: Option<PathBuf>,
        /// Write pass artifacts even when all checks pass.
        #[arg(long, requires = "dump_dir")]
        dump_passes: bool,
        /// Gitignore-style pattern to skip during directory discovery (repeatable).
        /// Added on top of any `exclude`/`extend-exclude` from `badness.toml`.
        #[arg(long, value_name = "PATTERN")]
        exclude: Vec<String>,
        /// Apply exclude patterns to files named explicitly on the command line
        /// too (they are normally always processed).
        #[arg(long)]
        force_exclude: bool,
    },
    /// Write the trailing arguments to a file, one per line, and exit 0.
    ///
    /// A stand-in PDF viewer for the forward-search integration tests: it is the
    /// one program guaranteed to exist on every platform CI runs on, and it
    /// records exactly what `%f`/`%p`/`%l` expanded to.
    EchoArgs {
        /// File to write the arguments to.
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
        /// The arguments to record. Taken verbatim, so a leading `-` or a `%`
        /// placeholder is never interpreted.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

/// Which checks `badness debug format` runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DebugChecksArg {
    /// Only the formatter fixed-point check: `fmt(fmt(x)) == fmt(x)`.
    Idempotency,
    /// Only the parser round-trip check: `reconstruct(x) == x`.
    Losslessness,
    /// Only the trivia-convergence oracle: every TeX-identical
    /// newline<->space perturbation of the input must format to a fixed
    /// point (`fmt(fmt(p)) == fmt(p)`) upholding the invariants. Wrap is
    /// pinned to `reflow` (`--wrap` ignored); `.bib` files are skipped.
    /// Deliberately *not* part of `all` — the smoke-test workflow's failure
    /// classes stay as they are.
    Trivia,
    /// Losslessness and idempotency (default). Does not include `trivia`.
    All,
}
