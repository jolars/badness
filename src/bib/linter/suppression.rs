//! BibTeX suppression directives in `@comment{…}` entries and `%` line comments.
//!
//! An `@comment` entry can suppress the next entry, a region, or the whole file.
//! Inside a regular entry, `% badness-lint skip` can suppress a single field.
//!
//! ```text
//! @comment{badness-lint skip <rule>: <reason>}       the next entry, one rule
//! @comment{badness-lint skip: <reason>}              the next entry, every rule
//! @comment{badness-lint off <rule>} … {… on <rule>}  a region of entries
//! @comment{badness-lint skip-file <rule>: <reason>}  the whole file
//! ```
//!
//! Both forms use [`crate::directives::parse_directive`]. The field target comes
//! from the same comment placement the formatter uses when sorting fields.

use std::collections::{HashMap, HashSet};

use rowan::{NodeOrToken, TextRange, TextSize};

use crate::bib::ast::{self, FieldCommentPlacement};
use crate::bib::comment_directive;
use crate::bib::syntax::{SyntaxKind, SyntaxNode};
use crate::directives::{Axis, Directive, DirectiveOutcome, Verb, parse_directive};

#[derive(Debug, Clone)]
pub(crate) struct LocatedDirective {
    pub directive: Directive,
    pub range: TextRange,
    pub family_range: TextRange,
    pub outcome: DirectiveOutcome,
    pub is_line_comment: bool,
}

