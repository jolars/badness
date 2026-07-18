//! Comment-based suppression: the `% badness-ignore` directive family.
//!
//! Three forms are recognized (LaTeX line comments, so `%` not `#`):
//!
//! ```text
//! % badness-ignore <rule>: <reason>        suppress <rule> on the next meaningful sibling
//! % badness-ignore-file <rule>: <reason>   suppress <rule> file-wide
//! % badness-ignore-file: <reason>          suppress ALL rules file-wide
//! ```
//!
//! Byte ranges are plain `usize` offsets (the
//! [`Diagnostic`](super::Diagnostic) stores plain offsets, not a rowan
//! `TextRange`), matched against LaTeX comment syntax. The comment-to-node attachment
//! for a node-level suppression is "next non-trivia sibling", computed during the
//! walk — no `place_comment` indirection.

use std::collections::{HashMap, HashSet};

use rowan::NodeOrToken;

use crate::syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

#[derive(Debug, Clone, Default)]
pub struct SuppressionMap {
    /// Rule IDs suppressed file-wide (`% badness-ignore-file <rule>: …`).
    file_rules: HashSet<String>,
    /// Whether the file has a "suppress everything" directive.
    file_all: bool,
    /// `rule → byte ranges`. A diagnostic is suppressed if its `[start, end)`
    /// falls fully inside one of the registered ranges for its rule.
    node_skips: HashMap<String, Vec<(usize, usize)>>,
}

impl SuppressionMap {
    pub fn build(root: &SyntaxNode) -> Self {
        let mut map = Self::default();
        for element in root.descendants_with_tokens() {
            if let NodeOrToken::Token(token) = element
                && token.kind() == SyntaxKind::COMMENT
            {
                classify_comment(&token, &mut map);
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

fn classify_comment(token: &SyntaxToken, map: &mut SuppressionMap) {
    let body = match token.text().strip_prefix('%') {
        Some(rest) => rest.trim_start(),
        None => return,
    };
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
        && let Some(target) = next_meaningful_sibling(token)
    {
        map.node_skips.entry(rule).or_default().push(target);
    }
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

/// The byte range of the next non-trivia, non-comment element after `token`,
/// bubbling up through parents whose remaining siblings are all trivia (e.g. a
/// comment on its own line under `ROOT`, whose target is the next block).
fn next_meaningful_sibling(token: &SyntaxToken) -> Option<(usize, usize)> {
    // A comment bound into a `DOC_COMMENT` node is always the *leading child*
    // of the `COMMAND`/`ENVIRONMENT` construct it documents, not a sibling
    // before it. Walking forward from the comment token only ever finds pieces
    // *inside* that same construct (e.g. just its control word, missing the
    // construct's own arguments), never the construct as a whole. Target the
    // whole construct directly in that case.
    if let Some(parent) = token.parent()
        && parent.kind() == SyntaxKind::DOC_COMMENT
    {
        let construct = parent.parent()?;
        let range = construct.text_range();
        return Some((usize::from(range.start()), usize::from(range.end())));
    }
    let mut current = token.clone();
    loop {
        let parent = current.parent()?;
        if let Some(range) = first_meaningful_after(&parent, &NodeOrToken::Token(current.clone())) {
            return Some(range);
        }
        // Nothing after `current` in `parent`: retry against `parent`'s own
        // following siblings one level up.
        let grand = parent.parent()?;
        if let Some(range) = first_meaningful_after(&grand, &NodeOrToken::Node(parent.clone())) {
            return Some(range);
        }
        current = grand.first_token()?;
        // Guard against a non-progressing climb (a single-child spine).
        if grand == parent {
            return None;
        }
    }
}

/// Scan `parent`'s children for the first non-trivia element strictly after
/// `after`, returning its byte range as `(start, end)`.
fn first_meaningful_after(
    parent: &SyntaxNode,
    after: &NodeOrToken<SyntaxNode, SyntaxToken>,
) -> Option<(usize, usize)> {
    let mut past = false;
    for element in parent.children_with_tokens() {
        if !past {
            if &element == after {
                past = true;
            }
            continue;
        }
        match &element {
            NodeOrToken::Token(t)
                if matches!(
                    t.kind(),
                    SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
                ) => {}
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
    use crate::parser::parse;

    fn map_of(src: &str) -> SuppressionMap {
        SuppressionMap::build(&SyntaxNode::new_root(parse(src).green))
    }

    #[test]
    fn file_all_suppresses_everything() {
        let m = map_of("% badness-ignore-file: noisy\n\\bf\n");
        assert!(m.is_suppressed("anything", 0, 1));
    }

    #[test]
    fn file_rule_suppresses_only_that_rule() {
        let m = map_of("% badness-ignore-file deprecated-command: legacy\n\\bf\n");
        assert!(m.is_suppressed("deprecated-command", 0, 1));
        assert!(!m.is_suppressed("duplicate-label", 0, 1));
    }

    #[test]
    fn non_directive_comment_is_inert() {
        let m = map_of("% just a note\n\\bf\n");
        assert!(!m.is_suppressed("deprecated-command", 0, 1));
        assert!(!m.file_all);
    }
}
