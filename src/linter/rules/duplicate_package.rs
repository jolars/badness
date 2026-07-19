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
//! **Branch exclusivity.** Two loads in different branches of one
//! `\if…\else…\fi` never both run, so they are not a duplicate —
//! `\iftrue\usepackage{p}\else\usepackage{p}\fi` loads `p` exactly once no
//! matter which branch TeX takes. Branch membership comes from the shared
//! [`crate::linter::conditional`] pre-pass (pair-and-trust: any `if*`-named
//! conditional counts, `\newif`-defined ones included; see the module docs for
//! the denylist of brace-argument `if*` macros like `\ifthenelse`, whose
//! branches are *not* recognized). A load is flagged only when some earlier
//! load of the same package can coexist with it.
//!
//! **No autofix:** removing a duplicate can drop options the surviving load
//! lacks, and choosing which load to keep is the author's call (mirroring
//! `duplicate-label`).
//!
//! Caret: each finding points at the redundant load *command*. A comma list
//! (`\usepackage{a,b}`) yields one edge per name sharing the single command
//! range, so a duplicate that lives inside or across a list underlines the whole
//! command rather than the individual name token.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::linter::conditional::{Frame, mutually_exclusive};
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
         when the options disagree, an option-clash error. Loads in mutually \
         exclusive branches of a TeX conditional \
         (`\\iftrue...\\else...\\fi`, `\\newif`-defined conditionals included) \
         are not duplicates and are not flagged; `if`-named macros that take \
         brace arguments instead of a `\\fi` terminator (`\\ifthenelse` and \
         friends) carry no recognized branches. No autofix: removing a \
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
        // collide on one key. Per key, every prior load's branch path is kept:
        // a load is a duplicate only if some prior load can coexist with it, so
        // a third unconditional load is still flagged even when the first two
        // were mutually exclusive with each other.
        let mut seen: HashMap<PathBuf, Vec<&[Frame]>> = HashMap::new();
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
            let path_here = ctx.conditional_path_at(usize::from(edge.range.start()));
            let priors = seen.entry(path.clone()).or_default();
            let is_duplicate = priors.iter().any(|p| !mutually_exclusive(p, path_here));
            // Record after comparing — a load is never compared against itself.
            priors.push(path_here);
            if !is_duplicate {
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
                related: Vec::new(),
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
        let ctx = RuleContext::new(
            std::path::Path::new("x.tex"),
            &root,
            &model,
            None,
            None,
            None,
        );
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

    // --- Conditional branch exclusivity -------------------------------------

    #[test]
    fn if_else_branches_are_not_flagged() {
        assert!(
            findings("\\iftrue\\usepackage[opt1]{pkg}\\else\\usepackage[opt2]{pkg}\\fi\n")
                .is_empty()
        );
    }

    #[test]
    fn ifcase_or_branches_are_not_flagged() {
        assert!(
            findings(
                "\\ifcase 0 \\usepackage{pkg}\\or\\usepackage{pkg}\\or\\usepackage{pkg}\\fi\n"
            )
            .is_empty()
        );
    }

    #[test]
    fn same_branch_is_still_flagged() {
        let out = findings("\\iftrue\\usepackage{a}\\usepackage{a}\\else\\usepackage{b}\\fi\n");
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains('a'), "got: {}", out[0].message);
    }

    #[test]
    fn unconditional_load_after_exclusive_pair_is_still_flagged() {
        // The two conditional loads are mutually exclusive, but the third,
        // unconditional one can coexist with whichever branch ran.
        let out =
            findings("\\iftrue\\usepackage{pkg}\\else\\usepackage{pkg}\\fi\n\\usepackage{pkg}\n");
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn unknown_conditional_branches_are_not_flagged() {
        // Pair-and-trust: a `\newif`-defined conditional's branches are as
        // exclusive as a primitive's.
        assert!(findings("\\ifmyflag\\usepackage{pkg}\\else\\usepackage{pkg}\\fi\n").is_empty());
    }

    #[test]
    fn unknown_conditionals_else_does_not_shield_a_real_duplicate() {
        // The `\else` belongs to `\ifmyflag`, so both loads sit in `\iftrue`'s
        // then-branch and can run together when `myflag` is false.
        let out = findings("\\iftrue \\usepackage{p} \\ifmyflag \\else \\usepackage{p} \\fi\\fi\n");
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn unknown_conditionals_fi_does_not_break_exclusivity() {
        // The inner `\ifmyflag x\fi` pairs with itself; the outer pair stays
        // exclusive.
        assert!(
            findings("\\iftrue \\ifmyflag x\\fi \\usepackage{p} \\else \\usepackage{p} \\fi\n")
                .is_empty()
        );
    }

    #[test]
    fn conditional_tokens_in_definition_bodies_are_inert() {
        // The `\else` inside the `\newcommand` body is carried code, not live
        // control flow: both loads share `\iftrue`'s then-branch.
        let out = findings(
            "\\iftrue \\usepackage{p} \\newcommand{\\rest}{\\else} \\usepackage{p} \\fi\n",
        );
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn if_named_operands_do_not_open_frames() {
        // `\iffalse` is `\ifdefined`'s operand, not a conditional here; the
        // outer `\iftrue…\else` pair stays exclusive.
        assert!(
            findings(
                "\\iftrue \\usepackage{p} \\ifdefined\\iffalse\\fi \\else \\usepackage{p} \\fi\n"
            )
            .is_empty()
        );
    }

    #[test]
    fn iftex_engine_dispatch_is_not_flagged() {
        // The kernel-provided iftex conditionals are the most common wrapper
        // for exactly this alternative-load pattern.
        assert!(
            findings("\\ifLuaTeX\\usepackage{fontspec}\\else\\usepackage{fontspec}\\fi\n")
                .is_empty()
        );
    }

    #[test]
    fn ifthenelse_arguments_carry_no_recognized_branches() {
        // `\ifthenelse`'s brace arguments are exclusive at runtime, but it is
        // a macro, not a `\fi`-terminated conditional: denylisted, so the two
        // loads are flagged as before (a documented limitation).
        let out = findings("\\ifthenelse{\\boolean{x}}{\\usepackage{a}}{\\usepackage{a}}\n");
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn math_iff_does_not_disturb_tracking() {
        assert!(findings("$a \\iff b$\n\\usepackage{x}\n\\usepackage{y}\n").is_empty());
    }

    #[test]
    fn newif_declaration_does_not_suppress_a_real_duplicate() {
        // `\newif\ifmyflag` declares a conditional; it must not open a frame
        // that would make the two unconditional loads look exclusive.
        let out = findings("\\newif\\ifmyflag\n\\usepackage{a}\n\\usepackage{a}\n");
        assert_eq!(out.len(), 1);
    }
}
