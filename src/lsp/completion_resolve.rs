//! `completionItem/resolve` computation. Completion items ship lean — only
//! `label`/`kind`/`insert_text` — so the list stays cheap to build over a large
//! candidate universe (the CWL command tier, a whole bibliography). When the
//! client highlights an item it sends it back here, and we attach the expensive
//! detail lazily: a one-line `detail` shown inline and a markdown `documentation`
//! card.
//!
//! The item carries an opaque [`CompletionResolveData`] in its `data` field
//! (serialized when the item was built, echoed back verbatim by the client per
//! the LSP spec). We deserialize it and recompute against the snapshot, reusing
//! the *same* renderers `hover` uses ([`super::hover`]):
//!
//! - **Citation** → the resolved `.bib` entry's card (author/title/year), walked
//!   cross-file against the project bibliography like hover's `render_citation`.
//! - **Command / environment** → the synthesized signature prototype + facts,
//!   looked up scope-first (the document's own + package defs) then built-in then
//!   CWL.
//!
//! Items with no `data` (file paths, bib fields, labels) round-trip unchanged.
//! Like [`super::hover`], the read runs against the snapshot under
//! [`salsa::Cancelled::catch`] at the call site ([`super::run_completion_resolve`]).

use super::*;
use crate::bib::ast as bib_ast;
use crate::bib::syntax::{SyntaxKind as BibSyntaxKind, SyntaxNode as BibSyntaxNode};
use crate::semantic::signature::ArgSpec;
use lsp_types::{Documentation, MarkupContent, MarkupKind};
use serde::{Deserialize, Serialize};

/// The opaque payload carried in a [`CompletionItem`]'s `data` field, identifying
/// what the item is so resolve can recompute its detail. `#[serde(tag = "kind")]`
/// tags the variant so an unrelated `data` shape (a future item type) fails the
/// deserialize cleanly and resolves to the item unchanged.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub(crate) enum CompletionResolveData {
    /// A `\cite` key: the citing file (its bibliography namespace) and the key.
    Citation { lint_path: PathBuf, key: String },
    /// A command name plus the originating document (for scope-first lookup).
    Command { name: String, file: PathBuf },
    /// An environment name plus the originating document.
    Environment { name: String, file: PathBuf },
}

impl CompletionResolveData {
    /// Serialize into a [`CompletionItem::data`] value. `None` on the (practically
    /// impossible) serialize failure, so the caller just omits `data`.
    pub(crate) fn into_value(self) -> Option<serde_json::Value> {
        serde_json::to_value(self).ok()
    }
}

/// Enrich `item` with `detail`/`documentation` from its `data`, or return it
/// unchanged when there is no (recognized) payload.
pub(crate) fn resolve(
    snapshot: &Analysis,
    mut item: CompletionItem,
    members: Vec<ProjectMember>,
) -> CompletionItem {
    let Some(data) = item
        .data
        .clone()
        .and_then(|v| serde_json::from_value::<CompletionResolveData>(v).ok())
    else {
        return item;
    };

    let detail_doc = match data {
        CompletionResolveData::Citation { lint_path, key } => {
            citation_detail(snapshot, members, &lint_path, &key)
        }
        CompletionResolveData::Command { name, file } => {
            command_detail(snapshot, members, &file, &name)
        }
        CompletionResolveData::Environment { name, file } => {
            environment_detail(snapshot, members, &file, &name)
        }
    };

    if let Some((detail, documentation)) = detail_doc {
        item.detail = Some(detail);
        item.documentation = Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: documentation,
        }));
    }
    item
}

// --- Citation -----------------------------------------------------------------

/// Walk the citing file's bibliography namespace for the `@entry` matching `key`
/// and render `(detail, documentation)`: a compact `author (year)` line and the
/// full hover-style card. Mirrors hover's `render_citation` walk, but keeps the
/// entry node so it can build the inline detail too.
fn citation_detail(
    snapshot: &Analysis,
    members: Vec<ProjectMember>,
    lint_path: &Path,
    key: &str,
) -> Option<(String, String)> {
    let (_, citations) = snapshot.resolve_project(members);
    for bib_path in citations.bib_definers(lint_path) {
        let Some(file) = snapshot.lookup_file(bib_path) else {
            continue;
        };
        let Some(entry) = snapshot
            .bib_semantic_model(file)
            .entries()
            .iter()
            .find(|e| e.key.eq_ignore_ascii_case(key))
        else {
            continue;
        };
        let root = snapshot.parsed_bib_tree(file);
        let Some(node) = root
            .descendants()
            .find(|n| n.kind() == BibSyntaxKind::ENTRY && n.text_range() == entry.range)
        else {
            continue;
        };
        let documentation = super::hover::render_entry(&entry.entry_type, &entry.key, &node);
        let detail =
            citation_inline_detail(&node).unwrap_or_else(|| format!("@{}", entry.entry_type));
        return Some((detail, documentation));
    }
    None
}

