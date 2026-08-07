//! Formatter configuration.
//!
//! The LaTeX-specific [`WrapMode`] (paragraph line-break policy, modeled on the
//! `panache` formatter) is the one field specific to badness.
//!
//! Under the optional `serde` feature every type here is (de)serializable, and
//! under `schema` it also derives `schemars::JsonSchema`. Both are off by
//! default: the CLI keeps its own serde-named mirrors in `config.rs` so the
//! TOML spelling stays a config concern. The features exist for embedders that
//! publish a config schema of their own — the dprint plugin borrows these
//! schemas rather than hand-listing the accepted values a third time. The wire
//! spellings are therefore load-bearing and must keep matching `badness.toml`:
//! `kebab-case` throughout (`single-line`, `line-width`).

/// How the formatter lays out the line breaks *inside* a paragraph. Modeled on
/// panache's `WrapMode` (`crates/panache-formatter/src/config.rs`).
///
/// The sentence-boundary detection behind [`WrapMode::Sentence`] and
/// [`WrapMode::Semantic`] is a per-language abbreviation profile
/// (`formatter::sentence`); the language and any user no-break abbreviations are
/// resolved from config into the [`SentenceOptions`](super::SentenceOptions)
/// threaded through the lowering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "kebab-case")
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum WrapMode {
    /// Greedy fill: pack words up to `line_width`, breaking only where the next
    /// word would not fit. The default.
    #[default]
    Reflow,
    /// Preserve acceptable authored breaks and redistribute only the smallest
    /// region needed to satisfy `line_width` and approach the soft equilibrium
    /// target ([`FormatStyle::stable_wrap_target`]). Aimed at keeping revision
    /// diffs small: a small prose edit perturbs the smallest possible region.
    Stable,
    /// Wrap after each sentence (one sentence per line). Line width is ignored — a
    /// long sentence stays on one line.
    Sentence,
    /// Semantic line breaks (<https://sembr.org/>): keep the author's soft line
    /// breaks *and* add a break after each sentence. Like [`WrapMode::Sentence`]
    /// plus preserving authored newlines; clause boundaries survive only where the
    /// author placed a break (no comma/colon detection).
    Semantic,
    /// Leave paragraph line breaks exactly as authored (only collapse trailing
    /// whitespace and blank-line runs, as before reflow existed).
    Preserve,
}

/// How a single-formula *display-math* body (`\[…\]`, `$$…$$`, a non-grid
/// `equation`) is line-broken. Grid environments (`align`, `gather`, matrices)
/// and inline `$…$` are unaffected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "kebab-case")
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum MathWrap {
    /// Derive from the resolved [`WrapMode`]: [`WrapMode::Preserve`] gives
    /// [`MathWrap::Preserve`], every other wrap mode gives [`MathWrap::Break`].
    /// The default.
    #[default]
    Auto,
    /// Keep the author's line breaks inside the body. Content within each
    /// authored line is still normalized (operator spacing, script tightening),
    /// and lines sit at the body indent.
    Preserve,
    /// Never insert breaks: the body stays on one line, overflowing the line
    /// width if too long (matching inline math's behavior).
    SingleLine,
    /// Break a too-long body before its top-level binary/relation operators
    /// (amsmath style).
    Break,
}

impl MathWrap {
    /// Resolve [`MathWrap::Auto`] against the effective wrap mode. Never
    /// returns `Auto`.
    #[must_use]
    pub fn resolve(self, wrap: WrapMode) -> Self {
        match self {
            Self::Auto => {
                if wrap == WrapMode::Preserve {
                    Self::Preserve
                } else {
                    Self::Break
                }
            }
            other => other,
        }
    }
}

/// The byte sequence the formatter's line breaks render as.
///
/// The layout engine always builds output with `\n` (the printer is the sole
/// authority on *where* breaks go); this only selects how those breaks, and the
/// ones carried through from the source, are spelled in the final string. The
/// conversion is document-wide — protected regions included, since a `verbatim`
/// body whose line terminators disagreed with the rest of the file would be a
/// mixed-ending document (see the invariant note in
/// `docs/src/development/architecture.md`, § *The formatter*).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "kebab-case")
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum LineEnding {
    /// Keep what the source used: `\r\n` if the document's first line break is a
    /// CRLF, `\n` otherwise. The default, so formatting never rewrites a file's
    /// line endings behind the author's back.
    #[default]
    Auto,
    /// Always `\n` (Unix).
    Lf,
    /// Always `\r\n` (Windows).
    Crlf,
    /// `\r\n` on Windows, `\n` everywhere else.
    Native,
}

