//! `label-before-caption`: a `\label` that precedes the statement which establishes
//! its intended counter—a float's `\caption` or an `enumerate` list's first `\item`.
//!
//! `\label` records whatever `\@currentlabel` holds, and inside a float that value
//! is set by `\caption` (which `\refstepcounter`s the `figure`/`table` counter).
//! A `\label` placed *before* the caption therefore stores whatever the last
//! `\refstepcounter` left behind—normally the enclosing section number—so
//! `\ref` prints a number that has nothing to do with the float it points at.
//! The same applies before the first `\item` in `enumerate`, which has not stepped
//! the item counter yet. LaTeX issues no warning: the reference resolves, it is
//! simply wrong.
//!
//! Scope is deliberately narrow, because a false positive here proposes moving
//! content the author placed on purpose:
//!
//! - **Only curated float environments** ([`OutlineKind::Float`]—`figure`,
//!   `table`, and their starred forms). The set is signature *data*, so widening
//!   it is a data change rather than a rule change.
//! - **Only the standard numbered `enumerate` list, and only before its first
//!   statement-level `\item`.** A label after any item may legitimately belong to
//!   that preceding item, even when another item follows. `itemize` and
//!   `description` are excluded because their items do not step a reference
//!   counter. An attached custom `[label]` and a complete Beamer overlay suffix
//!   remain part of the first item's marker.
//! - **Only statement-level `\label`s**—those reachable from the float or list
//!   through `PARAGRAPH` nodes alone. A `\label` nested in a group or in a
//!   command's argument belongs to whatever that construct does:
//!   `\caption{Text\label{x}}` is the *recommended* idiom, and
//!   `\subcaptionbox{A\label{x}}{…}` labels the subfigure. Neither may be touched,
//!   and greedy argument attachment makes "which command owns this group" too
//!   soft to lean on, so anything below statement level is skipped wholesale.
//! - **Counter steps are classified against the outer float.** A nested
//!   `subfigure`/`subtable` caption and `\subcaption`-family commands step a
//!   sub-counter, so they do not silence an outer label. Dynamic counter names,
//!   unrecognized nested scopes, starred captions, and `\stepcounter` remain
//!   conservative barriers: uncertainty costs a miss, not an invented finding.
//! - **A float with no caption or counter step is never flagged**: there is no
//!   evidence of a numbered target, so the shape is left alone.
//!
//! **Unsafe autofix**, when a target exists: delete the `\label` and re-insert it
//! immediately after the first statement-level outer `\caption` or first
//! statement-level `\item` marker. It is `Unsafe` because it changes typeset
//! output by design—that is the whole point—and because the author's intent
//! is inferred, matching the sibling `\label`-placement rule
//! `space-before-command`. The float insertion point is the *statement-level*
//! caption specifically: inserting after a nested `subfigure`'s caption would
//! move the label into that subfigure and relabel it.
//!
//! The fix owes correctness, never layout (AGENTS.md tenet 1). It re-inserts the
//! label glued to its target and leaves the line break to the formatter. In a
//! float, `\caption{…}\label{…}` is byte-equivalent to a separated spelling
//! because the body is in vertical mode. In a list,
//! `\item[marker]\label{…}` begins the item body immediately after `\item` has
//! stepped the counter. When the `\label` sits alone on its line, the deletion
//! takes the whole line so no whitespace-only line is left behind (that would be
//! a `\par`, not nothing).

use std::path::PathBuf;

use crate::ast::{AstNode, Environment, command_name, nth_group_text};
use crate::linter::diagnostic::{Diagnostic, Fix, Severity};
use crate::semantic::signature::{self, OutlineKind};
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken};

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[
    Example {
        caption: "A `\\label` above its `\\caption` picks up the section counter, not the figure number:",
        source: "\\begin{figure}\n  \\includegraphics{plot}\n  \\label{fig:plot}\n  \\caption{A plot.}\n\\end{figure}\n",
    },
    Example {
        caption: "A `\\label` before the first `\\item` has not seen the item counter step:",
        source: "\\begin{enumerate}\n  \\label{item:first}\n  \\item First\n\\end{enumerate}\n",
    },
];

