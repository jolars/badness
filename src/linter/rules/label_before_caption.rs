//! `label-before-caption`: a `\label` that precedes the `\caption` inside a float
//! environment, where it captures the *enclosing* counter instead of the float's.
//!
//! `\label` records whatever `\@currentlabel` holds, and inside a float that value
//! is set by `\caption` (which `\refstepcounter`s the `figure`/`table` counter).
//! A `\label` placed *before* the caption therefore stores whatever the last
//! `\refstepcounter` left behind — normally the enclosing section number — so
//! `\ref` prints a number that has nothing to do with the float it points at.
//! LaTeX issues no warning: the reference resolves, it is simply wrong.
//!
//! Scope is deliberately narrow, because a false positive here proposes moving
//! content the author placed on purpose:
//!
//! - **Only curated float environments** ([`OutlineKind::Float`] — `figure`,
//!   `table`, and their starred forms). The set is signature *data*, so widening
//!   it is a data change rather than a rule change. `\label` before `\item` in a
//!   list is the same underlying bug and is deliberately out of scope.
//! - **Only statement-level `\label`s** — those reachable from the float through
//!   `PARAGRAPH` nodes alone. A `\label` nested in a group or in a command's
//!   argument belongs to whatever that construct does: `\caption{Text\label{x}}`
//!   is the *recommended* idiom, and `\subcaptionbox{A\label{x}}{…}` labels the
//!   subfigure. Neither may be touched, and greedy argument attachment
//!   (AGENTS.md decision #8) makes "which command owns this group" too soft to
//!   lean on, so anything below statement level is skipped wholesale.
//! - **Any counter-stepping command before the label silences it**, at *any*
//!   depth: a `\caption`-family command or a hand-rolled
//!   `\refstepcounter`/`\stepcounter` may already have set `\@currentlabel`. That
//!   costs a true positive when the earlier caption belongs to a nested
//!   `subfigure` (the outer `\label` really is wrong there), which is the
//!   preferred direction — a miss over an invented finding.
//! - **A float with no caption at all is never flagged**: there is no counter to
//!   attach to and no place to move the label, so the shape is left alone.
//!
//! **Unsafe autofix**, when a target exists: delete the `\label` and re-insert it
//! immediately after the first statement-level `\caption`. It is `Unsafe` because
//! it changes typeset output by design — that is the whole point — and because the
//! author's intent is inferred, matching the sibling `\label`-placement rule
//! `space-before-command`. The insertion point is the *statement-level* caption
//! specifically: inserting after a nested `subfigure`'s caption would move the
//! label into that subfigure and relabel it.
//!
//! The fix owes correctness, never layout (AGENTS.md tenet 1). It re-inserts the
//! label glued to the caption (`\caption{…}\label{…}`) and leaves the line break
//! to the formatter — the two spellings are byte-identical to TeX, since a float
//! body is in vertical mode where the intervening space token is discarded. When
//! the `\label` sits alone on its line, the deletion takes the whole line so no
//! whitespace-only line is left behind (that would be a `\par`, not nothing).

use std::path::PathBuf;

use crate::ast::{AstNode, Environment, command_name};
use crate::linter::diagnostic::{Diagnostic, Fix, Severity};
use crate::semantic::signature::{self, OutlineKind};
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[Example {
    caption: "A `\\label` above its `\\caption` picks up the section counter, not the figure number:",
    source: "\\begin{figure}\n  \\includegraphics{plot}\n  \\label{fig:plot}\n  \\caption{A plot.}\n\\end{figure}\n",
}];

/// Commands that set `\@currentlabel` by typesetting a caption, so a `\label`
/// after one is correctly attached. Curated: an entry only ever *silences* a
/// finding, so a wrong one costs a miss rather than an invented diagnostic.
/// Starred forms are not distinguished — `\caption*` does not step the counter,
/// but treating it as though it does keeps the rule on the silent side.
const CAPTION_COMMANDS: &[&str] = &[
    "caption",
    "captionof",
    "captionlistentry",
    "phantomcaption",
    "subcaption",
    "subcaptionbox",
];

/// Commands that step a counter by hand. `\refstepcounter` sets `\@currentlabel`
/// (so a following `\label` is fine); `\stepcounter` does not, but is included
/// anyway for the same reason the starred captions are — silence over invention.
const COUNTER_STEPPERS: &[&str] = &["refstepcounter", "stepcounter"];

