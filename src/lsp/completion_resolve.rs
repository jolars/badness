//! `completionItem/resolve` computation. Command items include short signatures
//! in the initial response. When the client highlights an item, we attach its
//! markdown documentation and any detail not already supplied. Citation cards
//! and environment signatures remain lazy.
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
use lsp_types::{CompletionItemLabelDetails, Documentation, MarkupContent, MarkupKind};
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
pub(crate) fn resolve(snapshot: &Analysis, mut item: CompletionItem) -> CompletionItem {
    let Some(data) = item
        .data
        .clone()
        .and_then(|v| serde_json::from_value::<CompletionResolveData>(v).ok())
    else {
        return item;
    };

    let detail_doc = match data {
        CompletionResolveData::Citation { lint_path, key } => {
            citation_detail(snapshot, &lint_path, &key)
        }
        CompletionResolveData::Command { name, file } => command_detail(snapshot, &file, &name),
        CompletionResolveData::Environment { name, file } => {
            environment_detail(snapshot, &file, &name)
        }
    };

    if let Some((detail, documentation)) = detail_doc {
        // The document may have changed since completion. Preserve the initial
        // prototype so it stays consistent with the item's label details.
        item.detail.get_or_insert(detail);
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
fn citation_detail(snapshot: &Analysis, lint_path: &Path, key: &str) -> Option<(String, String)> {
    let (_, citations) = snapshot.resolve_project();
    for bib_path in citations.bib_definers(lint_path) {
        let Some(file) = snapshot.lookup_file(&bib_path) else {
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

/// Attach the short command signature using the scope already read by completion.
/// Documentation and provenance rendering stay on the resolve path.
pub(super) fn add_command_signature(item: &mut CompletionItem, scope: &SignatureDb) {
    let Some((sig, _)) = super::hover::lookup_command(scope, &item.label) else {
        return;
    };
    let (detail, slots) = command_signature(&item.label, &sig.args);
    item.detail = Some(detail);
    item.label_details = (!slots.is_empty()).then_some(CompletionItemLabelDetails {
        detail: Some(slots),
        description: None,
    });
}

/// The full command prototype and the suffix displayed beside its completion label.
fn command_signature(name: &str, args: &[ArgSpec]) -> (String, String) {
    let slots = arg_slots(args);
    (format!("\\{name}{slots}"), slots)
}

/// `(detail, documentation)` for a command: the synthesized prototype as the
/// inline detail and the full hover card as the documentation. Scope-first lookup
/// (tracked-document scope, else built-in/CWL only).
fn command_detail(snapshot: &Analysis, file: &Path, name: &str) -> Option<(String, String)> {
    let scope = scope_for(snapshot, file);
    let (sig, provenance) = super::hover::lookup_command(&scope, name)?;
    let (detail, _) = command_signature(name, &sig.args);
    Some((detail, super::hover::render_command(name, sig, &provenance)))
}

/// `(detail, documentation)` for an environment, like [`command_detail`] but with a
/// `\begin{name}…` prototype.
fn environment_detail(snapshot: &Analysis, file: &Path, name: &str) -> Option<(String, String)> {
    let scope = scope_for(snapshot, file);
    let (sig, provenance) = super::hover::lookup_environment(&scope, name)?;
    let detail = format!("\\begin{{{name}}}{}", arg_slots(&sig.args));
    Some((
        detail,
        super::hover::render_environment(name, sig, &provenance),
    ))
}

/// The merged signature scope for `file` when it is a tracked document, else an
/// empty scope (lookup falls back to the built-in/CWL tiers). Cloned because
/// resolve does not hold the snapshot borrow past this point.
fn scope_for(snapshot: &Analysis, file: &Path) -> SignatureDb {
    match snapshot.lookup_file(file) {
        Some(source) => snapshot.scope_signatures(source).clone(),
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

    /// Run completion after the last `needle`, returning the items (each
    /// carrying its `data` payload) so a test can resolve them.
    fn complete(
        db: &IncrementalDatabase,
        path: &Path,
        src: &str,
        needle: &str,
    ) -> Vec<CompletionItem> {
        let snapshot = db.snapshot();
        let offset = src.rfind(needle).expect("needle present") + needle.len();
        let idx = LineIndex::new(src);
        let (line, character) = idx.position(offset);
        let uri: Uri = format!("file://{}", path.display()).parse().expect("uri");
        // These tests never complete a package name, so the index is never resolved;
        // a disabled config keeps that guaranteed (and hermetic).
        let texmf = crate::project::texmf::TexmfConfig {
            enabled: false,
            ..Default::default()
        };
        super::compute_completion(
            &snapshot,
            &uri,
            path,
            &TextBuffer::new(src, PositionEncoding::Utf16),
            Position { line, character },
            &texmf,
        )
    }

    /// Resolve `item` against a fresh snapshot of `db`.
    fn resolve_item(db: &IncrementalDatabase, item: CompletionItem) -> CompletionItem {
        let snapshot = db.snapshot();
        resolve(&snapshot, item)
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
    fn citation_filter_text_carries_key_title_author() {
        let tex = "\\addbibresource{refs.bib}\n\\cite{knu";
        let bib = "@book{knuth1984,\n  author = {Knuth, Donald E.},\n  title = {The TeXbook},\n  year = {1984},\n}\n";
        let mut db = IncrementalDatabase::default();
        db.upsert_file(Path::new("/p/main.tex"), tex.to_string());
        db.upsert_file(Path::new("/p/refs.bib"), bib.to_string());

        let items = complete(&db, Path::new("/p/main.tex"), tex, "knu");
        let item = items
            .into_iter()
            .find(|i| i.label == "knuth1984")
            .expect("knuth1984 candidate");
        let filter = item.filter_text.expect("filter_text");
        assert!(filter.contains("knuth1984"), "key: {filter}");
        assert!(filter.contains("The TeXbook"), "title: {filter}");
        assert!(filter.contains("Knuth"), "author: {filter}");
        assert_eq!(item.sort_text.as_deref(), Some("knuth1984"), "sort_text");
    }

    #[test]
    fn citation_completion_is_not_key_prefixed() {
        // The typed prefix `Te` matches only the *title*, not the key. The server must
        // still return the entry (the client filters by filterText), so title-word
        // matching works on any editor.
        let tex = "\\addbibresource{refs.bib}\n\\cite{Te";
        let bib = "@book{knuth1984,\n  author = {Knuth, Donald E.},\n  title = {The TeXbook},\n}\n";
        let mut db = IncrementalDatabase::default();
        db.upsert_file(Path::new("/p/main.tex"), tex.to_string());
        db.upsert_file(Path::new("/p/refs.bib"), bib.to_string());

        let items = complete(&db, Path::new("/p/main.tex"), tex, "Te");
        assert!(
            items.iter().any(|i| i.label == "knuth1984"),
            "full namespace returned regardless of key prefix: {:?}",
            items.iter().map(|i| &i.label).collect::<Vec<_>>()
        );
    }

    #[test]
    fn command_signatures_are_available_before_resolve() {
        let path = Path::new("/p/main.tex");
        for (src, name, signature, slots) in [
            ("\\sec", "section", "\\section[]{}", Some("[]{}")),
            ("\\vsp", "vspace", "\\vspace{}", Some("{}")),
            ("\\ome", "omega", "\\omega", None),
            (
                "\\renewcommand{\\section}[2]{#1#2}\n\\sec",
                "section",
                "\\section{}{}",
                Some("{}{}"),
            ),
        ] {
            let mut db = IncrementalDatabase::default();
            // Both fresh-buffer fallback and the cached path must expose signatures.
            for cached in [false, true] {
                if cached {
                    let file = db.upsert_file(path, src.to_string());
                    db.reparse_stage_edits(file, None);
                }
                let prefix = src.rsplit('\n').next().unwrap();
                let items = complete(&db, path, src, prefix);
                let item = items.into_iter().find(|i| i.label == name).unwrap();
                assert_eq!(item.detail.as_deref(), Some(signature), "{src}, {cached}");
                assert_eq!(
                    item.label_details
                        .as_ref()
                        .and_then(|d| d.detail.as_deref()),
                    slots,
                );
                assert!(item.documentation.is_none(), "documentation stays lazy");
                assert_eq!(
                    item.kind,
                    Some(if name == "omega" {
                        CompletionItemKind::CONSTANT
                    } else {
                        CompletionItemKind::FUNCTION
                    }),
                );
                assert_eq!(item.insert_text_format, None);
                let Some(lsp_types::CompletionTextEdit::Edit(edit)) = &item.text_edit else {
                    panic!("command replacement edit")
                };
                assert_eq!(edit.new_text, name, "signature is display-only");
                if cached {
                    let resolved = resolve_item(&db, item.clone());
                    assert_eq!(resolved.detail, item.detail);
                    assert_eq!(resolved.label_details, item.label_details);
                    assert!(resolved.documentation.is_some());
                }
            }
        }
    }

    #[test]
    fn curated_completion_kinds_keep_signatures_and_resolve_data() {
        let path = Path::new("/p/main.tex");
        let mut db = IncrementalDatabase::default();
        for (name, kind) in [
            ("alpha", CompletionItemKind::CONSTANT),
            ("Gamma", CompletionItemKind::CONSTANT),
            ("omega", CompletionItemKind::CONSTANT),
            ("infty", CompletionItemKind::CONSTANT),
            ("sum", CompletionItemKind::CONSTANT),
            ("leq", CompletionItemKind::CONSTANT),
            ("rightarrow", CompletionItemKind::CONSTANT),
            ("langle", CompletionItemKind::CONSTANT),
            ("hbar", CompletionItemKind::CONSTANT),
            ("longrightarrow", CompletionItemKind::CONSTANT),
            ("TeX", CompletionItemKind::CONSTANT),
            ("LaTeX", CompletionItemKind::CONSTANT),
            ("copyright", CompletionItemKind::CONSTANT),
            ("newpage", CompletionItemKind::KEYWORD),
            ("par", CompletionItemKind::KEYWORD),
            ("quad", CompletionItemKind::KEYWORD),
            ("qquad", CompletionItemKind::KEYWORD),
            ("bfseries", CompletionItemKind::KEYWORD),
            ("vspace", CompletionItemKind::FUNCTION),
            ("sqrt", CompletionItemKind::FUNCTION),
            ("mathord", CompletionItemKind::FUNCTION),
            ("verb", CompletionItemKind::FUNCTION),
            ("item", CompletionItemKind::FUNCTION),
            ("def", CompletionItemKind::FUNCTION),
            ("kern", CompletionItemKind::FUNCTION),
            ("hskip", CompletionItemKind::FUNCTION),
        ] {
            let src = format!("\\{name}");
            let file = db.upsert_file(path, src.clone());
            db.reparse_stage_edits(file, None);
            let item = complete(&db, path, &src, &src)
                .into_iter()
                .find(|i| i.label == name)
                .unwrap();
            assert_eq!(item.kind, Some(kind), "{name}");
            assert!(item.data.is_some(), "{name} retains resolve data");
            assert!(item.detail.is_some(), "{name} retains its signature");
            assert!(item.documentation.is_none());
            assert_eq!(item.insert_text_format, None);
            let resolved = resolve_item(&db, item.clone());
            assert_eq!(resolved.kind, item.kind);
            assert_eq!(resolved.text_edit, item.text_edit);
            assert_eq!(resolved.detail, item.detail);
            assert!(resolved.documentation.is_some(), "{name} resolves");
        }
    }

    #[test]
    fn curated_completion_yields_to_document_and_package_definitions() {
        let path = Path::new("/p/main.tex");
        for (name, definition) in [
            ("omega", "\\renewcommand{\\omega}[1]{#1}"),
            ("omega", "\\renewcommand{\\omega}{x}"),
            ("omega", "\\renewcommand{\\omega}[1][x]{#1}"),
            ("omega", "\\def\\omega#1{#1}"),
            ("omega", "\\RenewDocumentCommand{\\omega}{m}{#1}"),
            ("LaTeX", "\\renewcommand{\\LaTeX}[1]{#1}"),
            ("newpage", "\\renewcommand{\\newpage}[1]{#1}"),
            ("quad", "\\renewcommand{\\quad}{x}"),
            (
                "omega",
                "\\makeatletter\\def\\omega{\\@dblarg\\helper}\\makeatother",
            ),
            (
                "newpage",
                "\\makeatletter\\renewcommand{\\newpage}{\\@dblarg\\helper}\\makeatother",
            ),
        ] {
            let mut db = IncrementalDatabase::default();
            let prefix = format!("\\{}", &name[..3]);
            let src = format!("{definition}\n{prefix}");
            for cached in [false, true] {
                if cached {
                    let file = db.upsert_file(path, src.clone());
                    db.reparse_stage_edits(file, None);
                }
                let item = complete(&db, path, &src, &prefix)
                    .into_iter()
                    .find(|i| i.label == name)
                    .unwrap();
                assert_eq!(
                    item.kind,
                    Some(CompletionItemKind::FUNCTION),
                    "{definition}"
                );
            }

            let file = db.upsert_file(Path::new("/p/mypkg.sty"), definition.to_string());
            db.reparse_stage_edits(file, None);
            let src = format!("\\usepackage{{mypkg}}\n{prefix}");
            let file = db.upsert_file(path, src.clone());
            db.reparse_stage_edits(file, None);
            let item = complete(&db, path, &src, &prefix)
                .into_iter()
                .find(|i| i.label == name)
                .unwrap();
            assert_eq!(
                item.kind,
                Some(CompletionItemKind::FUNCTION),
                "{definition}"
            );
        }
    }

    #[test]
    fn initial_command_signatures_use_the_loaded_package_scope() {
        let path = Path::new("/p/main.tex");
        let mut db = IncrementalDatabase::default();
        let file = db.upsert_file(
            Path::new("/p/mypkg.sty"),
            "\\renewcommand{\\section}[2]{#1#2}".to_string(),
        );
        db.reparse_stage_edits(file, None);
        for (src, signature) in [
            ("\\usepackage{mypkg}\n\\sec", "\\section{}{}"),
            (
                "\\usepackage{mypkg}\n\\renewcommand{\\section}[1]{#1}\n\\sec",
                "\\section{}",
            ),
        ] {
            let file = db.upsert_file(path, src.to_string());
            db.reparse_stage_edits(file, None);
            let item = complete(&db, path, src, "\\sec")
                .into_iter()
                .find(|i| i.label == "section")
                .unwrap();
            assert_eq!(item.detail.as_deref(), Some(signature));
            let resolved = resolve_item(&db, item.clone());
            assert_eq!(resolved.detail, item.detail);
        }
    }

    #[test]
    fn completion_only_names_have_no_signature() {
        let src = "\\ExplSyntaxOn\n\\cs_new:Nn \\democompletion:n {#1}\n\\democompletion:";
        let path = Path::new("/p/main.tex");
        let db = IncrementalDatabase::default();
        let item = complete(&db, path, src, "\\democompletion:")
            .into_iter()
            .find(|i| i.label == "democompletion:n")
            .unwrap();
        assert!(item.detail.is_none());
        assert!(item.label_details.is_none());
    }

    #[test]
    fn resolve_preserves_the_initial_command_signature_after_an_edit() {
        let src = "\\newcommand{\\demo}[1]{#1}\n\\dem";
        let path = Path::new("/p/main.tex");
        let mut db = IncrementalDatabase::default();
        let file = db.upsert_file(path, src.to_string());
        db.reparse_stage_edits(file, None);
        let item = complete(&db, path, src, "\\dem")
            .into_iter()
            .find(|i| i.label == "demo")
            .unwrap();
        assert_eq!(item.detail.as_deref(), Some("\\demo{}"));
        let file = db.upsert_file(path, src.replace("[1]{#1}", "[2]{#1#2}"));
        db.reparse_stage_edits(file, None);
        let resolved = resolve_item(&db, item.clone());
        assert_eq!(resolved.detail, item.detail);
        assert_eq!(resolved.label_details, item.label_details);
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
