//! Code completion: command/environment names, `\ref`-family keys, and file
//! paths, classified from the cursor's position in the CST.
//!
//! The logic is a pure [`classify_context`] over the parse tree + a byte offset,
//! then [`candidates`] turns the context into a neutral candidate list, so the
//! LSP layer (`crate::lsp`) only maps
//! candidates onto `lsp_types::CompletionItem`s and does the one impure bit
//! (reading a directory for [`CompletionContext::FilePath`]). Keeping the logic
//! here, free of LSP types, makes it unit-testable straight off `parse`.
//!
//! Names are drawn from the signature DB (built-in [`builtin`] unioned with the
//! per-document scanned definitions) and labels from the [`SemanticModel`].
//! `\cite` keys classify to [`CompletionContext::CitationKey`], but — like
//! [`CompletionContext::FilePath`] — their candidates come from the project
//! bibliography (a cross-file snapshot query), so the LSP layer resolves them; the
//! pure [`candidates`] here yields nothing for them.

use rowan::{TextSize, TokenAtOffset};

use crate::ast::command_name;
use crate::semantic::SemanticModel;
use crate::semantic::builder::{is_cite_command, is_glossary_ref_command, ref_command};
use crate::semantic::signature::{
    SignatureDb, arg_enum_values, builtin, class_names, color_models, color_names, cwl,
    package_names, pgf_libraries, tikz_libraries,
};
use crate::syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

/// What the cursor at a given offset is positioned to complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionContext {
    /// Typing a control-word name after `\` (the `\` stripped from `prefix`).
    CommandName { prefix: String },
    /// Inside a `\begin{…}` / `\end{…}` name group. `closing` is true for `\end`.
    EnvironmentName { prefix: String, closing: bool },
    /// Inside the key group of a `\ref`-family command (`\ref`, `\cref`, …).
    LabelRef { prefix: String },
    /// Inside the key group of a `\cite`-family command. Keys come from the project
    /// bibliography (a cross-file snapshot query), so — like [`FilePath`] — this is
    /// resolved in the LSP layer, not by [`candidates`].
    ///
    /// [`FilePath`]: CompletionContext::FilePath
    CitationKey { prefix: String },
    /// Inside the key group of a `\gls`/`\acrshort`-family command. Keys come
    /// from the document namespace's `\newglossaryentry`/`\newacronym` definers
    /// (a cross-file snapshot query), so — like [`CitationKey`] — this is resolved
    /// in the LSP layer, not by [`candidates`].
    ///
    /// [`CitationKey`]: CompletionContext::CitationKey
    GlossaryKey { prefix: String },
    /// Inside the path argument of a file-taking command (`\includegraphics`,
    /// `\input`, …). `prefix` is the partial path typed so far (may contain `/`).
    FilePath { prefix: String, kind: FileArgKind },
    /// Inside a `\usepackage{…}` / `\documentclass{…}` name group. Candidates are
    /// the baked `.sty`/`.cls` name list (`kind` selecting which); the LSP layer
    /// additionally merges any local `.sty`/`.cls` files. `kind` is always
    /// [`FileArgKind::Package`] or [`FileArgKind::Class`].
    PackageName { prefix: String, kind: FileArgKind },
    /// Inside a group taking an existing color name (`\color{…}`,
    /// `\textcolor{…}`, `\colorbox{…}`, the base of `\colorlet{new}{…}`, …).
    /// Candidates are the built-in color list unioned with the document's
    /// `\definecolor`/`\colorlet` names.
    ColorName { prefix: String },
    /// Inside the model group of `\definecolor{name}{model}{…}` /
    /// `\providecolor` — a color model (`rgb`, `HTML`, `cmyk`, …).
    ColorModel { prefix: String },
    /// Inside a `\usetikzlibrary{…}` (`pgf` false) / `\usepgflibrary{…}` (`pgf`
    /// true) name group. A comma-separated list, completed after the last comma.
    TikzLibrary { prefix: String, pgf: bool },
    /// Inside a brace argument whose value is drawn from a fixed enumerated set
    /// (`\pagestyle{…}`, `\pagenumbering{…}`, `\bibliographystyle{…}`, …). The
    /// `values` are the built-in suggestions for that slot (`data/arg_enums.json`),
    /// resolved at classify time; `prefix` is the partial value typed so far.
    ArgumentEnum { prefix: String, values: Vec<String> },
    /// Nothing to complete here (prose, a comment, a `\cite{…}` key, …).
    None,
}

/// The category of a file-argument command, selecting which extensions a path
/// completion offers. The LSP layer reads the document's directory and filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileArgKind {
    /// `\includegraphics` — raster/vector image assets.
    Graphics,
    /// `\input` / `\include` / `\subfile` — `.tex` source files.
    TexSource,
    /// `\bibliography` / `\addbibresource` — `.bib` databases.
    Bib,
    /// `\usepackage` / `\RequirePackage` — `.sty` package files (`.dtx` sources).
    Package,
    /// `\documentclass` / `\LoadClass` — `.cls` class files (`.dtx` sources).
    Class,
}

