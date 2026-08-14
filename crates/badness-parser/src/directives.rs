//! Comment directives that turn badness off for part of a file.
//!
//! Two families, both spelled as ordinary LaTeX line comments:
//!
//! ```text
//! % badness-format skip: <reason>       layout off for the next construct
//! % badness-format off                  layout off until the matching `on`
//! % badness-format on                   … and back on
//! % badness-format skip-file: <reason>  layout off for the whole file
//!
//! % badness skip: <reason>              the same four, for layout *and* every
//! % badness off                          lint rule at once
//! % badness on
//! % badness skip-file: <reason>
//! ```
//!
//! The verb carries the scope, so every form reads as an imperative
//! (`skip-file` is "skip this file", not "the file directive"). The `: <reason>`
//! is optional everywhere and is never interpreted.
//!
//! Rule-selective lint suppression stays in its own family
//! (`% badness-ignore <rule>`, see the `badness` crate's `linter::suppression`):
//! selecting a rule only makes sense for the linter, so a selector slot here
//! would be a slot nothing could ever fill. What this module adds on the lint
//! side is the *region* scope, which `% badness-ignore` has never had.
//!
//! This lives in the parser crate because both consumers need it and neither can
//! reach the other: the formatter is wasm-clean (and is what the dprint plugin
//! embeds), the linter lives in the root crate. Resolving a directive is a pure
//! function of the tree, so it sits below both.
//!
//! **Scope limit:** a directive is recognized in a [`SyntaxKind::COMMENT`] token
//! only. In a `.dtx` documentation line the leading `%` is a `DOC_MARGIN` and the
//! rest is prose, so a directive written there is inert; inside a `macrocode`
//! chunk (where `%` comments are ordinary) it works as everywhere else.

use rowan::{NodeOrToken, TextRange, TextSize};

use crate::syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

/// Which subsystem a directive turns off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// `% badness-format …` — layout only. Lint findings are still reported.
    Format,
    /// `% badness …` — layout *and* every lint rule.
    Both,
}

/// The scope a directive applies to. The verb *is* the scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    /// `skip` — the next meaningful sibling. When the directive comment binds
    /// forward into a `DOC_COMMENT` (parser trivia rule), the target is the
    /// whole construct that owns it, which is the shape an author writing a
    /// directive above `\begin{tikzpicture}` means.
    Skip,
    /// `off` — from the next meaningful thing (as [`Verb::Skip`] resolves it) to
    /// the matching `on`, or to end of file.
    Off,
    /// `on` — closes an open `off` on the same axis. Inert without one.
    On,
    /// `skip-file` — the whole file, wherever in it the directive sits.
    SkipFile,
}

/// Read a directive out of a comment token's text. Returns `None` for an
/// ordinary comment, for the `% badness-ignore` family (a different grammar,
/// owned by the linter), and for an unrecognized verb.
///
/// Leading `%`s are all stripped, so `%%% badness-format off` works; the verb
/// must be the first word after the family name, separated by whitespace.
pub fn parse_directive(comment: &str) -> Option<(Axis, Verb)> {
    let body = comment.trim_start_matches('%').trim_start();
    // Longest family name first: `badness-format …` would otherwise match the
    // bare `badness` family with a rest of `-format …`, which the whitespace
    // check below rejects — correct, but silently, and only by accident.
    let (axis, rest) = match body.strip_prefix("badness-format") {
        Some(rest) => (Axis::Format, rest),
        None => (Axis::Both, body.strip_prefix("badness")?),
    };
    // The family name must end at a word boundary. This is what keeps
    // `% badness-ignore deprecated-command` out (rest is `-ignore …`), and it is
    // load-bearing rather than incidental: that family is still live and means
    // something else.
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let rest = rest.trim_start();
    let end = rest
        .find(|c: char| c == ':' || c.is_whitespace())
        .unwrap_or(rest.len());
    let verb = match &rest[..end] {
        "skip" => Verb::Skip,
        "off" => Verb::Off,
        "on" => Verb::On,
        "skip-file" => Verb::SkipFile,
        _ => return None,
    };
    Some((axis, verb))
}

/// The byte ranges a file's directives suppress, resolved per axis.
///
/// Ranges are sorted and non-overlapping (touching ones are merged), so a
/// consumer can test membership with a linear scan or a binary search and never
/// has to reason about nesting.
#[derive(Debug, Clone, Default)]
pub struct Suppressions {
    format: Vec<TextRange>,
    lint: Vec<TextRange>,
}

