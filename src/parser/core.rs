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
