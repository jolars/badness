//! Associate `.dtx` documentation prose with the macro/environment it documents.
//!
//! A `.dtx` brackets each documented entity with ltxdoc vocabulary — a `macro` or
//! `environment` environment, or a `\DescribeMacro`/`\DescribeEnv` command — while
//! the implementation lives in a (usually nested) `macrocode` block. The parser
//! keeps these apart on purpose: a documentation margin is `DOC_MARGIN` trivia that
//! never binds into a `DOC_COMMENT` (AGENTS.md decision #9 forbids a signature
//! lookup in the trivia-binding decision). Connecting prose to code is therefore a
//! *semantic*-layer job — this query.
//!
//! It mirrors [`outline`](super::outline): a single CST walk producing LSP-agnostic
//! [`DocAssociation`]s (byte ranges, no `lsp_types`), unit-testable without the
//! language server. The ltxdoc set is static and standard, so — like the sectioning
//! commands and `\label` in [`outline`](super::outline) — the constructs are
//! recognized by name rather than through a per-document signature scan.
//!
//! The implementation a documented macro brackets is found *structurally*: the code
//! is the `macrocode`/`macrocode*` block(s) nested inside the documenting
//! environment, the conventional `.dtx` idiom. Descent stops at a nested `macro`/
//! `environment` so its `macrocode` is attributed to it, not the outer construct.
//! `\DescribeMacro`/`\DescribeEnv` carry no nested code (the definition lives
//! elsewhere; file-wide def-site linking is a deferred follow-up).

use rowan::TextRange;

use crate::ast::{
    command_name, environment_name, first_group_range, group_command_name, nth_group,
};
use crate::syntax::{SyntaxKind, SyntaxNode};

/// Which ltxdoc construct introduced the association.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocKind {
    /// A `\begin{macro}{\foo}…\end{macro}` documentation environment.
    Macro,
    /// A `\begin{environment}{name}…\end{environment}` documentation environment.
    Environment,
    /// A `\DescribeMacro{\foo}` (or `\DescribeMacro\foo`) command.
    DescribeMacro,
    /// A `\DescribeEnv{name}` command.
    DescribeEnv,
}

/// One documented entity: the documenting construct, the name it documents, and any
/// code it brackets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocAssociation {
    /// The documented name: `\foo` (backslash kept) for a macro, `tabular` for an
    /// environment.
    pub name: String,
    pub kind: DocKind,
    /// The full extent of the documenting construct.
    pub range: TextRange,
    /// The name-argument sub-range to highlight on selection (always `⊆ range`).
    pub name_range: TextRange,
    /// Ranges of the `macrocode`/`macrocode*` blocks nested inside the construct —
    /// the implementation it documents. Empty for `\DescribeMacro`/`\DescribeEnv`
    /// and for a prose-only `macro`/`environment` environment.
    pub code: Vec<TextRange>,
}

/// Collect every documentation↔code association in `root`, in document order.
pub fn doc_associations(root: &SyntaxNode) -> Vec<DocAssociation> {
    let mut out = Vec::new();
    collect(root, &mut out);
    out
}

/// Walk `node`'s children in document order. `COMMAND`/`ENVIRONMENT` are classified;
/// every other container is transparent (recursed into) so a documented construct
/// nested inside still surfaces.
fn collect(node: &SyntaxNode, out: &mut Vec<DocAssociation>) {
    for child in node.children() {
        match child.kind() {
            SyntaxKind::COMMAND => collect_command(&child, out),
            SyntaxKind::ENVIRONMENT => collect_environment(&child, out),
            _ => collect(&child, out),
        }
    }
}

