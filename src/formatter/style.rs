//! Formatter configuration.
//!
//! EXTRACTION CANDIDATE: copied ~wholesale from ravel's
//! `src/formatter/style.rs`. Keep close to ravel's version so the eventual
//! shared-crate extraction stays mechanical.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatStyle {
    pub line_width: usize,
    pub indent_width: usize,
}

impl Default for FormatStyle {
    fn default() -> Self {
        Self {
            line_width: 80,
            indent_width: 2,
        }
    }
}
