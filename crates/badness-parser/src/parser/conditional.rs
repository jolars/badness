//! Static recognition of TeX conditional control words — which `\if…`-named
//! command *opens* a `\fi`-terminated conditional, and which `\else`/`\or`/`\fi`
//! divides or closes one.
//!
//! Shared by the grammar (which builds [`SyntaxKind::CONDITIONAL`] nodes behind
//! a shape gate) and the linter's `ConditionalIndex` (which derives branch paths
//! for `duplicate-label`/`duplicate-package`), so the two read the *same* name
//! sets and the *same* state machine — the same arrangement as
//! [`super::lexer::expl_toggle`].
//!
//! What that buys is precise, and worth stating precisely: the two can never
//! disagree about **what an opener is**. They can still reach different verdicts
//! on a given token, because each feeds this scan a different stream on purpose.
//! The parser walks every token and suppresses openers inside an expl3 region
//! (in-region layout is the formatter's); the linter walks `COMMAND` nodes and
//! skips definition-command spans wholesale (`\def\stopit{\fi}` carries a `\fi`,
//! it does not run one), which the parser has no need to do because its own brace
//! anchor already refuses to pair across the body's group. Those are deliberate
//! per-consumer filters layered *around* a shared recognizer, not two recognizers.
//!
//! Recognition is **pair-and-trust**: a lowercase-`if`-prefixed name opens a
//! conditional unless it is a known brace-argument macro. That leaves two
//! families to subtract, both measured over latex2e/latex3/pgf/latexindent:
//!
//! - [`NOT_FI_TERMINATED`] — `\ifthenelse`, `\iftoggle`, the etoolbox test
//!   family (102 occurrences). These take `{true}{false}` arguments and are never
//!   `\fi`-terminated. Subtracting them is load-bearing rather than cosmetic:
//!   shape alone does not merely *fail* on one, it *mis-pairs*. In
//!   `latexindent`'s `test-cases/ifelsefi/issue-250.tex` an `\ifnumgreater` nests
//!   inside a real `\ifluatex`, so trusting it would steal the enclosing
//!   conditional's `\fi`.
//! - [`OPERAND_SKIPS`] — `\newif\if@foo` (574 occurrences) and
//!   `\let\ifpdf\iftrue`, where the `\ifX` sits in an *operand* slot and is data
//!   being declared or aliased, not live control flow.
//!
//! Both are curated compiled-in facts, on the same footing as the verbatim and
//! math environment tables: static lexical knowledge, never the mutable signature
//! database (`AGENTS.md` decision #8). A missing entry costs a desynced stack in
//! the linter and a demoted (never a mis-built) node in the parser, so the set
//! extends freely.
//!
//! [`SyntaxKind::CONDITIONAL`]: crate::syntax::SyntaxKind::CONDITIONAL

/// `if*`-named control words that are **not** `\fi`-terminated conditionals:
/// ordinary macros taking brace arguments (`{true}{false}`), which must not open
/// a conditional. Curated — amsmath's `\iff` arrow, ifthen's `\ifthenelse`,
/// babel's `\iflanguage`, and etoolbox's test family.
const NOT_FI_TERMINATED: &[&str] = &[
    "iff",        // amsmath: the ⟺ arrow, not a conditional
    "ifthenelse", // ifthen/xifthen: {test}{then}{else}
    "iflanguage", // babel: {lang}{then}{else}
    "iftoggle",   // etoolbox toggles: {toggle}{then}{else}
    // etoolbox def/cs/str/bool/num/dim tests, all brace-argument shaped:
    "ifdef",
    "ifcsdef",
    "ifundef",
    "ifcsundef",
    "ifdefmacro",
    "ifcsmacro",
    "ifdefempty",
    "ifcsempty",
    "ifdefvoid",
    "ifcsvoid",
    "ifdefstring",
    "ifcsstring",
    "ifdefequal",
    "ifcsequal",
    "ifbool",
    "ifboolexpr",
    "ifboolexpe",
    "ifstrequal",
    "ifstrempty",
    "ifblank",
    "ifnumcomp",
    "ifnumequal",
    "ifnumgreater",
    "ifnumless",
    "ifnumodd",
    "ifdimcomp",
    "ifdimequal",
    "ifdimgreater",
    "ifdimless",
];

