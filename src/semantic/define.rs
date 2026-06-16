//! Scan a document for **user definitions** — `\newcommand`/`\newenvironment` and
//! the xparse `\NewDocument…` family — and extract their argument *signatures* into
//! a per-document [`SignatureDb`]. Signatures only: we read the declared argument
//! shape, never the replacement text, and never execute anything (AGENTS.md
//! non-goals and decision #1).
//!
//! A single whole-tree walk (mirror of [`super::builder::build`]) collects every
//! definition; the result overlays the built-in DB via [`Signatures`] (scanned
//! first). The greedy parser attaches definitions like any other command, so they
//! surface as plain `COMMAND` descendants — those inside a comment or a verbatim
//! body never parse to a `COMMAND`, so they are skipped for free.
//!
//! [`Signatures`]: super::signature::Signatures
//!
//! ## Supported forms and the one gap
//!
//! Only the **braced** name form is extracted: `\newcommand{\foo}…`,
//! `\NewDocumentCommand{\foo}…`. The unbraced form `\newcommand\foo…` parses
//! awkwardly under greedy attachment — `\foo` becomes a *sibling* `COMMAND` and the
//! replacement group attaches to it, not to `\newcommand` — so `\newcommand` has no
//! name group and the scan skips it (no signature, never a crash). Recovering the
//! unbraced form needs sibling heuristics here (not parser changes — the parser
//! stays meaning-free, decision #2); it is a recorded follow-up.

use crate::ast::{command_name, group_command_name, group_inner_source, nth_group, nth_group_text};
use crate::semantic::signature::{ArgKind, ArgSpec, CommandSig, EnvironmentSig, SignatureDb};
use crate::semantic::xparse;
use crate::syntax::{SyntaxKind, SyntaxNode};

/// Scan `root` for user command/environment definitions and return their extracted
/// signatures. Names already defined earlier in the document are overwritten, so a
/// later `\renewcommand` wins — TeX's last-definition-wins, modulo execution order
/// we do not track.
pub fn scan_definitions(root: &SyntaxNode) -> SignatureDb {
    let mut db = SignatureDb::default();

    for command in root
        .descendants()
        .filter(|node| node.kind() == SyntaxKind::COMMAND)
    {
        let Some(name) = command_name(&command) else {
            continue;
        };
        match DefKind::of(&name) {
            Some(DefKind::Command) => scan_newcommand(&command, &mut db),
            Some(DefKind::Environment) => scan_newenvironment(&command, &mut db),
            Some(DefKind::XparseCommand) => scan_xparse_command(&command, &mut db),
            Some(DefKind::XparseEnvironment) => scan_xparse_environment(&command, &mut db),
            None => {}
        }
    }

    db
}

/// Which definition family a control word names, if any.
enum DefKind {
    Command,
    Environment,
    XparseCommand,
    XparseEnvironment,
}

impl DefKind {
    fn of(name: &str) -> Option<Self> {
        Some(match name {
            "newcommand" | "renewcommand" | "providecommand" | "DeclareRobustCommand" => {
                DefKind::Command
            }
            "newenvironment" | "renewenvironment" => DefKind::Environment,
            "NewDocumentCommand"
            | "RenewDocumentCommand"
            | "ProvideDocumentCommand"
            | "DeclareDocumentCommand" => DefKind::XparseCommand,
            "NewDocumentEnvironment"
            | "RenewDocumentEnvironment"
            | "ProvideDocumentEnvironment"
            | "DeclareDocumentEnvironment" => DefKind::XparseEnvironment,
            _ => return None,
        })
    }
}

/// `\newcommand{\name}[n][default]{body}` → a [`CommandSig`]. The name is the
/// control word in the first group; `[n]` (if present) is the arg count, and a
/// second optional `[default]` makes the first argument optional `[…]` while the
/// rest are mandatory `{…}` — LaTeX2e's `\newcommand` shape.
fn scan_newcommand(command: &SyntaxNode, db: &mut SignatureDb) {
    let Some(name) = nth_group(command, 0).as_ref().and_then(group_command_name) else {
        return;
    };
    let (arity, first_optional) = newcommand_arity(command);
    db.insert_command(
        name,
        CommandSig {
            args: latex2e_args(arity, first_optional),
            sectioning: None,
            verbatim: false,
            rule: false,
            inline: false,
        },
    );
}