impl Suppressions {
    /// Scan `root` for directives and resolve them into ranges.
    ///
    /// A `skip-file` becomes a range covering the whole document rather than a
    /// flag, so every consumer keeps one code path: whole-file suppression is
    /// just the widest region. (The document-level trailing-edge normalization
    /// and the `line_ending` post-pass still run over the result — the same
    /// carve-out protected regions already live under.)
    ///
    /// An `off` with no matching `on` runs to end of file, as it does in every
    /// other formatter that has the directive.
    pub fn build(root: &SyntaxNode) -> Self {
        let mut format = Vec::new();
        let mut lint = Vec::new();
        // One open region per axis, tracked independently: a `% badness off`
        // region is not closed by a `% badness-format on`, which turns off a
        // strictly narrower thing and so cannot speak for the lint half.
        let mut open_format: Option<TextSize> = None;
        let mut open_both: Option<TextSize> = None;
        // End of the most recent directive comment, on any axis. A region anchor
        // may never reach back past it — see the `Verb::Off` arm.
        let mut prev_directive_end = TextSize::new(0);

        for element in root.descendants_with_tokens() {
            let NodeOrToken::Token(token) = element else {
                continue;
            };
            if token.kind() != SyntaxKind::COMMENT {
                continue;
            }
            let Some((axis, verb)) = parse_directive(token.text()) else {
                continue;
            };
            match verb {
                Verb::SkipFile => {
                    let all = root.text_range();
                    format.push(all);
                    if axis == Axis::Both {
                        lint.push(all);
                    }
                }
                Verb::Skip => {
                    if let Some(range) = skip_target(&token) {
                        format.push(range);
                        if axis == Axis::Both {
                            lint.push(range);
                        }
                    }
                }
                // A region opens at the same place a `skip` would target: the
                // next meaningful thing. Anchoring to the raw byte after the
                // comment instead looks simpler and is wrong — an own-line `%`
                // binds *forward* into the following construct's `DOC_COMMENT`
                // (parser decision #9), so that construct begins at the comment,
                // ahead of the region, and a consumer testing containment would
                // find the very block the author meant to cover sticking out of
                // it. Resolving through the tree also picks up a preceding
                // comment run bound into the same `DOC_COMMENT`, which a byte
                // offset cannot see at all. Falls back to the byte after the
                // comment when nothing meaningful follows (a directive at EOF).
                //
                // Clamped so the anchor never reaches back past the previous
                // directive: consecutive own-line comments bind into *one*
                // `DOC_COMMENT`, so in `on` / `off` / `\b` the reopening `off`
                // resolves to a construct starting at the `on` — and the region
                // it opens would then swallow the very directive that closed the
                // one before it, fusing two deliberately separate regions into
                // one. The clamp is against directives only, so an ordinary
                // comment run above the directive is still covered.
                Verb::Off => {
                    let start = skip_target(&token)
                        .map(|r| r.start())
                        .unwrap_or_else(|| token.text_range().end())
                        .max(prev_directive_end);
                    let slot = match axis {
                        Axis::Format => &mut open_format,
                        Axis::Both => &mut open_both,
                    };
                    slot.get_or_insert(start);
                }
                Verb::On => {
                    let end = token.text_range().start();
                    match axis {
                        Axis::Format => {
                            if let Some(start) = open_format.take() {
                                format.push(TextRange::new(start, end));
                            }
                        }
                        Axis::Both => {
                            if let Some(start) = open_both.take() {
                                let range = TextRange::new(start, end);
                                format.push(range);
                                lint.push(range);
                            }
                        }
                    }
                }
            }
            prev_directive_end = token.text_range().end();
        }

        let eof = root.text_range().end();
        if let Some(start) = open_format {
            format.push(TextRange::new(start, eof));
        }
        if let Some(start) = open_both {
            let range = TextRange::new(start, eof);
            format.push(range);
            lint.push(range);
        }

        Self {
            format: merge(format),
            lint: merge(lint),
        }
    }

    /// Whether the document carries no directive at all — the fast path for the
    /// overwhelming majority of files, so a consumer can skip its per-node test.
    pub fn is_empty(&self) -> bool {
        self.format.is_empty() && self.lint.is_empty()
    }

    /// Ranges the formatter must reproduce byte-for-byte.
    pub fn format_ranges(&self) -> &[TextRange] {
        &self.format
    }

    /// Ranges in which every lint rule is suppressed.
    pub fn lint_ranges(&self) -> &[TextRange] {
        &self.lint
    }
}

/// Sort and coalesce, merging ranges that overlap *or touch*. Touching ranges
/// merge because two adjacent `off`/`on` regions describe one continuous span of
/// suppressed text, and leaving them split would let a consumer that tests
/// containment (rather than overlap) miss an element straddling the seam.
fn merge(mut ranges: Vec<TextRange>) -> Vec<TextRange> {
    ranges.sort_by_key(|r| (r.start(), r.end()));
    let mut out: Vec<TextRange> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match out.last_mut() {
            Some(last) if range.start() <= last.end() => {
                *last = TextRange::new(last.start(), last.end().max(range.end()));
            }
            _ => out.push(range),
        }
    }
    out
}

