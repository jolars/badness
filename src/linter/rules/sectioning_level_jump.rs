//! `sectioning-level-jump`: a heading that descends more than one sectioning
//! level below the preceding heading (`\section` straight to `\subsubsection`,
//! skipping `\subsection`). Modeled on textidote's `sh:secskip`.
//!
//! LaTeX's standard sectioning commands form a fixed ladder — `\part` (0),
//! `\chapter` (1), `\section` (2), `\subsection` (3), `\subsubsection` (4),
//! `\paragraph` (5), `\subparagraph` (6). Descending the outline should step one
//! rung at a time; jumping past a rung produces a lopsided table of contents and
//! usually signals a wrong command. We flag only *downward* jumps of more than
//! one level between consecutive headings: climbing back up (closing sections) is
//! normal, and repeated headings at the same level are fine.
//!
//! The comparison is purely *relative* to the immediately preceding heading, never
//! against an absolute top level, so the rule is document-class agnostic — an
//! `article` that opens with `\section` is not treated as "skipping `\part` and
//! `\chapter`". The first heading in the document sets the baseline and is never
//! flagged.
//!
//! **Report-only** (no autofix). Fixing a skip means either promoting the offending
//! heading or inserting an intermediate heading — a structural, meaning-changing
//! choice that is the author's to make (tenet 1), not a correct-by-construction
//! textual edit.
//!
//! Whole-file rather than node-shape: the finding depends on the *sequence* of
//! headings in document order, which a per-node `check` (stateless across
//! elements) cannot track. Classification reads the curated built-in
//! [`signature`](crate::semantic::signature) DB directly, like
//! [`outline`](crate::semantic::outline) — sectioning is a static standard set, so
//! the bulk CWL tier is deliberately not consulted.

use std::path::PathBuf;

use crate::ast::{command_name, control_word_range};
use crate::linter::diagnostic::{Diagnostic, Severity};
use crate::semantic::signature;
use crate::syntax::{SyntaxElement, SyntaxKind};

use super::{Example, Rule, RuleContext, StreamVisitor};

/// The standard sectioning ladder, indexed by level (`\part` = 0 …
/// `\subparagraph` = 6), matching `data/signatures.json`'s `sectioning` values.
/// Used to name the previous heading and the expected intervening level in the
/// diagnostic message.
const LEVEL_NAMES: [&str; 7] = [
    "part",
    "chapter",
    "section",
    "subsection",
    "subsubsection",
    "paragraph",
    "subparagraph",
];

const EXAMPLES: &[Example] = &[Example {
    caption: "A heading that drops two levels at once (skipping `\\subsection`):",
    source: "\\section{Introduction}\n\\subsubsection{Details}\n",
}];

pub struct SectioningLevelJump;

