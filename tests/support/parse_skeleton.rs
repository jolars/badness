//! Shared projector for the differential parse oracle (`parse_oracle.rs`,
//! `parse_compat.rs`).
//!
//! badness's CST is deliberately *generic* (`\section`→`COMMAND`, environments by
//! structure, greedy argument attachment, `PARAGRAPH` wrappers, trivia inline).
//! texlab's CST is *semantically enriched* (`\section`→`SECTION`, `\cite`→
//! `CITATION`, args attached by known signature, no paragraph wrappers, its own
//! whitespace/text tokenization). The two can never be bitwise-equal — AGENTS.md
//! says as much ("measure against, never match"). So we project *both* rowan trees
//! onto one coarse, common skeleton and measure structural concordance as a triage
//! signal, never a hard gate.
//!
//! The skeleton keeps only what both layers agree on: the command / environment /
//! group / math / verbatim *skeleton*, with command and environment names but
//! **not** semantic role. Everything else (text runs, whitespace, comments,
//! punctuation, document/paragraph wrappers, name groups) is dropped, because that
//! is exactly where the two tokenizations legitimately diverge.

#![allow(dead_code)] // each test binary uses only part of this module.

use rowan::{Language, NodeOrToken, SyntaxNode};

/// One node in the common skeleton. A document projects to a `Vec<Atom>` forest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Atom {
    /// `\begin{name} … \end{name}` with its projected body.
    Env(String, Vec<Atom>),
    /// A control sequence (generic or semantic) with its projected arguments.
    Cmd(String, Vec<Atom>),
    /// A required `{ … }` group / argument.
    Group(Vec<Atom>),
    /// An optional `[ … ]` group / argument.
    Opt(Vec<Atom>),
    /// Inline or display math (the inline/display distinction is intentionally
    /// not modelled — the `$$` vs `\[` split differs across the two parsers).
    Math(Vec<Atom>),
    /// Protected verbatim content (`\verb`, verbatim-like environment bodies).
    Verbatim,
}

/// How a `SyntaxKind` maps onto the skeleton.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Cat {
    Env,
    Cmd,
    Group,
    Opt,
    Math,
    /// A node with no skeleton counterpart whose children we still descend into
    /// (document/paragraph/text wrappers).
    Transparent,
    /// A node we drop wholesale (e.g. `BEGIN`/`END`, handled by their `Env`).
    Drop,
}

/// Per-language classification + name extraction. One impl per parser.
pub trait Projector {
    type Lang: Language;

    fn cat(kind: <Self::Lang as Language>::Kind) -> Cat;
    /// True for the `BEGIN` / `END` child nodes of an environment.
    fn is_begin_or_end(kind: <Self::Lang as Language>::Kind) -> bool;
    /// True for the token carrying a control-sequence name (e.g. `\section`).
    fn is_command_token(kind: <Self::Lang as Language>::Kind) -> bool;
    /// True for an ordinary word token (used to read an environment's name).
    fn is_word_token(kind: <Self::Lang as Language>::Kind) -> bool;
    /// True for a verbatim/protected token.
    fn is_verbatim_token(kind: <Self::Lang as Language>::Kind) -> bool;
}

type Node<P> = SyntaxNode<<P as Projector>::Lang>;

/// Project a whole document (a root node) into the skeleton forest.
pub fn project<P: Projector>(root: &Node<P>) -> Vec<Atom> {
    project_node::<P>(root)
}

fn project_node<P: Projector>(node: &Node<P>) -> Vec<Atom> {
    match P::cat(node.kind()) {
        Cat::Cmd => vec![Atom::Cmd(
            command_name::<P>(node),
            project_children::<P>(node),
        )],
        Cat::Env => {
            let body = node
                .children_with_tokens()
                .filter(|e| match e {
                    NodeOrToken::Node(n) => !P::is_begin_or_end(n.kind()),
                    NodeOrToken::Token(_) => true,
                })
                .flat_map(|e| project_elem::<P>(&e))
                .collect();
            vec![Atom::Env(env_name::<P>(node), body)]
        }
        Cat::Group => vec![Atom::Group(project_children::<P>(node))],
        Cat::Opt => vec![Atom::Opt(project_children::<P>(node))],
        Cat::Math => vec![Atom::Math(project_children::<P>(node))],
        Cat::Transparent => project_children::<P>(node),
        Cat::Drop => vec![],
    }
}

fn project_children<P: Projector>(node: &Node<P>) -> Vec<Atom> {
    node.children_with_tokens()
        .flat_map(|e| project_elem::<P>(&e))
        .collect()
}