/// The range a node-scoped directive covers: the next non-trivia, non-comment
/// element after `token`, bubbling up through parents whose remaining siblings
/// are all trivia. A comment bound into a `DOC_COMMENT` targets the whole
/// construct that owns it, not a sibling — walking forward from such a comment
/// only ever finds pieces *inside* that construct (its control word, missing its
/// arguments), never the construct as a whole.
///
/// This mirrors the linter's `next_meaningful_sibling`, deliberately: the two
/// families should agree about what "the next thing" is, so an author who knows
/// where `% badness-ignore` lands already knows where `% badness-format skip`
/// lands.
fn skip_target(token: &SyntaxToken) -> Option<TextRange> {
    if let Some(parent) = token.parent()
        && parent.kind() == SyntaxKind::DOC_COMMENT
    {
        return Some(parent.parent()?.text_range());
    }
    let mut current = token.clone();
    loop {
        let parent = current.parent()?;
        if let Some(range) = first_meaningful_after(&parent, &NodeOrToken::Token(current.clone())) {
            return Some(range);
        }
        let grand = parent.parent()?;
        if let Some(range) = first_meaningful_after(&grand, &NodeOrToken::Node(parent.clone())) {
            return Some(range);
        }
        // Guard against a non-progressing climb (a single-child spine).
        if grand == parent {
            return None;
        }
        current = grand.first_token()?;
    }
}

