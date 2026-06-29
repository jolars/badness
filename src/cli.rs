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
/// formatter API stays clap-free, mirroring arity's `cli.rs` convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum WrapArg {
    /// Greedy fill: wrap words to the line width (default).
    Reflow,
    /// One sentence per line. (Not yet implemented — behaves like `preserve`.)
    Sentence,
    /// Semantic line breaks (sembr.org). (Not yet implemented — like `preserve`.)
    Semantic,
    /// Leave authored line breaks untouched.
    Preserve,
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
    /// Ignore any `badness.toml` and use built-in defaults.
    #[arg(long, global = true)]
    pub no_config: bool,
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
        /// Gitignore-style pattern to skip during directory discovery (repeatable).
        /// Added on top of any `exclude`/`extend-exclude` from `badness.toml`.
        #[arg(long, value_name = "PATTERN")]
        exclude: Vec<String>,
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
        /// Run only these rules (repeatable). Overrides `[lint] select` from
        /// `badness.toml` when given.
        #[arg(long, value_name = "RULE")]
        select: Vec<String>,
        /// Disable these rules (repeatable). Overrides `[lint] ignore` from
        /// `badness.toml` when given.
        #[arg(long, value_name = "RULE")]
        ignore: Vec<String>,
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
}