impl LineEnding {
    /// Resolve to a concrete ending. `detected` is what the source used and is
    /// consulted only by [`LineEnding::Auto`]; the result is never `Auto` or
    /// `Native`.
    #[must_use]
    pub fn resolve(self, detected: Self) -> Self {
        match self {
            Self::Auto => match detected {
                Self::Crlf => Self::Crlf,
                _ => Self::Lf,
            },
            Self::Native => {
                if cfg!(windows) {
                    Self::Crlf
                } else {
                    Self::Lf
                }
            }
            other => other,
        }
    }

    /// The bytes this ending renders as. `Auto`/`Native` answer as `Lf`; call
    /// [`LineEnding::resolve`] first.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Crlf => "\r\n",
            _ => "\n",
        }
    }

    /// What `text` uses: [`LineEnding::Crlf`] if its first line break is a CRLF,
    /// [`LineEnding::Lf`] otherwise (including a document with no line break at
    /// all). Never returns `Auto` or `Native`.
    #[must_use]
    pub fn detect(text: &str) -> Self {
        match text.find('\n') {
            Some(idx) if idx > 0 && text.as_bytes()[idx - 1] == b'\r' => Self::Crlf,
            _ => Self::Lf,
        }
    }
}

/// [`LineEnding::detect`] over a tree's text, chunk by chunk, so a whole-document
/// `String` is never materialized just to look at the first line break. `\r\n` is
/// a single `NEWLINE` token, so the pair cannot straddle a chunk boundary — the
/// `prev_cr` carry is belt-and-braces.
pub(crate) fn detect_line_ending(text: &rowan::SyntaxText) -> LineEnding {
    let mut detected = LineEnding::Lf;
    let mut prev_cr = false;
    // `Err` is the early exit: the first line break decides.
    let _: Result<(), ()> = text.try_for_each_chunk(|chunk| {
        if let Some(idx) = chunk.find('\n') {
            let crlf = if idx == 0 {
                prev_cr
            } else {
                chunk.as_bytes()[idx - 1] == b'\r'
            };
            if crlf {
                detected = LineEnding::Crlf;
            }
            return Err(());
        }
        prev_cr = chunk.ends_with('\r');
        Ok(())
    });
    detected
}

