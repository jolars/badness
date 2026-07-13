//! `unknown-option`: a `\usepackage`/`\RequirePackage` load passing an option
//! the target package never declares.
//!
//! LaTeX raises an "Unknown option" error when a package processes an option it
//! has no `\DeclareOption` for (and no `\DeclareOption*` default handler to
//! swallow it). We can check this statically only when the target is an
//! *analyzed member* `.sty` — the project's package-option model
//! (`RuleContext::packages`) carries each member's declared set — so system
//! packages, which ship no option data, are never judged.
//!
//! **Conservative by construction** (a false positive here is worse than a
//! miss): the rule is silent when the options bracket holds non-literal
//! content, when an option looks key=value (a keyval processor we failed to
//! recognize is likelier than a typo), and when the target's
//! [`handles_unknown`](crate::project::PackageOptionFacts::handles_unknown)
//! flag is set — a `\DeclareOption*`, a dynamic-named `\DeclareOption`, a
//! keyval-family processor, an option-forwarding load, or an `\input` that
//! could smuggle in declarations.
//!
//! **No closed/rooted gate**, unlike `undefined-ref`: that rule proves a
//! *negative* over a whole namespace, so an unanalyzed member could hide the
//! definition it is looking for. Here the declared set lives inside the target
//! file itself — an unanalyzed file can only make us silent (the target is not
//! in the model), never wrong.
//!
//! Class loads (`\documentclass`/`\LoadClass`) are not checked: an unknown
//! *class* option is not a LaTeX error — it becomes an unused global option,
//! silently offered to every package.
//!
//! **No autofix:** dropping the option changes document behavior, and the
//! nearest declared option is the author's call.
//!
//! Caret: each finding underlines the offending option text itself, not the
//! whole load command.

use std::path::PathBuf;

use crate::linter::diagnostic::{Diagnostic, Severity};
use crate::project::{PackageKind, load_option_args, resolve_load_target};
use crate::syntax::{SyntaxElement, SyntaxKind};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[Example {
    caption: "Loading the sibling package with an option it never declares:",
    source: "\\usepackage[final]{mypkg}\n",
}];

/// The sibling `.sty` the docs example loads: declares only `draft`, so the
/// example's `final` is unknown.
const EXAMPLE_COMPANION_STY: &str = "\\ProvidesPackage{mypkg}[2026/01/01 v1.0 Demo package]\n\\DeclareOption{draft}{}\n\\ProcessOptions\\relax\n";

pub struct UnknownOption;

