//! Comment directives that turn badness off for part of a file.
//!
//! Three families, all spelled as ordinary LaTeX line comments. The **verb
//! carries the scope**, so every form reads as an imperative (`skip-file` is
//! "skip this file", not "the file directive") and all three share one grammar:
//!
//! ```text
//! % badness-format <verb>              layout only
//! % badness-lint   <verb> [<rule>]     linting only, optionally one rule
//! % badness        <verb>              both at once
//! ```
//!
//! with `<verb>` one of:
//!
//! ```text
//! skip        the next construct
//! off … on    everything between the two
//! skip-file   the whole file, wherever the directive sits
//! ```
//!
//! Only the lint axis takes a `<rule>`, because only the linter has anything to
//! select; omitting it means every rule. The `: <reason>` tail is optional
//! everywhere and is never interpreted.
//!
//! ## The retired `% badness-ignore` family
//!
//! ```text
//! % badness-ignore <rule>: <reason>        → % badness-lint skip <rule>: <reason>
//! % badness-ignore-file <rule>: <reason>   → % badness-lint skip-file <rule>: <reason>
//! % badness-ignore-file: <reason>          → % badness-lint skip-file: <reason>
//! ```
//!
//! Still recognized, and resolved through exactly the same path as their
//! replacements — the deprecation is in the documentation, never in the
//! behavior. A directive spelling is user-facing API; breaking one silently
//! would be worse than carrying it. [`Directive::deprecated`] marks them, so a
//! lint rule reporting the retired spelling can reuse the parsed fact.
//!
//! ## Why this lives in the parser crate
//!
//! Both consumers need it and neither can reach the other: the formatter is
//! wasm-clean (and is what the dprint plugin embeds), the linter lives in the
//! root crate. Resolving a directive is a pure function of the tree, so it sits
//! below both.
//!
//! **Scope limit:** a directive is recognized in a [`SyntaxKind::COMMENT`] token
//! only. In a `.dtx` documentation line the leading `%` is a `DOC_MARGIN` and the
//! rest is prose, so a directive written there is inert; inside a `macrocode`
//! chunk (where `%` comments are ordinary) it works as everywhere else.

use std::collections::BTreeMap;

use rowan::{NodeOrToken, TextRange, TextSize};

use crate::syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

/// Which subsystem a directive turns off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// `% badness-format …` — layout only. Lint findings are still reported.
    Format,
    /// `% badness-lint …` — linting only, for one rule or all of them.
    Lint,
    /// `% badness …` — layout *and* every lint rule.
    Both,
}

impl Axis {
    /// Whether a directive on this axis turns off layout.
    pub fn covers_format(self) -> bool {
        matches!(self, Axis::Format | Axis::Both)
    }

    /// Whether a directive on this axis turns off linting.
    pub fn covers_lint(self) -> bool {
        matches!(self, Axis::Lint | Axis::Both)
    }
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
    /// `on` — closes an open `off` with the same axis and rule. Inert without one.
    On,
    /// `skip-file` — the whole file, wherever in it the directive sits.
    SkipFile,
}

/// One directive, as written. Resolution against the tree happens in
/// [`Suppressions::build`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directive {
    pub axis: Axis,
    pub verb: Verb,
    /// The rule the directive selects; `None` means every rule. Only ever `Some`
    /// on [`Axis::Lint`] — the other two axes have nothing to select.
    pub rule: Option<String>,
    /// Written in the retired `% badness-ignore` spelling. Behaves identically —
    /// this exists so a lint rule can report the retired spelling and offer the
    /// rewrite, without having to re-parse the comment.
    pub deprecated: bool,
}

