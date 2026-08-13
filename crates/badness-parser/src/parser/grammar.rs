//! The Phase 1 recursive-descent grammar for LaTeX surface syntax.
//!
//! The parser walks the full token stream (trivia included) and emits a flat
//! list of [`Event`]s — `Start(kind)` / `Tok(idx)` / `Finish` — that
//! [`super::tree_builder`] replays into a green tree. Because every token is
//! emitted exactly once, in order, via [`Parser::bump`], losslessness holds by
//! construction: `pos` only ever advances through `bump`, and nothing else
//! touches it.
//!
//! It is **error-tolerant**: a malformed construct never aborts the parse. Each
//! recovery records a [`SyntaxError`] on the side channel and either closes the
//! current node gracefully or skips a single token, always making progress.
//! Recovery anchors are the LaTeX-natural ones: `\end`, `}`, `]`, `$`, blank
//! lines, and end of input.

mod facts;
mod prescan;
mod trivia;

use std::borrow::Cow;

use crate::parser::conditional;
use crate::parser::core::SyntaxError;
use crate::parser::events::Event;
use crate::parser::lexer::{ParseCtx, Token, is_block_environment, is_math_environment};
use crate::syntax::SyntaxKind;
use facts::{BracketPolicy, is_big_delimiter_command, is_definition_body_command};
use prescan::PreScan;
use smol_str::SmolStr;
use trivia::{BLANK_LINE_NEWLINES, CommentMode};

/// Re-exported at its historical path: `parser.rs`'s façade and the formatter's
/// expl3 region gate both spell it `grammar::is_def_prefix_command`.
pub use facts::is_def_prefix_command;

const BEGIN_CMD: &str = "\\begin";
const END_CMD: &str = "\\end";
const LEFT_CMD: &str = "\\left";
const RIGHT_CMD: &str = "\\right";

/// Maximum number of consecutive cursor peeks with **no** token consumed before
/// the parser aborts as stuck. Modeled on rust-analyzer's `PARSER_STEP_LIMIT`
/// (`crates/parser/src/parser.rs`), a catch-all against a non-advancing loop that
/// holds *independent of grammar correctness*. The counter resets on every cursor
/// advance (see [`Parser::step`]), so in normal parsing only O(1) peeks accrue
/// between two consumed tokens; this ceiling is astronomically above any real
/// document and can only be reached by a genuine infinite loop. Unlike the
/// module's structural "`pos` only advances through `bump`" argument, this holds
/// even for malformed or adversarial input (fuzzing, a corrupt corpus file).
const PARSER_STEP_LIMIT: u32 = 15_000_000;

/// A content region that groups its children into `PARAGRAPH` nodes separated
/// by blank lines. Differs only in how the region terminates.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    /// The whole document; ends at EOF.
    Document,
    /// An environment body; ends at the next `\end` (any name — the caller
    /// checks the name and decides whether to consume it).
    Environment,
    /// A `.dtx` `macrocode` body: macro code, so a bare `\end` in the code is a
    /// plain command, and the block ends *positionally* at the pre-scanned frame
    /// terminator ([`Parser::macrocode_end`]), never at an arbitrary `\end`.
    Macrocode,
}

/// Parse a token stream into parser events and a list of syntax errors.
pub(crate) fn parse(tokens: &[Token], ctx: &ParseCtx) -> (Vec<Event>, Vec<SyntaxError>) {
    let mut p = Parser::new(tokens, ctx);
    p.document();
    debug_assert_balanced(&p.events);
    (p.events, p.errors)
}

/// Debug-only structural tripwire: the event stream must be balanced — every
/// `Start` matched by a later `Finish`, no `Finish` before its `Start`, and the
/// document node closed exactly once. This is the cheap analog of
/// rust-analyzer's per-`Marker` `DropBomb`: a grammar edit that leaks an
/// [`Parser::open`] without a [`Parser::close`] (or a [`Parser::precede`] that
/// splices in a `Start` nobody closes) is caught right here,
/// counting *all* start/finish events regardless of how they were emitted,
/// before [`super::tree_builder`] feeds rowan's `GreenNodeBuilder` and fails with
/// a far more opaque `finish_node` panic. Compiled out of release builds.
fn debug_assert_balanced(events: &[Event]) {
    if !cfg!(debug_assertions) {
        return;
    }
    let mut depth: i32 = 0;
    for ev in events {
        match ev {
            Event::Start(_) => depth += 1,
            Event::Finish => {
                depth -= 1;
                debug_assert!(depth >= 0, "parser emitted a Finish with no open node");
            }
            Event::Tok(_) | Event::SubTok { .. } => {}
        }
    }
    debug_assert_eq!(
        depth, 0,
        "parser left {depth} node(s) unclosed at end of parse"
    );
}

/// The walk state a gate batch's scan reads, and therefore the key its
/// verdicts stay valid under ([`Parser::walk_key`]). Everything else a scan
/// consults is pre-scanned token state, a pure function of the text.
///
/// `plain_braces` rides a version counter rather than the set itself: it is
/// saved and restored in lockstep with `macrocode_end` per chunk
/// ([`Parser::macrocode_body`]), so pinning the frame already pins it, but a
/// counter makes that a fact instead of an argument — and a version can only
/// ever invalidate a batch the frame index would have kept.
#[derive(Clone, Copy, PartialEq, Eq)]
struct WalkKey {
    macrocode_end: Option<usize>,
    in_def_body: bool,
    in_group: bool,
    plain_braces: u32,
    /// The innermost enclosing math's flavor ([`Parser::math_dollar`]), read by
    /// [`MathBracketGate`] alone: inside `$…$` a `$` is that math's own closer
    /// and refutes, while inside `\[…\]` it opens a *transparent* nested inline
    /// region ([`DollarAnchor`]). Nothing else in this key moves when the flavor
    /// does — entering a `$` inside `\[…\]` changes no brace, group, or frame
    /// state — so without it a batch harvested under one flavor would answer for
    /// the other.
    enclosing_math_is_dollar: bool,
}

/// One batched run of a shape gate's scan ([`Parser::gate_batch`]): the
/// per-opener verdicts it harvested, valid under the walk state in `key`.
struct GateBatch {
    key: WalkKey,
    /// Opener index → the verdict that opener's own scan would have computed
    /// under `key`, for every same-frame opener the scan passed. Openers
    /// absent from the map sat behind a brace during the batch and re-batch
    /// when queried.
    verdicts: std::collections::HashMap<usize, Option<usize>>,
}

/// Where a batch deposits the verdicts it settles ([`Parser::gate_batch`]).
///
/// The driver has one loop and one set of `insert` calls; what varies is who
/// keeps the results. A memoized gate keeps all of them, since the whole point
/// is answering the openers ahead of the seed from one scan. A **single-entry**
/// gate ([`Parser::gate_verdict`]) opens no nested entry, so the seed is the
/// only opener its batch can ever settle — and building a `HashMap` to hold one
/// verdict costs an allocation per `$` and `\[` in the document.
///
/// Discarding the non-seed verdicts is safe for *any* gate, not just a
/// single-entry one: the driver's decisions never read back what it inserted.
/// It is only worth doing where there is nothing else to keep.
trait VerdictSink {
    fn insert(&mut self, opener: usize, verdict: Option<usize>);
}

impl VerdictSink for std::collections::HashMap<usize, Option<usize>> {
    fn insert(&mut self, opener: usize, verdict: Option<usize>) {
        std::collections::HashMap::insert(self, opener, verdict);
    }
}

/// The allocation-free [`VerdictSink`] for a single-entry gate: one slot for
/// the seed, and everything else dropped on the floor.
struct SeedVerdict {
    seed: usize,
    /// `None` until the batch settles the seed; the driver guarantees it does
    /// (`debug_assert` in [`Parser::gate_verdict`]).
    verdict: Option<Option<usize>>,
}

impl VerdictSink for SeedVerdict {
    fn insert(&mut self, opener: usize, verdict: Option<usize>) {
        if opener == self.seed {
            self.verdict = Some(verdict);
        }
    }
}

/// What a `}` at a gate's own brace level means to that gate. The scan reached
/// it without having opened the group, so the brace closes a group opened
/// *before* the entries — or no group at all, when the walk itself is at the
/// outer level.
///
/// The token event is the same for every gate — braces are catcode structure
/// while the gated delimiters are only macros, so the `}` always wins — but the
/// *verdict* it implies differs on two axes: the positive gate/demotion gate
/// line, and whether a brace with no group behind it means anything. Both are
/// policy, not bookkeeping.
#[derive(PartialEq, Eq)]
enum StrayBrace {
    /// Refutes every live entry, but only when the walk sits inside a group
    /// (the walk is inside a group): a conditional or an alias cannot pair across the
    /// `}` that ends the group its opener sits in, while at the outer level a
    /// stray `}` is somebody else's business and the scan carries on.
    RefutesInGroup,
    /// It *is* the closer, on the same condition:
    /// [`Parser::environment_escapes_group`] asks whether the `\begin` escapes
    /// its group, and this brace is the escape.
    ClosesInGroup,
    /// Refutes every live entry unconditionally. The math gates mirror
    /// [`Parser::dollar_math`] and [`Parser::delim_math`], which bail at *any*
    /// unbalanced `}` — for them the brace is a recovery anchor of the parse
    /// itself, not merely a group boundary — and a gate must mirror the parse
    /// it guards. [`LeftRightGate`] joins them: [`Parser::left_right`] bails at
    /// any unbalanced `}` too.
    RefutesAlways,
}

/// Which math delimiters anchor a gate's scan at the entries' own level.
///
/// Which side anchors follows from where the gated construct *lives*. A
/// conditional or an alias lives in text, so what defeats it is math
/// **starting**: the scan does not model the `$`/`\[`/`\(` shape gates, and a
/// closer behind a delimiter that opens math may well be swallowed by it. A
/// `\left…\right` pair already lives *inside* math, so what defeats it is the
/// enclosing math **ending** — exactly [`Parser::left_right`]'s own recovery
/// anchors. A `$` is both, and anchors either way.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MathAnchor {
    /// Nothing anchors: a foreign delimiter is ordinary content. The demotion
    /// gate reads it this way because refusing there would *keep* a construct
    /// the scan cannot vouch for, and for the math gates the delimiter is the
    /// closer itself.
    None,
    /// `$`, `\[`, `\(` — math starting.
    Opening,
    /// `$`, `\]`, `\)` — math ending.
    Closing,
}

impl MathAnchor {
    /// Whether the control symbol `text` anchors under this policy. `$` is
    /// handled by the driver's own [`SyntaxKind::DOLLAR`] arm, since it is both
    /// an opener and a closer ([`DollarAnchor`]).
    fn anchors(self, text: &str) -> bool {
        match self {
            MathAnchor::None => false,
            MathAnchor::Opening => matches!(text, "\\[" | "\\("),
            MathAnchor::Closing => matches!(text, "\\]" | "\\)"),
        }
    }
}

/// What a `$` at the entries' own brace level means — the one axis a gate reads
/// at *runtime* rather than from a const, because [`MathBracketGate`] decides it
/// from the enclosing math's flavor.
///
/// It is separate from [`MathAnchor`] because a `$` is both sides of the
/// delimited pair at once, so the two-sided question that enum answers does not
/// apply to it, and because it has a third reading no delimiter has.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DollarAnchor {
    /// Ordinary content: the math gates, whose own closer it is.
    Content,
    /// Refutes every live entry, the reading of every gate that anchors on math
    /// at all — a `$` opens math for a text-dwelling gate and ends it for a
    /// math-dwelling one.
    Refutes,
    /// Toggles a **transparent** nested inline region: the entries' own openers
    /// and closers stop counting until the matching `$`, and everything else
    /// (braces, the paragraph run, the delimiter and environment anchors) reads
    /// on unchanged. [`MathBracketGate`] inside `\[…\]`/`\(…\)` or a math
    /// environment, where a `$` really does open a nested inline region, so a
    /// balanced `$…$` inside the bracket is content rather than a boundary
    /// (`\[ \inferrule*[right=$\Pi$-eq]{A}{B} \]`). An unbalanced `$` leaves the
    /// region open, so no `]` is ever accepted and the scan runs to its bound.
    Transparent,
}

/// Whether a blank line refutes an entry, and at what brace level.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ParagraphAnchor {
    /// Never. An alias body legitimately spans blank lines, and an environment's
    /// escaping `}` may sit paragraphs away.
    None,
    /// At an entry's *own* level (`depth == 0 && envs == envs_at_push`) only.
    /// Deeper than that a break is ordinary body trivia, and a gate stricter
    /// than the parse it guards drops the node: a display equation built out of
    /// `tikzpicture` cells (issue #70) lost its math node this way.
    OwnLevel,
    /// At any depth, refuting every live entry outright. The bracket family:
    /// [`Parser::optional`] bails at a paragraph break wherever the cursor
    /// stands, since an unclosed `[` must never swallow the document, so its
    /// gates mirror that.
    AnyDepth,
}

/// What a `\begin`/`\end` pair between an entry and the token at hand means.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EnvAnchor {
    /// Counted, so a closer inside an environment the construct opened is known
    /// to be out of the walk's reach (`envs`/`envs_at_push`). Every gate whose
    /// construct may legitimately span one.
    Counts,
    /// Refutes every live entry outright, in *both* halves. The bracket family:
    /// [`Parser::optional`] bails at a `\begin` as readily as at an `\end`
    /// (either means a runaway `[`), so there is no shape in which an optional
    /// spans an environment and nothing to count.
    Refutes,
}

/// How a gate's nested entries relate to the `\begin`-opened environments
/// between them — the shape of the model its per-opener scan used, which the
/// batch must reproduce exactly.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Nesting {
    /// Two independent counters. A nested opener is counted *by name* and never
    /// un-counted, environments are counted separately, and an anchor settles
    /// every entry standing at its own environment level. This is what the
    /// pairing gates' per-opener scans did: they never modelled the order in
    /// which a nested opener and an environment were opened, only how many of
    /// each stood between an entry and the token at hand.
    Counted,
    /// One LIFO stack, entries and environments interleaved.
    /// [`Parser::left_right_closes`] tracks its nesting this way because
    /// [`Parser::left_right`] does: a pair is closed by *count* wherever it
    /// sits, so its scan pushes a frame per `\left`, per `\begin`, and per `{`
    /// alike, and any frame **mismatch** — an `\end` or a `\right` reaching a
    /// frame of the wrong kind — refuses outright.
    ///
    /// Two consequences the driver must honor, both following from *whose*
    /// frame is innermost. A mismatch is seen by every outer entry too, since
    /// the innermost frame is common to all of them, so it refutes the whole
    /// scan rather than one level of it. An *absence* of frames, on the other
    /// hand — the blank-line anchor's `stack.is_empty()` — is seen only by the
    /// entry that owns the innermost frame, so a nested entry **shields** the
    /// entries below it from a paragraph break.
    Interleaved,
}

/// The per-gate half of [`Parser::gate_batch`]: how far this gate's scan can
/// reach, which tokens open and close its construct, and whether a blank line
/// anchors it.
///
/// The driver owns the bookkeeping every gate shares — the bound, brace depth
/// under [`Parser::plain_braces`], environment counting, the `macrocode`
/// frame, the entry stack, the metering — and **never averages the
/// policies**: where two gates diverge, the divergence is a method here,
/// spelled out and documented, not a compromise inside the loop (`TODO.md`,
/// container stack C2).
///
/// Hooks arrive with the client that needs them, so the driver stays
/// *extracted* from C1's working batch rather than authored to a lowest common
/// denominator. Delimiters are asked for at any token kind at the entries' own
/// brace level, since the math gates close on a `DOLLAR` and a `CONTROL_SYMBOL`
/// — every policy tests the kind inside its own predicate anyway.
trait GatePolicy {
    /// Whether a blank line refutes an entry, and at what level. `OwnLevel` for
    /// conditionals, which mirror a parse that stops at a `\par`; `None` for
    /// aliases, whose body legitimately spans blank lines. See
    /// [`ParagraphAnchor`].
    const PARAGRAPH_ANCHOR: ParagraphAnchor;

    /// What a `}` at the entries' own brace level means. Defaults to the
    /// *positive* gates' reading; see [`StrayBrace`].
    const STRAY_BRACE: StrayBrace = StrayBrace::RefutesInGroup;

    /// Whether a brace with no match inside the current `macrocode` chunk
    /// ([`Parser::plain_braces`]) is an ordinary token rather than group
    /// structure. True everywhere but [`MathBracketGate`], whose pre-batch scan
    /// tracked every brace alike — preserved, not chosen, and one-directional:
    /// a chunk-unmatched `}` can only occur at chunk brace depth 0, so the scan
    /// meets it at its own depth 0 and refuses, while a chunk-unmatched `{` only
    /// adds depth. The unfiltered reading therefore refuses a bracket the
    /// filtered one would attach and never the reverse (`TODO.md`, container
    /// stack C2.5).
    const PLAIN_BRACES_ARE_TOKENS: bool = true;

    /// Which math delimiters at the entries' own level refute them, if any.
    ///
    /// Defaults to the text-dwelling positive gates' reading — math *starting*
    /// refutes, the conservative direction for a construct that only pairs when
    /// positively located. See [`MathAnchor`] for the other two.
    const MATH_ANCHOR: MathAnchor = MathAnchor::Opening;

    /// How this gate's nested entries interleave with the environments between
    /// them. See [`Nesting`]; the pairing and math gates all count, only
    /// [`LeftRightGate`] stacks.
    const NESTING: Nesting = Nesting::Counted;

