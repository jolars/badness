//! Comment-based suppression, as the linter consumes it.
//!
//! The directive grammar itself lives in [`crate::directives`], shared with the
//! formatter — one grammar for all three families:
//!
//! ```text
//! % badness-lint skip <rule>: <reason>       the next construct, one rule
//! % badness-lint skip: <reason>              the next construct, every rule
//! % badness-lint off <rule> … on <rule>      a region
//! % badness-lint skip-file <rule>: <reason>  the whole file
//! % badness skip / off / on / skip-file      the same, plus the formatter
//! ```
//!
//! Omitting `<rule>` means every rule. The retired `% badness-ignore` and
//! `% badness-ignore-file` spellings still resolve, through the same path, and
//! are no longer documented.
//!
//! This module is only the *lookup*: it flattens the resolved ranges into the
//! plain `usize` offsets [`Diagnostic`](super::Diagnostic) stores (a rowan
//! `TextRange` never reaches a diagnostic) and answers containment queries. A
//! finding is suppressed when its `[start, end)` falls fully inside a range
//! registered for its rule, or inside an every-rule range.

use std::collections::HashMap;

use rowan::TextRange;

use crate::syntax::SyntaxNode;

#[derive(Debug, Clone, Default)]
pub struct SuppressionMap {
    /// Byte ranges in which *every* rule is suppressed.
    all_ranges: Vec<(usize, usize)>,
    /// `rule → byte ranges` for the rule-selective directives.
    rule_ranges: HashMap<String, Vec<(usize, usize)>>,
}

impl SuppressionMap {
    pub fn build(root: &SyntaxNode) -> Self {
        let resolved = crate::directives::Suppressions::build(root);
        Self::from_suppressions(&resolved)
    }

    pub fn from_suppressions(resolved: &crate::directives::Suppressions) -> Self {
        Self {
            all_ranges: spans(resolved.lint_all_ranges()),
            rule_ranges: resolved
                .lint_rule_ranges()
                .iter()
                .map(|(rule, ranges)| (rule.clone(), spans(ranges)))
                .collect(),
        }
    }

    /// Whether a `[start, end)` diagnostic for `rule` is suppressed.
    pub fn is_suppressed(&self, rule: &str, start: usize, end: usize) -> bool {
        let covers =
            |ranges: &[(usize, usize)]| ranges.iter().any(|(rs, re)| *rs <= start && end <= *re);
        covers(&self.all_ranges) || self.rule_ranges.get(rule).is_some_and(|r| covers(r))
    }
}

/// Resolved ranges as the plain `(start, end)` offsets the map stores.
fn spans(ranges: &[TextRange]) -> Vec<(usize, usize)> {
    ranges
        .iter()
        .map(|r| (usize::from(r.start()), usize::from(r.end())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn map_of(src: &str) -> SuppressionMap {
        SuppressionMap::build(&SyntaxNode::new_root(parse(src).green))
    }

    #[test]
    fn skip_file_without_a_rule_suppresses_everything() {
        let m = map_of("% badness-lint skip-file: noisy\n\\bf\n");
        assert!(m.is_suppressed("anything", 0, 1));
    }

    #[test]
    fn skip_file_with_a_rule_suppresses_only_that_rule() {
        let m = map_of("% badness-lint skip-file deprecated-command: legacy\n\\bf\n");
        assert!(m.is_suppressed("deprecated-command", 0, 1));
        assert!(!m.is_suppressed("duplicate-label", 0, 1));
    }

    #[test]
    fn skip_targets_the_next_construct_only() {
        let src = "% badness-lint skip deprecated-command\n\\bf x\n\n\\bf y\n";
        let m = map_of(src);
        let first = src.find("\\bf x").expect("has first");
        assert!(m.is_suppressed("deprecated-command", first, first + 3));
        let second = src.find("\\bf y").expect("has second");
        assert!(!m.is_suppressed("deprecated-command", second, second + 3));
    }

    /// Region scope, which the retired family never had.
    #[test]
    fn lint_region_suppresses_one_rule_between_its_markers() {
        let src = "\\bf a\n% badness-lint off deprecated-command\n\\bf b\n\
                   % badness-lint on deprecated-command\n\\bf c\n";
        let m = map_of(src);
        let inside = src.find("\\bf b").expect("has b");
        assert!(m.is_suppressed("deprecated-command", inside, inside + 3));
        assert!(
            !m.is_suppressed("duplicate-label", inside, inside + 3),
            "a rule-selective region must not silence other rules"
        );
        let after = src.find("\\bf c").expect("has c");
        assert!(!m.is_suppressed("deprecated-command", after, after + 3));
    }

    /// The combined family covers every rule, and the formatter besides.
    #[test]
    fn combined_family_suppresses_every_rule_in_a_region() {
        let src = "\\bf\n% badness off\n\\it\n% badness on\n\\bf\n";
        let m = map_of(src);
        let inside = src.find("\\it").expect("has \\it");
        assert!(m.is_suppressed("any-rule", inside, inside + 3));
        let outside = src.rfind("\\bf").expect("has trailing \\bf");
        assert!(!m.is_suppressed("any-rule", outside, outside + 3));
    }

    /// …but the format-only family must leave the linter alone.
    #[test]
    fn format_family_does_not_suppress_lint() {
        let src = "% badness-format off\n\\bf\n% badness-format on\n";
        let m = map_of(src);
        let at = src.find("\\bf").expect("has \\bf");
        assert!(!m.is_suppressed("deprecated-command", at, at + 3));
    }

    /// The retired spellings keep working — deprecated in the docs, not in
    /// behavior. Deleting these tests is how the promise quietly breaks.
    #[test]
    fn retired_ignore_family_still_suppresses() {
        let node = map_of("% badness-ignore deprecated-command: legacy\n\\bf\n");
        let at = "% badness-ignore deprecated-command: legacy\n".len();
        assert!(node.is_suppressed("deprecated-command", at, at + 3));
        assert!(!node.is_suppressed("duplicate-label", at, at + 3));

        let file_rule = map_of("% badness-ignore-file deprecated-command: legacy\n\\bf\n");
        assert!(file_rule.is_suppressed("deprecated-command", 0, 1));
        assert!(!file_rule.is_suppressed("duplicate-label", 0, 1));

        let file_all = map_of("% badness-ignore-file: noisy\n\\bf\n");
        assert!(file_all.is_suppressed("anything", 0, 1));
    }

    #[test]
    fn non_directive_comment_is_inert() {
        let m = map_of("% just a note\n\\bf\n");
        assert!(!m.is_suppressed("deprecated-command", 0, 1));
    }
}
