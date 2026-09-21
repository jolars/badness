//! Shape gates that prove delimiters can pair before the grammar commits.
//!
//! Policies share one forward scan and memoize verdicts against explicit walk
//! state. The grammar queries these gates without depending on their policies
//! or cache internals. Linearity tests count token visits across all scans.

use super::trivia::BLANK_LINE_NEWLINES;
use super::{LEFT_CMD, Parser, RIGHT_CMD, peek_begin_name, peek_end_name};
use crate::parser::conditional;
use crate::syntax::SyntaxKind;
use smol_str::SmolStr;

#[derive(Clone, Copy, PartialEq, Eq)]
struct WalkKey {
    macrocode_end: Option<usize>,
    in_def_body: bool,
    in_group: bool,
    plain_braces: u32,
    enclosing_math_is_dollar: bool,
}

pub(super) struct GateBatch {
    key: WalkKey,
    verdicts: std::collections::HashMap<usize, Option<usize>>,
}

trait VerdictSink {
    fn insert(&mut self, opener: usize, verdict: Option<usize>);
}

impl VerdictSink for std::collections::HashMap<usize, Option<usize>> {
    fn insert(&mut self, opener: usize, verdict: Option<usize>) {
        std::collections::HashMap::insert(self, opener, verdict);
    }
}

struct SeedVerdict {
    seed: usize,
    verdict: Option<Option<usize>>,
}

impl VerdictSink for SeedVerdict {
    fn insert(&mut self, opener: usize, verdict: Option<usize>) {
        if opener == self.seed {
            self.verdict = Some(verdict);
        }
    }
}