/// Rewrite `out`'s line terminators as `resolved` (already through
/// [`LineEnding::resolve`], so `Auto`/`Native` render as LF).
///
/// Only the `\r\n`/`\n` pair is converted; a lone `\r` — which the parser also
/// lexes as a line break, but which can only reach the output through a verbatim
/// region — is left exactly as authored, keeping this transformation the
/// well-understood CRLF/LF one.
pub(crate) fn apply_line_ending(out: &mut String, resolved: LineEnding) {
    let needs_work = match resolved {
        LineEnding::Crlf => out.contains('\n'),
        _ => out.contains("\r\n"),
    };
    if !needs_work {
        return;
    }

    let ending = resolved.as_str();
    let mut result = String::with_capacity(out.len() + out.len() / 16);
    for (i, segment) in out.split('\n').enumerate() {
        if i > 0 {
            result.push_str(ending);
        }
        result.push_str(segment.strip_suffix('\r').unwrap_or(segment));
    }
    *out = result;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(default, rename_all = "kebab-case")
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FormatStyle {
    pub line_width: usize,
    pub indent_width: usize,
    pub wrap: WrapMode,
    pub math_wrap: MathWrap,
    pub line_ending: LineEnding,
}

impl Default for FormatStyle {
    fn default() -> Self {
        Self {
            line_width: 80,
            indent_width: 2,
            wrap: WrapMode::default(),
            math_wrap: MathWrap::default(),
            line_ending: LineEnding::default(),
        }
    }
}

/// Columns below `line_width` that [`WrapMode::Stable`] aims for as its soft
/// equilibrium target. A larger offset widens the acceptable band
/// `[target, line_width]`, so more authored breaks fall inside it and survive
/// untouched — which is the whole point of the mode (minimize revision diffs).
/// Deliberately *not* configurable yet: keeping the config surface minimal (see
/// the maintainer discussion on the PR). Promote this to a `FormatStyle`/config
/// field if a concrete user need for tuning it appears.
pub(crate) const STABLE_WRAP_TARGET_OFFSET: usize = 15;

impl FormatStyle {
    /// Soft equilibrium target for [`WrapMode::Stable`]: [`STABLE_WRAP_TARGET_OFFSET`]
    /// columns below the hard `line_width`, clamped to at least one column. It can
    /// never exceed the hard width, including for styles built directly by API
    /// callers.
    pub fn stable_wrap_target(self) -> usize {
        self.line_width
            .saturating_sub(STABLE_WRAP_TARGET_OFFSET)
            .clamp(1, self.line_width.max(1))
    }
}

#[cfg(all(test, feature = "serde"))]
mod serde_tests {
    use super::{FormatStyle, LineEnding, MathWrap, WrapMode};

    /// The wire spelling of every variant, as a `match` so a new variant fails
    /// to compile here instead of silently shipping an unchecked spelling.
    /// These strings are `badness.toml`'s, and embedders (the dprint plugin)
    /// publish them in their own config schema.
    fn wrap_wire(mode: WrapMode) -> &'static str {
        match mode {
            WrapMode::Reflow => "reflow",
            WrapMode::Stable => "stable",
            WrapMode::Sentence => "sentence",
            WrapMode::Semantic => "semantic",
            WrapMode::Preserve => "preserve",
        }
    }

    fn math_wrap_wire(mode: MathWrap) -> &'static str {
        match mode {
            MathWrap::Auto => "auto",
            MathWrap::Preserve => "preserve",
            MathWrap::SingleLine => "single-line",
            MathWrap::Break => "break",
        }
    }

    fn line_ending_wire(ending: LineEnding) -> &'static str {
        match ending {
            LineEnding::Auto => "auto",
            LineEnding::Lf => "lf",
            LineEnding::Crlf => "crlf",
            LineEnding::Native => "native",
        }
    }

    fn round_trip<T>(value: T, expected: &str)
    where
        T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string(&value).expect("serializes");
        assert_eq!(json, format!("\"{expected}\""));
        let back: T = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, value);
    }

    #[test]
    fn wrap_modes_use_their_toml_spelling() {
        for mode in [
            WrapMode::Reflow,
            WrapMode::Stable,
            WrapMode::Sentence,
            WrapMode::Semantic,
            WrapMode::Preserve,
        ] {
            round_trip(mode, wrap_wire(mode));
        }
    }

    #[test]
    fn math_wraps_use_their_toml_spelling() {
        for mode in [
            MathWrap::Auto,
            MathWrap::Preserve,
            MathWrap::SingleLine,
            MathWrap::Break,
        ] {
            round_trip(mode, math_wrap_wire(mode));
        }
    }

    #[test]
    fn line_endings_use_their_toml_spelling() {
        for ending in [
            LineEnding::Auto,
            LineEnding::Lf,
            LineEnding::Crlf,
            LineEnding::Native,
        ] {
            round_trip(ending, line_ending_wire(ending));
        }
    }

    #[test]
    fn format_style_keys_are_kebab_case_and_default_the_rest() {
        let json = serde_json::to_value(FormatStyle::default()).expect("serializes");
        let object = json.as_object().expect("an object");
        let mut keys: Vec<_> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "indent-width",
                "line-ending",
                "line-width",
                "math-wrap",
                "wrap"
            ]
        );

        let partial: FormatStyle =
            serde_json::from_str(r#"{"line-width": 100}"#).expect("deserializes");
        assert_eq!(
            partial,
            FormatStyle {
                line_width: 100,
                ..FormatStyle::default()
            }
        );
    }
}

#[cfg(all(test, feature = "schema"))]
mod schema_tests {
    use super::{LineEnding, MathWrap, WrapMode};

    /// The schema an embedder publishes has to enumerate the accepted values,
    /// not just say "string".
    fn variants(schema: schemars::Schema) -> Vec<String> {
        let json = serde_json::to_value(schema).expect("schema serializes");
        let mut values: Vec<String> = json
            .get("oneOf")
            .and_then(|one_of| one_of.as_array())
            .expect("an enum schema is a oneOf")
            .iter()
            .map(|variant| {
                variant
                    .get("const")
                    .and_then(|value| value.as_str())
                    .expect("each variant is a const")
                    .to_string()
            })
            .collect();
        values.sort();
        values
    }

    #[test]
    fn enums_advertise_their_wire_values() {
        assert_eq!(
            variants(schemars::schema_for!(WrapMode)),
            ["preserve", "reflow", "semantic", "sentence", "stable"]
        );
        assert_eq!(
            variants(schemars::schema_for!(MathWrap)),
            ["auto", "break", "preserve", "single-line"]
        );
        assert_eq!(
            variants(schemars::schema_for!(LineEnding)),
            ["auto", "crlf", "lf", "native"]
        );
    }
}