/// The range of the first non-trivia element of `parent` strictly after `after`.
fn first_meaningful_after(
    parent: &SyntaxNode,
    after: &NodeOrToken<SyntaxNode, SyntaxToken>,
) -> Option<TextRange> {
    let mut past = false;
    for element in parent.children_with_tokens() {
        if !past {
            past = &element == after;
            continue;
        }
        match &element {
            NodeOrToken::Token(t)
                if matches!(
                    t.kind(),
                    SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
                ) => {}
            _ => return Some(element.text_range()),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn suppressions_of(src: &str) -> Suppressions {
        Suppressions::build(&SyntaxNode::new_root(parse(src).green))
    }

    /// The suppressed slices of `src`, as text, for the given axis accessor.
    fn slices<'a>(src: &'a str, ranges: &[TextRange]) -> Vec<&'a str> {
        ranges
            .iter()
            .map(|r| &src[usize::from(r.start())..usize::from(r.end())])
            .collect()
    }

    #[test]
    fn parses_every_form() {
        for (text, want) in [
            ("% badness-format skip", (Axis::Format, Verb::Skip)),
            ("% badness-format off", (Axis::Format, Verb::Off)),
            ("% badness-format on", (Axis::Format, Verb::On)),
            ("% badness-format skip-file", (Axis::Format, Verb::SkipFile)),
            ("% badness skip", (Axis::Both, Verb::Skip)),
            ("% badness off", (Axis::Both, Verb::Off)),
            ("% badness on", (Axis::Both, Verb::On)),
            ("% badness skip-file", (Axis::Both, Verb::SkipFile)),
        ] {
            assert_eq!(parse_directive(text), Some(want), "parsing {text:?}");
        }
    }

    #[test]
    fn reason_is_optional_and_ignored() {
        assert_eq!(
            parse_directive("% badness-format skip: hand-aligned by eye"),
            Some((Axis::Format, Verb::Skip))
        );
        assert_eq!(
            parse_directive("%badness skip-file:generated"),
            Some((Axis::Both, Verb::SkipFile))
        );
    }

    #[test]
    fn repeated_percent_is_allowed() {
        assert_eq!(
            parse_directive("%%% badness-format off"),
            Some((Axis::Format, Verb::Off))
        );
    }

    /// The live `% badness-ignore` family must not be captured by the bare
    /// `badness` family — it means something else and is still supported.
    #[test]
    fn lint_ignore_family_is_not_a_directive() {
        assert_eq!(parse_directive("% badness-ignore deprecated-command"), None);
        assert_eq!(parse_directive("% badness-ignore-file: noisy"), None);
    }

    #[test]
    fn non_directives_are_inert() {
        for text in [
            "% just a note",
            "% badness",                 // no verb
            "% badness-format",          // no verb
            "% badness-format nonsense", // unknown verb
            "% badnessformat off",       // no word boundary
            "% badness-formatting off",  // no word boundary
            "% the badness-format off",  // not at the start
        ] {
            assert_eq!(parse_directive(text), None, "expected {text:?} to be inert");
        }
    }

    #[test]
    fn skip_targets_the_documented_construct() {
        let src = "% badness-format skip: hand-aligned\n\\begin{tikzpicture}\n\\draw (0,0);\n\\end{tikzpicture}\n";
        let s = suppressions_of(src);
        // The comment binds forward into the environment's `DOC_COMMENT`, so the
        // target is the whole construct — comment included.
        assert_eq!(
            slices(src, s.format_ranges()),
            vec![src.trim_end()],
            "skip should cover the whole documented environment"
        );
        assert!(
            s.lint_ranges().is_empty(),
            "format axis must not touch lint"
        );
    }

    /// A region runs from the construct the `off` documents (so the directive
    /// comment, bound into that construct's `DOC_COMMENT`, rides inside it) to
    /// the `on`.
    #[test]
    fn region_spans_from_off_to_on() {
        let src = "\\alpha\n% badness-format off\n\\beta\n% badness-format on\n\\gamma\n";
        let s = suppressions_of(src);
        assert_eq!(
            slices(src, s.format_ranges()),
            vec!["% badness-format off\n\\beta\n"]
        );
    }

    /// An ordinary comment above the directive binds into the same
    /// `DOC_COMMENT`, and the region covers it — the construct is what the
    /// author pointed at, whatever else got bound in front of it.
    #[test]
    fn region_covers_a_leading_comment_run() {
        let src = "\\alpha\n% a note\n% badness-format off\n\\beta\n% badness-format on\n";
        let s = suppressions_of(src);
        assert_eq!(
            slices(src, s.format_ranges()),
            vec!["% a note\n% badness-format off\n\\beta\n"]
        );
    }

    #[test]
    fn unclosed_region_runs_to_end_of_file() {
        let src = "\\alpha\n% badness-format off\n\\beta\n\\gamma\n";
        let s = suppressions_of(src);
        assert_eq!(
            slices(src, s.format_ranges()),
            vec!["% badness-format off\n\\beta\n\\gamma\n"]
        );
    }

    #[test]
    fn both_family_suppresses_both_axes() {
        let src = "% badness off\n\\beta\n% badness on\n";
        let s = suppressions_of(src);
        assert_eq!(s.format_ranges(), s.lint_ranges());
        assert_eq!(
            slices(src, s.lint_ranges()),
            vec!["% badness off\n\\beta\n"]
        );
    }

    /// A narrower `on` must not close a wider `off`: the format directive has
    /// nothing to say about the lint half of a combined region.
    #[test]
    fn format_on_does_not_close_a_both_region() {
        let src = "% badness off\n\\beta\n% badness-format on\n\\gamma\n";
        let s = suppressions_of(src);
        assert_eq!(
            slices(src, s.lint_ranges()),
            vec!["% badness off\n\\beta\n% badness-format on\n\\gamma\n"]
        );
    }

    #[test]
    fn skip_file_covers_the_document_on_its_axis() {
        let src = "\\alpha\n% badness-format skip-file: generated\n\\beta\n";
        let s = suppressions_of(src);
        assert_eq!(slices(src, s.format_ranges()), vec![src]);
        assert!(s.lint_ranges().is_empty());
    }

    #[test]
    fn stray_on_is_inert() {
        let src = "\\alpha\n% badness-format on\n\\beta\n";
        assert!(suppressions_of(src).is_empty());
    }

    /// A `skip-file` swallows every narrower range on its axis, so a consumer
    /// never sees the same byte twice.
    #[test]
    fn overlapping_ranges_merge() {
        let src = "% badness-format skip-file: generated\n% badness-format off\n\\b\n";
        let s = suppressions_of(src);
        assert_eq!(slices(src, s.format_ranges()), vec![src]);
    }

    /// …but two regions closed and reopened in one comment run stay distinct.
    /// Both directives bind into the same `DOC_COMMENT`, so without the
    /// previous-directive clamp the reopening `off` would anchor back onto the
    /// `on` and the two would fuse into one region.
    #[test]
    fn reopened_region_does_not_swallow_its_own_closer() {
        let src = "% badness-format off\n\\a\n% badness-format on\n% badness-format off\n\\b\n% badness-format on\n";
        let s = suppressions_of(src);
        assert_eq!(
            slices(src, s.format_ranges()),
            vec![
                "% badness-format off\n\\a\n",
                "\n% badness-format off\n\\b\n"
            ]
        );
    }

    #[test]
    fn clean_document_has_no_suppressions() {
        assert!(suppressions_of("\\alpha\n% an ordinary comment\n\\beta\n").is_empty());
    }
}
