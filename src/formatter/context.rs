//! Per-run formatter context: bundles the active [`FormatStyle`] with the
//! indent/width helpers the lowering passes lean on.

use super::style::FormatStyle;

// The identity lowering does not yet consult every helper; they are kept ready
// for real format rules.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct FormatContext {
    style: FormatStyle,
}

#[allow(dead_code)]
impl FormatContext {
    pub(crate) fn new(style: FormatStyle) -> Self {
        Self { style }
    }

    pub(crate) fn style(self) -> FormatStyle {
        self.style
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
