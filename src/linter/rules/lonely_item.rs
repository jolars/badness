//! `lonely-item`: an `\item` written directly in the document body.
//!
//! LaTeX raises "Lonely \item" when an item has no active list. The rule only
//! checks the document body's direct content, including low-level `\list` and
//! `\trivlist` pairs. An item in a custom environment, macro body, or included
//! fragment may acquire its list context outside the visible syntax. No
//! automatic edit can determine which list the author intended.

use std::path::PathBuf;

use crate::ast::{AstNode, Environment, command_name, control_word_range};
use crate::linter::diagnostic::{Diagnostic, Severity};
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};

use super::{Example, Rule, RuleContext, StreamVisitor};

const EXAMPLES: &[Example] = &[Example {
    caption: "An item directly in the document body has no list:",
    source: "\\begin{document}\n\\item A\n\\end{document}\n",
}];

pub struct LonelyItem;

impl Rule for LonelyItem {
    fn id(&self) -> &'static str {
        "lonely-item"
    }

    fn default_severity(&self) -> Severity {
        Severity::Error
    }

    fn description(&self) -> &'static str {
        "Flag `\\item` written directly in the `document` environment, where \
         LaTeX reports a lonely item because there is no list. The rule leaves \
         items inside other environments, command arguments, low-level \
         `\\list`/`\\trivlist` pairs, and standalone fragments alone because \
         their list context may come from a custom definition or an including \
         file. Report-only: the intended list type and boundaries cannot be \
         inferred from the item."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn stream(&self) -> Option<Box<dyn StreamVisitor>> {
        Some(Box::new(LonelyItemVisitor { raw_list_depth: 0 }))
    }
}

struct LonelyItemVisitor {
    raw_list_depth: usize,
}

impl StreamVisitor for LonelyItemVisitor {
    fn visit(&mut self, el: &SyntaxElement, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(command) = el.as_node() else {
            return;
        };
        if command.kind() != SyntaxKind::COMMAND || !directly_in_document(command) {
            return;
        }
        match command_name(command).as_deref() {
            Some("list" | "trivlist") => {
                self.raw_list_depth += 1;
                return;
            }
            Some("endlist" | "endtrivlist") => {
                self.raw_list_depth = self.raw_list_depth.saturating_sub(1);
                return;
            }
            Some("item") if self.raw_list_depth == 0 => {}
            _ => return,
        }
        if ctx.user_definitions().command("item").is_some() {
            return;
        }
        let Some(range) = control_word_range(command) else {
            return;
        };
        sink.push(Diagnostic {
            rule: "lonely-item",
            severity: Severity::Error,
            path: PathBuf::new(),
            start: usize::from(range.start()),
            end: usize::from(range.end()),
            message: "`\\item` has no enclosing list environment".to_owned(),
            fix: None,
            related: Vec::new(),
        });
    }
}

fn directly_in_document(command: &SyntaxNode) -> bool {
    let mut saw_document = false;
    for ancestor in command.ancestors().skip(1) {
        match ancestor.kind() {
            SyntaxKind::ENVIRONMENT => {
                if saw_document
                    || Environment::cast(ancestor)
                        .and_then(|env| env.name())
                        .as_deref()
                        != Some("document")
                {
                    return false;
                }
                saw_document = true;
            }
            SyntaxKind::GROUP | SyntaxKind::OPTIONAL | SyntaxKind::CONDITIONAL => return false,
            _ => {}
        }
    }
    saw_document
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use crate::semantic::SemanticModel;

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
        let mut visitor = LonelyItem.stream().unwrap();
        for el in root.descendants_with_tokens() {
            visitor.visit(&el, &ctx, &mut out);
        }
        out
    }

    #[test]
    fn flags_direct_document_item_with_a_tight_span() {
        let src = "\\documentclass{article}\n\\begin{document}\n\\item A\n\\end{document}\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].severity, Severity::Error);
        assert_eq!(&src[out[0].start..out[0].end], "\\item");
        assert!(out[0].fix.is_none());
    }

    #[test]
    fn leaves_items_in_lists_alone() {
        for env in ["itemize", "enumerate", "description"] {
            let src = format!(
                "\\begin{{document}}\n\\begin{{{env}}}\n\\item A\n\\end{{{env}}}\n\\end{{document}}\n"
            );
            assert!(findings(&src).is_empty(), "{env}");
        }
    }

    #[test]
    fn leaves_items_in_raw_lists_alone() {
        for (open, close) in [("\\list{}{}", "\\endlist"), ("\\trivlist", "\\endtrivlist")] {
            let src = format!(
                "\\begin{{document}}\n{open}\n\\item A\n{close}\n\\item B\n\\end{{document}}\n"
            );
            let out = findings(&src);
            assert_eq!(out.len(), 1, "{src}");
            assert_eq!(&src[out[0].start..out[0].end], "\\item");
            assert!(out[0].start > src.find(close).unwrap(), "{src}");
        }
    }

    #[test]
    fn leaves_uncertain_contexts_alone() {
        for src in [
            "\\item A\n",
            "\\begin{document}\\begin{custom}\\item A\\end{custom}\\end{document}",
            "\\begin{document}\\newcommand{\\foo}{\\item A}\\end{document}",
            "\\begin{document}{\\item A}\\end{document}",
            "\\begin{document}\\iftrue\\item A\\fi\\end{document}",
        ] {
            assert!(findings(src).is_empty(), "{src}");
        }
    }

    #[test]
    fn leaves_redefined_item_alone() {
        let src = "\\renewcommand{\\item}{A}\n\\begin{document}\\item\\end{document}";
        assert!(findings(src).is_empty());
    }
}
