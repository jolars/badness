//! `duplicate-package`: the same package loaded more than once in one file via
//! `\usepackage`/`\RequirePackage`.
//!
//! LaTeX loads a given `.sty` only once — a second `\usepackage{amsmath}` is a
//! no-op at best, and an *option clash* error when the two loads disagree on
//! options. Either way the second load is redundant, so this is a
//! [`Severity::Warning`]. `\usepackage` and `\RequirePackage` share one package
//! namespace (both pull in the same `.sty`), so a package loaded by one and then
//! the other still counts as a duplicate.
//!
//! **Intra-file only.** A package legitimately loaded by both the document and a
//! package it pulls in is idempotent in LaTeX, so there is no cross-file branch;
//! the rule reads only this file's load edges.
//!
//! **No autofix:** removing a duplicate can drop options the surviving load
//! lacks, and choosing which load to keep is the author's call (mirroring
//! `duplicate-label`).
//!
//! Caret: each finding points at the redundant load *command*. A comma list
//! (`\usepackage{a,b}`) yields one edge per name sharing the single command
//! range, so a duplicate that lives inside or across a list underlines the whole
//! command rather than the individual name token.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::linter::diagnostic::{Diagnostic, Severity};
use crate::project::{PackageKind, PackageTarget, collect_package_edges};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[Example {
    caption: "The same package loaded twice:",
    source: "\\usepackage{amsmath}\n\\usepackage{amsmath}\n",
}];

pub struct DuplicatePackage;

impl Rule for DuplicatePackage {
    fn id(&self) -> &'static str {
        "duplicate-package"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag a package loaded more than once in the same file with \
         `\\usepackage`/`\\RequirePackage` (which share one package namespace). \
         LaTeX loads a given package only once; a second load is redundant and, \
         when the options disagree, an option-clash error. No autofix: removing a \
         load can drop options the survivor lacks, and which load to keep is the \
         author's call. Class loads (`\\documentclass`/`\\LoadClass`) are a \
         separate concern and are not flagged."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn check_file(&self, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        // Package loads only (`\usepackage`/`\RequirePackage`); class loads share
        // no `.sty` namespace with them. `base_dir` is `None`, so a bare name like
        // `amsmath` resolves to `amsmath.sty` and both spellings of the same load
        // collide on one key.
        let mut seen: HashSet<PathBuf> = HashSet::new();
        for edge in collect_package_edges(ctx.root, None) {
            if !matches!(
                edge.kind,
                PackageKind::UsePackage | PackageKind::RequirePackage
            ) {
                continue;
            }
            // A `Dynamic` target (`\usepackage{\pkg}`) is not statically comparable.
            let PackageTarget::Path(path) = &edge.target else {
                continue;
            };
            if seen.insert(path.clone()) {
                continue;
            }
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_owned)
                .unwrap_or_else(|| path.display().to_string());
            sink.push(Diagnostic {
                rule: self.id(),
                severity: self.default_severity(),
                path: PathBuf::new(),
                start: usize::from(edge.range.start()),
                end: usize::from(edge.range.end()),
                message: format!("package `{name}` is loaded more than once"),
                fix: None,
            });
        }
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
        let ctx = RuleContext::new(std::path::Path::new("x.tex"), &root, &model, None, None);
        let mut out = Vec::new();
        DuplicatePackage.check_file(&ctx, &mut out);
        out
    }

    #[test]
    fn flags_only_the_second_load() {
        let out = findings("\\usepackage{amsmath}\n\\usepackage{amsmath}\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "duplicate-package");
        assert!(
            out[0].message.contains("amsmath"),
            "got: {}",
            out[0].message
        );
        // Points at the second `\usepackage{amsmath}` (bytes 21..41), not the first.
        assert_eq!((out[0].start, out[0].end), (21, 41));
    }

    #[test]
    fn distinct_packages_are_fine() {
        assert!(findings("\\usepackage{amsmath}\n\\usepackage{amssymb}\n").is_empty());
    }

    #[test]
    fn comma_list_then_reload_flags_once() {
        // `amsmath` appears in the list and again standalone: one duplicate.
        let out = findings("\\usepackage{amsmath,amssymb}\n\\usepackage{amsmath}\n");
        assert_eq!(out.len(), 1);
        assert!(
            out[0].message.contains("amsmath"),
            "got: {}",
            out[0].message
        );
    }

    #[test]
    fn require_package_and_usepackage_share_namespace() {
        let out = findings("\\RequirePackage{tools}\n\\usepackage{tools}\n");
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains("tools"), "got: {}", out[0].message);
    }

    #[test]
    fn duplicate_inside_one_list_flags_once() {
        let out = findings("\\usepackage{amsmath,amsmath}\n");
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn triple_load_flags_twice() {
        let out = findings("\\usepackage{a}\n\\usepackage{a}\n\\usepackage{a}\n");
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn dynamic_target_is_ignored() {
        assert!(findings("\\usepackage{\\pkg}\n\\usepackage{\\pkg}\n").is_empty());
    }

    #[test]
    fn duplicate_documentclass_is_not_this_rules_concern() {
        assert!(findings("\\documentclass{article}\n\\documentclass{article}\n").is_empty());
    }
}
