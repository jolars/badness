//! The parser entry point and its output type.
//!
//! `parse` runs the pipeline: [`lex`] → [`grammar::parse`] (the recursive
//! descent, which emits events + errors) → [`build_tree`] (the green tree).
//! Syntax errors ride a side channel and never abort the parse.

use rowan::GreenNode;
use smol_str::SmolStr;

use crate::parser::grammar;
use crate::parser::lexer::{LatexFlavor, LexConfig, VerbCtx, lex_with};
use crate::parser::tree_builder::build_tree;
use crate::semantic::define::scan_definitions;
use crate::syntax::SyntaxNode;

/// A parsed document: the green tree plus any syntax errors gathered alongside
/// it. Errors never abort the parse (see `AGENTS.md`, Core decision #5).
#[derive(Debug, Clone)]
pub struct Parse {
    pub green: GreenNode,
    pub errors: Vec<SyntaxError>,
}

impl Parse {
    /// Materialize a fresh red-tree cursor over the parsed document. Cheap (an
    /// atomic clone of the green node).
    pub fn syntax(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }
}

/// A syntax error, carried on a side channel keyed by byte range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    pub message: String,
    pub start: usize,
    pub end: usize,
}

/// Parse LaTeX source into a lossless CST.
///
/// A bounded **two-pass** parse handles user-defined verbatim-argument commands
/// (`\newcommand`/xparse definitions that other a special char's catcode — see
/// [`crate::semantic::define`]): the lexer needs to know such commands *before* it
/// tokenizes their call sites, but they are only discoverable from the parsed tree.
/// So pass 1 parses with built-in verbatim knowledge only, scans the result for
/// catcode-verbatim definitions, and — *only* when it finds any — pass 2 re-parses
/// with those commands fed into the lexer so their arguments become opaque `VERB`
/// tokens. Two passes is the deliberate, conservative bound: a definition visible
/// only after the second pass's re-tokenization is a tolerated false negative. The
/// common case (no such definition) is a single parse (AGENTS.md decisions #1, #6).
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
    let config = config.into();
    let pass1 = parse_with(input, &VerbCtx::default(), config);
    let ctx = verbatim_ctx(&pass1.syntax());
    if ctx.is_empty() {
        return pass1;
    }
    parse_with(input, &ctx, config)
}

/// Run the lex → grammar → tree-build pipeline once with a fixed verbatim context.
fn parse_with(input: &str, ctx: &VerbCtx, config: LexConfig) -> Parse {
    let tokens = lex_with(input, ctx, config);
    let (events, errors) = grammar::parse(&tokens, ctx);
    let green = build_tree(&tokens, &events);
    Parse { green, errors }
}

/// Scan `root` for user definitions and collect the catcode-verbatim commands and
/// environments into a lexer [`VerbCtx`]. Each scanned signature's verbatim flag is
/// already resolved (`scan_definitions`); a command's `args` hold its leading,
/// non-verbatim arguments and an environment's `args` its (all leading) arguments —
/// the exact shapes the lexer needs.
fn verbatim_ctx(root: &SyntaxNode) -> VerbCtx {
    let db = scan_definitions(root);
    let mut ctx = VerbCtx::default();
    for name in db.command_names() {
        if let Some(sig) = db.command(name).filter(|sig| sig.verbatim) {
            ctx.insert(SmolStr::new(name), sig.args.to_vec());
        }
    }
    for name in db.environment_names() {
        if let Some(sig) = db.environment(name).filter(|sig| sig.verbatim_body) {
            ctx.insert_environment(SmolStr::new(name), sig.args.to_vec());
        }
    }
    ctx
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
}
