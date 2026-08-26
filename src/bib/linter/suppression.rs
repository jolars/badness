//! Comment-based suppression for `.bib`, carried inside `@comment{…}` entries.
//!
//! A `%` line comment only exists *inside* an entry in BibTeX — free text between
//! entries lexes as `JUNK`, with no comment token to hang a directive on — so
//! unlike the LaTeX side the carrier is a structured `@comment` entry, a
//! [`COMMENT_ENTRY`] node:
//!
//! ```text
//! @comment{badness-lint skip <rule>: <reason>}       the next entry, one rule
//! @comment{badness-lint skip: <reason>}              the next entry, every rule
//! @comment{badness-lint off <rule>} … {… on <rule>}  a region of entries
//! @comment{badness-lint skip-file <rule>: <reason>}  the whole file
//! ```
//!
//! The **grammar** is [`crate::directives::parse_directive`], shared with the
//! LaTeX side and the formatter, so the two carriers cannot drift; only the
//! carrier and the next-sibling attachment differ. The retired
//! `@comment{badness-ignore …}` and `@comment{badness-ignore-file …}` spellings
//! still resolve through the same path and are no longer documented.
//!
//! **Only the lint axis acts here.** `badness-format` is accepted by the shared
//! grammar but does nothing in a `.bib`: the bib formatter is a canonical
//! re-emitter rather than a trivia-only pass, so "reproduce this span byte for
//! byte" is a different mechanism there, not a matter of routing these ranges
//! through. Such a directive is retained as [`DirectiveOutcome::Unsupported`]
//! for the `inert-suppression` rule.
//!
//! [`COMMENT_ENTRY`]: crate::bib::syntax::SyntaxKind::COMMENT_ENTRY

use std::collections::HashMap;

use rowan::NodeOrToken;

use crate::bib::syntax::{SyntaxKind, SyntaxNode};
use crate::directives::{Directive, DirectiveOutcome, Verb, parse_directive};

#[derive(Debug, Clone)]
pub(crate) struct LocatedDirective {
    pub directive: Directive,
    pub range: rowan::TextRange,
    pub family_range: rowan::TextRange,
    pub outcome: DirectiveOutcome,
}

#[derive(Debug, Clone, Default)]
pub struct BibSuppressionMap {
    /// Byte ranges in which *every* rule is suppressed.
    all_ranges: Vec<(usize, usize)>,
    /// `rule → byte ranges` for the rule-selective directives.
    rule_ranges: HashMap<String, Vec<(usize, usize)>>,
    directives: Vec<LocatedDirective>,
}

/// A region opened by an `off` and waiting for its `on`.
struct OpenRegion {
    rule: Option<String>,
    start: usize,
    directive_index: usize,
}

impl BibSuppressionMap {
    pub fn build(root: &SyntaxNode) -> Self {
        let mut map = Self::default();
        let mut open: Vec<OpenRegion> = Vec::new();

        for node in root.descendants() {
            if node.kind() != SyntaxKind::COMMENT_ENTRY {
                continue;
            }
            let Some(body) = comment_directive_text(&node) else {
                continue;
            };
            let Some(directive) = parse_directive(body.trim()) else {
                continue;
            };
            let text = node.to_string();
            let family = if directive.deprecated {
                match directive.verb {
                    Verb::Skip => "badness-ignore",
                    Verb::SkipFile => "badness-ignore-file",
                    Verb::Off | Verb::On => unreachable!("retired directives have no regions"),
                }
            } else {
                match directive.axis {
                    crate::directives::Axis::Format => "badness-format",
                    crate::directives::Axis::Lint => "badness-lint",
                    crate::directives::Axis::Both => "badness",
                }
            };
            let relative = text
                .find(family)
                .expect("parsed directive contains its family name");
            let start = usize::from(node.text_range().start()) + relative;
            let directive_index = map.directives.len();
            map.directives.push(LocatedDirective {
                directive: directive.clone(),
                range: node.text_range(),
                family_range: rowan::TextRange::new(
                    rowan::TextSize::from(start as u32),
                    rowan::TextSize::from((start + family.len()) as u32),
                ),
                outcome: if directive.axis.covers_lint() {
                    DirectiveOutcome::Honored
                } else {
                    DirectiveOutcome::Unsupported
                },
            });
            if !directive.axis.covers_lint() {
                continue;
            }
            match directive.verb {
                Verb::SkipFile => {
                    map.record(span(root.text_range()), &directive.rule);
                }
                Verb::Skip => {
                    if let Some(target) = next_meaningful_sibling(&node) {
                        map.record(target, &directive.rule);
                    } else {
                        map.directives[directive_index].outcome = DirectiveOutcome::DanglingSkip;
                    }
                }
                // Unlike the LaTeX side there is no forward comment binding to
                // work around here — an `@comment` entry is a sibling, never
                // reparented into the entry below it — so the region simply
                // opens at the next meaningful sibling.
                Verb::Off => {
                    if !open.iter().any(|o| o.rule == directive.rule) {
                        map.directives[directive_index].outcome = DirectiveOutcome::UnclosedOff;
                        let start = next_meaningful_sibling(&node)
                            .map(|(s, _)| s)
                            .unwrap_or_else(|| usize::from(node.text_range().end()));
                        open.push(OpenRegion {
                            rule: directive.rule.clone(),
                            start,
                            directive_index,
                        });
                    }
                }
                Verb::On => {
                    if let Some(i) = open.iter().position(|o| o.rule == directive.rule) {
                        let region = open.remove(i);
                        map.directives[region.directive_index].outcome = DirectiveOutcome::Honored;
                        let end = usize::from(node.text_range().start());
                        map.record((region.start, end), &region.rule);
                    } else {
                        map.directives[directive_index].outcome = DirectiveOutcome::UnmatchedOn;
                    }
                }
            }
        }

        // Unclosed regions run to end of file.
        let eof = usize::from(root.text_range().end());
        for region in open {
            map.record((region.start, eof), &region.rule);
        }
        map
    }

    fn record(&mut self, range: (usize, usize), rule: &Option<String>) {
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
}

/// The inner text of a `@comment{…}` / `@comment(…)` entry — everything between
/// the opening and closing delimiter. Returns `None` if no delimiter pair is
/// found. Used only to read a directive, so nested braces (which never occur in a
/// directive line) need no special handling.
fn comment_directive_text(node: &SyntaxNode) -> Option<String> {
    let text = node.to_string();
    let open = text.find(['{', '('])?;
    let close = text.rfind(['}', ')'])?;
    if close <= open {
        return None;
    }
    Some(text[open + 1..close].to_string())
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
fn span(range: rowan::TextRange) -> (usize, usize) {
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
                DirectiveOutcome::Unsupported,
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

    /// `badness-format` parses but has no bib meaning; it must not silence a
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
