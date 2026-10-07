//! `extra-math-linebreak`: a plain `\\` that adds an empty row at the end of
//! an amsmath display or immediately after intertext. A break before intertext
//! is ordinary usage and is left alone, as are modified breaks and subsidiary
//! environments such as `aligned`, whose final break does not add a row.
//!
//! The fix deletes only the two-byte control symbol, preserving comments and
//! surrounding trivia. It is unsafe because removing the row changes vertical
//! spacing and can change equation numbering. Recognition stays at the math
//! body's statement level and declines locally redefined environments or
//! intertext commands.

use std::path::PathBuf;

use super::{Example, Rule, RuleContext};
use crate::ast::{AstNode, Command, Environment};
use crate::linter::diagnostic::{Diagnostic, Fix};
use crate::syntax::{SyntaxElement, SyntaxKind, is_trivia};

pub struct ExtraMathLinebreak;

const EXAMPLES: &[Example] = &[
    Example {
        caption: "A final linebreak adds an empty equation row:",
        source: "\\begin{align}\n  a &= b \\\\\n\\end{align}\n",
    },
    Example {
        caption: "Intertext already separates the surrounding equation rows:",
        source: "\\begin{align*}\n  a &= b \\\\\n  \\intertext{and therefore}\\\\\n  c &= d\n\\end{align*}\n",
    },
];

impl Rule for ExtraMathLinebreak {
    fn id(&self) -> &'static str {
        "extra-math-linebreak"
    }

    fn emits_fix(&self) -> bool {
        true
    }

    fn description(&self) -> &'static str {
        "Flag a plain `\\\\` at the end of an `align`, `alignat`, `flalign`, \
         `gather`, or `multline` environment, including their starred forms, or \
         immediately after `\\intertext{...}` or `\\shortintertext{...}` in an \
         environment that supports intertext. These breaks add an empty row, \
         increasing vertical space and potentially adding an equation number. \
         Breaks before intertext, starred breaks, explicit spacing arguments, \
         subsidiary environments such as `aligned`, and locally redefined \
         environments or intertext commands are left alone. The fix deletes \
         only the offending `\\\\`, preserving comments and surrounding whitespace. \
         It is **unsafe** because it changes typeset spacing and potentially \
         numbering; use `--fix --unsafe-fixes` or an explicit editor action."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::LINE_BREAK]
    }

    fn check(&self, el: &SyntaxElement, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(linebreak) = el.as_node() else {
            return;
        };
        let Some(token) = linebreak.first_token() else {
            return;
        };
        if token.kind() != SyntaxKind::CONTROL_SYMBOL
            || token.text() != "\\\\"
            || token.text_range() != linebreak.text_range()
        {
            return;
        }

        let next = linebreak
            .siblings_with_tokens(rowan::Direction::Next)
            .skip(1)
            .find(|e| !is_trivia(e.kind()));
        // The lexer can keep a modifier star and following letters in one WORD.
        if next
            .as_ref()
            .and_then(|e| e.as_token())
            .is_some_and(|t| t.kind() == SyntaxKind::WORD && t.text().starts_with('*'))
        {
            return;
        }

        let Some(math) = linebreak.parent().filter(|n| n.kind() == SyntaxKind::MATH) else {
            return;
        };
        let Some(env) = math.parent().and_then(Environment::cast) else {
            return;
        };
        let Some(name) = env.name() else { return };
        let family = name.strip_suffix('*').unwrap_or(&name);
        if !matches!(
            family,
            "align" | "alignat" | "flalign" | "gather" | "multline"
        ) || env.end().and_then(|end| end.name()).as_deref() != Some(name.as_str())
            || ctx.user_definitions().environment(&name).is_some()
        {
            return;
        }

        let previous = linebreak
            .siblings_with_tokens(rowan::Direction::Prev)
            .skip(1)
            .find(|e| !is_trivia(e.kind()));
        let intertext = previous.and_then(|e| intertext_name(e, ctx));
        let message = if family != "multline"
            && let Some(command) = intertext
        {
            format!("linebreak after `\\{command}` adds an empty math row")
        } else if next.is_none()
            && math
                .siblings_with_tokens(rowan::Direction::Next)
                .skip(1)
                .find(|e| !is_trivia(e.kind()))
                .is_some_and(|e| e.kind() == SyntaxKind::END)
        {
            "final linebreak adds an empty math row".to_owned()
        } else {
            return;
        };

        let range = token.text_range();
        let start = usize::from(range.start());
        let end = usize::from(range.end());
        sink.push(Diagnostic {
            rule: self.id(),
            severity: self.default_severity(),
            path: PathBuf::new(),
            start,
            end,
            message,
            fix: Some(Fix::unsafe_(start, end, "", "Remove extra math linebreak")),
            related: Vec::new(),
        });
    }
}