    /// Whether this gate's openers are themselves `\begin`s, so the driver must
    /// count one in `envs` *before* pushing its entry. The entry's own
    /// environment is therefore *in* its `envs_at_push`, which is what makes the
    /// relative count `envs - envs_at_push` start at zero — matching its
    /// per-opener scan, which starts one token past the `\begin` and never saw
    /// that environment either.
    const OPENER_IS_ENV_BEGIN: bool = false;

    /// Whether the `\begin`/`\end` and math-*delimiter* anchors apply at any
    /// brace depth, rather than only at the entries' own. (A `$` is read at the
    /// entries' own level whatever this says — every gate's pre-batch scan did,
    /// and for [`MathBracketGate`] a `$` inside a group is not the boundary its
    /// depth-0 twin is. The paragraph anchor has its own knob for the same
    /// reason, [`ParagraphAnchor`].)
    ///
    /// False for the gates whose entries pair at their own brace level: an
    /// environment opened inside a `{…}` group is closed inside it too, so
    /// counting it would say nothing about what stands between an entry and its
    /// closer. The math gates set it true, as their pre-batch scans did: a math
    /// body descends into a group ([`Parser::math_group`]) and keeps parsing
    /// environments there, so `envs` tracks the whole body rather than one
    /// brace level of it. The bracket gates set it true because
    /// [`Parser::optional`]'s bail is depth-blind: a `\begin` inside a group is
    /// still a runaway `[`.
    const ANCHORS_AT_ANY_DEPTH: bool = false;

    /// What a `\begin`/`\end` between an entry and the token at hand means. See
    /// [`EnvAnchor`]; only the bracket family refuses rather than counts.
    const ENV_ANCHOR: EnvAnchor = EnvAnchor::Counts;

    /// Whether that anchor also fires *inside macro code*, where `\begin`/`\end`
    /// are plain commands that need not pair (issues #45/#60) and every other
    /// gate therefore ignores them.
    ///
    /// True for [`MathBracketGate`] alone, whose pre-batch scan carried no such
    /// filter — so it is stricter than the [`Parser::optional`] bail it mirrors,
    /// which does carry one. Preserved, not chosen: the divergence only ever
    /// refuses a bracket, the conservative direction (`TODO.md`, container stack
    /// C2.5).
    const ENV_ANCHOR_IN_MACRO_CODE: bool = false;

    /// Whether a `.dtx` doc margin or docstrip guard is transparent to the
    /// paragraph run, as it is to [`TriviaScan::saw_blank_line`].
    ///
    /// False for [`MacrocodeBracketGate`], whose pre-batch scan skipped
    /// `WHITESPACE` alone, so a guard line between two newlines breaks the run
    /// there. That is in fact the [`TriviaScan::saw_blank_line_outside_guards`]
    /// reading — docstrip *deletes* a guard-only line, so it does not part what
    /// surrounds it (issue #71) — which the other gates do not take. Preserved
    /// on both sides rather than unified, since unifying moves verdicts either
    /// way (`TODO.md`, container stack C2.5).
    const DOC_TRIVIA_FLOATS: bool = true;

    /// Whether a closer only settles an entry when no `\begin`-opened
    /// environment stands in the way (`envs == envs_at_push`).
    ///
    /// True for the gates whose closer is a *command*: one inside an environment
    /// the construct opened is consumed by that environment's body, so the walk
    /// never reaches it. The math gates set it false — their closer is a
    /// delimiter, and `$\begin{matrix} … $` really does end at the `$`, the
    /// environment's own recovery notwithstanding.
    const CLOSER_NEEDS_ENV_BALANCE: bool = true;

    /// Whether a `macrocode` chunk boundary refutes every live entry.
    ///
    /// True for the pairing gates: docstrip is line-oriented, so the code layer
    /// and the documentation layer around it are different files as far as TeX
    /// is concerned, and a construct that pairs across one runs over every chunk
    /// between (`ltboxes.dtx`). The math gates set it false, preserving their
    /// pre-batch scans, which counted `\begin{macrocode}` as an ordinary
    /// environment. Nothing is known to depend on the difference; it is kept
    /// because C2 migrates verdicts unchanged, not because it is the better
    /// reading.
    const MACROCODE_FRAME_ANCHORS: bool = true;

    /// What a `$` at the entries' own brace level means. Defaults to the
    /// two-sided reading of [`Self::MATH_ANCHOR`]: a `$` is an opening *and* a
    /// closing delimiter, so a gate that anchors on either side anchors on it,
    /// and only a gate for which math delimiters are content reads it as
    /// content too. [`MathBracketGate`] overrides it — the one gate that needs
    /// the enclosing math's flavor, which is walk state and cannot ride a const.
    fn dollar_anchor(&self) -> DollarAnchor {
        if Self::MATH_ANCHOR == MathAnchor::None {
            DollarAnchor::Content
        } else {
            DollarAnchor::Refutes
        }
    }

    /// The last index in the file that could ever settle an entry with a
    /// closer — the C0 bound. `None` refuses the whole gate without scanning.
    fn last_closer(&self, p: &Parser<'_>) -> Option<usize>;

    /// Whether the control word at `i` opens a nested entry.
    fn opens_at(&self, p: &Parser<'_>, i: usize) -> bool;

    /// Whether the control word at `i` closes the innermost live entry.
    fn closes_at(&self, p: &Parser<'_>, i: usize) -> bool;

    /// Whether the closer at `closer` really pairs with the entry opened at
    /// `opener`, for gates whose two halves carry a name to match. A `false`
    /// settles that entry as unpaired and consumes the closer either way, so
    /// an outer entry never inherits it.
    fn pairs(&self, p: &Parser<'_>, opener: usize, closer: usize) -> bool {
        let _ = (p, opener, closer);
        true
    }
}

/// [`Parser::conditional_closer`]'s policy: `\if…`-family openers pairing with
/// the next `\fi` at their own level.
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

/// [`Parser::alias_closer`]'s policy: environment-alias openers pairing with a
/// closer alias for the *same* target environment (issue #109).
///
/// Two divergences from [`ConditionalGate`], both deliberate:
///
/// - **No paragraph anchor.** An alias for `itemize` legitimately spans blank
///   lines, and the body is parsed with [`Parser::parse_block`], which builds
///   `PARAGRAPH`s inside it. Reading a blank line here would also key layout on
///   a trivia predicate the formatter does not preserve.
/// - **Names must match** ([`GatePolicy::pairs`]). Nesting counts *any* alias
///   opener and *any* alias closer, so `\bea \bce \ece \eea` pairs while the
///   crossing `\bea \bce \eea \ece` refuses outright instead of letting an
///   inner walk run past the outer bound.
///
/// Both halves are recognized only outside macro code, where `\begin`/`\end`
/// are plain commands that need not pair (issues #45/#60) — the same filter the
/// driver applies to its own `\begin`/`\end` counting.
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
        p.alias_closers.contains_key(&i) && !p.in_macro_code(i)
    }

    fn pairs(&self, p: &Parser<'_>, opener: usize, closer: usize) -> bool {
        p.alias_closers.get(&closer) == p.alias_openers.get(&opener)
    }
}

/// [`Parser::environment_escapes_group`]'s policy, and the driver's first
/// *demotion* gate: it asks whether a `\begin` is cut short by the closing brace
/// of a group it sits inside, so a located "closer" is the escaping `}` and the
/// verdict reads inverted — `Some` demotes the environment, `None` keeps it.
///
/// Three divergences from the positive gates, all following from that
/// inversion:
///
/// - **The stray `}` closes rather than refutes** ([`StrayBrace::ClosesInGroup`]).
///   Same token event, opposite verdict.
/// - **Math does not refuse** ([`MathAnchor::None`]). For a positive
///   gate, declining behind a math delimiter is the conservative direction; for
///   this one it would *keep* an environment the scan cannot vouch for. The
///   pre-batch scan had no math anchor, and this preserves that.
/// - **Openers are `\begin`s** ([`GatePolicy::OPENER_IS_ENV_BEGIN`]), so the
///   driver counts one in `envs` before pushing its entry.
///
/// `\end` is deliberately *not* a closer here: it is the level anchor the
/// driver already applies (an `\end` not owed to an intervening `\begin` means
/// this environment ends before any group boundary, so it does not escape),
/// which leaves the mismatch recovery in [`Parser::finish_environment`]
/// untouched. Running out of file is likewise not an escape — that is what
/// keeps the unclosed-environment diagnostic firing on a forgotten `\end`.
struct EnvGate;

impl GatePolicy for EnvGate {
    const PARAGRAPH_ANCHOR: ParagraphAnchor = ParagraphAnchor::None;
    const STRAY_BRACE: StrayBrace = StrayBrace::ClosesInGroup;
    const MATH_ANCHOR: MathAnchor = MathAnchor::None;
    const OPENER_IS_ENV_BEGIN: bool = true;

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

/// [`Parser::delim_math_closes`]'s policy: `\[`/`\(` paired with its own
/// `\]`/`\)` — and, with [`DollarGate`], one of the two **math** gates, which
/// decide whether a delimiter opens math at all. What follows is what the two
/// share; [`DollarGate`] adds its own wrinkle.
///
/// Both are **single-entry**: [`GatePolicy::opens_at`] is always false, so a
/// batch settles its seed and nothing else. Their openers do not nest the way a
/// conditional does — the parse they guard consumes the whole body, so an opener
/// inside a math body is never re-gated, and an opener that is *not* inside one
/// is answered by the first closer at its own level whatever came before it.
/// With no LIFO to model there is no closer scope to hook, and the driver runs
/// them for its bookkeeping alone.
///
/// Four divergences from the pairing gates, each stated as a policy rather than
/// averaged into the loop:
///
/// - **A `}` refutes unconditionally** ([`StrayBrace::RefutesAlways`]): the
///   guarded parses bail at any unbalanced `}`, group behind it or not.
/// - **Math is not an anchor** ([`MathAnchor::None`]) — a foreign
///   delimiter is ordinary content here, and for [`DollarGate`] the delimiter
///   *is* the closer.
/// - **Environments count at any depth** ([`GatePolicy::ANCHORS_AT_ANY_DEPTH`]).
/// - **The closer needs no environment balance**
///   ([`GatePolicy::CLOSER_NEEDS_ENV_BALANCE`]).
///
/// Both also read a `macrocode` frame as an ordinary environment
/// ([`GatePolicy::MACROCODE_FRAME_ANCHORS`]), as their pre-batch scans did.
///
/// The paragraph anchor stays on, and the driver applies it at the entry's own
/// level — `depth == 0 && envs == envs_at_push`, which for a lone seed is the
/// old `depth == 0 && envs == 0` of the pre-batch scans exactly.
struct DelimMathGate {
    /// The closer this opener wants, `\]` or `\)`. The gate is per-flavor: a
    /// `\)` never settles a `\[`, so each carries its own last-closer bound.
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

/// [`Parser::dollar_closes`]'s policy: `$`/`$$` paired with the next `$`/`$$` at
/// its own level. The math gates' shared policy is documented on
/// [`DelimMathGate`].
///
/// The one wrinkle beyond those: a display opener is two tokens, so the caller
/// seeds the driver at the *second* `$` (the driver scans from `seed + 1`)
/// rather than the driver growing a hook for it. That is also why the gate is
/// unmemoized ([`Parser::gate_verdict`]) — a demoted `$$` re-enters
/// [`Parser::element`] on its second `$`, which asks a *different* question
/// (`display: false`) about the very index the display query seeded.
struct DollarGate {
    /// Whether the opener is `$$`, in which case only a `$` whose successor is
    /// also a `$` closes it. A lone `$` inside display math is malformed but
    /// consumed, exactly as [`Parser::dollar_math`] consumes it.
    display: bool,
}

impl GatePolicy for DollarGate {
    const PARAGRAPH_ANCHOR: ParagraphAnchor = ParagraphAnchor::OwnLevel;
    const STRAY_BRACE: StrayBrace = StrayBrace::RefutesAlways;
    const MATH_ANCHOR: MathAnchor = MathAnchor::None;
    const ANCHORS_AT_ANY_DEPTH: bool = true;
    const CLOSER_NEEDS_ENV_BALANCE: bool = false;
    const MACROCODE_FRAME_ANCHORS: bool = false;

    /// The last `$` in the file. Vacuous as a bound in the shape that motivated
    /// it — a file of `$` openers ends at one — but the driver's contract is
    /// that a gate names the last index that could ever settle an entry, and
    /// this gate's closer is a `DOLLAR` like any other.
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

/// [`Parser::left_right_closes`]'s policy: a `\left` paired with the `\right`
/// that [`Parser::left_right`] would reach. The gate that lives *inside* math,
/// and the only one whose entries stack rather than count
/// ([`Nesting::Interleaved`]) — `\left`/`\right` pair by count wherever they
/// sit, so its scan reads one frame stack of `{`, `\begin`, and `\left` alike,
/// and any mismatch refuses.
///
/// The other three divergences follow from the same two facts:
///
/// - **Math *ending* anchors** ([`MathAnchor::Closing`]). A `\left` sits inside
///   a math body already, so `$`, `\]`, `\)` are the tokens that end it —
///   [`Parser::left_right`]'s own recovery anchors — while a `\[` in the way is
///   ordinary content.
/// - **A `}` refutes unconditionally** ([`StrayBrace::RefutesAlways`]), for the
///   same reason as the math gates: the parse it guards bails at any unbalanced
///   `}`, group behind it or not.
/// - **A `macrocode` frame is not an anchor**
///   ([`GatePolicy::MACROCODE_FRAME_ANCHORS`]), preserved from the pre-batch
///   scan, which counted `\begin{macrocode}` as an ordinary environment — the
///   reading the math gates keep too.
///
/// And the one thing this migration exists to consolidate: **`\left`/`\right`
/// recognition ignores `in_macro_code` on purpose** where the driver's own
/// `\begin`/`\end` counting does not. They are catcode-neutral math structure
/// that pairs by count no matter what, and [`Parser::left_right`] consumes them
/// unconditionally, so a definition body or a `macrocode` chunk — exactly where
/// package math like `$\left#2\right#4$` (delarray.dtx) or `$\left(…\right)$`
/// (ltmath.dtx's `\bordermatrix`) lives — must still be scanned for them.
/// Gating them out left the `\right` invisible, so the pair never opened and the
/// closer reported a spurious "`\right` without matching `\left`" that blocked
/// the whole file for the formatter (issue #95). As a policy that is two
/// predicates; as a hand-written scan it was a comment nothing enforced.
struct LeftRightGate;

impl GatePolicy for LeftRightGate {
    const PARAGRAPH_ANCHOR: ParagraphAnchor = ParagraphAnchor::OwnLevel;
    const STRAY_BRACE: StrayBrace = StrayBrace::RefutesAlways;
    const MATH_ANCHOR: MathAnchor = MathAnchor::Closing;
    const NESTING: Nesting = Nesting::Interleaved;
    const MACROCODE_FRAME_ANCHORS: bool = false;

