//! Name-based navigation targets: user **command names** (`\mycmd`) and
//! **environment names** (`\begin{myenv}`), the fallback tier of
//! references/rename/goto-definition behind the label/citation key resolution.
//!
//! Occurrences are collected by a request-time walk over the memoized parse tree,
//! not a salsa query: occurrence ranges shift on every edit above them, so an
//! Eq-keyed firewall would never backdate, and a whole-file token walk is sub-ms.
//! Definition sites come from [`scan_definition_sites`], the range-bearing sibling
//! of the signature scan. Tokens inside comments and verbatim bodies never parse
//! to `CONTROL_WORD`/`BEGIN`, so protected regions are skipped by construction.
//!
//! [`scan_definition_sites`]: crate::semantic::scan_definition_sites

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rowan::{TextRange, TextSize};
use smol_str::SmolStr;

use crate::ast::{environment_name, environment_name_range};
use crate::project::{PackageGraph, ResolvedLabels};
use crate::semantic::{DefSite, DefSiteKind};
use crate::syntax::{SyntaxKind, SyntaxNode};

/// Which TeX namespace a [`NameTarget`] lives in. Command and environment names
/// never collide: `\proof` and `\begin{proof}` are unrelated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NameKind {
    Command,
    Environment,
}

/// The command or environment name under the cursor: the bare name (no `\`), and
/// the span to report from `prepareRename` — the name text without the backslash,
/// so the prepare range, the placeholder, and the rename edits all agree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NameTarget {
    pub kind: NameKind,
    pub name: SmolStr,
    pub span: TextRange,
}

/// Resolve the name target at byte `offset`, trying both boundary tokens like
/// [`environment_pair_ranges`]. Precedence per token:
/// 1. any token inside a `BEGIN`/`END` (the `\begin` word, braces, or the name
///    itself) → the delimiter's environment name;
/// 2. a token inside the name argument of an environment *definition*
///    (`\newenvironment{myenv}`), matched by containment against `def_sites` —
///    plain `WORD` text a `BEGIN`/`END` walk can never see;
/// 3. a `CONTROL_WORD` → that command name (covers uses, the `\mycmd` inside
///    `\newcommand{\mycmd}`, and `\def\mycmd` alike).
///
/// A `CONTROL_SYMBOL` (`\\`, `\%`) is not a nameable target and falls through.
/// The caller runs label/citation resolution *first*; this is strictly the
/// fallback tier.
///
/// [`environment_pair_ranges`]: super::environment_pair_ranges
pub(crate) fn name_target_under_cursor(
    root: &SyntaxNode,
    offset: usize,
    def_sites: &[DefSite],
) -> Option<NameTarget> {
    let at = TextSize::new(offset.min(u32::MAX as usize) as u32);
    let (left, right) = match root.token_at_offset(at) {
        rowan::TokenAtOffset::None => return None,
        rowan::TokenAtOffset::Single(t) => (Some(t.clone()), Some(t)),
        rowan::TokenAtOffset::Between(l, r) => (Some(l), Some(r)),
    };
    let tokens: Vec<_> = [left, right].into_iter().flatten().collect();

    for token in &tokens {
        if let Some(delimiter) = token
            .parent_ancestors()
            .find(|n| matches!(n.kind(), SyntaxKind::BEGIN | SyntaxKind::END))
        {
            let name = environment_name(&delimiter)?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            return Some(NameTarget {
                kind: NameKind::Environment,
                name: SmolStr::new(name),
                span: environment_name_range(&delimiter)?,
            });
        }
    }

    if let Some(site) = def_sites
        .iter()
        .filter(|site| site.kind == DefSiteKind::Environment)
        .find(|site| site.name_range.contains_inclusive(at))
    {
        return Some(NameTarget {
            kind: NameKind::Environment,
            name: site.name.clone(),
            span: site.name_range,
        });
    }

    tokens
        .iter()
        .find(|token| token.kind() == SyntaxKind::CONTROL_WORD)
        .and_then(|token| {
            let name = token.text().strip_prefix('\\')?;
            (!name.is_empty()).then(|| NameTarget {
                kind: NameKind::Command,
                name: SmolStr::new(name),
                span: strip_backslash(token.text_range()),
            })
        })
}

/// Every `\name` control-word token in `root`, as full token ranges (backslash
/// included, matching [`DefSite::name_range`] for commands so declaration
/// classification can compare ranges for equality). Definition-site names
/// (`\newcommand{\name}`, `\def\name`) are themselves `CONTROL_WORD` tokens, so
/// the walk finds them too.
pub(crate) fn command_occurrences(root: &SyntaxNode, name: &str) -> Vec<TextRange> {
    root.descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .filter(|token| {
            token.kind() == SyntaxKind::CONTROL_WORD
                && token.text().strip_prefix('\\') == Some(name)
        })
        .map(|token| token.text_range())
        .collect()
}

/// Every `\begin{name}`/`\end{name}` name span in `root`. **Name-based, not
/// pair-based** — each matching delimiter is collected independently (unlike the
/// pair-based change-environment refactor), so unbalanced files degrade
/// gracefully. Definition-site names (`\newenvironment{name}`) are *not* found
/// here; the caller folds those in from [`scan_definition_sites`].
///
/// [`scan_definition_sites`]: crate::semantic::scan_definition_sites
pub(crate) fn environment_occurrences(root: &SyntaxNode, name: &str) -> Vec<TextRange> {
    root.descendants()
        .filter(|node| matches!(node.kind(), SyntaxKind::BEGIN | SyntaxKind::END))
        .filter(|node| environment_name(node).as_deref().map(str::trim) == Some(name))
        .filter_map(|node| environment_name_range(&node))
        .collect()
}

