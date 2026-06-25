//! `textDocument/hover` computation. Mirrors arity's `src/lsp/hover.rs` shape so
//! the eventual shared-crate extraction stays a lift, but the *content* is
//! LaTeX-specific: the cursor's command/environment **signature** (from the
//! package-merged signature scope) or its `\cite` key's resolved **`.bib` entry**.
//!
//! Two targets, tried in order (they sit at disjoint offsets, so order is only a
//! tie-break):
//!
//! - **Command / environment signature.** A `\command` control word, or an
//!   environment name in a `\begin{…}`/`\end{…}`, renders a synthesized prototype
//!   plus a facts line (arity, argument kinds, sectioning/float/theorem level,
//!   verbatim/math/list flags, and built-in vs. user/package-defined provenance).
//!   Looked up scope-first (the document's own + loaded packages' scanned defs),
//!   then the curated built-in DB, then the bulk CWL tier — mirroring
//!   [`super::build_completion_items`]'s tiering.
//! - **Citation → `.bib` entry.** A `\cite`-family key resolves cross-file against
//!   the project bibliography ([`Analysis::resolve_project`]); the matched
//!   `@entry`'s author/title/year/journal are pulled from the cached bib CST.
//!
//! Label resolution for `\ref` and the user-macro *definition body* are deferred
//! (see `TODO.md`). Like go-to-definition, the read runs off the snapshot's cached
//! parse when the tracked buffer still matches `text`, else a fresh parse, all
//! wrapped in [`salsa::Cancelled::catch`].

use std::fmt::Write as _;

use super::*;
use crate::bib::ast as bib_ast;
use crate::bib::syntax::{SyntaxKind as BibSyntaxKind, SyntaxNode as BibSyntaxNode};
use crate::semantic::signature::{ArgKind, CommandSig, EnvironmentSig, OutlineKind, builtin, cwl};
use crate::syntax::{SyntaxKind, SyntaxToken};
use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind};

/// Build hover contents for the construct at `position`, preferring the snapshot's
/// cached model and falling back to a fresh parse when it is stale or uncached. The
/// signature scope and `\cite` resolution both interne `members` against the db
/// snapshot, like [`super::compute_goto_definition`].
pub(crate) fn compute_hover(
    snapshot: &Analysis,
    path: &Path,
    text: &str,
    position: Position,
    members: Vec<ProjectMember>,
) -> Option<Hover> {
    let idx = LineIndex::new(text);
    let offset = idx.offset_at(text, position.line, position.character);

    let result = salsa::Cancelled::catch(AssertUnwindSafe(|| {
        match snapshot.lookup_file(path) {
            Some(file) if snapshot.file_text(file) == text => {
                let root = snapshot.parsed_tree(file);
                let model = snapshot.semantic_model(file);
                let scope = snapshot.scope_signatures(members.clone(), file);
                let lint_path = snapshot.file_path(file).to_path_buf();
                build_hover(
                    snapshot, &root, model, scope, &lint_path, members, offset, &idx, text,
                )
            }
            // Untracked or stale: a fresh parse + scan (no cross-package scope), like
            // completion's `reparse_tex_completion`. Cross-file `\cite` resolution
            // still runs against the snapshot's project resolvers, keyed by `path`.
            _ => {
                let root = SyntaxNode::new_root(parse(text).green);
                let model = SemanticModel::build(&root);
                let scanned = crate::semantic::scan_definitions(&root);
                build_hover(
                    snapshot, &root, &model, &scanned, path, members, offset, &idx, text,
                )
            }
        }
    }));
    result.ok().flatten()
}

/// The shared body: try a command/environment signature, then a citation entry.
#[allow(clippy::too_many_arguments)]
fn build_hover(
    snapshot: &Analysis,
    root: &SyntaxNode,
    model: &SemanticModel,
    scope: &SignatureDb,
    lint_path: &Path,
    members: Vec<ProjectMember>,
    offset: usize,
    idx: &LineIndex,
    text: &str,
) -> Option<Hover> {
    if let Some(target) = signature_target_at(root, offset) {
        let value = match target.kind {
            TargetKind::Command => {
                let (sig, user) = lookup_command(scope, &target.name)?;
                render_command(&target.name, sig, user)
            }
            TargetKind::Environment => {
                let (sig, user) = lookup_environment(scope, &target.name)?;
                render_environment(&target.name, sig, user)
            }
        };
        return Some(markup_hover(value, target.range, idx, text));
    }

    if let Some((name, key_range)) = citation_at(model, offset) {
        let (_, citations) = snapshot.resolve_project(members);
        let value = render_citation(snapshot, citations, lint_path, &name)?;
        return Some(markup_hover(value, key_range, idx, text));
    }

    None
}

