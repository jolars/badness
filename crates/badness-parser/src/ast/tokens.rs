//! Typed [`AstToken`] wrappers over CST *tokens*. Only [`ControlWord`] exists
//! today — the rest of the lexer's tokens (`L_BRACE`, `WORD`, trivia, …) are
//! matched raw by the formatter's token loops, which is idiomatic and should stay
//! that way. Add a wrapper here only when a token grows a named accessor consumer.

use rowan::TextRange;

use super::AstToken;
use crate::syntax::{SyntaxKind, SyntaxToken};

/// The `CONTROL_WORD` token leading a `COMMAND` node — `\foo` (backslash + ASCII
/// letters). Carries the command's name and its precise range.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ControlWord {
    syntax: SyntaxToken,
}

impl AstToken for ControlWord {
    fn can_cast(kind: SyntaxKind) -> bool {
        kind == SyntaxKind::CONTROL_WORD
    }

    fn cast(syntax: SyntaxToken) -> Option<Self> {
        Self::can_cast(syntax.kind()).then_some(Self { syntax })
    }

    fn syntax(&self) -> &SyntaxToken {
        &self.syntax
    }
}

impl ControlWord {
    /// The control-word name with the leading `\` stripped (`section` for
    /// `\section`).
    pub fn name(&self) -> String {
        self.syntax.text().trim_start_matches('\\').to_string()
    }

    /// The byte range of the `\foo` token, backslash included.
    pub fn range(&self) -> TextRange {
        self.syntax.text_range()
    }
}