/// The file set a macro name is visible in: the include-graph component of
/// `origin` **plus package-load reachability in both directions**, as a BFS fixed
/// point. A `.sty` is usually a singleton include component, so the label
/// namespace alone would miss it: definitions live in local `.sty`/`.cls` files
/// reached via `\usepackage` (`loads`), and a rename started *inside* the `.sty`
/// must reach the documents loading it (`loaded_by`). Sorted for determinism.
pub(crate) fn macro_namespace(
    resolution: &ResolvedLabels,
    packages: &PackageGraph,
    origin: &Path,
) -> Vec<PathBuf> {
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    let mut queue = vec![origin.to_path_buf()];
    seen.insert(origin.to_path_buf());
    while let Some(path) = queue.pop() {
        let neighbors = resolution
            .namespace_members(&path)
            .into_iter()
            .map(Path::to_path_buf)
            .chain(packages.loads(&path).iter().map(|load| load.to.clone()))
            .chain(packages.loaded_by(&path).iter().cloned());
        for neighbor in neighbors {
            if seen.insert(neighbor.clone()) {
                queue.push(neighbor);
            }
        }
    }
    seen.into_iter().collect()
}

/// `range` with its leading backslash byte dropped — the bare-name span a rename
/// edit rewrites (the new name is inserted without a `\`, keeping the backslash
/// byte untouched).
pub(crate) fn strip_backslash(range: TextRange) -> TextRange {
    TextRange::new(
        (range.start() + TextSize::new(1)).min(range.end()),
        range.end(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use crate::semantic::scan_definition_sites;

    fn root_of(src: &str) -> SyntaxNode {
        SyntaxNode::new_root(parse(src).green)
    }

    fn target_at(src: &str, offset: usize) -> Option<NameTarget> {
        let root = root_of(src);
        let sites = scan_definition_sites(&root);
        name_target_under_cursor(&root, offset, &sites)
    }

    #[test]
    fn command_use_under_cursor() {
        let src = "text \\foo{x}\n";
        let target = target_at(src, 7).expect("a command target");
        assert_eq!(target.kind, NameKind::Command);
        assert_eq!(target.name, "foo");
        assert_eq!(&src[target.span], "foo");
    }

    #[test]
    fn command_name_inside_newcommand() {
        let src = "\\newcommand{\\foo}{x}\n";
        // Cursor on the `\foo` inside the name group.
        let target = target_at(src, 14).expect("a command target");
        assert_eq!(target.kind, NameKind::Command);
        assert_eq!(target.name, "foo");
    }

    #[test]
    fn environment_name_in_begin() {
        let src = "\\begin{myenv}\nx\n\\end{myenv}\n";
        let target = target_at(src, 9).expect("an environment target");
        assert_eq!(target.kind, NameKind::Environment);
        assert_eq!(target.name, "myenv");
        assert_eq!(&src[target.span], "myenv");
    }

    #[test]
    fn begin_control_word_resolves_to_environment() {
        let src = "\\begin{myenv}\nx\n\\end{myenv}\n";
        // Cursor on `\begin` itself: the delimiter names the environment.
        let target = target_at(src, 2).expect("an environment target");
        assert_eq!(target.kind, NameKind::Environment);
        assert_eq!(target.name, "myenv");
    }

    #[test]
    fn environment_definition_name_targets_environment() {
        let src = "\\newenvironment{myenv}{a}{b}\n";
        // Cursor inside the plain-text name argument.
        let target = target_at(src, 18).expect("an environment target");
        assert_eq!(target.kind, NameKind::Environment);
        assert_eq!(target.name, "myenv");
        assert_eq!(&src[target.span], "myenv");
    }

    #[test]
    fn control_symbol_and_prose_decline() {
        assert_eq!(target_at("a \\\\ b\n", 3), None);
        assert_eq!(target_at("plain text\n", 2), None);
    }

    #[test]
    fn command_occurrences_include_definition_and_uses() {
        let src = "\\newcommand{\\foo}{x}\n\\foo and \\foo{y} but not \\foobar\n";
        let ranges = command_occurrences(&root_of(src), "foo");
        assert_eq!(ranges.len(), 3);
        assert!(ranges.iter().all(|r| &src[*r] == "\\foo"));
    }

    #[test]
    fn command_occurrences_skip_verbatim_and_comments() {
        let src = "\\foo\n% \\foo in a comment\n\\begin{verbatim}\n\\foo\n\\end{verbatim}\n";
        let ranges = command_occurrences(&root_of(src), "foo");
        assert_eq!(ranges.len(), 1, "only the real token counts");
    }

    #[test]
    fn environment_occurrences_are_name_based() {
        // Unbalanced: two begins, one end — all three collected independently.
        let src = "\\begin{myenv}\n\\begin{myenv}\nx\n\\end{myenv}\n";
        let ranges = environment_occurrences(&root_of(src), "myenv");
        assert_eq!(ranges.len(), 3);
        assert!(ranges.iter().all(|r| &src[*r] == "myenv"));
    }
}
