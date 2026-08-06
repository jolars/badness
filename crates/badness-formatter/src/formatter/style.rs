//! Formatter configuration.
//!
//! The LaTeX-specific [`WrapMode`] (paragraph line-break policy, modeled on the
//! `panache` formatter) is the one field specific to badness.

/// How the formatter lays out the line breaks *inside* a paragraph. Modeled on
/// panache's `WrapMode` (`crates/panache-formatter/src/config.rs`).
///
/// The sentence-boundary detection behind [`WrapMode::Sentence`] and
/// [`WrapMode::Semantic`] is a per-language abbreviation profile
/// (`formatter::sentence`); the language and any user no-break abbreviations are
/// resolved from config into the [`SentenceOptions`](super::SentenceOptions)
/// threaded through the lowering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatStyle {
    pub line_width: usize,
    pub indent_width: usize,
    pub wrap: WrapMode,
    pub math_wrap: MathWrap,
}

impl Default for FormatStyle {
    fn default() -> Self {
        Self {
            line_width: 80,
            indent_width: 2,
            wrap: WrapMode::default(),
            math_wrap: MathWrap::default(),
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
