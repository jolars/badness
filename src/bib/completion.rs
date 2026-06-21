//! Bib code completion: entry types after `@`, field names per entry type, and
//! `@string` macro names in value position, classified from the cursor's position
//! in the BibTeX CST.
//!
//! The bib analog of [`crate::completion`], and the same shape: a pure
//! [`classify_bib_context`] over the parse tree + a byte offset, then
//! [`bib_candidates`] turns the context into a neutral candidate list. The LSP layer
//! (`crate::lsp`) only maps candidates onto `lsp_types::CompletionItem`s. Keeping the
//! logic here, free of LSP types, makes it unit-testable straight off [`crate::bib::parse`].
//!
//! Entry types and field names are drawn from the bib field/entry DB
//! ([`builtin`](crate::bib::semantic::builtin)); `@string` macro names from the
//! per-file [`Model`] (its `@string` definitions) plus the predefined month macros.
//! Cite *keys* are a `.tex`-side concern (see [`crate::completion`]); they are not
//! completed in `.bib` (a key is user-authored), so a key position yields
//! [`BibCompletionContext::None`].

use rowan::{TextSize, TokenAtOffset};

use std::collections::HashSet;

use crate::bib::ast;
use crate::bib::semantic::{MONTH_MACROS, Model, RequiredField, builtin};
use crate::bib::syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

/// What the cursor at a given offset is positioned to complete in a `.bib` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BibCompletionContext {
    /// Typing an entry-type word after `@` (the `@` stripped). `@art|`, `@|`.
    EntryType { prefix: String },
    /// A field-name position inside a regular entry body. `entry_type` is the
    /// enclosing entry's type, lowercased (`None` if unreadable). `present` is the
    /// set of field names already used in the entry (lowercased), so the candidate
    /// list can hide them.
    FieldName {
        entry_type: Option<String>,
        prefix: String,
        present: Vec<String>,
    },
    /// A bare-word value after `=` — a possible `@string` macro reference.
    ValueMacro { prefix: String },
    /// Nothing to complete here (a cite key, a braced/quoted value, prose, …).
    None,
}

/// The kind of a bib candidate, mapped to an LSP `CompletionItemKind` by the server
/// layer (kept LSP-type-free here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BibCandidateKind {
    EntryType,
    FieldName,
    StringMacro,
}

/// A completion candidate before it becomes an `lsp_types::CompletionItem`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BibCompletionCandidate {
    pub label: String,
    pub kind: BibCandidateKind,
}

/// Classify what the cursor at byte `offset` is positioned to complete.
pub fn classify_bib_context(root: &SyntaxNode, offset: usize) -> BibCompletionContext {
    let offset = TextSize::new(offset.min(u32::MAX as usize) as u32);
    let (left, right) = match root.token_at_offset(offset) {
        TokenAtOffset::None => return BibCompletionContext::None,
        TokenAtOffset::Single(t) => (Some(t.clone()), Some(t)),
        TokenAtOffset::Between(l, r) => (Some(l), Some(r)),
    };

    // A word being typed extends the token to the left of (or under) the cursor.
    for tok in [left.as_ref(), right.as_ref()].into_iter().flatten() {
        if let Some(ctx) = word_context(tok, offset) {
            return ctx;
        }
    }
    // Empty positions: a fresh field name, a fresh value, or just after `@`.
    for tok in [left.as_ref(), right.as_ref()].into_iter().flatten() {
        if let Some(ctx) = empty_context(tok, offset) {
            return ctx;
        }
    }
    BibCompletionContext::None
}