pub struct LabelBeforeCaption;

impl Rule for LabelBeforeCaption {
    fn id(&self) -> &'static str {
        "label-before-caption"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn emits_fix(&self) -> bool {
        true
    }

    fn description(&self) -> &'static str {
        "Flag a `\\label` placed before the `\\caption` inside a float \
         (`figure`, `table`, and their starred forms). `\\label` records \
         `\\@currentlabel`, which inside a float is set by `\\caption`; a label \
         above the caption therefore captures whatever the last `\\refstepcounter` \
         left behind — usually the enclosing section number — so `\\ref` silently \
         prints a number unrelated to the float. LaTeX gives no warning. Scoped to \
         statement-level labels, so the recommended `\\caption{Text\\label{x}}` \
         idiom and a `\\subcaptionbox{A\\label{x}}{…}` subfigure label are never \
         touched; any earlier caption or hand-rolled \
         `\\refstepcounter`/`\\stepcounter` also silences it, and a float with no \
         caption is left alone. The fix moves the label to just after the first \
         statement-level `\\caption`, and is Unsafe because it changes what `\\ref` \
         prints (by design) from an inferred intent."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::ENVIRONMENT]
    }

    fn check(&self, el: &SyntaxElement, _ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(float) = el.as_node() else {
            return;
        };
        let Some(name) = float_name(float) else {
            return;
        };

        // Detection cutoff: the first counter-stepping command anywhere in the
        // float. Nothing at or after it can be diagnosed, since `\@currentlabel`
        // may legitimately already name this float (or a nested one).
        let Some(cutoff) = first_stepper(float) else {
            return; // no caption and no manual step: nothing to attach to.
        };

        // Insertion target: the first *statement-level* caption. A nested
        // subfigure's caption is not a legal destination — moving the label there
        // would relabel the subfigure — so its absence means report-only.
        let target = statement_level_captions(float).next();

        for label in float.descendants() {
            if label.kind() != SyntaxKind::COMMAND {
                continue;
            }
            if command_name(&label).as_deref() != Some("label") {
                continue;
            }
            let start = usize::from(label.text_range().start());
            if start >= cutoff {
                continue;
            }
            if !at_statement_level(&label, float) {
                continue;
            }
            let fix = target
                .as_ref()
                .and_then(|caption| build_fix(&label, caption));
            sink.push(Diagnostic {
                rule: self.id(),
                severity: self.default_severity(),
                path: PathBuf::new(),
                start,
                end: usize::from(label.text_range().end()),
                message: format!(
                    "`\\label` before `\\caption` in this `{name}` captures the enclosing \
                     counter, not the float number"
                ),
                fix,
                related: Vec::new(),
            });
        }
    }
}

/// The environment's name when it is a curated float, else `None`. Reads the
/// curated tier's outline category only (like the LSP's symbol routing): the CWL
/// tier carries no float classification, and inventing one would put findings on
/// environments we cannot vouch for.
fn float_name(env: &SyntaxNode) -> Option<String> {
    let name = Environment::cast(env.clone())
        .and_then(|e| e.begin())
        .and_then(|begin| begin.name())?;
    signature::builtin()
        .environment(&name)
        .filter(|sig| sig.outline == Some(OutlineKind::Float))
        .map(|_| name)
}

/// Whether `node` is reachable from `float` through `PARAGRAPH` nodes alone —
/// the float's own statement level. Anything separated by a `GROUP`, an
/// `OPTIONAL`, or a nested `ENVIRONMENT` is owned by that construct instead.
fn at_statement_level(node: &SyntaxNode, float: &SyntaxNode) -> bool {
    let mut cursor = node.parent();
    while let Some(current) = cursor {
        if &current == float {
            return true;
        }
        if current.kind() != SyntaxKind::PARAGRAPH {
            return false;
        }
        cursor = current.parent();
    }
    false
}

/// The start offset of the first counter-stepping command anywhere inside
/// `float`. `descendants()` is preorder, so the first match is also the earliest.
fn first_stepper(float: &SyntaxNode) -> Option<usize> {
    float
        .descendants()
        .filter(|node| node.kind() == SyntaxKind::COMMAND)
        .find(|node| {
            command_name(node).is_some_and(|name| {
                let bare = name.strip_suffix('*').unwrap_or(&name);
                CAPTION_COMMANDS.contains(&bare) || COUNTER_STEPPERS.contains(&bare)
            })
        })
        .map(|node| usize::from(node.text_range().start()))
}

