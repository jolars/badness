//! Static **recognition** of the package/class authoring commands — the metadata a
//! `.sty`/`.cls` declares about itself. We read the declared facts only; nothing is
//! ever executed (AGENTS.md non-goals).
//!
//! Three declarations are extracted, mirroring how `\definecolor`/`\newglossaryentry`
//! feed the [`SemanticModel`](super::SemanticModel):
//!
//! - **`\ProvidesPackage`/`\ProvidesClass`/`\ProvidesFile`** and the expl3
//!   **`\ProvidesExplPackage`/`Class`/`File`** — the package/class *identity*
//!   (name, date, version, description) → [`ProvidesDecl`].
//! - **`\NeedsTeXFormat`** — the required format and optional release date →
//!   [`NeedsFormatDecl`].
//! - **`\DeclareOption`** (and the starred default handler `\DeclareOption*`) — a
//!   declared option name → [`OptionDecl`].
//!
//! `\ProcessOptions`/`\ExecuteOptions` carry no extractable identity, so they get no
//! model entry — the LSP hover renders a static note for them.
//!
//! The extraction helpers are `pub` and populate the [`SemanticModel`](super::SemanticModel)
//! in the builder's single CST walk; the LSP hover then matches the cursor against the
//! stored declarations by their control-word range, so extraction lives in one place.
//! The forthcoming package-aware diagnostics (TODO.md) consume the same model fields.

use rowan::TextRange;
use smol_str::SmolStr;

use crate::ast::{AstNode, Optional, child, command_name, control_word_range, nth_group_text};
use crate::syntax::{SyntaxKind, SyntaxNode};

/// Which of the three `\Provides…` namespaces a declaration names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvidesKind {
    Package,
    Class,
    File,
}

impl ProvidesKind {
    /// The lowercase noun for rendering (`package`, `class`, `file`).
    pub fn noun(self) -> &'static str {
        match self {
            ProvidesKind::Package => "package",
            ProvidesKind::Class => "class",
            ProvidesKind::File => "file",
        }
    }
}

/// A `\ProvidesPackage`/`Class`/`File` (or its expl3 variant) self-identification.
///
/// The LaTeX2e form is `\ProvidesPackage{name}[date version and other info]`; the
/// bracket's free text is kept verbatim in [`info`](Self::info), with a best-effort
/// split into [`date`](Self::date) and [`version`](Self::version). The expl3 form
/// `\ProvidesExplPackage{name}{date}{version}{description}` fills the fields from its
/// four braced groups directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvidesDecl {
    pub kind: ProvidesKind,
    pub name: SmolStr,
    /// The raw `[date version desc]` bracket text (LaTeX2e form), or the description
    /// group (expl3 form). `None` when absent.
    pub info: Option<SmolStr>,
    /// Best-effort release date (`YYYY/MM/DD`), split from `info` or the expl3 date
    /// group.
    pub date: Option<SmolStr>,
    /// Best-effort version (a `v`-prefixed or digit-leading token), split from `info`
    /// or the expl3 version group.
    pub version: Option<SmolStr>,
    /// The `\ProvidesPackage` control-word token range — the hover anchor and the key
    /// the LSP matches the cursor against.
    pub range: TextRange,
}

/// A `\NeedsTeXFormat{format}[date]` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeedsFormatDecl {
    pub format: SmolStr,
    pub date: Option<SmolStr>,
    /// The `\NeedsTeXFormat` control-word token range.
    pub range: TextRange,
}

/// A `\DeclareOption{name}{code}` declaration, or the starred default handler
/// `\DeclareOption*{code}` (with [`name`](Self::name) `None`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionDecl {
    /// The declared option name, or `None` for the `\DeclareOption*` default handler.
    pub name: Option<SmolStr>,
    /// The `\DeclareOption` control-word token range.
    pub range: TextRange,
}

/// Whether `name` is a `\Provides…` declaration, and which namespace it names.
pub fn provides_kind(name: &str) -> Option<(ProvidesKind, ProvidesForm)> {
    Some(match name {
        "ProvidesPackage" => (ProvidesKind::Package, ProvidesForm::Latex2e),
        "ProvidesClass" => (ProvidesKind::Class, ProvidesForm::Latex2e),
        "ProvidesFile" => (ProvidesKind::File, ProvidesForm::Latex2e),
        "ProvidesExplPackage" => (ProvidesKind::Package, ProvidesForm::Expl3),
        "ProvidesExplClass" => (ProvidesKind::Class, ProvidesForm::Expl3),
        "ProvidesExplFile" => (ProvidesKind::File, ProvidesForm::Expl3),
        _ => return None,
    })
}