/// Emit an association for a `macro`/`environment` documentation environment, then
/// recurse into its body so a nested documented construct also surfaces. A
/// non-ltxdoc environment is transparent.
fn collect_environment(env: &SyntaxNode, out: &mut Vec<DocAssociation>) {
    // The name and the documented-entity argument both live on the `\begin` node.
    let begin = env.children().find(|c| c.kind() == SyntaxKind::BEGIN);
    let kind = begin
        .as_ref()
        .and_then(environment_name)
        .and_then(|name| match name.as_str() {
            "macro" => Some(DocKind::Macro),
            "environment" => Some(DocKind::Environment),
            _ => None,
        });

    if let (Some(kind), Some(begin)) = (kind, begin.as_ref())
        && let Some(group) = nth_group(begin, 0)
        && let Some(name) = documented_name(&group, kind)
    {
        let mut code = Vec::new();
        collect_code(env, &mut code);
        out.push(DocAssociation {
            name,
            kind,
            range: env.text_range(),
            name_range: group.text_range(),
            code,
        });
    }

    // Recurse regardless: nested `macro` envs surface as their own (flat)
    // associations, and a non-ltxdoc environment is transparent.
    collect(env, out);
}

/// Emit an association for a `\DescribeMacro`/`\DescribeEnv` command.
fn collect_command(command: &SyntaxNode, out: &mut Vec<DocAssociation>) {
    let Some(cmd) = command_name(command) else {
        return;
    };
    let kind = match cmd.as_str() {
        "DescribeMacro" => DocKind::DescribeMacro,
        "DescribeEnv" => DocKind::DescribeEnv,
        _ => return,
    };

    // The braced form `\DescribeMacro{\foo}` carries the name as the first group;
    // the conventional braceless form `\DescribeMacro\foo` carries it as the next
    // sibling command (control words are not greedily attached as arguments, so the
    // macro is a sibling — AGENTS.md decision #8).
    if let Some(group) = nth_group(command, 0) {
        if let Some(name) = documented_name(&group, kind) {
            out.push(DocAssociation {
                name,
                kind,
                range: first_group_range(command),
                name_range: group.text_range(),
                code: Vec::new(),
            });
        }
    } else if kind == DocKind::DescribeMacro
        && let Some(sib) = command.next_sibling()
        && sib.kind() == SyntaxKind::COMMAND
        && let Some(name) = command_name(&sib)
    {
        out.push(DocAssociation {
            name: format!("\\{name}"),
            kind,
            range: TextRange::new(command.text_range().start(), sib.text_range().end()),
            name_range: sib.text_range(),
            code: Vec::new(),
        });
    }
}

/// The documented name carried by a braced argument `group`: a macro's control word
/// (re-prefixed with `\`) for the macro forms, or the trimmed literal for the
/// environment forms. `None` when the argument is empty or holds nested macros —
/// matching [`outline`](super::outline)'s conservative `\label` handling.
fn documented_name(group: &SyntaxNode, kind: DocKind) -> Option<String> {
    match kind {
        DocKind::Macro | DocKind::DescribeMacro => {
            group_command_name(group).map(|name| format!("\\{name}"))
        }
        DocKind::Environment | DocKind::DescribeEnv => {
            // The env name is a flat literal. `crate::ast::nth_group_text` does this
            // but keys on a parent + index; we already hold the group node, so read
            // it directly via the group-level twin below.
            let text = group_inner_text(group)?;
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_owned())
        }
    }
}

/// The flat literal text inside `group`, braces dropped. `None` if it holds a nested
/// node (a `\macro` — not a flat literal). Mirrors [`crate::ast::nth_group_text`],
/// which operates on a parent + index rather than a group node.
fn group_inner_text(group: &SyntaxNode) -> Option<String> {
    let mut text = String::new();
    for element in group.children_with_tokens() {
        match element {
            rowan::NodeOrToken::Token(token) => match token.kind() {
                SyntaxKind::L_BRACE | SyntaxKind::R_BRACE => {}
                _ => text.push_str(token.text()),
            },
            rowan::NodeOrToken::Node(_) => return None,
        }
    }
    Some(text)
}