/// The float's statement-level caption commands, in document order.
fn statement_level_captions(float: &SyntaxNode) -> impl Iterator<Item = SyntaxNode> + '_ {
    float.descendants().filter(move |node| {
        node.kind() == SyntaxKind::COMMAND
            && command_name(node).is_some_and(|name| {
                CAPTION_COMMANDS.contains(&name.strip_suffix('*').unwrap_or(&name))
            })
            && at_statement_level(node, float)
    })
}

/// The two-edit move: delete the `\label` where it stands, re-insert it directly
/// after `caption`. Returns `None` when the caption does not follow the label
/// (nothing to move it past) or the spans would overlap.
fn build_fix(label: &SyntaxNode, caption: &SyntaxNode) -> Option<Fix> {
    let text = label.text().to_string();
    let (start, end) = removal_span(label);
    let insert_at = usize::from(caption.text_range().end());
    // The caption must sit strictly after the deleted region, or the two edits
    // are not disjoint and the move is not expressible.
    if insert_at <= end {
        return None;
    }
    Some(Fix::unsafe_edits(
        vec![
            crate::linter::diagnostic::Edit::new(start, end, ""),
            crate::linter::diagnostic::Edit::new(insert_at, insert_at, text),
        ],
        "move `\\label` after `\\caption`",
    ))
}

/// The bytes to delete when lifting `label` out. When the label sits alone on its
/// line, that is the whole line (indent and terminating newline included) — a
/// bare node deletion would leave a whitespace-only line, which is a paragraph
/// break rather than nothing. Otherwise it is exactly the node.
fn removal_span(label: &SyntaxNode) -> (usize, usize) {
    let node_span = (
        usize::from(label.text_range().start()),
        usize::from(label.text_range().end()),
    );
    let (Some(first), Some(last)) = (label.first_token(), label.last_token()) else {
        return node_span;
    };
    // Backward: only indentation may precede the label on its line. Record where
    // the line's content starts, and where its leading newline starts (the
    // fallback when the label ends the file).
    let (line_start, newline_start) = match line_head(&first) {
        Some(pair) => pair,
        None => return node_span,
    };
    // Forward: only trailing spaces, then the line's newline.
    let mut cursor = last.next_token();
    let mut scanned_end = usize::from(last.text_range().end());
    loop {
        match cursor {
            Some(token) if token.kind() == SyntaxKind::WHITESPACE => {
                scanned_end = usize::from(token.text_range().end());
                cursor = token.next_token();
            }
            Some(token) if token.kind() == SyntaxKind::NEWLINE => {
                return (line_start, usize::from(token.text_range().end()));
            }
            // End of file: no newline to consume, so take the *preceding* one
            // instead, again leaving no blank line behind.
            None => return (newline_start, scanned_end),
            _ => return node_span,
        }
    }
}

