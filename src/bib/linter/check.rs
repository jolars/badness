//! The bib lint driver: run every bib rule over one `.bib` file, stamp paths, and
//! sort. The bib analog of [`crate::linter::check`], and the entry point the CLI
//! calls for `.bib` inputs.
//!
//! BibTeX suppression directives live in structured `@comment{...}` entries.
//! The shared driver shape is the same as LaTeX: build the map once, expose it
//! to whole-file rules, and filter ordinary findings after dispatch.

use std::path::Path;

use crate::bib::parse;
use crate::bib::semantic::{self, Model};
use crate::bib::syntax::{SyntaxKind, SyntaxNode};
use crate::linter::diagnostic::Diagnostic;
use crate::linter::rules::is_unsuppressible_suppression_meta_rule;

use super::rules::{BibRuleContext, all_rules};
use super::suppression::BibSuppressionMap;

/// Parse and lint a single `.bib` file's `text` from scratch, returning its parse
/// diagnostics plus rule findings. The self-contained analog of
/// [`crate::linter::check::check_document`] for `.bib`: callers that hold only text
/// (notably the CLI) get everything in one call.
pub fn check_document(path: &Path, text: &str) -> Vec<Diagnostic> {
    let parsed = parse(text);
    let mut diagnostics: Vec<Diagnostic> = parsed
        .errors
        .iter()
        .map(|err| Diagnostic::from_parse(path.to_path_buf(), err))
        .collect();
    let root = parsed.syntax();
    let model = Model::build(&root);
    diagnostics.extend(lint_document(path, &root, &model));
    diagnostics
}

/// Run all built-in bib rules against `root`/`model`, returning the diagnostics
/// (`path` stamped, sorted by position).
///
/// `root` and `model` must describe the same file as `path`. Mirrors
/// [`crate::linter::check::lint_document`], minus the cross-file `resolution`
/// argument (no bib rule is cross-file-sensitive yet).
pub fn lint_document(path: &Path, root: &SyntaxNode, model: &Model) -> Vec<Diagnostic> {
    let suppress = BibSuppressionMap::build(root);
    let ctx = BibRuleContext {
        path,
        root,
        model,
        db: semantic::builtin(),
        suppressions: &suppress,
    };
    let rules = all_rules();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();

    // Build the node-dispatch table: kind discriminant -> indices of subscribed
    // rules. Bib `SyntaxKind` is a contiguous `#[repr(u16)]` with `ROOT` last, so a
    // flat Vec indexed by `kind as usize` beats a hash map. Same shape as the LaTeX
    // driver.
    let mut by_kind: Vec<Vec<usize>> = vec![Vec::new(); SyntaxKind::COUNT];
    let mut any_node_rules = false;
    for (i, rule) in rules.iter().enumerate() {
        for kind in rule.interests() {
            by_kind[*kind as usize].push(i);
            any_node_rules = true;
        }
    }

    // Single shared traversal feeding every node-shape rule. Visits tokens too so a
    // future token-level rule (e.g. an encoding-hint scan) can subscribe.
    if any_node_rules {
        for el in root.descendants_with_tokens() {
            for &i in &by_kind[el.kind() as usize] {
                rules[i].check(&el, &ctx, &mut diagnostics);
            }
        }
    }

    // Whole-file pass for model-driven rules.
    for rule in &rules {
        rule.check_file(&ctx, &mut diagnostics);
    }

    // Filter out findings suppressed by a `@comment{badness-lint …}` carrier.
    diagnostics.retain(|d| {
        is_unsuppressible_suppression_meta_rule(d.rule)
            || !suppress.is_suppressed(d.rule, d.start, d.end)
    });

    for d in &mut diagnostics {
        d.path = path.to_path_buf();
    }
    diagnostics.sort_by_key(|d| (d.start, d.end, d.rule));
    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules_of(src: &str) -> Vec<&'static str> {
        check_document(Path::new("x.bib"), src)
            .iter()
            .map(|d| d.rule)
            .collect()
    }

    #[test]
    fn clean_file_produces_nothing() {
        let src = "@article{k,\n  author = {A},\n  title = {T},\n  journaltitle = {J},\n  year = 2020,\n}\n";
        assert!(rules_of(src).is_empty(), "got: {:?}", rules_of(src));
    }

    #[test]
    fn collects_multiple_rule_families_sorted() {
        // A duplicate key and an unused @string, sorted by position.
        let src = "@string{unused = {U}}\n@misc{k, title = {A}}\n@misc{k, title = {B}}\n";
        let rules = rules_of(src);
        assert!(rules.contains(&"unused-string"));
        assert!(rules.contains(&"duplicate-key"));
    }
}