/// Commands that may set `\@currentlabel` by typesetting a caption.
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
/// anyway for the same reason the starred captions are—silence over invention.
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
        "Flag a `\\label` placed before the statement that establishes its \
         intended counter: the outer `\\caption` in a curated float (`figure`, \
         `table`, and their starred forms), or the first `\\item` in the standard \
         numbered `enumerate` list. In either position, `\\label` captures the \
         previous `\\@currentlabel`—usually an enclosing section number—so \
         `\\ref` silently prints an unrelated number. LaTeX gives no warning. \
         The list case is limited to statement-level labels before the first item; \
         labels after an item may belong to it, while `itemize` and `description` \
         items do not step a reference counter. Attached custom item labels and \
         complete Beamer overlay markers remain intact. The float case likewise \
         skips labels nested in command arguments, and classifies nested counter \
         steps conservatively. The fix moves the label just after the proven \
         caption or item marker, and is Unsafe because it intentionally changes \
         what `\\ref` prints from an inferred intent."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::ENVIRONMENT]
    }

    fn check(&self, el: &SyntaxElement, _ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(env) = el.as_node() else {
            return;
        };

        if let Some(name) = float_name(env) {
            self.check_float(env, &name, sink);
            return;
        }
        if is_numbered_list(env) {
            self.check_numbered_list(env, sink);
        }
    }
}

