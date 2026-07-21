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
    /// region needed to satisfy `line_width` and approach `wrap_target`.
    Minimal,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatStyle {
    pub line_width: usize,
    pub indent_width: usize,
    pub wrap: WrapMode,
    /// Soft line-length target used only by [`WrapMode::Minimal`]. `None` uses
    /// ten columns below `line_width` (clamped to at least one column).
    pub wrap_target: Option<usize>,
}

impl Default for FormatStyle {
    fn default() -> Self {
        Self {
            line_width: 80,
            indent_width: 2,
            wrap: WrapMode::default(),
            wrap_target: None,
        }
    }
}

impl FormatStyle {
    /// Effective soft target for minimal wrapping. It can never exceed the hard
    /// line width, including for styles constructed directly by API callers.
    pub(crate) fn effective_wrap_target(self) -> usize {
        self.wrap_target
            .unwrap_or_else(|| self.line_width.saturating_sub(10).max(1))
            .clamp(1, self.line_width.max(1))
    }
}