impl Rule for SectioningLevelJump {
    fn id(&self) -> &'static str {
        "sectioning-level-jump"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag a heading that descends more than one sectioning level below the \
         preceding heading -- `\\section` straight to `\\subsubsection`, skipping \
         `\\subsection` (textidote's `sh:secskip`). Standard sectioning commands \
         form a fixed ladder (`\\part`, `\\chapter`, `\\section`, `\\subsection`, \
         `\\subsubsection`, `\\paragraph`, `\\subparagraph`); descending it a rung \
         at a time keeps the outline sound, and a jump usually signals the wrong \
         command. Only *downward* jumps between consecutive headings are flagged -- \
         climbing back up to close sections is normal, as are repeated headings at \
         one level. The comparison is relative to the previous heading, never an \
         absolute top level, so an `article` opening with `\\section` is fine. \
         Report-only: repairing a skip (promote the heading or insert an \
         intermediate one) is a structural choice for the author, not a \
         correct-by-construction edit."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    // Streaming rather than node-shape: the finding depends on the *sequence* of
    // headings in document order (the previous heading's level), which a stateless
    // per-element `check` cannot track. Rides the driver's one shared walk.
    fn stream(&self) -> Option<Box<dyn StreamVisitor>> {
        Some(Box::new(SectioningLevelJumpVisitor { prev_level: None }))
    }
}

/// Tracks the level of the immediately preceding heading across the shared walk;
/// a heading deeper than `prev + 1` skipped at least one rung of the ladder.
struct SectioningLevelJumpVisitor {
    prev_level: Option<u8>,
}

impl StreamVisitor for SectioningLevelJumpVisitor {
    fn visit(&mut self, el: &SyntaxElement, _ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(node) = el.as_node() else {
            return;
        };
        if node.kind() != SyntaxKind::COMMAND {
            return;
        }
        let Some(name) = command_name(node) else {
            return;
        };
        let Some(level) = signature::builtin()
            .command(&name)
            .and_then(|c| c.sectioning)
        else {
            return;
        };
        if let Some(prev) = self.prev_level
            && level > prev + 1
        {
            // The nearest missing rung; `prev + 1 <= level <= 6`, so the index is
            // in range.
            let expected = LEVEL_NAMES[(prev + 1) as usize];
            let previous = LEVEL_NAMES[prev as usize];
            let range = control_word_range(node).unwrap_or_else(|| node.text_range());
            sink.push(Diagnostic {
                rule: "sectioning-level-jump",
                severity: Severity::Warning,
                path: PathBuf::new(),
                start: usize::from(range.start()),
                end: usize::from(range.end()),
                message: format!(
                    "`\\{name}` skips a sectioning level after `\\{previous}` \
                     (expected `\\{expected}`)"
                ),
                fix: None,
                related: Vec::new(),
            });
        }
        self.prev_level = Some(level);
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
        let mut visitor = SectioningLevelJump.stream().expect("streaming rule");
        for el in root.descendants_with_tokens() {
            visitor.visit(&el, &ctx, &mut out);
        }
        visitor.finish(&ctx, &mut out);
        out
    }

    #[test]
    fn flags_section_to_subsubsection() {
        let src = "\\section{A}\n\\subsubsection{B}\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "sectioning-level-jump");
        assert!(
            out[0].message.contains("\\subsubsection")
                && out[0].message.contains("\\section")
                && out[0].message.contains("expected `\\subsection`"),
            "got: {}",
            out[0].message
        );
        // Report-only.
        assert!(out[0].fix.is_none());
        // Caret covers just the `\subsubsection` control word (bytes 12..26).
        let at = src.find("\\subsubsection").unwrap();
        assert_eq!(
            (out[0].start, out[0].end),
            (at, at + "\\subsubsection".len())
        );
    }

    #[test]
    fn stepwise_descent_is_fine() {
        assert!(findings("\\section{A}\n\\subsection{B}\n\\subsubsection{C}\n").is_empty());
    }

    #[test]
    fn climbing_back_up_is_fine() {
        // subsubsection -> section closes sections; not a downward jump.
        assert!(
            findings("\\section{A}\n\\subsection{B}\n\\subsubsection{C}\n\\section{D}\n")
                .is_empty()
        );
    }

    #[test]
    fn repeated_same_level_is_fine() {
        assert!(findings("\\section{A}\n\\section{B}\n\\section{C}\n").is_empty());
    }

    #[test]
    fn first_heading_sets_baseline_not_flagged() {
        // A document opening at subsubsection has no preceding heading to skip
        // against; it is the baseline, so nothing fires.
        assert!(findings("\\subsubsection{A}\n").is_empty());
    }

    #[test]
    fn sibling_after_jump_is_not_reflagged() {
        // The jump is flagged once; the following same-level heading is a sibling.
        let out = findings("\\section{A}\n\\subsubsection{B}\n\\subsubsection{C}\n");
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn part_to_section_skips_chapter() {
        let out = findings("\\part{A}\n\\section{B}\n");
        assert_eq!(out.len(), 1);
        assert!(
            out[0].message.contains("expected `\\chapter`"),
            "got: {}",
            out[0].message
        );
    }

    #[test]
    fn non_sectioning_commands_ignored() {
        assert!(findings("\\textbf{A}\n\\emph{B}\n\\label{c}\n").is_empty());
    }

    #[test]
    fn each_jump_flagged_independently() {
        // Two separate section subtrees, each skipping into subsubsection.
        let out = findings("\\section{A}\n\\subsubsection{B}\n\\section{C}\n\\subsubsection{D}\n");
        assert_eq!(out.len(), 2);
    }
}