/// A compact one-line citation summary for the inline `detail`: the first author
/// (or editor) joined with the year, e.g. `Knuth, Donald E. (1984)`. `None` when
/// neither field is present (the caller falls back to the entry type).
fn citation_inline_detail(node: &BibSyntaxNode) -> Option<String> {
    let author = bib_field(node, "author").or_else(|| bib_field(node, "editor"));
    let year = bib_field(node, "year");
    match (author, year) {
        (Some(a), Some(y)) => Some(format!("{} ({y})", first_author(&a))),
        (Some(a), None) => Some(first_author(&a)),
        (None, Some(y)) => Some(format!("({y})")),
        (None, None) => None,
    }
}

/// The cleaned value of the first field named `want` (case-insensitive), if any.
fn bib_field(node: &BibSyntaxNode, want: &str) -> Option<String> {
    for field in bib_ast::fields(node) {
        let Some(name) = bib_ast::field_name(&field) else {
            continue;
        };
        if !name.eq_ignore_ascii_case(want) {
            continue;
        }
        let value = bib_ast::field_value(&field).map(|v| super::hover::clean_value(&v))?;
        return (!value.is_empty()).then_some(value);
    }
    None
}

/// The first author of a BibTeX `and`-joined author list, trimmed. Used only for
/// the compact inline detail (the full list lives in the documentation card).
fn first_author(authors: &str) -> String {
    authors
        .split(" and ")
        .next()
        .unwrap_or(authors)
        .trim()
        .to_string()
}

// --- Command / environment ----------------------------------------------------

/// `(detail, documentation)` for a command: the synthesized prototype as the
/// inline detail and the full hover card as the documentation. Scope-first lookup
/// (tracked-document scope, else built-in/CWL only).
fn command_detail(
    snapshot: &Analysis,
    members: Vec<ProjectMember>,
    file: &Path,
    name: &str,
) -> Option<(String, String)> {
    let scope = scope_for(snapshot, members, file);
    let (sig, user) = super::hover::lookup_command(&scope, name)?;
    let mut detail = format!("\\{name}");
    for arg in &sig.args {
        detail.push_str(super::hover::arg_slot(arg.kind));
    }
    Some((detail, super::hover::render_command(name, sig, user)))
}

/// `(detail, documentation)` for an environment, like [`command_detail`] but with a
/// `\begin{name}…` prototype.
fn environment_detail(
    snapshot: &Analysis,
    members: Vec<ProjectMember>,
    file: &Path,
    name: &str,
) -> Option<(String, String)> {
    let scope = scope_for(snapshot, members, file);
    let (sig, user) = super::hover::lookup_environment(&scope, name)?;
    let detail = format!("\\begin{{{name}}}{}", arg_slots(&sig.args));
    Some((detail, super::hover::render_environment(name, sig, user)))
}

/// The merged signature scope for `file` when it is a tracked document, else an
/// empty scope (lookup falls back to the built-in/CWL tiers). Cloned because
/// resolve does not hold the snapshot borrow past this point.
fn scope_for(snapshot: &Analysis, members: Vec<ProjectMember>, file: &Path) -> SignatureDb {
    match snapshot.lookup_file(file) {
        Some(source) => snapshot.scope_signatures(members, source).clone(),
        None => SignatureDb::default(),
    }
}

