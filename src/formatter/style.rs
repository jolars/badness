//! Formatter configuration.
//!
//! The LaTeX-specific [`WrapMode`] (paragraph line-break policy, modeled on the
//! `panache` formatter) is the one field specific to badness.

/// How the formatter lays out the line breaks *inside* a paragraph. Modeled on
/// panache's `WrapMode` (`crates/panache-formatter/src/config.rs`).
///
/// Only [`WrapMode::Reflow`] and [`WrapMode::Preserve`] are implemented today;
/// [`WrapMode::Sentence`] and [`WrapMode::Semantic`] are accepted but currently
/// fall back to [`WrapMode::Preserve`] behavior (see `formatter::core`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WrapMode {
    /// Greedy fill: pack words up to `line_width`, breaking only where the next
    /// word would not fit. The default.
    #[default]
    Reflow,
    /// Wrap after each sentence (one sentence per line). *Not yet implemented —
    /// falls back to [`WrapMode::Preserve`].*
    Sentence,
    /// Semantic line breaks (<https://sembr.org/>): keep authored breaks and add
    /// breaks at sentence boundaries. *Not yet implemented — falls back to
    /// [`WrapMode::Preserve`].*
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
}

impl Default for FormatStyle {
    fn default() -> Self {
        Self {
            line_width: 80,
            indent_width: 2,
            wrap: WrapMode::default(),
        }
    }
}
