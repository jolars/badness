//! `badness.toml` configuration: schema, file loading, ancestor-walk
//! discovery, and the `BADNESS_CONFIG`/global user-config fallbacks.
//!
//! The CLI is the only consumer; the library API (`format_with_style`,
//! `check_paths_with_style`, the linter) continues to take a fully-resolved
//! [`FormatStyle`] / [`ExcludeFilter`](crate::file_discovery::ExcludeFilter) /
//! rule selection.
//!
//! Two deliberate design choices:
//!
//! - **Excludes use the Ruff model** (`exclude` + `extend-exclude`): a present
//!   `exclude` *replaces* the built-in
//!   [`DEFAULT_EXCLUDE`] set; `extend-exclude` is always *additive* on top.
//! - **`[format]` carries `wrap`, not `line-ending`** (the LaTeX paragraph
//!   line-break policy). `wrap` is optional: when omitted the formatter falls back
//!   to each file kind's default ([`FileKind::default_wrap`](crate::file_discovery::FileKind::default_wrap)),
//!   so it is resolved per file, not baked into [`FormatStyle`] here.
//!
//! There is no `[index]` section (badness has no R-package index) and no
//! `[lint]`-driven network egress key.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::formatter::{FormatStyle, MathWrap, WrapMode};

pub const CONFIG_FILE_NAME: &str = "badness.toml";

/// Environment variable naming a config file to use when no project
/// `badness.toml` is discovered. Sits between project discovery and the
/// [global user config](global_config_path) in [`Config::resolve`]'s
/// precedence, and shadows the global file entirely when set.
pub const CONFIG_ENV_VAR: &str = "BADNESS_CONFIG";

const MIN_WIDTH: u32 = 1;
const MAX_WIDTH: u32 = 1000;

const DEFAULT_LINE_WIDTH: u32 = 80;
const DEFAULT_INDENT_WIDTH: u32 = 2;

/// Built-in exclude patterns applied when `exclude` is unset (a present `exclude`
/// replaces them). Kept deliberately small: badness only ever processes
/// `.tex`/`.sty`/`.cls`/`.dtx`/`.ins`/`.bib`, so most generated-file noise never
/// reaches discovery anyway. Tune as real-world LaTeX trees demand. `extend-exclude`
/// is always layered on top of whichever base is in effect.
pub const DEFAULT_EXCLUDE: &[&str] = &[".git/"];

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Config {
    /// Gitignore-style patterns to exclude from directory discovery, resolved
    /// relative to the directory containing this `badness.toml`. Applies to *both*
    /// `format` and `lint` (which share one file walk), so it is a top-level key,
    /// not nested under `[format]`.
    ///
    /// When present it **replaces** the built-in [`DEFAULT_EXCLUDE`] set (Ruff's
    /// `exclude` semantics); when absent the defaults apply. Either way,
    /// [`extend_exclude`](Self::extend_exclude) is added on top.
    #[serde(default)]
    pub exclude: Option<Vec<String>>,
    /// Gitignore-style patterns added *in addition to* whichever base set
    /// [`exclude`](Self::exclude) selects (Ruff's `extend-exclude` semantics). Use
    /// this to skip a few extra paths without restating the defaults.
    #[serde(default)]
    pub extend_exclude: Vec<String>,
    #[serde(default)]
    pub format: FormatConfig,
    #[serde(default)]
    pub lint: LintConfig,
    #[serde(default)]
    pub build: BuildConfig,
}