/// Commands whose next N *control words* are operands (tokens being tested or
/// aliased), not live control flow: `\if`/`\ifx`/`\ifcat` compare two tokens,
/// eTeX's `\ifdefined` tests one, `\newif\ifmyflag` declares one, and
/// `\let\ifpdf\iftrue` aliases two. `\ifcsname` is handled separately
/// (skip until `\endcsname`); `\ifincsname` takes no operand at all.
///
/// Counting *control words* rather than TeX tokens is the deliberate
/// approximation: `\if ab\ifsomething` compares the characters `a` and `b`, so
/// TeX's own count would put `\ifsomething` outside the operand slots. Reading
/// character tokens here would mean modelling `\if`'s expansion, which is exactly
/// the meaning the syntactic layer does not carry.
const OPERAND_SKIPS: &[(&str, u8)] = &[
    ("if", 2),
    ("ifx", 2),
    ("ifcat", 2),
    ("ifdefined", 1),
    ("newif", 1),
    ("let", 2),
];

/// The `\ifcsname` … `\endcsname` pair, whose body names a control sequence
/// character by character and so contains no live conditionals.
const CSNAME_OPENER: &str = "ifcsname";
const CSNAME_CLOSER: &str = "endcsname";

/// A control word that divides or closes a conditional.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowWord {
    /// `\else` — opens the alternative branch.
    Else,
    /// `\or` — opens the next `\ifcase` branch.
    Or,
    /// `\fi` — closes the conditional.
    Fi,
}

/// Classify a control word's *name* (no leading backslash) as a conditional
/// divider or closer. Stateless: `\else`/`\or`/`\fi` are never anything else.
pub fn flow_word(name: &str) -> Option<FlowWord> {
    match name {
        "else" => Some(FlowWord::Else),
        "or" => Some(FlowWord::Or),
        "fi" => Some(FlowWord::Fi),
        _ => None,
    }
}

/// Pair-and-trust: a lowercase-`if`-prefixed name opens a conditional unless it
/// is a known brace-argument macro. Reads the *name*, no leading backslash.
///
/// Positional context (an operand slot, an `\ifcsname` body) is **not** checked
/// here — that needs the running state [`OpenerScan`] carries.
pub fn is_conditional_opener(name: &str) -> bool {
    name.starts_with("if") && !NOT_FI_TERMINATED.contains(&name)
}

/// How many following control words `name` claims as operands, if any.
pub fn operand_skips(name: &str) -> Option<u8> {
    OPERAND_SKIPS
        .iter()
        .find(|(op, _)| *op == name)
        .map(|&(_, n)| n)
}

/// What a control word does to the conditional structure, once positional
/// context is taken into account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Word {
    /// Opens a conditional.
    Opens,
    /// Divides or closes the innermost open conditional.
    Flow(FlowWord),
    /// Neither — an ordinary command, or an `\ifX` sitting in an operand slot.
    Inert,
}

/// The running state that turns [`is_conditional_opener`] into a positional
/// decision: the operand-slot countdown and the `\ifcsname` body.
///
/// Feed the control words in document order through [`Self::visit`] — the parser
/// in a pre-pass over the token stream, the linter walking `COMMAND` nodes in
/// preorder.
///
/// The state is a running countdown, so *whether* a word is visited is itself a
/// decision, and the two consumers make it differently on purpose (module doc).
/// The rule is which question the filter answers. Tokens the document does not
/// **execute** must not be visited at all — the linter withholds the whole span of
/// a `\def` body, because a `\let` carried inside one must not arm the countdown
/// for the code after it. Tokens that merely have no *node* to build are visited
/// and then discarded from the result — the parser does this for expl3 regions,
/// so an in-region `\ifcsname` still opens and closes its skip window for the
/// words that follow.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenerScan {
    /// Remaining control words claimed as operands by an earlier command.
    pending_skips: u8,
    /// Inside an `\ifcsname` … `\endcsname` body.
    in_csname: bool,
}