    /// The last `\right` in the file. The recording ignores brace nesting, which
    /// only ever places the bound *past* the last viable closer. Without it a
    /// math body of `\left` openers with no `\right` kept the frame stack
    /// non-empty — so the blank-line anchor never fired — and scanned each opener
    /// to the math's end.
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

/// What the three **bracket** gates share (`TODO.md`, container stack C2.5).
/// Each asks whether the `]` closing a `[` is reachable before the token that
/// would make [`Parser::optional`] bail, in the mode the bracket sits in — text,
/// math, or a `macrocode` chunk.
///
/// Their nesting is what the migration turned out to be about. A per-opener
/// bracket scan counts the `]`s *owed* to the command-abutting `[`s it passes:
/// such a `[` is itself argument-shaped (or a `\left`/`\Big` delimiter) and will
/// claim the next `]` when parsed, so that `]` cannot also satisfy the outer one
/// (`\P[\gamma[0, \infty) \cap A = \emptyset]`, issue #55). That claim countdown
/// **is** the driver's nested-opener stack once an opener is defined as a
/// command-abutting `[` — closer matching is pure LIFO either way — so the
/// family needed no new nesting model, only the two anchors it reads
/// differently:
///
/// - **A `\begin`/`\end` refutes rather than counts** ([`EnvAnchor::Refutes`]),
///   in both halves: an optional never legitimately spans an environment, so
///   either half means a runaway `[`.
/// - **Both that anchor and the paragraph break are depth-blind**
///   ([`GatePolicy::ANCHORS_AT_ANY_DEPTH`], [`ParagraphAnchor::AnyDepth`]),
///   because `optional`'s own bail is: it bails wherever the cursor stands, and
///   a gate stricter *or* looser than the parse it guards is a bug.
///
/// A `}` refutes unconditionally ([`StrayBrace::RefutesAlways`]) for the reason
/// the math gates give: `optional` bails at any unbalanced `}`, group behind it
/// or not.
///
/// [`Parser::bracket_closes_in_text`]'s own policy is this and nothing else — it
/// is the text-mode gate, and macro code's freely-passed lone brackets
/// (`\@ifnextchar [\@xmpar\@ympar`, issue #60) are exactly what it exists to
/// leave alone.
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

/// [`Parser::bracket_closes_before_math_end`]'s policy: the bracket gate that
/// runs lexically inside math. The family's policy is documented on
/// [`TextBracketGate`]; three things are this gate's own, and all three are
/// **preserved** from its pre-batch scan rather than chosen (`TODO.md`,
/// container stack C2.5).
///
/// - **A `$` depends on the enclosing math's flavor**
///   ([`GatePolicy::dollar_anchor`]). Inside `\[…\]` it opens a genuine nested
///   inline region, so a balanced `$…$` in the bracket is
///   [`DollarAnchor::Transparent`]; inside `$…$` TeX cannot nest one, so the
///   first depth-0 `$` is *this* math's closer and refutes — without which a
///   missing `]` in dollar math (`$\mathcal{N}[\mathcal{S}$`, issue #99) scanned
///   into the following math and attached an optional that swallowed the closing
///   `$`. That flavor is walk state, so it rides [`WalkKey`].
/// - **`\]`/`\)` refute** ([`MathAnchor::Closing`]): inside math they mean the
///   bracket is not an argument at all, e.g. the open-interval `$]0;\num{0.5}[$`.
/// - **The `\begin`/`\end` anchor fires inside macro code too**
///   ([`GatePolicy::ENV_ANCHOR_IN_MACRO_CODE`]), and chunk-unmatched braces are
///   not plain tokens ([`GatePolicy::PLAIN_BRACES_ARE_TOKENS`]). Both make it
///   stricter than the [`Parser::optional`] bail it mirrors, in the direction
///   that only ever declines to attach.
///
/// This is also the one gate whose `macrocode` bound is not the C0 argument.
/// Every other gate's pre-batch scan already stopped at [`Parser::macrocode_end`];
/// this one ran to EOF, so the driver's frame bound is new to it, and "past the
/// last closer only refusals remain" is not why it is verdict-preserving. The
/// reason is the bullet above: the `\begin`/`\end` anchor here carries no
/// `in_macro_code` filter, and the token at `macrocode_end` is the frame's own
/// `\end{macrocode}`, which the anchor refuses at — so the pre-batch scan
/// stopped at exactly the index the bound now stops before.
struct MathBracketGate {
    /// Whether the innermost enclosing math is `$…$`/`$$…$$`
    /// ([`Parser::math_dollar`]).
    enclosing_is_dollar: bool,
}

impl GatePolicy for MathBracketGate {
    const PARAGRAPH_ANCHOR: ParagraphAnchor = ParagraphAnchor::AnyDepth;
    const STRAY_BRACE: StrayBrace = StrayBrace::RefutesAlways;
    const MATH_ANCHOR: MathAnchor = MathAnchor::Closing;
    const ANCHORS_AT_ANY_DEPTH: bool = true;
    const ENV_ANCHOR: EnvAnchor = EnvAnchor::Refutes;
    const ENV_ANCHOR_IN_MACRO_CODE: bool = true;
    const PLAIN_BRACES_ARE_TOKENS: bool = false;

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

/// [`Parser::bracket_closes_before_macrocode_end`]'s policy: inside a `macrocode`
/// chunk a `[` is an argument only when its `]` closes *within* the chunk, whose
/// frame is an absolute terminator. Two divergences from its two siblings, both
/// preserved from its pre-batch scan:
///
/// - **It is single-entry** ([`GatePolicy::opens_at`] always false): the scan
///   ran no claim countdown, so the first `]` at brace level settles the seed.
///   That is also why it is the one bracket gate the batch cannot make linear —
///   there is no neighbor to settle — and it runs unmemoized like the math
///   gates.
/// - **A doc margin or guard breaks the paragraph run**
///   ([`GatePolicy::DOC_TRIVIA_FLOATS`]), where every other gate floats it.
///
/// Its [`EnvAnchor`] can never fire: a chunk body sets `in_def_body`
/// ([`Parser::macrocode_body`]), so `in_macro_code` holds at every index the
/// scan can reach and the driver's filter takes the arm out — which is the
/// pre-batch scan's silence about `\begin`/`\end` restated as the reason for it,
/// rather than as an omission.
struct MacrocodeBracketGate;

impl GatePolicy for MacrocodeBracketGate {
    const PARAGRAPH_ANCHOR: ParagraphAnchor = ParagraphAnchor::AnyDepth;
    const STRAY_BRACE: StrayBrace = StrayBrace::RefutesAlways;
    const MATH_ANCHOR: MathAnchor = MathAnchor::None;
    const ANCHORS_AT_ANY_DEPTH: bool = true;
    const ENV_ANCHOR: EnvAnchor = EnvAnchor::Refutes;
    const DOC_TRIVIA_FLOATS: bool = false;

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

struct Parser<'t> {
    tokens: &'t [Token],
    /// User-defined verbatim constructs, consulted to route a verbatim environment to
    /// its raw-body branch (its body is already one `VERBATIM_BODY` token from the
    /// lexer; the grammar must not try to parse it structurally).
    ctx: &'t ParseCtx,
    /// `starts[i]` is the byte offset of token `i`; `starts[len]` is the total
    /// length. Used to give syntax errors byte ranges.
    starts: Vec<usize>,
    pos: usize,
    events: Vec<Event>,
    errors: Vec<SyntaxError>,
    /// Consecutive-peek budget for the stuck-loop guard ([`Self::step`]).
    /// `Cell` because the lookahead primitives that tick it are `&self`.
    steps: std::cell::Cell<u32>,
    /// The cursor position at the last [`Self::step`] tick; the budget resets
    /// whenever `pos` has advanced past it (i.e. real progress was made).
    last_step_pos: std::cell::Cell<usize>,
    /// One entry per lexically enclosing math body (`$…$`, `\[…\]`, `\(…\)`,
    /// math environments), innermost last, holding that level's *flavor*:
    /// `true` for a `$…$`/`$$…$$` (dollar-delimited) level, `false` for
    /// `\[…\]`/`\(…\)` and math environments.
    ///
    /// Its **depth** ([`Self::in_math`]) is read where the `math` routing flags
    /// threaded through the grammar are not enough: it *persists* into the
    /// text-mode body of an unknown environment nested inside math
    /// (`\[ … \begin{myaligned} … \]`), where the grammar can't verify the body
    /// is math but the enclosing delimiters are a static lexical fact, and
    /// optional-argument attachment uses it to treat a spaced `[` as content
    /// (see [`Self::attach_arguments`]).
    ///
    /// Its **last entry** ([`Self::enclosing_math_is_dollar`]) is read by
    /// [`Self::bracket_closes_before_math_end`]: inside dollar math a `$` is the
    /// closer (a boundary), whereas inside `\[…\]` a `$` opens a genuine nested
    /// inline region (`\inferrule*[right=$\Pi$-eq]`), so the two must be scanned
    /// differently.
    ///
    /// Not every `MATH` node pushes here: [`Self::left_right`] opens one without
    /// a push, because a `\left…\right` always sits inside math already.
    math_dollar: Vec<bool>,
    /// True while parsing the attached arguments of a definition-body command
    /// ([`is_definition_body_command`], issues #45/#55). Those groups are
    /// macro-code definition bodies that need not self-balance
    /// `\begin`/`\end`, so while set, `\begin`/`\end` parse as plain commands
    /// ([`Self::element`], [`Self::math_atom`]) and stop being bail anchors for
    /// an optional argument ([`Self::optional`]). Saved and restored around
    /// [`Self::attach_arguments`] in [`Self::command`], so it covers the whole
    /// definition subtree (nested groups included) and nothing after it.
    in_def_body: bool,
    /// Environment names whose `\begin` the brace-group gate demoted to a plain
    /// command ([`Self::environment_escapes_group`]). Their `\end` is then an
    /// orphan by construction — the gate removed its partner, not the author — so
    /// [`Self::end_orphans_a_demoted_begin`] demotes it in the same way instead of
    /// letting it unwind (and falsely un-close) every enclosing environment.
    demoted_envs: std::collections::HashSet<String>,
    /// Names of the environments open around the cursor, outermost first. Read
    /// only by [`Self::end_orphans_a_demoted_begin`], to tell an `\end` that
    /// really does close something from one whose `\begin` was demoted.
    open_envs: Vec<String>,
    /// Token index of the `{` opening each currently-open brace group, innermost
    /// last ([`Self::group`] / [`Self::math_group`]).
    ///
    /// Its **depth** ([`Self::in_group`]) is the `\end`-side twin of
    /// [`Self::environment_escapes_group`]: an `\end` reached inside a group has
    /// its `\begin` outside it, so it is macro code rather than a stray
    /// (`\StopEventually{\end{document}}`, issue #71).
    ///
    /// Its **last entry** is read by [`Self::doc_margin_exempt`] to tell a group
    /// the `.dtx` *documentation* layer opened itself from one stranded by the
    /// code layer.
    group_opens: Vec<usize>,
    /// Inside a `.dtx` `macrocode` body: the token index of the terminating
    /// frame `\end` (or `tokens.len()` when the frame is missing), pre-scanned
    /// by [`Self::macrocode_body`]. `None` outside a macrocode body. The frame
    /// is the *only* terminator of the chunk (docstrip is line-oriented), so
    /// [`Self::at_block_end`] and the bracket/optional guards read it to keep
    /// any construct from consuming past the frame.
    macrocode_end: Option<usize>,
    /// Brace tokens inside the current `macrocode` body with no match within
    /// the chunk. A `macrocode` chunk is macro code: a definition regularly
    /// opens a `{` in one chunk and closes it in a later one (`\def\foo#1{%` …
    /// frame … `bar}`), so an unmatched brace is an ordinary token — no
    /// `GROUP`, no unclosed/unmatched diagnostic. Matched pairs still parse as
    /// groups. Computed per chunk by [`Self::macrocode_body`].
    plain_braces: std::collections::HashSet<usize>,
    /// Bumped on every mutation of [`Self::plain_braces`], so a gate batch can
    /// key on the set without cloning it ([`WalkKey`]).
    plain_braces_version: u32,
    /// expl3 catcode-mode toggle tokens, ascending: `(token index, state after
    /// the toggle)`. The same fixed toggle set the lexer flips
    /// ([`expl_toggle`]), pre-scanned once so [`Self::in_expl_region`] is a
    /// binary search. An expl3 region is *code* — token lists pass
    /// `\begin`/`\end` around as data (`\tl_set:Nn { \begin{longtable} … }`,
    /// issue #60) — so inside one, `\begin`/`\end` parse as plain commands
    /// exactly as in a definition body ([`Self::plain_env`]). `.dtx` doc-margin
    /// lines are exempt: a region regularly spans macrocode chunks, and the
    /// doc-layer markup between them (`\begin{macro}`, the frames) must keep
    /// pairing.
    expl_toggles: Vec<(usize, bool)>,
    /// The `.dtx` doc-margin lines, as `(first DOC_MARGIN on the line, the line's
    /// terminating NEWLINE)`, ascending and disjoint. Pre-scanned once so
    /// [`Self::on_doc_margin_line`] is a binary search rather than a walk back to
    /// the previous newline. **Empty for every non-`.dtx` file** — only that lexer
    /// mode emits `DOC_MARGIN` — so the predicate costs nothing there.
    doc_margin_lines: Vec<(usize, usize)>,
    /// Token indices of the `CONTROL_WORD`s that are *live* conditional openers:
    /// `\if`-prefixed, not one of the brace-argument `if*` macros, and not
    /// sitting in an operand slot (`\newif\if@foo`, `\let\ifpdf\iftrue`) or an
    /// `\ifcsname` body. Pre-scanned once in [`Self::new`] because the
    /// operand-slot rule is a *running* state over the whole token stream, which
    /// the recursive-descent walk cannot carry, and because both
    /// [`Self::element`] and [`Self::conditional_pairs`] need the same verdict.
    ///
    /// Openers inside an expl3 region are excluded outright: in-region layout is
    /// the formatter's, owned through `semantic::expl3`'s statement segmentation
    /// (`AGENTS.md`, expl3 code formatting), and a `CONDITIONAL` node there would
    /// contend with it. The exclusion also keeps the `\else:`/`\or:`/`\fi:`
    /// spellings out of scope. Recognition itself is shared with the linter's
    /// `ConditionalIndex` ([`conditional::OpenerScan`]) so the two cannot drift.
    conditional_openers: std::collections::HashSet<usize>,
    /// Token indices of *environment-alias openers* — bare control words whose
    /// definition body is exactly `\begin{X}` — mapped to the target environment
    /// `X` (issue #109). Pre-scanned in [`Self::new`] for the same reason as
    /// [`Self::conditional_openers`]: the definee filter is a running state over
    /// the stream that the recursive walk cannot carry.
    ///
    /// That filter is load-bearing, not defensive, and it counts *slots* rather
    /// than testing a single word ([`definition_name_slots`]). [`Self::command`]
    /// sets `in_def_body` after a `\def`-family head only when the definee is a
    /// `CONTROL_SYMBOL`, so in `\def\bea{\begin{eqnarray}}` the definee `\bea`
    /// reaches [`Self::element`] as an ordinary sibling command at brace depth 0
    /// with `in_macro_code` false. Unfiltered, the dispatch fires on it, the scan
    /// finds `\def\eea`'s definee at the same depth, and the two *definition lines*
    /// pair into an `ENVIRONMENT` — lossless and silent, but layout is destroyed.
    /// `\let\oldbea\bea` is the same failure one slot over: the *source* operand
    /// is a mention, not a call, and left live it pairs with a later `\eea` and
    /// swallows the prose in between. The braced `\newcommand{\bea}{…}` form is
    /// covered by `in_def_body` instead. Expl3 regions are excluded outright, as
    /// for conditionals.
    alias_openers: std::collections::HashMap<usize, SmolStr>,
    /// The closer mirror of [`Self::alias_openers`] (`\end{X}` bodies).
    alias_closers: std::collections::HashMap<usize, SmolStr>,
    /// The largest index in [`Self::alias_closers`], or `None` when the file has
    /// no alias closer at all. [`Self::alias_closer`] can only ever return an
    /// index in that map, so this bounds its forward scan — which is what keeps
    /// a file of openers that never pair linear instead of quadratic.
    last_alias_closer: Option<usize>,
    /// The `last_alias_closer` treatment, generalized (`TODO.md`, container
    /// stack C0): each shape gate succeeds only at one closer token shape, so
    /// truncating its scan at the last occurrence of that shape is
    /// verdict-preserving — past it, every path is a refusal, whether by anchor
    /// or by running out of range — and a file with none refuses without
    /// scanning at all. Recording may *over*-approximate (a `\fi` inside an
    /// expl3 region, a `\right` inside a brace group): a bound only needs to be
    /// at or past the last index that could ever succeed.
    ///
    /// This one is the last `]`, bounding [`Self::bracket_closes_in_text`] and
    /// [`Self::bracket_closes_before_math_end`].
    last_r_bracket: Option<usize>,
    /// Last `\]` — bounds [`Self::delim_math_closes`] for a `\[` opener.
    last_display_math_closer: Option<usize>,
    /// Last `\)` — bounds [`Self::delim_math_closes`] for a `\(` opener.
    last_inline_math_closer: Option<usize>,
    /// Last `\right` — bounds [`Self::left_right_closes`].
    last_right: Option<usize>,
    /// Last `}` — bounds [`Self::environment_escapes_group`], whose only `true`
    /// is a `}` at depth 0. Rarely effective (every `\begin{…}` opener carries a
    /// `}` in its own name group, so this index usually sits near EOF), but
    /// sound and free; the gate's residual quadratic shape is recorded in
    /// `TODO.md` (container stack, C2).
    last_r_brace: Option<usize>,
    /// Last `\fi`-flavored flow word — bounds [`Self::conditional_closer`].
    last_fi: Option<usize>,
    /// Last `$` — bounds [`Self::dollar_closes`]. The weakest of these bounds by
    /// construction, since this gate's closer is its opener's own token kind: in
    /// the adversarial shape (a file of `$` openers) the last one *is* an
    /// opener, so the bound cuts nothing. Recorded anyway, because the driver's
    /// contract is that every gate names the last index that could settle an
    /// entry, and a file whose `$`s all sit before a long tail does get the cut.
    last_dollar: Option<usize>,
    /// The most recent [`ConditionalGate`] batch ([`Self::gate_batch`]),
    /// memoized with the walk state its scan read. A lookup hits only when
    /// that key matches the walk's current state *and* the queried opener was
    /// settled by the batch; anything else re-batches from the queried opener.
    /// One slot is all the reuse there is: [`Self::element`] queries each
    /// opener once, in ascending order, under a stable state between
    /// re-batches. `RefCell` because the gate is `&self` — the pattern the
    /// alias gate's pre-batch memo used, with a map of settled openers where
    /// that one kept a single verdict.
    conditional_batch: std::cell::RefCell<Option<GateBatch>>,
    /// Tokens visited by the shape-gate scans, summed over the whole parse. A
    /// measurement hook for the linearity regression tests in this file's
    /// `mod tests` — never a budget (`TODO.md` rejects scan budgets as
    /// hard-coded special cases). `Cell` because the gates take `&self`, and
    /// `cfg(test)` because the counter is pure measurement: ticking it once per
    /// scanned token is a real cost in the driver's hottest loop, paid for
    /// nothing in a release build.
    #[cfg(test)]
    scan_work: std::cell::Cell<usize>,
    /// The [`EnvGate`] twin of [`Self::conditional_batch`]. Its verdicts are
    /// the *scan's* alone: [`Self::environment_escapes_group`]'s per-opener
    /// pre-checks (the group depth, the `.dtx` doc-margin exemption) are applied
    /// at query time, so a batch entry never carries them.
    env_batch: std::cell::RefCell<Option<GateBatch>>,
    /// The [`AliasGate`] twin of [`Self::conditional_batch`]. Both
    /// [`Self::starts_block_env`] and the [`Self::element`] dispatch ask about
    /// the same opener at the same cursor position, so even before the batch
    /// settled its neighbors this slot was load-bearing: without it every
    /// opener paid for its walk twice.
    alias_batch: std::cell::RefCell<Option<GateBatch>>,
    /// The [`LeftRightGate`] twin of [`Self::conditional_batch`]. Its openers
    /// nest densely — a `\left` whose `\right` the walk cannot reach is retried
    /// as a plain command and every `\left` after it asked in turn — so the
    /// batch is what keeps a run of them from being quadratic.
    left_right_batch: std::cell::RefCell<Option<GateBatch>>,
    /// The [`TextBracketGate`] twin of [`Self::conditional_batch`]. A `[` the
    /// gate refuses stays an ordinary token the walk steps over, and the next
    /// command-abutting `[` is asked in turn, so a run of them re-scanned per
    /// opener before the batch.
    text_bracket_batch: std::cell::RefCell<Option<GateBatch>>,
    /// The [`MathBracketGate`] twin, keyed like the others — including on the
    /// enclosing math's flavor, which this gate alone reads ([`WalkKey`]).
    math_bracket_batch: std::cell::RefCell<Option<GateBatch>>,
    /// Token index of the alias closer bounding the environment body currently
    /// being parsed, if any. Saved and restored around the body in
    /// [`Self::alias_environment`]. An alias environment has no `\end{…}` to stop
    /// at, so this positional bound is what terminates it — read by
    /// [`Self::at_block_end`], [`Self::trivia_run_is_separator`], and
    /// [`Self::binding_run`].
    alias_end: Option<usize>,
}

impl<'t> Parser<'t> {
    fn new(tokens: &'t [Token], ctx: &'t ParseCtx) -> Self {
        let pre = PreScan::run(tokens, ctx);
        Self {
            tokens,
            ctx,
            starts: pre.starts,
            pos: 0,
            events: Vec::new(),
            steps: std::cell::Cell::new(0),
            last_step_pos: std::cell::Cell::new(0),
            errors: Vec::new(),
            math_dollar: Vec::new(),
            in_def_body: false,
            demoted_envs: std::collections::HashSet::new(),
            open_envs: Vec::new(),
            group_opens: Vec::new(),
            macrocode_end: None,
            plain_braces: std::collections::HashSet::new(),
            plain_braces_version: 0,
            expl_toggles: pre.expl_toggles,
            doc_margin_lines: pre.doc_margin_lines,
            conditional_openers: pre.conditional_openers,
            last_alias_closer: pre.alias_closers.keys().copied().max(),
            last_r_bracket: pre.last_r_bracket,
            last_display_math_closer: pre.last_display_math_closer,
            last_inline_math_closer: pre.last_inline_math_closer,
            last_right: pre.last_right,
            last_r_brace: pre.last_r_brace,
            last_fi: pre.last_fi,
            last_dollar: pre.last_dollar,
            conditional_batch: std::cell::RefCell::new(None),
            #[cfg(test)]
            scan_work: std::cell::Cell::new(0),
            alias_openers: pre.alias_openers,
            alias_closers: pre.alias_closers,
            alias_batch: std::cell::RefCell::new(None),
            env_batch: std::cell::RefCell::new(None),
            left_right_batch: std::cell::RefCell::new(None),
            text_bracket_batch: std::cell::RefCell::new(None),
            math_bracket_batch: std::cell::RefCell::new(None),
            alias_end: None,
        }
    }