impl Rule for UnknownOption {
    fn id(&self) -> &'static str {
        "unknown-option"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag a `\\usepackage`/`\\RequirePackage` option that the loaded package \
         never declares with `\\DeclareOption`, which LaTeX reports as an \
         \"Unknown option\" error at compile time. Checked only against packages \
         that are analyzed project files (a sibling `.sty`) — no option data \
         ships for system packages — and only when the package's declared set is \
         trustworthy: a `\\DeclareOption*` default handler, a key-value option \
         processor (`kvoptions`, `\\ProcessKeyOptions`, …), option forwarding, or \
         an `\\input` in the package silences the rule, as does a `key=value` \
         option. Class loads (`\\documentclass`) are not checked: an unknown \
         class option is not an error, it becomes an unused global option. No \
         autofix: dropping or renaming the option is the author's call."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn example_companions(&self) -> &'static [(&'static str, &'static str)] {
        &[("mypkg.sty", EXAMPLE_COMPANION_STY)]
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::COMMAND]
    }

    fn check(&self, el: &SyntaxElement, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(packages) = ctx.packages else {
            return;
        };
        let Some(node) = el.as_node() else { return };
        let kind = match crate::ast::command_name(node).as_deref() {
            Some("usepackage") => PackageKind::UsePackage,
            Some("RequirePackage") => PackageKind::RequirePackage,
            _ => return,
        };

        // `None`: no bracket, or dynamic content — nothing literal to judge.
        let Some(options) = load_option_args(node) else {
            return;
        };
        if options.is_empty() {
            return;
        }
        // Dynamic target (`\usepackage[x]{\pkg}`) — nothing to resolve.
        let Some(names) = crate::ast::nth_group_text(node, 0) else {
            return;
        };

        for name in names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
            let target = resolve_load_target(name, kind, ctx.path.parent());
            // A system package or non-member: no facts, no judgment.
            let Some(facts) = packages.get(&target) else {
                continue;
            };
            if facts.handles_unknown {
                continue;
            }
            for opt in &options {
                // `key=value` implies a keyval processor our recognizer missed;
                // interior whitespace means we mis-read the bracket. Skip both.
                if opt.text.contains('=') || opt.text.contains(char::is_whitespace) {
                    continue;
                }
                if facts.declares(&opt.text) {
                    continue;
                }
                sink.push(Diagnostic {
                    rule: self.id(),
                    severity: self.default_severity(),
                    path: PathBuf::new(),
                    start: usize::from(opt.range.start()),
                    end: usize::from(opt.range.end()),
                    message: format!("unknown option `{}` for package `{name}`", opt.text),
                    fix: None,
                    related: Vec::new(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::parser::{parse, parse_with_flavor};
    use crate::project::{ResolvedPackageOptions, package_option_facts};
    use crate::semantic::SemanticModel;
    use crate::syntax::SyntaxNode;

    /// Build a `ResolvedPackageOptions` from sibling `.sty` sources, then lint
    /// `src` (as `/p/main.tex`) against it.
    fn findings_with(src: &str, stys: &[(&str, &str)]) -> Vec<Diagnostic> {
        let facts = stys.iter().filter_map(|(path, sty_src)| {
            let kind = crate::file_discovery::file_kind_or_tex(Path::new(path));
            let root = SyntaxNode::new_root(parse_with_flavor(sty_src, kind.lex_config()).green);
            let model = SemanticModel::build(&root);
            package_option_facts(Path::new(path), &root, &model)
        });
        let packages = ResolvedPackageOptions::build(facts);
        findings_against(src, Some(&packages))
    }

    fn findings_against(src: &str, packages: Option<&ResolvedPackageOptions>) -> Vec<Diagnostic> {
        let root = SyntaxNode::new_root(parse(src).green);
        let model = SemanticModel::build(&root);
        let ctx = RuleContext::new(
            Path::new("/p/main.tex"),
            &root,
            &model,
            None,
            None,
            packages,
        );
        let mut out = Vec::new();
        for el in root.descendants_with_tokens() {
            if UnknownOption.interests().contains(&el.kind()) {
                UnknownOption.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    const DECLARES_DRAFT: &str = "\\DeclareOption{draft}{}\n\\ProcessOptions\\relax\n";

    #[test]
    fn unknown_option_is_flagged_with_tight_span() {
        let src = "\\usepackage[final]{mypkg}\n";
        let out = findings_with(src, &[("/p/mypkg.sty", DECLARES_DRAFT)]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "unknown-option");
        assert_eq!(&src[out[0].start..out[0].end], "final");
        assert!(
            out[0].message.contains("final") && out[0].message.contains("mypkg"),
            "got: {}",
            out[0].message
        );
        assert!(out[0].fix.is_none());
    }

    #[test]
    fn declared_option_is_silent() {
        let out = findings_with(
            "\\usepackage[draft]{mypkg}\n",
            &[("/p/mypkg.sty", DECLARES_DRAFT)],
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn star_handler_target_is_silent() {
        let out = findings_with(
            "\\usepackage[anything]{mypkg}\n",
            &[("/p/mypkg.sty", "\\DeclareOption*{}\n")],
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn non_member_target_is_silent() {
        let out = findings_with(
            "\\usepackage[fleqn]{amsmath}\n",
            &[("/p/mypkg.sty", DECLARES_DRAFT)],
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn no_project_view_is_inert() {
        let out = findings_against("\\usepackage[final]{mypkg}\n", None);
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn each_unknown_option_is_flagged_separately() {
        let src = "\\usepackage[a,b]{mypkg}\n";
        let out = findings_with(src, &[("/p/mypkg.sty", DECLARES_DRAFT)]);
        assert_eq!(out.len(), 2);
        assert_eq!(&src[out[0].start..out[0].end], "a");
        assert_eq!(&src[out[1].start..out[1].end], "b");
    }

    #[test]
    fn key_value_option_is_skipped() {
        let out = findings_with(
            "\\usepackage[width=3cm]{mypkg}\n",
            &[("/p/mypkg.sty", DECLARES_DRAFT)],
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn dynamic_bracket_is_inert() {
        let out = findings_with(
            "\\usepackage[\\myopts]{mypkg}\n",
            &[("/p/mypkg.sty", DECLARES_DRAFT)],
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn comma_list_flags_only_the_member_and_names_it() {
        let src = "\\usepackage[x]{amsmath,mypkg}\n";
        let out = findings_with(src, &[("/p/mypkg.sty", DECLARES_DRAFT)]);
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains("mypkg"), "got: {}", out[0].message);
    }

    #[test]
    fn require_package_fires_like_usepackage() {
        let out = findings_with(
            "\\RequirePackage[final]{mypkg}\n",
            &[("/p/mypkg.sty", DECLARES_DRAFT)],
        );
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn documentclass_is_never_checked() {
        // Even with a member class next door, class options are global options,
        // not errors. (The facts extractor also returns `None` for `.cls`.)
        let out = findings_with(
            "\\documentclass[weird]{myclass}\n",
            &[("/p/myclass.cls", DECLARES_DRAFT)],
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn empty_declared_set_without_processors_fires() {
        // A package declaring no options at all: any option is a real LaTeX
        // "Unknown option" error.
        let out = findings_with(
            "\\usepackage[anything]{mypkg}\n",
            &[("/p/mypkg.sty", "\\ProvidesPackage{mypkg}\n")],
        );
        assert_eq!(out.len(), 1);
    }
}