/// Read a directive out of a comment token's text. Returns `None` for an
/// ordinary comment and for an unrecognized verb.
///
/// Leading `%`s are all stripped, so `%%% badness-format off` works; the verb
/// must be the first word after the family name, separated by whitespace.
pub fn parse_directive(comment: &str) -> Option<Directive> {
    let body = comment.trim_start_matches('%').trim_start();
    // Longest family name first, or a shorter one swallows a longer one's prefix
    // and the word-boundary check below rejects it for the wrong reason.
    if let Some(rest) = body.strip_prefix("badness-ignore-file") {
        // `…-file:` or a bare `…-file` is every rule; `…-file <rule>` is one.
        return Some(Directive {
            axis: Axis::Lint,
            verb: Verb::SkipFile,
            rule: parse_rule(rest),
            deprecated: true,
        });
    }
    if let Some(rest) = body.strip_prefix("badness-ignore") {
        // The retired node form always required a rule; a bare `% badness-ignore`
        // was inert and stays inert, rather than silently widening to every rule
        // on the way through the new grammar.
        return Some(Directive {
            axis: Axis::Lint,
            verb: Verb::Skip,
            rule: Some(parse_rule(rest)?),
            deprecated: true,
        });
    }
    let (axis, rest) = if let Some(rest) = body.strip_prefix("badness-format") {
        (Axis::Format, rest)
    } else if let Some(rest) = body.strip_prefix("badness-lint") {
        (Axis::Lint, rest)
    } else {
        (Axis::Both, body.strip_prefix("badness")?)
    };
    // The family name must end at a word boundary, so `% badness-formatting off`
    // and `% badnesslint skip` are ordinary comments.
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let rest = rest.trim_start();
    let end = word_end(rest);
    let verb = match &rest[..end] {
        "skip" => Verb::Skip,
        "off" => Verb::Off,
        "on" => Verb::On,
        "skip-file" => Verb::SkipFile,
        _ => return None,
    };
    // Only the lint axis takes a selector. A word after the verb on another axis
    // is prose in the reason position, not a rule we should quietly honor.
    let rule = if axis == Axis::Lint {
        parse_rule(&rest[end..])
    } else {
        None
    };
    Some(Directive {
        axis,
        verb,
        rule,
        deprecated: false,
    })
}

/// The leading `<rule>` word of a `<rule>: <reason>` tail, or `None` when the
/// tail opens with `:` (a reason and no rule) or is empty.
fn parse_rule(tail: &str) -> Option<String> {
    let trimmed = tail.trim_start();
    let end = word_end(trimmed);
    if end == 0 {
        return None;
    }
    Some(trimmed[..end].to_string())
}

/// The end of the first word of `s`, delimited by `:` or whitespace.
fn word_end(s: &str) -> usize {
    s.find(|c: char| c == ':' || c.is_whitespace())
        .unwrap_or(s.len())
}

/// The byte ranges a file's directives suppress, resolved per axis.
///
/// Ranges are sorted and non-overlapping (touching ones are merged), so a
/// consumer can test containment with a plain scan and never has to reason
/// about nesting.
#[derive(Debug, Clone, Default)]
pub struct Suppressions {
    format: Vec<TextRange>,
    lint_all: Vec<TextRange>,
    lint_rules: BTreeMap<String, Vec<TextRange>>,
}

/// A region opened by an `off` and waiting for its `on`.
struct OpenRegion {
    axis: Axis,
    rule: Option<String>,
    start: TextSize,
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
        let mut lint_all = Vec::new();
        let mut lint_rules: BTreeMap<String, Vec<TextRange>> = BTreeMap::new();
        // Regions are keyed by axis *and* rule: a `% badness-lint off` covering
        // every rule is not closed by a `% badness-lint on some-rule`, which
        // speaks for a strictly narrower thing.
        let mut open: Vec<OpenRegion> = Vec::new();
        // End of the most recent directive comment. A region anchor may never
        // reach back past it — see the `Verb::Off` arm.
        let mut prev_directive_end = TextSize::new(0);