fn project_elem<P: Projector>(
    elem: &NodeOrToken<Node<P>, rowan::SyntaxToken<P::Lang>>,
) -> Vec<Atom> {
    match elem {
        NodeOrToken::Node(n) => project_node::<P>(n),
        NodeOrToken::Token(t) if P::is_verbatim_token(t.kind()) => vec![Atom::Verbatim],
        NodeOrToken::Token(_) => vec![],
    }
}

/// The control-sequence name of a command node, with the leading `\` stripped.
fn command_name<P: Projector>(node: &Node<P>) -> String {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| P::is_command_token(t.kind()))
        .map(|t| t.text().trim_start_matches('\\').to_string())
        .unwrap_or_default()
}

/// The environment name, read from the first word token inside its `BEGIN`.
fn env_name<P: Projector>(node: &Node<P>) -> String {
    node.descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| P::is_word_token(t.kind()))
        .map(|t| t.text().to_string())
        .unwrap_or_default()
}

// --- serialization & similarity ------------------------------------------

/// Render a skeleton forest as indented S-expression lines (one atom per line,
/// indentation encodes depth so structure is comparison-significant).
pub fn render_lines(forest: &[Atom]) -> Vec<String> {
    let mut out = Vec::new();
    for atom in forest {
        render_atom(atom, 0, &mut out);
    }
    out
}

fn render_atom(atom: &Atom, depth: usize, out: &mut Vec<String>) {
    let pad = "  ".repeat(depth);
    let (head, children): (String, &[Atom]) = match atom {
        Atom::Env(name, ch) => (format!("(env {name})"), ch),
        Atom::Cmd(name, ch) => (format!("(cmd {name})"), ch),
        Atom::Group(ch) => ("(group)".to_string(), ch),
        Atom::Opt(ch) => ("(opt)".to_string(), ch),
        Atom::Math(ch) => ("(math)".to_string(), ch),
        Atom::Verbatim => ("(verbatim)".to_string(), &[]),
    };
    out.push(format!("{pad}{head}"));
    for child in children {
        render_atom(child, depth + 1, out);
    }
}

