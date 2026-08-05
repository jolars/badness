# Formatter

The formatter is the **sole authority on layout** (tenet #1 in
[Architecture](architecture.md#tenets)). It lowers the CST into a
Wadler/Prettier-style `Doc` IR, which a printer lays out under a flat/break fit
model. This page covers the IR, the paragraph and math wrap modes, table column
alignment, expl3 code formatting, and math operator spacing.

## The formatter is whitespace-only

The layout engine changes only **trivia**—whitespace, newlines, comments, and
the `.dtx` margins/guards. It never inserts, deletes, or rewrites a *non-trivia*
token. Everything else is emitted verbatim; the mechanism is purely that each
maximal run of `WHITESPACE`/`NEWLINE` trivia is replaced by a break primitive
and indentation is computed by the printer.

This means meaning-preserving *content* rewrites do **not** live here. Stripping
redundant braces around a single-token script (`x^{2}` → `x^2`) and rewriting
plain-TeX `$$…$$` → `\[…\]` are both *linter autofixes*
(`redundant-script-braces` and `dollar-display-math`; see [Linter](linter.md)),
not layout. This is the mirror of tenet #1's fix-then-format rule: just as the
formatter never runs inside `--fix`, content rewrites never run inside `format`.
The payoff is a by-construction guarantee— the non-trivia-content oracle in
`assert_format_invariants` (`tests/format.rs`)—rather than a
meaning-preservation argument checked only by fixtures.

The one subtlety: the formatter may still change CST *shape*. The parser's [math
operator split](parser.md#math-operator-atoms) re-groups a catcode-12 `WORD`, so
inserting insignificant math whitespace (`a+2` → `a + 2`) makes the output
re-lex into separate atoms. The oracle compares the *concatenated text* of
non-trivia tokens (not their boundaries), so it tolerates this re-grouping while
still catching any inserted or deleted non-trivia character.

## Trivia-invariant layout

Whitespace-only says what the formatter may *write*. Trivia-invariant layout
says what the lowering may *read*:

> Layout is a function of non-trivia content, config, and only those trivia
> predicates the formatter itself preserves.

A predicate `P` is **preserved** when `P(fmt(x)) == P(x)`. Reading a preserved
predicate is safe—the formatter cannot change the answer—while reading an
unpreserved one means pass 1's layout silently edits pass 2's input.

  | Predicate                              | Preserved?                                                                                     |            |
  | -------------------------------------- | ---------------------------------------------------------------------------------------------- | ---------- |
  | blank line present (`newlines >= 2`)   | yes—a blank line in, a blank line out                                                          | **safe**   |
  | comment present, own-line vs. trailing | yes—comments are never relocated                                                               | **safe**   |
  | `%` margin or `%<…>` guard at column 0 | yes—pinned by `Ir::ColumnZero`                                                                 | **safe**   |
  | **gap is a lone newline vs. a space**  | **no**—`alpha\nbeta` → `alpha beta`, and a width wrap writes a newline where there was a space | **unsafe** |

### Why this makes idempotence a theorem

The formatter changes only trivia, so `fmt(x)` *is* a trivia-perturbation of
`x`. If layout is invariant under trivia perturbation, then
`fmt(fmt(x)) == fmt(x)` follows immediately. Idempotence stops being an
empirical property defended one decision at a time and becomes a consequence of
a single rule.

That matters because the alternative does not scale: every layout decision that
reads the unsafe predicate is an independent latent bug, and the supply of
decisions is unbounded. The whole K&R↔Allman family (issues \#71, \#94, \#96,
\#97) is that pattern—a soft width break becomes a hard statement boundary on
the reparse, `contains_forced_break` flips, and the layout with it.

This is **not** parse stability, which `AGENTS.md` deliberately declines: the
math operator split re-granulates a `WORD` driven by non-trivia *content* and
reads no trivia at all, so it is unaffected.

### Two tiers

**Tier 1 (default).** The lowering must not be *able* to read the unsafe
predicate. The enforcement is to delete the information at the boundary: the
lowering consumes a normalized inter-token gap

```
Gap = Glued | Space | BlankLine | Comment(..) | Guard(..) | Margin(..)
```

with no `Newline` variant, rather than raw trivia tokens. A rule cannot key on
what it cannot see, which is the only form of this constraint that survives
contact with a large codebase.

**Tier 2 (opt-in modes).** Some modes are *defined* by reading authored breaks:
`WrapMode::Stable`/`Sentence`/`Semantic` (via `RunAtom::preferred_break_before`
feeding `Ir::PreferredFill`) and `ReflowKind::Statement` (a lone newline ends
the line so `\draw …;` lists keep one statement per line). These take a widened
gap, and each owes a written **fixed-point argument**: every layout the rule can
emit must re-read to itself.

`ReflowKind::Statement` already carries one, and it is the model—its
continuation is *flush*, so a width-wrapped tail re-parses as a line already at
the body indent and lays out identically. expl3's structural statements (S4)
reach the same end a different way: a call unit is re-derived from *content* on
every pass, so a width wrap anywhere inside it re-consumes to the same unit and
the layout re-decides identically.

### Known violations

Three sites read the unsafe predicate; one is a bounded residue and two are
Tier 2:

- The **expl3 fallback statement** (`formatter::expl_stmt`)—a statement whose
  head has no derivable arity (no `:` suffix; a `w`/`D`/mid-spec-`T`/`F` or
  unknown letter; a slot-shape mismatch; a guard mid-unit; the stream ending
  mid-unit) is the authored physical line, and a recognized unit's *same-line
  trailing junk* extent is line-bounded too. This is `SplitAtNewlines`
  demoted from *the* mechanism to a per-statement residue, and it carries its
  fixed-point argument in the module docs: fallback lines commit as plain
  greedy fills (each printed line re-segments to a fallback statement that
  re-fills to itself), gaps that could put a *recognized* head at a printed
  line start are unbreakable, and junk-glued statements render with every
  top-level gap hard so their newline-keyed extent can never move. The
  strict-invariance oracle cannot gate a stream containing a fallback head;
  the convergence oracle validates the argument empirically.
- `spans_multiple_lines` (`core.rs`)—block-vs-inline for `Opaque` groups
  *outside* expl3 regions. **Accidental**; already filed in `TODO.md` as
  "Opaque-group layout non-determinism".
- `RunAtom::preferred_break_before`—**Tier 2**, `WrapMode::Stable` and friends.
- `ReflowKind::Statement`—**Tier 2**, argument already written.

Statement boundaries for *recognized* expl3 heads are no longer on this list:
they are structural (S4, below), and the strict oracle holds for
recognized-only streams
(`perturb::tests::strict_oracle_accepts_structural_expl3_statements`).

### The oracle

Trivia perturbation: generate TeX-identical trivia perturbations of each input
(swap a lone newline for a space and back wherever the swap is
meaning-preserving) and check the formatter over them. One generator
(`formatter::perturb::trivia_perturbations`) feeds two oracles:

- **Convergence** (`check_trivia_convergence`, today's gate): every perturbed
  variant must format to a *fixed point* whose output parses cleanly,
  round-trips losslessly, and carries the same non-trivia content. This is
  strictly stronger than idempotence, which only ever exercises the single
  trivia configuration `fmt` itself produces—a hybrid needs a corpus file to
  land on exactly the right column arithmetic, while the perturbations
  synthesize those configurations directly. Deliberate authored-break
  *preservation* passes by construction, so a failure is always a real bug,
  and the check is valid under **every** wrap mode: it empirically validates
  the Tier-2 fixed-point arguments rather than exempting them.
- **Strict invariance** (`check_trivia_invariance`, the end-state gate):
  `fmt(perturbed) == fmt(original)`. This is the full trivia-invariant-layout
  contract; until the lowering no longer reads the unsafe predicate at all
  (the Gap-enum endgame) it fails wherever the formatter preserves an
  authored break, so it gates nothing yet.

Per file the generator emits two bulk variants (every eligible lone newline →
space, every eligible single space → newline) plus a few deterministic
single-flip reproducers. Eligibility encodes *meaning* safety only—blank
lines, comment-adjacent gaps, margined/guarded `.dtx` lines, and verbatim
neighbors are never touched—never layout ownership. Each variant is verified
post hoc (clean parse, identical non-trivia content, identical trivia-blind
CST skeleton) so newline-sensitive *parser* shape gates are dropped and
counted (`dropped_unsafe`) instead of polluting the layout inventory. The
convergence oracle runs as `badness debug format --checks trivia` (wrap
pinned to `reflow`, since `Preserve` converges trivially and stresses
nothing; opt-in, not part of `--checks all`) and inside the corpus invariants
sweep in `tests/format.rs`.

The staged rollout, with per-stage gates, is in `TODO.md`.

## The Doc IR

The engine is a Wadler/Prettier-style `Doc` IR (`formatter::ir::Ir`). Its core
variants are `Group`, `Line`, `SoftLine`, `HardLine`, `EmptyLine`, and `Indent`,
plus `Ir::Fill` (per-gap greedy break decisions) and `Ir::PreferredFill`
(source-break-aware global minimum-cost decisions) for paragraph reflow. The
enum also carries `Align`, `IfBreak`, `ConditionalGroup` (`AllLines`),
`Verbatim`, `ColumnZero`, `MarginPrefix`, and `Nil`—see `ir.rs` for the
authoritative list. Between lowering and printing, `Ir::propagate_breaks`
saturates every non-hug `Group`'s `expand` flag from its content, making the
flag the single representation of "forced open" (see *One representation of
"forced"* under the expl3 layout section).

### `Mode::Flat` is an honest contract (S2)

The printer's layout mode is a *verified claim*, not a hint. A producer may
dispatch a subtree in `Mode::Flat` only after verifying that the subtree's
whole flat-mode rendering fits (every line, for subtrees whose structural
`HardLine`s split it); consumers trust the claim, so a `Group` or conditional
group dispatched in `Flat` honours it instead of re-deciding—the parent's
verification and the print agree by construction. A producer that cannot make
the whole-subtree claim dispatches `Break` and lets children decide for
themselves: the choice of a `ConditionalGroup` candidate by first-line fit is
the whole decision (`pick_candidate` announces `Break`), and a
`Cmd::PreferredFill` break plan *places* atoms without verifying them, so
atoms inherit the fill's mode. Both dispatch decisions measure from
`Writer::current_col()`—the column the next visible character lands at,
counting a pending indent—because a fit verified from the wrong column is
still a lie, one the honest contract would then pin instead of letting nested
re-decisions paper over.

Two calibrated exceptions keep the contract honest at its edges:

- An **`expand` group** prints `Break` even under an incoming `Flat`: a
  subtree carrying a hard break was never a flat claim (its `HardLine`s fire
  in either mode), and pinning its `Line`s flat would glue content onto a
  forced-open line. This requires the `propagate_breaks`-saturated tree.
- A **trailing-block hug** claims only `Mode::FlatPrefix`: its prefix
  measurement (`FlatMeasure::HugPrefix`) stops *successfully* at the block's
  first forced break, so only the prefix was verified. Trivia-level layout (`Line`, `SoftLine`, `IfBreak`, fill gaps)
  renders flat exactly as under `Flat`—that keeps the head glued—but a group
  may sit *past* the break the measurement stopped at, so groups under
  `FlatPrefix` re-decide (`\@@_if_key_value:VTF {T}{F}`: the hug verified up
  to `T`'s detonation; `F` was never measured, and pinning it flat printed a
  125-column line).

The honest contract is what finally delivers the trailing-hang rule's stated
intent (see *Trailing hang groups*): an `AllLines` candidate is verified by
rendering the whole subtree flat and checking every line, so the chosen
candidate's nested groups now keep exactly the measured layout in the real
print, instead of re-deciding against the rest of the line and detonating
into the K&R hybrid the measurement never saw. It also resolved the
`latexrelease.sty` issue-#97 residue (the step-fill/`group_fits`
rest-awareness disagreement) ahead of S3's predicate consolidation.

### A group's fit measures the rest of the line in the mode it will print in

Deciding a `Group` dispatched in `Break` mode measures its flat rendering
*plus* the already-queued commands up to the next line break
(`printer::group_fits` → `rest_fits`, the Wadler/Prettier "fits the rest of
the line" rule); a group dispatched in `Flat` skips the decision entirely (the
honest contract above). A later group in that rest is measured in the mode it
will actually print in: **flat when its own flat form still fits from here,
broken otherwise.** Measuring a doomed group flat charges the group being
decided for width that will never land on this line—and the charge depends on
where the doomed group's own body happens to break, which the *previous
formatting pass* decided.

That is a fixed-point hazard, not just a cosmetic one. In
`\EditInstance{block}{thm}{␣…long keyvals…␣}` inside an expl3 region
(`latex-lab-block.dtx`, issue #71) pass 1 broke `{block}` and `{thm}` out onto
their own lines because the trailing block measured flat and overflowed; pass 2
kept them inline because that block had by then acquired a hard break, and
idempotence failed. Deciding the rest group locally makes both passes agree—and
gives the trailing block the hug the expl3 `COMMAND` lowering always intended:
short leading arguments stay inline, only the over-long trailing one detonates.

The same rest-awareness extends to a fill's *last* atom (`printer::step_fill`):
the lowering glues trailing content after a statement's fill (a final
`\l_…_tl` riding the line), so the last atom's flat claim must survive the
rest of the line too. Without it the atom's folded hang break is never taken:
the atom alone fits, goes `Flat`, and the glued tail overflows a line the
measurement never saw (`\prop_get:cnN {…}{…}\l__tag_get_parent_tmpc_tl`,
which wants the l3 continuation hang).

### The fit predicates share two traversals (S3)

Every fit decision is served by exactly two measurement walkers in
`printer.rs`, each carrying a small explicit policy—the S3 consolidation of
what had grown into five overlapping predicates (`flat_width`, `fits`,
`group_fits`, `rest_fits`, `first_line_fits`) whose hand-copied traversals
had drifted apart:

- **`flat_end`** simulates a subtree laid out flat and returns the column it
  ends at (or `None`: the measurement failed). Its `FlatMeasure` policy names
  the three deliberate readings. `Footprint` is the unbounded flat width
  behind `flat_width` (a single-line comment counts—it shares the line,
  forcing a break only after—and `expand` is ignored); `Fits` asks whether
  the subtree lies fully flat within the width (any forced break or `expand`
  group fails; the non-hug `Group` decision and `group_fits`'s flat phase);
  `HugPrefix` is the trailing-block hug's claim (a forced line break stops
  the measurement *successfully*, a comment still fails it, and
  `excuse_overflow` excuses an atom no break could rescue).
- **`line_fits`** walks a pending work list mode-aware up to the first
  newline that would actually be emitted. `first_line_fits` (the
  `ConditionalGroup` picker's probe) and `rest_fits` (the queued-commands
  adapter behind `group_fits` and `step_fill`'s last-atom check) are thin
  seeds over it. Its one policy knob, `CommentFit`, records the one
  deliberate difference between those contexts: a candidate carrying a
  standalone comment can never render flat (`Fails`), while a comment in the
  rest of an already-committed line is there either way and counts its width
  (`SharesLine`).

Sharing the traversal dissolved the drift the copies had accumulated: a
later group in the rest is now decided with its own hug flags, a later
conditional group through `pick_candidate` rather than assumed flat-most,
and a `Break`-mode preferred fill by its first atom rather than measured
whole-flat—the arms `first_line_fits` always had. All of it measured
behaviour-neutral on the gate corpora (byte-identical sweeps at widths
60/80/120), because mode propagation (S2) had already removed every reachable
disagreement: a group inside a flat parent is never *asked* whether it fits.

## Paragraph line breaks

Paragraph line breaks are controlled by a `WrapMode` (`Reflow` default,
`Stable`, `Sentence`, `Semantic`/sembr, `Preserve`), modeled on the sibling
[panache](https://github.com/jolars/panache) formatter and mechanized through
the `Doc` IR, not a separate line-filler. All five are implemented:

- `Reflow` width-fills.
- `Stable` keeps acceptable authored breaks while optimizing
  overflow/underflow/change/displacement/raggedness against a hard-coded soft
  target (`FormatStyle::stable_wrap_target`, `line-width - 15`; not yet
  configurable). Its completed prose runs render through `Ir::PreferredFill`, so
  an already-reasonable paragraph is left close to how it was authored (small
  stable diffs) rather than fully reflowed.
- `Preserve` keeps authored breaks.
- `Sentence`/`Semantic` split one sentence per line (width ignored) through the
  shared `reflow_elements` engine—each completed prose run is rendered as a
  `Fill` (reflow), a `PreferredFill` (stable), or as space-joined sentences
  (sentence/semantic). `Semantic` additionally ends a line at every authored
  newline (sembr; no clause detection).

Sentence-boundary detection is a per-language abbreviation profile
(`formatter::sentence`, ported from panache) resolved from `[format] lang` +
`[format.no-break-abbreviations]` into a `SentenceOptions` threaded on
`FormatContext`; babel/polyglossia auto-detection is deferred.

The `\\` line break (with a tightly-bound `*`/`[len]`) is grouped by the
*parser* into a `LINE_BREAK` node so the formatter sees `\\[2ex]` as one unit.

## Display-math line breaks

Display-math line breaks have their own knob, `MathWrap` (`[format] math-wrap`:
`auto`/`preserve`/`single-line`/`break`), scoped to single-formula display
bodies (`\[…\]`, `$$…$$`, non-grid `equation`; grids and inline math are
untouched). `auto` (the default) resolves against the effective `WrapMode` at
`LowerCtx` construction (`Preserve` → preserve authored breaks, else the
amsmath-style breaker), so per-file-kind wrap defaults carry over to math for
free.

A body whose first non-trivia atom is a `\label{…}` splits that label onto its
own line, starting the formula fresh below it—the label is equation bookkeeping,
not part of the math. This runs under every `MathWrap` policy (it is applied
before the mode dispatch, then recurses so the remaining formula lowers under
its own policy), so `single-line` and `preserve` bodies split the label too. The
split is otherwise deliberately narrow: it fires only on a *leading* `\label` (a
trailing one stays glued to its line, keeping the rule out of `aligned`-style
bodies), and it is keyed to the single `\label` command by name, not a wider
bookkeeping set. A body that is nothing but a label stays on one line rather
than gaining a dangling break.

## Math operator spacing

Operator atoms are produced by the parser's [math operator
split](parser.md#math-operator-atoms); their *spacing* is a formatter concern
(tenet #1): a single space around each binary/relation atom, with unary signs
and scripts tight.

## Table column alignment

Table column alignment (`tabular`/`array`) is a formatter concern—layout, so the
formatter owns it. The `{lcr}` column spec is parsed by `formatter::colspec`
into per-column `ColAlign`s, reading only the static argument text (no macro
meaning). It is **conservative**, bailing to all-left on any token it does not
model (`p`/`m`/`b` count as left, `*{n}{}` expands, `>{}`/`<{}`/`@{}`/`!{}` and
vertical rules add no column).

The grid renderer aligns each cell L/C/R; a right/center *last* cell pads on the
left only (no trailing whitespace, so idempotence holds—padding re-trims on
re-parse). A `\multicolumn{n}{spec}{…}` spans `n` columns: excluded from
single-column widths, aligned within its span by its own spec, and left to
overflow rather than ballooning narrow data columns. The rule-line recognizer
(`non_row_line`) tolerates the booktabs `\cmidrule(lr){2-3}` paren trim (the
`(lr)` `WORD` and detached `{2-3}` group are consumed as part of the rule line),
and a same-line `\\ \hline` is normalized onto its own passthrough line.

### Which environments grid-align: signature flag plus a `&`-shape gate

Routing to the grid is primarily a **semantic** fact: the curated `align`
signature (`tabular`, `array`, and every math grid—`align`, `pmatrix`, …) picks
the grid path, and the parallel `math` flag additionally routes the math-aware
lowerer so cells get role-aware operator spacing. But the signature DB cannot
name a *user-defined* environment (`\newenvironment{myaligned}{…}{…}`, or one it
never sees defined at all), so an environment shaped exactly like an alignment
would otherwise miss the grid (issue #84).

So, after the curated `align`/`math`/list arms have had their say, one more arm
routes **any** remaining environment whose body carries a **top-level `&`** to
the non-math grid (`body_has_top_level_ampersand`). A `&` at catcode 4 is a
column tab—a static CST-shape fact, read exactly as `build_alignment_grid`
defines a cell boundary (a direct child of the body or its single wrapping
`PARAGRAPH`; a nested `&` lives in a child node and stays invisible). It is the
same move the environment group-boundary gate makes for `\begin`/`\end`
(`parser.md`): generalize a curated set to the package code it cannot name, from
shape alone. Three properties keep it safe:

- **Keyed on `&`, never `\\`.** A `\\`-only body is a line stack, not a column
  alignment; gridding an arbitrary `\begin{center}a \\ b\end{center}` would
  reflow it. Only a `&` opts an unknown environment in.
- **Whitespace-only and self-correcting.** The grid renderer touches only trivia
  (the non-trivia-content oracle stays green), and any shape it cannot lay out
  on aligned rows falls back to `lower_environment`—today's plain indented body.
- **Placed *after* the curated arms.** Known list/math/align environments are
  already routed, so a stray top-level `&` inside, say, an `itemize` body never
  reroutes it. Doc-margined bodies are excluded (grid padding would push a `%`
  margin off column 0), matching the neighboring environment arms.

An unknown environment takes the *non-math* grid, so its `&` columns align and
gain the single `" & "` spacing, but its cells are **not** given math operator
spacing (the parser never entered math mode for it): `a&=b` becomes `a & =b`,
not `a & = b`. Inferring math mode for an unnamed environment is exactly the
meaning the parser declines to guess.

## expl3 code formatting

The expl3 *letter* mode is a lexer fact (see [expl3 regions are macro
code](parser.md#expl3-regions-are-macro-code)). The matching *whitespace*
catcodes are a **formatter** concern: inside an expl3 region
(`\ExplSyntaxOn`…`\ExplSyntaxOff`, or `\ProvidesExpl*` to EOF) source spaces and
tabs are catcode 9 (ignored) and `~` is catcode 10 (a literal space). Because
inter-token whitespace is provably insignificant, the formatter owns the layout
of in-region code—indentation and line breaks—**regardless of `WrapMode`**.

This is **idempotent by construction**: the inserted whitespace is itself
catcode-insignificant, so re-lexing the output yields the same token sequence
and the deterministic layout is a fixed point. It is the property the generic
"hanging continuation indent" could not get, supplied here at the catcode level.

Region membership is **not** recorded in the CST: the lexer's expl3 toggle stays
transient, and the formatter recomputes in-region byte ranges in a read-only
pre-pass (`formatter::core::expl3_regions`) over the same fixed toggle *name*
set the lexer uses (`parser::lexer::expl_toggle`, shared so the two never
drift), stored as a `Vec<TextRange>` side channel in `LowerCtx`—the same
byte-range pattern as parser diagnostics. The CST, lexer, events, and
tree_builder are untouched, so losslessness is unaffected; the reformatted
output is a different valid text with the same meaning.

### House style (l3styleguide)

The target for in-region layout is the LaTeX Project's own **house style**, "The
LaTeX3 kernel: style guide for code authors" (`l3styleguide.tex` in
[`latex3/latex3`](https://github.com/latex3/latex3), `l3kernel/doc/`, LPPL). Its
mechanical, formatter-enforceable rules are:

  | #   | Rule                                                                                                                       |
  | --- | -------------------------------------------------------------------------------------------------------------------------- |
  | R1  | Lines under **80 characters** where possible.                                                                              |
  | R2  | A **two-space indent** per "level" of code.                                                                                |
  | R3  | Divide everything with **single spaces**, *except* "simple runs of parameter (`{#1}`, `#1#2`)", which stay **tight**.      |
  | R4  | Each **conceptually-separate step** on its own line.                                                                       |
  | R5  | Canonical brace layout: the body `{` on its **own line at +2**, the body at +4, `nTF` branches at +6, nested groups at +8. |
  | R6  | Related variants (`\cs_generate_variant:Nn`) *may* be **aligned** (optional).                                              |
  | R7  | **No tabs.**                                                                                                               |

The guide's own worked example is the gold reference (guide §*Format of the code
itself*):

```latex
\cs_new:Npn \module_foo:nn #1#2
  {
    \tl_if_empty:nTF {#1}
      { \module_foo_aux:n { X #2 } }
      {
        \module_foo_aux:nn {#1} {#2}
        \module_foo_aux:n { #1 #2 }
      }
  }
```

**Conformance.** The formatter satisfies R1, R2, R6 (it normalizes any alignment
away—permitted, since R6 is optional), and R7. R3 is the
`expl_group_is_spaced`/`is_simple_param_run` split below; R4/R5 are the
conditional-break rule below. The brace-column progression (R5: body `{` at +2,
body +4, `nTF` branches +6, nested +8) falls out of the nested `Ir::indent` the
`hang_group` rule and the conditional lowering emit.

There is **no path divergence** between file flavors for genuine expl3 code:
`.sty`/`.tex` and a `.dtx` `macrocode` body both route the code through
`lower_expl_paragraph` → `lower_expl_code`, and the `.dtx` margin frame's
column-0 base composes with the same in-region `hang_group`/branch rules, so
identical in-region code lays out byte-identically under either flavor (pinned
by the `dtx_expl3_conditional` fixture against `expl_conditional_gold`). The
"body `{` at column 0" a bare `macrocode` block shows is *generic* LaTeX
lowering: a `macrocode` with no `\ExplSyntaxOn`/`\ProvidesExpl*` toggle is by
design **not an expl3 region** (`expl3_regions` keys on the toggle, not the
environment), so its body never reaches `lower_expl_code` and its inner spacing
is not normalized.

The l3styleguide's non-layout rules (naming prefixes, `:D`-primitive discipline,
expandability) are **out of scope**—they are meaning, not trivia, and belong to
a linter, not the layout engine.

### Simple parameter runs stay tight (R3)

R3's exception is the `is_simple_param_run` gate on `expl_group_is_spaced`. The
default for an expl3-*named* command's brace argument is the canonical inner
space (`{ value }`); a group whose body—ignoring outer padding—is a run of
**adjacent parameter tokens** (`{#1}`, `{#1#2}`, `{##1}`) instead stays tight,
overriding the command-name rule. A padded `{ #1 }` therefore *normalizes* to
tight `{#1}`, but any whitespace *between* the parameters (`{ #1 #2 }`) or any
non-parameter token (`{ X #2 }`) keeps the inner spaces—exactly the
discrimination the gold example draws (`{#1}` tight beside `{ #1 #2 }` spaced).
The gate reads only token kinds and single-digit index text, no signature or
meaning; the removed padding is catcode-9 whitespace, so the rewrite is
trivia-only and idempotent by construction.

### Conditional branches break structurally (R4)

R4 ("each conceptually-separate step on its own line") is enforced for expl3
conditionals by `lower_expl_conditional`, routed from `lower_expl_code`'s node
loop. A *statement-leading* conditional—one that starts its logical line,
nothing accumulated before it—explodes **unconditionally**: the head and any
leading arguments on one line, then each `T`/`F` branch on its own line hung one
indent step (+6 inside a +4 body; a multi-line branch nests its interior +8,
giving the R5 progression). It breaks even when the whole conditional would fit
on one line; the l3styleguide's gold example puts a short true branch on its own
line beside a multi-line false branch, and the house style treats the branch
list as structure, not a width-fill.

The conditional is recognized by `expl_conditional_branches`: the argspec after
the final `:` in the command name, if it ends in a run of `T`/`F`, gives the
branch count (`:nTF` → 2, `:nT`/`:nF` → 1). In an expl3 argspec `T`/`F` mark
only the true/false slots, so this is exact—and it is a purely lexical fact of
the name (the whole name lexes as one `CONTROL_WORD` in-region), so meaning
stays out of the parser (decision #2), mirroring the R3 name rule. Each branch
is lowered as a *soft* `lower_expl_group`, so a short branch stays `{ … }`
inline on its line while a long one breaks internally.

Two scopes keep it precise. A conditional used **mid-line as a value**
(`,key = \tl_if_empty:nTF …`) is not statement-leading, so it stays on the
width-driven path (issue #71, `expl_trailing_block_hug`). And a conditional
whose branch groups do **not attach** to it—an `:NTF`/`:nNnTF` whose
single-token `N`/`V`/operator argument breaks greedy brace attachment, leaving
the branches on a following sibling—falls back to the width path rather than
mis-lower a partial shape. The statement-leading break is width-independent, so
it is a fixed point: the exploded output re-parses to the same greedy `COMMAND`
(brace arguments attach across the inserted newlines) in statement position and
re-explodes identically. A LaTeX2e conditional (`\@ifpackageloaded`, no
`:`-argspec) is never matched, so issue #94's sticky-fill handling is untouched.

The mid-line value scope is itself **width-conditional**, and that is where
idempotence is subtle (issue #96, `lthooks.dtx`). A *trailing* conditional (head
atoms before it, only trivia after it in the statement) that would simply join
the sticky fill is not pass-stable: when head + conditional overflow, the fill
drops the head to its own line, which on the next parse makes the conditional
statement-leading—so it then explodes *unconditionally*, and the two passes
disagree. `lower_expl_code` therefore commits head and conditional as one
`group(Ir::if_break { flat, broken })`, decided by the group's **flat** width
(head included, via `group_fits`, so it is neither fooled by a branch that
detonates internally nor evaluated apart from its head): fits ⇒ `head cond` on
one line; overflows ⇒ head on its own line then the R4 explosion, which
re-parses statement-leading and re-explodes to the identical bytes. Gated by
`is_trailing_in_statement`, so a conditional with real content after it stays on
the ordinary fill path.

### Positional gate on layout ownership

The shared *name* set is necessary but not sufficient to open a region: the
formatter additionally requires the toggle to be a **top-level statement**
(`toggle_is_top_level`), because the catcode-9 whitespace assumption only holds
where TeX actually *executes* the toggle at load. A toggle spelling that is
never run is a false positive—mis-owning its layout rewrites real space tokens
even though the byte-level losslessness and idempotency oracles stay green
(issue \#69, `l3kernel/expl3.sty`). Two shapes are rejected:

- **Definee position:** the toggle command's immediately-preceding non-trivia
  sibling is a `\def`/`\let`-family primitive, so the toggle is the control
  sequence being *defined*, not executed
  (`\protected\def\ProvidesExplPackage{…}` in the loader). Reuses the parser's
  `is_def_prefix_command`.
- **Nested in a group / definition body:** an ancestor of the toggle's command
  is a `GROUP` or `OPTIONAL`, so the toggle is tokenized into a replacement text
  and only ever executed—if at all—when that macro runs, not at load.

This deliberately **splits** the "same toggle set" invariant: the *name* set
stays shared between lexer and formatter (so a new toggle spelling is recognized
in both), but the *positional layout-ownership* rule is the formatter's alone.
The lexer keeps the naive name-only model on purpose—mis-lexing a name in letter
mode only splits CST tokens (lossless, cosmetic), whereas mis-*owning* layout
rewrites meaning, so only the higher-stakes side gates. See `AGENTS.md` (core
decisions) for the recorded rationale.

### Statement boundaries are structural (S4)

Statement boundaries are **call units**, not source newlines. A pure shape
scan (`formatter::expl_stmt::segment_expl_statements`) runs over each in-region
element stream before layout and decides, per gap, whether a statement ends
there; the layout loop commits logical lines where the map says. The formatter
owns one-call-per-line: authored same-line calls split, authored mid-call
newlines join, and `\cs_new:Npn \foo:n #1 {…}`—several sibling CST nodes—is one
statement however it was authored.

A unit is a head `COMMAND` whose name has derivable arity
(`semantic::expl3::expl3_slots`, the argspec suffix read letter by letter:
`N`/`V` one token, `n c v o x e f` one brace group, trailing `T`/`F` branch
groups, `p` parameter text) plus the elements its slots consume. Consumption
draws from the head's greedily-attached children first, then following
siblings, with three load-bearing rules:

- **Peel-back.** Greedy attachment routinely gives an argument to the wrong
  owner (`\cs_new:Nn \foo:n {body}` attaches `{body}` to `\foo:n`); a
  `COMMAND` consumed into a single-token slot has its own attached children
  pushed back onto the scan queue for the *outer* head's remaining slots.
  Only the head's argspec ever drives consumption—an argument's own argspec
  is inert data, exactly as TeX grabs it.
- **The p-scan.** Parameter text ends at the first explicit `{` (TeX's own
  static rule), scanning the flattened peeled order so a delimited text
  (`#1 \q_stop {body}`) finds the body wherever attachment put it.
- **Preserved-trivia reads only.** A blank line ends the unit where it stands
  (the partial unit commits, pass-stably); a comment is transparent to
  consumption; a docstrip guard or doc margin aborts to the fallback
  (guarded alternative bodies make arity lie, issue #78). The region toggles
  are recognized zero-arity units, so a region's opening line is structural
  too.

Whatever the scan cannot resolve degrades to the **fallback**: the authored
physical line, the old newline rule demoted to a per-statement residue (see
*Known violations* for its fixed-point argument). A completed unit also
absorbs trailing *same-line* junk—punctuation, unrecognized command tokens, a
trailing comment (`\int_use:N \c@… , %mc-num`)—and a junk-bearing statement
renders with every top-level gap unbreakable so the authored line shape
survives (xparse's `\bool_if:NTF … { \cs_set:cpn } … ##1 \q_@@ …` definition
trickery).

Within one command's attached arguments (`Statements::Ignore`) there are no
statements at all: a newline is inert whitespace and only the width fill
breaks. A single inserted space at any preserved token boundary keeps
re-lexing from merging two tokens.

The subsumed idempotency mechanism: a width wrap inside a recognized unit is
harmless because the next pass re-derives the same unit from content and
re-runs the same width decisions—the K&R↔Allman family's root (a wrap
re-reading as a statement boundary) is gone for recognized heads, by
construction rather than per-shape countermeasures.

### Trailing comments

A *trailing comment* rides its statement line **zero-width** (`Ir::ZeroWidth`,
rustfmt-style): the line may overflow, but prose length never re-breaks code,
and the comment is never relocated—moving it would rebind it as the *next*
statement's leading doc comment on the second pass (see [trivia
attachment](parser.md#trivia-attachment)), changing its attachment.
gofmt/rustfmt never relocate trailing comments either; ruff exempts pragma
comments from width for the same reason.

### Continuation groups

A *continuation group*—a brace group that **starts a fresh atom** (nothing glued
before it: any trivia flushed the atom)—indents **one step** under its head
statement (`\cs_new:Npn \foo:n #1` / `␣␣{ body }`, the l3styleguide shape). The
step wraps the break and *the group alone* in one `Indent` (as a folded
statement separator, or a folded fill gap for a mid-statement group): the rest
of the line stays at base, because a width break re-reads its atoms as ordinary
base-indent statements on the next pass—break and group body must land at the
same column either way for the layout to be a fixed point.

The rule keys only on the group shape
(`child.kind() == GROUP && atom.is_empty()`), **not** the statement mode, so it
fires identically whether statement boundaries are structural
(`Statements::Structural`) or absent within one command's attached
arguments (`Statements::Ignore`). This is what gives an *attached* brace
argument the l3 hang: `\hbox_set:Nn \l_tmpa_box` / `␣␣{ … }`, or the `T`/`F`
branches of `\cs_if_exist:NTF` each hung one step. A directly-abutting argument
(`\EditInstance{a}{b}{…}`, no space) leaves `atom` non-empty and stays K&R-glued
instead—the same `atom` emptiness discriminates space from glue.

**Head-hug.** A detonating *non-group* child (a command subtree whose first line
is a head atom—e.g. the `N`-argument
`\__kernel_dependency_version_check:nn{T}{F}` of `\cs_if_exist:NTF`) that
follows a head on the current line, space-separated, is kept on that line by an
`Ir::group_hug` wrapping `[head, sep, block]`. The hug is rest-aware (its
`FlatMeasure::HugPrefix` measurement stops *successfully* at the block's
first forced break), so it measures only the
prefix `head␣<block-first-line>`, never the block body—the issue-#71-safe
measurement, deliberately not the `step_fill` local `flat_width` cascade that
would split a short head off a detonating trailing block. Because only the
prefix was verified, a successful hug dispatches its inner as
`Mode::FlatPrefix`, not `Flat` (see *`Mode::Flat` is an honest contract*): the
head's gaps render flat, but any group past the first forced break re-decides
for itself.

**Sibling coupling.** Within one command's attached arguments (`Ignore` only),
if any brace argument detonates on a *forced* break—a docstrip guard, comment,
or `.dtx` margin (`expl_group_forces_break`, a cheap token scan, no
lowering)—every brace sibling is forced to the broken (Allman) form via
`lower_expl_group`'s `force_break`, so a short false-branch
(`{ \tex_endinput:D }`) expands to match a multi-line true-branch. Keyed on the
*forced* trigger only, so the coupling is a pass-stable function of the arg-list
content; a sibling that would break solely from **width** does not couple (width
is a printer decision, invisible at build time).

**One representation of "forced": `propagate_breaks` saturates `expand` (S1).**
After lowering, a single bottom-up prepass (`Ir::propagate_breaks`, run at the
lowering→printer seam in `format_root`) marks every non-hug `Ir::Group` whose
inner contains an unconditional forced break as `expand`, with
`Ir::contains_forced_break`'s exact semantics: an `IfBreak` shields its
branches, a conditional group's flat-most candidate decides, and every
candidate and branch is still saturated inside. The flag is thereafter the one
representation of "forced open" the printer trusts; `contains_forced_break`
survives as the *query* the lowering asks about pre-pass sub-IR (block-amid-
prose, the hang paths, grid cells, flat-collapse).

The mode pin that issue \#97 needed now falls out instead of being a hand-made
special case. A group's body laid out as a bare concat inherits whatever mode
the caller was dispatched in, and inheriting `Flat` was the bug:
`printer::step_fill` in flat mode lays *every* gap flat without measuring,
while the groups hanging off those gaps still decide their own break—the K&R
hybrid `\int_set:Nn \l_…_int {` with the body wrapped below, not a fixed point
since the wrapped lines re-parse as separate statements and the next pass lays
the same group out Allman (latex3's `l3trial/l3auxdata/l3auxdata.dtx`).
`lower_expl_group`'s forced (Allman) form now differs from the soft form only
in its boundary separator—a `HardLine` instead of `Line`/`SoftLine`—and those
in-shape hard lines are what the prepass reads to mark the group, which is what
pins the body's mode to `Break`. Fixture `expl_forced_block_body_mode` still
pins the layout; its leading `%` comment is load-bearing, being what forces the
enclosing blocks open.

Two carve-outs keep the flag honest. Hug groups are never marked: their inner
holds a forced break *by construction* (the trailing block), and their break
decision is the hug fit, not the flag. And two measurements deliberately ignore
`expand` (both now explicit `FlatMeasure` policies): the flat *footprint*
(`Footprint`, behind `flat_width`) recurses instead (a group forced open only
by a single-line comment still has a flat width—the comment shares the line,
forcing a break only after), and the hug-prefix measurement (`HugPrefix`)
lets the content decide, since a nested block's first hard break must stop
the measurement *successfully* while a prefix comment must fail it—a
distinction the flag cannot carry. That last point is S1's one deliberate, narrow layout
change: an interior-comment-forced or sibling-coupled block detonating in a
head-hug prefix now hugs (`\global\setbox9 \vtop{%`) instead of splitting the
head onto its own line, which is the head-hug rule's documented semantics
(corpus sweep: 12 files, all this family, gate sets unchanged).

### Trailing hang groups (K&R↔Allman idempotence)

A *trailing* greedily-hung `{body}`—a brace group after head atoms with only
trivia after it, whose body is a **multi-command fill**
(`\int_gset:Nn \g…_int {…}`)—would otherwise flip K&R↔Allman across passes
(issue \#96 residue, `tagpdf.sty` line 1007,
`pdfmanagement/latex-lab-testphase-bookmark.sty` line 298). A body authored on
one source line hangs K&R on pass 1 (`\tl_put_right:Ne \l_tmpa_tl {`, `{` glued,
the body's fill wrapping below), but those wrapped lines re-parsed as several
newline-split statements under the pre-S4 model, so the body then carried a
*forced* break and the continuation branch detonated it Allman on pass 2—the
same "a width break becomes a structural boundary on the next parse" class as
the trailing conditional above. (Structural boundaries have since removed the
re-split for recognized heads; the three-way remains for the body-fit
flip a *fallback* body can still exhibit.) Since S2, the accepted candidate is dispatched in honest
`Mode::Flat`, so the real print keeps exactly the layout all-lines-fit
measured—before that, nested groups re-decided rest-aware at print time and
could still detonate into the hybrid the measurement had rejected.

`lower_expl_code` commits such a group as one `Ir::conditional_group_all_lines`
over three candidates—**flat** (`head { body }` on one line), **Allman-inline**
(head on its own line, `{ body }` inline one step under it), **Allman-broken**
(`{` on its own line, body a further step, `}` back)—keyed on the body's *real*
one-line fit rather than its incidental source-line count. All-lines-fit
measures each candidate with its nested brace groups forced **flat** (a very
wide probe line), so a candidate is accepted only as a genuine one-liner and
never as a hybrid where an inner group detonated to keep each printed line
short; both Allman forms re-parse to a head statement plus a statement-leading
`{body}` that the continuation branch re-emits identically, so each is a fixed
point.

Narrow guards keep this off the shapes the ordinary hang path already lays out
stably: a single-command or bare-value body (`expl_group_body_is_multi_atom`, a
top-level `COMMAND` count—no top-level wrap), a body that *already* carries a
forced break (comment/guard/margin, or several statements—it wants the plain
Allman block), coupled siblings, and the multi-argument/conditional-branch
shapes (`statement_has_preceding_group`, `head_command_has_grouped_sibling_arg`)
whose head this branch—seeing only its own command's `Ignore` stream—cannot
measure as one unit, so intercepting them would detonate a *preceding* argument
group.

### Sticky-break statement fills

Every *structural* expl3 statement line is committed as an `Ir::StickyFill`,
not a plain `Ir::Fill`. (A *fallback* or junk-glued line instead commits as a
plain greedy fill: greedy packing is self-fulfilling—each printed line
re-segments to a fallback statement that re-fills to exactly itself—while a
sticky cascade forces atoms that would fit onto broken lines, a shape the next
pass's shorter per-line statements do not reproduce.) Both greedily fill atoms
across the width; the difference is the
break *cascade*: in a plain fill each gap decides independently (a long word
breaks, the next words keep filling—correct for prose reflow), whereas in a
sticky fill, **once any atom lands on a broken line every later atom breaks
too**. The cascade lives in `printer::step_fill` (`sticky`/`broken` on
`Cmd::Fill`); prose reflow keeps the plain greedy `Ir::Fill`.

This is what a *width*-broken sibling needs that the forced-only **sibling
coupling** above cannot give. When a true-branch block detonates purely from
width (no guard/comment/margin), the greedy fill would let a following empty
false-branch `{}` glue back onto the block's short closing `}` line (`} {}`),
because at that column the two-byte `{}` fits. But whether the block's own body
broke **hard** (a source newline, `contains_forced_break`) or **soft** (a width
fill) is not pass-invariant—the formatter's own reflow turns one into the
other—so pass 1 (`} {}`) and pass 2 (`}` then `{}` on its own line) disagreed
and idempotence failed (issue #94, josephwright/siunitx's
`\@ifpackageloaded {pkg} {…block…} {}`). The sticky cascade defers the decision
to the printer's *actual*, column-aware break: the empty branch follows the
block onto its own line on **every** pass. Unlike the width-independent sibling
coupling it fixes the exact width-driven case that coupling deliberately skips,
and it does so without exploding the short leading arguments into blocks—`{pkg}`
stays inline on the head line; only the arguments *after* the detonated one
move.

### Interaction with `.dtx` doc margins

In a `.dtx`, a region regularly spans several `macrocode` chunks
(`\ExplSyntaxOn` in one, the `Off` chunks later), so the doc-margined lines in
between—doc prose and the frame lines themselves—are subtracted from the regions
(`subtract_doc_margin_lines`): only code lines are formatter-owned, and a `%`
margin stays in column 0.

More generally, the line-oriented `.dtx` tokens (`DOC_MARGIN`, `GUARD`) are only
margins/guards *at line start*, so no relayout may merge or re-indent their
lines (the `contains_doc_margin` gates in `formatter::core`, issue #57). An
in-region code group carrying such a token in its body is held to the same rule:
`lower_expl_group` forces the broken (multi-line) form so the guard/margin rides
its own line and `lower_loose_token` pins it to column 0—flattening it into
`{ %<trace> … }` would re-lex the guard as an ordinary `%` comment that swallows
the closing brace, unbalancing the group on the next parse (issue #61,
l3ldb.dtx; the same swallow reasoning as an in-body `%` comment).
