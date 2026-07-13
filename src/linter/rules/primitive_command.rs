//! `primitive-command`: raw plain-TeX primitives discouraged in LaTeX documents,
//! reported with the LaTeX construct that supersedes them (ChkTeX 41, lacheck,
//! l2tabu). A sibling of [`deprecated_command`](super::deprecated_command): that
//! rule handles the obsolete two-letter font *switches*, this one the underlying
//! TeX *primitives* people should not reach for in LaTeX source.
//!
//! Most entries are **report-only**: their LaTeX replacement *restructures
//! arguments* (`a \over b` becomes `\frac{a}{b}`, `\centerline{x}` becomes a
//! `\centering` declaration or a `center` environment), so no single contiguous
//! textual edit can rewrite them correctly by construction — a fix would have to
//! move the surrounding operands, which is layout, not our job (tenet 1). We
//! still report the finding.
//!
//! A few entries carry a **`Safe` fix**: a 1:1 control-word swap for a primitive
//! whose LaTeX form is a single, meaning-identical token (`\sb`/`\sp`, the plain
//! TeX subscript/superscript aliases, become `_`/`^`). The swap replaces exactly
//! the `CONTROL_WORD` token, leaving operands untouched, so it is correct by
//! construction and stays lossless.
//!
//! The table lives here, not in `data/signatures.json`: "this primitive is
//! discouraged in LaTeX" is a lint judgment, not the structural arity/verbatim
//! fact the signature DB carries (AGENTS.md core decision #2).

use std::path::PathBuf;

use crate::ast::{command_name, control_word_range};
use crate::linter::diagnostic::{Diagnostic, Fix, Severity};
use crate::syntax::{SyntaxElement, SyntaxKind};

use super::{Example, Rule, RuleContext};

/// A discouraged primitive: its control word (backslash stripped), the LaTeX
/// construct named in the message, and — where a 1:1 meaning-identical
/// replacement exists — the swap text for a `Safe` control-word fix.
struct Primitive {
    /// Control word without the leading backslash (e.g. `"over"`).
    name: &'static str,
    /// LaTeX replacement named in the diagnostic message (markdown, already
    /// wrapped in backticks where it is a code span).
    suggest: &'static str,
    /// The replacement text for a `Safe` control-word swap (e.g. `"_"`), or
    /// `None` for report-only primitives whose LaTeX equivalent restructures
    /// arguments and so cannot be fixed by a single textual edit.
    swap: Option<&'static str>,
}

/// The discouraged-primitive table. Report-only rows leave `swap: None`; the
/// argument-restructuring replacements (`\over`, `\centerline`, …) can only be
/// suggested, not mechanically applied (tenet 1).
const PRIMITIVES: &[Primitive] = &[
    Primitive {
        name: "over",
        suggest: "`\\frac{...}{...}`",
        swap: None,
    },
    Primitive {
        name: "atop",
        suggest: "`\\genfrac` or `\\substack`",
        swap: None,
    },
    Primitive {
        name: "choose",
        suggest: "`\\binom{...}{...}`",
        swap: None,
    },
    Primitive {
        name: "above",
        suggest: "`\\genfrac`",
        swap: None,
    },
    Primitive {
        name: "centerline",
        suggest: "`\\centering` or the `center` environment",
        swap: None,
    },
    Primitive {
        name: "eqno",
        suggest: "`amsmath` equation numbering",
        swap: None,
    },
    Primitive {
        name: "leqno",
        suggest: "`amsmath`'s `leqno` option or `\\tag`",
        swap: None,
    },
    Primitive {
        name: "bgroup",
        suggest: "`{`",
        swap: None,
    },
    Primitive {
        name: "egroup",
        suggest: "`}`",
        swap: None,
    },
    Primitive {
        name: "sb",
        suggest: "`_`",
        swap: Some("_"),
    },
    Primitive {
        name: "sp",
        suggest: "`^`",
        swap: Some("^"),
    },
];

const EXAMPLES: &[Example] = &[
    Example {
        caption: "A plain-TeX fraction primitive (report-only; the LaTeX form restructures its operands):",
        source: "$a \\over b$\n",
    },
    Example {
        caption: "The plain-TeX subscript alias, carrying a safe swap to `_`:",
        source: "$x\\sb2$\n",
    },
];