#[derive(PartialEq, Eq)]
enum StrayBrace {
    RefutesInGroup,
    ClosesInGroup,
    RefutesAlways,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MathAnchor {
    None,
    Opening,
    Closing,
}

impl MathAnchor {
    fn anchors(self, text: &str) -> bool {
        match self {
            MathAnchor::None => false,
            MathAnchor::Opening => matches!(text, "\\[" | "\\("),
            MathAnchor::Closing => matches!(text, "\\]" | "\\)"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DollarAnchor {
    Content,
    Refutes,
    Transparent,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ParagraphAnchor {
    None,
    OwnLevel,
    AnyDepth,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EnvAnchor {
    Counts,
    Refutes,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Nesting {
    Counted,
    Interleaved,
}

trait GatePolicy {
    const PARAGRAPH_ANCHOR: ParagraphAnchor;

    const STRAY_BRACE: StrayBrace = StrayBrace::RefutesInGroup;

    const MATH_ANCHOR: MathAnchor = MathAnchor::Opening;

    const NESTING: Nesting = Nesting::Counted;

    const OPENER_IS_ENV_BEGIN: bool = false;

    const ENV_END_UNWINDS_OPENERS: bool = false;

    const ANCHORS_AT_ANY_DEPTH: bool = false;

    const ENV_ANCHOR: EnvAnchor = EnvAnchor::Counts;

    const ENV_ANCHOR_IN_MACRO_CODE: bool = false;

    const CLOSER_NEEDS_ENV_BALANCE: bool = true;

    const MACROCODE_FRAME_ANCHORS: bool = true;

    fn dollar_anchor(&self) -> DollarAnchor {
        if Self::MATH_ANCHOR == MathAnchor::None {
            DollarAnchor::Content
        } else {
            DollarAnchor::Refutes
        }
    }

    fn last_closer(&self, p: &Parser<'_>) -> Option<usize>;

    fn opens_at(&self, p: &Parser<'_>, i: usize) -> bool;

    fn closes_at(&self, p: &Parser<'_>, i: usize) -> bool;

    fn pairs(&self, p: &Parser<'_>, opener: usize, closer: usize) -> bool {
        let _ = (p, opener, closer);
        true
    }
}

struct ConditionalGate;

impl GatePolicy for ConditionalGate {
    const PARAGRAPH_ANCHOR: ParagraphAnchor = ParagraphAnchor::OwnLevel;

    fn last_closer(&self, p: &Parser<'_>) -> Option<usize> {
        p.last_fi
    }

    fn opens_at(&self, p: &Parser<'_>, i: usize) -> bool {
        p.conditional_openers.contains(&i)
    }

    fn closes_at(&self, p: &Parser<'_>, i: usize) -> bool {
        p.conditional_flow_at(i) == Some(conditional::FlowWord::Fi)
    }
}

struct AliasGate;

impl GatePolicy for AliasGate {
    const PARAGRAPH_ANCHOR: ParagraphAnchor = ParagraphAnchor::None;

    fn last_closer(&self, p: &Parser<'_>) -> Option<usize> {
        p.last_alias_closer
    }

    fn opens_at(&self, p: &Parser<'_>, i: usize) -> bool {
        p.alias_openers.contains_key(&i) && !p.in_macro_code(i)
    }

    fn closes_at(&self, p: &Parser<'_>, i: usize) -> bool {
        p.closer_target(i).is_some() && !p.in_macro_code(i)
    }

    fn pairs(&self, p: &Parser<'_>, opener: usize, closer: usize) -> bool {
        p.closer_target(closer) == p.alias_openers.get(&opener).map(SmolStr::as_str)
    }
}

struct EnvGate;

impl GatePolicy for EnvGate {
    const PARAGRAPH_ANCHOR: ParagraphAnchor = ParagraphAnchor::None;
    const STRAY_BRACE: StrayBrace = StrayBrace::ClosesInGroup;
    const MATH_ANCHOR: MathAnchor = MathAnchor::None;
    const OPENER_IS_ENV_BEGIN: bool = true;
    const ENV_END_UNWINDS_OPENERS: bool = true;

    fn last_closer(&self, p: &Parser<'_>) -> Option<usize> {
        p.last_r_brace
    }

    fn opens_at(&self, p: &Parser<'_>, i: usize) -> bool {
        p.env_begin_at(i) && !p.in_macro_code(i)
    }

    fn closes_at(&self, _p: &Parser<'_>, _i: usize) -> bool {
        false
    }
}

struct DelimMathGate {
    closer: &'static str,
}

impl GatePolicy for DelimMathGate {
    const PARAGRAPH_ANCHOR: ParagraphAnchor = ParagraphAnchor::OwnLevel;
    const STRAY_BRACE: StrayBrace = StrayBrace::RefutesAlways;
    const MATH_ANCHOR: MathAnchor = MathAnchor::None;
    const ANCHORS_AT_ANY_DEPTH: bool = true;
    const CLOSER_NEEDS_ENV_BALANCE: bool = false;
    const MACROCODE_FRAME_ANCHORS: bool = false;

    fn last_closer(&self, p: &Parser<'_>) -> Option<usize> {
        if self.closer == "\\]" {
            p.last_display_math_closer
        } else {
            p.last_inline_math_closer
        }
    }

    fn opens_at(&self, _p: &Parser<'_>, _i: usize) -> bool {
        false
    }

    fn closes_at(&self, p: &Parser<'_>, i: usize) -> bool {
        let t = &p.tokens[i];
        t.kind == SyntaxKind::CONTROL_SYMBOL && t.text.as_str() == self.closer
    }
}

struct DollarGate {
    display: bool,
}

impl GatePolicy for DollarGate {
    const PARAGRAPH_ANCHOR: ParagraphAnchor = ParagraphAnchor::OwnLevel;
    const STRAY_BRACE: StrayBrace = StrayBrace::RefutesAlways;
    const MATH_ANCHOR: MathAnchor = MathAnchor::None;
    const ANCHORS_AT_ANY_DEPTH: bool = true;
    const CLOSER_NEEDS_ENV_BALANCE: bool = false;
    const MACROCODE_FRAME_ANCHORS: bool = false;

    fn last_closer(&self, p: &Parser<'_>) -> Option<usize> {
        p.last_dollar
    }

    fn opens_at(&self, _p: &Parser<'_>, _i: usize) -> bool {
        false
    }

    fn closes_at(&self, p: &Parser<'_>, i: usize) -> bool {
        p.tokens[i].kind == SyntaxKind::DOLLAR
            && (!self.display || p.tokens.get(i + 1).map(|t| t.kind) == Some(SyntaxKind::DOLLAR))
    }
}

struct LeftRightGate;

impl GatePolicy for LeftRightGate {
    const PARAGRAPH_ANCHOR: ParagraphAnchor = ParagraphAnchor::OwnLevel;
    const STRAY_BRACE: StrayBrace = StrayBrace::RefutesAlways;
    const MATH_ANCHOR: MathAnchor = MathAnchor::Closing;
    const NESTING: Nesting = Nesting::Interleaved;
    const MACROCODE_FRAME_ANCHORS: bool = false;

    fn last_closer(&self, p: &Parser<'_>) -> Option<usize> {
        p.last_right
    }

    fn opens_at(&self, p: &Parser<'_>, i: usize) -> bool {
        let t = &p.tokens[i];
        t.kind == SyntaxKind::CONTROL_WORD && t.text.as_str() == LEFT_CMD
    }

    fn closes_at(&self, p: &Parser<'_>, i: usize) -> bool {
        let t = &p.tokens[i];
        t.kind == SyntaxKind::CONTROL_WORD && t.text.as_str() == RIGHT_CMD
    }
}

struct TextBracketGate;

impl GatePolicy for TextBracketGate {
    const PARAGRAPH_ANCHOR: ParagraphAnchor = ParagraphAnchor::AnyDepth;
    const STRAY_BRACE: StrayBrace = StrayBrace::RefutesAlways;
    const MATH_ANCHOR: MathAnchor = MathAnchor::None;
    const ANCHORS_AT_ANY_DEPTH: bool = true;
    const ENV_ANCHOR: EnvAnchor = EnvAnchor::Refutes;

    fn last_closer(&self, p: &Parser<'_>) -> Option<usize> {
        p.last_r_bracket
    }

    fn opens_at(&self, p: &Parser<'_>, i: usize) -> bool {
        p.bracket_abuts_command(i)
    }

    fn closes_at(&self, p: &Parser<'_>, i: usize) -> bool {
        p.tokens[i].kind == SyntaxKind::R_BRACKET
    }
}

/// The high-confidence paragraph-spanning text-bracket shape. It shares every
/// structural anchor with [`TextBracketGate`], but a blank line does not refute
/// the candidate. The caller supplies the remaining proof: both delimiter
/// junctions are tight, and the located `]` is followed by a mandatory group.
struct LongTextBracketGate;

impl GatePolicy for LongTextBracketGate {
    const PARAGRAPH_ANCHOR: ParagraphAnchor = ParagraphAnchor::None;
    const STRAY_BRACE: StrayBrace = StrayBrace::RefutesAlways;
    const MATH_ANCHOR: MathAnchor = MathAnchor::None;
    const ANCHORS_AT_ANY_DEPTH: bool = true;
    const ENV_ANCHOR: EnvAnchor = EnvAnchor::Refutes;

    fn last_closer(&self, p: &Parser<'_>) -> Option<usize> {
        p.last_r_bracket
    }

    fn opens_at(&self, p: &Parser<'_>, i: usize) -> bool {
        p.bracket_abuts_command(i)
    }

    fn closes_at(&self, p: &Parser<'_>, i: usize) -> bool {
        p.tokens[i].kind == SyntaxKind::R_BRACKET
    }
}

struct MathBracketGate {
    enclosing_is_dollar: bool,
}

impl GatePolicy for MathBracketGate {
    const PARAGRAPH_ANCHOR: ParagraphAnchor = ParagraphAnchor::AnyDepth;
    const STRAY_BRACE: StrayBrace = StrayBrace::RefutesAlways;
    const MATH_ANCHOR: MathAnchor = MathAnchor::Closing;
    const ANCHORS_AT_ANY_DEPTH: bool = true;
    const ENV_ANCHOR: EnvAnchor = EnvAnchor::Refutes;
    const ENV_ANCHOR_IN_MACRO_CODE: bool = true;
    fn dollar_anchor(&self) -> DollarAnchor {
        if self.enclosing_is_dollar {
            DollarAnchor::Refutes
        } else {
            DollarAnchor::Transparent
        }
    }

    fn last_closer(&self, p: &Parser<'_>) -> Option<usize> {
        p.last_r_bracket
    }

    fn opens_at(&self, p: &Parser<'_>, i: usize) -> bool {
        p.bracket_abuts_command(i)
    }

    fn closes_at(&self, p: &Parser<'_>, i: usize) -> bool {
        p.tokens[i].kind == SyntaxKind::R_BRACKET
    }
}

struct MacrocodeBracketGate;

impl GatePolicy for MacrocodeBracketGate {
    const PARAGRAPH_ANCHOR: ParagraphAnchor = ParagraphAnchor::AnyDepth;
    const STRAY_BRACE: StrayBrace = StrayBrace::RefutesAlways;
    const MATH_ANCHOR: MathAnchor = MathAnchor::None;
    const ANCHORS_AT_ANY_DEPTH: bool = true;
    const ENV_ANCHOR: EnvAnchor = EnvAnchor::Refutes;

    fn last_closer(&self, p: &Parser<'_>) -> Option<usize> {
        p.last_r_bracket
    }

    fn opens_at(&self, _p: &Parser<'_>, _i: usize) -> bool {
        false
    }

    fn closes_at(&self, p: &Parser<'_>, i: usize) -> bool {
        p.tokens[i].kind == SyntaxKind::R_BRACKET
    }
}

impl Parser<'_> {
    /// One tick per token a shape-gate scan visits, into [`Self::scan_work`].
    /// Compiled away outside `cfg(test)`: the linearity regression tests are
    /// the only reader, and [`Self::gate_batch`]'s loop is hot enough that an
    /// unconditional counter shows up in the parse benchmarks.
    #[cfg(test)]
    pub(super) fn tick_scan(&self) {
        self.scan_work.set(self.scan_work.get() + 1);
    }

    #[cfg(not(test))]
    pub(super) fn tick_scan(&self) {}

    /// True if the `[` at token index `open` is closed by a `]` before the
    /// current macrocode chunk's frame terminator. Depth-tracks only the braces
    /// that really form groups (chunk-matched ones — [`Self::plain_braces`] are
    /// plain tokens), and gives up at a *blank line* — the same paragraph-break
    /// bail as [`Self::optional`], so an optional the formatter has re-wrapped
    /// over several lines still attaches on the second pass. Keeps a code
    /// bracket (`\@tempcnta[` with no `]` in the chunk) an ordinary token
    /// instead of an optional that would swallow the frame.
    ///
    /// Runs on the shared batch driver as [`MacrocodeBracketGate`] (`TODO.md`,
    /// container stack C2.5), which carries the chunk frame as its bound and
    /// adds the C0 last-`]` bound the hand-written scan never had. It is the one
    /// bracket gate the batch cannot make linear: single-entry by policy, so a
    /// chunk of `\cmd[` atoms whose only `]` sits outside it still scans to the
    /// frame per opener.
    pub(super) fn bracket_closes_before_macrocode_end(&self, open: usize) -> bool {
        // Total in `open` for a caller outside a chunk, where the frame that
        // bounds this gate does not exist and every `[` passes.
        if self.macrocode_end.is_none() {
            return true;
        }
        self.gate_verdict(open, &MacrocodeBracketGate).is_some()
    }

    /// True if the `[` at token index `open` is closed by a `]` before a token
    /// that would end the enclosing math. Mirrors [`Self::optional`]'s bail
    /// anchors (an unbalanced `}`, `\begin`/`\end`, a paragraph break, EOF) and
    /// adds the delimited math closers (`\]`, `\)`), which `optional` cannot
    /// stop at in text mode (`\item[$x$]` is legit) but which inside math mean
    /// the `[` is not an argument at all — e.g. the open-interval notation
    /// `$]0;\num{0.5}[$`. A `]` counts only outside `{…}` nesting, matching how
    /// `optional` consumes whole groups via `element` — and only past the `]`s
    /// owed to intervening *command-abutting* `[`s: such a `[` is itself
    /// argument-shaped (or a `\left`/`\Big` delimiter) and will claim the next
    /// `]` when parsed, so that `]` cannot also satisfy the outer `[`
    /// (`\P[\gamma[0, \infty) \cap A = \emptyset]`, issue #55 — the lone `]`
    /// belongs to `\gamma[`, so `\P[` stays an ordinary atom). A `[` abutting
    /// anything else (`x[i]`, the interval `[0, \infty)`) parses as an ordinary
    /// atom and claims nothing, so it adds no nesting here either.
    ///
    /// How a `$` at brace depth 0 is read depends on the *innermost enclosing
    /// math's flavor* ([`Self::math_dollar`]):
    /// - **Enclosing `\[…\]`/`\(…\)` (or a math environment).** A `$` opens a
    ///   genuine nested inline region, so a balanced `$…$` pair inside the
    ///   bracket is *transparent*: the `$` toggles an inline region rather than
    ///   ending the search, and `]`/`[` inside it are math content, ignored
    ///   (`\[ \inferrule*[right=$\Pi$-eq]{A}{B} \]` — the `$\Pi$` label sits
    ///   inside the optional). An *unbalanced* `$` leaves the region open, no
    ///   `]` is ever accepted, and the scan falls through to `false`.
    /// - **Enclosing `$…$`/`$$…$$`.** TeX cannot nest a `$` inside dollar math,
    ///   so the first depth-0 `$` is this math's *closer*: a `]` beyond it lives
    ///   in a later math and cannot be this bracket's, so bail like `\]`/`\)`.
    ///   Without this a stray `[` in dollar math (`$\mathcal{N}[\mathcal{S}$`,
    ///   a missing `]`, stacks-project issue #99) would scan past the closing
    ///   `$` into following math and wrongly attach an optional that swallows
    ///   it. Does not consume.
    ///
    /// Runs on the shared batch driver as [`MathBracketGate`] (`TODO.md`,
    /// container stack C2.5), where the transparent `$…$` region, the flavor
    /// that decides it, and the gate's two preserved strictnesses (a
    /// `\begin`/`\end` anchors inside macro code too, and a chunk-unmatched
    /// brace is group structure) are named policies. The enclosing flavor is
    /// walk state, so it rides the batch's memo key ([`WalkKey`]).
    pub(super) fn bracket_closes_before_math_end(&self, open: usize) -> bool {
        let gate = MathBracketGate {
            enclosing_is_dollar: self.enclosing_math_is_dollar(),
        };
        self.gated_closer(open, &gate, &self.math_bracket_batch)
            .is_some()
    }

    /// True if the `[` at token index `open` is closed by a `]` before a token
    /// that would make [`Self::optional`] bail in text mode. `[`/`]` are not
    /// real grouping in TeX, and macro code tests for and re-emits lone
    /// brackets (`\@ifnextchar [\@xmpar\@ympar`, `\def\@xfloat#1[#2]{…}`
    /// re-implementations — issue #60) at least as often as prose writes real
    /// optionals, so — like the `$` shape gate ([`Self::dollar_closes`]) — a
    /// bracket attaches only when it *reads* as an argument: its closer must be
    /// reachable. Mirrors `optional`'s bail anchors (an unbalanced `}`,
    /// `\begin`/`\end` outside a definition body, a paragraph break, EOF). A
    /// `]` counts only outside `{…}` nesting (matching how `optional` consumes
    /// whole groups via `element`) and only past the `]`s owed to intervening
    /// *command-abutting* `[`s, exactly as in
    /// [`Self::bracket_closes_before_math_end`] (issue #55). A gated bracket
    /// stays an ordinary token with **no diagnostic**: in code the shape is
    /// routine, so it is not statically an error. Does not consume.
    ///
    /// Runs on the shared batch driver as [`TextBracketGate`] (`TODO.md`,
    /// container stack C2.5). The claim countdown above *is* the driver's
    /// nested-opener stack — closer matching is LIFO either way — so one scan
    /// now settles every command-abutting `[` in the seed's own brace frame,
    /// where a refused bracket used to leave the walk to ask the next one from
    /// scratch. The C0 bound (the last `]` in the file) rides
    /// [`GatePolicy::last_closer`].
    pub(super) fn bracket_closes_in_text(&self, open: usize) -> bool {
        self.gated_closer(open, &TextBracketGate, &self.text_bracket_batch)
            .is_some()
    }

    /// Locate the closer for a paragraph-spanning text optional. This is only a
    /// structural half-proof: [`Self::attach_arguments`] also requires tight
    /// opener and mandatory-suffix junctions before admitting the node.
    pub(super) fn long_bracket_closer_in_text(&self, open: usize) -> Option<usize> {
        self.gated_closer(open, &LongTextBracketGate, &self.long_text_bracket_batch)
    }

    /// True if the `$` (or `$$`) opener at token index `open` is closed by a
    /// matching delimiter before a token that would end the math. `$`/`$$` are
    /// data in macro code at least as often as they are math delimiters (a
    /// tabular preamble's `>{$}`, an expl3 token list's `{ $ }`, catcode
    /// comparisons in `\def` bodies), so — like `[…]` attachment (issue #43) —
    /// a dollar opens math only when it *reads* as math: a closer must be
    /// reachable. Mirrors [`Self::dollar_math`]'s recovery anchors (an
    /// unbalanced `}`, an `\end` not owed to an intervening `\begin`, a
    /// paragraph break, EOF, the macrocode chunk end). A closing `$` counts
    /// only outside `{…}` nesting — [`Self::math_group`] consumes a nested
    /// dollar as an ordinary atom, never as the closer — and for `$$` a lone
    /// `$` is skipped exactly as `dollar_math` skips it (malformed but
    /// consumed). Likewise a paragraph break blocks only at the math body's
    /// own level. Inside a definition body `\begin`/`\end` are plain commands
    /// (issue #45), so neither anchors nor nests there. Does not consume.
    ///
    /// Runs on the shared batch driver as [`DollarGate`] (`TODO.md`, container
    /// stack C2.3) — for the uniformity, not for speed: the gate is
    /// single-entry, so its "batch" is one verdict, and its residual adversarial
    /// shape (a `${` per line: depth ratchets upward, so the level-gated
    /// paragraph anchor never fires and no depth-0 `$` ever appears) is one only
    /// a precomputed map could reach.
    ///
    /// A display opener is two tokens and its scan starts past both, so the seed
    /// handed to the driver — which scans from `seed + 1` — is the *second* `$`.
    pub(super) fn dollar_closes(&self, open: usize, display: bool) -> bool {
        let seed = if display { open + 1 } else { open };
        self.gate_verdict(seed, &DollarGate { display }).is_some()
    }

    /// The delimited-math twin of [`Self::dollar_closes`]: `\[`/`\(` opens
    /// math only when its `\]`/`\)` is reachable. Macro code passes the
    /// delimiters around as data tokens — stacks-project feeds `\[` to a
    /// splitter (`\expandafter\@tempa\[\@nil`, issue #65) — so an opener with
    /// no reachable closer is an ordinary token, no math, **no diagnostic**
    /// (the shape is routine in code, so it is not statically an error; a
    /// likely-typo unclosed `\[` in prose is linter territory, exactly as for
    /// `$`). Same blockers as `dollar_closes`, mirroring
    /// [`Self::delim_math`]'s recovery anchors: an unbalanced `}`, an `\end`
    /// not owed to an intervening `\begin`, a paragraph break, the macrocode
    /// chunk end, EOF. The closer counts only outside `{…}` nesting, and a
    /// paragraph break blocks only at the math body's own level.
    ///
    /// Runs on the shared batch driver as [`DelimMathGate`] (`TODO.md`,
    /// container stack C2.3), which carries the C0 bound — the last `\]`/`\)` in
    /// the file — as [`GatePolicy::last_closer`]. The gate is single-entry: a
    /// `\[` whose closer is reachable swallows every opener up to it, so there
    /// is never a same-frame neighbor left to settle, and it was measured linear
    /// before the migration. It joins the driver for the one copy of the
    /// bookkeeping, not for speed.
    pub(super) fn delim_math_closes(&self, open: usize, closer: &'static str) -> bool {
        self.gate_verdict(open, &DelimMathGate { closer }).is_some()
    }

    /// The `\left…\right` twin of [`Self::delim_math_closes`]: whether the
    /// `\left` at token index `open` has a matching `\right` reachable before a
    /// token that would end its body. `\left`/`\right` pair by *count* (nested
    /// pairs recurse in [`Self::left_right`]), so — unlike `$`/`\[` which are
    /// often data in code — an unclosed `\left` is genuinely malformed math, but
    /// it is still a *likely-typo* the linter should flag, never a parser error
    /// that blocks the whole file for the formatter (issue #77's
    /// `\left(1 …) …\left(…\right)` and `\left\bra …` with no `\right`). So it
    /// gets the same shape gate as `\[`: a `\left` whose `\right` is unreachable
    /// stays an ordinary command, **no diagnostic**. Mirrors [`Self::left_right`]'s
    /// recovery anchors — an unbalanced `}`, a closing `$`/`\]`/`\)`, an `\end`
    /// not owed to an intervening `\begin`, a paragraph break, EOF — with `\right`
    /// and the anchors counting only at the `\left`'s own brace/env/pair level.
    /// Does not consume.
    ///
    /// Runs on the shared batch driver as [`LeftRightGate`] (`TODO.md`,
    /// container stack C2.4), which is where those anchors and the deliberate
    /// `in_macro_code` blind spot now live as policy.
    pub(super) fn left_right_closes(&self, open: usize) -> bool {
        self.gated_closer(open, &LeftRightGate, &self.left_right_batch)
            .is_some()
    }

    /// The environment twin of [`Self::delim_math_closes`]: whether the
    /// `\begin` at `open` is cut short by the closing brace of a group it sits
    /// *inside*, with no `\end` of its own reachable first.
    ///
    /// Brace groups are catcode-level structure while `\begin`/`\end` are only
    /// macros, so a `}` closing a group opened before the `\begin` always wins —
    /// the environment cannot span it. Package code leans on this constantly:
    /// the two halves sit in sibling groups
    /// (`\newcolumntype{w}[2]{>{\begin{lrbox}…}c<{\end{lrbox}…}}`, array.sty),
    /// in sibling macros (`\newcommand\BeginExample{…\begin{VerbatimOut}…}`
    /// paired with `\EndExample`, rotex.tex), or the `\begin` is prose in a
    /// message argument that never runs as structure
    /// (`\PackageError{amstex}{\string\begin{split} is not allowed…}`,
    /// amstex.sty — all issue #71). In each the `\begin` is an ordinary token:
    /// it opens no `ENVIRONMENT` and draws **no diagnostic**, the same shape
    /// gate `\[` already gets from [`Self::delim_math_closes`]. Without it the
    /// environment swallows the `}` and cascades into unmatched-brace noise
    /// that fails the whole file for the formatter.
    ///
    /// Only the *group boundary* suppresses the environment. A `\begin` that
    /// merely runs out of file still opens one, so the unclosed-environment
    /// diagnostic keeps firing on a genuinely forgotten `\end`. A `\end` of
    /// another name terminates the scan too, leaving the existing mismatch
    /// recovery in [`Self::finish_environment`] untouched. Does not consume.
    pub(super) fn environment_escapes_group(&self, open: usize) -> bool {
        // Only a group the `\begin` is *actually* inside can cut it short. At
        // the outer level there is no such brace, and a later unbalanced `}`
        // is somebody else's business — notably a `.dtx` doc-line
        // `\begin{macro}`, whose intervening `macrocode` chunks split
        // definitions across braces on purpose ([`Self::plain_braces`], only
        // populated once that chunk is entered). Without this guard the scan
        // reads those as its own boundary and unnests the whole doc layer.
        if !self.in_group() {
            return false;
        }
        // `.dtx` doc-margin lines are exempt, exactly as they are from the
        // expl3 carve-out ([`Self::expl_toggles`]): `\begin{macro}` and friends
        // are the *documentation* layer and must keep pairing across the
        // macrocode chunks between them. Those bodies routinely span code that
        // leaves a brace open on purpose — a `\iffalse}\fi` editor-balance
        // hack, a `` \char`} `` constant, a catcode-swapped region — which
        // leaves a group open for the rest of the file and would
        // otherwise unnest the whole doc layer behind it. (A paragraph-break
        // bound cannot stand in here: a blank `.dtx` doc line is still a `%`
        // margin, so it never reads as a `\par`.)
        //
        // The exemption is about *stranded* braces, so it lifts when the
        // enclosing group opened on a doc-margin line too: that `{` is the
        // documentation layer's own, locally visible, and the `\begin` really is
        // inside it. `% \def\deflist#1{\begin{list}…}` paired with
        // `% \def\enddeflist{\end{list}}` (theorem.dtx, issue #71) is the split
        // environment definition the gate exists for, merely written as doc
        // prose.
        if self.doc_margin_exempt(open) {
            return false;
        }
        // Both checks above are per-opener walk state, so they stay outside the
        // batch: a `\begin` they reject never consults it, and the batch stores
        // only what the *scan* decided.
        //
        // The `{name}` group of the `\begin` itself nests and unnests inside the
        // scan, so it resumes at the environment's own level. The only escape is
        // a `}` at that level, so the last `}` in the file bounds the scan
        // ([`Self::last_r_brace`]) — sound, but rarely effective, since a
        // `\begin{…}` opener's own name group carries one and pushes the index
        // toward EOF. That is why this gate needed the batch
        // ([`EnvGate`], `TODO.md` container stack C2.2): the bound alone left it
        // quadratic in the number of openers.
        self.gated_closer(open, &EnvGate, &self.env_batch).is_some()
    }

    /// The conditional twin of [`Self::delim_math_closes`]: whether the live
    /// opener at token `open` ([`Self::conditional_openers`]) has its own `\fi`
    /// reachable before a token that would end it.
    ///
    /// `\if…\else…\or…\fi` is not a construct the surface syntax guarantees. A
    /// `\fi` is routinely assembled elsewhere — `\def\stopit{\fi}`,
    /// `\expandafter\fi`, an `\iffalse…\fi` used to comment a region out — so
    /// after subtracting the `\newif` and `\ifthenelse` families 268 of 6205
    /// corpus files still have unbalanced opener/`\fi` counts. An opener that
    /// does not pair is therefore ordinary macro code: it stays a plain
    /// `COMMAND` with **no diagnostic**, exactly as a gated `$`/`\[`/`\begin`
    /// does (`AGENTS.md` decision #1). Does not consume.
    ///
    /// The anchors mirror the math gates — an unbalanced `}`, an `\end` not owed
    /// to an intervening `\begin`, a paragraph break, the macrocode chunk end,
    /// EOF — with two deliberate differences from
    /// [`Self::environment_escapes_group`]:
    ///
    /// - **EOF does not pair.** The environment gate keeps a run-out-of-file
    ///   `\begin` so `finish_environment` can still report an unclosed
    ///   environment. A conditional has no diagnostic to preserve, and an
    ///   unpaired `\if` is routine, so running out of file just demotes.
    /// - **No `.dtx` doc-margin exemption.** That exemption exists so the
    ///   documentation layer keeps pairing `\begin{macro}` across the macrocode
    ///   chunks between them. A conditional has no such split-across-chunks
    ///   story, and bounding the scan at `macrocode_end` is precisely what makes
    ///   the `\iffalse}\fi` editor-balance hack demote instead of swallowing the
    ///   chunk.
    ///
    /// A paragraph break anchors at the construct's own level only, so the ~11%
    /// of corpus conditionals that span a blank line demote and keep their
    /// pre-node layout. That keeps
    /// `CONDITIONAL` a within-paragraph construct: it can never straddle a
    /// `PARAGRAPH` boundary, so no paragraph nests inside one.
    ///
    /// The closer must be reachable at the opener's **own level of every nesting
    /// the parse itself recognizes** — braces, environments, and math alike — not
    /// just braces. A token scan that counts a `\fi` the parse will consume inside
    /// some other construct promises a pairing the walk cannot honor, and
    /// [`Self::conditional`] then runs past it looking for a closer that is gone:
    /// `ltboxes.dtx`'s `\else\@pboxswtrue $\vcenter \fi\fi\fi … \if@pboxsw
    /// \m@th$\fi` puts all three `\fi`s inside a `$…$`, and the construct ran over
    /// 160 lines and every `macrocode` chunk in between. Hence the `envs == 0`
    /// requirement on the closer and the math anchor.
    ///
    /// The guarantee this buys is **one-directional, and that is the direction
    /// that matters**: the walk never runs *past* the index returned here (it is
    /// bounded by it outright). The walk may still stop *earlier*, because this
    /// scan counts nested openers by name while the walk re-gates each one and may
    /// demote it — and a demoted opener's `\fi` is then a closer the walk reaches
    /// first. `\ifA \begin{center} \ifB \end{center} \fi \fi` is the shape: the
    /// scan counts `\ifB` as nested and picks the second `\fi`, while the walk
    /// demotes `\ifB` (whose own scan meets an unowed `\end`) and closes at the
    /// first, leaving the second a plain `COMMAND`. Lossless, and the node is still
    /// well formed — but it is why [`crate::ast::Conditional::closer`] is fallible
    /// and why nothing downstream may assume the two indices agree
    /// (`conditional_walk_may_close_before_the_located_fi`, `tests/parser.rs`).
    ///
    /// **Cost.** Verdicts are computed in *batches* (`TODO.md`, container-stack
    /// C1): one forward scan seeded at the queried opener settles every
    /// same-frame opener it passes ([`Self::gate_batch`] under
    /// [`ConditionalGate`]), and the batch is memoized against the walk state
    /// it read
    /// ([`Self::conditional_batch`]) — so a run of top-level openers costs one
    /// O(n) pass where it used to cost one scan each. The scan stays bounded
    /// by the last `\fi`-flavored word in the file ([`Self::last_fi`], C0), so
    /// a file with none refuses without scanning at all. Openers the batch did
    /// not settle (they sat behind a brace at batch time) and queries under a
    /// changed walk state re-batch; every ordinary anchor still cuts a scan
    /// short, which is why real conditional-heavy packages (`biblatex.sty`,
    /// `latexrelease.sty`, `memoir.cls`) were within noise of the pre-node
    /// parser even before the batch.
    pub(super) fn conditional_closer(&self, open: usize) -> Option<usize> {
        self.gated_closer(open, &ConditionalGate, &self.conditional_batch)
    }

    /// The walk state a gate batch's scan reads — see [`WalkKey`].
    fn walk_key(&self) -> WalkKey {
        WalkKey {
            macrocode_end: self.macrocode_end,
            in_def_body: self.in_def_body,
            in_group: self.in_group(),
            plain_braces: self.plain_braces_version,
            enclosing_math_is_dollar: self.enclosing_math_is_dollar(),
        }
    }

    /// Whether the innermost enclosing math body is dollar-delimited
    /// ([`Self::math_dollar`]). Outside math the answer is unused; `false` is
    /// the reading a bracket gate would take there anyway.
    fn enclosing_math_is_dollar(&self) -> bool {
        self.math_dollar.last().copied().unwrap_or(false)
    }

    /// Whether the token at `i` is a `[` that **directly abuts** a command, and
    /// so claims the next `]` for itself when parsed — the bracket family's
    /// nested opener ([`TextBracketGate`]). The pre-batch scans derived this
    /// from a running `abuts_command` flag that every token kind but a control
    /// word or symbol cleared, trivia included, which is this test one token
    /// back.
    fn bracket_abuts_command(&self, i: usize) -> bool {
        self.tokens[i].kind == SyntaxKind::L_BRACKET
            && i > 0
            && matches!(
                self.tokens[i - 1].kind,
                SyntaxKind::CONTROL_WORD | SyntaxKind::CONTROL_SYMBOL
            )
    }

    /// The memoized front of [`Self::gate_batch`]: answer `open` from `memo`
    /// when the batch there was harvested under the current walk state and
    /// settled this opener, and otherwise re-batch from `open` and keep the
    /// result.
    ///
    /// One slot per gate is all the reuse there is *for verdicts*: the walk
    /// queries each opener once, in ascending order, under a state that is
    /// stable between re-batches. The slot's **storage** is reused further
    /// than that — a miss takes the stale map, clears it, and refills it, so a
    /// gate allocates about once per parse instead of once per re-batch. A
    /// cleared `HashMap` keeps its capacity, and the batches of one gate over
    /// one file are all much of a size.
    fn gated_closer<P: GatePolicy>(
        &self,
        open: usize,
        policy: &P,
        memo: &std::cell::RefCell<Option<GateBatch>>,
    ) -> Option<usize> {
        // The C0 bound as an early-out: a file with no closer of this gate's
        // shape refuses without scanning at all.
        policy.last_closer(self)?;
        let key = self.walk_key();
        if let Some(batch) = memo.borrow().as_ref()
            && batch.key == key
            && let Some(&verdict) = batch.verdicts.get(&open)
        {
            return verdict;
        }
        // Recycle the superseded batch's map: its verdicts are stale (the key
        // missed, or it did not settle this opener), but its allocation is not.
        let mut verdicts =
            memo.borrow_mut()
                .take()
                .map_or_else(std::collections::HashMap::new, |stale| {
                    let mut map = stale.verdicts;
                    map.clear();
                    map
                });
        self.gate_batch(open, policy, &mut verdicts);
        let verdict = verdicts.get(&open).copied();
        debug_assert!(verdict.is_some(), "the batch must settle its own seed");
        *memo.borrow_mut() = Some(GateBatch { key, verdicts });
        verdict.flatten()
    }

    /// The unmemoized front, for a **single-entry** gate ([`DelimMathGate`],
    /// [`DollarGate`]): one that opens no nested entry, so its batch settles the
    /// seed and nothing else and there is no neighbor to save.
    ///
    /// A memo slot would not merely be idle here, it would be a hazard. The one
    /// re-query these gates see is a demoted `$$` whose second `$` re-enters
    /// [`Self::element`] as a fresh opener: same token index, same walk state,
    /// but `display: false` — a *different question*, which a slot keyed on the
    /// walk state alone would answer from the display verdict.
    ///
    /// With nothing to memoize and nothing but the seed to settle, the batch
    /// collects into a [`SeedVerdict`] rather than a map: these are the gates
    /// the walk queries most (`$` and `\[` are everywhere), and a per-query
    /// allocation for a single verdict is the whole cost of asking.
    fn gate_verdict<P: GatePolicy>(&self, open: usize, policy: &P) -> Option<usize> {
        // The C0 bound as an early-out, as in [`Self::gated_closer`].
        policy.last_closer(self)?;
        let mut sink = SeedVerdict {
            seed: open,
            verdict: None,
        };
        self.gate_batch(open, policy, &mut sink);
        debug_assert!(sink.verdict.is_some(), "the batch must settle its own seed");
        sink.verdict.flatten()
    }

    /// The batched walk behind every shape gate: one forward scan seeded at
    /// `open` that also settles, as a by-product, every opener it passes in
    /// the seed's own brace frame — the exact verdict each one's own scan
    /// would have computed under the current walk state. Settled verdicts go
    /// to `verdicts`, whose two implementations decide how many are kept
    /// ([`VerdictSink`]); the scan itself never reads them back.
    ///
    /// The transform from a per-opener scan is possible because such a scan
    /// counts nested openers only at `depth == 0`: every opener this scan
    /// passes shares the seed's brace frame exactly, so `depth` is common to
    /// all of them, an entry's environment count relative to itself is
    /// `envs - envs_at_push`, and its nested-opener count is the number of
    /// stack entries above it — closer matching is pure LIFO.
    ///
    /// The one non-obvious rule: a refuted entry is **settled, never
    /// removed**. A per-opener scan counts nested openers *by name*
    /// ([`GatePolicy::opens_at`] membership) and never un-counts one, so a
    /// later closer must still be consumed by the refuted entry's slot. In
    /// `\ifA \begin{center} \ifB \end{center} \fi \fi`, the unowed `\end`
    /// refutes `\ifB` — but `\ifA`'s own scan still counts `\ifB` as nested
    /// and pairs with the *second* `\fi`. Popping `\ifB` at the `\end` would
    /// hand the first `\fi` to `\ifA`: a different verdict, a different tree.
    /// A closer that pops an already-settled entry records nothing. Every gate
    /// that joins this driver has the same never-un-counted countdown, so the
    /// rule is the driver's, not the conditional gate's.
    ///
    /// Per anchor, mirroring the pre-batch conditional scan token for token:
    /// - a closer at depth 0 pops the top entry; if it was still live, its
    ///   verdict is `Some` iff no `\begin`-opened environment stands in the
    ///   way (`envs == envs_at_push`, the old `envs == 0` restated — waived by
    ///   [`GatePolicy::CLOSER_NEEDS_ENV_BALANCE`]) and [`GatePolicy::pairs`]
    ///   accepts it;
    /// - a paragraph break (for a gate that anchors on one) or an unowed
    ///   `\end` refutes exactly the live
    ///   entries at their own level (`envs_at_push == envs`) — a contiguous
    ///   top suffix of the live stack, whose `envs_at_push` values are
    ///   non-decreasing and capped at `envs` by construction — and the `\end`
    ///   then decrements `envs` for the survivors;
    /// - math, an unbalanced `}` (under an enclosing group, or anywhere for a
    ///   gate reading [`StrayBrace::RefutesAlways`]), a `macrocode` frame, and
    ///   the end bound refute everything still live.
    ///
    /// The scan ends as soon as no live entry remains.
    fn gate_batch<P: GatePolicy, S: VerdictSink>(&self, open: usize, policy: &P, verdicts: &mut S) {
        struct Entry {
            opener: usize,
            envs_at_push: usize,
            settled: bool,
        }
        /// Settle every live entry sitting at its own environment level: the
        /// level anchor at hand refutes exactly those.
        fn settle_level<S: VerdictSink>(
            pending: &mut [Entry],
            live: &mut Vec<usize>,
            verdicts: &mut S,
            envs: usize,
        ) {
            while let Some(&idx) = live.last() {
                let entry = &mut pending[idx];
                if entry.envs_at_push != envs {
                    break;
                }
                entry.settled = true;
                verdicts.insert(entry.opener, None);
                live.pop();
            }
        }
        /// The [`Nesting::Interleaved`] twin of [`settle_level`]: settle the one
        /// entry that owns the innermost frame, and only when no environment
        /// stands inside it. The entries below are shielded by that frame and
        /// keep scanning — a settled entry keeps its frame, so a later closer
        /// still consumes it.
        fn settle_innermost<S: VerdictSink>(
            pending: &mut [Entry],
            live: &mut Vec<usize>,
            verdicts: &mut S,
            envs: usize,
        ) {
            let Some(entry) = pending.last_mut() else {
                return;
            };
            if entry.settled || entry.envs_at_push != envs {
                return;
            }
            entry.settled = true;
            verdicts.insert(entry.opener, None);
            // An unsettled top of `pending` is the topmost live entry: an entry
            // leaves `live` only by being settled or by being popped from
            // `pending` outright.
            debug_assert_eq!(live.last().copied(), Some(pending.len() - 1));
            live.pop();
        }
        let mut pending = vec![Entry {
            opener: open,
            envs_at_push: 0,
            settled: false,
        }];
        // Indices into `pending` of the entries still awaiting a verdict,
        // ascending.
        let mut live = vec![0usize];
        let mut depth = 0usize;
        let mut envs = 0usize;
        let mut newlines = 0;
        // Inside a `$…$` region the entries read *through*: their openers and
        // closers stop counting until the matching `$`. Only
        // [`DollarAnchor::Transparent`] ever sets it.
        let mut transparent = false;
        let end = self
            .macrocode_end
            .unwrap_or(self.tokens.len())
            .min(self.tokens.len())
            .min(policy.last_closer(self).map_or(0, |last| last + 1));
        let mut i = open + 1;
        while i < end {
            self.tick_scan();
            let t = &self.tokens[i];
            match t.kind {
                SyntaxKind::NEWLINE => {
                    newlines += 1;
                    // A break anchors at an entry's *own* level only,
                    // `depth == 0 && envs == envs_at_push`. Deeper than that it
                    // is ordinary body trivia, and a gate stricter than the
                    // parse it guards drops the node: a display equation built
                    // out of `tikzpicture` cells (`\[ \begin{array}…
                    // \begin{tikzpicture}<blank line>… \]`, issue #70) lost its
                    // math node and reported its own `\]` as unmatched. The
                    // bracket family is the exception, and for the same reason:
                    // `optional` bails at a break wherever the cursor stands
                    // ([`ParagraphAnchor::AnyDepth`]).
                    if newlines >= BLANK_LINE_NEWLINES
                        && match P::PARAGRAPH_ANCHOR {
                            ParagraphAnchor::None => false,
                            ParagraphAnchor::OwnLevel => depth == 0,
                            ParagraphAnchor::AnyDepth => true,
                        }
                    {
                        if P::PARAGRAPH_ANCHOR == ParagraphAnchor::AnyDepth {
                            break;
                        }
                        // Under interleaved nesting the break is seen only by
                        // the entry owning the innermost frame: every entry
                        // below has that frame on its own stack, so its
                        // `stack.is_empty()` test cannot fire ([`Nesting`]).
                        match P::NESTING {
                            Nesting::Counted => {
                                settle_level(&mut pending, &mut live, verdicts, envs);
                            }
                            Nesting::Interleaved => {
                                settle_innermost(&mut pending, &mut live, verdicts, envs);
                            }
                        }
                        if live.is_empty() {
                            return;
                        }
                    }
                    i += 1;
                    continue;
                }
                // A `.dtx` doc margin floats like whitespace — it is one byte of
                // layout, not content — so a margin-only line `%\n%\n` still reads
                // as the blank line its two `NEWLINE`s make it.
                SyntaxKind::WHITESPACE | SyntaxKind::DOC_MARGIN => {
                    i += 1;
                    continue;
                }
                // A docstrip guard is content *on its line*, and a line docstrip
                // deletes outright when it strips the file, so `%<*dtx>` between
                // two lines does not part them (issue #71): it breaks the newline
                // run without being a newline. That is
                // [`TriviaScan::saw_blank_line_outside_guards`], the considered
                // model, and every gate reads it — see the type-level note on
                // [`GatePolicy`].
                SyntaxKind::GUARD => {
                    newlines = 0;
                    i += 1;
                    continue;
                }
                // Math swallows whatever it spans, and this scan does not model
                // the `$`/`\[`/`\(` shape gates that decide whether a delimiter
                // opens any. Rather than re-derive them, a gate that lives in
                // text refuses at math *starting*: a construct whose closer sits
                // behind such a delimiter stays a plain command. A conservative
                // false negative, per the parser's standing preference for them.
                // The demotion gate reverses the direction and a gate that lives
                // *inside* math reverses the side ([`MathAnchor`]). A `$` is both
                // sides at once, so it anchors for either — unless it opens a
                // region the gate reads *through* ([`DollarAnchor`]).
                SyntaxKind::DOLLAR
                    if depth == 0 && policy.dollar_anchor() == DollarAnchor::Refutes =>
                {
                    break;
                }
                SyntaxKind::DOLLAR
                    if depth == 0 && policy.dollar_anchor() == DollarAnchor::Transparent =>
                {
                    transparent = !transparent;
                }
                SyntaxKind::CONTROL_SYMBOL
                    if (depth == 0 || P::ANCHORS_AT_ANY_DEPTH)
                        && P::MATH_ANCHOR.anchors(t.text.as_str()) =>
                {
                    break;
                }
                SyntaxKind::L_BRACE if !self.plain_braces.contains(&i) => depth += 1,
                SyntaxKind::R_BRACE if !self.plain_braces.contains(&i) => {
                    if depth == 0 {
                        // A `}` closing a group opened before the opener always
                        // wins: braces are catcode structure while the gated
                        // delimiters are only macros. Whether one with *no* such
                        // group behind it (the walk is at the outer level) means anything,
                        // and what it means at all, is the gate's own call
                        // ([`StrayBrace`]).
                        match P::STRAY_BRACE {
                            StrayBrace::RefutesInGroup if self.in_group() => break,
                            StrayBrace::ClosesInGroup if self.in_group() => {
                                // Every live entry escapes at the same brace:
                                // `depth` is common to the whole frame, so each
                                // one's own scan would reach this `}` at its own
                                // depth 0 too.
                                for &idx in &live {
                                    verdicts.insert(pending[idx].opener, Some(i));
                                }
                                return;
                            }
                            StrayBrace::RefutesAlways => break,
                            _ => {}
                        }
                    } else {
                        depth -= 1;
                    }
                }
                // Any token at the entries' own brace level may be a delimiter:
                // the pairing gates close on a `CONTROL_WORD`, but the math
                // gates close on a `DOLLAR` and a `CONTROL_SYMBOL`. Every policy
                // tests the kind inside its own predicate, so asking wider costs
                // the narrow ones nothing but the call.
                _ => {
                    if !transparent && depth == 0 && policy.opens_at(self, i) {
                        // A gate whose openers are `\begin`s counts this one
                        // before pushing, so the entry's own environment is not
                        // in its `envs_at_push` — its per-opener scan starts one
                        // token past the `\begin` and never saw it either.
                        if P::OPENER_IS_ENV_BEGIN {
                            envs += 1;
                        }
                        live.push(pending.len());
                        pending.push(Entry {
                            opener: i,
                            envs_at_push: envs,
                            settled: false,
                        });
                    } else if !transparent && depth == 0 && policy.closes_at(self, i) {
                        let entry = pending
                            .pop()
                            .expect("a live entry remains, so pending is non-empty");
                        // Under interleaved nesting the closer pops the
                        // *innermost frame*, so an environment opened since this
                        // entry is a frame mismatch — and one every outer entry
                        // sees too, since this entry's frame is their innermost
                        // one. It refuses the whole scan rather than one entry
                        // ([`Nesting`]). This entry is out of `pending` already,
                        // so it settles itself here and the trailing refusal
                        // covers the rest.
                        if P::NESTING == Nesting::Interleaved && envs != entry.envs_at_push {
                            if !entry.settled {
                                live.pop();
                                verdicts.insert(entry.opener, None);
                            }
                            break;
                        }
                        if !entry.settled {
                            live.pop();
                            // `envs == envs_at_push` for the same reason as
                            // `depth == 0`: a closer inside an environment the
                            // construct opened is consumed by that
                            // environment's body, so it is not a closer the
                            // walk can reach — unless the closer is a *math
                            // delimiter*, which ends the body wherever it sits
                            // ([`GatePolicy::CLOSER_NEEDS_ENV_BALANCE`]).
                            let balanced =
                                !P::CLOSER_NEEDS_ENV_BALANCE || envs == entry.envs_at_push;
                            let paired = balanced && policy.pairs(self, entry.opener, i);
                            verdicts.insert(entry.opener, paired.then_some(i));
                            if live.is_empty() {
                                return;
                            }
                        }
                    } else if t.kind == SyntaxKind::CONTROL_WORD
                        && (depth == 0 || P::ANCHORS_AT_ANY_DEPTH)
                        && (P::ENV_ANCHOR_IN_MACRO_CODE || !self.in_macro_code(i))
                    {
                        // In a definition body or an expl3 region `\begin`/`\end`
                        // are plain commands that need not pair, so neither
                        // anchors nor nests there (issues #45/#60) — bar the one
                        // gate whose pre-batch scan never carried the filter
                        // ([`GatePolicy::ENV_ANCHOR_IN_MACRO_CODE`]).
                        if self.env_begin_at(i) {
                            // A `macrocode` chunk is a hard boundary in both
                            // directions: docstrip is line-oriented, so the code
                            // layer and the documentation layer around it are
                            // different files as far as TeX is concerned. Nothing
                            // is gained by pairing across one, and a `.dtx` doc
                            // layer that does — `%<latexrelease>` guarded
                            // `\if#1b\vbox \else…` blocks in `ltboxes.dtx` — runs
                            // the construct over every chunk in between, stranding
                            // the cursor past `macrocode_end` for every
                            // chunk-bounded scan downstream. (The other direction
                            // is already bounded: a conditional *inside* a chunk
                            // scans only to `macrocode_end`.) The math gates opt
                            // out ([`GatePolicy::MACROCODE_FRAME_ANCHORS`]).
                            if P::MACROCODE_FRAME_ANCHORS
                                && peek_begin_name(self.tokens, i).is_some_and(|n| {
                                    matches!(n.as_ref(), "macrocode" | "macrocode*")
                                })
                            {
                                break;
                            }
                            // An optional never legitimately spans an
                            // environment, so for the bracket family either half
                            // is a runaway `[` and there is nothing to count
                            // ([`EnvAnchor`]).
                            if P::ENV_ANCHOR == EnvAnchor::Refutes {
                                break;
                            }
                            envs += 1;
                        } else if self.env_end_at(i) {
                            if P::ENV_ANCHOR == EnvAnchor::Refutes {
                                break;
                            }
                            if P::ENV_END_UNWINDS_OPENERS {
                                let end_name = peek_end_name(self.tokens, i);
                                let mut matched = false;
                                while let Some(entry) = pending.pop() {
                                    envs = entry.envs_at_push;
                                    if !entry.settled {
                                        let live_entry = live.pop();
                                        debug_assert_eq!(live_entry, Some(pending.len()));
                                        verdicts.insert(entry.opener, None);
                                    }
                                    if peek_begin_name(self.tokens, entry.opener).as_deref()
                                        == end_name.as_deref()
                                    {
                                        matched = true;
                                        break;
                                    }
                                }
                                // A mismatched closer is the same recovery
                                // anchor every per-opener scan used. A named
                                // match may unwind several nested environments,
                                // exactly as `finish_environment` does.
                                if !matched || live.is_empty() {
                                    for &idx in &live {
                                        verdicts.insert(pending[idx].opener, None);
                                    }
                                    return;
                                }
                                i += 1;
                                newlines = 0;
                                continue;
                            }
                            match P::NESTING {
                                // The `\end` must find an environment innermost.
                                // It does not when the entry on top of `pending`
                                // was pushed at the current `envs`: that entry's
                                // frame is in the way, for it and for every entry
                                // below it alike, so the mismatch refuses the
                                // whole scan. A settled entry still holds its
                                // frame ([`Nesting`]).
                                Nesting::Interleaved => {
                                    if pending.last().is_some_and(|e| e.envs_at_push == envs) {
                                        break;
                                    }
                                }
                                Nesting::Counted => {
                                    settle_level(&mut pending, &mut live, verdicts, envs);
                                    if live.is_empty() {
                                        return;
                                    }
                                }
                            }
                            // A survivor has `envs_at_push < envs`, so the
                            // decrement cannot underflow.
                            envs -= 1;
                        }
                    }
                }
            }
            newlines = 0;
            i += 1;
        }
        // Global refusals — math, an unbalanced `}`, a `macrocode` frame, the
        // end bound: everything still live demotes.
        for &idx in &live {
            verdicts.insert(pending[idx].opener, None);
        }
    }

    /// The token index closing the environment-alias opener at `open`, or `None`
    /// when it does not pair — in which case the opener stays a plain `COMMAND`
    /// with **no diagnostic**, like a gated `$`/`\[`/`\begin`.
    ///
    /// This is a **positive** gate, transcribed from [`Self::conditional_closer`]
    /// rather than from [`Self::environment_escapes_group`]. The `\begin` gate is a
    /// *demotion* gate on a construct that pairs by default and carries an
    /// unclosed-environment diagnostic worth preserving. An alias opener is a bare
    /// control word with no `{name}` corroborating it and no diagnostic to keep, so
    /// "pair unless refuted" would be far too optimistic: it must be refused unless
    /// its closer is positively located, and the walk is then bounded by that index.
    ///
    /// Requirements the driver ([`Self::gate_batch`]) carries for it, shared
    /// with the sibling gates:
    ///
    /// - **Brace level.** A `}` closing a group opened before the opener always
    ///   wins — braces are catcode structure, an alias is only a macro (issue #71).
    /// - **`envs == 0`.** A closer inside an environment the alias opened is
    ///   consumed by that environment's body, so the walk cannot reach it.
    /// - **Math refuses.** The scan does not model the `$`/`\[`/`\(` shape gates,
    ///   so rather than re-derive them it declines behind one.
    /// - **`macrocode` bounds it both ways**, as for conditionals.
    ///
    /// What is this gate's own is in [`AliasGate`]: no paragraph anchor, and a
    /// closer that must name the opener's target.
    ///
    /// Batched and memoized like the conditional gate — and here the memo was
    /// load-bearing before the batch existed, since the caller asks twice
    /// ([`Self::alias_batch`]).
    pub(super) fn alias_closer(&self, open: usize) -> Option<usize> {
        // Total in `open`: [`Self::starts_block_env`] asks about any index, and
        // the driver would otherwise seed an entry for a token that opens
        // nothing.
        self.alias_openers.get(&open)?;
        self.gated_closer(open, &AliasGate, &self.alias_batch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::lexer::{ParseCtx, lex};

    fn scan_work(input: &str) -> usize {
        let tokens = lex(input);
        let ctx = ParseCtx::default();
        let mut p = Parser::new(&tokens, &ctx);
        p.document();
        p.scan_work.get()
    }

    #[track_caller]
    fn assert_scan_work_linear(small: &str, doubled: &str) {
        let (w1, w2) = (scan_work(small), scan_work(doubled));
        assert!(
            w2 < 3 * w1 + 64,
            "gate-scan work grew superlinearly: {w1} -> {w2}"
        );
    }

    #[test]
    fn gate_scans_stay_linear_without_closers() {
        let shape = "\\ifabc x\n";
        assert_scan_work_linear(&shape.repeat(200), &shape.repeat(400));
        let shape = "\\cmd[x\n";
        assert_scan_work_linear(&shape.repeat(200), &shape.repeat(400));
        let shape = "\\[ x\n";
        assert_scan_work_linear(&shape.repeat(200), &shape.repeat(400));
    }

    #[test]
    fn expl3_arity_scan_stays_linear() {
        let body = |n: usize| {
            format!(
                "\\ExplSyntaxOn\n{}",
                "\\tl_set:Nn \\l_a { x y z }\n".repeat(n)
            )
        };
        assert_scan_work_linear(&body(200), &body(400));
        let body = |n: usize| {
            format!(
                "\\ExplSyntaxOn\n{}",
                "\\prop_get:NnNTF \\p { k } \\l { t } x\n".repeat(n)
            )
        };
        assert_scan_work_linear(&body(200), &body(400));
    }

    #[test]
    fn expl3_arity_nested_scans_stay_linear() {
        let body = |n: usize| {
            format!(
                "\\ExplSyntaxOn\n{}x{}\n",
                "\\use:n { ".repeat(n),
                " }".repeat(n)
            )
        };
        assert_scan_work_linear(&body(100), &body(200));
        let body = |n: usize| {
            format!(
                "\\ExplSyntaxOn\n\\prop_get:NnNTF \\p {{ k }} \\l {}x{} y\n",
                "\\use:n { ".repeat(n),
                " }".repeat(n)
            )
        };
        assert_scan_work_linear(&body(100), &body(200));
    }

    #[test]
    fn conditional_batch_keeps_shared_frame_openers_linear() {
        let body = |n: usize| format!("{}\\fi\n", "\\ifabc x\n".repeat(n));
        assert_scan_work_linear(&body(200), &body(400));
    }

    #[test]
    fn alias_batch_keeps_shared_frame_openers_linear() {
        let scan_work = |input: &str| {
            let tokens = lex(input);
            let mut ctx = ParseCtx::default();
            ctx.insert_begin_alias(SmolStr::new("bc"), SmolStr::new("center"));
            ctx.insert_end_alias(SmolStr::new("ec"), SmolStr::new("center"));
            let mut p = Parser::new(&tokens, &ctx);
            p.document();
            p.scan_work.get()
        };
        let body = |n: usize| format!("{}\\ec\n", "\\bc x\n".repeat(n));
        let (w1, w2) = (scan_work(&body(200)), scan_work(&body(400)));
        assert!(
            w2 < 3 * w1 + 64,
            "gate-scan work grew superlinearly: {w1} -> {w2}"
        );
    }

    #[test]
    fn env_batch_keeps_shared_frame_openers_linear() {
        let body = |n: usize| format!("{{\n{}", "\\begin{itemize}\n".repeat(n));
        assert_scan_work_linear(&body(200), &body(400));
    }

    #[test]
    fn left_right_batch_keeps_shared_frame_openers_linear() {
        let body = |n: usize| format!("$ {}\\right)$\n", "\\left( x ".repeat(n));
        assert_scan_work_linear(&body(200), &body(400));
    }

    #[test]
    fn bracket_batch_keeps_shared_frame_openers_linear() {
        let body = |n: usize| format!("{}]\n", "\\cmd[x\n".repeat(n));
        assert_scan_work_linear(&body(200), &body(400));
        let body = |n: usize| format!("$ {}]$\n", "\\cmd[x ".repeat(n));
        assert_scan_work_linear(&body(200), &body(400));
        let body = |n: usize| format!("{}]{{tail}}\n", "\\cmd[x\n\n".repeat(n));
        assert_scan_work_linear(&body(200), &body(400));
    }

    #[test]
    fn math_gate_scans_stay_linear_without_closers() {
        let body = |n: usize| format!("$ {}$", "\\cmd[x ".repeat(n));
        assert_scan_work_linear(&body(200), &body(400));
        let body = |n: usize| format!("\\[\n{}\\]\n", "\\left( x\n".repeat(n));
        assert_scan_work_linear(&body(200), &body(400));
    }

    #[test]
    fn math_batch_stays_linear_with_one_closer_at_eof() {
        let body = |n: usize| format!("{}\\]\n", "\\[ x\n".repeat(n));
        assert_scan_work_linear(&body(200), &body(400));
        let body = |n: usize| format!("{}\\)\n", "\\( x\n".repeat(n));
        assert_scan_work_linear(&body(200), &body(400));
        let body = |n: usize| format!("{}$\n", "$ x\n".repeat(n));
        assert_scan_work_linear(&body(200), &body(400));
        let body = |n: usize| format!("{}$$\n", "$$ x\n".repeat(n));
        assert_scan_work_linear(&body(200), &body(400));
    }
}
