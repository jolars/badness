//! `indented-docstrip-guard`: docstrip guard near matches preceded by horizontal
//! whitespace in a `.dtx` file.
//!
//! The `.dtx` lexer recognizes a real column-zero marker as [`SyntaxKind::GUARD`]
//! and leaves an indented near match as a [`SyntaxKind::COMMENT`]. This rule uses
//! that distinction, plus the preceding CST trivia, instead of rescanning the
//! source. It offers no fix because moving the marker to column zero activates
//! docstrip selection and can change generated files.

use std::path::PathBuf;

use crate::linter::diagnostic::Diagnostic;
use crate::syntax::{SyntaxElement, SyntaxKind};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[Example {
    caption: "A docstrip guard indented by one space:",
    source: " %<*package>\n\\ProvidesPackage{example}\n %</package>\n",
}];

pub struct IndentedDocstripGuard;

impl Rule for IndentedDocstripGuard {
    fn id(&self) -> &'static str {
        "indented-docstrip-guard"
    }

    fn description(&self) -> &'static str {
        "Flag a syntactically complete `%<…>` marker in a `.dtx` file when it is \
         preceded only by horizontal whitespace on its physical line. Docstrip \
         recognizes guards only at column zero, so an indented near match is an \
         ordinary comment and does not select or delimit generated code. No \
         autofix is offered because activating a guard can change generated files."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn example_path(&self) -> &'static str {
        "example.dtx"
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::COMMENT]
    }

    fn check(&self, el: &SyntaxElement, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        if !ctx
            .path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("dtx"))
        {
            return;
        }
        let Some(comment) = el.as_token() else {
            return;
        };
        let Some(marker_len) = comment.text().find('>').map(|end| end + 1) else {
            return;
        };
        if !comment.text().starts_with("%<") {
            return;
        }
        let Some(indent) = comment.prev_token() else {
            return;
        };
        if indent.kind() != SyntaxKind::WHITESPACE
            || !indent.text().chars().all(|ch| matches!(ch, ' ' | '\t'))
            || indent
                .prev_token()
                .is_some_and(|token| token.kind() != SyntaxKind::NEWLINE)
        {
            return;
        }

        let start = usize::from(comment.text_range().start());
        let end = start + marker_len;
        sink.push(Diagnostic {
            rule: self.id(),
            severity: self.default_severity(),
            path: PathBuf::new(),
            start,
            end,
            message: "docstrip guards are recognized only at column zero".to_owned(),
            fix: None,
            related: Vec::new(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_discovery::FileKind;
    use crate::linter::diagnostic::Severity;
    use crate::parser::parse_with_flavor;
    use crate::semantic::SemanticModel;
    use crate::syntax::SyntaxNode;

    fn findings(src: &str, path: &str) -> Vec<Diagnostic> {
        let root = SyntaxNode::new_root(parse_with_flavor(src, FileKind::Dtx.lex_config()).green);
        let model = SemanticModel::build(&root);
        let ctx = RuleContext::new(std::path::Path::new(path), &root, &model, None, None, None);
        let mut out = Vec::new();
        for el in root.descendants_with_tokens() {
            if IndentedDocstripGuard.interests().contains(&el.kind()) {
                IndentedDocstripGuard.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_indented_block_and_inline_guards() {
        let src = "  %<*package>\n\t%</package>\n %<driver>\\input docstrip\n";
        let out = findings(src, "pkg.dtx");
        assert_eq!(out.len(), 3);
        assert_eq!(&src[out[0].start..out[0].end], "%<*package>");
        assert_eq!(&src[out[1].start..out[1].end], "%</package>");
        assert_eq!(&src[out[2].start..out[2].end], "%<driver>");
        assert!(out.iter().all(|finding| {
            finding.rule == "indented-docstrip-guard"
                && finding.severity == Severity::Warning
                && finding.fix.is_none()
        }));
    }

    #[test]
    fn accepts_column_zero_and_rejects_non_guards() {
        let src = "%<*package>\n%</package>\n  %<unterminated\ntext %<inline>\n  % ordinary\n";
        assert!(findings(src, "pkg.dtx").is_empty());
    }

    #[test]
    fn inert_outside_dtx_files() {
        assert!(findings("  %<*package>\n", "pkg.tex").is_empty());
    }
}
