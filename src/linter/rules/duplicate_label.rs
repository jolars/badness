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
//! [`ResolvedLabels`]: crate::project::ResolvedLabels

use std::collections::HashMap;
use std::path::PathBuf;

use rowan::TextRange;

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
         keeps the last definition. No autofix: resolving a collision (rename \
         vs delete) is the author's call."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn check_file(&self, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        // Count occurrences of each key, remembering the *first* definition's key
        // range so a later duplicate can point back at it. Within a file, flag
        // every definition past the first — the first is the "canonical" one
        // LaTeX would keep. On a key's *first* occurrence in this file, the
        // cross-file branch instead checks whether another file in the namespace
        // defines it (the intra-file branch already owns the 2nd+ occurrences, so
        // no overlap).
        let mut seen: HashMap<&str, TextRange> = HashMap::new();
        for label in ctx.model.labels() {
            let (message, related) = match seen.get(label.name.as_str()) {
                Some(&first) => (
                    Some(format!("label `{}` is defined more than once", label.name)),
                    // The secondary points at the first definition, in this file.
                    vec![RelatedInfo {
                        path: ctx.path.to_path_buf(),
                        start: usize::from(first.start()),
                        end: usize::from(first.end()),
                        message: format!("first definition of `{}`", label.name),
                    }],
                ),
                None => ctx
                    .resolution
                    .and_then(|resolution| cross_file_finding(ctx, resolution, &label.name))
                    .map_or((None, Vec::new()), |(msg, related)| (Some(msg), related)),
            };
            seen.entry(label.name.as_str()).or_insert(label.key_range);
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