impl LocatedDirective {
    fn new(node: &SyntaxNode, text: &str, directive: Directive, outcome: DirectiveOutcome) -> Self {
        let family = if directive.deprecated {
            match directive.verb {
                Verb::Skip => "badness-ignore",
                Verb::SkipFile => "badness-ignore-file",
                Verb::Off | Verb::On => unreachable!("retired directives have no regions"),
            }
        } else {
            match directive.axis {
                Axis::Format => "badness-format",
                Axis::Lint => "badness-lint",
                Axis::Both => "badness",
            }
        };
        let relative = text
            .find(family)
            .expect("parsed directive contains its family name");
        let range = node.text_range();
        let start = range.start() + TextSize::from(relative as u32);
        Self {
            directive,
            range,
            family_range: TextRange::at(start, TextSize::of(family)),
            outcome,
            is_line_comment: node.kind() == SyntaxKind::COMMENT,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct BibSuppressionMap {
    /// Byte ranges in which *every* rule is suppressed.
    all_ranges: Vec<(usize, usize)>,
    /// `rule → byte ranges` for the rule-selective directives.
    rule_ranges: HashMap<String, Vec<(usize, usize)>>,
    field_targets: HashSet<TextRange>,
    /// Directives in source order, including unsupported forms.
    directives: Vec<LocatedDirective>,
}

/// A region opened by an `off` and waiting for its `on`.
struct OpenRegion {
    axis: Axis,
    rule: Option<String>,
    start: usize,
    directive_index: usize,
}

impl BibSuppressionMap {
    pub fn build(root: &SyntaxNode) -> Self {
        let mut map = Self::default();
        let mut open: Vec<OpenRegion> = Vec::new();

        for node in root.children() {
            if node.kind() == SyntaxKind::ENTRY {
                map.record_field_directives(&node);
                continue;
            }
            let Some(directive) = comment_directive(&node) else {
                continue;
            };
            let directive_index = map.directives.len();
            map.directives.push(LocatedDirective::new(
                &node,
                &node.to_string(),
                directive.clone(),
                DirectiveOutcome::Honored,
            ));
            match directive.verb {
                Verb::SkipFile => {
                    map.record(directive.axis, span(root.text_range()), &directive.rule);
                }
                Verb::Skip => {
                    if let Some(target) = next_meaningful_sibling(&node) {
                        map.record(directive.axis, target, &directive.rule);
                    } else {
                        map.directives[directive_index].outcome = DirectiveOutcome::DanglingSkip;
                    }
                }
                // Unlike the LaTeX side there is no forward comment binding to
                // work around here — an `@comment` entry is a sibling, never
                // reparented into the entry below it — so the region simply
                // opens at the next meaningful sibling.
                Verb::Off => {
                    if !open
                        .iter()
                        .any(|o| o.axis == directive.axis && o.rule == directive.rule)
                    {
                        map.directives[directive_index].outcome = DirectiveOutcome::UnclosedOff;
                        let start = next_meaningful_sibling(&node)
                            .map(|(s, _)| s)
                            .unwrap_or_else(|| usize::from(node.text_range().end()));
                        open.push(OpenRegion {
                            axis: directive.axis,
                            rule: directive.rule.clone(),
                            start,
                            directive_index,
                        });
                    }
                }
                Verb::On => {
                    if let Some(i) = open
                        .iter()
                        .position(|o| o.axis == directive.axis && o.rule == directive.rule)
                    {
                        let region = open.remove(i);
                        map.directives[region.directive_index].outcome = DirectiveOutcome::Honored;
                        let end = usize::from(node.text_range().start());
                        map.record(region.axis, (region.start, end), &region.rule);
                    } else {
                        map.directives[directive_index].outcome = DirectiveOutcome::UnmatchedOn;
                    }
                }
            }
        }

        // Unclosed regions run to end of file.
        let eof = usize::from(root.text_range().end());
        for region in open {
            map.record(region.axis, (region.start, eof), &region.rule);
        }
        map
    }

    fn record_field_directives(&mut self, entry: &SyntaxNode) {
        for (comment, placement) in ast::field_comments(entry) {
            let text = comment.to_string();
            let Some(directive) = parse_directive(&text) else {
                continue;
            };
            let outcome = match (directive.axis, directive.verb, placement) {
                (
                    Axis::Lint,
                    Verb::Skip,
                    FieldCommentPlacement::Leading(field) | FieldCommentPlacement::SameLine(field),
                ) => {
                    self.record(directive.axis, span(field), &directive.rule);
                    self.field_targets.insert(field);
                    DirectiveOutcome::Honored
                }
                (Axis::Lint, Verb::Skip, FieldCommentPlacement::Trailing) => {
                    DirectiveOutcome::DanglingSkip
                }
                _ => DirectiveOutcome::Unsupported,
            };
            self.directives
                .push(LocatedDirective::new(&comment, &text, directive, outcome));
        }
    }

    fn record(&mut self, axis: Axis, range: (usize, usize), rule: &Option<String>) {
        if !axis.covers_lint() {
            return;
        }
        match rule {
            Some(rule) => self
                .rule_ranges
                .entry(rule.clone())
                .or_default()
                .push(range),
            None => self.all_ranges.push(range),
        }
    }

    /// Whether a `[start, end)` diagnostic for `rule` is suppressed.
    pub fn is_suppressed(&self, rule: &str, start: usize, end: usize) -> bool {
        let covers =
            |ranges: &[(usize, usize)]| ranges.iter().any(|(rs, re)| *rs <= start && end <= *re);
        covers(&self.all_ranges) || self.rule_ranges.get(rule).is_some_and(|r| covers(r))
    }

    pub(crate) fn directives(&self) -> &[LocatedDirective] {
        &self.directives
    }

    /// Deleting a field must preserve directive comments and their targets.
    pub(crate) fn blocks_field_deletion(&self, field: TextRange, deletion: TextRange) -> bool {
        if self.field_targets.contains(&field) {
            return true;
        }
        let first = self
            .directives
            .partition_point(|d| d.range.end() <= deletion.start());
        self.directives[first..]
            .iter()
            .take_while(|d| d.range.start() < deletion.end())
            .any(|d| d.is_line_comment)
    }
}

/// The byte range of the next non-trivia block after the directive entry,
/// skipping whitespace/newlines and further `@comment` entries (so two stacked
/// directives both attach to the entry that follows them).
fn next_meaningful_sibling(node: &SyntaxNode) -> Option<(usize, usize)> {
    let parent = node.parent()?;
    let mut past = false;
    for element in parent.children_with_tokens() {
        if !past {
            if matches!(&element, NodeOrToken::Node(n) if n == node) {
                past = true;
            }
            continue;
        }
        match &element {
            NodeOrToken::Token(t)
                if matches!(t.kind(), SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE) => {}
            NodeOrToken::Node(n) if n.kind() == SyntaxKind::COMMENT_ENTRY => {}
            _ => return Some(span(element.text_range())),
        }
    }
    None
}

/// A rowan `TextRange` as the plain `(start, end)` offsets the map stores.
fn span(range: TextRange) -> (usize, usize) {
    (usize::from(range.start()), usize::from(range.end()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bib::parse;
    use crate::directives::DirectiveOutcome;

    fn map_of(src: &str) -> BibSuppressionMap {
        BibSuppressionMap::build(&parse(src).syntax())
    }

    #[test]
    fn classifies_inert_and_incomplete_directives() {
        for (src, expected) in [
            (
                "@comment{badness-lint skip missing-required-field}\n",
                DirectiveOutcome::DanglingSkip,
            ),
            (
                "@comment{badness-lint on missing-required-field}\n",
                DirectiveOutcome::UnmatchedOn,
            ),
            (
                "@comment{badness-lint off missing-required-field}\n@book{a}\n",
                DirectiveOutcome::UnclosedOff,
            ),
            (
                "@comment{badness-format skip-file}\n@book{a}\n",
                DirectiveOutcome::Honored,
            ),
        ] {
            let map = map_of(src);
            assert_eq!(map.directives().len(), 1, "{src:?}");
            assert_eq!(map.directives()[0].outcome, expected, "{src:?}");
        }
    }

    #[test]
    fn retains_the_full_bib_directive_range() {
        let src = "@comment{badness-format skip-file}\n";
        let map = map_of(src);
        let [located] = map.directives() else {
            panic!("expected one retained directive: {:?}", map.directives());
        };
        assert_eq!(
            &src[usize::from(located.range.start())..usize::from(located.range.end())],
            "@comment{badness-format skip-file}"
        );
    }

    #[test]
    fn skip_file_without_a_rule_suppresses_everything() {
        let m = map_of("@comment{badness-lint skip-file: noisy}\n@book{a, title={T}}\n");
        assert!(m.is_suppressed("anything", 0, 1));
    }

    #[test]
    fn skip_file_with_a_rule_suppresses_only_that_rule() {
        let m = map_of("@comment{badness-lint skip-file missing-required-field: gone}\n");
        assert!(m.is_suppressed("missing-required-field", 0, 1));
        assert!(!m.is_suppressed("duplicate-key", 0, 1));
    }

    #[test]
    fn skip_targets_the_following_entry() {
        let src = "@comment{badness-lint skip missing-required-field: gone}\n@book{a, title={T}}\n";
        let m = map_of(src);
        let at = src.find("@book").expect("has the entry");
        assert!(m.is_suppressed("missing-required-field", at, at + 5));
    }

    #[test]
    fn skip_does_not_leak_to_later_entries() {
        let src = "@comment{badness-lint skip missing-required-field: gone}\n\
                   @book{a, title={T}}\n@book{b, title={U}}\n";
        let m = map_of(src);
        let later = src.rfind("@book").expect("has a second entry");
        assert!(!m.is_suppressed("missing-required-field", later, later + 5));
    }

    /// Region scope, which the retired family never had on either side.
    #[test]
    fn region_covers_the_entries_between_its_markers() {
        let src = "@comment{badness-lint off missing-required-field}\n\
                   @book{a, title={T}}\n\
                   @comment{badness-lint on missing-required-field}\n\
                   @book{b, title={U}}\n";
        let m = map_of(src);
        let inside = src.find("@book{a").expect("has a");
        assert!(m.is_suppressed("missing-required-field", inside, inside + 5));
        let outside = src.find("@book{b").expect("has b");
        assert!(!m.is_suppressed("missing-required-field", outside, outside + 5));
    }

    /// The retired spellings keep working — deprecated in the docs, not in
    /// behavior.
    #[test]
    fn retired_ignore_family_still_suppresses() {
        let src = "@comment{badness-ignore missing-required-field: gone}\n@book{a, title={T}}\n";
        let m = map_of(src);
        let at = src.find("@book").expect("has the entry");
        assert!(m.is_suppressed("missing-required-field", at, at + 5));

        let file = map_of("@comment{badness-ignore-file: noisy}\n@book{a, title={T}}\n");
        assert!(file.is_suppressed("anything", 0, 1));
    }

    /// `badness-format` must not silence a
    /// diagnostic on its way through.
    #[test]
    fn format_family_does_not_suppress_lint() {
        let m = map_of("@comment{badness-format skip-file}\n@book{a, title={T}}\n");
        assert!(!m.is_suppressed("missing-required-field", 0, 1));
    }

    #[test]
    fn non_directive_comment_is_inert() {
        let m = map_of("@comment{just a note}\n@book{a, title={T}}\n");
        assert!(!m.is_suppressed("missing-required-field", 0, 1));
    }
}
