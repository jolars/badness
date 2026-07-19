//! `duplicate-label`: a `\label{key}` defined more than once in the same label
//! namespace — within one file, or across files that share a document (the
//! cross-file branch, when a [`ResolvedLabels`] is available).
//!
//! LaTeX itself only *warns* on multiply-defined labels (and silently uses the
//! last), so this is a [`Severity::Warning`], not an error. The cross-file branch
//! needs no closed/rooted gate: a key defined in two analyzed files is a true
//! duplicate regardless of any unanalyzed includes (adding files only reveals
//! *more* duplicates).
//!
//! **Branch exclusivity (intra-file).** Two definitions in different branches
//! of one `\if…\else…\fi` never both run, so they are not a duplicate —
//! `\iftrue\label{a}\else\label{a}\fi` defines `a` exactly once. Branch
//! membership comes from the shared [`crate::linter::conditional`] pre-pass
//! (pair-and-trust; see its docs for the `\ifthenelse`-style denylist). A
//! definition is flagged only when some earlier definition of the same key can
//! coexist with it, and the related location points at the first such
//! coexisting definition. The cross-file branch stays name-only by design and
//! is untouched.
//!
//! [`ResolvedLabels`]: crate::project::ResolvedLabels

use std::collections::HashMap;
use std::path::PathBuf;

use rowan::TextRange;

use crate::linter::conditional::{Frame, mutually_exclusive};
use crate::linter::diagnostic::{Diagnostic, RelatedInfo, Severity};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[Example {
    caption: "The same key defined twice in one file:",
    source: "\\section{One}\\label{sec:x}\n\\section{Two}\\label{sec:x}\n",
}];

pub struct DuplicateLabel;

impl Rule for DuplicateLabel {
    fn id(&self) -> &'static str {
        "duplicate-label"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag a `\\label{key}` defined more than once in the same label \
         namespace -- within one file, or across files that share a document \
         when a project view is available. LaTeX itself only warns and silently \
         keeps the last definition. Definitions in mutually exclusive branches \
         of a TeX conditional (`\\iftrue...\\else...\\fi`, `\\newif`-defined \
         conditionals included) are not duplicates and are not flagged. No \
         autofix: resolving a collision (rename vs delete) is the author's \
         call."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn check_file(&self, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        // Track every prior definition of each key with its conditional branch
        // path. Within a file, flag a definition only when some earlier one can
        // coexist with it (all-exclusive priors mean at most one runs), and
        // point the secondary at the first coexisting prior — the definition
        // LaTeX would actually clash with. On a key's *first* occurrence in
        // this file, the cross-file branch instead checks whether another file
        // in the namespace defines it (the intra-file branch already owns the
        // 2nd+ occurrences, so no overlap).
        let mut seen: HashMap<&str, Vec<(TextRange, &[Frame])>> = HashMap::new();
        for label in ctx.model.labels() {
            let path_here = ctx.conditional_path_at(usize::from(label.range.start()));
            let priors = seen.entry(label.name.as_str()).or_default();
            let coexisting = priors
                .iter()
                .find(|(_, p)| !mutually_exclusive(p, path_here))
                .map(|&(key_range, _)| key_range);
            let (message, related) = match coexisting {
                Some(first) => (
                    Some(format!("label `{}` is defined more than once", label.name)),
                    // The secondary points at the first coexisting definition,
                    // in this file.
                    vec![RelatedInfo {
                        path: ctx.path.to_path_buf(),
                        start: usize::from(first.start()),
                        end: usize::from(first.end()),
                        message: format!("first definition of `{}`", label.name),
                    }],
                ),
                None if priors.is_empty() => ctx
                    .resolution
                    .and_then(|resolution| cross_file_finding(ctx, resolution, &label.name))
                    .map_or((None, Vec::new()), |(msg, related)| (Some(msg), related)),
                // Later definitions that are exclusive with every prior get no
                // finding at all (and no cross-file re-report).
                None => (None, Vec::new()),
            };
            priors.push((label.key_range, path_here));
            if let Some(message) = message {
                sink.push(Diagnostic {
                    rule: self.id(),
                    severity: self.default_severity(),
                    path: PathBuf::new(),
                    start: usize::from(label.range.start()),
                    end: usize::from(label.range.end()),
                    message,
                    fix: None,
                    related,
                });
            }
        }
    }
}