impl Config {
    /// The final, ordered exclude pattern list: the base set (configured
    /// `exclude`, or [`DEFAULT_EXCLUDE`] when unset) followed by `extend-exclude`.
    /// `extra` (the CLI `--exclude` patterns) is appended last so command-line
    /// excludes always layer on top. The
    /// [`ExcludeFilter`](crate::file_discovery::ExcludeFilter) compiles this list.
    pub fn exclude_patterns(&self, extra: &[String]) -> Vec<String> {
        let mut patterns: Vec<String> = match &self.exclude {
            Some(patterns) => patterns.clone(),
            None => DEFAULT_EXCLUDE.iter().map(|p| p.to_string()).collect(),
        };
        patterns.extend(self.extend_exclude.iter().cloned());
        patterns.extend(extra.iter().cloned());
        patterns
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct FormatConfig {
    #[serde(default = "default_line_width")]
    pub line_width: u32,
    #[serde(default = "default_indent_width")]
    pub indent_width: u32,
    /// The paragraph line-break policy. See [`WrapModeConfig`]. When omitted, the
    /// formatter uses each file kind's default
    /// ([`FileKind::default_wrap`](crate::file_discovery::FileKind::default_wrap)):
    /// `.sty`/`.cls`/`.dtx`/`.ins` → `preserve`, `.tex` → `reflow`.
    #[serde(default)]
    pub wrap: Option<WrapModeConfig>,
    /// The display-math line-break policy. See [`MathWrapConfig`]. When omitted
    /// (or `auto`), it derives from the effective `wrap`: `preserve` keeps
    /// authored math breaks, every other wrap mode uses the amsmath-style
    /// breaker.
    #[serde(default)]
    pub math_wrap: Option<MathWrapConfig>,
    /// Document language (a BCP-47-style code, e.g. `en`, `de`, `pt-BR`), used by
    /// the `sentence`/`semantic` wrap modes to pick the sentence-boundary
    /// abbreviation profile. Unknown or absent languages fall back to English.
    /// (Auto-detection from babel/polyglossia is not yet implemented.)
    #[serde(default)]
    pub lang: Option<String>,
    /// User-supplied no-break abbreviations for the `sentence`/`semantic` wrap
    /// modes, keyed by language code or the literal `default` bucket (applied to
    /// every document). An abbreviation here never ends a sentence, so a line is
    /// not broken after it. Merged on top of the built-in per-language lists.
    #[serde(default)]
    pub no_break_abbreviations: BTreeMap<String, Vec<String>>,
}

impl Default for FormatConfig {
    fn default() -> Self {
        Self {
            line_width: DEFAULT_LINE_WIDTH,
            indent_width: DEFAULT_INDENT_WIDTH,
            wrap: None,
            math_wrap: None,
            lang: None,
            no_break_abbreviations: BTreeMap::new(),
        }
    }
}

/// The `wrap` key under `[format]`. A thin, serde-named mirror of [`WrapMode`]
/// (the formatter's own type), kept separate so the TOML spelling (`kebab-case`)
/// is a config concern, not baked into the formatter API — the same split as the
/// CLI's `WrapArg` in `cli.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WrapModeConfig {
    /// Greedy fill: wrap words to the line width.
    Reflow,
    /// Prefer acceptable authored breaks, changing the smallest possible region
    /// (revision-stable wrapping).
    Stable,
    /// One sentence per line (width ignored).
    Sentence,
    /// Semantic line breaks (sembr.org): keep authored breaks and add breaks at
    /// sentence boundaries.
    Semantic,
    /// Leave authored line breaks untouched.
    Preserve,
}

impl From<WrapModeConfig> for WrapMode {
    fn from(value: WrapModeConfig) -> Self {
        match value {
            WrapModeConfig::Reflow => WrapMode::Reflow,
            WrapModeConfig::Stable => WrapMode::Stable,
            WrapModeConfig::Sentence => WrapMode::Sentence,
            WrapModeConfig::Semantic => WrapMode::Semantic,
            WrapModeConfig::Preserve => WrapMode::Preserve,
        }
    }
}

/// The `math-wrap` key under `[format]`. A serde-named mirror of [`MathWrap`],
/// the same split as [`WrapModeConfig`]. Scope: single-formula display math
/// (`\[…\]`, `$$…$$`, a non-grid `equation`); grid environments and inline
/// `$…$` are unaffected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MathWrapConfig {
    /// Derive from the effective `wrap`: `preserve` → `preserve`, else `break`.
    Auto,
    /// Keep authored line breaks inside the body (in-line spacing still
    /// normalized).
    Preserve,
    /// Never insert breaks; a long body overflows the line width.
    SingleLine,
    /// Break a too-long body before its top-level operators (amsmath style).
    Break,
}

impl From<MathWrapConfig> for MathWrap {
    fn from(value: MathWrapConfig) -> Self {
        match value {
            MathWrapConfig::Auto => MathWrap::Auto,
            MathWrapConfig::Preserve => MathWrap::Preserve,
            MathWrapConfig::SingleLine => MathWrap::SingleLine,
            MathWrapConfig::Break => MathWrap::Break,
        }
    }
}

impl FormatConfig {
    /// Validate values, returning a [`ConfigError::InvalidValue`] with the
    /// originating file path (when known) for diagnostics.
    pub fn validate(&self, path: Option<&Path>) -> Result<(), ConfigError> {
        validate_width("line-width", self.line_width, path)?;
        validate_width("indent-width", self.indent_width, path)?;
        Ok(())
    }
}

fn default_line_width() -> u32 {
    DEFAULT_LINE_WIDTH
}

fn default_indent_width() -> u32 {
    DEFAULT_INDENT_WIDTH
}

/// The `[build]` section: where the TeX compiler leaves its artifacts. Read by the
/// language server only (label-number hover and document symbols pull resolved
/// numbers from the `.aux`); never by the formatter or linter, which stay hermetic
/// (see `AGENTS.md`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct BuildConfig {
    /// Directory holding the build's `.aux` files (latexmk's `-auxdir`/`-outdir`),
    /// resolved relative to the root document's directory when not absolute. When
    /// unset, each document's `.aux` is expected next to it (plain
    /// `latex`/`pdflatex` runs).
    #[serde(default)]
    pub aux_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct LintConfig {
    /// Explicit allowlist of rule IDs. When `Some`, only these rules run.
    /// Unknown rule IDs are reported at lint-time, not at config parse-time.
    #[serde(default)]
    pub select: Option<Vec<String>>,
    /// Rule IDs to disable. Applied on top of either `select` (subtracts) or the
    /// default rule set.
    #[serde(default)]
    pub ignore: Vec<String>,
}