impl OpenerScan {
    /// A fresh scan, at the start of a document.
    pub fn new() -> Self {
        Self::default()
    }

    /// Classify the next control word `name` (no leading backslash) and advance
    /// the state.
    pub fn visit(&mut self, name: &str) -> Word {
        let flow = flow_word(name);
        if self.in_csname {
            if name == CSNAME_CLOSER {
                self.in_csname = false;
                return Word::Inert;
            }
            if flow.is_none() {
                return Word::Inert;
            }
            // Malformed input (an `\ifcsname` never closed): a flow word
            // re-enables interpretation rather than going dark to EOF.
            self.in_csname = false;
        }
        if self.pending_skips > 0 {
            if flow.is_none() {
                self.pending_skips -= 1;
                return Word::Inert;
            }
            self.pending_skips = 0;
        }
        if let Some(flow) = flow {
            return Word::Flow(flow);
        }
        let opens = is_conditional_opener(name);
        if opens && name == CSNAME_OPENER {
            self.in_csname = true;
        } else if let Some(n) = operand_skips(name) {
            self.pending_skips = n;
        }
        if opens { Word::Opens } else { Word::Inert }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Classify each name in order, returning the verdicts.
    fn scan(names: &[&str]) -> Vec<Word> {
        let mut s = OpenerScan::new();
        names.iter().map(|n| s.visit(n)).collect()
    }

    #[test]
    fn a_plain_conditional_opens_and_closes() {
        assert_eq!(
            scan(&["ifnum", "else", "fi"]),
            [
                Word::Opens,
                Word::Flow(FlowWord::Else),
                Word::Flow(FlowWord::Fi)
            ]
        );
    }

    #[test]
    fn newif_declares_rather_than_opens() {
        // `\newif\if@foo`: the `\ifX` is the flag being declared, not an opener.
        assert_eq!(scan(&["newif", "if@foo"]), [Word::Inert, Word::Inert]);
    }

    #[test]
    fn ifx_operands_are_inert_even_when_if_named() {
        // `\ifx\ifpdf\iftrue`: both operands are tokens being compared.
        assert_eq!(
            scan(&["ifx", "ifpdf", "iftrue"]),
            [Word::Opens, Word::Inert, Word::Inert]
        );
    }

    #[test]
    fn let_aliases_two_tokens_without_opening() {
        // `\let\ifpdf\iftrue`.
        assert_eq!(
            scan(&["let", "ifpdf", "iftrue"]),
            [Word::Inert, Word::Inert, Word::Inert]
        );
    }

    #[test]
    fn brace_argument_tests_open_nothing() {
        assert_eq!(
            scan(&["ifthenelse", "ifnumgreater", "iftoggle", "iff"]),
            [Word::Inert; 4]
        );
    }

    #[test]
    fn a_flow_word_cancels_a_pending_operand_run() {
        // `\ifx\a\else`: the `\else` is real flow, not `\ifx`'s second operand,
        // and it must not leave the countdown armed for what follows.
        assert_eq!(
            scan(&["ifx", "ifone", "else", "ifnum"]),
            [
                Word::Opens,
                Word::Inert,
                Word::Flow(FlowWord::Else),
                Word::Opens
            ]
        );
    }

    #[test]
    fn csname_bodies_hold_no_conditionals() {
        assert_eq!(
            scan(&["ifcsname", "ifnum", "endcsname", "ifdim"]),
            [Word::Opens, Word::Inert, Word::Inert, Word::Opens]
        );
    }

    #[test]
    fn an_unclosed_csname_reopens_at_a_flow_word() {
        assert_eq!(
            scan(&["ifcsname", "ifnum", "fi", "ifdim"]),
            [
                Word::Opens,
                Word::Inert,
                Word::Flow(FlowWord::Fi),
                Word::Opens
            ]
        );
    }
}
