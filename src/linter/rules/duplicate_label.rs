//! `duplicate-label`: a `\label{key}` defined more than once in a file.
//!
//! LaTeX itself only *warns* on multiply-defined labels (and silently uses the
//! last), so this is a [`Severity::Warning`], not an error. Cross-file dupes
//! (the same key defined in two `\input`-ed files) are out of scope here — this
//! reads only the per-file model — and wait on the cross-file resolver.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::linter::diagnostic::{Diagnostic, Severity};

use super::{Rule, RuleContext};

pub struct DuplicateLabel;

impl Rule for DuplicateLabel {
    fn id(&self) -> &'static str {
        "duplicate-label"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn run(&self, ctx: &RuleContext<'_>) -> Vec<Diagnostic> {
        // Count occurrences of each key, then flag every definition past the
        // first — the first is the "canonical" one LaTeX would keep.
        let mut seen: HashMap<&str, usize> = HashMap::new();
        let mut out = Vec::new();
        for label in ctx.model.labels() {
            let count = seen.entry(label.name.as_str()).or_insert(0);
            *count += 1;
            if *count > 1 {
                out.push(Diagnostic {
                    rule: self.id(),
                    severity: self.default_severity(),
                    path: PathBuf::new(),
                    start: usize::from(label.range.start()),
                    end: usize::from(label.range.end()),
                    message: format!("label `{}` is defined more than once", label.name),
                });
            }
        }
        out
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
        let ctx = RuleContext {
            path: std::path::Path::new("x.tex"),
            root: &root,
            model: &model,
        };
        DuplicateLabel.run(&ctx)
    }

    #[test]
    fn flags_only_the_second_definition() {
        let src = "\\label{a}\\label{a}\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "duplicate-label");
        // Points tightly at the second `\label{a}` (bytes 9..18), not beyond.
        assert_eq!((out[0].start, out[0].end), (9, 18));
    }

    #[test]
    fn caret_excludes_an_over_attached_second_group() {
        // The greedy parser attaches `{x}` as a spurious second arg to the
        // duplicate `\label{a}`; the caret must still stop at the key group.
        let out = findings("\\label{a}\\label{a}\n{x}\n");
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].start, out[0].end), (9, 18));
    }

    #[test]
    fn distinct_keys_are_fine() {
        assert!(findings("\\label{a}\\label{b}\n").is_empty());
    }

    #[test]
    fn three_definitions_flag_two() {
        assert_eq!(findings("\\label{a}\\label{a}\\label{a}\n").len(), 2);
    }
}
