//! Shared projector for the BibTeX differential parse oracle (`bib_parse_oracle.rs`,
//! `bib_parse_compat.rs`).
//!
//! Like the LaTeX `parse_skeleton`, this projects both badness's and texlab's bib CSTs
//! onto one coarse, common skeleton and measures structural concordance as a triage
//! signal, never a hard tree-equality gate. AGENTS.md: measure against, never match.
//!
//! Two facts shape the bib oracle differently from the LaTeX one:
//!
//! 1. **texlab's BibTeX parser has no error channel.** Unlike its LaTeX parser (which
//!    emits `ERROR` nodes), the bib parser is fully lenient — a missing `}` / `=` / key
//!    is silently skipped via `expect`, and there is no `ERROR` kind at all. So the
//!    LaTeX-style "texlab must not error" soundness gate is vacuous here. The hard gate
//!    instead checks an **entry-recognition floor** (see [`count_entries`]).
//! 2. **The two parsers genuinely disagree inside values.** texlab models accents
//!    (`ACCENT`), commands (`COMMAND`), and `~` (`NBSP`) inside braces/quotes; badness
//!    keeps value internals generic. That divergence is *expected*, so the skeleton
//!    deliberately stops at the field level: it keeps the entry type and field names —
//!    the structure both parsers should agree on — and drops value internals, cite-key
//!    text, junk, and comments.

#![allow(dead_code)] // each test binary uses only part of this module.

use badness::bib;
use texlab_syntax::bibtex;

/// One node in the common bib skeleton. A document projects to a `Vec<BibAtom>` forest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BibAtom {
    /// `@type{ key, fields }`: the entry type (lowercased, `@` stripped) and its field
    /// atoms. Cite-key text and value internals are intentionally not modeled.
    Entry(String, Vec<BibAtom>),
    /// `@string{ name = value }` — a macro definition (the name/value are dropped; only
    /// its presence and position matter for concordance).
    StringDef,
    /// `@preamble{ value }`.
    Preamble,
    /// `name = value` — the field name, lowercased. The value is intentionally dropped.
    Field(String),
}

// --- badness projector ----------------------------------------------------

/// Project badness's bib CST for `text` onto the skeleton forest.
pub fn project_badness(text: &str) -> Vec<BibAtom> {
    use bib::syntax::SyntaxKind::*;
    let root = bib::parse(text).syntax();
    let mut out = Vec::new();
    for node in root.children() {
        match node.kind() {
            ENTRY => out.push(BibAtom::Entry(
                badness_entry_type(&node),
                badness_fields(&node),
            )),
            STRING_ENTRY => out.push(BibAtom::StringDef),
            PREAMBLE_ENTRY => out.push(BibAtom::Preamble),
            // COMMENT_ENTRY and JUNK carry no structure both parsers agree on.
            _ => {}
        }
    }
    out
}

/// The entry-type word of a badness `ENTRY` node, lowercased.
fn badness_entry_type(entry: &bib::syntax::SyntaxNode) -> String {
    use bib::syntax::SyntaxKind::ENTRY_TYPE;
    entry
        .children()
        .find(|n| n.kind() == ENTRY_TYPE)
        .map(|n| n.text().to_string().trim().to_ascii_lowercase())
        .unwrap_or_default()
}

/// The `Field(name)` atoms of a badness `ENTRY` node.
fn badness_fields(entry: &bib::syntax::SyntaxNode) -> Vec<BibAtom> {
    use bib::syntax::SyntaxKind::{FIELD, FIELD_NAME};
    entry
        .children()
        .filter(|n| n.kind() == FIELD)
        .map(|field| {
            let name = field
                .children()
                .find(|n| n.kind() == FIELD_NAME)
                .map(|n| n.text().to_string().trim().to_ascii_lowercase())
                .unwrap_or_default();
            BibAtom::Field(name)
        })
        .collect()
}

// --- texlab projector -----------------------------------------------------

/// Project texlab's bib CST for `text` onto the skeleton forest.
pub fn project_texlab(text: &str) -> Vec<BibAtom> {
    use bibtex::SyntaxKind::*;
    let root = texlab_root(text);
    let mut out = Vec::new();
    for node in root.children() {
        match node.kind() {
            ENTRY => out.push(BibAtom::Entry(
                texlab_entry_type(&node),
                texlab_fields(&node),
            )),
            STRING => out.push(BibAtom::StringDef),
            PREAMBLE => out.push(BibAtom::Preamble),
            _ => {}
        }
    }
    out
}

fn texlab_root(text: &str) -> bibtex::SyntaxNode {
    let green = texlab_parser::parse_bibtex(text);
    bibtex::SyntaxNode::new_root(green)
}

/// The entry-type word of a texlab `ENTRY` node. texlab keeps the leading `@` on the
/// `TYPE` token (e.g. `@article`), so strip it and lowercase.
fn texlab_entry_type(entry: &bibtex::SyntaxNode) -> String {
    use bibtex::SyntaxKind::TYPE;
    entry
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == TYPE)
        .map(|t| t.text().trim().trim_start_matches('@').to_ascii_lowercase())
        .unwrap_or_default()
}

/// The `Field(name)` atoms of a texlab `ENTRY` node. In texlab a field's name is the
/// first `NAME` token inside the `FIELD` node.
fn texlab_fields(entry: &bibtex::SyntaxNode) -> Vec<BibAtom> {
    use bibtex::SyntaxKind::{FIELD, NAME};
    entry
        .children()
        .filter(|n| n.kind() == FIELD)
        .map(|field| {
            let name = field
                .children_with_tokens()
                .filter_map(|e| e.into_token())
                .find(|t| t.kind() == NAME)
                .map(|t| t.text().trim().to_ascii_lowercase())
                .unwrap_or_default();
            BibAtom::Field(name)
        })
        .collect()
}

// --- entry-recognition floor (hard gate) ----------------------------------

/// Count the top-level structured entries (`Entry` / `StringDef` / `Preamble`) in a
/// skeleton forest. The hard gate requires texlab to recognize at least as many as
/// badness does for a badness-clean file: a one-way soundness floor that catches
/// badness hallucinating structure texlab does not see, without depending on an error
/// channel texlab's bib parser lacks.
pub fn count_entries(forest: &[BibAtom]) -> usize {
    forest
        .iter()
        .filter(|a| {
            matches!(
                a,
                BibAtom::Entry(..) | BibAtom::StringDef | BibAtom::Preamble
            )
        })
        .count()
}

// --- serialization & similarity -------------------------------------------

/// Render a skeleton forest as indented S-expression lines (one atom per line;
/// indentation encodes depth so structure is comparison-significant).
pub fn render_lines(forest: &[BibAtom]) -> Vec<String> {
    let mut out = Vec::new();
    for atom in forest {
        render_atom(atom, 0, &mut out);
    }
    out
}

fn render_atom(atom: &BibAtom, depth: usize, out: &mut Vec<String>) {
    let pad = "  ".repeat(depth);
    let (head, children): (String, &[BibAtom]) = match atom {
        BibAtom::Entry(ty, ch) => (format!("(entry {ty})"), ch),
        BibAtom::StringDef => ("(string)".to_string(), &[]),
        BibAtom::Preamble => ("(preamble)".to_string(), &[]),
        BibAtom::Field(name) => (format!("(field {name})"), &[]),
    };
    out.push(format!("{pad}{head}"));
    for child in children {
        render_atom(child, depth + 1, out);
    }
}

/// Length of the longest common subsequence of two line slices. Copied from
/// `parse_skeleton.rs` / arity's `air_compat.rs`.
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