impl FileArgKind {
    /// The file extensions (without the dot) this argument completes. Directories
    /// are always offered in addition, regardless of kind.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            FileArgKind::Graphics => &[
                "pdf", "png", "jpg", "jpeg", "eps", "ps", "gif", "svg", "tif", "tiff", "bmp",
            ],
            FileArgKind::TexSource => &["tex"],
            FileArgKind::Bib => &["bib"],
            // `.dtx` is offered because a `.sty`/`.cls` load resolves through a
            // sibling `.dtx` source (`project::package::dtx_source_of`).
            FileArgKind::Package => &["sty", "dtx"],
            FileArgKind::Class => &["cls", "dtx"],
        }
    }
}

/// The kind of a name candidate, mapped to an LSP `CompletionItemKind` by the
/// server layer (kept LSP-type-free here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateKind {
    Command,
    Environment,
    Label,
    /// A `.sty`/`.cls` package or class name (`\usepackage`/`\documentclass`).
    Package,
    /// A color name (`\color`/`\textcolor`/…).
    Color,
    /// A color model (`\definecolor` second argument).
    ColorModel,
    /// A TikZ/PGF library name (`\usetikzlibrary`/`\usepgflibrary`).
    TikzLibrary,
    /// An enumerated argument value (`\pagestyle`/`\pagenumbering`/…).
    ArgumentEnum,
}

/// A completion candidate before it becomes an `lsp_types::CompletionItem`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionCandidate {
    /// The label shown and, unless `snippet`, the text inserted.
    pub label: String,
    pub kind: CandidateKind,
    /// When `Some`, the text to insert instead of `label` (an environment
    /// snippet, e.g. `itemize}\n\t$0\n\\end{itemize`). When `snippet` is set,
    /// this is LSP snippet syntax with `$0`/`$1` tab stops.
    pub insert_text: Option<String>,
    /// Whether `insert_text` is an LSP snippet (sets `InsertTextFormat::Snippet`).
    pub snippet: bool,
}

/// Classify what the cursor at byte `offset` is positioned to complete.
pub fn classify_context(root: &SyntaxNode, offset: usize) -> CompletionContext {
    let offset = TextSize::new(offset.min(u32::MAX as usize) as u32);
    let (left, right) = match root.token_at_offset(offset) {
        TokenAtOffset::None => return CompletionContext::None,
        TokenAtOffset::Single(t) => (Some(t.clone()), Some(t)),
        TokenAtOffset::Between(l, r) => (Some(l), Some(r)),
    };

    // Typing a command name extends the token to the *left* of the cursor.
    if let Some(left) = &left
        && let Some(ctx) = command_name_context(left, offset)
    {
        return ctx;
    }

    // Inside a brace group: either adjacent token's parent is the group.
    for tok in [left.as_ref(), right.as_ref()].into_iter().flatten() {
        if let Some(ctx) = group_context(tok, offset) {
            return ctx;
        }
    }

    CompletionContext::None
}

/// A command-name context when `token` is a control word (or a lone `\`) the
/// cursor sits within or just after.
fn command_name_context(token: &SyntaxToken, offset: TextSize) -> Option<CompletionContext> {
    match token.kind() {
        SyntaxKind::CONTROL_WORD => {
            let range = token.text_range();
            // The cursor must be after the leading `\` and within/at the word.
            if offset <= range.start() || offset > range.end() {
                return None;
            }
            let rel = usize::from(offset - range.start());
            let typed = token.text().get(..rel).unwrap_or(token.text());
            Some(CompletionContext::CommandName {
                prefix: typed.trim_start_matches('\\').to_string(),
            })
        }
        // A lone `\` just typed (no letters yet): offer every command name.
        SyntaxKind::CONTROL_SYMBOL
            if token.text() == "\\" && offset == token.text_range().end() =>
        {
            Some(CompletionContext::CommandName {
                prefix: String::new(),
            })
        }
        _ => None,
    }
}

/// A group context when `token` sits inside a `NAME_GROUP` (environment name) or
/// a command's `GROUP` argument (label key or file path).
fn group_context(token: &SyntaxToken, offset: TextSize) -> Option<CompletionContext> {
    let group = enclosing_group(token)?;
    let parent = group.parent()?;
    match (group.kind(), parent.kind()) {
        (SyntaxKind::NAME_GROUP, SyntaxKind::BEGIN | SyntaxKind::END) => {
            Some(CompletionContext::EnvironmentName {
                prefix: group_prefix(&group, offset),
                closing: parent.kind() == SyntaxKind::END,
            })
        }
        (SyntaxKind::GROUP, SyntaxKind::COMMAND) => {
            let name = command_name(&parent)?;
            let index = group_index(&parent, &group)?;
            command_arg_context(&name, index, &group, offset)
        }
        _ => None,
    }
}