/// A context when `token` is a `WORD` the cursor sits within or just after — the
/// word's parent decides whether it is an entry type, a field name, or a value macro.
fn word_context(token: &SyntaxToken, offset: TextSize) -> Option<BibCompletionContext> {
    if token.kind() != SyntaxKind::WORD {
        return None;
    }
    let range = token.text_range();
    if offset <= range.start() || offset > range.end() {
        return None;
    }
    let rel = usize::from(offset - range.start());
    let prefix = token.text().get(..rel).unwrap_or(token.text()).to_string();
    let parent = token.parent()?;
    match parent.kind() {
        SyntaxKind::ENTRY_TYPE => Some(BibCompletionContext::EntryType { prefix }),
        SyntaxKind::FIELD_NAME => {
            let entry = enclosing_entry(token)?;
            // Only a regular entry has completable fields; `@string`/`@preamble`/
            // `@comment` names are user-defined or absent.
            if entry.kind() != SyntaxKind::ENTRY {
                return None;
            }
            let field = parent.parent();
            Some(BibCompletionContext::FieldName {
                entry_type: ast::entry_type(&entry).map(|t| t.to_lowercase()),
                prefix,
                present: present_field_names(&entry, field.as_ref()),
            })
        }
        // A `LITERAL` value piece is the only macro-reference position; a value inside
        // `{…}`/`"…"` lexes under `BRACE_GROUP`/`QUOTED`, not `LITERAL`.
        SyntaxKind::LITERAL if parent.parent().map(|p| p.kind()) == Some(SyntaxKind::VALUE) => {
            Some(BibCompletionContext::ValueMacro { prefix })
        }
        _ => None,
    }
}

/// A context at an *empty* position (no word typed yet): just after `@`, at a fresh
/// value (just after `=`), or at a fresh field name (just after a `,`).
fn empty_context(token: &SyntaxToken, offset: TextSize) -> Option<BibCompletionContext> {
    // Just after `@`, with no entry-type word yet.
    if token.kind() == SyntaxKind::AT
        && offset == token.text_range().end()
        && token.parent().is_some_and(|p| is_entry_node(p.kind()))
    {
        return Some(BibCompletionContext::EntryType {
            prefix: String::new(),
        });
    }

    let entry = enclosing_entry(token)?;
    // The nearest significant token before the cursor pins the position: `=` opens a
    // value, `,` opens a field. (A `{` opens the cite-key slot — not completed.)
    match last_significant_kind_before(&entry, offset)? {
        SyntaxKind::EQ => Some(BibCompletionContext::ValueMacro {
            prefix: String::new(),
        }),
        SyntaxKind::COMMA if entry.kind() == SyntaxKind::ENTRY => {
            Some(BibCompletionContext::FieldName {
                entry_type: ast::entry_type(&entry).map(|t| t.to_lowercase()),
                prefix: String::new(),
                present: present_field_names(&entry, None),
            })
        }
        _ => None,
    }
}

/// Whether `kind` is one of the four entry node kinds.
fn is_entry_node(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::ENTRY
            | SyntaxKind::STRING_ENTRY
            | SyntaxKind::PREAMBLE_ENTRY
            | SyntaxKind::COMMENT_ENTRY
    )
}

/// The nearest ancestor entry node of `token`, or `None` (cursor outside any entry).
fn enclosing_entry(token: &SyntaxToken) -> Option<SyntaxNode> {
    token
        .parent()?
        .ancestors()
        .find(|n| is_entry_node(n.kind()))
}

/// The kind of the last non-trivia token of `entry` ending at or before `offset`.
fn last_significant_kind_before(entry: &SyntaxNode, offset: TextSize) -> Option<SyntaxKind> {
    entry
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| {
            !matches!(t.kind(), SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE)
                && t.text_range().end() <= offset
        })
        .last()
        .map(|t| t.kind())
}

/// The lowercased field names already present in `entry`, excluding `exclude` (the
/// field the cursor is editing, so a partially-typed name does not hide itself).
fn present_field_names(entry: &SyntaxNode, exclude: Option<&SyntaxNode>) -> Vec<String> {
    let mut names = Vec::new();
    for field in ast::fields(entry) {
        if let Some(ex) = exclude
            && &field == ex
        {
            continue;
        }
        if let Some(name) = ast::field_name(&field) {
            names.push(name.to_lowercase());
        }
    }
    names
}

/// Build candidates for a bib `context`. [`BibCompletionContext::None`] yields none.
pub fn bib_candidates(
    context: &BibCompletionContext,
    model: &Model,
) -> Vec<BibCompletionCandidate> {
    match context {
        BibCompletionContext::EntryType { prefix } => entry_type_candidates(prefix),
        BibCompletionContext::FieldName {
            entry_type,
            prefix,
            present,
        } => field_name_candidates(entry_type.as_deref(), prefix, present),
        BibCompletionContext::ValueMacro { prefix } => value_macro_candidates(model, prefix),
        BibCompletionContext::None => Vec::new(),
    }
}