/// The cross-file finding when `name` is defined in another file of `ctx.path`'s
/// namespace: an "also defined in …" message plus one file-level [`RelatedInfo`]
/// per other definer. `None` when `name` is unique across files. Other definers
/// are sorted (from [`ResolvedLabels`]) so both message and related list are
/// deterministic.
///
/// The related locations are **file-level** (a `0..0` range pointing at the
/// file's start): [`ResolvedLabels`] tracks which files define a label, not the
/// definition's byte range, and it stays that way deliberately (the name-only
/// `file_labels` firewall keeps cross-file resolution stable under prose edits).
fn cross_file_finding(
    ctx: &RuleContext<'_>,
    resolution: &crate::project::ResolvedLabels,
    name: &str,
) -> Option<(String, Vec<RelatedInfo>)> {
    let others: Vec<&PathBuf> = resolution
        .definers(ctx.path, name)
        .iter()
        .filter(|definer| definer.as_path() != ctx.path)
        .collect();
    if others.is_empty() {
        return None;
    }
    let message = format!(
        "label `{name}` is also defined in {}",
        others
            .iter()
            .map(|definer| format!("`{}`", definer.display()))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let related = others
        .iter()
        .map(|definer| RelatedInfo {
            path: (*definer).clone(),
            start: 0,
            end: 0,
            message: format!("other definition of `{name}`"),
        })
        .collect();
    Some((message, related))
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
        DuplicateLabel.check_file(&ctx, &mut out);
        out
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
    fn intra_file_related_points_at_the_first_definition() {
        // The second `\label{a}` is flagged; its related location is the *first*
        // definition's key (`a`, bytes 7..8), in the same file.
        let out = findings("\\label{a}\\label{a}\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].related.len(), 1);
        let ri = &out[0].related[0];
        assert_eq!(ri.path, std::path::Path::new("x.tex"));
        assert_eq!((ri.start, ri.end), (7, 8));
        assert_eq!(ri.message, "first definition of `a`");
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

    // --- Conditional branch exclusivity -------------------------------------

    #[test]
    fn if_else_branches_are_not_flagged() {
        assert!(findings("\\iftrue\\label{a}\\else\\label{a}\\fi\n").is_empty());
    }

    #[test]
    fn same_branch_is_still_flagged() {
        assert_eq!(
            findings("\\iftrue\\label{a}\\label{a}\\else x\\fi\n").len(),
            1
        );
    }

    #[test]
    fn unknown_conditional_branches_are_not_flagged() {
        assert!(findings("\\ifmyflag\\label{a}\\else\\label{a}\\fi\n").is_empty());
    }

    #[test]
    fn unconditional_definition_after_exclusive_pair_is_flagged_once() {
        // The two conditional definitions are mutually exclusive, but the
        // third, unconditional one can coexist with whichever branch ran; its
        // related location is the *first* coexisting prior (the then-branch
        // definition's key, bytes 15..16).
        let src = "\\iftrue\\label{a}\\else\\label{a}\\fi\\label{a}\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].related.len(), 1);
        let ri = &out[0].related[0];
        assert_eq!((ri.start, ri.end), (14, 15));
        assert_eq!(&src[ri.start..ri.end], "a");
    }

    #[test]
    fn ifthenelse_arguments_carry_no_recognized_branches() {
        // `\ifthenelse` is a brace-argument macro, not a `\fi`-terminated
        // conditional: denylisted, so the pair is flagged as before.
        assert_eq!(
            findings("\\ifthenelse{\\boolean{x}}{\\label{a}}{\\label{a}}\n").len(),
            1
        );
    }

    #[test]
    fn conditional_tokens_in_definition_bodies_are_inert() {
        // The `\else` inside the `\newcommand` body must not make the two
        // same-branch definitions look exclusive.
        assert_eq!(
            findings("\\iftrue\\label{a}\\newcommand{\\x}{\\else}\\label{a}\\fi\n").len(),
            1
        );
    }

    // --- Cross-file branch (needs a `ResolvedLabels`) -----------------------

    use crate::project::ResolvedLabels;
    use crate::project::graph::{FileFacts, IncludeGraph};
    use smol_str::SmolStr;

    /// A two-file namespace (`main.tex` `\input`s `other.tex`), each defining the
    /// labels given. Both share one component, so a key in both is a cross-file
    /// duplicate.
    fn two_file_resolution(main_labels: &[&str], other_labels: &[&str]) -> ResolvedLabels {
        let graph = IncludeGraph::build(
            &[
                FileFacts {
                    path: PathBuf::from("main.tex"),
                    include_edges: vec![crate::project::IncludeEdgeKey {
                        kind: crate::project::IncludeKind::Input,
                        target: crate::project::IncludeTarget::Path(PathBuf::from("other.tex")),
                    }],
                },
                FileFacts {
                    path: PathBuf::from("other.tex"),
                    include_edges: Vec::new(),
                },
            ],
            None,
        );
        let names = |list: &[&str]| list.iter().map(SmolStr::new).collect::<Vec<_>>();
        ResolvedLabels::build(
            &[
                (
                    PathBuf::from("main.tex"),
                    names(main_labels),
                    Vec::new(),
                    true,
                ),
                (
                    PathBuf::from("other.tex"),
                    names(other_labels),
                    Vec::new(),
                    false,
                ),
            ],
            &graph,
        )
    }

    fn cross_findings(src: &str, path: &str, resolution: &ResolvedLabels) -> Vec<Diagnostic> {
        let root = SyntaxNode::new_root(parse(src).green);
        let model = SemanticModel::build(&root);
        let ctx = RuleContext::new(
            std::path::Path::new(path),
            &root,
            &model,
            Some(resolution),
            None,
            None,
        );
        let mut out = Vec::new();
        DuplicateLabel.check_file(&ctx, &mut out);
        out
    }

    #[test]
    fn cross_file_duplicate_names_the_other_file() {
        let r = two_file_resolution(&["shared"], &["shared"]);
        let out = cross_findings("\\label{shared}\n", "main.tex", &r);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "duplicate-label");
        assert!(
            out[0].message.contains("also defined in `other.tex`"),
            "got: {}",
            out[0].message
        );
        // The secondary is a file-level link (`0..0`) to the other definer.
        assert_eq!(out[0].related.len(), 1);
        let ri = &out[0].related[0];
        assert_eq!(ri.path, PathBuf::from("other.tex"));
        assert_eq!((ri.start, ri.end), (0, 0));
        assert_eq!(ri.message, "other definition of `shared`");
    }

    #[test]
    fn cross_file_unique_key_is_fine() {
        let r = two_file_resolution(&["only-main"], &["only-other"]);
        assert!(cross_findings("\\label{only-main}\n", "main.tex", &r).is_empty());
    }

    #[test]
    fn intra_and_cross_file_do_not_double_report_first_occurrence() {
        // `dup` appears twice in main AND once in other. The first occurrence is
        // flagged cross-file ("also defined in other"); the second intra-file
        // ("defined more than once"). Two findings, distinct messages.
        let r = two_file_resolution(&["dup"], &["dup"]);
        let out = cross_findings("\\label{dup}\\label{dup}\n", "main.tex", &r);
        assert_eq!(out.len(), 2);
        assert!(out[0].message.contains("also defined in"));
        assert!(out[1].message.contains("defined more than once"));
    }
}