/// `\newenvironment{name}[n][default]{begin}{end}` → an [`EnvironmentSig`]. Same
/// arg-count shape as [`scan_newcommand`]; the body is assumed reflowable prose (a
/// user verbatim-like environment would be declared with package-specific macros we
/// do not scan).
fn scan_newenvironment(command: &SyntaxNode, db: &mut SignatureDb) {
    let Some(name) = nth_group_text(command, 0) else {
        return;
    };
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    let (arity, first_optional) = newcommand_arity(command);
    db.insert_environment(name, environment_sig(latex2e_args(arity, first_optional)));
}

/// `\NewDocumentCommand{\name}{spec}{body}` → a [`CommandSig`] with args from the
/// xparse spec.
fn scan_xparse_command(command: &SyntaxNode, db: &mut SignatureDb) {
    let Some(name) = nth_group(command, 0).as_ref().and_then(group_command_name) else {
        return;
    };
    let Some(spec) = nth_group(command, 1) else {
        return;
    };
    db.insert_command(
        name,
        CommandSig {
            args: xparse::parse_spec(&group_inner_source(&spec)),
            sectioning: None,
            verbatim: false,
            rule: false,
            inline: false,
        },
    );
}

/// `\NewDocumentEnvironment{name}{spec}{begin}{end}` → an [`EnvironmentSig`] with
/// args from the xparse spec.
fn scan_xparse_environment(command: &SyntaxNode, db: &mut SignatureDb) {
    let Some(name) = nth_group_text(command, 0) else {
        return;
    };
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    let Some(spec) = nth_group(command, 1) else {
        return;
    };
    db.insert_environment(
        name,
        environment_sig(xparse::parse_spec(&group_inner_source(&spec))),
    );
}

/// The `(arity, first_arg_optional)` pair for a LaTeX2e definition: the integer in
/// the first `[…]` optional (default `0`), and whether a *second* optional is
/// present (which makes the first argument optional).
fn newcommand_arity(command: &SyntaxNode) -> (usize, bool) {
    let optionals: Vec<SyntaxNode> = command
        .children()
        .filter(|child| child.kind() == SyntaxKind::OPTIONAL)
        .collect();
    let arity = optionals
        .first()
        .and_then(optional_number)
        .unwrap_or(0)
        .min(9); // LaTeX caps macro arity at 9.
    (arity, optionals.len() >= 2)
}

/// The integer inside an `OPTIONAL` node (`[2]` → `2`), or `None` if it isn't a
/// bare number.
fn optional_number(node: &SyntaxNode) -> Option<usize> {
    let text = node.text().to_string();
    let inner = text.strip_prefix('[').unwrap_or(&text);
    let inner = inner.strip_suffix(']').unwrap_or(inner);
    inner.trim().parse().ok()
}

/// Build the LaTeX2e argument slots: `arity` arguments, all mandatory `{…}` unless
/// `first_optional`, in which case the first is optional `[…]`.
fn latex2e_args(arity: usize, first_optional: bool) -> Vec<ArgSpec> {
    (0..arity)
        .map(|i| {
            if i == 0 && first_optional {
                ArgSpec {
                    required: false,
                    kind: ArgKind::Bracket,
                    prose: false,
                }
            } else {
                ArgSpec {
                    required: true,
                    kind: ArgKind::Brace,
                    prose: false,
                }
            }
        })
        .collect()
}