/// Known entry types plus the three reserved forms (`@string`/`@comment`/`@preamble`,
/// absent from the data-model DB), prefix-filtered, sorted, deduped.
fn entry_type_candidates(prefix: &str) -> Vec<BibCompletionCandidate> {
    const RESERVED: [&str; 3] = ["string", "comment", "preamble"];
    let prefix = prefix.to_lowercase();
    let mut names: Vec<String> = builtin()
        .entry_names()
        .chain(RESERVED)
        .filter(|name| name.starts_with(&prefix))
        .map(str::to_string)
        .collect();
    names.sort();
    names.dedup();
    candidates(names, BibCandidateKind::EntryType)
}

/// The fields for `entry_type` (its required ∪ optional set), or — when the type is
/// unknown — the global known-field set, minus the `present` fields, prefix-filtered.
fn field_name_candidates(
    entry_type: Option<&str>,
    prefix: &str,
    present: &[String],
) -> Vec<BibCompletionCandidate> {
    let prefix = prefix.to_lowercase();
    let present: HashSet<&str> = present.iter().map(String::as_str).collect();
    let db = builtin();
    let mut names: Vec<String> = match entry_type.and_then(|t| db.entry(t)) {
        Some(sig) => required_field_names(&sig.required)
            .chain(sig.optional.iter().map(smol_str::SmolStr::as_str))
            .map(str::to_string)
            .collect(),
        None => db.field_names().map(str::to_string).collect(),
    };
    names.retain(|name| name.starts_with(&prefix) && !present.contains(name.as_str()));
    names.sort();
    names.dedup();
    candidates(names, BibCandidateKind::FieldName)
}

/// In-file `@string` definitions plus the predefined month macros, prefix-filtered.
fn value_macro_candidates(model: &Model, prefix: &str) -> Vec<BibCompletionCandidate> {
    let prefix = prefix.to_lowercase();
    let mut names: Vec<String> = model
        .string_defs()
        .iter()
        .map(|def| def.name.to_string())
        .chain(MONTH_MACROS.iter().map(|m| (*m).to_string()))
        .filter(|name| name.starts_with(&prefix))
        .collect();
    names.sort();
    names.dedup();
    candidates(names, BibCandidateKind::StringMacro)
}

/// Flatten a `required` list into its constituent field names (both members of an
/// `OneOf` alternative are offered).
fn required_field_names(required: &[RequiredField]) -> impl Iterator<Item = &str> {
    required
        .iter()
        .flat_map(|req| match req {
            RequiredField::One(name) => std::slice::from_ref(name),
            RequiredField::OneOf(names) => names.as_slice(),
        })
        .map(smol_str::SmolStr::as_str)
}