/// The argument shape of a `\Provides…` declaration: the LaTeX2e `{name}[info]` or the
/// expl3 `{name}{date}{version}{description}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvidesForm {
    Latex2e,
    Expl3,
}

/// Extract a [`ProvidesDecl`] from a `\Provides…` `COMMAND` node, or `None` if the
/// name group is a non-literal (nested macro) or the command is not a `\Provides…`.
pub fn provides_from_command(command: &SyntaxNode) -> Option<ProvidesDecl> {
    let name = command_name(command)?;
    let (kind, form) = provides_kind(&name)?;
    let range = control_word_range(command)?;
    let pkg_name = nth_group_text(command, 0)?;

    let (info, date, version) = match form {
        ProvidesForm::Latex2e => {
            // `[date version and other info]` is an OPTIONAL node, not a GROUP, so it
            // never shifts the name's group index. Keep the free text verbatim and
            // split it best-effort.
            match first_optional_text(command) {
                Some(info) => {
                    let (date, version) = split_date_version(&info);
                    (Some(SmolStr::from(info.trim())), date, version)
                }
                None => (None, None, None),
            }
        }
        ProvidesForm::Expl3 => {
            // `{name}{date}{version}{description}` — fields straight from the groups.
            let date = nonempty(nth_group_text(command, 1));
            let version = nonempty(nth_group_text(command, 2));
            let desc = nonempty(nth_group_text(command, 3));
            (desc, date, version)
        }
    };

    Some(ProvidesDecl {
        kind,
        name: SmolStr::from(pkg_name.trim()),
        info,
        date,
        version,
        range,
    })
}

/// Extract a [`NeedsFormatDecl`] from a `\NeedsTeXFormat` `COMMAND` node.
pub fn needs_format_from_command(command: &SyntaxNode) -> Option<NeedsFormatDecl> {
    if command_name(command).as_deref() != Some("NeedsTeXFormat") {
        return None;
    }
    let range = control_word_range(command)?;
    let format = nth_group_text(command, 0)?;
    let date = first_optional_text(command).and_then(|t| nonempty(Some(t)));
    Some(NeedsFormatDecl {
        format: SmolStr::from(format.trim()),
        date,
        range,
    })
}

/// Extract an [`OptionDecl`] from a `\DeclareOption` `COMMAND` node. The non-star form
/// `\DeclareOption{name}{code}` reads the name from group 0; the starred default
/// handler `\DeclareOption*{code}` (recognized by a following `*` `WORD` sibling the
/// greedy parser leaves unattached) records `name: None`.
pub fn option_from_command(command: &SyntaxNode) -> Option<OptionDecl> {
    if command_name(command).as_deref() != Some("DeclareOption") {
        return None;
    }
    let range = control_word_range(command)?;
    let name = if has_trailing_star(command) {
        None
    } else {
        nth_group_text(command, 0).map(|n| SmolStr::from(n.trim()))
    };
    Some(OptionDecl { name, range })
}

/// The text inside the first `OPTIONAL` (`[…]`) child of `command`, braces stripped.
/// `None` when there is no optional or it holds non-literal content — a nested macro
/// in the bracket makes the whole thing unresolvable, like [`nth_group_text`].
fn first_optional_text(command: &SyntaxNode) -> Option<String> {
    let optional = child::<Optional>(command)?;
    let mut text = String::new();
    for element in optional.syntax().children_with_tokens() {
        match element {
            rowan::NodeOrToken::Token(token) => match token.kind() {
                SyntaxKind::L_BRACKET | SyntaxKind::R_BRACKET => {}
                _ => text.push_str(token.text()),
            },
            rowan::NodeOrToken::Node(_) => return None,
        }
    }
    Some(text)
}

/// Whether a `*` `WORD` immediately follows `command`'s control word (the starred
/// form). The greedy parser attaches no `*`, leaving it a sibling `WORD` token; a
/// starred command therefore has no attached groups, so the `*` is the command's next
/// meaningful sibling token.
fn has_trailing_star(command: &SyntaxNode) -> bool {
    let mut sibling = command.next_sibling_or_token();
    while let Some(el) = sibling {
        match el {
            rowan::NodeOrToken::Token(token) => match token.kind() {
                SyntaxKind::WHITESPACE | SyntaxKind::COMMENT => {
                    sibling = token.next_sibling_or_token();
                }
                SyntaxKind::WORD => return token.text() == "*",
                _ => return false,
            },
            rowan::NodeOrToken::Node(_) => return false,
        }
    }
    false
}