impl LabelBeforeCaption {
    fn check_float(&self, float: &SyntaxNode, name: &str, sink: &mut Vec<Diagnostic>) {
        let outer_counter = name.strip_suffix('*').unwrap_or(name);

        // Detection cutoff: the first command that may have stepped this float's
        // own counter. Proven sub-counter steps do not silence an outer label.
        let Some(cutoff) = outer_counter_cutoff(float, outer_counter) else {
            return; // no caption and no manual step: nothing to attach to.
        };

        // Only a statement-level caption proven to step the outer counter is a
        // legal destination for the unsafe move.
        let target = outer_caption_targets(float, outer_counter).next();

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
                    "`\\label` before the outer `\\caption` in this `{name}` does not \
                     capture the float number"
                ),
                fix,
                related: Vec::new(),
            });
        }
    }

    fn check_numbered_list(&self, list: &SyntaxNode, sink: &mut Vec<Diagnostic>) {
        let Some(item) = first_statement_item(list) else {
            return;
        };
        let cutoff = usize::from(item.text_range().start());
        let target = item_marker_end(&item);

        for label in list.descendants() {
            if label.kind() != SyntaxKind::COMMAND
                || command_name(&label).as_deref() != Some("label")
            {
                continue;
            }
            let start = usize::from(label.text_range().start());
            if start >= cutoff || !at_statement_level(&label, list) {
                continue;
            }
            let fix = target.and_then(|insert_at| {
                build_move_fix(&label, insert_at, "move `\\label` after the first `\\item`")
            });
            sink.push(Diagnostic {
                rule: self.id(),
                severity: self.default_severity(),
                path: PathBuf::new(),
                start,
                end: usize::from(label.text_range().end()),
                message: "`\\label` before the first `\\item` in this `enumerate` does not \
                          capture the item number"
                    .to_owned(),
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

/// Whether `env` is the standard numbered list. The curated `list` flag alone
/// is too broad: `itemize` and `description` items do not step a reference
/// counter, so moving a label past them would not repair anything.
fn is_numbered_list(env: &SyntaxNode) -> bool {
    let name = Environment::cast(env.clone())
        .and_then(|e| e.begin())
        .and_then(|begin| begin.name());
    name.as_deref() == Some("enumerate")
        && signature::builtin()
            .environment("enumerate")
            .is_some_and(|sig| sig.list)
}

/// Whether `node` is reachable from `container` through `PARAGRAPH` nodes
/// alone—the container's own statement level. Anything separated by a `GROUP`, an
/// `OPTIONAL`, or a nested `ENVIRONMENT` is owned by that construct instead.
fn at_statement_level(node: &SyntaxNode, container: &SyntaxNode) -> bool {
    let mut cursor = node.parent();
    while let Some(current) = cursor {
        if &current == container {
            return true;
        }
        if current.kind() != SyntaxKind::PARAGRAPH {
            return false;
        }
        cursor = current.parent();
    }
    false
}

/// The first outer `\item` in `list`. Nested lists and command arguments are not
/// item boundaries for the enclosing list.
fn first_statement_item(list: &SyntaxNode) -> Option<SyntaxNode> {
    list.descendants().find(|node| {
        node.kind() == SyntaxKind::COMMAND
            && command_name(node).as_deref() == Some("item")
            && at_statement_level(node, list)
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CounterEffect {
    Outer,
    Other,
    Unknown,
}

/// The first command that may establish the outer float's label, or the end of
/// the float when it contains only proven sub-counter steps. The latter keeps
/// report-only findings for outer labels in caption-less floats that nevertheless
/// contain a nested caption.
fn outer_counter_cutoff(float: &SyntaxNode, outer_counter: &str) -> Option<usize> {
    let mut saw_other = false;
    for command in float
        .descendants()
        .filter(|node| node.kind() == SyntaxKind::COMMAND)
    {
        match counter_effect(&command, float, outer_counter) {
            Some(CounterEffect::Outer | CounterEffect::Unknown) => {
                return Some(usize::from(command.text_range().start()));
            }
            Some(CounterEffect::Other) => saw_other = true,
            None => {}
        }
    }
    saw_other.then(|| usize::from(float.text_range().end()))
}

/// Statement-level captions proven to step the outer float's counter.
fn outer_caption_targets<'a>(
    float: &'a SyntaxNode,
    outer_counter: &'a str,
) -> impl Iterator<Item = SyntaxNode> + 'a {
    float.descendants().filter(move |node| {
        node.kind() == SyntaxKind::COMMAND
            && command_name(node).is_some_and(|name| {
                CAPTION_COMMANDS.contains(&name.strip_suffix('*').unwrap_or(&name))
            })
            && at_statement_level(node, float)
            && counter_effect(node, float, outer_counter) == Some(CounterEffect::Outer)
    })
}

/// Classify a command's effect on `\@currentlabel` relative to the outer float.
/// Unknown effects are deliberately barriers, preserving the rule's silent-side
/// bias for syntax or caption scopes that cannot be proved from source shape.
fn counter_effect(
    command: &SyntaxNode,
    float: &SyntaxNode,
    outer_counter: &str,
) -> Option<CounterEffect> {
    let name = command_name(command)?;
    let bare = name.strip_suffix('*').unwrap_or(&name);
    if !CAPTION_COMMANDS.contains(&bare) && !COUNTER_STEPPERS.contains(&bare) {
        return None;
    }
    if name.ends_with('*') || bare == "stepcounter" {
        return Some(CounterEffect::Unknown);
    }

    match bare {
        "subcaption" | "subcaptionbox" => Some(CounterEffect::Other),
        "captionof" | "refstepcounter" => Some(counter_argument_effect(command, outer_counter)),
        "caption" | "captionlistentry" | "phantomcaption" => {
            Some(caption_scope_effect(command, float))
        }
        _ => Some(CounterEffect::Unknown),
    }
}

fn counter_argument_effect(command: &SyntaxNode, outer_counter: &str) -> CounterEffect {
    match nth_group_text(command, 0) {
        Some(counter) if counter.trim() == outer_counter => CounterEffect::Outer,
        Some(_) => CounterEffect::Other,
        None => CounterEffect::Unknown,
    }
}

/// Plain caption commands use the current caption scope. Only the float's own
/// statement level and the two standard subcaption environments are modeled;
/// every other nested owner remains an unknown barrier.
fn caption_scope_effect(command: &SyntaxNode, float: &SyntaxNode) -> CounterEffect {
    let mut cursor = command.parent();
    while let Some(current) = cursor {
        if &current == float {
            return CounterEffect::Outer;
        }
        match current.kind() {
            SyntaxKind::PARAGRAPH => {}
            SyntaxKind::ENVIRONMENT => {
                let name = Environment::cast(current.clone())
                    .and_then(|env| env.begin())
                    .and_then(|begin| begin.name());
                return match name.as_deref() {
                    Some("subfigure" | "subtable") => CounterEffect::Other,
                    _ => CounterEffect::Unknown,
                };
            }
            _ => return CounterEffect::Unknown,
        }
        cursor = current.parent();
    }
    CounterEffect::Unknown
}

/// The byte immediately after the complete first-item marker. Besides the
/// command's attached `[label]`, this recognizes Beamer's bounded
/// `\item<overlay>[label]<overlay>` spelling. An incomplete suffix withholds the
/// fix while leaving the diagnostic intact.
fn item_marker_end(item: &SyntaxNode) -> Option<usize> {
    let (mut end, has_attached_body) = item_command_marker_end(item)?;
    if has_attached_body {
        return Some(end);
    }

    let parent = item.parent()?;
    let siblings: Vec<SyntaxElement> = parent.children_with_tokens().collect();
    let mut index = siblings
        .iter()
        .position(|element| element.as_node() == Some(item))?
        + 1;

    if let Some((next, suffix_end)) = angle_marker_suffix(&siblings, index).ok()? {
        index = next;
        end = suffix_end;
    }
    if let Some((next, suffix_end)) = bracket_marker_suffix(&siblings, index).ok()? {
        index = next;
        end = suffix_end;
    }
    if let Some((_, suffix_end)) = angle_marker_suffix(&siblings, index).ok()? {
        end = suffix_end;
    }
    Some(end)
}

/// The attached portion of an item marker and whether the greedy command parse
/// also captured body content. In the latter case the insertion point is inside
/// the `COMMAND`, immediately before that content.
fn item_command_marker_end(item: &SyntaxNode) -> Option<(usize, bool)> {
    let mut end = None;
    let mut saw_control_word = false;
    for child in item.children_with_tokens() {
        match child.kind() {
            SyntaxKind::DOC_COMMENT => {}
            SyntaxKind::CONTROL_WORD => {
                saw_control_word = true;
                end = Some(usize::from(child.text_range().end()));
            }
            SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE if saw_control_word => {}
            SyntaxKind::OPTIONAL if saw_control_word => {
                end = Some(usize::from(child.text_range().end()));
            }
            _ if saw_control_word => return end.map(|end| (end, true)),
            _ => {}
        }
    }
    end.map(|end| (end, false))
}

/// A complete `<...>` suffix starting at `start`. `Err` means
/// a would-be suffix or comment could not be proved safe to cross.
fn angle_marker_suffix(
    elements: &[SyntaxElement],
    start: usize,
) -> Result<Option<(usize, usize)>, ()> {
    let mut index = skip_marker_trivia(elements, start);
    let first = match elements.get(index) {
        Some(first) if first.kind() == SyntaxKind::COMMENT => return Err(()),
        Some(first) => element_text(first),
        None => return Ok(None),
    };
    if !first.starts_with('<') {
        return Ok(None);
    }

    loop {
        let element = elements.get(index).ok_or(())?;
        if element.kind() == SyntaxKind::COMMENT {
            return Err(());
        }
        let closes = !matches!(element.kind(), SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE)
            && element_text(element).ends_with('>');
        index += 1;
        if closes {
            return Ok(Some((index, usize::from(element.text_range().end()))));
        }
    }
}

/// A complete raw `[...]` suffix after a Beamer overlay. Ordinarily the parser
/// attaches an item's optional label to the `COMMAND`; the raw form is what
/// remains when an angle suffix intervenes.
fn bracket_marker_suffix(
    elements: &[SyntaxElement],
    start: usize,
) -> Result<Option<(usize, usize)>, ()> {
    let mut index = skip_marker_trivia(elements, start);
    let first = match elements.get(index) {
        Some(first) if first.kind() == SyntaxKind::COMMENT => return Err(()),
        Some(first) => first,
        None => return Ok(None),
    };
    if first.kind() == SyntaxKind::OPTIONAL {
        return Ok(Some((index + 1, usize::from(first.text_range().end()))));
    }
    if first.kind() != SyntaxKind::L_BRACKET {
        return Ok(None);
    }

    let mut depth = 0usize;
    loop {
        let element = elements.get(index).ok_or(())?;
        match element.kind() {
            SyntaxKind::L_BRACKET => depth += 1,
            SyntaxKind::R_BRACKET => depth = depth.checked_sub(1).ok_or(())?,
            SyntaxKind::COMMENT => return Err(()),
            _ => {}
        }
        index += 1;
        if depth == 0 {
            return Ok(Some((index, usize::from(element.text_range().end()))));
        }
    }
}

fn skip_marker_trivia(elements: &[SyntaxElement], mut index: usize) -> usize {
    while elements.get(index).is_some_and(|element| {
        matches!(element.kind(), SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE)
    }) {
        index += 1;
    }
    index
}

fn element_text(element: &SyntaxElement) -> String {
    match element {
        SyntaxElement::Node(node) => node.text().to_string(),
        SyntaxElement::Token(token) => token.text().to_owned(),
    }
}

/// The two-edit move: delete the `\label` where it stands, re-insert it directly
/// after `caption`. Returns `None` when the caption does not follow the label
/// (nothing to move it past) or the spans would overlap.
fn build_fix(label: &SyntaxNode, caption: &SyntaxNode) -> Option<Fix> {
    let insert_at = usize::from(caption.text_range().end());
    build_move_fix(label, insert_at, "move `\\label` after `\\caption`")
}

fn build_move_fix(label: &SyntaxNode, insert_at: usize, description: &str) -> Option<Fix> {
    let text = label.text().to_string();
    let (start, end) = removal_span(label);
    // The destination must sit strictly after the deleted region, or the two
    // edits are not disjoint and the move is not expressible.
    if insert_at <= end {
        return None;
    }
    Some(Fix::unsafe_edits(
        vec![
            crate::linter::diagnostic::Edit::new(start, end, ""),
            crate::linter::diagnostic::Edit::new(insert_at, insert_at, text),
        ],
        description,
    ))
}

/// The bytes to delete when lifting `label` out. When the label sits alone on its
/// line, that is the whole line (indent and terminating newline included)—a
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
    fn flags_a_statement_level_label_before_the_first_enumerate_item() {
        let src = "\\begin{enumerate}\n  \\label{item:first}\n  \\item First\n\\end{enumerate}\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(&src[out[0].start..out[0].end], "\\label{item:first}");
        assert!(out[0].message.contains("`\\item`"));
    }

    #[test]
    fn list_gate_is_silent_after_an_item_and_below_statement_level() {
        for src in [
            "\\begin{enumerate}\n  \\item First\n  \\label{item:first}\n  \\item Second\n\\end{enumerate}\n",
            "\\begin{enumerate}\n  {\\label{item:first}}\n  \\item First\n\\end{enumerate}\n",
            "\\begin{enumerate}\n  \\textbf{\\label{item:first}}\n  \\item First\n\\end{enumerate}\n",
        ] {
            assert!(findings(src).is_empty(), "{src}");
        }
    }

    #[test]
    fn list_gate_is_silent_for_non_numbered_lists() {
        for env in ["itemize", "description"] {
            let src = format!(
                "\\begin{{{env}}}\n  \\label{{item:first}}\n  \\item[Term] First\n\\end{{{env}}}\n"
            );
            assert!(findings(&src).is_empty(), "{env}");
        }
    }

    #[test]
    fn nested_items_do_not_supply_an_outer_list_target() {
        let src = "\\begin{enumerate}\n  \\label{outer}\n  \\begin{enumerate}\n    \\item Inner\n  \\end{enumerate}\n\\end{enumerate}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn list_fix_moves_the_label_after_the_complete_item_marker() {
        for (src, expected) in [
            (
                "\\begin{enumerate}\n  \\label{item:first}\n  \\item[(a)] First\n\\end{enumerate}\n",
                "\\begin{enumerate}\n  \\item[(a)]\\label{item:first} First\n\\end{enumerate}\n",
            ),
            (
                "\\begin{enumerate}\n  \\label{item:first}\n  \\item<2->[custom]<3-> First\n\\end{enumerate}\n",
                "\\begin{enumerate}\n  \\item<2->[custom]<3->\\label{item:first} First\n\\end{enumerate}\n",
            ),
            (
                "\\begin{enumerate}\n  \\label{item:first}\n  \\item {First}\n\\end{enumerate}\n",
                "\\begin{enumerate}\n  \\item\\label{item:first} {First}\n\\end{enumerate}\n",
            ),
        ] {
            assert_eq!(fixed(src), expected);
        }
    }

    #[test]
    fn incomplete_item_overlay_is_reported_without_a_fix() {
        let src =
            "\\begin{enumerate}\n  \\label{item:first}\n  \\item<2- First\n\\end{enumerate}\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert!(out[0].fix.is_none());
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
    fn flags_outer_label_after_a_nested_subcaption() {
        for (outer, inner) in [("figure", "subfigure"), ("table", "subtable")] {
            let src = format!(
                "\\begin{{{outer}}}\n  \\begin{{{inner}}}{{b}}\n    \\caption{{Sub}}\n  \\end{{{inner}}}\n  \\label{{outer:x}}\n  \\caption{{Outer}}\n\\end{{{outer}}}\n"
            );
            let out = findings(&src);
            assert_eq!(out.len(), 1, "{outer}/{inner}");
            assert_eq!(&src[out[0].start..out[0].end], "\\label{outer:x}");
        }
    }

    #[test]
    fn fix_targets_the_outer_caption_after_a_nested_subcaption() {
        let src = "\\begin{figure}\n  \\begin{subfigure}{b}\n    \\caption{Sub}\n  \\end{subfigure}\n  \\label{fig:x}\n  \\caption{Outer}\n\\end{figure}\n";
        assert_eq!(
            fixed(src),
            "\\begin{figure}\n  \\begin{subfigure}{b}\n    \\caption{Sub}\n  \\end{subfigure}\n  \\caption{Outer}\\label{fig:x}\n\\end{figure}\n"
        );
    }

    #[test]
    fn subcaptionbox_does_not_hide_an_outer_label_or_receive_its_fix() {
        for src in [
            "\\begin{figure}\n  \\subcaptionbox{Sub}{x}\n  \\label{fig:x}\n  \\caption{Outer}\n\\end{figure}\n",
            "\\begin{figure}\n  \\label{fig:x}\n  \\subcaptionbox{Sub}{x}\n  \\caption{Outer}\n\\end{figure}\n",
        ] {
            assert_eq!(
                fixed(src),
                "\\begin{figure}\n  \\subcaptionbox{Sub}{x}\n  \\caption{Outer}\\label{fig:x}\n\\end{figure}\n"
            );
        }
    }

    #[test]
    fn silent_after_a_manual_refstepcounter() {
        for env in ["figure", "figure*"] {
            let src = format!(
                "\\begin{{{env}}}\n  \\refstepcounter{{figure}}\n  \\label{{fig:x}}\n  \\caption{{Cap}}\n\\end{{{env}}}\n"
            );
            assert!(findings(&src).is_empty(), "{env}");
        }
    }

    #[test]
    fn a_different_manual_counter_does_not_hide_the_finding() {
        let src = "\\begin{figure}\n  \\refstepcounter{subfigure}\n  \\label{fig:x}\n  \
                   \\caption{Cap}\n\\end{figure}\n";
        assert_eq!(findings(src).len(), 1);
    }

    #[test]
    fn an_unknown_manual_counter_remains_a_conservative_barrier() {
        let src = "\\begin{figure}\n  \\refstepcounter{\\countername}\n  \\label{fig:x}\n  \
                   \\caption{Cap}\n\\end{figure}\n";
        assert!(findings(src).is_empty());
    }

    #[test]
    fn captionof_is_matched_against_the_outer_counter() {
        let matching = "\\begin{figure}\n  \\captionof{figure}{Earlier}\n  \\label{fig:x}\n  \
                        \\caption{Later}\n\\end{figure}\n";
        assert!(findings(matching).is_empty());

        let different = "\\begin{figure}\n  \\captionof{table}{Table}\n  \\label{fig:x}\n  \
                         \\caption{Figure}\n\\end{figure}\n";
        assert_eq!(findings(different).len(), 1);
    }

    #[test]
    fn starred_caption_remains_a_conservative_barrier() {
        let src = "\\begin{figure}\n  \\caption*{Unnumbered}\n  \\label{fig:x}\n  \
                   \\caption{Numbered}\n\\end{figure}\n";
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
