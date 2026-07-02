//! Per-run formatter context: bundles the active [`FormatStyle`] with the
//! sentence-mode language options and the indent/width helpers the lowering
//! passes lean on.

use super::sentence::SentenceOptions;
use super::style::FormatStyle;

// The identity lowering does not yet consult every helper; they are kept ready
// for real format rules.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct FormatContext<'a> {
    style: FormatStyle,
    /// Language configuration for the [`Sentence`](super::WrapMode::Sentence) /
    /// [`Semantic`](super::WrapMode::Semantic) wrap modes. Borrows the merged
    /// no-break abbreviation slice, so it keeps [`FormatStyle`] `Copy` and out of
    /// the language-config business.
    sentence: SentenceOptions<'a>,
}

#[allow(dead_code)]
impl FormatContext<'static> {
    pub(crate) fn new(style: FormatStyle) -> Self {
        Self {
            style,
            sentence: SentenceOptions::default(),
        }
    }
}

#[allow(dead_code)]
impl<'a> FormatContext<'a> {
    pub(crate) fn with_sentence(style: FormatStyle, sentence: SentenceOptions<'a>) -> Self {
        Self { style, sentence }
    }

    pub(crate) fn style(self) -> FormatStyle {
        self.style
    }

    pub(crate) fn sentence(self) -> SentenceOptions<'a> {
        self.sentence
    }

    pub(crate) fn indent_text(self, indent: usize) -> String {
        " ".repeat(self.style.indent_width * indent)
    }

    pub(crate) fn fits_inline(self, indent: usize, text: &str) -> bool {
        !text.contains('\n') && text.chars().count() <= self.max_inline_width(indent)
    }

    fn max_inline_width(self, indent: usize) -> usize {
        self.style
            .line_width
            .saturating_sub(self.style.indent_width * indent)
    }
}