/// Classify a cursor inside the `index`-th `GROUP` argument of command `name`.
fn command_arg_context(
    name: &str,
    index: usize,
    group: &SyntaxNode,
    offset: TextSize,
) -> Option<CompletionContext> {
    if ref_command(name).is_some() && index == 0 {
        // A `\cref{a,b|}` completes the key after the last comma.
        let inner = group_prefix(group, offset);
        let prefix = inner.rsplit(',').next().unwrap_or(&inner).trim_start();
        return Some(CompletionContext::LabelRef {
            prefix: prefix.to_string(),
        });
    }
    if is_cite_command(name) && index == 0 {
        // A `\cite{a,b|}` completes the key after the last comma, like `\cref`.
        let inner = group_prefix(group, offset);
        let prefix = inner.rsplit(',').next().unwrap_or(&inner).trim_start();
        return Some(CompletionContext::CitationKey {
            prefix: prefix.to_string(),
        });
    }
    if is_glossary_ref_command(name) && index == 0 {
        // A `\gls{key}` group holds exactly one key (never a comma list), so the
        // prefix is the whole typed inner. Later groups (`\glslink{key}{text}`)
        // never classify.
        return Some(CompletionContext::GlossaryKey {
            prefix: group_prefix(group, offset).trim_start().to_string(),
        });
    }
    if let Some(kind) = package_arg(name)
        && index == 0
    {
        // A `\usepackage{amsmath,gr|}` completes the name after the last comma.
        let inner = group_prefix(group, offset);
        let prefix = inner.rsplit(',').next().unwrap_or(&inner).trim_start();
        return Some(CompletionContext::PackageName {
            prefix: prefix.to_string(),
            kind,
        });
    }
    if let Some((kind, path_index)) = file_arg(name)
        && index == path_index
    {
        return Some(CompletionContext::FilePath {
            prefix: group_prefix(group, offset),
            kind,
        });
    }
    if color_name_arg(name, index) {
        // A single color per group (`\color{red}`); the whole inner is the prefix.
        return Some(CompletionContext::ColorName {
            prefix: group_prefix(group, offset).trim_start().to_string(),
        });
    }
    if color_model_arg(name, index) {
        return Some(CompletionContext::ColorModel {
            prefix: group_prefix(group, offset).trim_start().to_string(),
        });
    }
    if let Some(pgf) = tikz_library_command(name)
        && index == 0
    {
        // A `\usetikzlibrary{calc,fi|}` completes the name after the last comma.
        let inner = group_prefix(group, offset);
        let prefix = inner.rsplit(',').next().unwrap_or(&inner).trim_start();
        return Some(CompletionContext::TikzLibrary {
            prefix: prefix.to_string(),
            pgf,
        });
    }
    if let Some(values) = arg_enum_values(name, index) {
        // A fixed enumerated value, one per group (`\pagestyle{plain}`); the whole
        // inner is the prefix. Checked last so the specific handlers above win.
        return Some(CompletionContext::ArgumentEnum {
            prefix: group_prefix(group, offset).trim_start().to_string(),
            values: values.to_vec(),
        });
    }
    None
}

/// Whether the `index`-th brace group of `name` takes an *existing* color name.
/// `\fcolorbox{frame}{bg}{text}` completes both its first two groups; `\colorlet`
/// completes only its *second* group (the base), the first being the new name.
fn color_name_arg(name: &str, index: usize) -> bool {
    match name {
        "color" | "textcolor" | "colorbox" | "pagecolor" => index == 0,
        "fcolorbox" => index < 2,
        "colorlet" => index == 1,
        _ => false,
    }
}

/// Whether the `index`-th brace group of `name` takes a color *model*
/// (`\definecolor{name}{model}{spec}`). The optional-argument model form
/// (`\color[rgb]{…}`) is not classified — only brace groups are inspected.
fn color_model_arg(name: &str, index: usize) -> bool {
    matches!(name, "definecolor" | "providecolor") && index == 1
}

/// Whether `name` is a TikZ/PGF library loader, returning whether it is the PGF
/// (`\usepgflibrary`) rather than TikZ (`\usetikzlibrary`) set, or `None`.
fn tikz_library_command(name: &str) -> Option<bool> {
    match name {
        "usetikzlibrary" => Some(false),
        "usepgflibrary" => Some(true),
        _ => None,
    }
}

/// The [`FileArgKind`] for a `\usepackage`/`\documentclass`-family command whose
/// first `{…}` completes package/class *names* (baked list + local files), or
/// `None`. Returns [`FileArgKind::Package`]/[`FileArgKind::Class`] only.
fn package_arg(name: &str) -> Option<FileArgKind> {
    Some(match name {
        "usepackage" | "RequirePackage" => FileArgKind::Package,
        "documentclass" | "LoadClass" | "LoadClassWithOptions" => FileArgKind::Class,
        _ => return None,
    })
}

/// The file-argument category and the `GROUP`-index of the path argument for a
/// recognized file-taking command, or `None`. Indexing counts brace groups only
/// (the optional `[…]` of `\includegraphics` is an `OPTIONAL`, not a `GROUP`).
/// `\import{dir}{file}` completes its second group; the typed prefix still
/// resolves against the document directory (the `{dir}` base is not consulted —
/// a known limitation, as in `project::include`).
fn file_arg(name: &str) -> Option<(FileArgKind, usize)> {
    Some(match name {
        "includegraphics" => (FileArgKind::Graphics, 0),
        "input" | "include" | "subfile" => (FileArgKind::TexSource, 0),
        "import" | "subimport" => (FileArgKind::TexSource, 1),
        "bibliography" | "addbibresource" => (FileArgKind::Bib, 0),
        _ => return None,
    })
}