        for element in root.descendants_with_tokens() {
            let NodeOrToken::Token(token) = element else {
                continue;
            };
            if token.kind() != SyntaxKind::COMMENT {
                continue;
            }
            let Some(directive) = parse_directive(token.text()) else {
                continue;
            };
            let mut record = |range: TextRange, rule: &Option<String>| {
                if directive.axis.covers_format() {
                    format.push(range);
                }
                if directive.axis.covers_lint() {
                    match rule {
                        Some(rule) => lint_rules.entry(rule.clone()).or_default().push(range),
                        None => lint_all.push(range),
                    }
                }
            };
            match directive.verb {
                Verb::SkipFile => record(root.text_range(), &directive.rule),
                Verb::Skip => {
                    if let Some(range) = skip_target(&token) {
                        record(range, &directive.rule);
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
                    if !open
                        .iter()
                        .any(|o| o.axis == directive.axis && o.rule == directive.rule)
                    {
                        open.push(OpenRegion {
                            axis: directive.axis,
                            rule: directive.rule.clone(),
                            start,
                        });
                    }
                }
                Verb::On => {
                    if let Some(i) = open
                        .iter()
                        .position(|o| o.axis == directive.axis && o.rule == directive.rule)
                    {
                        let region = open.remove(i);
                        record(
                            TextRange::new(region.start, token.text_range().start()),
                            &region.rule,
                        );
                    }
                }
            }
            prev_directive_end = token.text_range().end();
        }

        // Unclosed regions run to end of file.
        let eof = root.text_range().end();
        for region in open {
            let range = TextRange::new(region.start, eof);
            if region.axis.covers_format() {
                format.push(range);
            }
            if region.axis.covers_lint() {
                match &region.rule {
                    Some(rule) => lint_rules.entry(rule.clone()).or_default().push(range),
                    None => lint_all.push(range),
                }
            }
        }

        Self {
            format: merge(format),
            lint_all: merge(lint_all),
            lint_rules: lint_rules
                .into_iter()
                .map(|(rule, ranges)| (rule, merge(ranges)))
                .collect(),
        }
    }

    /// Whether the document carries no directive at all — the fast path for the
    /// overwhelming majority of files, so a consumer can skip its per-node test.
    pub fn is_empty(&self) -> bool {
        self.format.is_empty() && self.lint_all.is_empty() && self.lint_rules.is_empty()
    }

    /// Ranges the formatter must reproduce byte-for-byte.
    pub fn format_ranges(&self) -> &[TextRange] {
        &self.format
    }

    /// Ranges in which *every* lint rule is suppressed.
    pub fn lint_all_ranges(&self) -> &[TextRange] {
        &self.lint_all
    }

    /// Ranges in which one named rule is suppressed.
    pub fn lint_rule_ranges(&self) -> &BTreeMap<String, Vec<TextRange>> {
        &self.lint_rules
    }
}