/// Collect the ranges of `macrocode`/`macrocode*` environments nested in `node`,
/// not descending into a nested `macro`/`environment` (whose code belongs to it).
fn collect_code(node: &SyntaxNode, out: &mut Vec<TextRange>) {
    for child in node.children() {
        if child.kind() == SyntaxKind::ENVIRONMENT {
            let name = child
                .children()
                .find(|c| c.kind() == SyntaxKind::BEGIN)
                .and_then(|b| environment_name(&b));
            match name.as_deref() {
                Some("macrocode" | "macrocode*") => out.push(child.text_range()),
                // A nested documented construct owns its own code; stop here.
                Some("macro" | "environment") => {}
                _ => collect_code(&child, out),
            }
        } else {
            collect_code(&child, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{LatexFlavor, LexConfig, parse_with_flavor};

    fn assoc_of(src: &str) -> Vec<DocAssociation> {
        let config = LexConfig {
            flavor: LatexFlavor::Document,
            dtx: true,
        };
        let parsed = parse_with_flavor(src, config);
        assert_eq!(parsed.syntax().to_string(), src, "losslessness violated");
        doc_associations(&parsed.syntax())
    }

    #[test]
    fn macro_env_documents_its_name() {
        let items = assoc_of("% \\begin{macro}{\\foo}\n% docs.\n% \\end{macro}\n");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "\\foo");
        assert_eq!(items[0].kind, DocKind::Macro);
        assert!(items[0].code.is_empty());
    }

    #[test]
    fn nested_macrocode_is_the_code() {
        let src = "% \\begin{macro}{\\foo}\n% docs.\n%    \\begin{macrocode}\n\\def\\foo{x}\n%    \\end{macrocode}\n% \\end{macro}\n";
        let items = assoc_of(src);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "\\foo");
        assert_eq!(items[0].code.len(), 1);
        // The recorded range is the `macrocode` environment.
        let code = &src[items[0].code[0]];
        assert!(code.starts_with("\\begin{macrocode}"));
        assert!(code.contains("\\def\\foo{x}"));
    }

    #[test]
    fn environment_env_documents_a_plain_name() {
        let items = assoc_of("% \\begin{environment}{myenv}\n% docs.\n% \\end{environment}\n");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "myenv");
        assert_eq!(items[0].kind, DocKind::Environment);
    }

    #[test]
    fn describe_macro_braced() {
        let items = assoc_of("% \\DescribeMacro{\\foo} does foo.\n");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "\\foo");
        assert_eq!(items[0].kind, DocKind::DescribeMacro);
        assert!(items[0].code.is_empty());
    }

    #[test]
    fn describe_macro_braceless() {
        // The conventional doctools form takes its argument without braces.
        let items = assoc_of("% \\DescribeMacro\\foo does foo.\n");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "\\foo");
        assert_eq!(items[0].kind, DocKind::DescribeMacro);
    }

    #[test]
    fn describe_env() {
        let items = assoc_of("% \\DescribeEnv{myenv} is an env.\n");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "myenv");
        assert_eq!(items[0].kind, DocKind::DescribeEnv);
    }

    #[test]
    fn nested_macro_envs_both_surface_with_own_code() {
        let src = "% \\begin{macro}{\\outer}\n%    \\begin{macrocode}\n\\def\\outer{o}\n%    \\end{macrocode}\n% \\begin{macro}{\\inner}\n%    \\begin{macrocode}\n\\def\\inner{i}\n%    \\end{macrocode}\n% \\end{macro}\n% \\end{macro}\n";
        let items = assoc_of(src);
        let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["\\outer", "\\inner"]);
        // The outer construct's code stops at the nested `macro`: it owns only its
        // own `macrocode`, not the inner one.
        let outer = &items[0];
        assert_eq!(outer.code.len(), 1);
        assert!(src[outer.code[0]].contains("\\def\\outer{o}"));
        let inner = &items[1];
        assert_eq!(inner.code.len(), 1);
        assert!(src[inner.code[0]].contains("\\def\\inner{i}"));
    }

    #[test]
    fn empty_or_nested_macro_name_is_skipped() {
        assert!(assoc_of("% \\begin{macro}{}\n% \\end{macro}\n").is_empty());
        // `{\foo\bar}` is not a single flat control word; environment name with a
        // nested macro is likewise skipped.
        assert!(assoc_of("% \\DescribeEnv{\\foo}\n").is_empty());
    }

    #[test]
    fn non_ltxdoc_constructs_are_ignored() {
        let items =
            assoc_of("% \\section{Intro}\n% \\begin{itemize}\n% \\item x\n% \\end{itemize}\n");
        assert!(items.is_empty());
    }
}