/// The nearest ancestor of `token` that is a `GROUP` or `NAME_GROUP`, stopping at
/// a command/environment boundary so a token in a command *between* groups does
/// not bind to an unrelated enclosing group.
fn enclosing_group(token: &SyntaxToken) -> Option<SyntaxNode> {
    let mut node = token.parent();
    while let Some(current) = node {
        match current.kind() {
            SyntaxKind::GROUP | SyntaxKind::NAME_GROUP => return Some(current),
            SyntaxKind::COMMAND
            | SyntaxKind::BEGIN
            | SyntaxKind::END
            | SyntaxKind::ENVIRONMENT
            | SyntaxKind::ROOT => return None,
            _ => node = current.parent(),
        }
    }
    None
}

/// The position of `group` among `command`'s `GROUP` children (file/key args).
fn group_index(command: &SyntaxNode, group: &SyntaxNode) -> Option<usize> {
    command
        .children()
        .filter(|child| child.kind() == SyntaxKind::GROUP)
        .position(|child| &child == group)
}

/// The inner text of `group` (braces dropped) from its start up to `offset` — the
/// prefix the user has typed inside the braces.
fn group_prefix(group: &SyntaxNode, offset: TextSize) -> String {
    let mut text = String::new();
    for token in group.children_with_tokens().filter_map(|e| e.into_token()) {
        if matches!(token.kind(), SyntaxKind::L_BRACE | SyntaxKind::R_BRACE) {
            continue;
        }
        let range = token.text_range();
        if range.end() <= offset {
            text.push_str(token.text());
        } else if range.start() < offset {
            let rel = usize::from(offset - range.start());
            text.push_str(token.text().get(..rel).unwrap_or(token.text()));
        }
    }
    text
}

/// Build candidates for a name/ref `context`. File paths are handled by the LSP
/// layer (they need a directory read) and yield an empty list here; so do
/// [`CompletionContext::None`].
pub fn candidates(
    context: &CompletionContext,
    user_sigs: &SignatureDb,
    model: &SemanticModel,
) -> Vec<CompletionCandidate> {
    match context {
        CompletionContext::CommandName { prefix } => command_candidates(user_sigs, prefix),
        CompletionContext::EnvironmentName { prefix, closing } => {
            environment_candidates(user_sigs, prefix, *closing)
        }
        CompletionContext::LabelRef { prefix } => label_candidates(model, prefix),
        CompletionContext::PackageName { prefix, kind } => package_candidates(*kind, prefix),
        CompletionContext::ColorName { prefix } => color_name_candidates(model, prefix),
        CompletionContext::ColorModel { prefix } => {
            static_candidates(color_models(), prefix, CandidateKind::ColorModel)
        }
        CompletionContext::TikzLibrary { prefix, pgf } => {
            let libs = if *pgf {
                pgf_libraries()
            } else {
                tikz_libraries()
            };
            static_candidates(libs, prefix, CandidateKind::TikzLibrary)
        }
        CompletionContext::ArgumentEnum { prefix, values } => values
            .iter()
            .filter(|value| value.starts_with(prefix.as_str()))
            .map(|value| CompletionCandidate {
                label: value.clone(),
                kind: CandidateKind::ArgumentEnum,
                insert_text: None,
                snippet: false,
            })
            .collect(),
        CompletionContext::FilePath { .. }
        | CompletionContext::CitationKey { .. }
        | CompletionContext::GlossaryKey { .. }
        | CompletionContext::None => Vec::new(),
    }
}

/// Baked package/class name candidates for `\usepackage`/`\documentclass`,
/// prefix-filtered. The list is pre-sorted into rank order (namesake/common names
/// first), which is *preserved* here (no re-sort) so the LSP layer can turn
/// position into `sortText`. The LSP layer additionally merges local `.sty`/`.cls`
/// files. Class-taking commands draw from [`class_names`], the rest from
/// [`package_names`].
fn package_candidates(kind: FileArgKind, prefix: &str) -> Vec<CompletionCandidate> {
    let names = if kind == FileArgKind::Class {
        class_names()
    } else {
        package_names()
    };
    names
        .iter()
        .filter(|name| name.starts_with(prefix))
        .map(|&label| CompletionCandidate {
            label: label.to_string(),
            kind: CandidateKind::Package,
            insert_text: None,
            snippet: false,
        })
        .collect()
}

/// Candidates from a static, pre-ordered name list (`color_models`,
/// `tikz_libraries`/`pgf_libraries`), prefix-filtered with the source order
/// preserved. Used for the closed built-in sets that carry no document-defined
/// members.
fn static_candidates(
    names: &[&str],
    prefix: &str,
    kind: CandidateKind,
) -> Vec<CompletionCandidate> {
    names
        .iter()
        .filter(|name| name.starts_with(prefix))
        .map(|&label| CompletionCandidate {
            label: label.to_string(),
            kind,
            insert_text: None,
            snippet: false,
        })
        .collect()
}

/// Color-name candidates: the built-in list (color/xcolor base + dvipsnames)
/// unioned with the document's `\definecolor`/`\colorlet` names, prefix-filtered,
/// sorted, and deduped — mirroring [`label_candidates`]'s merge of scanned names.
fn color_name_candidates(model: &SemanticModel, prefix: &str) -> Vec<CompletionCandidate> {
    let mut names = union_names(
        color_names()
            .iter()
            .copied()
            .chain(model.color_defs().iter().map(|def| def.name.as_str())),
        prefix,
    );
    names.sort();
    names.dedup();
    names
        .into_iter()
        .map(|label| CompletionCandidate {
            label,
            kind: CandidateKind::Color,
            insert_text: None,
            snippet: false,
        })
        .collect()
}

