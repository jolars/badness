//! The parser entry point and its output type.
//!
//! `parse` runs the pipeline: [`lex`](crate::parser::lex) → [`grammar::parse`] (the recursive
//! descent, which emits events + errors) → [`build_tree`] (the green tree).
//! Syntax errors ride a side channel and never abort the parse.

use std::collections::HashMap;

use rowan::GreenNode;
use smol_str::SmolStr;

use crate::declarations::ResolvedDeclarations;
use crate::parser::grammar;
use crate::parser::lexer::{LatexFlavor, LexConfig, ParseCtx, lex_with, lex_with_implicit_expl};
use crate::parser::tree_builder::build_tree;
use crate::semantic::define::scan_definitions;
use crate::semantic::signature::builtin;
use crate::syntax::SyntaxNode;

pub use crate::SyntaxError;

/// A green tree and the syntax errors gathered while parsing it.
#[derive(Debug, Clone)]
pub struct Parse {
    pub green: GreenNode,
    pub errors: Vec<SyntaxError>,
}

impl Parse {
    /// Returns a red-tree cursor over the parsed document.
    pub fn syntax(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }
}

/// Parse LaTeX source into a lossless CST.
///
/// A bounded two-pass parse handles user-defined verbatim-argument commands
/// (`\newcommand`/xparse definitions that other a special char's catcode — see
/// [`crate::semantic::define`]): the lexer needs to know such commands *before* it
/// tokenizes their call sites, but they are only discoverable from the parsed tree.
/// So pass 1 parses with built-in verbatim knowledge only, scans the result for
/// catcode-verbatim definitions, and — *only* when it finds any — pass 2 re-parses
/// with those commands fed into the lexer so their arguments become opaque `VERB`
/// tokens. Two passes is a conservative bound: a definition visible
/// only after the second pass's re-tokenization is a tolerated false negative. The
/// common case is a single parse.
pub fn parse(input: &str) -> Parse {
    parse_with_flavor(input, LatexFlavor::Document)
}

/// Parse LaTeX source into a lossless CST under an explicit [`LexConfig`].
///
/// Identical to [`parse`] but fixes the lexer's initial catcode regime (a
/// [`Package`](LatexFlavor::Package) flavor — `.sty`/`.cls` — starts with `@` as a
/// letter) and whether to run the `.dtx` docstrip mode. A bare [`LatexFlavor`]
/// coerces in, so most callers pass one directly; [`parse`] is the
/// [`Document`](LatexFlavor::Document) wrapper.
pub fn parse_with_flavor(input: &str, config: impl Into<LexConfig>) -> Parse {
    parse_with_declarations(input, config, &ResolvedDeclarations::default())
}

/// Parse LaTeX source under an explicit [`LexConfig`] *and* a project's
/// [declarations](crate::declarations) — the one input to the tree that is not
/// the text (`AGENTS.md` decision #12).
///
/// The declarations are applied to **pass 1**, so a declaring project pays no
/// extra parse: they are known before a byte is lexed, unlike the file's own
/// definitions, which are what the two-pass scan exists to discover. The second
/// pass is then decided by asking whether the *scan* contributed anything the
/// declarations did not already say — not by asking whether the context is
/// empty, which a seeded context never is.
///
/// [`ResolvedDeclarations`] rather than a bare `SignatureDb`: this is the only
/// signature data the parser accepts, and a type that can only come from a
/// declaration block is what keeps a document's merged scope (package scans,
/// scanned definitions, the CWL tier) from reaching the tree.
pub fn parse_with_declarations(
    input: &str,
    config: impl Into<LexConfig>,
    declared: &ResolvedDeclarations,
) -> Parse {
    parse_with_declarations_resolved(input, config, declared).0
}