impl From<&FormatConfig> for FormatStyle {
    /// Maps the two width knobs. `wrap` is left at [`WrapMode::default`] as a
    /// placeholder: the effective wrap is resolved *per file* by the caller (CLI
    /// flag → configured `wrap` → file-kind default), so this field is always
    /// overwritten before the style reaches the formatter.
    fn from(config: &FormatConfig) -> Self {
        FormatStyle {
            line_width: config.line_width as usize,
            indent_width: config.indent_width as usize,
            wrap: WrapMode::default(),
            // Unlike `wrap`, `math-wrap` has no per-file-kind default: the
            // configured value (or `Auto`) maps straight through, and `Auto`
            // resolves against the effective wrap inside the formatter.
            math_wrap: config.math_wrap.map_or(MathWrap::Auto, Into::into),
        }
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse {
        path: PathBuf,
        line: usize,
        column: usize,
        message: String,
    },
    InvalidValue {
        path: Option<PathBuf>,
        field: &'static str,
        message: String,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "failed to read {}: {source}", path.display())
            }
            Self::Parse {
                path,
                line,
                column,
                message,
            } => write!(f, "{}:{line}:{column}: {message}", path.display()),
            Self::InvalidValue {
                path,
                field,
                message,
            } => match path {
                Some(path) => write!(f, "{}: invalid `{field}`: {message}", path.display()),
                None => write!(f, "invalid `{field}`: {message}"),
            },
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl Config {
    /// Parse a `badness.toml` from disk and validate it.
    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        let text = fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse_str(&text, path)
    }

    fn parse_str(text: &str, path: &Path) -> Result<Self, ConfigError> {
        let config: Self = toml::from_str(text).map_err(|err| {
            let (line, column) = match err.span() {
                Some(span) => byte_offset_to_line_col(text, span.start),
                None => (1, 1),
            };
            ConfigError::Parse {
                path: path.to_path_buf(),
                line,
                column,
                message: err.message().to_string(),
            }
        })?;
        config.validate(Some(path))?;
        Ok(config)
    }

    fn validate(&self, path: Option<&Path>) -> Result<(), ConfigError> {
        self.format.validate(path)
    }

    /// Walk `start` and its ancestors looking for a `badness.toml`. Stops at the
    /// first match or at a directory that contains a `.git` entry (repo root),
    /// whichever comes first. Returns `None` if neither is found before the
    /// filesystem root. The global user config is *not* consulted here; that
    /// fallback lives in [`resolve`](Self::resolve).
    pub fn discover(start: &Path) -> Result<Option<(PathBuf, Self)>, ConfigError> {
        let canonical = start.canonicalize().map_err(|source| ConfigError::Io {
            path: start.to_path_buf(),
            source,
        })?;
        for dir in canonical.ancestors() {
            let candidate = dir.join(CONFIG_FILE_NAME);
            if candidate.is_file() {
                let config = Self::load_from(&candidate)?;
                return Ok(Some((candidate, config)));
            }
            if dir.join(".git").exists() {
                return Ok(None);
            }
        }
        Ok(None)
    }

    /// Config resolution for the CLI and the language server. Precedence:
    /// an explicit `--config` path, then a discovered project `badness.toml`,
    /// then a file named by [`$BADNESS_CONFIG`](CONFIG_ENV_VAR), then the
    /// [global user config](global_config_path), then built-in defaults.
    /// Whole-file fallback, never a merge. `no_config` skips every file
    /// (project, env, and global). CLI flag overrides for the formatter/lint
    /// knobs are applied by the caller after this returns.
    pub fn resolve(
        explicit: Option<&Path>,
        no_config: bool,
        anchor: &Path,
    ) -> Result<(Self, ConfigSource), ConfigError> {
        Self::resolve_with_fallbacks(
            explicit,
            no_config,
            anchor,
            env_config_path().as_deref(),
            global_config_path().as_deref(),
        )
    }