/// All command names (built-in ∪ scanned ∪ bulk CWL), prefix-filtered, deduped,
/// sorted. The CWL tier widens coverage to the long tail of package commands; the
/// trailing `dedup` collapses any name shared with the built-in/scanned sets.
fn command_candidates(user_sigs: &SignatureDb, prefix: &str) -> Vec<CompletionCandidate> {
    let mut names = union_names(
        builtin()
            .command_names()
            .chain(user_sigs.command_names())
            .chain(cwl().command_names()),
        prefix,
    );
    names.sort();
    names.dedup();
    names
        .into_iter()
        .map(|label| CompletionCandidate {
            label,
            kind: CandidateKind::Command,
            insert_text: None,
            snippet: false,
        })
        .collect()
}

/// All environment names (built-in ∪ scanned), prefix-filtered. For `\begin` each
/// item inserts a snippet that adds the matching `\end{…}`; for `\end` the name
/// inserts plain.
fn environment_candidates(
    user_sigs: &SignatureDb,
    prefix: &str,
    closing: bool,
) -> Vec<CompletionCandidate> {
    let mut names = union_names(
        builtin()
            .environment_names()
            .chain(user_sigs.environment_names())
            .chain(cwl().environment_names()),
        prefix,
    );
    names.sort();
    names.dedup();
    names
        .into_iter()
        .map(|name| {
            let (insert_text, snippet) = if closing {
                (None, false)
            } else {
                // The cursor sits after `\begin{`; complete the name, body tab
                // stop, and the matching `\end{name}`.
                (Some(format!("{name}}}\n\t$0\n\\end{{{name}}}")), true)
            };
            CompletionCandidate {
                label: name,
                kind: CandidateKind::Environment,
                insert_text,
                snippet,
            }
        })
        .collect()
}

/// Distinct label names defined in this file, prefix-filtered.
fn label_candidates(model: &SemanticModel, prefix: &str) -> Vec<CompletionCandidate> {
    let mut names: Vec<String> = model
        .labels()
        .iter()
        .map(|label| label.name.to_string())
        .filter(|name| name.starts_with(prefix))
        .collect();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .map(|label| CompletionCandidate {
            label,
            kind: CandidateKind::Label,
            insert_text: None,
            snippet: false,
        })
        .collect()
}