/// An [`EnvironmentSig`] for a scanned environment with the given args: a
/// reflowable, non-math, non-verbatim body (the only shape LaTeX2e/xparse
/// definitions give us without package-specific knowledge).
fn environment_sig(args: Vec<ArgSpec>) -> EnvironmentSig {
    EnvironmentSig {
        args,
        verbatim_body: false,
        math: false,
        align: false,
        reflow: true,
        no_indent: false,
        // A user `\newenvironment` is not assumed to be a list; the built-in DB
        // is the source of truth for `\item`-bearing list layout.
        list: false,
        // Block-ness of a user-defined environment is unknown without
        // package-specific knowledge; default to non-block (the parser keeps the
        // conservative `PARAGRAPH` wrapper for it).
        block: false,
        // A scanned user environment carries no outline category; only the curated
        // built-in floats/theorem-likes show up in the document-symbol outline.
        outline: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{parse, reconstruct};

    fn db_of(src: &str) -> SignatureDb {
        // New parser-adjacent feature: assert losslessness on every input.
        assert_eq!(reconstruct(src), src, "reconstruct must round-trip");
        scan_definitions(&SyntaxNode::new_root(parse(src).green))
    }

    fn arg_kinds(args: &[ArgSpec]) -> Vec<ArgKind> {
        args.iter().map(|a| a.kind).collect()
    }

    #[test]
    fn newcommand_counts_mandatory_args() {
        let db = db_of("\\newcommand{\\foo}[2]{#1#2}\n");
        let sig = db.command("foo").expect("foo defined");
        assert_eq!(arg_kinds(&sig.args), vec![ArgKind::Brace, ArgKind::Brace]);
        assert!(sig.args.iter().all(|a| a.required));
    }

    #[test]
    fn newcommand_optional_first_arg() {
        let db = db_of("\\newcommand{\\foo}[2][d]{#1#2}\n");
        let sig = db.command("foo").expect("foo defined");
        assert_eq!(arg_kinds(&sig.args), vec![ArgKind::Bracket, ArgKind::Brace]);
        assert!(!sig.args[0].required);
        assert!(sig.args[1].required);
    }

    #[test]
    fn newcommand_zero_args() {
        let db = db_of("\\newcommand{\\foo}{bar}\n");
        assert!(db.command("foo").expect("foo defined").args.is_empty());
    }

    #[test]
    fn renew_and_provide_recognized() {
        let db = db_of("\\renewcommand{\\a}[1]{x}\\providecommand{\\b}[1]{y}\n");
        assert_eq!(db.command("a").unwrap().args.len(), 1);
        assert_eq!(db.command("b").unwrap().args.len(), 1);
    }

    #[test]
    fn newenvironment_args() {
        let db = db_of("\\newenvironment{thm}[1]{begin #1}{end}\n");
        let sig = db.environment("thm").expect("thm defined");
        assert_eq!(arg_kinds(&sig.args), vec![ArgKind::Brace]);
        assert!(sig.reflow);
        assert!(!sig.verbatim_body);
        assert!(!sig.math);
    }

    #[test]
    fn xparse_command_spec() {
        let db = db_of("\\NewDocumentCommand{\\foo}{m O{d} m}{x}\n");
        let sig = db.command("foo").expect("foo defined");
        assert_eq!(
            arg_kinds(&sig.args),
            vec![ArgKind::Brace, ArgKind::Bracket, ArgKind::Brace]
        );
    }

    #[test]
    fn xparse_environment_spec() {
        let db = db_of("\\NewDocumentEnvironment{env}{O{x} m}{a}{b}\n");
        let sig = db.environment("env").expect("env defined");
        assert_eq!(arg_kinds(&sig.args), vec![ArgKind::Bracket, ArgKind::Brace]);
    }

    #[test]
    fn unbraced_name_form_skipped() {
        // Known limitation: `\newcommand\foo{x}` parses with `\foo` as a sibling, so
        // `\newcommand` has no name group and nothing is extracted.
        let db = db_of("\\newcommand\\foo{x}\n");
        assert!(db.command("foo").is_none());
    }

    #[test]
    fn redefinition_last_wins() {
        let db = db_of("\\newcommand{\\foo}[1]{x}\\renewcommand{\\foo}[3]{y}\n");
        assert_eq!(db.command("foo").unwrap().args.len(), 3);
    }

    #[test]
    fn garbage_definition_degrades_to_no_insert() {
        // No name group at all: nothing inserted, no panic.
        let db = db_of("\\newcommand\n");
        assert!(db.command("foo").is_none());
    }

    #[test]
    fn nested_definition_collected() {
        let db = db_of("\\begin{document}\n\\newcommand{\\foo}[1]{x}\n\\end{document}\n");
        assert_eq!(db.command("foo").unwrap().args.len(), 1);
    }

    #[test]
    fn commented_definition_ignored() {
        let db = db_of("% \\newcommand{\\foo}[1]{x}\n");
        assert!(db.command("foo").is_none());
    }
}
