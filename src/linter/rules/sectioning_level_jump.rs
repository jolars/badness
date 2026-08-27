//! `sectioning-level-jump`: a heading that descends more than one sectioning
//! level below the preceding heading (`\section` straight to `\subsubsection`,
//! skipping `\subsection`). Modeled on textidote's `sh:secskip`.
//!
//! The structural ladder runs from `\part` through `\subsubsection`. `\chapter`
//! participates only for classes known to provide it, or once the source uses it;
//! chapterless and unknown classes conservatively place `\section` below `\part`.
//! `\paragraph` and `\subparagraph` are treated as transparent run-in labels,
//! matching their common use in contemporary technical papers. Descending the
//! outline should otherwise step one rung at a time. We flag only *downward* jumps
//! of more than one level between consecutive structural headings: climbing back
//! up (closing sections) is normal, and repeated headings at one level are fine.
//!
//! The comparison is purely *relative* to the immediately preceding structural
//! heading, never against an absolute top level. An `article` that opens with
//! `\section`, or uses `\part` followed by `\section`, is therefore valid. The
//! first structural heading in the document sets the baseline and is never flagged.
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

use crate::ast::{command_name, control_word_range, nth_group_text};
use crate::linter::diagnostic::{Diagnostic, Severity};
use crate::semantic::signature;
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};

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
        "Flag a structural heading that descends more than one level below the \
         preceding structural heading -- `\\section` straight to \
         `\\subsubsection`, skipping `\\subsection` (textidote's `sh:secskip`). \
         The active ladder follows the document class: `\\chapter` is included \
         only for classes known to provide it or when the source uses it, while \
         unknown classes conservatively omit it. `\\paragraph` and \
         `\\subparagraph` are transparent because technical papers commonly use \
         them as run-in labels rather than outline subdivisions. Only *downward* \
         jumps are flagged -- climbing back up and repeated headings at one level \
         are normal. The comparison is relative to the previous structural \
         heading, never an absolute top level. Report-only: repairing a skip is a \
         structural choice for the author, not a correct-by-construction edit."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    // Streaming rather than node-shape: the finding depends on the *sequence* of
    // headings in document order (the previous heading's level), which a stateless
    // per-element `check` cannot track. Rides the driver's one shared walk.
    fn stream(&self) -> Option<Box<dyn StreamVisitor>> {
        Some(Box::new(SectioningLevelJumpVisitor {
            prev_level: None,
            has_chapter: false,
        }))
    }
}

/// Tracks the level of the immediately preceding heading across the shared walk;
/// a heading deeper than `prev + 1` skipped at least one rung of the ladder.
struct SectioningLevelJumpVisitor {
    prev_level: Option<u8>,
    has_chapter: bool,
}

/// Whether a statically named document class is known to provide `\chapter`.
/// Unknown and article-like classes conservatively omit that rung: assuming an
/// unavailable heading would create a false positive that the source cannot
/// disprove. An encountered `\chapter` independently proves the rung exists.
fn class_has_chapter(name: &str) -> bool {
    matches!(
        name,
        "amsbook"
            | "book"
            | "extbook"
            | "extreport"
            | "memoir"
            | "report"
            | "scrbook"
            | "scrreprt"
            | "tufte-book"
    )
}

/// Rank a sectioning level in the active class ladder. Classes without
/// `\chapter` place `\section` directly below `\part`.
fn active_level(level: u8, has_chapter: bool) -> u8 {
    if !has_chapter && level > 1 {
        level - 1
    } else {
        level
    }
}

