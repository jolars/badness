//! `invalid-macrocode-frame`: a `.dtx` `macrocode` closing frame whose margin
//! spacing does not match the literal delimiter required by the `doc` package.
//!
//! The parser deliberately treats the margin as trivia and pairs a near match as
//! an ordinary [`SyntaxKind::ENVIRONMENT`], but `doc` scans for the physical line
//! `%    \end{macrocode}` (or the starred form). This rule therefore reads the
//! already-parsed environment and its immediately preceding `DOC_MARGIN` and
//! horizontal-space tokens; it never rescans source text. The replacement is
//! [`crate::linter::diagnostic::Applicability::Safe`] because it changes only the
//! malformed delimiter spelling to the literal spelling `doc` requires.

use std::path::PathBuf;

use crate::ast::{AstNode, Environment};
use crate::linter::diagnostic::{Diagnostic, Fix, Severity};
use crate::syntax::{SyntaxElement, SyntaxKind};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[Example {
    caption: "A `macrocode` closer with only three spaces after `%`:",
    source: "%    \\begin{macrocode}\n\\def\\example{value}\n%   \\end{macrocode}\n",
}];

pub struct InvalidMacrocodeFrame;

impl Rule for InvalidMacrocodeFrame {
    fn id(&self) -> &'static str {
        "invalid-macrocode-frame"
    }

    fn default_severity(&self) -> Severity {
        Severity::Error
    }

    fn description(&self) -> &'static str {
        "Flag a `.dtx` `macrocode` or `macrocode*` closing frame unless exactly \
         four spaces separate its column-one `%` from `\\end{…}`. The `doc` \
         package scans for that literal physical delimiter, so a near match does \
         not close the code chunk even though it looks like an ordinary \
         environment to Badness. The safe autofix replaces only the malformed \
         horizontal space with the required four spaces."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn example_path(&self) -> &'static str {
        "example.dtx"
    }

    fn emits_fix(&self) -> bool {
        true
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::ENVIRONMENT]
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
        let Some(env) = el
            .as_node()
            .and_then(|node| Environment::cast(node.clone()))
        else {
            return;
        };
        if !matches!(env.name().as_deref(), Some("macrocode" | "macrocode*")) {
            return;
        }
        let Some((start, end)) = malformed_closing_space(&env) else {
            return;
        };
        sink.push(Diagnostic {
            rule: self.id(),
            severity: self.default_severity(),
            path: PathBuf::new(),
            start,
            end,
            message: "`macrocode` closing frame requires exactly four spaces after `%`".to_owned(),
            fix: Some(Fix::safe(
                start,
                end,
                "    ",
                "Use four spaces in the `macrocode` closing frame",
            )),
            related: Vec::new(),
        });
    }
}

/// Return the malformed horizontal-space span before a `macrocode` closer.
///
/// `DOC_MARGIN` is the lexer's structural proof that `%` occupies column one in
/// a `.dtx` documentation line. Requiring it immediately before the optional
/// `WHITESPACE` and `END` nodes restricts the fix to the one malformed component
/// of an otherwise exact closing frame. A missing span is represented by the
/// zero-width insertion point after `%`.
fn malformed_closing_space(env: &Environment) -> Option<(usize, usize)> {
    let end_node = env.end()?;
    let before_end = end_node.syntax().prev_sibling_or_token()?;
    let (margin, range, valid) = if before_end.kind() == SyntaxKind::WHITESPACE {
        let token = before_end.as_token()?;
        let range = token.text_range();
        (
            before_end.prev_sibling_or_token()?,
            (usize::from(range.start()), usize::from(range.end())),
            token.text() == "    ",
        )
    } else {
        let offset = usize::from(end_node.syntax().text_range().start());
        (before_end, (offset, offset), false)
    };
    if margin.kind() != SyntaxKind::DOC_MARGIN || valid {
        return None;
    }
    Some(range)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_discovery::FileKind;
    use crate::linter::diagnostic::Applicability;
    use crate::linter::fix::apply_fixes;
    use crate::parser::parse_with_flavor;
    use crate::semantic::SemanticModel;
    use crate::syntax::SyntaxNode;

    fn findings(src: &str, path: &str) -> Vec<Diagnostic> {
        let root = SyntaxNode::new_root(parse_with_flavor(src, FileKind::Dtx.lex_config()).green);
        let model = SemanticModel::build(&root);
        let ctx = RuleContext::new(std::path::Path::new(path), &root, &model, None, None, None);
        let mut out = Vec::new();
        for el in root.descendants_with_tokens() {
            if InvalidMacrocodeFrame.interests().contains(&el.kind()) {
                InvalidMacrocodeFrame.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_malformed_closing_space() {
        let src = "%    \\begin{macrocode}\n\\def\\foo{bar}\n%   \\end{macrocode}\n";
        let out = findings(src, "pkg.dtx");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "invalid-macrocode-frame");
        assert_eq!(out[0].severity, Severity::Error);
        assert_eq!((out[0].start, out[0].end), (38, 41));
    }

    #[test]
    fn fixes_only_the_horizontal_space() {
        let src = "%    \\begin{macrocode*}\n\\def\\foo{bar}\n%     \\end{macrocode*}\n";
        let out = findings(src, "pkg.dtx");
        let fix = out[0].fix.as_ref().expect("a spacing fix");
        assert_eq!(fix.applicability, Applicability::Safe);
        assert_eq!(fix.edits.len(), 1);
        assert_eq!(fix.edits[0].content, "    ");
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), false).output,
            "%    \\begin{macrocode*}\n\\def\\foo{bar}\n%    \\end{macrocode*}\n"
        );
    }

    #[test]
    fn accepts_exact_closing_frame() {
        assert!(
            findings(
                "%    \\begin{macrocode}\n\\def\\foo{bar}\n%    \\end{macrocode}\n",
                "pkg.dtx"
            )
            .is_empty()
        );
    }

    #[test]
    fn fixes_missing_space_and_tabs() {
        for malformed in ["", "\t", "  \t "] {
            let src = format!(
                "%    \\begin{{macrocode}}\n\\def\\foo{{bar}}\n%{malformed}\\end{{macrocode}}\n"
            );
            let out = findings(&src, "pkg.dtx");
            assert_eq!(out.len(), 1, "malformed spacing {malformed:?}");
            let fix = out[0].fix.as_ref().expect("a spacing fix");
            assert_eq!(
                apply_fixes(&src, std::slice::from_ref(fix), false).output,
                "%    \\begin{macrocode}\n\\def\\foo{bar}\n%    \\end{macrocode}\n"
            );
        }
    }

    #[test]
    fn does_not_police_opener_spacing() {
        assert!(
            findings(
                "%   \\begin{macrocode}\n\\def\\foo{bar}\n%    \\end{macrocode}\n",
                "pkg.dtx"
            )
            .is_empty()
        );
    }

    #[test]
    fn inert_outside_dtx_files() {
        assert!(
            findings(
                "%    \\begin{macrocode}\n\\def\\foo{bar}\n%   \\end{macrocode}\n",
                "pkg.tex"
            )
            .is_empty()
        );
    }

    #[test]
    fn uppercase_dtx_extension_is_supported() {
        assert_eq!(
            findings(
                "%    \\begin{macrocode}\n\\def\\foo{bar}\n%   \\end{macrocode}\n",
                "PKG.DTX"
            )
            .len(),
            1
        );
    }
}