/// Sort and coalesce, merging ranges that overlap *or touch*. Touching ranges
/// merge because two adjacent `off`/`on` regions describe one continuous span of
/// suppressed text, and leaving them split would let a consumer that tests
/// containment miss an element straddling the seam.
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

    fn slices<'a>(src: &'a str, ranges: &[TextRange]) -> Vec<&'a str> {
        ranges
            .iter()
            .map(|r| &src[usize::from(r.start())..usize::from(r.end())])
            .collect()
    }

    fn directive(axis: Axis, verb: Verb) -> Directive {
        Directive {
            axis,
            verb,
            rule: None,
            deprecated: false,
        }
    }

    #[test]
    fn parses_every_form_on_every_axis() {
        for (family, axis) in [
            ("badness-format", Axis::Format),
            ("badness-lint", Axis::Lint),
            ("badness", Axis::Both),
        ] {
            for (word, verb) in [
                ("skip", Verb::Skip),
                ("off", Verb::Off),
                ("on", Verb::On),
                ("skip-file", Verb::SkipFile),
            ] {
                let text = format!("% {family} {word}");
                assert_eq!(
                    parse_directive(&text),
                    Some(directive(axis, verb)),
                    "parsing {text:?}"
                );
            }
        }
    }

    #[test]
    fn only_the_lint_axis_takes_a_rule() {
        assert_eq!(
            parse_directive("% badness-lint skip deprecated-command: legacy"),
            Some(Directive {
                axis: Axis::Lint,
                verb: Verb::Skip,
                rule: Some("deprecated-command".into()),
                deprecated: false,
            })
        );
        // A word after the verb on another axis is reason prose, not a selector.
        assert_eq!(
            parse_directive("% badness-format skip deprecated-command"),
            Some(directive(Axis::Format, Verb::Skip))
        );
        assert_eq!(
            parse_directive("% badness skip deprecated-command"),
            Some(directive(Axis::Both, Verb::Skip))
        );
    }

    #[test]
    fn lint_rule_is_optional_and_means_every_rule() {
        assert_eq!(
            parse_directive("% badness-lint skip-file: generated"),
            Some(directive(Axis::Lint, Verb::SkipFile))
        );
    }

    #[test]
    fn reason_is_optional_and_ignored() {
        assert_eq!(
            parse_directive("% badness-format skip: hand-aligned by eye"),
            Some(directive(Axis::Format, Verb::Skip))
        );
        assert_eq!(
            parse_directive("%badness skip-file:generated"),
            Some(directive(Axis::Both, Verb::SkipFile))
        );
    }

    #[test]
    fn repeated_percent_is_allowed() {
        assert_eq!(
            parse_directive("%%% badness-format off"),
            Some(directive(Axis::Format, Verb::Off))
        );
    }

    /// The retired spellings resolve exactly like their replacements, and are
    /// flagged so the lint rule can offer the rewrite.
    #[test]
    fn retired_ignore_family_still_parses() {
        assert_eq!(
            parse_directive("% badness-ignore deprecated-command: legacy"),
            Some(Directive {
                axis: Axis::Lint,
                verb: Verb::Skip,
                rule: Some("deprecated-command".into()),
                deprecated: true,
            })
        );
        assert_eq!(
            parse_directive("% badness-ignore-file deprecated-command: legacy"),
            Some(Directive {
                axis: Axis::Lint,
                verb: Verb::SkipFile,
                rule: Some("deprecated-command".into()),
                deprecated: true,
            })
        );
        assert_eq!(
            parse_directive("% badness-ignore-file: noisy"),
            Some(Directive {
                axis: Axis::Lint,
                verb: Verb::SkipFile,
                rule: None,
                deprecated: true,
            })
        );
    }

    /// The retired node form always required a rule. A bare one was inert and
    /// must not widen to "every rule" on its way through the new grammar.
    #[test]
    fn bare_retired_node_directive_stays_inert() {
        assert_eq!(parse_directive("% badness-ignore"), None);
        assert_eq!(parse_directive("% badness-ignore: no rule named"), None);
    }

    #[test]
    fn non_directives_are_inert() {
        for text in [
            "% just a note",
            "% badness",                 // no verb
            "% badness-lint",            // no verb
            "% badness-format nonsense", // unknown verb
            "% badnessformat off",       // no word boundary
            "% badness-formatting off",  // no word boundary
            "% badnesslint skip",        // no word boundary
            "% the badness-format off",  // not at the start
        ] {
            assert_eq!(parse_directive(text), None, "expected {text:?} to be inert");
        }
    }

    #[test]
    fn skip_targets_the_documented_construct() {
        let src = "% badness-format skip: hand-aligned\n\\begin{tikzpicture}\n\\draw (0,0);\n\\end{tikzpicture}\n";
        let s = suppressions_of(src);
        assert_eq!(slices(src, s.format_ranges()), vec![src.trim_end()]);
        assert!(s.lint_all_ranges().is_empty(), "format axis must not lint");
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
        assert_eq!(s.format_ranges(), s.lint_all_ranges());
        assert_eq!(
            slices(src, s.lint_all_ranges()),
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
            slices(src, s.lint_all_ranges()),
            vec!["% badness off\n\\beta\n% badness-format on\n\\gamma\n"]
        );
    }

    /// The same rule one axis down: a rule-selective `on` does not close an
    /// every-rule `off`.
    #[test]
    fn rule_selective_on_does_not_close_an_every_rule_region() {
        let src = "% badness-lint off\n\\beta\n% badness-lint on deprecated-command\n\\gamma\n";
        let s = suppressions_of(src);
        assert_eq!(s.lint_all_ranges().len(), 1);
        assert!(
            slices(src, s.lint_all_ranges())[0].ends_with("\\gamma\n"),
            "the every-rule region stays open to EOF"
        );
    }

    #[test]
    fn lint_region_is_rule_selective() {
        let src =
            "% badness-lint off deprecated-command\n\\beta\n% badness-lint on deprecated-command\n";
        let s = suppressions_of(src);
        assert!(s.lint_all_ranges().is_empty(), "one rule, not all of them");
        assert!(s.format_ranges().is_empty(), "lint axis must not format");
        let ranges = s
            .lint_rule_ranges()
            .get("deprecated-command")
            .expect("rule recorded");
        assert_eq!(
            slices(src, ranges),
            vec!["% badness-lint off deprecated-command\n\\beta\n"]
        );
    }

    #[test]
    fn skip_file_covers_the_document_on_its_axis() {
        let src = "\\alpha\n% badness-format skip-file: generated\n\\beta\n";
        let s = suppressions_of(src);
        assert_eq!(slices(src, s.format_ranges()), vec![src]);
        assert!(s.lint_all_ranges().is_empty());
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

    /// Two regions closed and reopened in one comment run stay distinct. Both
    /// directives bind into the same `DOC_COMMENT`, so without the
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

    /// The retired spellings resolve through the same path as their
    /// replacements — the deprecation is documentation, never behavior.
    ///
    /// Compared by what the range *covers*, not by the text it slices: the two
    /// directive comments have different lengths and both ride inside the range,
    /// so the slices can never be equal even when the resolution is identical.
    #[test]
    fn retired_and_current_spellings_resolve_identically() {
        /// Whether `\bf`, the construct the directive points at, is covered.
        fn covers_target(src: &str, ranges: &[TextRange]) -> bool {
            let at = TextSize::new(src.find("\\bf").expect("has a target") as u32);
            ranges.iter().any(|r| r.contains(at))
        }
        for (old, new) in [
            (
                "% badness-ignore deprecated-command: legacy\n\\bf x\n",
                "% badness-lint skip deprecated-command: legacy\n\\bf x\n",
            ),
            (
                "% badness-ignore-file deprecated-command: legacy\n\\bf x\n",
                "% badness-lint skip-file deprecated-command: legacy\n\\bf x\n",
            ),
        ] {
            for (src, label) in [(old, "retired"), (new, "current")] {
                let s = suppressions_of(src);
                let ranges = s
                    .lint_rule_ranges()
                    .get("deprecated-command")
                    .unwrap_or_else(|| panic!("{label} spelling records the rule: {src:?}"));
                assert!(
                    covers_target(src, ranges),
                    "{label} spelling must cover its target: {src:?}"
                );
                assert!(
                    s.lint_all_ranges().is_empty() && s.format_ranges().is_empty(),
                    "{label} spelling is lint-only and rule-selective: {src:?}"
                );
            }
        }
        // …and the every-rule file form likewise.
        let old = suppressions_of("% badness-ignore-file: noisy\n\\bf x\n");
        let new = suppressions_of("% badness-lint skip-file: noisy\n\\bf x\n");
        assert_eq!(old.lint_all_ranges().len(), 1);
        assert_eq!(new.lint_all_ranges().len(), 1);
        assert!(old.lint_rule_ranges().is_empty() && new.lint_rule_ranges().is_empty());
    }

    #[test]
    fn clean_document_has_no_suppressions() {
        assert!(suppressions_of("\\alpha\n% an ordinary comment\n\\beta\n").is_empty());
    }
}