/// The concatenated `{}`/`[]` slots for an argument list.
fn arg_slots(args: &[ArgSpec]) -> String {
    args.iter()
        .map(|a| super::hover::arg_slot(a.kind))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::incremental::IncrementalDatabase;

    /// Run completion at the first byte of `needle`, returning the items (each
    /// carrying its `data` payload) so a test can resolve them.
    fn complete(
        db: &IncrementalDatabase,
        path: &Path,
        src: &str,
        needle: &str,
    ) -> Vec<CompletionItem> {
        let snapshot = db.snapshot();
        let members = super::members_of(&snapshot);
        let offset = src.find(needle).expect("needle present") + needle.len();
        let idx = LineIndex::new(src);
        let (line, character) = idx.utf16_position(src, offset);
        let uri: Uri = format!("file://{}", path.display()).parse().expect("uri");
        super::compute_completion(
            &snapshot,
            &uri,
            path,
            src,
            Position { line, character },
            members,
        )
    }

    /// Resolve `item` against a fresh snapshot of `db`.
    fn resolve_item(db: &IncrementalDatabase, item: CompletionItem) -> CompletionItem {
        let snapshot = db.snapshot();
        let members = super::members_of(&snapshot);
        resolve(&snapshot, item, members)
    }

    fn documentation(item: &CompletionItem) -> String {
        match item.documentation.as_ref().expect("documentation") {
            Documentation::MarkupContent(m) => m.value.clone(),
            other => panic!("expected markup, got {other:?}"),
        }
    }

    #[test]
    fn citation_resolves_to_card_and_detail() {
        let tex = "\\addbibresource{refs.bib}\n\\cite{knu";
        let bib = "@book{knuth1984,\n  author = {Knuth, Donald E.},\n  title = {The TeXbook},\n  year = {1984},\n}\n";
        let tex_path = Path::new("/p/main.tex");
        let bib_path = Path::new("/p/refs.bib");
        let mut db = IncrementalDatabase::default();
        db.upsert_file(tex_path, tex.to_string());
        db.upsert_file(bib_path, bib.to_string());

        let items = complete(&db, tex_path, tex, "knu");
        let item = items
            .into_iter()
            .find(|i| i.label == "knuth1984")
            .expect("knuth1984 candidate");
        // Lean before resolve.
        assert!(item.documentation.is_none(), "documentation is lazy");
        assert!(item.data.is_some(), "carries resolve data");

        let resolved = resolve_item(&db, item);
        let doc = documentation(&resolved);
        assert!(doc.contains("@book"), "type: {doc}");
        assert!(doc.contains("The TeXbook"), "title: {doc}");
        assert!(doc.contains("Knuth"), "author: {doc}");
        let detail = resolved.detail.expect("detail");
        assert!(detail.contains("Knuth"), "detail author: {detail}");
        assert!(detail.contains("1984"), "detail year: {detail}");
    }

    #[test]
    fn command_resolves_to_signature() {
        let src = "\\sec";
        let path = Path::new("/p/main.tex");
        let mut db = IncrementalDatabase::default();
        db.upsert_file(path, src.to_string());

        let items = complete(&db, path, src, "\\sec");
        let item = items
            .into_iter()
            .find(|i| i.label == "section")
            .expect("section candidate");
        assert!(item.documentation.is_none(), "documentation is lazy");

        let resolved = resolve_item(&db, item);
        let doc = documentation(&resolved);
        assert!(doc.contains("\\section"), "prototype: {doc}");
        assert!(doc.contains("sectioning level"), "facts: {doc}");
        // `\section` takes an optional short-title plus the mandatory title.
        assert_eq!(resolved.detail.as_deref(), Some("\\section[]{}"), "detail");
    }

    #[test]
    fn environment_resolves_to_signature() {
        let src = "\\begin{ali";
        let path = Path::new("/p/main.tex");
        let mut db = IncrementalDatabase::default();
        db.upsert_file(path, src.to_string());

        let items = complete(&db, path, src, "{ali");
        let item = items
            .into_iter()
            .find(|i| i.label == "align")
            .expect("align candidate");

        let resolved = resolve_item(&db, item);
        let doc = documentation(&resolved);
        assert!(doc.contains("\\begin{align}"), "prototype: {doc}");
        assert!(doc.contains("math"), "facts: {doc}");
    }

    #[test]
    fn item_without_data_round_trips_unchanged() {
        let mut db = IncrementalDatabase::default();
        db.upsert_file(Path::new("/p/main.tex"), String::new());
        let item = CompletionItem {
            label: "bare".to_owned(),
            ..Default::default()
        };
        let resolved = resolve_item(&db, item.clone());
        assert_eq!(resolved, item);
    }
}