/// Wrap labels in candidates of a single `kind`.
fn candidates(names: Vec<String>, kind: BibCandidateKind) -> Vec<BibCompletionCandidate> {
    names
        .into_iter()
        .map(|label| BibCompletionCandidate { label, kind })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bib::parse;

    fn root(src: &str) -> SyntaxNode {
        parse(src).syntax()
    }

    /// Byte offset just past `needle` in `src`.
    fn at(src: &str, needle: &str) -> usize {
        src.find(needle).expect("needle present") + needle.len()
    }

    fn classify(src: &str, offset: usize) -> BibCompletionContext {
        classify_bib_context(&root(src), offset)
    }

    /// Candidate labels for a source + cursor, using the file's own `@string` model.
    fn cands(src: &str, offset: usize) -> Vec<String> {
        let tree = root(src);
        let ctx = classify_bib_context(&tree, offset);
        let model = Model::build(&tree);
        bib_candidates(&ctx, &model)
            .into_iter()
            .map(|c| c.label)
            .collect()
    }

    #[test]
    fn entry_type_prefix_classified() {
        let src = "@art";
        assert_eq!(
            classify(src, at(src, "@art")),
            BibCompletionContext::EntryType {
                prefix: "art".to_string()
            }
        );
    }

    #[test]
    fn lone_at_offers_entry_types() {
        let src = "@";
        assert_eq!(
            classify(src, at(src, "@")),
            BibCompletionContext::EntryType {
                prefix: String::new()
            }
        );
        let got = cands(src, at(src, "@"));
        assert!(got.contains(&"article".to_string()), "{got:?}");
    }

    #[test]
    fn entry_type_candidates_prefix_filtered() {
        let src = "@inpro";
        let got = cands(src, at(src, "@inpro"));
        assert!(got.contains(&"inproceedings".to_string()), "{got:?}");
        assert!(got.iter().all(|n| n.starts_with("inpro")), "{got:?}");
    }

    #[test]
    fn reserved_types_offered() {
        let src = "@s";
        let got = cands(src, at(src, "@s"));
        assert!(got.contains(&"string".to_string()), "{got:?}");
    }

    #[test]
    fn field_name_in_entry_classified() {
        let src = "@article{k, au}\n";
        assert_eq!(
            classify(src, at(src, "@article{k, au")),
            BibCompletionContext::FieldName {
                entry_type: Some("article".to_string()),
                prefix: "au".to_string(),
                present: Vec::new(),
            }
        );
    }

    #[test]
    fn field_candidates_scoped_to_type() {
        let src = "@article{k, au}\n";
        let got = cands(src, at(src, "@article{k, au"));
        assert!(got.contains(&"author".to_string()), "{got:?}");
        assert!(got.iter().all(|n| n.starts_with("au")), "{got:?}");
    }

    #[test]
    fn field_name_empty_after_comma() {
        let src = "@article{k, }\n";
        let got = cands(src, at(src, "@article{k, "));
        assert!(got.contains(&"author".to_string()), "{got:?}");
        assert!(got.contains(&"title".to_string()), "{got:?}");
    }

    #[test]
    fn field_candidates_exclude_present() {
        let src = "@article{k, author = {x}, au}\n";
        let got = cands(src, at(src, "author = {x}, au"));
        assert!(!got.contains(&"author".to_string()), "{got:?}");
    }

    #[test]
    fn field_candidates_unknown_type_fallback() {
        let src = "@bogustype{k, au}\n";
        let got = cands(src, at(src, "@bogustype{k, au"));
        // Falls back to the global field set, which includes `author`.
        assert!(got.contains(&"author".to_string()), "{got:?}");
    }

    #[test]
    fn post_open_brace_is_key_not_field() {
        let src = "@article{}\n";
        assert_eq!(
            classify(src, at(src, "@article{")),
            BibCompletionContext::None
        );
    }

    #[test]
    fn string_entry_name_not_field() {
        let src = "@string{na}\n";
        assert_eq!(
            classify(src, at(src, "@string{na")),
            BibCompletionContext::None
        );
    }

    #[test]
    fn value_macro_after_eq() {
        let src = "@article{k, journal = }\n";
        assert_eq!(
            classify(src, at(src, "journal = ")),
            BibCompletionContext::ValueMacro {
                prefix: String::new()
            }
        );
    }

    #[test]
    fn value_macro_bare_word() {
        let src = "@article{k, publisher = els}\n";
        assert_eq!(
            classify(src, at(src, "publisher = els")),
            BibCompletionContext::ValueMacro {
                prefix: "els".to_string()
            }
        );
    }

    #[test]
    fn value_macro_candidates_include_months_and_defs() {
        let src = "@string{els = {Elsevier}}\n@article{k, publisher = e}\n";
        let got = cands(src, at(src, "publisher = e"));
        assert!(got.contains(&"els".to_string()), "{got:?}");

        let src = "@article{k, month = j}\n";
        let got = cands(src, at(src, "month = j"));
        assert!(got.contains(&"jan".to_string()), "{got:?}");
        assert!(got.contains(&"jun".to_string()), "{got:?}");
    }

    #[test]
    fn value_in_braces_not_macro() {
        let src = "@article{k, title = {els}}\n";
        assert_eq!(
            classify(src, at(src, "title = {els")),
            BibCompletionContext::None
        );
    }

    #[test]
    fn number_value_not_macro() {
        let src = "@article{k, year = 20}\n";
        assert_eq!(
            classify(src, at(src, "year = 20")),
            BibCompletionContext::None
        );
    }
}