/// `(content start, preceding newline start)` for the line `first` opens, or
/// `None` when something other than indentation precedes it on that line.
fn line_head(first: &SyntaxToken) -> Option<(usize, usize)> {
    let mut cursor = first.prev_token();
    while let Some(token) = cursor {
        match token.kind() {
            SyntaxKind::WHITESPACE => cursor = token.prev_token(),
            SyntaxKind::NEWLINE => {
                let range = token.text_range();
                return Some((usize::from(range.end()), usize::from(range.start())));
            }
            _ => return None,
        }
    }
    // Start of file: the line begins at byte 0 and has no preceding newline.
    Some((0, 0))
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
        for el in root.descendants_with_tokens() {
            if LabelBeforeCaption.interests().contains(&el.kind()) {
                LabelBeforeCaption.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    fn fixed(src: &str) -> String {
        let out = findings(src);
        let fix = out[0].fix.as_ref().expect("expected a fix");
        crate::linter::fix::apply_fixes(src, std::slice::from_ref(fix), true).output
    }

    #[test]
    fn flags_label_above_caption_in_a_figure() {
        let src = "\\begin{figure}\n  \\label{fig:x}\n  \\caption{Cap}\n\\end{figure}\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "label-before-caption");
        assert_eq!(out[0].severity, Severity::Warning);
        // Span is exactly the offending command, not the line or the float.
        assert_eq!(&src[out[0].start..out[0].end], "\\label{fig:x}");
    }

    #[test]
    fn flags_in_a_table_and_starred_forms() {
        for env in ["table", "figure*", "table*"] {
            let src =
                format!("\\begin{{{env}}}\n  \\label{{a}}\n  \\caption{{C}}\n\\end{{{env}}}\n");
            assert_eq!(findings(&src).len(), 1, "{env}");
        }
    }

    #[test]
    fn silent_when_label_follows_caption() {
        let src = "\\begin{figure}\n  \\caption{Cap}\n  \\label{fig:x}\n\\end{figure}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn silent_outside_a_float() {
        // `center` is not a float: no caption counter is involved.
        let src = "\\begin{center}\n  \\label{a}\n  \\caption{C}\n\\end{center}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn silent_when_the_float_has_no_caption() {
        let src = "\\begin{figure}\n  \\includegraphics{a}\n  \\label{fig:x}\n\\end{figure}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn silent_on_a_label_inside_the_caption_argument() {
        // The recommended idiom: the label rides inside the caption's argument.
        let src = "\\begin{figure}\n  \\caption{Cap\\label{fig:x}}\n\\end{figure}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn silent_on_a_label_in_a_command_argument_before_the_caption() {
        // `\subcaptionbox` labels the subfigure from inside its own argument;
        // moving that label would break it.
        let src = "\\begin{figure}\n  \\subcaptionbox{A\\label{sub:a}}{\\includegraphics{a}}\n  \
                   \\caption{Main}\n\\end{figure}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn silent_on_a_subfigure_label_that_follows_its_own_caption() {
        let src = "\\begin{figure}\n  \\begin{subfigure}{b}\n    \\caption{a}\n    \
                   \\label{sub:a}\n  \\end{subfigure}\n  \\caption{Main}\n  \\label{fig:m}\n\
                   \\end{figure}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn silent_after_a_manual_refstepcounter() {
        let src = "\\begin{figure}\n  \\refstepcounter{figure}\n  \\label{fig:x}\n  \
                   \\caption{Cap}\n\\end{figure}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn flags_each_offending_label() {
        let src = "\\begin{figure}\n  \\label{a}\n  \\label{b}\n  \\caption{C}\n\\end{figure}\n";
        assert_eq!(findings(src).len(), 2);
    }

    #[test]
    fn fix_moves_the_label_after_the_caption_and_removes_the_line() {
        let src = "\\begin{figure}\n  \\includegraphics{a}\n  \\label{fig:x}\n  \
                   \\caption{Cap}\n\\end{figure}\n";
        let out = findings(src);
        assert_eq!(
            out[0].fix.as_ref().unwrap().applicability,
            crate::linter::diagnostic::Applicability::Unsafe
        );
        assert_eq!(
            fixed(src),
            "\\begin{figure}\n  \\includegraphics{a}\n  \\caption{Cap}\\label{fig:x}\n\\end{figure}\n"
        );
    }

    #[test]
    fn fix_leaves_no_blank_line_behind() {
        let src = "\\begin{figure}\n  \\label{a}\n  \\caption{C}\n\\end{figure}\n";
        let out = fixed(src);
        assert!(
            !out.contains("\n  \n") && !out.contains("\n\n"),
            "blank line left behind: {out:?}"
        );
    }

    #[test]
    fn fix_swaps_an_inline_label_without_touching_the_line() {
        let src = "\\begin{figure}\\label{a}\\caption{C}\\end{figure}\n";
        assert_eq!(
            fixed(src),
            "\\begin{figure}\\caption{C}\\label{a}\\end{figure}\n"
        );
    }

    #[test]
    fn fix_keeps_other_content_on_the_label_line() {
        // Not alone on its line: only the node is lifted, the neighbor stays put.
        let src = "\\begin{figure}\n  x \\label{a} y\n  \\caption{C}\n\\end{figure}\n";
        assert_eq!(
            fixed(src),
            "\\begin{figure}\n  x  y\n  \\caption{C}\\label{a}\n\\end{figure}\n"
        );
    }

    #[test]
    fn reports_without_a_fix_when_only_a_nested_caption_exists() {
        // The only caption belongs to the subfigure, so there is no legal
        // destination; the finding still surfaces.
        let src = "\\begin{figure}\n  \\label{fig:m}\n  \\begin{subfigure}{b}\n    \
                   \\caption{a}\n  \\end{subfigure}\n\\end{figure}\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert!(out[0].fix.is_none());
    }
}