/// Wrap rendered markdown in a [`Hover`], anchoring its range to `range` for the
/// client's highlight.
fn markup_hover(value: String, range: TextRange, idx: &LineIndex, text: &str) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: Some(byte_range_to_lsp(
            idx,
            text,
            usize::from(range.start()),
            usize::from(range.end()),
        )),
    }
}

// --- Command / environment signature ------------------------------------------

enum TargetKind {
    Command,
    Environment,
}

/// A signature-hoverable construct under the cursor: a command name or an
/// environment name, with the byte range to highlight.
struct SigTarget {
    kind: TargetKind,
    name: String,
    range: TextRange,
}

/// The command/environment name token the cursor sits on, if any: a `CONTROL_WORD`
/// child of a `COMMAND`, or a name token inside the `NAME_GROUP` of a
/// `\begin`/`\end`. Mirrors completion's `command_name_context`/`group_context`, but
/// over a *complete* construct rather than a typed prefix.
fn signature_target_at(root: &SyntaxNode, offset: usize) -> Option<SigTarget> {
    let at = TextSize::new(offset.min(u32::MAX as usize) as u32);
    let (left, right) = match root.token_at_offset(at) {
        rowan::TokenAtOffset::None => return None,
        rowan::TokenAtOffset::Single(t) => (Some(t.clone()), Some(t)),
        rowan::TokenAtOffset::Between(l, r) => (Some(l), Some(r)),
    };

    for token in [left, right].into_iter().flatten() {
        if token.kind() == SyntaxKind::CONTROL_WORD
            && let Some(parent) = token.parent()
            && parent.kind() == SyntaxKind::COMMAND
        {
            return Some(SigTarget {
                kind: TargetKind::Command,
                name: token.text().trim_start_matches('\\').to_string(),
                range: token.text_range(),
            });
        }
        if let Some(target) = environment_target(&token) {
            return Some(target);
        }
    }
    None
}

/// The `NAME_GROUP` of a `\begin`/`\end` an enclosing-named `token` sits in: its
/// inner name text (`*`-suffix included) and the range of that inner text.
fn environment_target(token: &SyntaxToken) -> Option<SigTarget> {
    let group = token
        .parent_ancestors()
        .find(|n| n.kind() == SyntaxKind::NAME_GROUP)?;
    let parent = group.parent()?;
    if !matches!(parent.kind(), SyntaxKind::BEGIN | SyntaxKind::END) {
        return None;
    }
    let (name, range) = name_group_inner(&group)?;
    Some(SigTarget {
        kind: TargetKind::Environment,
        name,
        range,
    })
}

/// The inner text of a `NAME_GROUP` (the `{name}` minus its braces) and that text's
/// byte range. Concatenates the non-brace tokens so a starred name (`figure*`)
/// reassembles. `None` for an empty group.
fn name_group_inner(group: &SyntaxNode) -> Option<(String, TextRange)> {
    let mut text = String::new();
    let mut start = None;
    let mut end = None;
    for token in group.children_with_tokens().filter_map(|e| e.into_token()) {
        match token.kind() {
            SyntaxKind::L_BRACE | SyntaxKind::R_BRACE => {}
            _ => {
                let r = token.text_range();
                start.get_or_insert(r.start());
                end = Some(r.end());
                text.push_str(token.text());
            }
        }
    }
    Some((text, TextRange::new(start?, end?)))
}

/// A command signature, scope-first then built-in then CWL, with `true` when the hit
/// came from the local/package scope (rendered as "user-defined").
pub(super) fn lookup_command<'a>(
    scope: &'a SignatureDb,
    name: &str,
) -> Option<(&'a CommandSig, bool)> {
    if let Some(sig) = scope.command(name) {
        return Some((sig, true));
    }
    builtin()
        .command(name)
        .or_else(|| cwl().command(name))
        .map(|sig| (sig, false))
}

/// An environment signature, with the same tiering as [`lookup_command`].
pub(super) fn lookup_environment<'a>(
    scope: &'a SignatureDb,
    name: &str,
) -> Option<(&'a EnvironmentSig, bool)> {
    if let Some(sig) = scope.environment(name) {
        return Some((sig, true));
    }
    builtin()
        .environment(name)
        .or_else(|| cwl().environment(name))
        .map(|sig| (sig, false))
}

/// `{}`/`[]` slot for an argument kind, for the synthesized prototype.
pub(super) fn arg_slot(kind: ArgKind) -> &'static str {
    match kind {
        ArgKind::Brace => "{}",
        ArgKind::Bracket => "[]",
    }
}