pub struct PrimitiveCommand;

impl Rule for PrimitiveCommand {
    fn id(&self) -> &'static str {
        "primitive-command"
    }

    fn emits_fix(&self) -> bool {
        true
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag raw plain-TeX primitives discouraged in LaTeX source, naming the \
         LaTeX construct that supersedes each one (ChkTeX 41, lacheck, l2tabu). A \
         sibling of `deprecated-command`, which covers the obsolete font \
         switches. Most primitives are reported only: their LaTeX replacement \
         restructures arguments (`a \\over b` becomes `\\frac{a}{b}`, \
         `\\centerline{x}` becomes a `\\centering` declaration or a `center` \
         environment), so no single textual edit can rewrite them correctly by \
         construction. A few carry a `Safe` autofix — a 1:1 control-word swap for \
         a primitive whose LaTeX form is a single meaning-identical token \
         (`\\sb`/`\\sp` become `_`/`^`); the swap replaces just the control word, \
         so it stays lossless and meaning-preserving."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::COMMAND]
    }

    fn check(&self, el: &SyntaxElement, _ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(command) = el.as_node() else {
            return;
        };
        let Some(name) = command_name(command) else {
            return;
        };
        let Some(primitive) = PRIMITIVES.iter().find(|p| p.name == name) else {
            return;
        };
        // Underline just the control word, not any greedily-attached group, so
        // the caret sits tightly on the primitive.
        let control_word = control_word_range(command);
        let range = control_word.unwrap_or_else(|| command.text_range());
        // A Safe fix only when the table gives a 1:1 swap *and* we can isolate
        // the control word: the span covers exactly the `CONTROL_WORD` token, the
        // replacement is a single meaning-identical token, so it is correct by
        // construction (tenet 1). Report-only primitives (argument-restructuring
        // replacements) carry no fix.
        let fix = primitive.swap.zip(control_word).map(|(swap, r)| {
            Fix::safe(
                usize::from(r.start()),
                usize::from(r.end()),
                swap.to_string(),
                format!("Replace `\\{name}` with `{swap}`"),
            )
        });
        sink.push(Diagnostic {
            rule: self.id(),
            severity: self.default_severity(),
            path: PathBuf::new(),
            start: usize::from(range.start()),
            end: usize::from(range.end()),
            message: format!(
                "`\\{name}` is a raw TeX primitive; use {}",
                primitive.suggest
            ),
            fix,
            related: Vec::new(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
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
            if PrimitiveCommand.interests().contains(&el.kind()) {
                PrimitiveCommand.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_report_only_primitive() {
        let out = findings("$a \\over b$\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "primitive-command");
        assert!(out[0].message.contains("\\frac"), "got: {}", out[0].message);
        // Report-only: the LaTeX form restructures operands.
        assert!(out[0].fix.is_none());
        // Caret covers just `\over` (bytes 3..8), not the operands.
        assert_eq!((out[0].start, out[0].end), (3, 8));
    }

    #[test]
    fn latex_constructs_are_fine() {
        assert!(findings("\\frac{a}{b}\\binom{n}{k}\n").is_empty());
    }

    #[test]
    fn flags_each_occurrence() {
        assert_eq!(findings("$a\\over b$ $c\\atop d$\n").len(), 2);
    }

    #[test]
    fn carries_safe_swap_fix() {
        use crate::linter::diagnostic::Applicability;
        use crate::linter::fix::apply_fixes;

        let src = "$x\\sb2$\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        let fix = out[0].fix.as_ref().expect("should carry a fix");
        assert_eq!(fix.applicability, Applicability::Safe);
        // The fix spans just the `\sb` control word (bytes 2..5), swapping it for
        // the LaTeX subscript token while leaving the operand untouched.
        assert_eq!((fix.start, fix.end), (2, 5));
        assert_eq!(fix.content, "_");
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), false).output,
            "$x_2$\n"
        );
    }

    #[test]
    fn superscript_alias_swaps_to_caret() {
        let out = findings("$x\\sp2$\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].fix.as_ref().unwrap().content, "^");
    }
}
