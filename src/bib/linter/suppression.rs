//! Comment-based suppression for `.bib`: the `badness-ignore` directive family,
//! carried inside `@comment{…}` entries.
//!
//! BibTeX has no line-comment token (free text outside an entry lexes as `JUNK`),
//! so unlike the LaTeX side (`% badness-ignore …` in a `COMMENT` token) the
//! carrier is a structured `@comment` entry — a [`COMMENT_ENTRY`] node:
//!
//! ```text
//! @comment{badness-ignore <rule>: <reason>}        suppress <rule> on the next entry
//! @comment{badness-ignore-file <rule>: <reason>}   suppress <rule> file-wide
//! @comment{badness-ignore-file: <reason>}          suppress ALL rules file-wide
//! ```
//!
//! The bib analog of [`crate::linter::suppression`]; the directive grammar and the
//! `is_suppressed` range test are identical, only the carrier (a `@comment` entry
//! instead of a `%` comment) and the next-sibling attachment differ.
//!
//! [`COMMENT_ENTRY`]: crate::bib::syntax::SyntaxKind::COMMENT_ENTRY

use std::collections::{HashMap, HashSet};

use rowan::NodeOrToken;

use crate::bib::syntax::{SyntaxKind, SyntaxNode};

#[derive(Debug, Clone, Default)]
pub struct BibSuppressionMap {
    /// Rule IDs suppressed file-wide (`@comment{badness-ignore-file <rule>: …}`).
    file_rules: HashSet<String>,
    /// Whether the file has a "suppress everything" directive.
    file_all: bool,
    /// `rule → byte ranges`. A diagnostic is suppressed if its `[start, end)`
    /// falls fully inside one of the registered ranges for its rule.
    node_skips: HashMap<String, Vec<(usize, usize)>>,
}

impl BibSuppressionMap {
    pub fn build(root: &SyntaxNode) -> Self {
        let mut map = Self::default();
        for node in root.descendants() {
            if node.kind() == SyntaxKind::COMMENT_ENTRY {
                classify_comment(&node, &mut map);
            }
        }
        map
    }

    /// Whether a `[start, end)` diagnostic for `rule` is suppressed.
    pub fn is_suppressed(&self, rule: &str, start: usize, end: usize) -> bool {
        if self.file_all {
            return true;
        }
        if self.file_rules.contains(rule) {
            return true;
        }
        if let Some(ranges) = self.node_skips.get(rule) {
            return ranges.iter().any(|(rs, re)| *rs <= start && end <= *re);
        }
        false
    }
}

fn classify_comment(node: &SyntaxNode, map: &mut BibSuppressionMap) {
    let Some(body) = comment_directive_text(node) else {
        return;
    };
    let body = body.trim();
    if let Some(rest) = body.strip_prefix("badness-ignore-file") {
        let rest = rest.trim_start();
        // `…-file:` (no rule) suppresses everything; `…-file <rule>:` one rule.
        if rest.starts_with(':') {
            map.file_all = true;
        } else if let Some(rule) = parse_rule(rest) {
            map.file_rules.insert(rule);
        }
        return;
    }
    if let Some(rest) = body.strip_prefix("badness-ignore")
        && let Some(rule) = parse_rule(rest.trim_start())
        && let Some(target) = next_meaningful_sibling(node)
    {
        map.node_skips.entry(rule).or_default().push(target);
    }
}

/// The inner text of a `@comment{…}` / `@comment(…)` entry — everything between
/// the opening and closing delimiter. Returns `None` if no delimiter pair is
/// found. Used only to read a directive, so nested braces (which never occur in a
/// `badness-ignore` line) need no special handling.
fn comment_directive_text(node: &SyntaxNode) -> Option<String> {
    let text = node.to_string();
    let open = text.find(['{', '('])?;
    let close = text.rfind(['}', ')'])?;
    if close <= open {
        return None;
    }
    Some(text[open + 1..close].to_string())
}

/// Read the leading `<rule>` token of `<rule>: <reason>` (or a bare `<rule>`).
fn parse_rule(rest: &str) -> Option<String> {
    let trimmed = rest.trim_start();
    let end = trimmed
        .find(|c: char| c == ':' || c.is_whitespace())
        .unwrap_or(trimmed.len());
    if end == 0 {
        return None;
    }
    Some(trimmed[..end].to_string())
}

/// The byte range of the next non-trivia block after the `@comment` directive,
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
            _ => {
                let range = element.text_range();
                return Some((usize::from(range.start()), usize::from(range.end())));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bib::parse;

    fn map_of(src: &str) -> BibSuppressionMap {
        BibSuppressionMap::build(&parse(src).syntax())
    }

    #[test]
    fn file_all_suppresses_everything() {
        let m = map_of("@comment{badness-ignore-file: noisy}\n@misc{k}\n");
        assert!(m.is_suppressed("anything", 0, 1));
    }

    #[test]
    fn file_rule_suppresses_only_that_rule() {
        let m = map_of("@comment{badness-ignore-file unused-string: legacy}\n");
        assert!(m.is_suppressed("unused-string", 0, 1));
        assert!(!m.is_suppressed("duplicate-key", 0, 1));
    }

    #[test]
    fn non_directive_comment_is_inert() {
        let m = map_of("@comment{just a note}\n@misc{k}\n");
        assert!(!m.is_suppressed("unused-string", 0, 1));
        assert!(!m.file_all);
    }

    #[test]
    fn node_directive_targets_following_entry() {
        let src = "@comment{badness-ignore empty-field: ok}\n@misc{k, title = {}}\n";
        let m = map_of(src);
        // The empty `title` field lives inside the following entry's range.
        let entry_start = src.find("@misc").unwrap();
        assert!(m.is_suppressed("empty-field", entry_start + 1, entry_start + 5));
        // A different rule at the same place is not suppressed.
        assert!(!m.is_suppressed("duplicate-key", entry_start + 1, entry_start + 5));
    }

    #[test]
    fn node_directive_does_not_leak_to_later_entries() {
        let src = "@comment{badness-ignore empty-field: ok}\n@misc{a, t = {}}\n@misc{b, t = {}}\n";
        let m = map_of(src);
        let second = src.rfind("@misc").unwrap();
        assert!(!m.is_suppressed("empty-field", second + 1, second + 5));
    }
}