    /// [`resolve`](Self::resolve) with the env and global fallback paths
    /// injected, so tests can exercise them without touching the real
    /// environment or home directory.
    fn resolve_with_fallbacks(
        explicit: Option<&Path>,
        no_config: bool,
        anchor: &Path,
        env: Option<&Path>,
        global: Option<&Path>,
    ) -> Result<(Self, ConfigSource), ConfigError> {
        if no_config {
            return Ok((Self::default(), ConfigSource::None));
        }
        if let Some(path) = explicit {
            let config = Self::load_from(path)?;
            return Ok((config, ConfigSource::Explicit(path.to_path_buf())));
        }
        if let Some((path, config)) = Self::discover(anchor)? {
            return Ok((config, ConfigSource::Discovered(path)));
        }
        // A set `$BADNESS_CONFIG` shadows the global config entirely, and a
        // missing or broken file is a hard error rather than a fall-through:
        // it is the config that would apply, and silently ignoring it would
        // hide a typo'd path indefinitely.
        if let Some(path) = env {
            let config = Self::load_from(path)?;
            return Ok((config, ConfigSource::Env(path.to_path_buf())));
        }
        // Same rationale: a broken global config is a hard error, not a
        // silent fall-through to built-in defaults.
        if let Some(path) = global {
            let config = Self::load_from(path)?;
            return Ok((config, ConfigSource::Global(path.to_path_buf())));
        }
        Ok((Self::default(), ConfigSource::None))
    }
}

/// Which configuration source [`Config::resolve`] loaded, carrying its path.
///
/// The distinction matters for relative exclude patterns: a project-local file
/// anchors them at its own directory, while the global config has no project
/// location and anchors at the caller's directory instead (see
/// [`exclude_root`](Self::exclude_root)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigSource {
    /// Loaded from an explicit `--config <path>`.
    Explicit(PathBuf),
    /// Discovered by the ancestor walk from the input's directory.
    Discovered(PathBuf),
    /// Named by the [`$BADNESS_CONFIG`](CONFIG_ENV_VAR) environment variable,
    /// used when no project config is discovered.
    Env(PathBuf),
    /// The global user config (e.g. `~/.config/badness/config.toml`), used when
    /// no project config is discovered and `$BADNESS_CONFIG` is unset.
    Global(PathBuf),
    /// No config file found; built-in defaults are in use.
    None,
}

impl ConfigSource {
    /// Path of the resolved config file, if any.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Explicit(p) | Self::Discovered(p) | Self::Env(p) | Self::Global(p) => Some(p),
            Self::None => None,
        }
    }

    /// The directory relative exclude patterns resolve against: the config
    /// file's own directory for a project-local file, or `anchor` (the CLI
    /// working directory, or the document's directory in the LSP) for the
    /// env and global configs and the no-config case, which have no project
    /// location.
    pub fn exclude_root<'a>(&'a self, anchor: &'a Path) -> &'a Path {
        match self {
            Self::Explicit(p) | Self::Discovered(p) => p.parent().unwrap_or(anchor),
            Self::Env(_) | Self::Global(_) | Self::None => anchor,
        }
    }
}

/// Path named by the [`$BADNESS_CONFIG`](CONFIG_ENV_VAR) environment variable,
/// or `None` when unset or empty (an empty value counts as unset, the usual
/// shell convention).
fn env_config_path() -> Option<PathBuf> {
    let value = std::env::var_os(CONFIG_ENV_VAR)?;
    if value.is_empty() {
        return None;
    }
    Some(PathBuf::from(value))
}

/// Path to the global user config, the fallback when no project `badness.toml`
/// is discovered: the first existing file among
/// `$XDG_CONFIG_HOME/badness/config.toml`, `~/.config/badness/config.toml`, and
/// `<platform config dir>/badness/config.toml` (Windows `%APPDATA%`, macOS
/// `~/Library/Application Support`). The `~/.config` candidate is checked on
/// every platform so the CLI-dotfile convention works on macOS and Windows too.
fn global_config_path() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        candidates.push(PathBuf::from(xdg));
    }
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".config"));
    }
    if let Some(config) = dirs::config_dir() {
        candidates.push(config);
    }
    candidates
        .into_iter()
        .map(|dir| dir.join("badness").join("config.toml"))
        .find(|path| path.is_file())
}

fn validate_width(field: &'static str, value: u32, path: Option<&Path>) -> Result<(), ConfigError> {
    if !(MIN_WIDTH..=MAX_WIDTH).contains(&value) {
        return Err(ConfigError::InvalidValue {
            path: path.map(Path::to_path_buf),
            field,
            message: format!("must be between {MIN_WIDTH} and {MAX_WIDTH}, got {value}"),
        });
    }
    Ok(())
}