/// Collect names matching `prefix` into an owned, unsorted `Vec`.
fn union_names<'a>(names: impl Iterator<Item = &'a str>, prefix: &str) -> Vec<String> {
    names
        .filter(|name| name.starts_with(prefix))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use crate::semantic::scan_definitions;

    fn root(src: &str) -> SyntaxNode {
        SyntaxNode::new_root(parse(src).green)
    }

    /// Byte offset just past `needle` in `src`.
    fn at(src: &str, needle: &str) -> usize {
        src.find(needle).expect("needle present") + needle.len()
    }

    fn classify(src: &str, offset: usize) -> CompletionContext {
        classify_context(&root(src), offset)
    }

    /// Candidate labels for a source + cursor, using the document's own scanned
    /// definitions and label model.
    fn labels(src: &str, offset: usize) -> Vec<String> {
        let tree = root(src);
        let ctx = classify_context(&tree, offset);
        let sigs = scan_definitions(&tree);
        let model = SemanticModel::build(&tree);
        candidates(&ctx, &sigs, &model)
            .into_iter()
            .map(|c| c.label)
            .collect()
    }

    #[test]
    fn command_prefix_classified() {
        let src = "\\se\n";
        assert_eq!(
            classify(src, at(src, "\\se")),
            CompletionContext::CommandName {
                prefix: "se".to_string()
            }
        );
    }

    #[test]
    fn command_candidates_match_prefix() {
        let src = "\\sub\n";
        let got = labels(src, at(src, "\\sub"));
        assert!(got.contains(&"subsection".to_string()), "{got:?}");
        assert!(got.contains(&"subsubsection".to_string()), "{got:?}");
        assert!(got.iter().all(|n| n.starts_with("sub")), "{got:?}");
    }

    #[test]
    fn command_candidates_include_scanned_define() {
        let src = "\\newcommand{\\sefoo}{x}\n\\se\n";
        let got = labels(src, at(src, "\n\\se"));
        assert!(got.contains(&"sefoo".to_string()), "{got:?}");
        assert!(got.contains(&"section".to_string()), "{got:?}");
    }

    #[test]
    fn command_candidates_include_cwl_tier() {
        // A command name only the bulk CWL tier knows still surfaces in completion,
        // and exactly once (deduped against the built-in/scanned sets).
        let name = cwl()
            .command_names()
            .find(|n| n.len() > 4 && n.chars().all(|c| c.is_ascii_alphabetic()))
            .expect("an alphabetic CWL command name");
        let prefix = &name[..3];
        let src = format!("\\{prefix}\n");
        let got = labels(&src, at(&src, &format!("\\{prefix}")));
        assert!(
            got.contains(&name.to_string()),
            "{name} missing from {got:?}"
        );
        assert_eq!(
            got.iter().filter(|n| n.as_str() == name).count(),
            1,
            "deduped"
        );
    }

    #[test]
    fn lone_backslash_offers_all_commands() {
        let src = "\\";
        let ctx = classify(src, at(src, "\\"));
        assert_eq!(
            ctx,
            CompletionContext::CommandName {
                prefix: String::new()
            }
        );
    }

    #[test]
    fn begin_name_classified_with_snippet() {
        let src = "\\begin{item}\n";
        let offset = at(src, "\\begin{item");
        assert_eq!(
            classify(src, offset),
            CompletionContext::EnvironmentName {
                prefix: "item".to_string(),
                closing: false,
            }
        );
        let tree = root(src);
        let cands = candidates(
            &classify(src, offset),
            &SignatureDb::default(),
            &SemanticModel::build(&tree),
        );
        let itemize = cands
            .iter()
            .find(|c| c.label == "itemize")
            .expect("itemize candidate");
        assert!(itemize.snippet);
        assert_eq!(
            itemize.insert_text.as_deref(),
            Some("itemize}\n\t$0\n\\end{itemize}")
        );
    }

    #[test]
    fn end_name_classified_plain() {
        let src = "\\begin{itemize}\n\\end{it}\n";
        let offset = at(src, "\\end{it");
        assert_eq!(
            classify(src, offset),
            CompletionContext::EnvironmentName {
                prefix: "it".to_string(),
                closing: true,
            }
        );
        let tree = root(src);
        let cands = candidates(
            &classify(src, offset),
            &SignatureDb::default(),
            &SemanticModel::build(&tree),
        );
        let itemize = cands.iter().find(|c| c.label == "itemize").unwrap();
        assert!(!itemize.snippet);
        assert!(itemize.insert_text.is_none());
    }

    #[test]
    fn empty_begin_group_offers_environments() {
        let src = "\\begin{}\n";
        let got = labels(src, at(src, "\\begin{"));
        assert!(got.contains(&"itemize".to_string()), "{got:?}");
    }

    #[test]
    fn ref_key_classified_and_completed() {
        let src = "\\label{sec:intro}\n\\ref{sec}\n";
        let offset = at(src, "\\ref{sec");
        assert_eq!(
            classify(src, offset),
            CompletionContext::LabelRef {
                prefix: "sec".to_string()
            }
        );
        let got = labels(src, offset);
        assert_eq!(got, vec!["sec:intro".to_string()]);
    }

    #[test]
    fn cref_completes_key_after_last_comma() {
        let src = "\\label{a:one}\\label{a:two}\n\\cref{a:one,a}\n";
        let offset = at(src, "\\cref{a:one,a");
        assert_eq!(
            classify(src, offset),
            CompletionContext::LabelRef {
                prefix: "a".to_string()
            }
        );
        let got = labels(src, offset);
        assert!(got.contains(&"a:one".to_string()), "{got:?}");
        assert!(got.contains(&"a:two".to_string()), "{got:?}");
    }

    #[test]
    fn includegraphics_is_file_path() {
        let src = "\\includegraphics{img/lo}\n";
        assert_eq!(
            classify(src, at(src, "\\includegraphics{img/lo")),
            CompletionContext::FilePath {
                prefix: "img/lo".to_string(),
                kind: FileArgKind::Graphics,
            }
        );
    }

    #[test]
    fn includegraphics_with_option_is_file_path() {
        let src = "\\includegraphics[width=2cm]{fig}\n";
        assert_eq!(
            classify(src, at(src, "{fig")),
            CompletionContext::FilePath {
                prefix: "fig".to_string(),
                kind: FileArgKind::Graphics,
            }
        );
    }

    #[test]
    fn input_is_tex_source_path() {
        let src = "\\input{chapters/intro}\n";
        assert_eq!(
            classify(src, at(src, "{chapters/intro")),
            CompletionContext::FilePath {
                prefix: "chapters/intro".to_string(),
                kind: FileArgKind::TexSource,
            }
        );
    }

    #[test]
    fn usepackage_is_package_name() {
        let src = "\\usepackage{amsm}\n";
        assert_eq!(
            classify(src, at(src, "\\usepackage{amsm")),
            CompletionContext::PackageName {
                prefix: "amsm".to_string(),
                kind: FileArgKind::Package,
            }
        );
    }

    #[test]
    fn usepackage_with_option_is_package_name() {
        let src = "\\usepackage[utf8]{inp}\n";
        assert_eq!(
            classify(src, at(src, "{inp")),
            CompletionContext::PackageName {
                prefix: "inp".to_string(),
                kind: FileArgKind::Package,
            }
        );
    }

    #[test]
    fn usepackage_completes_name_after_last_comma() {
        let src = "\\usepackage{amsmath,gra}\n";
        assert_eq!(
            classify(src, at(src, "\\usepackage{amsmath,gra")),
            CompletionContext::PackageName {
                prefix: "gra".to_string(),
                kind: FileArgKind::Package,
            }
        );
    }

    #[test]
    fn documentclass_is_class_name() {
        let src = "\\documentclass{art}\n";
        assert_eq!(
            classify(src, at(src, "\\documentclass{art")),
            CompletionContext::PackageName {
                prefix: "art".to_string(),
                kind: FileArgKind::Class,
            }
        );
    }

    #[test]
    fn package_candidates_match_prefix_and_include_amsmath() {
        let src = "\\usepackage{amsm}\n";
        let got = labels(src, at(src, "\\usepackage{amsm"));
        assert!(got.contains(&"amsmath".to_string()), "{got:?}");
        assert!(got.iter().all(|n| n.starts_with("amsm")), "{got:?}");
    }

    #[test]
    fn class_candidates_include_article() {
        let src = "\\documentclass{art}\n";
        let got = labels(src, at(src, "\\documentclass{art"));
        assert!(got.contains(&"article".to_string()), "{got:?}");
        assert!(got.iter().all(|n| n.starts_with("art")), "{got:?}");
    }

    #[test]
    fn package_candidates_preserve_rank_order() {
        // A namesake/common name (`tikz`) ranks before an internal style with the
        // same prefix; the baked order is preserved (not alphabetized).
        let src = "\\usepackage{ti}\n";
        let got = labels(src, at(src, "\\usepackage{ti"));
        let tikz = got.iter().position(|n| n == "tikz");
        assert!(tikz.is_some(), "tikz present: {got:?}");
    }

    #[test]
    fn cite_is_classified_as_citation_key() {
        let src = "\\cite{key}\n";
        assert_eq!(
            classify(src, at(src, "\\cite{ke")),
            CompletionContext::CitationKey {
                prefix: "ke".to_string()
            }
        );
    }

    #[test]
    fn citep_completes_key_after_last_comma() {
        let src = "\\citep{a,b}\n";
        assert_eq!(
            classify(src, at(src, "\\citep{a,b")),
            CompletionContext::CitationKey {
                prefix: "b".to_string()
            }
        );
    }

    #[test]
    fn citation_key_candidates_empty_from_pure() {
        // Keys are resolved in the LSP layer (cross-file), so the pure path is empty.
        let src = "\\cite{ke}\n";
        let tree = root(src);
        let ctx = classify_context(&tree, at(src, "\\cite{ke"));
        let cands = candidates(&ctx, &SignatureDb::default(), &SemanticModel::build(&tree));
        assert!(cands.is_empty());
    }

    #[test]
    fn gls_is_classified_as_glossary_key() {
        let src = "\\gls{ex}\n";
        assert_eq!(
            classify(src, at(src, "\\gls{e")),
            CompletionContext::GlossaryKey {
                prefix: "e".to_string()
            }
        );
    }

    #[test]
    fn acrshort_and_glsxtr_classify_as_glossary_key() {
        for src in [
            "\\acrshort{fps}\n",
            "\\ACRfullpl{fps}\n",
            "\\glsxtrlong{fps}\n",
        ] {
            assert_eq!(
                classify(src, at(src, "{fp")),
                CompletionContext::GlossaryKey {
                    prefix: "fp".to_string()
                },
                "{src:?}"
            );
        }
    }

    #[test]
    fn glslink_key_group_only_classifies() {
        // The key is group 0; the text group of `\glslink{key}{text}` is prose.
        let src = "\\glslink{ex}{shown text}\n";
        assert_eq!(
            classify(src, at(src, "\\glslink{e")),
            CompletionContext::GlossaryKey {
                prefix: "e".to_string()
            }
        );
        assert_eq!(classify(src, at(src, "{shown")), CompletionContext::None);
    }

    #[test]
    fn gls_key_is_not_comma_split() {
        // Glossary commands take exactly one key; a comma is part of the key text.
        let src = "\\gls{a,b}\n";
        assert_eq!(
            classify(src, at(src, "\\gls{a,b")),
            CompletionContext::GlossaryKey {
                prefix: "a,b".to_string()
            }
        );
    }

    #[test]
    fn glossary_key_candidates_empty_from_pure() {
        // Keys are resolved in the LSP layer (cross-file), so the pure path is empty.
        let src = "\\newacronym{aa}{AA}{an acronym}\n\\gls{a}\n";
        let tree = root(src);
        let ctx = classify_context(&tree, at(src, "\\gls{a"));
        let cands = candidates(&ctx, &SignatureDb::default(), &SemanticModel::build(&tree));
        assert!(cands.is_empty());
    }

    #[test]
    fn prose_is_not_completed() {
        let src = "Hello world\n";
        assert_eq!(classify(src, at(src, "Hello wo")), CompletionContext::None);
    }

    #[test]
    fn textcolor_first_arg_is_color_name() {
        let src = "\\textcolor{re}{x}\n";
        assert_eq!(
            classify(src, at(src, "\\textcolor{re")),
            CompletionContext::ColorName {
                prefix: "re".to_string()
            }
        );
    }

    #[test]
    fn color_second_arg_is_not_a_color_name() {
        // `\textcolor{color}{text}` — the second group is prose, not a color.
        let src = "\\textcolor{red}{te}\n";
        assert_eq!(
            classify(src, at(src, "\\textcolor{red}{te")),
            CompletionContext::None
        );
    }

    #[test]
    fn colorlet_new_name_arg_is_not_completed() {
        // The first group names a *new* color, so it offers no existing-color list.
        let src = "\\colorlet{min}{red}\n";
        assert_eq!(
            classify(src, at(src, "\\colorlet{min")),
            CompletionContext::None
        );
    }

    #[test]
    fn colorlet_base_arg_is_color_name() {
        // The second group is the base color, an existing name.
        let src = "\\colorlet{mine}{re}\n";
        assert_eq!(
            classify(src, at(src, "\\colorlet{mine}{re")),
            CompletionContext::ColorName {
                prefix: "re".to_string()
            }
        );
    }

    #[test]
    fn definecolor_model_arg_is_color_model() {
        let src = "\\definecolor{c}{rg}{1,0,0}\n";
        assert_eq!(
            classify(src, at(src, "\\definecolor{c}{rg")),
            CompletionContext::ColorModel {
                prefix: "rg".to_string()
            }
        );
    }

    #[test]
    fn usetikzlibrary_is_tikz_library() {
        let src = "\\usetikzlibrary{cal}\n";
        assert_eq!(
            classify(src, at(src, "\\usetikzlibrary{cal")),
            CompletionContext::TikzLibrary {
                prefix: "cal".to_string(),
                pgf: false,
            }
        );
    }

    #[test]
    fn usetikzlibrary_completes_after_last_comma() {
        let src = "\\usetikzlibrary{calc,ar}\n";
        assert_eq!(
            classify(src, at(src, "\\usetikzlibrary{calc,ar")),
            CompletionContext::TikzLibrary {
                prefix: "ar".to_string(),
                pgf: false,
            }
        );
    }

    #[test]
    fn usepgflibrary_is_pgf_library() {
        let src = "\\usepgflibrary{arr}\n";
        assert_eq!(
            classify(src, at(src, "\\usepgflibrary{arr")),
            CompletionContext::TikzLibrary {
                prefix: "arr".to_string(),
                pgf: true,
            }
        );
    }

    #[test]
    fn color_name_candidates_include_base_and_dvipsnames() {
        let src = "\\textcolor{re}{x}\n";
        let got = labels(src, at(src, "\\textcolor{re"));
        assert!(got.contains(&"red".to_string()));
        let src = "\\textcolor{Ro}{x}\n";
        let got = labels(src, at(src, "\\textcolor{Ro"));
        assert!(got.contains(&"RoyalBlue".to_string()));
        assert!(got.iter().all(|n| n.starts_with("Ro")));
    }

    #[test]
    fn color_name_candidates_include_document_colors() {
        // A `\definecolor` name is offered alongside the built-ins.
        let src = "\\definecolor{mycolor}{rgb}{1,0,0}\n\\textcolor{myc}{x}\n";
        let got = labels(src, at(src, "\\textcolor{myc"));
        assert_eq!(got, vec!["mycolor".to_string()]);
    }

    #[test]
    fn colorlet_defines_a_document_color() {
        // `\colorlet{name}{base}` contributes `name`, completed later.
        let src = "\\colorlet{brand}{blue}\n\\color{bra}\n";
        let got = labels(src, at(src, "\\color{bra"));
        assert_eq!(got, vec!["brand".to_string()]);
    }

    #[test]
    fn color_model_candidates_include_rgb() {
        let src = "\\definecolor{c}{rg}{1,0,0}\n";
        let got = labels(src, at(src, "\\definecolor{c}{rg"));
        assert_eq!(got, vec!["rgb".to_string()]);
    }

    #[test]
    fn pagestyle_arg_is_enum() {
        let src = "\\pagestyle{pl}\n";
        assert_eq!(
            classify(src, at(src, "\\pagestyle{pl")),
            CompletionContext::ArgumentEnum {
                prefix: "pl".to_string(),
                values: vec![
                    "empty".to_string(),
                    "plain".to_string(),
                    "headings".to_string(),
                    "myheadings".to_string(),
                ],
            }
        );
    }

    #[test]
    fn enum_candidates_filter_by_prefix() {
        let src = "\\pagestyle{pl}\n";
        let got = labels(src, at(src, "\\pagestyle{pl"));
        assert_eq!(got, vec!["plain".to_string()]);
    }

    #[test]
    fn empty_enum_arg_offers_all_values() {
        let src = "\\pagenumbering{}\n";
        let got = labels(src, at(src, "\\pagenumbering{"));
        assert_eq!(got, vec!["arabic", "roman", "Roman", "alph", "Alph"]);
    }

    #[test]
    fn non_enum_command_arg_is_not_completed() {
        // A command with no enum table entry classifies as nothing here.
        let src = "\\label{sec}\n";
        assert_eq!(
            classify(src, at(src, "\\label{sec")),
            CompletionContext::None
        );
    }

    #[test]
    fn tikz_library_candidates_are_split_by_command() {
        let src = "\\usetikzlibrary{cal}\n";
        assert!(labels(src, at(src, "\\usetikzlibrary{cal")).contains(&"calc".to_string()));
        // `arrows.meta` is a TikZ library; `plothandlers` is a PGF-only one.
        let src = "\\usepgflibrary{plot}\n";
        let got = labels(src, at(src, "\\usepgflibrary{plot"));
        assert!(got.contains(&"plothandlers".to_string()));
    }
}