    /// The conditional divider or closer at token `idx`, if any. Flow words are
    /// classified from the name alone — `\else`/`\or`/`\fi` are never anything
    /// else — but never inside an expl3 region, where the openers are excluded
    /// too (see [`Self::conditional_openers`]).
    ///
    /// Total in `idx`: `Parser::pos` is one past the last token at EOF, and
    /// [`Self::conditional`] asks about the cursor after its loop has run out of
    /// input, so an out-of-range index is "no flow word here", not a bug.
    fn conditional_flow_at(&self, idx: usize) -> Option<conditional::FlowWord> {
        let t = self.tokens.get(idx)?;
        if t.kind != SyntaxKind::CONTROL_WORD || self.in_expl_region(idx) {
            return None;
        }
        t.text.strip_prefix('\\').and_then(conditional::flow_word)
    }

    /// True when token `idx` sits inside an expl3 region (after an
    /// `\ExplSyntaxOn`/`\ProvidesExpl*` with no intervening `\ExplSyntaxOff`).
    /// The toggle token itself is outside its own region.
    fn in_expl_region(&self, idx: usize) -> bool {
        let n = self.expl_toggles.partition_point(|&(i, _)| i < idx);
        n > 0 && self.expl_toggles[n - 1].1
    }

    /// True when token `idx` lies on a `.dtx` doc-margin line (a `DOC_MARGIN`
    /// opens its physical line).
    ///
    /// Answered from the pre-scanned [`Self::doc_margin_lines`], the same posture
    /// as [`Self::in_expl_region`]. This used to walk back to the preceding
    /// `NEWLINE`, justified by doc lines being short and [`Self::in_macro_code`]
    /// reaching it only for a token already inside an expl3 region — but
    /// [`Self::doc_margin_exempt`] calls it *unconditionally*, and that runs from
    /// [`Self::environment_escapes_group`] and its `\end` mirror for every
    /// `\begin`/`\end` in the file. On a document written as one long line the
    /// walk is `O(line length)` per opener, so the pair was `O(N x line length)`.
    fn on_doc_margin_line(&self, idx: usize) -> bool {
        // The candidate is the last line whose margin opens strictly before
        // `idx`; the lines are disjoint, so no earlier one can reach. It reaches
        // when `idx` is still on it — at or before its terminating newline, which
        // is where the backward scan would have stopped.
        let n = self.doc_margin_lines.partition_point(|&(m, _)| m < idx);
        n > 0 && self.doc_margin_lines[n - 1].1 >= idx
    }

    /// Whether token `idx` is covered by the `.dtx` doc-margin exemption from the
    /// brace-group gates ([`Self::environment_escapes_group`] and its `\end`-side
    /// mirror): it sits on a documentation line *and* every group open around it
    /// was opened by the code layer.
    ///
    /// The exemption exists for braces the *code* layer stranded — a
    /// `\iffalse{\fi` editor-balance hack, a `` \char`{ `` constant, a
    /// catcode-swapped region — which keep a group open for the rest of the
    /// file and would otherwise unnest the whole doc layer behind them. A
    /// group the documentation layer opened itself is not stranded: it is right
    /// there on a doc line, so a `\begin`/`\end` inside it really is inside it
    /// and the gates apply as they do in code (theorem.dtx's
    /// `% \def\deflist#1{\begin{list}…}` / `% \def\enddeflist{\end{list}}`
    /// split definition, issue #71).
    fn doc_margin_exempt(&self, idx: usize) -> bool {
        self.on_doc_margin_line(idx)
            && !self
                .group_opens
                .last()
                .is_some_and(|&brace| self.on_doc_margin_line(brace))
    }

    /// Whether the `\end` at `idx` is the orphaned partner of a `\begin` the
    /// brace-group gate demoted: its name was gated somewhere earlier
    /// ([`Self::demoted_envs`]) and no environment of that name is open here.
    ///
    /// The gate turns a `\begin` into a plain command, and a lone `\end` then
    /// unwinds every enclosing environment on its way to the root — one gated
    /// `\begin` inside a `\lowercase{…}` group un-closes the whole `document`
    /// (amsldoc.tex, issue #71). Demoting the `\end` too keeps the gate's two
    /// halves consistent. A genuine typo (`\end{itemiz}`) is untouched: nothing
    /// demoted that name, so it stays a stray `\end`.
    fn end_orphans_a_demoted_begin(&self, idx: usize) -> bool {
        if self.demoted_envs.is_empty() {
            return false;
        }
        peek_end_name(self.tokens, idx).is_some_and(|name| {
            self.demoted_envs.contains(name.as_ref())
                && !self.open_envs.iter().any(|open| open == name.as_ref())
        })
    }

    /// True when token `idx` sits in *macro code*: inside a definition body
    /// (issues #45/#55) or inside an expl3 region (issue #60; `.dtx` doc-margin
    /// lines exempt, see [`Self::expl_toggles`]). There `\begin`/`\end` are
    /// plain commands that need not pair, and an orphan `\]`/`\)` is data
    /// (`AGENTS.md` decision #1).
    fn in_macro_code(&self, idx: usize) -> bool {
        self.in_def_body || (self.in_expl_region(idx) && !self.on_doc_margin_line(idx))
    }

    /// True when the cursor sits lexically inside a math body — including inside
    /// a text-mode block (unknown environment, `\text{…}`-style group) nested in
    /// one. See the [`Self::math_dollar`] field.
    fn in_math(&self) -> bool {
        !self.math_dollar.is_empty()
    }

    /// True when at least one brace group is open around the cursor. See the
    /// [`Self::group_opens`] field.
    fn in_group(&self) -> bool {
        !self.group_opens.is_empty()
    }

    // --- cursor primitives -------------------------------------------------

    /// Tick the stuck-loop guard, called from every lookahead primitive. Resets
    /// the budget whenever the cursor has advanced since the last tick (real
    /// progress — via `bump` or the math-split fast path, both of which move
    /// `pos`), so the surviving count is the number of *consecutive* peeks with no
    /// token consumed. Exceeding [`PARSER_STEP_LIMIT`] means the parser is wedged
    /// in a non-advancing loop; abort loudly rather than hang. This can only fire
    /// on a grammar bug or pathological input, never on a real document, and the
    /// async callers (the language server's worker + read pool) already recover
    /// from a parse panic, degrading a wedged parse to a logged error.
    #[inline]
    fn step(&self) {
        if self.pos != self.last_step_pos.get() {
            self.last_step_pos.set(self.pos);
            self.steps.set(0);
        }
        let steps = self.steps.get();
        assert!(
            steps < PARSER_STEP_LIMIT,
            "parser exceeded {PARSER_STEP_LIMIT} peeks without consuming a token at position {} \
             — non-advancing loop",
            self.pos
        );
        self.steps.set(steps + 1);
    }

    fn kind(&self) -> Option<SyntaxKind> {
        self.step();
        self.tokens.get(self.pos).map(|t| t.kind)
    }

    fn nth_kind(&self, n: usize) -> Option<SyntaxKind> {
        self.step();
        self.tokens.get(self.pos + n).map(|t| t.kind)
    }

    fn text(&self) -> &str {
        self.tokens
            .get(self.pos)
            .map(|t| t.text.as_str())
            .unwrap_or("")
    }