/// Length of the longest common subsequence of two line slices. Copied from
/// arity's `air_compat.rs`.
pub fn lcs_len(a: &[String], b: &[String]) -> usize {
    if a.is_empty() || b.is_empty() {
        return 0;
    }
    let mut prev = vec![0usize; b.len() + 1];
    for line_a in a {
        let mut cur = vec![0usize; b.len() + 1];
        for (j, line_b) in b.iter().enumerate() {
            cur[j + 1] = if line_a == line_b {
                prev[j] + 1
            } else {
                cur[j].max(prev[j + 1])
            };
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Dice coefficient over skeleton lines: `2·LCS / (|a| + |b|)`, in `0.0..=1.0`.
pub fn dice(a: &[String], b: &[String]) -> f64 {
    let denom = a.len() + b.len();
    if denom == 0 {
        return 1.0;
    }
    2.0 * lcs_len(a, b) as f64 / denom as f64
}

// --- badness projector ----------------------------------------------------

pub enum Badness {}

impl Projector for Badness {
    type Lang = badness::syntax::BadnessLang;

    fn cat(kind: badness::syntax::SyntaxKind) -> Cat {
        use badness::syntax::SyntaxKind::*;
        match kind {
            COMMAND => Cat::Cmd,
            ENVIRONMENT => Cat::Env,
            GROUP => Cat::Group,
            OPTIONAL => Cat::Opt,
            INLINE_MATH | DISPLAY_MATH => Cat::Math,
            BEGIN | END | NAME_GROUP => Cat::Drop,
            // ROOT, PARAGRAPH, TEXT, MATH, ARGUMENT, and any stray token-bearing
            // node: descend without emitting a skeleton atom.
            _ => Cat::Transparent,
        }
    }

    fn is_begin_or_end(kind: badness::syntax::SyntaxKind) -> bool {
        use badness::syntax::SyntaxKind::{BEGIN, END};
        matches!(kind, BEGIN | END)
    }

    fn is_command_token(kind: badness::syntax::SyntaxKind) -> bool {
        use badness::syntax::SyntaxKind::{CONTROL_SYMBOL, CONTROL_WORD};
        matches!(kind, CONTROL_WORD | CONTROL_SYMBOL)
    }

    fn is_word_token(kind: badness::syntax::SyntaxKind) -> bool {
        kind == badness::syntax::SyntaxKind::WORD
    }

    fn is_verbatim_token(kind: badness::syntax::SyntaxKind) -> bool {
        use badness::syntax::SyntaxKind::{VERB, VERBATIM_BODY};
        matches!(kind, VERB | VERBATIM_BODY)
    }
}

/// Project badness's CST for `text`.
pub fn project_badness(text: &str) -> Vec<Atom> {
    let parsed = badness::parser::parse(text);
    project::<Badness>(&parsed.syntax())
}

// --- texlab projector -----------------------------------------------------

pub enum Texlab {}

impl Projector for Texlab {
    type Lang = texlab_syntax::latex::LatexLanguage;

    fn cat(kind: texlab_syntax::latex::SyntaxKind) -> Cat {
        use texlab_syntax::latex::SyntaxKind::*;
        match kind {
            ENVIRONMENT => Cat::Env,
            BEGIN | END => Cat::Drop,
            FORMULA | EQUATION => Cat::Math,
            CURLY_GROUP
            | CURLY_GROUP_WORD
            | CURLY_GROUP_WORD_LIST
            | CURLY_GROUP_COMMAND
            | CURLY_GROUP_KEY_VALUE => Cat::Group,
            BRACK_GROUP | BRACK_GROUP_WORD | BRACK_GROUP_KEY_VALUE | MIXED_GROUP => Cat::Opt,
            // Every command-like node (generic + semantic) collapses to `Cmd`,
            // dropping texlab's semantic role so it lines up with badness's
            // generic `COMMAND`.
            GENERIC_COMMAND
            | PART
            | CHAPTER
            | SECTION
            | SUBSECTION
            | SUBSUBSECTION
            | PARAGRAPH
            | SUBPARAGRAPH
            | ENUM_ITEM
            | CITATION
            | PACKAGE_INCLUDE
            | CLASS_INCLUDE
            | LATEX_INCLUDE
            | BIBLATEX_INCLUDE
            | BIBTEX_INCLUDE
            | GRAPHICS_INCLUDE
            | SVG_INCLUDE
            | INKSCAPE_INCLUDE
            | VERBATIM_INCLUDE
            | IMPORT
            | LABEL_DEFINITION
            | LABEL_REFERENCE
            | LABEL_REFERENCE_RANGE
            | OLD_COMMAND_DEFINITION
            | NEW_COMMAND_DEFINITION
            | MATH_OPERATOR
            | GLOSSARY_ENTRY_DEFINITION
            | GLOSSARY_ENTRY_REFERENCE
            | ACRONYM_DEFINITION
            | ACRONYM_DECLARATION
            | ACRONYM_REFERENCE
            | THEOREM_DEFINITION_AMSTHM
            | THEOREM_DEFINITION_THMTOOLS
            | COLOR_REFERENCE
            | COLOR_DEFINITION
            | COLOR_SET_DEFINITION
            | TIKZ_LIBRARY_IMPORT
            | ENVIRONMENT_DEFINITION
            | GRAPHICS_PATH
            | CAPTION
            | LABEL_NUMBER
            | BIBITEM
            | TOC_CONTENTS_LINE
            | TOC_NUMBER_LINE => Cat::Cmd,
            // ROOT, PREAMBLE, TEXT, KEY/VALUE wrappers, PAREN_GROUP (badness treats
            // parens as text), etc.: descend transparently.
            _ => Cat::Transparent,
        }
    }

    fn is_begin_or_end(kind: texlab_syntax::latex::SyntaxKind) -> bool {
        use texlab_syntax::latex::SyntaxKind::{BEGIN, END};
        matches!(kind, BEGIN | END)
    }

    fn is_command_token(kind: texlab_syntax::latex::SyntaxKind) -> bool {
        kind == texlab_syntax::latex::SyntaxKind::COMMAND_NAME
    }

    fn is_word_token(kind: texlab_syntax::latex::SyntaxKind) -> bool {
        use texlab_syntax::latex::SyntaxKind::{KEY, WORD};
        matches!(kind, WORD | KEY)
    }

    fn is_verbatim_token(kind: texlab_syntax::latex::SyntaxKind) -> bool {
        kind == texlab_syntax::latex::SyntaxKind::VERBATIM
    }
}

/// Project texlab's CST for `text`.
pub fn project_texlab(text: &str) -> Vec<Atom> {
    let green = texlab_parser::parse_latex(text, &texlab_parser::SyntaxConfig::default());
    let root = texlab_syntax::latex::SyntaxNode::new_root(green);
    project::<Texlab>(&root)
}

/// True if texlab parsed `text` with an `ERROR` node anywhere in the tree.
pub fn texlab_has_error(text: &str) -> bool {
    let green = texlab_parser::parse_latex(text, &texlab_parser::SyntaxConfig::default());
    let root = texlab_syntax::latex::SyntaxNode::new_root(green);
    root.descendants_with_tokens()
        .any(|e| e.kind() == texlab_syntax::latex::SyntaxKind::ERROR)
}