fn byte_offset_to_line_col(source: &str, offset: usize) -> (usize, usize) {
    let mut line = 1usize;
    let mut column = 1usize;
    let clamped = offset.min(source.len());
    for ch in source[..clamped].chars() {
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn parse(text: &str) -> Result<Config, ConfigError> {
        Config::parse_str(text, Path::new("badness.toml"))
    }

    #[test]
    fn default_config_matches_format_style_default_widths() {
        let config = Config::default();
        let style = FormatStyle::from(&config.format);
        assert_eq!(style.line_width, FormatStyle::default().line_width);
        assert_eq!(style.indent_width, FormatStyle::default().indent_width);
    }

    #[test]
    fn empty_file_yields_defaults() {
        let config = parse("").expect("parse");
        assert_eq!(config, Config::default());
    }

    #[test]
    fn parses_minimal_format_section() {
        let config = parse("[format]\nline-width = 100\n").expect("parse");
        let style = FormatStyle::from(&config.format);
        assert_eq!(style.line_width, 100);
        assert_eq!(style.indent_width, 2);
    }

    #[test]
    fn parses_indent_width() {
        let config = parse("[format]\nindent-width = 4\n").expect("parse");
        let style = FormatStyle::from(&config.format);
        assert_eq!(style.indent_width, 4);
        assert_eq!(style.line_width, 80);
    }

    #[test]
    fn wrap_defaults_to_none() {
        let config = parse("[format]\n").expect("parse");
        assert_eq!(config.format.wrap, None);
    }

    #[test]
    fn rejects_texmf_section() {
        // `[texmf]` moved to the LSP editor settings (it is machine configuration,
        // not project data); a leftover section is surfaced, not silently ignored.
        assert!(parse("[texmf]\nenabled = false\n").is_err());
    }

    #[test]
    fn build_aux_dir_defaults_to_none() {
        let config = parse("").expect("parse");
        assert_eq!(config.build.aux_dir, None);
    }

    #[test]
    fn parses_build_section() {
        let config = parse("[build]\naux-dir = \"out\"\n").expect("parse");
        assert_eq!(config.build.aux_dir, Some(PathBuf::from("out")));
    }

    #[test]
    fn parses_wrap_variants() {
        for (key, expected) in [
            ("reflow", WrapModeConfig::Reflow),
            ("stable", WrapModeConfig::Stable),
            ("sentence", WrapModeConfig::Sentence),
            ("semantic", WrapModeConfig::Semantic),
            ("preserve", WrapModeConfig::Preserve),
        ] {
            let text = format!("[format]\nwrap = \"{key}\"\n");
            let config = parse(&text).unwrap_or_else(|e| panic!("parse {key}: {e}"));
            assert_eq!(config.format.wrap, Some(expected), "for {key}");
            assert_eq!(WrapMode::from(expected), expected_wrap_mode(key));
        }
    }

    fn expected_wrap_mode(key: &str) -> WrapMode {
        match key {
            "reflow" => WrapMode::Reflow,
            "stable" => WrapMode::Stable,
            "sentence" => WrapMode::Sentence,
            "semantic" => WrapMode::Semantic,
            "preserve" => WrapMode::Preserve,
            other => panic!("unexpected {other}"),
        }
    }

    #[test]
    fn rejects_unknown_wrap() {
        let err = parse("[format]\nwrap = \"smart\"\n").expect_err("unknown variant");
        assert!(matches!(err, ConfigError::Parse { .. }));
    }

    #[test]
    fn parses_math_wrap_variants() {
        for (key, expected) in [
            ("auto", MathWrapConfig::Auto),
            ("preserve", MathWrapConfig::Preserve),
            ("single-line", MathWrapConfig::SingleLine),
            ("break", MathWrapConfig::Break),
        ] {
            let text = format!("[format]\nmath-wrap = \"{key}\"\n");
            let config = parse(&text).unwrap_or_else(|e| panic!("parse {key}: {e}"));
            assert_eq!(config.format.math_wrap, Some(expected), "for {key}");
        }
    }

    #[test]
    fn math_wrap_defaults_to_none_and_maps_to_auto() {
        let config = parse("[format]\n").expect("parse");
        assert_eq!(config.format.math_wrap, None);
        let style = FormatStyle::from(&config.format);
        assert_eq!(style.math_wrap, MathWrap::Auto);
    }

    #[test]
    fn rejects_unknown_math_wrap() {
        let err = parse("[format]\nmath-wrap = \"never\"\n").expect_err("unknown variant");
        assert!(matches!(err, ConfigError::Parse { .. }));
    }

    #[test]
    fn stable_wrap_target_sits_below_line_width() {
        let config = parse("[format]\nline-width = 100\nwrap = \"stable\"\n").expect("parse");
        let style = FormatStyle::from(&config.format);
        assert_eq!(config.format.wrap, Some(WrapModeConfig::Stable));
        assert_eq!(style.stable_wrap_target(), 85);
    }

    #[test]
    fn rejects_unknown_top_level_table() {
        let err = parse("[formatt]\nline-width = 80\n").expect_err("unknown table");
        match err {
            ConfigError::Parse { message, .. } => {
                assert!(message.contains("formatt"), "got: {message}");
            }
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_field_in_format() {
        let err = parse("[format]\nline-widht = 80\n").expect_err("unknown field");
        match err {
            ConfigError::Parse { message, .. } => {
                assert!(message.contains("line-widht"), "got: {message}");
            }
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_snake_case_keys() {
        // We use kebab-case in the schema; snake_case must be rejected so users get
        // a clear error instead of silent fallthrough to defaults.
        let err = parse("[format]\nline_width = 80\n").expect_err("snake_case");
        assert!(matches!(err, ConfigError::Parse { .. }));
    }

    #[test]
    fn lang_defaults_to_none_and_abbreviations_empty() {
        let config = parse("[format]\n").expect("parse");
        assert_eq!(config.format.lang, None);
        assert!(config.format.no_break_abbreviations.is_empty());
    }

    #[test]
    fn parses_lang_and_no_break_abbreviations() {
        let text = "[format]\nwrap = \"sentence\"\nlang = \"de\"\n\n\
                    [format.no-break-abbreviations]\n\
                    default = [\"ibid.\"]\n\
                    de = [\"bzw.\", \"Abb.\"]\n";
        let config = parse(text).expect("parse");
        assert_eq!(config.format.lang.as_deref(), Some("de"));
        assert_eq!(
            config.format.no_break_abbreviations.get("default"),
            Some(&vec!["ibid.".to_string()])
        );
        assert_eq!(
            config.format.no_break_abbreviations.get("de"),
            Some(&vec!["bzw.".to_string(), "Abb.".to_string()])
        );
    }

    #[test]
    fn rejects_zero_line_width() {
        let err = parse("[format]\nline-width = 0\n").expect_err("zero width");
        match err {
            ConfigError::InvalidValue { field, message, .. } => {
                assert_eq!(field, "line-width");
                assert!(message.contains('0'));
            }
            other => panic!("expected InvalidValue, got {other:?}"),
        }
    }

    #[test]
    fn rejects_huge_line_width() {
        let err = parse("[format]\nline-width = 10000\n").expect_err("too big");
        assert!(matches!(
            err,
            ConfigError::InvalidValue {
                field: "line-width",
                ..
            }
        ));
    }

    #[test]
    fn rejects_negative_width_as_parse_error() {
        // u32 deserialization rejects negatives at the type layer.
        let err = parse("[format]\nline-width = -1\n").expect_err("negative");
        assert!(matches!(err, ConfigError::Parse { .. }));
    }

    #[test]
    fn exclude_defaults_to_none_and_uses_builtin_set() {
        let config = Config::default();
        assert_eq!(config.exclude, None);
        assert!(config.extend_exclude.is_empty());
        // With no `exclude`, the base list is the built-in defaults.
        assert_eq!(
            config.exclude_patterns(&[]),
            DEFAULT_EXCLUDE
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn present_exclude_replaces_defaults() {
        let config = parse("exclude = [\"vendor/\"]\n").expect("parse");
        assert_eq!(
            config.exclude.as_deref(),
            Some(&["vendor/".to_string()][..])
        );
        assert_eq!(config.exclude_patterns(&[]), vec!["vendor/".to_string()]);
    }

    #[test]
    fn empty_exclude_drops_defaults() {
        let config = parse("exclude = []\n").expect("parse");
        assert_eq!(config.exclude.as_deref(), Some(&[][..]));
        assert!(config.exclude_patterns(&[]).is_empty());
    }

    #[test]
    fn extend_exclude_is_additive_over_defaults() {
        let config = parse("extend-exclude = [\"build/\"]\n").expect("parse");
        let mut expected: Vec<String> = DEFAULT_EXCLUDE.iter().map(|p| p.to_string()).collect();
        expected.push("build/".to_string());
        assert_eq!(config.exclude_patterns(&[]), expected);
    }

    #[test]
    fn extend_exclude_layers_on_present_exclude_then_cli() {
        let config =
            parse("exclude = [\"vendor/\"]\nextend-exclude = [\"build/\"]\n").expect("parse");
        assert_eq!(
            config.exclude_patterns(&["tmp/".to_string()]),
            vec![
                "vendor/".to_string(),
                "build/".to_string(),
                "tmp/".to_string(),
            ]
        );
    }

    #[test]
    fn rejects_exclude_under_format() {
        // `exclude` is a top-level key (it governs both format and lint), never
        // nested under `[format]`.
        let err = parse("[format]\nexclude = [\"x\"]\n").expect_err("exclude is top-level");
        assert!(matches!(err, ConfigError::Parse { .. }));
    }

    #[test]
    fn accepts_empty_lint_section() {
        let config = parse("[lint]\n").expect("parse");
        assert_eq!(config.lint, LintConfig::default());
    }

    #[test]
    fn rejects_unknown_field_in_lint() {
        let err = parse("[lint]\nstyle = \"strict\"\n").expect_err("unknown field");
        assert!(matches!(err, ConfigError::Parse { .. }));
    }

    #[test]
    fn parses_lint_select() {
        let config = parse("[lint]\nselect = [\"duplicate-label\"]\n").expect("parse");
        assert_eq!(
            config.lint.select.as_deref(),
            Some(&["duplicate-label".to_string()][..])
        );
    }

    #[test]
    fn parses_lint_ignore() {
        let config = parse("[lint]\nignore = [\"deprecated-command\"]\n").expect("parse");
        assert_eq!(config.lint.ignore, vec!["deprecated-command".to_string()]);
    }

    #[test]
    fn parse_error_reports_file_path_and_line() {
        let path = Path::new("/tmp/oops.toml");
        let err = Config::parse_str("[format]\nbogus = 1\n", path).expect_err("bad field");
        let rendered = err.to_string();
        assert!(rendered.starts_with("/tmp/oops.toml:"));
    }

    #[test]
    fn load_from_missing_file_returns_io_error() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nope.toml");
        let err = Config::load_from(&path).expect_err("missing file");
        assert!(matches!(err, ConfigError::Io { .. }));
    }

    #[test]
    fn discover_finds_config_in_parent() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join(CONFIG_FILE_NAME),
            "[format]\nline-width = 70\n",
        )
        .unwrap();
        let nested = dir.path().join("a").join("b");
        fs::create_dir_all(&nested).unwrap();

        let (path, config) = Config::discover(&nested).expect("discover").expect("found");
        assert_eq!(
            path,
            dir.path().canonicalize().unwrap().join(CONFIG_FILE_NAME)
        );
        assert_eq!(config.format.line_width, 70);
    }

    #[test]
    fn discover_stops_at_git_boundary() {
        let dir = tempdir().unwrap();
        // Ancestor sets a config we must NOT pick up.
        fs::write(
            dir.path().join(CONFIG_FILE_NAME),
            "[format]\nline-width = 70\n",
        )
        .unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let nested = repo.join("src");
        fs::create_dir_all(&nested).unwrap();

        let found = Config::discover(&nested).expect("discover");
        assert!(
            found.is_none(),
            "should stop at .git boundary, got {found:?}"
        );
    }

    #[test]
    fn discover_prefers_config_at_repo_root() {
        let dir = tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::write(repo.join(CONFIG_FILE_NAME), "[format]\nline-width = 70\n").unwrap();
        let nested = repo.join("src");
        fs::create_dir_all(&nested).unwrap();

        let (path, config) = Config::discover(&nested).expect("discover").expect("found");
        assert_eq!(path, repo.canonicalize().unwrap().join(CONFIG_FILE_NAME));
        assert_eq!(config.format.line_width, 70);
    }

    #[test]
    fn resolve_no_config_returns_defaults() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join(CONFIG_FILE_NAME),
            "[format]\nline-width = 20\n",
        )
        .unwrap();
        let (config, source) = Config::resolve(None, true, dir.path()).expect("resolve");
        assert_eq!(config, Config::default());
        assert_eq!(source, ConfigSource::None);
    }

    #[test]
    fn resolve_explicit_overrides_discovery() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join(CONFIG_FILE_NAME),
            "[format]\nline-width = 20\n",
        )
        .unwrap();
        let explicit = dir.path().join("custom.toml");
        fs::write(&explicit, "[format]\nline-width = 40\n").unwrap();

        let (config, source) =
            Config::resolve(Some(&explicit), false, dir.path()).expect("resolve");
        assert_eq!(config.format.line_width, 40);
        assert_eq!(source, ConfigSource::Explicit(explicit.clone()));
    }

    #[test]
    fn resolve_discovers_when_no_explicit_and_not_disabled() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join(CONFIG_FILE_NAME),
            "[format]\nline-width = 50\n",
        )
        .unwrap();
        let (config, source) = Config::resolve(None, false, dir.path()).expect("resolve");
        assert_eq!(config.format.line_width, 50);
        assert!(matches!(source, ConfigSource::Discovered(_)));
    }

    /// A project directory bounded by a `.git` entry, so the discovery walk in
    /// the global-fallback tests never escapes the tempdir.
    fn bounded_project() -> (tempfile::TempDir, PathBuf) {
        let dir = tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        (dir, repo)
    }

    fn write_global(dir: &Path) -> PathBuf {
        let global = dir.join("config-home").join("badness").join("config.toml");
        fs::create_dir_all(global.parent().unwrap()).unwrap();
        fs::write(&global, "[format]\nline-width = 66\n").unwrap();
        global
    }

    #[test]
    fn resolve_falls_back_to_global_config() {
        let (dir, repo) = bounded_project();
        let global = write_global(dir.path());

        let (config, source) =
            Config::resolve_with_fallbacks(None, false, &repo, None, Some(&global))
                .expect("resolve");
        assert_eq!(config.format.line_width, 66);
        assert_eq!(source, ConfigSource::Global(global.clone()));
        // Global config has no project location: excludes anchor at the caller.
        assert_eq!(source.exclude_root(&repo), repo.as_path());
    }

    #[test]
    fn discovered_config_beats_global() {
        let (dir, repo) = bounded_project();
        let global = write_global(dir.path());
        fs::write(repo.join(CONFIG_FILE_NAME), "[format]\nline-width = 50\n").unwrap();

        let (config, source) =
            Config::resolve_with_fallbacks(None, false, &repo, None, Some(&global))
                .expect("resolve");
        assert_eq!(config.format.line_width, 50);
        assert!(matches!(source, ConfigSource::Discovered(_)));
    }

    #[test]
    fn no_config_skips_global() {
        let (dir, repo) = bounded_project();
        let global = write_global(dir.path());

        let (config, source) =
            Config::resolve_with_fallbacks(None, true, &repo, None, Some(&global))
                .expect("resolve");
        assert_eq!(config, Config::default());
        assert_eq!(source, ConfigSource::None);
    }

    #[test]
    fn broken_global_config_is_an_error() {
        let (dir, repo) = bounded_project();
        let global = dir
            .path()
            .join("config-home")
            .join("badness")
            .join("config.toml");
        fs::create_dir_all(global.parent().unwrap()).unwrap();
        fs::write(&global, "[format]\nline-widht = 80\n").unwrap();

        let err = Config::resolve_with_fallbacks(None, false, &repo, None, Some(&global))
            .expect_err("typo'd global config must not be silently ignored");
        assert!(matches!(err, ConfigError::Parse { .. }));
    }

    fn write_env_config(dir: &Path) -> PathBuf {
        let env = dir.join("synced").join("badness.toml");
        fs::create_dir_all(env.parent().unwrap()).unwrap();
        fs::write(&env, "[format]\nline-width = 44\n").unwrap();
        env
    }

    #[test]
    fn env_config_beats_global() {
        let (dir, repo) = bounded_project();
        let env = write_env_config(dir.path());
        let global = write_global(dir.path());

        let (config, source) =
            Config::resolve_with_fallbacks(None, false, &repo, Some(&env), Some(&global))
                .expect("resolve");
        assert_eq!(config.format.line_width, 44);
        assert_eq!(source, ConfigSource::Env(env.clone()));
        // Env config has no project location: excludes anchor at the caller.
        assert_eq!(source.exclude_root(&repo), repo.as_path());
    }

    #[test]
    fn discovered_config_beats_env() {
        let (dir, repo) = bounded_project();
        let env = write_env_config(dir.path());
        fs::write(repo.join(CONFIG_FILE_NAME), "[format]\nline-width = 50\n").unwrap();

        let (config, source) =
            Config::resolve_with_fallbacks(None, false, &repo, Some(&env), None).expect("resolve");
        assert_eq!(config.format.line_width, 50);
        assert!(matches!(source, ConfigSource::Discovered(_)));
    }

    #[test]
    fn explicit_config_beats_env() {
        let (dir, repo) = bounded_project();
        let env = write_env_config(dir.path());
        let explicit = dir.path().join("custom.toml");
        fs::write(&explicit, "[format]\nline-width = 40\n").unwrap();

        let (config, source) =
            Config::resolve_with_fallbacks(Some(&explicit), false, &repo, Some(&env), None)
                .expect("resolve");
        assert_eq!(config.format.line_width, 40);
        assert_eq!(source, ConfigSource::Explicit(explicit.clone()));
    }

    #[test]
    fn no_config_skips_env() {
        let (dir, repo) = bounded_project();
        let env = write_env_config(dir.path());

        let (config, source) =
            Config::resolve_with_fallbacks(None, true, &repo, Some(&env), None).expect("resolve");
        assert_eq!(config, Config::default());
        assert_eq!(source, ConfigSource::None);
    }

    #[test]
    fn missing_env_config_is_an_error() {
        let (dir, repo) = bounded_project();
        let env = dir.path().join("nowhere").join("badness.toml");
        let global = write_global(dir.path());

        // A set but dangling `$BADNESS_CONFIG` must not silently fall through
        // to the global config or the defaults.
        let err = Config::resolve_with_fallbacks(None, false, &repo, Some(&env), Some(&global))
            .expect_err("typo'd $BADNESS_CONFIG path must not be silently ignored");
        assert!(matches!(err, ConfigError::Io { .. }));
    }

    #[test]
    fn broken_env_config_is_an_error() {
        let (dir, repo) = bounded_project();
        let env = dir.path().join("synced").join("badness.toml");
        fs::create_dir_all(env.parent().unwrap()).unwrap();
        fs::write(&env, "[format]\nline-widht = 80\n").unwrap();

        let err = Config::resolve_with_fallbacks(None, false, &repo, Some(&env), None)
            .expect_err("broken $BADNESS_CONFIG file must not be silently ignored");
        assert!(matches!(err, ConfigError::Parse { .. }));
    }

    #[test]
    fn exclude_root_for_project_config_is_its_directory() {
        let source = ConfigSource::Discovered(PathBuf::from("/proj/badness.toml"));
        assert_eq!(
            source.exclude_root(Path::new("/elsewhere")),
            Path::new("/proj")
        );
        let explicit = ConfigSource::Explicit(PathBuf::from("/proj/custom.toml"));
        assert_eq!(
            explicit.exclude_root(Path::new("/elsewhere")),
            Path::new("/proj")
        );
        assert_eq!(
            ConfigSource::None.exclude_root(Path::new("/elsewhere")),
            Path::new("/elsewhere")
        );
    }
}