/// Whether the `COMMAND` node carries a `*` variant token (`\section*`): the star
/// lexes as a `WORD "*"` sibling right after the `CONTROL_WORD`, before any
/// argument group. A forward-bound comment run (`DOC_COMMENT`, decision #9) can
/// sit ahead of the control word, so it is skipped along with trivia; the first
/// significant element after the control word decides. Reads only static token
/// shape, no meaning.
fn is_starred(command: &SyntaxNode) -> bool {
    for child in command.children_with_tokens() {
        match child {
            SyntaxElement::Node(n) if n.kind() == SyntaxKind::DOC_COMMENT => continue,
            SyntaxElement::Node(_) => return false,
            SyntaxElement::Token(t) => match t.kind() {
                SyntaxKind::CONTROL_WORD
                | SyntaxKind::WHITESPACE
                | SyntaxKind::NEWLINE
                | SyntaxKind::COMMENT => continue,
                SyntaxKind::WORD if t.text() == "*" => return true,
                _ => return false,
            },
        }
    }
    false
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
        if name == "documentclass" {
            self.has_chapter =
                nth_group_text(node, 0).is_some_and(|class| class_has_chapter(class.trim()));
            return;
        }
        let Some(level) = signature::builtin()
            .command(&name)
            .and_then(|c| c.sectioning)
        else {
            return;
        };
        // A starred sectioning command (`\section*`, `\subsubsection*`) is
        // unnumbered and contributes nothing to the table of contents, so it is
        // outside the numbered outline: it can neither create a lopsided-ToC jump
        // nor set the baseline the next numbered heading is measured against. Skip
        // it entirely (dalcde/cam-notes: 23 false positives, all `\subsubsection*`).
        if is_starred(node) {
            return;
        }
        // In contemporary technical papers these are conventionally run-in
        // topic labels rather than outline subdivisions. Keep them transparent
        // so they neither trigger a jump nor hide one between structural headings.
        if matches!(name.as_str(), "paragraph" | "subparagraph") {
            return;
        }
        if name == "chapter" {
            self.has_chapter = true;
        }
        if let Some(prev) = self.prev_level
            && active_level(level, self.has_chapter) > active_level(prev, self.has_chapter) + 1
        {
            let expected_level = if !self.has_chapter && prev == 0 {
                2 // `\section` follows `\part` in chapterless classes.
            } else {
                prev + 1
            };
            let expected = LEVEL_NAMES[expected_level as usize];
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
    fn part_to_section_skips_chapter_in_book() {
        let out = findings("\\documentclass{book}\n\\part{A}\n\\section{B}\n");
        assert_eq!(out.len(), 1);
        assert!(
            out[0].message.contains("expected `\\chapter`"),
            "got: {}",
            out[0].message
        );
    }

    #[test]
    fn part_to_section_is_fine_in_article() {
        assert!(findings("\\documentclass{article}\n\\part{A}\n\\section{B}\n").is_empty());
    }

    #[test]
    fn unknown_class_does_not_assume_a_chapter_level() {
        assert!(findings("\\documentclass{custom}\n\\part{A}\n\\section{B}\n").is_empty());
    }

    #[test]
    fn an_encountered_chapter_proves_the_level_exists() {
        let out = findings("\\documentclass{custom}\n\\chapter{A}\n\\subsection{B}\n");
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains("expected `\\section`"));
    }

    #[test]
    fn part_to_subsection_skips_section_in_article() {
        let out = findings("\\documentclass{article}\n\\part{A}\n\\subsection{B}\n");
        assert_eq!(out.len(), 1);
        assert!(
            out[0].message.contains("expected `\\section`"),
            "got: {}",
            out[0].message
        );
    }

    #[test]
    fn paragraph_headings_are_transparent() {
        assert!(findings("\\section{A}\n\\paragraph{B}\n\\subparagraph{C}\n").is_empty());
    }

    #[test]
    fn paragraph_does_not_hide_a_later_jump() {
        let out = findings("\\section{A}\n\\paragraph{note}\n\\subsubsection{B}\n");
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains("expected `\\subsection`"));
    }

    #[test]
    fn non_sectioning_commands_ignored() {
        assert!(findings("\\textbf{A}\n\\emph{B}\n\\label{c}\n").is_empty());
    }

    #[test]
    fn starred_heading_is_not_flagged() {
        // A starred sectioning command is unnumbered and absent from the table of
        // contents, so descending to it cannot make the outline lopsided
        // (dalcde/cam-notes mixes numbered `\subsubsection` with unnumbered
        // `\subsubsection*` under a `\section`).
        assert!(findings("\\section{A}\n\\subsubsection*{B}\n").is_empty());
    }

    #[test]
    fn starred_heading_with_leading_comment_is_not_flagged() {
        // A run of own-line `%` comments binds forward into the command as a
        // `DOC_COMMENT`, so the star sits after that node, not first — the star
        // check must see past it (dalcde/cam-notes:
        // `%Here …\n\subsubsection*{Regime 2}`).
        let src = "\\section{A}\n%a comment\n%another\n\\subsubsection*{B}\n";
        assert!(findings(src).is_empty(), "got: {:?}", findings(src));
    }

    #[test]
    fn starred_heading_does_not_set_baseline() {
        // Out of the numbered hierarchy, a starred heading neither flags nor shifts
        // the baseline: the following numbered heading is still compared against the
        // last numbered one, so a genuine jump after it is still caught.
        let out = findings("\\section{A}\n\\subsubsection*{note}\n\\subsubsection{B}\n");
        assert_eq!(out.len(), 1, "got: {out:?}");
        assert!(out[0].message.contains("expected `\\subsection`"));
    }

    #[test]
    fn each_jump_flagged_independently() {
        // Two separate section subtrees, each skipping into subsubsection.
        let out = findings("\\section{A}\n\\subsubsection{B}\n\\section{C}\n\\subsubsection{D}\n");
        assert_eq!(out.len(), 2);
    }
}