/// Best-effort split of a LaTeX2e `[date version and other info]` free-text bracket:
/// a leading `YYYY/MM/DD`-shaped field becomes the date, and a `v`-prefixed or
/// digit-leading token becomes the version. Everything is heuristic and free text, so
/// a miss just leaves the field `None` (the raw `info` is always kept).
fn split_date_version(info: &str) -> (Option<SmolStr>, Option<SmolStr>) {
    let mut fields = info.split_whitespace();
    let date = fields.next().filter(|f| is_date_like(f)).map(SmolStr::from);
    let version = info
        .split_whitespace()
        .find(|f| is_version_like(f))
        .map(SmolStr::from);
    (date, version)
}

/// A `YYYY/MM/DD`-shaped token (the LaTeX package date convention).
fn is_date_like(field: &str) -> bool {
    let parts: Vec<&str> = field.split('/').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// A version-ish token: a `v`/`V` prefix followed by a digit (`v1.2`), or a bare
/// digit-leading token that is not itself a date.
fn is_version_like(field: &str) -> bool {
    if is_date_like(field) {
        return false;
    }
    let mut bytes = field.bytes();
    match bytes.next() {
        Some(b'v') | Some(b'V') => field[1..]
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_digit()),
        _ => false,
    }
}

/// Trim a group's text and drop it if empty.
fn nonempty(text: Option<String>) -> Option<SmolStr> {
    text.map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .map(SmolStr::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use crate::syntax::SyntaxNode;

    fn command_named(src: &str, name: &str) -> SyntaxNode {
        let root = SyntaxNode::new_root(parse(src).green);
        root.descendants()
            .filter(|n| n.kind() == SyntaxKind::COMMAND)
            .find(|n| command_name(n).as_deref() == Some(name))
            .expect("command present")
    }

    #[test]
    fn provides_package_latex2e() {
        let cmd = command_named(
            "\\ProvidesPackage{mypkg}[2024/01/01 v1.2 My package]\n",
            "ProvidesPackage",
        );
        let decl = provides_from_command(&cmd).expect("extracted");
        assert_eq!(decl.kind, ProvidesKind::Package);
        assert_eq!(decl.name, "mypkg");
        assert_eq!(decl.date.as_deref(), Some("2024/01/01"));
        assert_eq!(decl.version.as_deref(), Some("v1.2"));
        assert_eq!(decl.info.as_deref(), Some("2024/01/01 v1.2 My package"));
    }

    #[test]
    fn provides_class_no_bracket() {
        let cmd = command_named("\\ProvidesClass{myclass}\n", "ProvidesClass");
        let decl = provides_from_command(&cmd).expect("extracted");
        assert_eq!(decl.kind, ProvidesKind::Class);
        assert_eq!(decl.name, "myclass");
        assert_eq!(decl.info, None);
        assert_eq!(decl.date, None);
        assert_eq!(decl.version, None);
    }

    #[test]
    fn provides_expl_package_four_groups() {
        let cmd = command_named(
            "\\ProvidesExplPackage{mypkg}{2024/01/01}{1.2}{My package}\n",
            "ProvidesExplPackage",
        );
        let decl = provides_from_command(&cmd).expect("extracted");
        assert_eq!(decl.name, "mypkg");
        assert_eq!(decl.date.as_deref(), Some("2024/01/01"));
        assert_eq!(decl.version.as_deref(), Some("1.2"));
        assert_eq!(decl.info.as_deref(), Some("My package"));
    }

    #[test]
    fn needs_tex_format() {
        let cmd = command_named("\\NeedsTeXFormat{LaTeX2e}[2020/10/01]\n", "NeedsTeXFormat");
        let decl = needs_format_from_command(&cmd).expect("extracted");
        assert_eq!(decl.format, "LaTeX2e");
        assert_eq!(decl.date.as_deref(), Some("2020/10/01"));
    }

    #[test]
    fn declare_option_named() {
        let cmd = command_named("\\DeclareOption{draft}{\\@draft}\n", "DeclareOption");
        let decl = option_from_command(&cmd).expect("extracted");
        assert_eq!(decl.name.as_deref(), Some("draft"));
    }

    #[test]
    fn declare_option_star_is_default_handler() {
        let cmd = command_named(
            "\\DeclareOption*{\\PassOptionsToPackage{\\CurrentOption}{base}}\n",
            "DeclareOption",
        );
        let decl = option_from_command(&cmd).expect("extracted");
        assert_eq!(decl.name, None);
    }

    #[test]
    fn nested_macro_name_is_skipped() {
        // A non-literal name group yields `None`, conservative like `\label{\foo}`.
        let cmd = command_named(
            "\\ProvidesPackage{\\jobname}[2024/01/01]\n",
            "ProvidesPackage",
        );
        assert_eq!(provides_from_command(&cmd), None);
    }
}