fn intertext_name(element: SyntaxElement, ctx: &RuleContext<'_>) -> Option<String> {
    let command = element.into_node().and_then(Command::cast)?;
    let name = command.name()?;
    if !matches!(name.as_str(), "intertext" | "shortintertext")
        || ctx.user_definitions().command(&name).is_some()
    {
        return None;
    }

    // Greedy attachment can include formula groups after the text argument.
    // Such content starts a math row, so its following break is necessary.
    let mut content = command
        .syntax()
        .children_with_tokens()
        .filter(|e| !is_trivia(e.kind()));
    if content.next()?.kind() != SyntaxKind::CONTROL_WORD {
        return None;
    }
    let group = content.next()?.into_node()?;
    if group.kind() != SyntaxKind::GROUP
        || group.first_token()?.kind() != SyntaxKind::L_BRACE
        || group.last_token()?.kind() != SyntaxKind::R_BRACE
        || content.next().is_some()
    {
        return None;
    }
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linter::diagnostic::{Applicability, Severity};
    use crate::linter::fix::apply_fixes;
    use crate::parser::{parse, reconstruct};
    use crate::semantic::SemanticModel;
    use crate::syntax::SyntaxNode;

    fn findings(src: &str) -> Vec<Diagnostic> {
        let root = SyntaxNode::new_root(parse(src).green);
        let model = SemanticModel::build(&root);
        let ctx = RuleContext::new(
            std::path::Path::new("x.tex"),
            &root,
            &model,
            None,
            None,
            None,
        );
        let mut out = Vec::new();
        for el in root.descendants_with_tokens() {
            if ExtraMathLinebreak.interests().contains(&el.kind()) {
                ExtraMathLinebreak.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_final_breaks_in_display_environment_families() {
        for family in ["align", "alignat", "flalign", "gather", "multline"] {
            for star in ["", "*"] {
                let name = format!("{family}{star}");
                let args = if family == "alignat" { "{1}" } else { "" };
                let row = if matches!(family, "gather" | "multline") {
                    "a=b"
                } else {
                    "a&=b"
                };
                let src = format!("\\begin{{{name}}}{args}{row}\\\\\\end{{{name}}}\n");
                let out = findings(&src);
                assert_eq!(out.len(), 1, "{src}");
                assert_eq!(out[0].rule, "extra-math-linebreak");
                assert_eq!(out[0].severity, Severity::Warning);
                assert_eq!(&src[out[0].start..out[0].end], "\\\\");
            }
        }
    }

    #[test]
    fn flags_breaks_after_intertext_with_or_without_a_preceding_break() {
        for family in ["align", "alignat", "flalign", "gather"] {
            for star in ["", "*"] {
                for command in ["intertext", "shortintertext"] {
                    for before in ["", "\\\\"] {
                        let name = format!("{family}{star}");
                        let args = if family == "alignat" { "{1}" } else { "" };
                        let src = format!(
                            "\\begin{{{name}}}{args}a=b{before}\n\\{command}{{hello}} % text\n\\\\ % break\nc=d\\end{{{name}}}\n"
                        );
                        let out = findings(&src);
                        assert_eq!(out.len(), 1, "{src}");
                        assert_eq!(
                            out[0].message,
                            format!("linebreak after `\\{command}` adds an empty math row"),
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn removal_is_tight_unsafe_and_lossless() {
        for src in [
            "\\begin{align}a&=b\\\\ % tail\n\\end{align}\n",
            "\\begin{align}a&=b\\intertext{hello}\\\\c&=d\\end{align}\n",
        ] {
            let out = findings(src);
            assert_eq!(out.len(), 1);
            let finding = &out[0];
            let fix = finding.fix.as_ref().unwrap();
            assert_eq!(fix.applicability, Applicability::Unsafe);
            assert_eq!(fix.edits.len(), 1);
            assert_eq!(
                (fix.edits[0].start, fix.edits[0].end),
                (finding.start, finding.end)
            );
            assert_eq!(&src[finding.start..finding.end], "\\\\");
            assert_eq!(
                apply_fixes(src, std::slice::from_ref(fix), false).output,
                src
            );
            let fixed = apply_fixes(src, std::slice::from_ref(fix), true).output;
            assert_eq!(
                fixed,
                format!("{}{}", &src[..finding.start], &src[finding.end..])
            );
            assert!(parse(&fixed).errors.is_empty());
            assert_eq!(reconstruct(&fixed), fixed);
            assert!(findings(&fixed).is_empty());
        }
    }

    #[test]
    fn leaves_regular_rows_and_breaks_before_intertext_alone() {
        for src in [
            "\\begin{align}a&=b\\\\c&=d\\end{align}",
            "\\begin{align}a&=b\\\\\\intertext{hello}c&=d\\end{align}",
            "\\begin{align}a&=b\\\\\\shortintertext{hello}c&=d\\end{align}",
            "\\begin{align}a&=b\\intertext{hello}{c}\\\\d&=e\\end{align}",
            "\\begin{align}a&=b\\intertext{hello}[c]\\\\d&=e\\end{align}",
            "\\begin{align}a&=b\\intertext\\\\c&=d\\end{align}",
            "\\begin{multline}a=b\\intertext{hello}\\\\c=d\\end{multline}",
        ] {
            assert!(findings(src).is_empty(), "{src}");
        }
    }

    #[test]
    fn leaves_starred_and_explicit_spacing_breaks_alone() {
        for modifier in ["*", "[2ex]", "*[2ex]"] {
            for src in [
                format!("\\begin{{align}}a&=b\\\\{modifier}\\end{{align}}"),
                format!("\\begin{{align}}a&=b\\intertext{{hello}}\\\\{modifier}c&=d\\end{{align}}"),
            ] {
                assert!(findings(&src).is_empty(), "{src}");
            }
        }
    }

    #[test]
    fn leaves_nested_content_and_other_environments_alone() {
        for name in [
            "aligned",
            "alignedat",
            "gathered",
            "split",
            "matrix",
            "array",
            "tabular",
            "equation",
            "custom",
        ] {
            let src = format!("\\begin{{{name}}}a&=b\\\\\\end{{{name}}}");
            assert!(findings(&src).is_empty(), "{src}");
        }
        for src in [
            "\\begin{align}{a\\\\}\\end{align}",
            "\\begin{align}\\text{a\\\\}\\end{align}",
            "\\begin{align}\\begin{aligned}a&=b\\\\\\end{aligned}\\end{align}",
            "\\begin{align}a&=b\\\\",
            "\\begin{align}a&=b\\\\\\end{gather}",
            "$a\\\\$",
            "\\[a\\\\\\]",
            "text\\\\",
            "\\begin{verbatim}\\begin{align}a&=b\\\\\\end{align}\\end{verbatim}",
        ] {
            assert!(findings(src).is_empty(), "{src}");
        }
    }

    #[test]
    fn leaves_local_redefinitions_alone() {
        for src in [
            "\\renewenvironment{align}{}{}\n\\begin{align}a&=b\\\\\\end{align}",
            "\\renewenvironment{align*}{}{}\n\\begin{align*}a&=b\\\\\\end{align*}",
            "\\renewcommand{\\intertext}[1]{#1}\n\\begin{align}a&=b\\intertext{hello}\\\\c&=d\\end{align}",
            "\\renewcommand{\\shortintertext}[1]{#1}\n\\begin{align}a&=b\\shortintertext{hello}\\\\c&=d\\end{align}",
        ] {
            assert!(findings(src).is_empty(), "{src}");
        }
    }
}
