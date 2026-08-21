//! Effective text/math mode over a lossless syntax tree.

use rowan::{TextRange, TextSize};

use crate::ast::command_name;
use crate::semantic::define::scan_definitions;
use crate::semantic::signature::{ArgKind, ArgumentDomain, Signatures, match_arg_slot};
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};

/// The effective mode at a source position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Math,
    Text,
    Unknown,
}

impl From<ArgumentDomain> for Mode {
    fn from(domain: ArgumentDomain) -> Self {
        match domain {
            ArgumentDomain::Math => Self::Math,
            ArgumentDomain::Text => Self::Text,
            ArgumentDomain::Unknown => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ModeRange {
    range: TextRange,
    mode: Mode,
}

/// A sorted, coalesced partition of the document's token ranges by effective
/// mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeIndex {
    ranges: Vec<ModeRange>,
}

/// Return the curated positional domain of an attached argument group.
/// Unowned, unmatched, over-attached, and unknown-owner groups are unknown.
pub fn argument_domain(group: &SyntaxNode) -> ArgumentDomain {
    if !matches!(group.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL) {
        return ArgumentDomain::Unknown;
    }
    let Some(owner) = group.parent() else {
        return ArgumentDomain::Unknown;
    };
    let root = owner.ancestors().last().unwrap_or_else(|| owner.clone());
    let user = scan_definitions(&root);
    let signatures = Signatures::new(&user);
    let args = match owner.kind() {
        SyntaxKind::COMMAND => command_name(&owner)
            .and_then(|name| signatures.command(&name))
            .map(|sig| sig.args.as_ref()),
        SyntaxKind::BEGIN => environment_name(&owner)
            .as_deref()
            .and_then(|name| signatures.environment(name))
            .map(|sig| sig.args.as_ref()),
        _ => None,
    };
    let mut slot = 0usize;
    for candidate in owner
        .children()
        .filter(|child| matches!(child.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL))
    {
        let kind = if candidate.kind() == SyntaxKind::OPTIONAL {
            ArgKind::Bracket
        } else {
            ArgKind::Brace
        };
        let domain = args
            .and_then(|args| match_arg_slot(args, &mut slot, kind))
            .map_or(ArgumentDomain::Unknown, |spec| spec.domain);
        if candidate == *group {
            return domain;
        }
    }
    ArgumentDomain::Unknown
}

impl ModeIndex {
    pub fn build(root: &SyntaxNode) -> Self {
        let mut ranges = Vec::new();
        let user = scan_definitions(root);
        let signatures = Signatures::new(&user);
        walk(root, Mode::Text, &signatures, &mut ranges);
        Self { ranges }
    }

    /// Return the mode of the token containing `offset`.
    ///
    /// Offsets outside the document, including EOF, have no token and are
    /// conservatively unknown.
    pub fn mode_at(&self, offset: usize) -> Mode {
        let offset = TextSize::from(offset as u32);
        match self
            .ranges
            .binary_search_by(|entry| entry.range.start().cmp(&offset))
        {
            Ok(i) => self.ranges[i].mode,
            Err(0) => Mode::Unknown,
            Err(i) => {
                if self.ranges[i - 1].range.contains(offset) {
                    self.ranges[i - 1].mode
                } else {
                    Mode::Unknown
                }
            }
        }
    }
}

fn walk(
    node: &SyntaxNode,
    inherited: Mode,
    signatures: &Signatures<'_>,
    ranges: &mut Vec<ModeRange>,
) {
    let mode = match node.kind() {
        SyntaxKind::MATH => Mode::Math,
        SyntaxKind::NAME_GROUP => Mode::Unknown,
        _ => inherited,
    };

    let argument_modes = match node.kind() {
        SyntaxKind::COMMAND => command_argument_modes(node, signatures),
        SyntaxKind::BEGIN => environment_argument_modes(node, signatures),
        _ => Vec::new(),
    };
    let mut argument = 0usize;

    for element in node.children_with_tokens() {
        match element {
            SyntaxElement::Token(token) => push_range(ranges, token.text_range(), mode),
            SyntaxElement::Node(child) => {
                let child_mode = if matches!(child.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL)
                    && matches!(node.kind(), SyntaxKind::COMMAND | SyntaxKind::BEGIN)
                {
                    let mode = argument_modes
                        .get(argument)
                        .copied()
                        .unwrap_or(Mode::Unknown);
                    argument += 1;
                    mode
                } else {
                    mode
                };
                walk(&child, child_mode, signatures, ranges);
            }
        }
    }
}

fn command_argument_modes(node: &SyntaxNode, signatures: &Signatures<'_>) -> Vec<Mode> {
    let args = command_name(node)
        .and_then(|name| signatures.command(&name))
        .map(|sig| sig.args.as_ref());
    match_groups(node, args, false)
}

fn environment_argument_modes(begin: &SyntaxNode, signatures: &Signatures<'_>) -> Vec<Mode> {
    let name = environment_name(begin);
    let args = name
        .as_deref()
        .and_then(|name| signatures.environment(name))
        .map(|sig| sig.args.as_ref());
    match_groups(begin, args, false)
}

fn environment_name(begin: &SyntaxNode) -> Option<String> {
    begin
        .children()
        .find(|child| child.kind() == SyntaxKind::NAME_GROUP)
        .map(|group| group.text().to_string())
        .and_then(|text| text.strip_prefix('{')?.strip_suffix('}').map(str::to_owned))
}

fn match_groups(
    node: &SyntaxNode,
    args: Option<&[crate::semantic::ArgSpec]>,
    skip_first: bool,
) -> Vec<Mode> {
    let mut slot = 0usize;
    let mut first = skip_first;
    node.children()
        .filter(|child| matches!(child.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL))
        .map(|group| {
            if first {
                first = false;
                return Mode::Unknown;
            }
            let kind = if group.kind() == SyntaxKind::OPTIONAL {
                ArgKind::Bracket
            } else {
                ArgKind::Brace
            };
            args.and_then(|args| match_arg_slot(args, &mut slot, kind))
                .map_or(Mode::Unknown, |spec| spec.domain.into())
        })
        .collect()
}

fn push_range(ranges: &mut Vec<ModeRange>, range: TextRange, mode: Mode) {
    if range.is_empty() {
        return;
    }
    if let Some(last) = ranges.last_mut()
        && last.mode == mode
        && last.range.end() == range.start()
    {
        last.range = TextRange::new(last.range.start(), range.end());
    } else {
        ranges.push(ModeRange { range, mode });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn modes(source: &str, needles: &[&str]) -> Vec<Mode> {
        let parsed = parse(source);
        needles
            .iter()
            .scan(0usize, |from, needle| {
                let offset = source[*from..].find(needle).unwrap() + *from;
                *from = offset + needle.len();
                Some(ModeIndex::build(&parsed.syntax()).mode_at(offset))
            })
            .collect()
    }

    #[test]
    fn nested_domains_override_ambient_mode() {
        assert_eq!(
            modes(
                r"$a \frac{b \text{c $d$}}{e} \unknown{f}$",
                &["a", "b", "c", "d", "e", "f"]
            ),
            vec![
                Mode::Math,
                Mode::Math,
                Mode::Text,
                Mode::Math,
                Mode::Math,
                Mode::Unknown
            ]
        );
    }

    #[test]
    fn omitted_optional_slots_keep_their_positions() {
        assert_eq!(
            modes(r"\sqrt{x}\sqrt[n]{y}", &["x", "n", "y"]),
            vec![Mode::Math; 3]
        );
    }

    #[test]
    fn known_math_arguments_work_in_text_and_unknown_arguments_override_math() {
        assert_eq!(
            modes(
                r"before \ensuremath{x_i} $\foo{x_i}$",
                &["before", "x_i", "x_i"]
            ),
            vec![Mode::Text, Mode::Math, Mode::Unknown]
        );
    }

    #[test]
    fn redefined_builtin_arguments_are_unknown() {
        assert_eq!(
            modes(
                r"\renewcommand{\text}[1]{\ensuremath{#1}} $\text{5-10}$",
                &["5-10"]
            ),
            vec![Mode::Unknown]
        );
    }

    #[test]
    fn prose_arguments_establish_text_mode() {
        assert_eq!(
            modes(
                r"$\textbf{pages 5-10} \section[short 5-10]{pages 5-10} \footnote{pages 5-10}$",
                &["pages 5-10", "short 5-10", "pages 5-10", "pages 5-10"]
            ),
            vec![Mode::Text; 4]
        );
    }

    #[test]
    fn environment_name_and_header_are_not_body_math() {
        assert_eq!(
            modes(r"\begin{align}[t]x\end{align}", &["align", "t", "x"]),
            vec![Mode::Unknown, Mode::Unknown, Mode::Math]
        );
    }

    #[test]
    fn boundaries_belong_to_the_token_starting_there() {
        let parsed = parse(r"a\ensuremath{b}c");
        let index = ModeIndex::build(&parsed.syntax());
        assert_eq!(index.mode_at(0), Mode::Text);
        assert_eq!(index.mode_at(13), Mode::Math);
        assert_eq!(index.mode_at(15), Mode::Text);
        assert_eq!(index.mode_at(16), Mode::Unknown);
    }
}