/// [`parse_with_declarations`], additionally handing back the [`ParseCtx`] the
/// returned tree was parsed under.
///
/// The context is not a second output so much as a *witness*: an incremental
/// reparse that relexes a fragment of this text must do so under the same context,
/// or the fragment's tokens are not the ones the tree holds — a `\newcommand` the
/// scan found makes its call sites lex differently, and a relex under a default
/// context would silently disagree. It is free to hand back, since both passes
/// compute it anyway (the one-pass case returns the seed it compared equal to).
///
/// The plain [`parse_with_declarations`] stays the entry point for everyone who
/// only wants a tree.
pub fn parse_with_declarations_resolved(
    input: &str,
    config: impl Into<LexConfig>,
    declared: &ResolvedDeclarations,
) -> (Parse, ParseCtx) {
    let config = config.into();
    let mut seed = ParseCtx::default();
    seed.overlay_declarations(declared);

    let pass1 = parse_with(input, &seed, config);
    let mut ctx = parse_ctx(&pass1.syntax());
    // Declared wins over scanned, so the overlay is applied *after* the scan.
    ctx.overlay_declarations(declared);
    if ctx == seed {
        return (pass1, ctx);
    }
    let pass2 = parse_with(input, &ctx, config);
    (pass2, ctx)
}

/// Run the lex → grammar → tree-build pipeline once with a fixed scan context.
fn parse_with(input: &str, ctx: &ParseCtx, config: LexConfig) -> Parse {
    let tokens = lex_with(input, ctx, config);
    let (events, errors) = grammar::parse(&tokens, ctx);
    let green = build_tree(&tokens, &events);
    Parse { green, errors }
}

/// Parse a root fragment under the exact file-level context of an existing parse.
///
/// Unlike [`parse_with_declarations_resolved`], this does not rescan definitions
/// from the fragment. Incremental tiers need the full file's context to remain the
/// authority, including the full file's one-shot `.dtx` implicit-expl signal.
pub(crate) fn parse_fragment_with_ctx(
    input: &str,
    ctx: &ParseCtx,
    config: LexConfig,
    implicit_expl: bool,
) -> Parse {
    let tokens = lex_with_implicit_expl(input, ctx, config, implicit_expl);
    let (events, errors) = grammar::parse(&tokens, ctx);
    let green = build_tree(&tokens, &events);
    Parse { green, errors }
}

/// Scan `root` for user definitions and collect the facts pass 2 needs into a
/// [`ParseCtx`]. Each scanned signature's verbatim flag is already resolved
/// (`scan_definitions`); a command's `args` hold its leading, non-verbatim
/// arguments and an environment's `args` its (all leading) arguments — the exact
/// shapes the lexer needs.
///
/// The inverse case also feeds the context: a command the file redefines *non-verbatim*
/// whose name collides with a built-in raw-argument command (`\code`, `\href`, …) is
/// recorded as *suppressed*, so the local definition shadows the built-in and pass 2
/// lexes `\code{…}` as an ordinary group (follow-up to issue #53).
///
/// Environment aliases (issue #109) are projected the same way, but only when the
/// alias is *called* somewhere: a `.sty` that defines `\bea`/`\eea` for its users
/// and never uses them must not pay a second parse for nothing. The occurrence
/// count is a sound one-sided filter — a definition always contributes at least one
/// `COMMAND` node, so a called alias always has two or more, and the worst a
/// `\renewcommand` can do is admit an unused alias (one wasted pass, never a missed
/// pairing).
fn parse_ctx(root: &SyntaxNode) -> ParseCtx {
    let db = scan_definitions(root);
    let mut ctx = ParseCtx::default();
    for name in db.command_names() {
        match db.command(name) {
            Some(sig) if sig.verbatim => ctx.insert(SmolStr::new(name), sig.args.to_vec()),
            // Redefined non-verbatim but shadowing a built-in raw-argument command:
            // suppress the built-in capture.
            Some(_)
                if builtin()
                    .command(name)
                    .is_some_and(|sig| sig.verbatim || sig.args.iter().any(|arg| arg.verbatim)) =>
            {
                ctx.suppress(SmolStr::new(name));
            }
            _ => {}
        }
    }
    for name in db.environment_names() {
        if let Some(sig) = db.environment(name).filter(|sig| sig.verbatim_body) {
            ctx.insert_environment(SmolStr::new(name), sig.args.to_vec());
        }
    }
    // Gated on there being any alias at all: this is an extra tree walk, and the
    // overwhelmingly common case is a file with no aliases, which must not pay for
    // the feature.
    if db.env_begin_aliases().next().is_some() || db.env_end_aliases().next().is_some() {
        let called = command_call_counts(root);
        let is_called = |name: &str| called.get(name).is_some_and(|n| *n >= 2);
        for (name, target) in db.env_begin_aliases() {
            if is_called(name) {
                ctx.insert_begin_alias(SmolStr::new(name), SmolStr::new(target));
            }
        }
        // No "its target must have a live opener" filter: since issue #117 the
        // literal `\begin{X}` is an opener spelling too, so a closer alias whose
        // partner is never defined still pairs. The `is_called` filter alone
        // carries what that one was for — keeping an alias no call site uses from
        // buying the file a second parse.
        for (name, target) in db.env_end_aliases() {
            if is_called(name) {
                ctx.insert_end_alias(SmolStr::new(name), SmolStr::new(target));
            }
        }
    }
    ctx
}