/// A human summary of an argument list: e.g. `2 required, 1 optional`. Empty when the
/// construct takes no arguments.
fn arg_summary(args: &[crate::semantic::signature::ArgSpec]) -> Option<String> {
    let req = args.iter().filter(|a| a.required).count();
    let opt = args.len() - req;
    let mut parts = Vec::new();
    if req > 0 {
        parts.push(format!("{req} required"));
    }
    if opt > 0 {
        parts.push(format!("{opt} optional"));
    }
    (!parts.is_empty()).then(|| format!("{} argument{}", parts.join(", "), plural(args.len())))
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// `\name{}{}` prototype + a `·`-joined facts line.
pub(super) fn render_command(name: &str, sig: &CommandSig, user_defined: bool) -> String {
    let mut out = String::new();
    let _ = write!(out, "```latex\n\\{name}");
    for arg in sig.args.iter() {
        out.push_str(arg_slot(arg.kind));
    }
    out.push_str("\n```\n");

    let mut facts = vec![if user_defined {
        "user-defined command".to_string()
    } else {
        "command".to_string()
    }];
    if let Some(level) = sig.sectioning {
        facts.push(format!("sectioning level {level}"));
    }
    if sig.verbatim {
        facts.push("verbatim argument".to_string());
    }
    if let Some(summary) = arg_summary(&sig.args) {
        facts.push(summary);
    }
    out.push_str(&facts.join(" · "));
    out
}

/// `\begin{name} … \end{name}` prototype + a `·`-joined facts line.
pub(super) fn render_environment(name: &str, sig: &EnvironmentSig, user_defined: bool) -> String {
    let mut out = String::new();
    let _ = write!(out, "```latex\n\\begin{{{name}}}");
    for arg in sig.args.iter() {
        out.push_str(arg_slot(arg.kind));
    }
    let _ = write!(out, " … \\end{{{name}}}\n```\n");

    let mut facts = vec![if user_defined {
        "user-defined environment".to_string()
    } else {
        "environment".to_string()
    }];
    match sig.outline {
        Some(OutlineKind::Float) => facts.push("float".to_string()),
        Some(OutlineKind::Theorem) => facts.push("theorem-like".to_string()),
        None => {}
    }
    if sig.math {
        facts.push("math".to_string());
    }
    if sig.align {
        facts.push("alignment".to_string());
    }
    if sig.list {
        facts.push("list".to_string());
    }
    if sig.verbatim_body {
        facts.push("verbatim body".to_string());
    } else if sig.code {
        facts.push("code body".to_string());
    }
    if let Some(summary) = arg_summary(&sig.args) {
        facts.push(summary);
    }
    out.push_str(&facts.join(" · "));
    out
}

// --- Citation → bib entry -----------------------------------------------------

/// The cite key whose *key* range covers `offset`, with that range. Uses `key_range`
/// (not the whole-command range), so a multi-key `\cite{a,b}` resolves the one key
/// under the cursor — the same per-key precision rename relies on.
fn citation_at(model: &SemanticModel, offset: usize) -> Option<(SmolStr, TextRange)> {
    let at = TextSize::new(offset.min(u32::MAX as usize) as u32);
    model
        .citations()
        .iter()
        .find(|c| c.key_range.contains_inclusive(at))
        .map(|c| (c.name.clone(), c.key_range))
}

/// Render the `@entry` a cite key resolves to: its type + key, then a few canonical
/// fields. `None` when the key resolves to no entry (no useful card to show). Mirrors
/// [`super::resolve_citation_locations`]'s namespace walk.
fn render_citation(
    snapshot: &Analysis,
    citations: &ResolvedCitations,
    lint_path: &Path,
    name: &SmolStr,
) -> Option<String> {
    for bib_path in citations.bib_definers(lint_path) {
        let Some(file) = snapshot.lookup_file(bib_path) else {
            continue;
        };
        let Some(entry) = snapshot
            .bib_semantic_model(file)
            .entries()
            .iter()
            .find(|e| e.key.eq_ignore_ascii_case(name))
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
        return Some(render_entry(&entry.entry_type, &entry.key, &node));
    }
    None
}

/// The fields worth surfacing in a citation hover, in display order.
const HOVER_FIELDS: &[&str] = &["author", "editor", "title", "year", "journal", "booktitle"];

/// Format a bib entry node: `@type · \`key\`` then bold field lines for the
/// [`HOVER_FIELDS`] it carries.
pub(super) fn render_entry(entry_type: &str, key: &str, node: &BibSyntaxNode) -> String {
    let mut out = format!("@{entry_type} · `{key}`");
    for &want in HOVER_FIELDS {
        for field in bib_ast::fields(node) {
            let Some(fname) = bib_ast::field_name(&field) else {
                continue;
            };
            if !fname.eq_ignore_ascii_case(want) {
                continue;
            }
            if let Some(value) = bib_ast::field_value(&field).map(|v| clean_value(&v))
                && !value.is_empty()
            {
                let _ = write!(out, "\n\n**{fname}:** {value}");
            }
            break;
        }
    }
    out
}

/// A bib field value as plain display text: the node text, trimmed, with one layer of
/// surrounding `{…}`/`"…"` removed and interior whitespace collapsed.
pub(super) fn clean_value(value: &BibSyntaxNode) -> String {
    let raw = value.text().to_string();
    let trimmed = raw.trim();
    let inner = trimmed
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .or_else(|| trimmed.strip_prefix('"').and_then(|s| s.strip_suffix('"')))
        .unwrap_or(trimmed);
    inner.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::incremental::IncrementalDatabase;

    /// Render hover at the first byte of `needle` in `src`, returning its markdown.
    fn hover_md(src: &str, needle: &str) -> Option<String> {
        let path = Path::new("/p/main.tex");
        let mut db = IncrementalDatabase::default();
        db.upsert_file(path, src.to_string());
        let offset = src.find(needle).expect("needle present");
        markdown_at(&db, path, src, offset)
    }

    /// Build the snapshot's members and render the hover markdown at `offset`.
    fn markdown_at(
        db: &IncrementalDatabase,
        path: &Path,
        src: &str,
        offset: usize,
    ) -> Option<String> {
        let snapshot = db.snapshot();
        let members = super::members_of(&snapshot);
        let position = byte_to_position(src, offset);
        let hover = compute_hover(&snapshot, path, src, position, members)?;
        match hover.contents {
            HoverContents::Markup(m) => Some(m.value),
            other => panic!("expected markup, got {other:?}"),
        }
    }

    fn byte_to_position(src: &str, offset: usize) -> Position {
        let idx = LineIndex::new(src);
        let (line, character) = idx.utf16_position(src, offset);
        Position { line, character }
    }

    #[test]
    fn command_signature_shows_sectioning_level() {
        let md = hover_md("\\section{Intro}\n", "section").expect("hover for \\section");
        assert!(md.contains("\\section"), "prototype: {md}");
        assert!(md.contains("sectioning level"), "facts: {md}");
        assert!(md.contains("command"), "kind: {md}");
    }

    #[test]
    fn environment_signature_shows_math_flag() {
        let src = "\\begin{align}\nx &= y\n\\end{align}\n";
        let md = hover_md(src, "align").expect("hover for align");
        assert!(md.contains("\\begin{align}"), "prototype: {md}");
        assert!(md.contains("math"), "facts: {md}");
    }

    #[test]
    fn user_defined_command_is_marked() {
        let src = "\\newcommand{\\foo}[1]{#1}\n\\foo{bar}\n";
        // Hover the *use* site, not the definition.
        let offset = src.rfind("foo").expect("use site");
        let path = Path::new("/p/main.tex");
        let mut db = IncrementalDatabase::default();
        db.upsert_file(path, src.to_string());
        let md = markdown_at(&db, path, src, offset).expect("hover for \\foo");
        assert!(md.contains("user-defined command"), "provenance: {md}");
        assert!(md.contains("1 required argument"), "arity: {md}");
    }

    #[test]
    fn citation_resolves_to_bib_entry() {
        let tex = "\\addbibresource{refs.bib}\n\\cite{knuth1984}\n";
        let bib = "@book{knuth1984,\n  author = {Knuth, Donald E.},\n  title = {The TeXbook},\n  year = {1984},\n}\n";
        let tex_path = Path::new("/p/main.tex");
        let bib_path = Path::new("/p/refs.bib");
        let mut db = IncrementalDatabase::default();
        db.upsert_file(tex_path, tex.to_string());
        db.upsert_file(bib_path, bib.to_string());

        let offset = tex.find("knuth1984").expect("cite key");
        let md = markdown_at(&db, tex_path, tex, offset).expect("hover for \\cite key");
        assert!(md.contains("@book"), "type: {md}");
        assert!(md.contains("knuth1984"), "key: {md}");
        assert!(md.contains("The TeXbook"), "title: {md}");
        assert!(md.contains("Knuth"), "author: {md}");
    }

    #[test]
    fn no_hover_on_plain_prose() {
        assert!(hover_md("Just some words here.\n", "words").is_none());
    }
}