    fn at_end(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    fn at_command(&self, name: &str) -> bool {
        self.kind() == Some(SyntaxKind::CONTROL_WORD) && self.text() == name
    }

    /// True if the `\begin`/`\end` at token index `pos` reads as a LaTeX
    /// environment delimiter: a `{` follows across trivia, without crossing a
    /// blank line, and the name inside is name-shaped. Macro code uses the
    /// bare TeX primitive and delimiter patterns (`\let\end\@@end`,
    /// `\long\def\@gobble@nv#1\end#2{…}`, `\expandafter\end`, xparse's
    /// `\begin \end {#3}` argument data — issue #60) at least as often as
    /// prose omits the brace by mistake, so a brace-less `\begin`/`\end` is a
    /// plain command everywhere: no environment, no diagnostic, and no
    /// recovery anchor. Likewise a name group holding a parameter or control
    /// word (`\end{#2}`, `\edef…{\noexpand\end{\reserved@a}}`) is computed
    /// macro data — statically unpairable — so it too stays a plain command
    /// (the group attaches as an ordinary argument).
    fn env_name_follows(&self, pos: usize) -> bool {
        let s = self.scan_trivia(pos + 1, CommentMode::Skip);
        if s.saw_blank_line || s.next_kind != Some(SyntaxKind::L_BRACE) {
            return false;
        }
        // Scan the name up to the closing `}` on the same line: a parameter
        // (`#`), a control word/symbol, or a nested `{` before it is macro
        // data, not a name. An *unterminated* name (line end or EOF first) is
        // an in-progress edit — stay optimistic so `\begin{ali` still parses
        // as a `BEGIN` + `NAME_GROUP` and environment-name completion sees it.
        for t in &self.tokens[s.next + 1..] {
            match t.kind {
                SyntaxKind::R_BRACE | SyntaxKind::NEWLINE => return true,
                SyntaxKind::HASH
                | SyntaxKind::CONTROL_WORD
                | SyntaxKind::CONTROL_SYMBOL
                | SyntaxKind::L_BRACE => return false,
                _ => {}
            }
        }
        true
    }

    /// The cursor is on a `\begin` that reads as an environment delimiter
    /// ([`Self::env_name_follows`]).
    fn at_env_begin(&self) -> bool {
        self.at_command(BEGIN_CMD) && self.env_name_follows(self.pos)
    }

    /// The `\end` twin of [`Self::at_env_begin`].
    fn at_env_end(&self) -> bool {
        self.at_command(END_CMD) && self.env_name_follows(self.pos)
    }

    /// [`Self::at_env_begin`] at an explicit index.
    ///
    /// Deliberately *not* routed through [`Self::at_command`]: that ticks the
    /// stuck-loop budget ([`Self::step`]), which is a peek counter for the walk,
    /// and this form is called once per token from inside the gate scans — where
    /// a visit is progress, not a non-advancing peek. Indexes directly, as every
    /// call site did before.
    fn env_begin_at(&self, idx: usize) -> bool {
        self.tokens[idx].text == BEGIN_CMD && self.env_name_follows(idx)
    }

    /// The `\end` twin of [`Self::env_begin_at`], with the same no-tick rule.
    fn env_end_at(&self, idx: usize) -> bool {
        self.tokens[idx].text == END_CMD && self.env_name_follows(idx)
    }

    // --- event emission ----------------------------------------------------

    fn bump(&mut self) {
        debug_assert!(!self.at_end(), "bump past end of input");
        self.events.push(Event::Tok(self.pos));
        self.pos += 1;
    }

    fn open(&mut self, kind: SyntaxKind) {
        self.events.push(Event::Start(kind));
    }

    fn close(&mut self) {
        self.events.push(Event::Finish);
    }

    /// Open a node *retroactively*, wrapping everything emitted since
    /// `checkpoint` — the event-stream analog of rust-analyzer's
    /// `Marker::precede`, done locally without a marker type. The caller still
    /// owes the matching [`Self::close`]; [`debug_assert_balanced`] catches it
    /// if not.
    ///
    /// Used where a construct can only be classified *after* parsing it: a
    /// `PARAGRAPH` (whether the run held a lone block environment), a `SCRIPTED`
    /// (whether a `^`/`_` followed the base atom).
    fn precede(&mut self, checkpoint: usize, kind: SyntaxKind) {
        self.events.insert(checkpoint, Event::Start(kind));
    }

    /// Move the `Start` already sitting at `at` back to `checkpoint`, so the
    /// node it opens also covers everything emitted between them. Both its kind
    /// and its `Finish` are the ones already in the stream, so the node's extent
    /// grows and nothing else changes.
    ///
    /// Used to pull a construct's own node back over the `DOC_COMMENT` bound in
    /// front of it ([`Self::doc_comment_bind`]): the construct self-opens, and
    /// only then is its kind known.
    fn extend_back(&mut self, checkpoint: usize, at: usize) {
        debug_assert!(checkpoint <= at, "extend_back must move a Start backwards");
        if let Event::Start(kind) = self.events[at] {
            self.events.remove(at);
            self.events.insert(checkpoint, Event::Start(kind));
        }
    }

    fn error(&mut self, message: impl Into<String>) {
        let (start, end) = if self.at_end() {
            let end = *self.starts.last().expect("starts is non-empty");
            (end, end)
        } else {
            (self.starts[self.pos], self.starts[self.pos + 1])
        };
        self.errors.push(SyntaxError {
            message: message.into(),
            start,
            end,
        });
    }

    /// Report an error at an explicit byte range. Used for *unclosed*-delimiter
    /// errors, which are detected at the closing anchor (a recovery token or EOF)
    /// but belong on the *opener* (`{`, `$`, `\[`, `\left`, `\begin{…}`)—the
    /// token the reader must fix. Pointing them at the detection site would land
    /// every unclosed error on EOF (a zero-width span at end of file).
    fn error_at(&mut self, range: (usize, usize), message: impl Into<String>) {
        self.errors.push(SyntaxError {
            message: message.into(),
            start: range.0,
            end: range.1,
        });
    }

    /// Byte range of the token at `pos` (`[starts[pos], starts[pos + 1])`).
    /// Captured at a construct's opener before it is consumed, so an unclosed
    /// error can point back at it (see [`Self::error_at`]).
    fn token_span(&self, pos: usize) -> (usize, usize) {
        (self.starts[pos], self.starts[pos + 1])
    }

    // --- grammar -----------------------------------------------------------

    fn document(&mut self) {
        self.parse_block(Block::Document);
    }

    /// Whether the construct at token `idx` opens a *block* environment — one
    /// [`parse_block`](Self::parse_block) leaves bare rather than wrapping in a
    /// `PARAGRAPH`. Block-ness is read from the built-in signature DB
    /// ([`is_block_environment`]), never from a name list here.
    ///
    /// Covers both spellings, so an alias formats like the environment it stands
    /// for: `\bea … \eea` must not be wrapped in a `PARAGRAPH` when the identical
    /// `\begin{eqnarray} … \end{eqnarray}` is not. The alias arm re-runs the shape
    /// gate, since a demoted opener is a plain command and must keep its paragraph.
    fn starts_block_env(&self, idx: usize) -> bool {
        if self.tokens.get(idx).is_some_and(|t| t.text == BEGIN_CMD) {
            return peek_begin_name(self.tokens, idx)
                .as_deref()
                .is_some_and(is_block_environment);
        }
        self.alias_openers
            .get(&idx)
            .is_some_and(|target| is_block_environment(target) && self.alias_closer(idx).is_some())
    }

    /// Consume a leading comment-bind located by [`Self::binding_run`]: float
    /// the trivia before `comment_start`, group the bound `%` run into a
    /// `DOC_COMMENT` node, parse the construct at `construct_pos`, and extend
    /// the construct's own node back over the comments
    /// ([`Self::extend_back`] — the construct self-opens, so its kind is only
    /// known afterwards).
    ///
    /// The bound run becomes a named node rather than bare leaves — the
    /// named-trivia enrichment `AGENTS.md` #9 reserved — so downstream
    /// (LSP/formatter) sees the doc comment as one unit.
    ///
    /// Shared by [`Self::parse_block`] and [`Self::conditional`], which differ
    /// only in what they check *before* calling (a conditional divider is not
    /// documentable) and what they track *after*.
    fn doc_comment_bind(&mut self, comment_start: usize, construct_pos: usize) {
        while self.pos < comment_start {
            self.bump();
        }
        let checkpoint = self.events.len();
        self.open(SyntaxKind::DOC_COMMENT);
        while self.pos < construct_pos {
            self.bump();
        }
        self.close();
        let construct_start = self.events.len();
        self.element();
        self.extend_back(checkpoint, construct_start);
    }

    /// Parse a content region, grouping runs of content into `PARAGRAPH` nodes
    /// delimited by blank lines (the TeX `\par` boundary). Blank-line trivia
    /// (and any trailing trivia) sits between paragraphs as direct children of
    /// the enclosing node, not inside a paragraph.
    fn parse_block(&mut self, block: Block) {
        loop {
            if self.at_block_end(block) {
                break;
            }
            // Separator trivia (blank lines / trailing whitespace) is emitted
            // directly, never wrapped in a paragraph — except a trailing own-line
            // comment run that binds into the construct after it: stop before that
            // comment so the construct (next iteration) absorbs it as leading.
            if self.kind().is_some_and(Self::is_trivia) && self.trivia_run_is_separator(block) {
                let stop = self
                    .binding_run(self.pos)
                    .map_or(self.tokens.len(), |(comment_start, ..)| comment_start);
                while self.pos < stop && self.kind().is_some_and(Self::is_trivia) {
                    self.bump();
                }
                continue;
            }
            // Otherwise we're at paragraph content (guaranteed ≥1 token, so no
            // empty paragraph and no infinite loop). Parse the run first, then
            // splice in the `PARAGRAPH` wrapper afterwards (the `precede` idiom,
            // cf. `math_scripted`) — unless the run's only non-trivia element is a
            // lone block environment, which we leave bare. Block-ness is read from
            // the built-in signature DB (`is_block_environment`).
            let checkpoint = self.events.len();
            let mut nontrivia_count = 0usize;
            let mut lone_block_env = false;
            loop {
                if self.at_block_end(block) {
                    break;
                }
                if self.kind().is_some_and(Self::is_trivia) && self.trivia_run_is_separator(block) {
                    break;
                }
                // Leading comment-bind: an own-line `%` run immediately before a
                // documentable construct attaches *leading* into it (see
                // `doc_comment_bind`). Block-ness is peeked from the construct's
                // index, so it reads the same before or after the bind.
                if let Some((comment_start, construct_pos, _)) = self.binding_run(self.pos) {
                    let starts_block_env = self.starts_block_env(construct_pos);
                    self.doc_comment_bind(comment_start, construct_pos);
                    nontrivia_count += 1;
                    lone_block_env = nontrivia_count == 1 && starts_block_env;
                    continue;
                }
                let is_nontrivia = !self.kind().is_some_and(Self::is_trivia);
                // Peek block-env status *before* consuming (the name is only
                // available while still on the `\begin`).
                let starts_block_env = self.starts_block_env(self.pos);
                self.element();
                if is_nontrivia {
                    nontrivia_count += 1;
                    lone_block_env = nontrivia_count == 1 && starts_block_env;
                }
            }
            if !lone_block_env {
                self.precede(checkpoint, SyntaxKind::PARAGRAPH);
                self.close(); // matching Finish for PARAGRAPH
            }
        }
    }

    fn at_block_end(&self, block: Block) -> bool {
        self.at_end()
            || match block {
                Block::Document => false,
                Block::Environment => {
                    // An alias environment has no `\end{…}`: its body ends at the
                    // closer the gate located. Checked first because
                    // `math_environment_body` hardcodes `Block::Environment`, so
                    // this one bound terminates both the math and the prose body.
                    self.alias_end.is_some_and(|end| self.pos >= end)
                        || (self.at_env_end() && !self.end_orphans_a_demoted_begin(self.pos))
                }
                // `>=` (not `==`): defensive against an element overshooting the
                // pre-scanned terminator, so the loop still stops.
                Block::Macrocode => self.macrocode_end.is_some_and(|end| self.pos >= end),
            }
    }

    /// True if the contiguous trivia run at the current position should separate
    /// paragraphs: it contains a blank line, or only trivia remains before the
    /// block terminator (the `\end`, or EOF).
    fn trivia_run_is_separator(&self, block: Block) -> bool {
        let s = self.scan_trivia(self.pos, CommentMode::Skip);
        if s.saw_blank_line {
            return true;
        }
        // A macrocode body ends positionally at the frame terminator; trivia
        // reaching it (the frame line's own margin and indent) is a separator.
        if block == Block::Macrocode {
            return s.next_kind.is_none() || self.macrocode_end.is_some_and(|end| s.next >= end);
        }
        match s.next_kind {
            // Only trivia remains before the block terminator (`\end`, or EOF).
            None => true,
            Some(SyntaxKind::CONTROL_WORD) => {
                block == Block::Environment
                    && (self.env_end_at(s.next)
                            // The alias twin: the run reaches the located closer.
                            || self.alias_end.is_some_and(|end| s.next >= end))
            }
            Some(_) => false,
        }
    }

    /// One element in text mode. Always consumes at least one token.
    fn element(&mut self) {
        let Some(k) = self.kind() else { return };
        match k {
            k if Self::is_trivia(k) => self.bump(),
            SyntaxKind::CONTROL_WORD => {
                // Inside a definition body or an expl3 region, `\begin`/`\end`
                // are plain commands: the two need not balance within one group
                // (issues #45/#60), so neither opens an environment nor is
                // stray. A brace-less `\begin`/`\end` is likewise a plain
                // command (`env_name_follows`).
                if !self.in_macro_code(self.pos) && self.at_env_begin() {
                    // Shape-gated like `\[`: an environment cannot outlive the
                    // brace group it opened in, so one whose `\end` is not
                    // reachable before that group closes is macro code — a
                    // plain command, no diagnostic (issue #71).
                    if self.environment_escapes_group(self.pos) {
                        if let Some(name) = peek_end_name(self.tokens, self.pos) {
                            self.demoted_envs.insert(name.into_owned());
                        }
                        self.command();
                    } else {
                        self.environment();
                    }
                } else if !self.in_macro_code(self.pos) && self.at_env_end() {
                    // The mirror case: reached inside a group, this `\end`'s
                    // `\begin` is outside it, so it is macro code rather than
                    // stray (`\StopEventually{\end{document}}`, issue #71).
                    if (self.in_group() && !self.doc_margin_exempt(self.pos))
                        || self.end_orphans_a_demoted_begin(self.pos)
                    {
                        self.command();
                    } else {
                        self.stray_end();
                    }
                } else if let Some((target, closer)) = (!self.in_macro_code(self.pos))
                    .then(|| {
                        let target = self.alias_openers.get(&self.pos)?.clone();
                        Some((target, self.alias_closer(self.pos)?))
                    })
                    .flatten()
                {
                    // A command whose definition body is exactly `\begin{X}`, whose
                    // partner is reachable: pair the two into an `ENVIRONMENT` of
                    // `X` (issue #109). Shape-gated like `\begin` and `\if`, and
                    // like them it demotes silently when the gate refuses.
                    self.alias_environment(&target, closer);
                } else if let Some(closer) = self
                    .conditional_openers
                    .contains(&self.pos)
                    .then(|| self.conditional_closer(self.pos))
                    .flatten()
                {
                    // Shape-gated like `\[` and `\begin`: an `\if` whose own
                    // `\fi` is not reachable is macro code — a plain command,
                    // no diagnostic ([`Self::conditional_closer`]).
                    self.conditional(closer);
                } else {
                    self.command();
                }
            }
            SyntaxKind::CONTROL_SYMBOL => {
                let sym = self.text().to_owned();
                match sym.as_str() {
                    // Shape-gated like `$` ([`Self::delim_math_closes`]): an
                    // opener with no reachable closer is macro-code data
                    // (`\expandafter\@tempa\[\@nil`, issue #65) — an ordinary
                    // token, no math, no diagnostic.
                    "\\[" => {
                        if self.delim_math_closes(self.pos, "\\]") {
                            self.delim_math(SyntaxKind::DISPLAY_MATH, "\\[", "\\]");
                        } else {
                            self.bump();
                        }
                    }
                    "\\(" => {
                        if self.delim_math_closes(self.pos, "\\)") {
                            self.delim_math(SyntaxKind::INLINE_MATH, "\\(", "\\)");
                        } else {
                            self.bump();
                        }
                    }
                    "\\]" | "\\)" => {
                        // In macro code (a definition body, macrocode chunk,
                        // or expl3 region) an orphan closer is data, not a
                        // stray delimiter (`\char_set_catcode_letter:N \)`,
                        // issue #60) — an ordinary token, no diagnostic. In
                        // prose it still diagnoses, catching a `\[…\]` typo'd
                        // across a paragraph break on its closer.
                        if !self.in_macro_code(self.pos) {
                            self.error(format!("unmatched `{sym}`"));
                        }
                        self.bump();
                    }
                    // `\\` line break, with its tightly-bound `*` / `[len]`.
                    "\\\\" => self.line_break(),
                    // Any other bare control symbol (`\,`, `\%`, `\;`, …). Surface
                    // model: emit as a token; these take no arguments.
                    _ => self.bump(),
                }
            }
            // A brace unmatched within a `macrocode` chunk is an ordinary macro-
            // code token (the definition it belongs to spans chunks): no `GROUP`,
            // no diagnostic.
            SyntaxKind::L_BRACE => {
                if self.plain_braces.contains(&self.pos) {
                    self.bump();
                } else {
                    self.group();
                }
            }
            SyntaxKind::R_BRACE => {
                if !self.plain_braces.contains(&self.pos) {
                    self.error("unmatched `}`");
                }
                self.bump();
            }
            SyntaxKind::DOLLAR => {
                let display = self.nth_kind(1) == Some(SyntaxKind::DOLLAR);
                if self.dollar_closes(self.pos, display) {
                    self.dollar_math();
                } else {
                    // No reachable closer: this dollar is macro-code data
                    // (`>{$}`, `{ $ }`), not a math delimiter — an ordinary
                    // token, no math, no diagnostic. Each `$` of an ungated
                    // `$$` re-enters here and is gated independently.
                    self.bump();
                }
            }
            // WORD, brackets, & # ^ _ ~, ERROR: ordinary tokens in text mode.
            _ => self.bump(),
        }
    }

    /// `\foo` followed by its greedily-attached argument groups.
    ///
    /// Arity is unknown without the semantic layer, so we attach every trailing
    /// `{…}` / `[…]` group (see `AGENTS.md`, Core decision #8, and
    /// [`Self::attach_arguments`] for the `[…]` shape gates). The one curated
    /// exception: a delimiter-size command (`\Big`, `\bigl`, …) never takes a
    /// `[…]` argument — its `[` is the delimiter it sizes (`\Big[ x \Big]`),
    /// mirroring the `\left`/`\right` special case.
    fn command(&mut self) {
        let bracket = if is_big_delimiter_command(self.text()) {
            BracketPolicy::Forbid
        } else {
            BracketPolicy::Greedy
        };
        // A definition-body command's attached groups are macro-code bodies
        // (issues #45/#55): flag them so `\begin`/`\end` inside parse as
        // plain commands. OR-ed with the saved flag so a definition nested in
        // another definition's body stays flagged; restored after the
        // arguments so following siblings are unaffected.
        let saved = self.in_def_body;
        self.in_def_body = saved || is_definition_body_command(self.text());
        let def_prefix = is_def_prefix_command(self.text());
        self.open(SyntaxKind::COMMAND);
        self.bump(); // the control word
        // A `\def`-family primitive's next token is the control sequence being
        // defined ([`is_def_prefix_command`]). A control-symbol name is
        // consumed here as a plain token so it is never misparsed as syntax
        // (`\def\[{…}` is not a math opener), and the attached body is then a
        // macro-code body: the stacks-project redefinition opens `trivlist` in
        // `\def\[`'s body and closes it in `\def\]`'s (issue #65), the same
        // no-balance fact as `is_definition_body_command`.
        if def_prefix {
            let scan = self.scan_trivia(self.pos, CommentMode::Skip);
            if scan.next_kind == Some(SyntaxKind::CONTROL_SYMBOL) && !scan.saw_blank_line {
                self.skip_trivia();
                self.bump(); // the defined name
                self.in_def_body = true;
            }
        }
        self.attach_arguments(bracket);
        self.in_def_body = saved;
        self.close();
    }

    /// The `\\` line break and its tightly-bound modifiers: an optional `*`
    /// (no-page-break variant) and an optional `[length]` (`\\`, `\\*`,
    /// `\\[2ex]`, `\\*[2ex]`). These bind to the `\\` only when they *directly*
    /// abut it — no intervening trivia is crossed — so a lone `\\` at end of line
    /// stays bare and the modifiers are never pulled across a break. Grouping
    /// them into one `LINE_BREAK` node (rather than leaving loose tokens) is what
    /// lets the formatter treat `\\[2ex]` as one unit instead of stranding the
    /// `[2ex]` on the next line.
    ///
    /// Unlike `command`, this attaches *no* `{…}` arguments (`\\` takes none) and
    /// does not skip trivia. The `*` is recognized only as its own `WORD` token
    /// (the lexer glues `*` into following letters, so `\\*foo` keeps the star on
    /// the word — a vanishingly rare form we deliberately leave alone).
    fn line_break(&mut self) {
        self.open(SyntaxKind::LINE_BREAK);
        self.bump(); // \\
        if self.kind() == Some(SyntaxKind::WORD) && self.text() == "*" {
            self.bump(); // *
        }
        if self.kind() == Some(SyntaxKind::L_BRACKET) {
            self.optional(); // [length]
        }
        self.close();
    }

    /// Greedily attach trailing `{…}` / `[…]` argument groups to the currently
    /// open node, allowing intervening trivia but stopping at a paragraph break.
    /// Shared by `\foo` commands and `\begin{env}` (see `AGENTS.md`, Core
    /// decision #8). Arity is unknown without the semantic layer.
    ///
    /// `[…]` attachment is additionally shape-gated (issue #43) — `[`/`]` are
    /// not real grouping in TeX, so a bracket is an argument only when it reads
    /// as one:
    /// - **Lexically inside math, only when it directly abuts.** Real math
    ///   optionals are written tight (`\sqrt[3]{x}`, `\\[2ex]`); a spaced `[`
    ///   is a delimiter or interval (`\bE [ x ]`). This uses [`Self::in_math`],
    ///   so it also covers text-mode bodies of unknown environments nested in
    ///   math (`\[ … \begin{myaligned} \Big [ … \]`).
    /// - **Inside math, only when [`Self::bracket_closes_before_math_end`]
    ///   finds its `]`**; otherwise it is left for the math loop as an ordinary
    ///   atom, so open-interval notation (`$]0;\num{0.5}[$`) does not swallow
    ///   the math closer as an optional-argument body.
    /// - **In text mode, only when [`Self::bracket_closes_in_text`] finds its
    ///   `]`** (issue #60): macro code tests for and re-emits lone brackets
    ///   (`\@ifnextchar [\@xmpar\@ympar`), so a `[` whose closer is not
    ///   reachable stays an ordinary token — no `OPTIONAL`, no diagnostic —
    ///   mirroring the `$` shape gate ([`Self::dollar_closes`]).
    /// - **Per the caller's [`BracketPolicy`]:** `Tight` (a curated math
    ///   environment's `\begin` — its math body starts right after, so a
    ///   detached `[` is content: `\begin{align}` + newline + `[a]_1`) demands
    ///   a directly-abutting `[` even outside math; `Forbid` (the
    ///   delimiter-size commands, [`Self::command`]) never attaches one.
    ///   `Greedy` — everything else — keeps decision #8's trivia-crossing
    ///   attachment, which the semantic layer legitimizes downstream (the
    ///   xparse-signature glue relies on a next-line `[Warning]` still
    ///   attaching to `\begin{note}`).
    fn attach_arguments(&mut self, bracket: BracketPolicy) {
        loop {
            let (next, paragraph_break) = self.peek_meaningful();
            if paragraph_break {
                break;
            }
            match next {
                Some(SyntaxKind::L_BRACE) => {
                    // A chunk-unmatched macrocode brace is a plain token, never
                    // an argument group (`\gdef\foo{%` … next chunk).
                    let scan = self.scan_trivia(self.pos, CommentMode::Skip);
                    if self.plain_braces.contains(&scan.next) {
                        break;
                    }
                    self.skip_trivia();
                    self.group();
                }
                Some(SyntaxKind::L_BRACKET) => {
                    if bracket == BracketPolicy::Forbid {
                        break;
                    }
                    let scan = self.scan_trivia(self.pos, CommentMode::Skip);
                    let tight_only = self.in_math() || bracket == BracketPolicy::Tight;
                    if tight_only && scan.next != self.pos {
                        break;
                    }
                    if self.in_math() && !self.bracket_closes_before_math_end(scan.next) {
                        break;
                    }
                    // In a macrocode body, a `[` is an argument only when its `]`
                    // closes inside the chunk: macro code uses bare brackets
                    // freely, and an optional must never consume the frame.
                    if self.macrocode_end.is_some()
                        && !self.bracket_closes_before_macrocode_end(scan.next)
                    {
                        break;
                    }
                    // In text mode, a `[` is an argument only when its `]` is
                    // reachable ([`Self::bracket_closes_in_text`]): macro code
                    // tests for and re-emits lone brackets at least as often as
                    // prose writes real optionals (`\@ifnextchar [\@xmpar\@ympar`,
                    // issue #60), so an unreachable closer means the bracket is
                    // data, not an argument.
                    if !self.in_math()
                        && self.macrocode_end.is_none()
                        && !self.bracket_closes_in_text(scan.next)
                    {
                        break;
                    }
                    self.skip_trivia();
                    self.optional();
                }
                // A verbatim-argument command's body (`\url{…}`, `\lstinline|…|`,
                // the final arg of `\mintinline{lang}{code}`) is lexed as a single
                // `VERB` token immediately following the command, so attach it as a
                // child like any other argument (decision #8) instead of leaving it
                // a sibling. A *standalone* `\verb…`/`\verb*…` token (its text starts
                // with `\`) is self-contained and belongs to no command — never
                // capture it. `lex_verbatim_command` emits its non-`\` `VERB`
                // *directly* after its own command tokens, so only a directly
                // abutting `VERB` attaches: a spaced one is a doc short-verb span
                // (`\emph{x} |y|`), a freestanding sibling that must keep its
                // interword space.
                Some(SyntaxKind::VERB)
                    if self.scan_trivia(self.pos, CommentMode::Skip).next == self.pos
                        && !self
                            .peek_meaningful_text()
                            .is_some_and(|t| t.starts_with('\\')) =>
                {
                    self.bump(); // the VERB argument
                }
                // A starred-variant marker `*` folds into the invocation so the
                // arguments that follow it still attach (`\section*{…}`,
                // `\inferrule*[…]`, `\\*[2pt]`).
                Some(SyntaxKind::WORD) if self.at_star_variant_marker() => {
                    self.bump(); // the `*`
                }
                _ => break,
            }
        }
    }

    /// Whether the next token is a *starred-variant marker* to fold into the
    /// command invocation: a lone `*` tight to the command, itself followed by
    /// an argument opener (`[`/`{`). LaTeX's `\@ifstar` commands carry the star
    /// before their arguments (`\section*{…}`, mathpartir's `\inferrule*[…]`,
    /// the `\\*[2pt]` line break), so folding it lets those arguments attach
    /// (decision #8) instead of the `*` breaking the run. Gating on a *following
    /// argument* keeps a math operator (`\pi*r`, `\Gamma * x`) — a `*` with no
    /// argument after it — from being mistaken for a marker. The `*` must be a
    /// lone token tight to the command: a spaced `\foo *` is not a marker, and
    /// `\foo*bar` lexes the star into a single `*bar` word (text ≠ `*`), so
    /// neither folds. Does not consume.
    fn at_star_variant_marker(&self) -> bool {
        if self.scan_trivia(self.pos, CommentMode::Skip).next != self.pos {
            return false; // the star must be tight to the command
        }
        if self.tokens.get(self.pos).map(|t| (t.kind, t.text.as_str()))
            != Some((SyntaxKind::WORD, "*"))
        {
            return false;
        }
        matches!(
            self.scan_trivia(self.pos + 1, CommentMode::Skip).next_kind,
            Some(SyntaxKind::L_BRACKET | SyntaxKind::L_BRACE)
        )
    }

    /// A brace group `{ … }`.
    fn group(&mut self) {
        debug_assert_eq!(self.kind(), Some(SyntaxKind::L_BRACE));
        let opener = self.token_span(self.pos);
        self.open(SyntaxKind::GROUP);
        self.bump(); // {
        self.group_opens.push(self.pos - 1);
        loop {
            match self.kind() {
                None => {
                    self.error_at(opener, "unclosed `{`");
                    break;
                }
                Some(SyntaxKind::R_BRACE) => {
                    self.bump();
                    break;
                }
                _ => self.element(),
            }
        }
        self.group_opens.pop();
        self.close();
    }

    /// An optional-argument group `[ … ]`.
    ///
    /// `[` and `]` are not real grouping in TeX, so this is heuristic: it ends
    /// at the first `]`, and bails defensively (rather than swallowing the
    /// document) on a `}`, a `\begin`/`\end`, a paragraph break, or EOF.
    fn optional(&mut self) {
        debug_assert_eq!(self.kind(), Some(SyntaxKind::L_BRACKET));
        let opener = self.token_span(self.pos);
        self.open(SyntaxKind::OPTIONAL);
        self.bump(); // [
        loop {
            match self.kind() {
                None | Some(SyntaxKind::R_BRACE) => {
                    self.error_at(opener, "unclosed `[`");
                    break;
                }
                Some(SyntaxKind::R_BRACKET) => {
                    self.bump();
                    break;
                }
                // In a definition body or expl3 region `\begin`/`\end` are
                // plain commands (issues #45/#60), so they don't signal a
                // runaway `[` — nor does a brace-less one (issue #60).
                Some(SyntaxKind::CONTROL_WORD)
                    if !self.in_macro_code(self.pos)
                        && (self.at_env_begin() || self.at_env_end()) =>
                {
                    self.error_at(opener, "unclosed `[`");
                    break;
                }
                _ => {
                    // The macrocode frame terminator is absolute: an optional
                    // still open there is abandoned, never consumes the frame.
                    if self.at_paragraph_break_outside_guards()
                        || self.macrocode_end.is_some_and(|end| self.pos >= end)
                    {
                        self.error_at(opener, "unclosed `[`");
                        break;
                    }
                    self.element();
                }
            }
        }
        self.close();
    }

    /// One tick per token a shape-gate scan visits, into [`Self::scan_work`].
    /// Compiled away outside `cfg(test)`: the linearity regression tests are
    /// the only reader, and [`Self::gate_batch`]'s loop is hot enough that an
    /// unconditional counter shows up in the parse benchmarks.
    #[cfg(test)]
    fn tick_scan(&self) {
        self.scan_work.set(self.scan_work.get() + 1);
    }

    #[cfg(not(test))]
    fn tick_scan(&self) {}

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
    fn bracket_closes_before_macrocode_end(&self, open: usize) -> bool {
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
    fn bracket_closes_before_math_end(&self, open: usize) -> bool {
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
    fn bracket_closes_in_text(&self, open: usize) -> bool {
        self.gated_closer(open, &TextBracketGate, &self.text_bracket_batch)
            .is_some()
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
    fn dollar_closes(&self, open: usize, display: bool) -> bool {
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
    fn delim_math_closes(&self, open: usize, closer: &'static str) -> bool {
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
    fn left_right_closes(&self, open: usize) -> bool {
        self.gated_closer(open, &LeftRightGate, &self.left_right_batch)
            .is_some()
    }

    /// Inline `$ … $` or display `$$ … $$` math. The body's atoms are wrapped in
    /// a `MATH` node (the delimiters stay direct children of the math node); the
    /// atoms themselves are parsed in math mode (see [`Self::math_element`]).
    /// Entry is gated by [`Self::dollar_closes`]: the caller has already
    /// verified a closer is reachable, so the unclosed-math recovery paths
    /// below fire only for shapes the gate scan cannot see (they remain as
    /// belt-and-braces recovery, never the expected path).
    fn dollar_math(&mut self) {
        let display = self.nth_kind(1) == Some(SyntaxKind::DOLLAR);
        let (kind, label) = if display {
            (SyntaxKind::DISPLAY_MATH, "$$")
        } else {
            (SyntaxKind::INLINE_MATH, "$")
        };
        let opener = (
            self.starts[self.pos],
            self.starts[self.pos + if display { 2 } else { 1 }],
        );
        self.open(kind);
        self.bump(); // $
        if display {
            self.bump(); // second $
        }
        self.open(SyntaxKind::MATH);
        self.math_dollar.push(true);
        loop {
            match self.kind() {
                None => {
                    self.error_at(opener, format!("unclosed `{label}`"));
                    break;
                }
                // `}` and `\end` are recovery anchors: `$`-math cannot span a
                // group or environment boundary, so a `}` here closes the
                // enclosing group (a math subgroup would have entered via `{`)
                // and a `\end` belongs to an enclosing environment. Leave the
                // token for the caller and report the unclosed math.
                Some(SyntaxKind::R_BRACE) => {
                    self.error_at(opener, format!("unclosed `{label}`"));
                    break;
                }
                Some(SyntaxKind::CONTROL_WORD) if self.at_env_end() => {
                    self.error_at(opener, format!("unclosed `{label}`"));
                    break;
                }
                Some(SyntaxKind::DOLLAR) => {
                    if display && self.nth_kind(1) != Some(SyntaxKind::DOLLAR) {
                        // A lone `$` inside `$$`: malformed; emit and continue.
                        self.bump();
                        continue;
                    }
                    // The closing delimiter belongs to the math node, not its
                    // body: break and bump it after closing `MATH`.
                    break;
                }
                _ => {
                    if self.at_paragraph_break() {
                        // Faithful to TeX: a blank line is a `\par`, and `\par`
                        // in math mode is "Missing $ inserted" — even inside an
                        // alignment cell (#35). Name the cause so the opener
                        // span isn't read as a bogus report.
                        self.error_at(
                            opener,
                            format!("unclosed `{label}` (a blank line ends math)"),
                        );
                        break;
                    }
                    self.math_element();
                }
            }
        }
        self.math_dollar.pop();
        self.close(); // MATH
        if self.kind() == Some(SyntaxKind::DOLLAR) {
            self.bump(); // closing $
            if display {
                self.bump(); // second closing $
            }
        }
        self.close(); // INLINE_MATH / DISPLAY_MATH
    }

    /// Delimited math: `\[ … \]` (display) or `\( … \)` (inline). As with
    /// [`Self::dollar_math`], the body's atoms are wrapped in a `MATH` node and
    /// parsed in math mode.
    fn delim_math(&mut self, kind: SyntaxKind, opener: &str, closer: &str) {
        let opener_span = self.token_span(self.pos);
        self.open(kind);
        self.bump(); // \[ or \(
        self.open(SyntaxKind::MATH);
        self.math_dollar.push(false);
        loop {
            match self.kind() {
                None => {
                    self.error_at(opener_span, format!("unclosed `{opener}`"));
                    break;
                }
                Some(SyntaxKind::CONTROL_SYMBOL) if self.text() == closer => {
                    // The closer belongs to the math node, not its body.
                    break;
                }
                // A `}` closes an enclosing group: it cannot belong to this
                // math (a subgroup would have entered via `{`). Leave it for
                // the caller and report the unclosed math.
                Some(SyntaxKind::R_BRACE) => {
                    self.error_at(opener_span, format!("unclosed `{opener}`"));
                    break;
                }
                Some(SyntaxKind::CONTROL_WORD) if self.at_env_end() => {
                    self.error_at(opener_span, format!("unclosed `{opener}`"));
                    break;
                }
                _ => {
                    if self.at_paragraph_break() {
                        // Same rationale as in `dollar_math`: `\par` ends math.
                        self.error_at(
                            opener_span,
                            format!("unclosed `{opener}` (a blank line ends math)"),
                        );
                        break;
                    }
                    self.math_element();
                }
            }
        }
        self.math_dollar.pop();
        self.close(); // MATH
        if self.kind() == Some(SyntaxKind::CONTROL_SYMBOL) && self.text() == closer {
            self.bump(); // \] or \)
        }
        self.close(); // INLINE_MATH / DISPLAY_MATH
    }

    /// One element inside a math body. Trivia is emitted inline (for
    /// losslessness); everything else is an atom, possibly carrying `^`/`_`
    /// scripts (see [`Self::math_scripted`]). Callers guard the math closers and
    /// recovery anchors before invoking this, so the cursor is at body content.
    fn math_element(&mut self) {
        match self.kind() {
            Some(k) if Self::is_trivia(k) => self.bump(),
            _ => self.math_scripted(),
        }
    }

    /// A base atom with any tightly-bound `^`/`_` scripts — the one sanctioned
    /// Pratt site (`AGENTS.md`, decision #3). Sub/superscripts are postfix with a
    /// single-atom right operand, so this is a base atom followed by a postfix
    /// loop, not full precedence climbing.
    ///
    /// We only wrap the base in a `SCRIPTED` node when a script actually
    /// attaches, so an unscripted atom stays a bare token/node (matching the
    /// `LINE_BREAK`-only-when-modifiers idiom). Because the base atom's extent is
    /// not known until parsed (a command greedily attaches its args), we parse it
    /// first and, if a script follows, retroactively splice a `SCRIPTED` start
    /// event in front of it — the event-stream analog of rust-analyzer's
    /// `precede`, done locally without touching the event layer.
    fn math_scripted(&mut self) {
        // A math `WORD` glued around operators (`a+2*1`) splits into separate
        // operand/operator atoms (`AGENTS.md`, decision #3). Only the trailing
        // piece is the scriptable base, so `a+2*1^5` binds `^5` to `1` (matching
        // TeX); the leading pieces are flat sibling atoms of the math body. This
        // is a byte-range split of the WORD's text, not a re-lex — see
        // [`split_math_word`].
        if self.kind() == Some(SyntaxKind::WORD)
            && let Some(pieces) = split_math_word(self.text())
        {
            let idx = self.pos;
            let (last, lead) = pieces.split_last().expect("split yields >= 2 pieces");
            for &(start, end) in lead {
                self.events.push(Event::SubTok { idx, start, end });
            }
            let checkpoint = self.events.len();
            self.events.push(Event::SubTok {
                idx,
                start: last.0,
                end: last.1,
            });
            self.pos += 1; // the whole WORD is consumed by its pieces
            self.math_scripts(checkpoint);
            return;
        }
        let checkpoint = self.events.len();
        self.math_atom();
        self.math_scripts(checkpoint);
    }

    /// Attach any `^`/`_` scripts that follow the base atom emitted since
    /// `checkpoint`, retro-splicing a `SCRIPTED` wrapper in front of it
    /// ([`Self::precede`]). No script → the base stays a bare atom.
    fn math_scripts(&mut self, checkpoint: usize) {
        if !self.at_script() {
            return; // bare atom, no wrapper
        }
        self.precede(checkpoint, SyntaxKind::SCRIPTED);
        while self.at_script() {
            self.skip_trivia(); // trivia between base/scripts rides inside SCRIPTED
            let sub = self.kind() == Some(SyntaxKind::UNDERSCORE);
            self.open(if sub {
                SyntaxKind::SUBSCRIPT
            } else {
                SyntaxKind::SUPERSCRIPT
            });
            self.bump(); // `_` or `^`
            self.math_script_arg();
            self.close();
        }
        self.close(); // SCRIPTED
    }

    /// True if a `^`/`_` script operator directly follows, skipping only
    /// `WHITESPACE`/`NEWLINE` (not a comment, which must end its line — so a
    /// script never binds across a comment) and not a blank line (a paragraph
    /// break ends the math).
    fn at_script(&self) -> bool {
        // `CommentMode::Stop`: a comment ends the line, so it stops the scan (and
        // is reported as the next meaningful token, which is not a script), rather
        // than being skipped as it is elsewhere. A blank line ends the math.
        let s = self.scan_trivia(self.pos, CommentMode::Stop);
        !s.saw_blank_line
            && matches!(
                s.next_kind,
                Some(SyntaxKind::CARET | SyntaxKind::UNDERSCORE)
            )
    }

    /// A single base atom: a `{…}` group (parsed in math mode), a command with
    /// its greedily-attached arguments, an environment, a `\\` line break, or one
    /// ordinary token. Always consumes at least one token.
    ///
    /// **Caller contract: the cursor must not be at EOF.** The `None` arm below
    /// consumes nothing and emits nothing, so a caller that reaches it from a
    /// loop spins until [`PARSER_STEP_LIMIT`] and panics far from the mistake.
    /// Every loop that reaches here guards EOF already — the four math bodies
    /// with an explicit `None` arm, `math_environment_body` through
    /// [`Self::at_block_end`], and [`Self::math_script_arg`] through its own
    /// missing-argument check — and this turns that unwritten contract into a
    /// tripwire that fires at the offending call instead.
    fn math_atom(&mut self) {
        debug_assert!(!self.at_end(), "math_atom at EOF: caller must guard first");
        match self.kind() {
            Some(SyntaxKind::L_BRACE) => self.math_group(),
            Some(SyntaxKind::CONTROL_WORD) => {
                // Same definition-body/expl3-region and brace-less gates as
                // [`Self::element`] (issues #45/#60).
                if !self.in_macro_code(self.pos) && self.at_env_begin() {
                    self.environment();
                } else if !self.in_macro_code(self.pos) && self.at_env_end() {
                    self.stray_end();
                } else if self.at_command(LEFT_CMD) && self.left_right_closes(self.pos) {
                    self.left_right();
                } else if self.at_command(RIGHT_CMD) {
                    self.stray_right();
                } else {
                    self.command();
                }
            }
            // `\\` line break (with its tightly-bound `*`/`[len]`) vs. a bare
            // control symbol (`\,`, `\;`, `\!`, spacing) — emit the latter as a
            // single token.
            Some(SyntaxKind::CONTROL_SYMBOL) if self.text() == "\\\\" => self.line_break(),
            // Any other single token (WORD, digit, `&`, `~`, `#`, brackets, a
            // bare control symbol, or a `^`/`_` with no base): one token, so the
            // loop always makes progress.
            Some(_) => self.bump(),
            // Ruled out by the caller contract above; kept because release
            // builds compile the assert away and the match must be total.
            None => {}
        }
    }

    /// One script argument: a single atom (a `{…}` group, a command with its
    /// args, or one token). A missing argument (the next meaningful token is a
    /// closer, `\end`, a paragraph break, or EOF) is reported, not consumed —
    /// the closer must stay for the enclosing math loop.
    fn math_script_arg(&mut self) {
        if self.at_paragraph_break() {
            self.error("missing argument after `^`/`_`");
            return;
        }
        self.skip_trivia();
        let missing = match self.kind() {
            None | Some(SyntaxKind::R_BRACE | SyntaxKind::DOLLAR) => true,
            Some(SyntaxKind::CONTROL_SYMBOL) => matches!(self.text(), "\\]" | "\\)"),
            Some(SyntaxKind::CONTROL_WORD) => self.at_env_end(),
            _ => false,
        };
        if missing {
            self.error("missing argument after `^`/`_`");
            return;
        }
        self.math_atom();
    }

    /// A brace group `{ … }` whose body is parsed in math mode (so `x^{a_b}`
    /// nests). Recovery mirrors [`Self::group`].
    fn math_group(&mut self) {
        debug_assert_eq!(self.kind(), Some(SyntaxKind::L_BRACE));
        let opener = self.token_span(self.pos);
        self.open(SyntaxKind::GROUP);
        self.bump(); // {
        self.group_opens.push(self.pos - 1);
        loop {
            match self.kind() {
                None => {
                    self.error_at(opener, "unclosed `{`");
                    break;
                }
                Some(SyntaxKind::R_BRACE) => {
                    self.bump();
                    break;
                }
                _ => self.math_element(),
            }
        }
        self.group_opens.pop();
        self.close();
    }

    /// A `\left<delim> … \right<delim>` matched delimiter pair (`AGENTS.md`,
    /// decision #3: the one precedence-climbing site — here just balanced
    /// matching by *count*, which is exactly how TeX pairs them, so a mismatched
    /// `\left( … \right]` still nests correctly). The `\left`/`\right` control
    /// words and their delimiter tokens are direct children (mirroring how `$` /
    /// `\[` delimiters stay direct children of the math node); the enclosed atoms
    /// are wrapped in a `MATH` body. Nested pairs recurse via [`Self::math_atom`].
    ///
    /// An unclosed `\left` recovers at the enclosing math/group/environment
    /// closer (the same anchors the surrounding math loop uses), leaving that
    /// token for the caller.
    fn left_right(&mut self) {
        debug_assert!(self.at_command(LEFT_CMD));
        let opener = self.token_span(self.pos);
        self.open(SyntaxKind::LEFT_RIGHT);
        self.bump(); // \left
        self.math_delim(LEFT_CMD);
        self.open(SyntaxKind::MATH);
        loop {
            match self.kind() {
                None => {
                    self.error_at(opener, "unclosed `\\left`");
                    break;
                }
                Some(SyntaxKind::CONTROL_WORD) if self.at_command(RIGHT_CMD) => break,
                // Enclosing-scope closers: `\left … \right` cannot span a group,
                // math, or environment boundary, so hand the token back.
                Some(SyntaxKind::R_BRACE | SyntaxKind::DOLLAR) => {
                    self.error_at(opener, "unclosed `\\left`");
                    break;
                }
                Some(SyntaxKind::CONTROL_SYMBOL) if matches!(self.text(), "\\]" | "\\)") => {
                    self.error_at(opener, "unclosed `\\left`");
                    break;
                }
                Some(SyntaxKind::CONTROL_WORD) if self.at_env_end() => {
                    self.error_at(opener, "unclosed `\\left`");
                    break;
                }
                _ => {
                    if self.at_paragraph_break() {
                        self.error_at(opener, "unclosed `\\left`");
                        break;
                    }
                    self.math_element();
                }
            }
        }
        self.close(); // MATH
        if self.at_command(RIGHT_CMD) {
            self.bump(); // \right
            self.math_delim(RIGHT_CMD);
        }
        self.close(); // LEFT_RIGHT
    }

    /// Consume the single delimiter token following `\left`/`\right`: skip inline
    /// trivia (it rides as a direct child of the pair for losslessness; the
    /// formatter drops it), then take one token. The lexer has already isolated a
    /// word-character delimiter (`(`, `|`, `.`, …) into its own token, so a single
    /// `bump` suffices. A missing delimiter — the next meaningful token is a
    /// closer, another `\left`/`\right`, `\end`, a paragraph break, or EOF — is
    /// reported, not consumed.
    fn math_delim(&mut self, after: &str) {
        self.skip_trivia();
        let missing = match self.kind() {
            None | Some(SyntaxKind::R_BRACE | SyntaxKind::DOLLAR) => true,
            Some(SyntaxKind::CONTROL_SYMBOL) => matches!(self.text(), "\\]" | "\\)"),
            Some(SyntaxKind::CONTROL_WORD) => {
                self.at_env_end() || self.at_command(LEFT_CMD) || self.at_command(RIGHT_CMD)
            }
            _ => false,
        };
        if missing {
            self.error(format!("missing delimiter after `{after}`"));
            return;
        }
        self.bump();
    }

    /// A `\right` with no open `\left` (the math loop only reaches one here when
    /// it is unmatched). Report it and consume it with its delimiter so the parse
    /// stays lossless and makes progress.
    fn stray_right(&mut self) {
        debug_assert!(self.at_command(RIGHT_CMD));
        self.error("`\\right` without matching `\\left`");
        self.bump(); // \right
        self.math_delim(RIGHT_CMD);
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
    fn environment_escapes_group(&self, open: usize) -> bool {
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
    fn conditional_closer(&self, open: usize) -> Option<usize> {
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
                SyntaxKind::WHITESPACE => {
                    i += 1;
                    continue;
                }
                SyntaxKind::DOC_MARGIN | SyntaxKind::GUARD if P::DOC_TRIVIA_FLOATS => {
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
                SyntaxKind::L_BRACE
                    if !P::PLAIN_BRACES_ARE_TOKENS || !self.plain_braces.contains(&i) =>
                {
                    depth += 1
                }
                SyntaxKind::R_BRACE
                    if !P::PLAIN_BRACES_ARE_TOKENS || !self.plain_braces.contains(&i) =>
                {
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
    fn alias_closer(&self, open: usize) -> Option<usize> {
        // Total in `open`: [`Self::starts_block_env`] asks about any index, and
        // the driver would otherwise seed an entry for a token that opens
        // nothing.
        self.alias_openers.get(&open)?;
        self.gated_closer(open, &AliasGate, &self.alias_batch)
    }

    /// `\bea … \eea`: an environment opened and closed by bare control words, for
    /// the closer [`Self::alias_closer`] located at token index `closer`.
    ///
    /// Emits the *same* `ENVIRONMENT > BEGIN … END` shape a spelled-out
    /// `\begin{X} … \end{X}` does, so every consumer downstream — the formatter's
    /// lowering, folding, the outline, [`crate::ast::Environment`] — works
    /// unchanged. The only difference is that `BEGIN`/`END` hold a bare
    /// `CONTROL_WORD` instead of `\begin` plus a `NAME_GROUP`, which is why
    /// [`crate::ast::Begin::name`] falls back to the head control word.
    ///
    /// No arguments are attached to either delimiter: the alias head consumes none
    /// (that is an admission rule of the scan, `semantic::define`), and attaching
    /// them from the *target's* signature would be arity-directed grouping from
    /// scanned data, which `AGENTS.md` decision #8 holds the line on.
    fn alias_environment(&mut self, target: &str, closer: usize) {
        self.open(SyntaxKind::ENVIRONMENT);
        self.open(SyntaxKind::BEGIN);
        self.bump(); // the opening control word
        self.close();

        let saved = self.alias_end.replace(closer);
        self.open_envs.push(target.to_owned());
        // Body routing reads the *target* name through the same curated-data-only
        // predicates a spelled-out environment uses, so no behavior flag ever comes
        // from the alias itself.
        if self.ctx.is_verbatim_environment(target) {
            self.verbatim_body(target);
        } else if is_math_environment(target) {
            self.math_environment_body();
        } else {
            self.parse_block(Block::Environment);
        }
        self.open_envs.pop();
        self.alias_end = saved;

        // The walk is bounded by `closer`, so it normally stops exactly there. It
        // may stop earlier when a nested construct re-gates and closes first — the
        // same one-directional guarantee `conditional_closer` documents — in which
        // case the closer stays a plain command and this environment simply has no
        // `END`, exactly as an unclosed `\begin` does.
        if self.pos == closer {
            self.open(SyntaxKind::END);
            self.bump();
            self.close();
        }
        self.close(); // ENVIRONMENT
    }

    /// `\if… … \else … \or … \fi`, for the closer [`Self::conditional_closer`]
    /// located at token index `closer`.
    ///
    /// The shape is a run of `CONDITIONAL_BRANCH` nodes closed by the `\fi` as
    /// the last child, mirroring `ENVIRONMENT > BEGIN … END`. The opener and its
    /// *test* ride the first branch rather than a head node of their own: the
    /// test's extent is not statically resolvable — `\ifnum\radius>5` scans
    /// ⟨number⟩⟨rel⟩⟨number⟩ by TeX's own scanner, `\ifx` takes two tokens, a
    /// `\newif`-defined `\if@foo` takes none — and inventing a boundary there
    /// would be the macro expansion the parser does not do.
    ///
    /// Every later branch *starts with* its divider, so a consumer finds the
    /// boundaries positionally and never by matching the name `\else`.
    fn conditional(&mut self, closer: usize) {
        self.open(SyntaxKind::CONDITIONAL);
        self.open(SyntaxKind::CONDITIONAL_BRANCH);
        self.command(); // the opener, with its usual greedy attachment
        loop {
            // The walk is bounded by the closer the gate located, so a nested
            // construct that consumes more than the token scan predicted can
            // never carry the conditional past it. Without the bound an
            // overrunning construct strands the cursor past `macrocode_end`, and
            // every chunk-bounded scan downstream then slices backwards.
            if self.pos >= closer || self.at_block_end(Block::Macrocode) {
                break;
            }
            match self.conditional_flow_at(self.pos) {
                Some(conditional::FlowWord::Fi) => break,
                Some(conditional::FlowWord::Else | conditional::FlowWord::Or) => {
                    self.close(); // CONDITIONAL_BRANCH
                    self.open(SyntaxKind::CONDITIONAL_BRANCH);
                    self.flow_command();
                    continue;
                }
                None => {}
            }
            // Leading comment-bind, as in [`Self::parse_block`]: an own-line `%`
            // run immediately before a documentable construct attaches *leading*
            // into it. A divider is not documentable, so a comment run before one
            // floats (the trivia falls through to `element` a token at a time and
            // the loop reaches the divider above).
            if let Some((comment_start, construct_pos, _)) = self.binding_run(self.pos)
                && self.conditional_flow_at(construct_pos).is_none()
            {
                self.doc_comment_bind(comment_start, construct_pos);
                continue;
            }
            self.element();
        }
        self.close(); // CONDITIONAL_BRANCH
        if self.conditional_flow_at(self.pos) == Some(conditional::FlowWord::Fi) {
            self.flow_command();
        }
        self.close(); // CONDITIONAL
    }

    /// A conditional divider or closer as a bare `COMMAND`, with **no** argument
    /// attachment.
    ///
    /// Inside a `CONDITIONAL` an `\else`/`\or`/`\fi` is a structural delimiter,
    /// parsed like `\end`, so a following group is the next branch's first
    /// element rather than the divider's argument. Greedy attachment is the
    /// text-pure default precisely because the text carries no arity protocol
    /// (`AGENTS.md` decision #8); here position in the construct *is* that
    /// protocol, and it is a static fact, so this is a sanctioned deviation on
    /// the same footing as the starred-variant fold.
    fn flow_command(&mut self) {
        self.open(SyntaxKind::COMMAND);
        self.bump();
        self.close();
    }

    /// `\begin{name} … \end{name}`, with environment-mismatch recovery.
    fn environment(&mut self) {
        self.open(SyntaxKind::ENVIRONMENT);

        let begin_pos = self.pos;
        let begin_start = self.starts[self.pos];
        self.open(SyntaxKind::BEGIN);
        self.bump(); // \begin
        let name = self.name_group();
        // Span of the opener `\begin{name}` (before any trailing arguments), so
        // an unclosed environment points back at the `\begin`, not at EOF.
        let opener = (begin_start, self.starts[self.pos]);
        // A frame-lexed `.dtx` macrocode `\begin` (it rides a `DOC_MARGIN`, so
        // this never fires on a stray `\begin{macrocode}` in a plain document).
        // The frame line holds nothing but the name (`lex_macrocode_frame`), so
        // it takes *no* arguments — the next line's `{` is body macro code, not
        // an attachment — and the body routes to `macrocode_body` below.
        let macrocode_frame = name
            .as_deref()
            .is_some_and(|n| matches!(n, "macrocode" | "macrocode*"))
            && self.frame_margin_before(begin_pos);
        // `\begin{tabular}{ll}`, `[options]`, etc. A curated math environment's
        // body starts right after its `\begin`, so only a directly-abutting
        // `[t]`-style optional attaches; a detached bracket is body content
        // (`\begin{align}` + newline + `[\partial_\mu V]_1`, issue #43).
        let bracket = if name.as_deref().is_some_and(is_math_environment) {
            BracketPolicy::Tight
        } else {
            BracketPolicy::Greedy
        };
        if !macrocode_frame {
            self.attach_arguments(bracket);
        }
        self.close(); // BEGIN

        if let Some(open) = name.as_deref() {
            self.open_envs.push(open.to_owned());
        }
        if name
            .as_deref()
            .is_some_and(|n| self.ctx.is_verbatim_environment(n))
        {
            self.verbatim_body(name.as_deref().expect("verbatim name"));
        } else if name.as_deref().is_some_and(is_math_environment) {
            self.math_environment_body();
        } else if macrocode_frame {
            // A frame-lexed macrocode body is macro code, not document
            // structure (see `macrocode_frame` above).
            self.macrocode_body(name.as_deref().expect("macrocode name"));
        } else {
            self.parse_block(Block::Environment);
        }
        if name.is_some() {
            self.open_envs.pop();
        }
        self.finish_environment(&name, opener);
    }

    /// True if the token at `pos` sits on a `.dtx` frame line: walking back over
    /// inline whitespace, the preceding token is a `DOC_MARGIN`. Margins never
    /// occur *inside* a macrocode body (code lines own their `%`), so this
    /// fingerprint distinguishes the frame `\begin`/`\end{macrocode}` from any
    /// look-alike in the code. Pinned by
    /// `macrocode_frame_margins_sit_where_the_formatter_expects` (`tests/dtx.rs`).
    fn frame_margin_before(&self, pos: usize) -> bool {
        let mut i = pos;
        while i > 0 {
            i -= 1;
            match self.tokens[i].kind {
                SyntaxKind::WHITESPACE => continue,
                SyntaxKind::DOC_MARGIN => return true,
                _ => return false,
            }
        }
        false
    }

    /// The body of a `.dtx` `macrocode`/`macrocode*` environment: macro code
    /// whose one true terminator is the frame line (`%    \end{macrocode}`),
    /// a line-oriented docstrip fact. TeX places no balance requirements on the
    /// chunk — a definition regularly opens a brace in one chunk and closes it
    /// several chunks later, and kernel code uses the `\end` primitive — so,
    /// like the definition bodies of decision #1 (issues #45/#55):
    /// - `\begin`/`\end` inside parse as plain commands ([`Self::in_def_body`]),
    /// - chunk-unmatched braces are plain tokens with no diagnostics
    ///   ([`Self::plain_braces`]; matched pairs still parse as `GROUP`s),
    /// - a `[` attaches as an optional only when it closes inside the chunk.
    ///
    /// The terminator is pre-scanned here (the first `\end` on a margin whose
    /// name matches — [`Self::frame_margin_before`]) and parsing stops
    /// positionally at it ([`Block::Macrocode`]); [`Self::finish_environment`]
    /// then consumes and name-checks it as usual. Nesting is impossible (the
    /// lexer never opens a frame inside a body), but state is saved/restored
    /// anyway so a malformed tree cannot leak it.
    fn macrocode_body(&mut self, name: &str) {
        let mut end = self.tokens.len();
        for i in self.pos..self.tokens.len() {
            if self.tokens[i].kind == SyntaxKind::CONTROL_WORD
                && self.tokens[i].text == END_CMD
                && self.frame_margin_before(i)
                && peek_end_name(self.tokens, i).as_deref() == Some(name)
            {
                end = i;
                break;
            }
        }

        let saved_plain = std::mem::take(&mut self.plain_braces);
        let saved_end = self.macrocode_end;
        let saved_def = self.in_def_body;

        let mut open_stack = Vec::new();
        for i in self.pos..end {
            match self.tokens[i].kind {
                SyntaxKind::L_BRACE => open_stack.push(i),
                SyntaxKind::R_BRACE if open_stack.pop().is_none() => {
                    self.plain_braces.insert(i);
                }
                _ => {}
            }
        }
        self.plain_braces.extend(open_stack);
        self.plain_braces_version += 1;
        self.macrocode_end = Some(end);
        self.in_def_body = true;

        self.parse_block(Block::Macrocode);

        self.plain_braces = saved_plain;
        self.plain_braces_version += 1;
        self.macrocode_end = saved_end;
        self.in_def_body = saved_def;
    }

    /// Consume the matching `\end`, or recover. `parse_block` / `verbatim_body`
    /// leave the cursor at a `\end` or at EOF.
    fn finish_environment(&mut self, name: &Option<String>, opener: (usize, usize)) {
        match self.kind() {
            None => {
                self.error_at(
                    opener,
                    format!("unclosed environment `{}`", name.as_deref().unwrap_or("")),
                );
            }
            // The cursor is at a `\end` (the only non-EOF stop condition).
            Some(_) => {
                let end_name = peek_end_name(self.tokens, self.pos);
                if name.is_none() || name.as_deref() == end_name.as_deref() {
                    // Matching \end: consume it as our END.
                    self.open(SyntaxKind::END);
                    self.bump(); // \end
                    self.name_group();
                    self.close();
                } else {
                    // Mismatched \end: it belongs to an enclosing environment.
                    // Close this one with a diagnostic and leave the \end for
                    // the caller (this unwinds the stack until some level
                    // matches, or it becomes a stray \end at the root).
                    self.error_at(
                        opener,
                        format!(
                            "unclosed environment `{}` (found `\\end{{{}}}`)",
                            name.as_deref().unwrap_or(""),
                            end_name.as_deref().unwrap_or("")
                        ),
                    );
                }
            }
        }
        self.close(); // ENVIRONMENT
    }

    /// The body of a named math environment (`equation`, `align`, `gather`, …): its
    /// atoms wrapped in a `MATH` node and parsed in math mode, exactly as `\[…\]`
    /// (see [`Self::delim_math`]) — so `^`/`_` build `SCRIPTED` nodes, the operator
    /// split fires, and `\left…\right` pair. Routed here for environments the
    /// built-in signature DB flags `math` ([`is_math_environment`]).
    ///
    /// The terminator is the matching `\end` (or EOF), read via [`Self::at_block_end`]
    /// just like [`Self::parse_block`]; [`Self::finish_environment`] then consumes and
    /// name-checks it. Unlike `$`-math (where a `\end` is an *unclosed*-recovery
    /// anchor), `\end` is the normal, expected terminator here. A blank line inside the
    /// body stays trivia within the `MATH` node — no paragraph split — so losslessness
    /// holds. Progress is guaranteed: [`Self::math_element`] bumps trivia or descends
    /// into [`Self::math_scripted`], whose atom parser always consumes a token.
    fn math_environment_body(&mut self) {
        self.open(SyntaxKind::MATH);
        self.math_dollar.push(false);
        while !self.at_block_end(Block::Environment) {
            self.math_element();
        }
        self.math_dollar.pop();
        self.close(); // MATH
    }

    /// The raw body of a verbatim-like environment: consume tokens unstructured
    /// until the matching `\end{name}`. The lexer has already collapsed the body
    /// into a single `VERBATIM_BODY` token; this loop also serves as a fallback.
    fn verbatim_body(&mut self, name: &str) {
        loop {
            match self.kind() {
                None => break,
                Some(SyntaxKind::CONTROL_WORD)
                    if self.at_command(END_CMD)
                        && peek_end_name(self.tokens, self.pos).as_deref() == Some(name) =>
                {
                    break;
                }
                _ => self.bump(),
            }
        }
    }

    /// A `\end` with no matching open environment at this level.
    fn stray_end(&mut self) {
        self.error("`\\end` without matching `\\begin`");
        self.open(SyntaxKind::END);
        self.bump(); // \end
        self.name_group();
        self.close();
    }

    /// The `{name}` group following `\begin` / `\end`. Returns the trimmed name.
    fn name_group(&mut self) -> Option<String> {
        self.skip_trivia();
        if self.kind() != Some(SyntaxKind::L_BRACE) {
            self.error("expected `{` for environment name");
            return None;
        }
        self.open(SyntaxKind::NAME_GROUP);
        self.bump(); // {
        let mut name = String::new();
        loop {
            match self.kind() {
                None => {
                    self.error("unclosed environment name");
                    break;
                }
                Some(SyntaxKind::R_BRACE) => {
                    self.bump();
                    break;
                }
                _ => {
                    name.push_str(self.text());
                    self.bump();
                }
            }
        }
        self.close();
        Some(name.trim().to_owned())
    }
}

/// Split a math `WORD`'s text at operator boundaries into `[start, end)` byte
/// ranges covering the whole text, or `None` when it holds no operator (a single
/// operand run needs no split). Operators are catcode-12 "other" characters that
/// glue into `WORD` (`a+2*1`); isolating them lets the math-aware parser and
/// formatter treat them as atoms (spacing, line breaks) without a catcode-carrying
/// lexer. The rule:
///
/// - `+ - * /`: each is its own single-char piece (so `2*-1` → `2`,`*`,`-`,`1`,
///   letting the formatter read a leading `-`/`+` as unary).
/// - `= < >`: a maximal run coalesces into one piece (`<=`, `>=`, `==` stay
///   together), but never merges with an adjacent sign (`=-` → `=`,`-`).
/// - anything else: a maximal operand run.
///
/// The pieces concatenate back to the input, preserving losslessness.
fn split_math_word(text: &str) -> Option<Vec<(usize, usize)>> {
    #[derive(PartialEq, Clone, Copy)]
    enum Cls {
        Operand,
        /// `+ - * /`: always its own single-char piece.
        Sign,
        /// `= < >`: coalescing relation run.
        Rel,
    }
    let classify = |c: char| match c {
        '+' | '-' | '*' | '/' => Cls::Sign,
        '=' | '<' | '>' => Cls::Rel,
        _ => Cls::Operand,
    };
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut prev: Option<Cls> = None;
    for (i, c) in text.char_indices() {
        let cls = classify(c);
        // Break before this char when the class changed, or when either side is a
        // sign (each sign stands alone). A same-class run (operand/operand or
        // rel/rel) coalesces.
        let boundary = prev.is_some_and(|p| p != cls || cls == Cls::Sign);
        if boundary {
            pieces.push((start, i));
            start = i;
        }
        prev = Some(cls);
    }
    pieces.push((start, text.len()));
    (pieces.len() >= 2).then_some(pieces)
}

/// Read the environment name from a `\begin{…}` at `begin_pos` without consuming.
/// Identical in shape to [`peek_end_name`] (skip the control word and trivia, then
/// read the `{name}` group); named separately for call-site clarity.
fn peek_begin_name(tokens: &[Token], begin_pos: usize) -> Option<Cow<'_, str>> {
    peek_end_name(tokens, begin_pos)
}

/// Read the environment name from a `\end{…}` at `end_pos` without consuming.
///
/// Borrows the token's own text for the single-token name every ordinary
/// environment has, and only allocates for one spelled across several tokens
/// (`\end{align *}`, a name holding a digit or a `-`). Three of the callers are
/// forward scans that ask once per token and only ever compare the result, so
/// the common case must not allocate.
fn peek_end_name(tokens: &[Token], end_pos: usize) -> Option<Cow<'_, str>> {
    let mut i = end_pos + 1; // past the \end control word
    while tokens.get(i).is_some_and(|t| Parser::is_trivia(t.kind)) {
        i += 1;
    }
    if tokens.get(i).map(|t| t.kind) != Some(SyntaxKind::L_BRACE) {
        return None;
    }
    i += 1;
    let start = i;
    while tokens.get(i).is_some_and(|t| t.kind != SyntaxKind::R_BRACE) {
        i += 1;
    }
    Some(match &tokens[start..i] {
        [] => Cow::Borrowed(""),
        [t] => Cow::Borrowed(t.text.trim()),
        many => {
            let mut name = String::new();
            for t in many {
                name.push_str(&t.text);
            }
            Cow::Owned(name.trim().to_owned())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::lexer::lex;

    /// The stuck-loop guard aborts once `PARSER_STEP_LIMIT` peeks accrue with no
    /// cursor advance, turning a hypothetical non-advancing loop into a loud
    /// panic instead of a hang.
    #[test]
    fn step_guard_trips_when_wedged() {
        let tokens = lex("x");
        let ctx = ParseCtx::default();
        let p = Parser::new(&tokens, &ctx);
        // Park one tick short of the ceiling with the cursor pinned, so no reset
        // fires on the next peeks.
        p.last_step_pos.set(p.pos);
        p.steps.set(PARSER_STEP_LIMIT - 1);
        p.step(); // reaches the ceiling exactly — still allowed
        let wedged = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| p.step()));
        assert!(wedged.is_err(), "the guard must abort a non-advancing loop");
    }

    /// A real cursor advance resets the budget, so an arbitrarily long *advancing*
    /// parse never trips the guard.
    #[test]
    fn step_budget_resets_on_cursor_progress() {
        let tokens = lex("xx");
        let ctx = ParseCtx::default();
        let mut p = Parser::new(&tokens, &ctx);
        p.last_step_pos.set(p.pos);
        p.steps.set(PARSER_STEP_LIMIT - 1);
        // Advance the cursor as a real consume would; the next peek must reset.
        p.pos += 1;
        p.step();
        assert_eq!(p.steps.get(), 1, "progress should reset the peek budget");
    }

    /// Drive a full parse and return how many tokens the shape-gate scans
    /// visited ([`Parser::scan_work`]).
    fn scan_work(input: &str) -> usize {
        let tokens = lex(input);
        let ctx = ParseCtx::default();
        let mut p = Parser::new(&tokens, &ctx);
        p.document();
        p.scan_work.get()
    }

    /// Doubling the input must not blow the gate-scan work up quadratically:
    /// `3·work(N)` leaves room for per-parse constants while a quadratic gate
    /// (ratio 4 per doubling) still fails loudly. The slack constant absorbs
    /// shapes whose bounded work is near zero.
    #[track_caller]
    fn assert_scan_work_linear(small: &str, doubled: &str) {
        let (w1, w2) = (scan_work(small), scan_work(doubled));
        assert!(
            w2 < 3 * w1 + 64,
            "gate-scan work grew superlinearly: {w1} -> {w2}"
        );
    }

    /// The adversarial no-closer shapes (`TODO.md`, container stack C0): a file
    /// of openers whose gate can never succeed must refuse via its absent
    /// last-closer bound instead of scanning forward per opener.
    #[test]
    fn gate_scans_stay_linear_without_closers() {
        // `conditional_closer`: `\if`-prefixed openers, no `\fi` anywhere.
        let shape = "\\ifabc x\n";
        assert_scan_work_linear(&shape.repeat(200), &shape.repeat(400));
        // `bracket_closes_in_text`: command-abutting `[`s, no `]` anywhere.
        let shape = "\\cmd[x\n";
        assert_scan_work_linear(&shape.repeat(200), &shape.repeat(400));
        // `delim_math_closes`: display-math openers, no `\]` anywhere.
        let shape = "\\[ x\n";
        assert_scan_work_linear(&shape.repeat(200), &shape.repeat(400));
    }

    /// The batched conditional gate (`TODO.md`, container stack C1): a run of
    /// same-frame openers whose lone `\fi` sits at EOF defeats the C0
    /// last-closer bound (the bound spans the whole file), so only the batch —
    /// one shared scan settling every opener it passes — keeps it linear.
    #[test]
    fn conditional_batch_keeps_shared_frame_openers_linear() {
        let body = |n: usize| format!("{}\\fi\n", "\\ifabc x\n".repeat(n));
        assert_scan_work_linear(&body(200), &body(400));
    }

    /// The alias twin of `conditional_batch_keeps_shared_frame_openers_linear`
    /// (`TODO.md`, container stack C2.1): a run of same-frame alias openers
    /// whose lone closer sits at EOF spans the whole file, so the C0
    /// last-closer bound cuts nothing and only the batch keeps it linear.
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

    /// The `\begin` gate's own shape (`TODO.md`, container stack C2.2): openers
    /// inside a group with no `}` of their own. Its C0 bound is the last `}` in
    /// the file, which every `\begin{…}` name group pushes toward EOF, so the
    /// bound cuts nothing here and only the batch keeps it linear.
    #[test]
    fn env_batch_keeps_shared_frame_openers_linear() {
        let body = |n: usize| format!("{{\n{}", "\\begin{itemize}\n".repeat(n));
        assert_scan_work_linear(&body(200), &body(400));
    }

    /// The `\left` gate's own one-closer-at-EOF shape (`TODO.md`, container
    /// stack C2.4): a run of same-frame openers inside one math body, closed by
    /// a single `\right` at the end. The C0 bound spans the whole run, and every
    /// opener but the last is demoted and re-queried as the walk passes it, so
    /// only the batch — one scan settling all of them — keeps it linear.
    #[test]
    fn left_right_batch_keeps_shared_frame_openers_linear() {
        let body = |n: usize| format!("$ {}\\right)$\n", "\\left( x ".repeat(n));
        assert_scan_work_linear(&body(200), &body(400));
    }

    /// The bracket family's one-closer-at-EOF shapes (`TODO.md`, container
    /// stack C2.5). A `[` the gate refuses stays an ordinary token the walk
    /// steps over, so the next command-abutting `[` is asked in turn and a run
    /// of them re-scanned per opener; the C0 last-`]` bound spans the whole run
    /// here, so only the batch — one scan whose nested-opener stack *is* the old
    /// claim countdown — keeps it linear.
    ///
    /// [`MacrocodeBracketGate`] has no such shape to pin: it is single-entry by
    /// policy (its pre-batch scan ran no countdown), so a chunk of `\cmd[`
    /// openers whose only `]` sits past the frame still scans to the frame per
    /// opener. Recorded rather than pinned, like the `${` ratchet above.
    #[test]
    fn bracket_batch_keeps_shared_frame_openers_linear() {
        // `bracket_closes_in_text`: command-abutting `[`s, one `]` at EOF.
        let body = |n: usize| format!("{}]\n", "\\cmd[x\n".repeat(n));
        assert_scan_work_linear(&body(200), &body(400));
        // `bracket_closes_before_math_end`: the same run inside one `$…$`.
        let body = |n: usize| format!("$ {}]$\n", "\\cmd[x ".repeat(n));
        assert_scan_work_linear(&body(200), &body(400));
    }

    /// The in-math no-closer shapes: the enclosing math's own gate passes (its
    /// closer is reachable), and every opener inside it must still refuse
    /// without a per-opener scan to the math's end.
    #[test]
    fn math_gate_scans_stay_linear_without_closers() {
        // `bracket_closes_before_math_end`: `\cmd[` atoms in dollar math, no
        // `]` anywhere.
        let body = |n: usize| format!("$ {}$", "\\cmd[x ".repeat(n));
        assert_scan_work_linear(&body(200), &body(400));
        // `left_right_closes`: `\left` openers in display math, no `\right`
        // anywhere — the pair stack stays non-empty, so the blank-line anchor
        // alone never cuts the scan.
        let body = |n: usize| format!("\\[\n{}\\]\n", "\\left( x\n".repeat(n));
        assert_scan_work_linear(&body(200), &body(400));
    }

    /// The math gates' one-closer-at-EOF shapes (`TODO.md`, container stack
    /// C2.3). These stay linear for a reason the batch does not supply: the
    /// gates are single-entry, and the first opener whose closer is reachable
    /// *swallows* every opener after it, so there is no second query to make.
    /// The pin is on the shape, not on the mechanism — a future gate that
    /// re-gated openers inside a math body would fail here.
    ///
    /// The one shape that stays quadratic is deliberate and recorded: a `${`
    /// per line ratchets the brace depth upward and never returns to 0, so no
    /// later opener sits at the seed's level and every one re-scans. Only a
    /// precomputed per-frame map could reach it; a batch cannot, and this test
    /// does not pretend otherwise.
    #[test]
    fn math_batch_stays_linear_with_one_closer_at_eof() {
        // `delim_math_closes`, both flavors.
        let body = |n: usize| format!("{}\\]\n", "\\[ x\n".repeat(n));
        assert_scan_work_linear(&body(200), &body(400));
        let body = |n: usize| format!("{}\\)\n", "\\( x\n".repeat(n));
        assert_scan_work_linear(&body(200), &body(400));
        // `dollar_closes`: a run of `$` openers, each paired by the next.
        let body = |n: usize| format!("{}$\n", "$ x\n".repeat(n));
        assert_scan_work_linear(&body(200), &body(400));
        // The display twin, whose seed sits one token past the opener.
        let body = |n: usize| format!("{}$$\n", "$$ x\n".repeat(n));
        assert_scan_work_linear(&body(200), &body(400));
    }
}