/// How many `COMMAND` nodes name each control word in `root`. Used only for the
/// alias "is it called anywhere" filter above, so it counts occurrences rather
/// than distinguishing definitions from calls — see [`parse_ctx`] for why that
/// one-sided approximation is sound.
fn command_call_counts(root: &SyntaxNode) -> HashMap<SmolStr, usize> {
    let mut counts: HashMap<SmolStr, usize> = HashMap::new();
    for node in root
        .descendants()
        .filter(|n| n.kind() == crate::syntax::SyntaxKind::COMMAND)
    {
        if let Some(name) = crate::ast::command_name(&node) {
            *counts.entry(SmolStr::new(name)).or_default() += 1;
        }
    }
    counts
}

/// Parse `input` and render the CST back to source. By the losslessness
/// invariant this always equals `input`.
pub fn reconstruct(input: &str) -> String {
    parse(input).syntax().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconstruct_is_identity() {
        let input = "\\section{Hi}\n\nbody $x^2$ % c\n";
        assert_eq!(reconstruct(input), input);
    }

    /// The resolved context is the one the *returned* tree was parsed under, which
    /// for a two-pass file is the scanned one, not the seed. An incremental reparse
    /// relexes fragments under it, so handing back the seed here would relex a
    /// `\shellcmd{…}` call site as an ordinary group and disagree with the tree.
    #[test]
    fn the_resolved_context_carries_the_second_pass_scan() {
        let input = "\\newcommand\\shellcmd[1]{\\@makeother\\$#1}\n\\shellcmd{a_$b$}\n";
        let (parse, ctx) =
            parse_with_declarations_resolved(input, LatexFlavor::Document, &Default::default());

        assert_ne!(
            ctx,
            ParseCtx::default(),
            "the scan found a verbatim definition, so the context must not be the seed"
        );
        // The witness property: relexing the whole input under the resolved context
        // reproduces the token run the tree holds. Under the seed it would not — the
        // call site would lex as an ordinary group rather than a `VERB`.
        let relexed: String = lex_with(input, &ctx, LatexFlavor::Document.into())
            .iter()
            .map(|t| t.text.as_str())
            .collect();
        assert_eq!(relexed, parse.syntax().to_string());
        assert!(
            parse.syntax().to_string().contains("a_$b$"),
            "the call-site argument should be captured whole"
        );
    }

    /// The one-pass case still hands back a usable witness rather than nothing: the
    /// seed and the scan compared equal, so either is the context the tree was
    /// parsed under.
    #[test]
    fn the_resolved_context_is_the_seed_when_no_scan_contributes() {
        let input = "\\section{Hi}\n\nplain prose\n";
        let (_, ctx) =
            parse_with_declarations_resolved(input, LatexFlavor::Document, &Default::default());
        assert_eq!(ctx, ParseCtx::default());
    }

    #[test]
    fn command_wraps_its_argument_group() {
        use crate::syntax::SyntaxKind;
        let parse = parse(r"\a{b}");
        let command = parse
            .syntax()
            .descendants()
            .find(|n| n.kind() == SyntaxKind::COMMAND)
            .expect("a COMMAND node");
        assert!(
            command.children().any(|n| n.kind() == SyntaxKind::GROUP),
            "the argument should be a nested GROUP node"
        );
        assert!(parse.errors.is_empty());
    }

    /// A named math environment (`equation`, flagged `math` in the built-in DB)
    /// parses its body in math mode: a `MATH` node whose scripts become `SCRIPTED`,
    /// exactly as `\[…\]`. Previously the body was a prose `PARAGRAPH` of loose
    /// tokens.
    #[test]
    fn math_environment_body_is_a_math_node() {
        use crate::syntax::SyntaxKind;
        let input = "\\begin{equation}\n  x_i^2 = y\n\\end{equation}\n";
        let root = parse(input).syntax();
        let math = root
            .descendants()
            .find(|n| n.kind() == SyntaxKind::MATH)
            .expect("the equation body is wrapped in a MATH node");
        assert!(
            math.descendants().any(|n| n.kind() == SyntaxKind::SCRIPTED),
            "scripts inside the math environment build SCRIPTED nodes"
        );
        assert!(
            !root
                .descendants()
                .any(|n| n.kind() == SyntaxKind::PARAGRAPH),
            "the body is math, not a prose PARAGRAPH"
        );
        assert_eq!(reconstruct(input), input);
    }

    /// An alignment math environment keeps its `&` columns and `\\` rows as
    /// `AMPERSAND` / `LINE_BREAK` inside the `MATH` node, so the formatter's grid
    /// builder still sees them.
    #[test]
    fn align_environment_keeps_grid_tokens_inside_math() {
        use crate::syntax::SyntaxKind;
        let input = "\\begin{align}\n  a &= b \\\\\n  c &= d\n\\end{align}\n";
        let root = parse(input).syntax();
        let math = root
            .descendants()
            .find(|n| n.kind() == SyntaxKind::MATH)
            .expect("the align body is wrapped in a MATH node");
        assert!(
            math.children_with_tokens()
                .any(|e| e.kind() == SyntaxKind::AMPERSAND),
            "top-level `&` stays a direct MATH child"
        );
        assert!(
            math.children().any(|n| n.kind() == SyntaxKind::LINE_BREAK),
            "top-level `\\\\` stays a LINE_BREAK child of MATH"
        );
        assert_eq!(reconstruct(input), input);
    }

    /// A non-math environment (`itemize`, not flagged `math`) is unchanged: its body
    /// stays a prose block with no `MATH` node.
    #[test]
    fn non_math_environment_body_is_unchanged() {
        use crate::syntax::SyntaxKind;
        let input = "\\begin{itemize}\n  \\item a\n\\end{itemize}\n";
        let root = parse(input).syntax();
        assert!(
            !root.descendants().any(|n| n.kind() == SyntaxKind::MATH),
            "a text environment never enters math mode"
        );
        assert_eq!(reconstruct(input), input);
    }

    /// An unclosed math environment recovers at EOF (the `MATH` body ends, the
    /// `ENVIRONMENT` closes) rather than looping or corrupting; losslessness holds.
    #[test]
    fn unclosed_math_environment_recovers() {
        use crate::syntax::SyntaxKind;
        let input = "\\begin{equation}\n  a = b\n";
        let parse = parse(input);
        assert!(
            parse
                .syntax()
                .descendants()
                .any(|n| n.kind() == SyntaxKind::MATH),
            "the body still parses as math"
        );
        assert!(!parse.errors.is_empty(), "an unclosed environment reports");
        assert_eq!(reconstruct(input), input);
    }
}
