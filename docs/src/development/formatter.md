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

## The Doc IR

The engine is a Wadler/Prettier-style `Doc` IR (`formatter::ir::Ir`). Its core
variants are `Group`, `Line`, `SoftLine`, `HardLine`, `EmptyLine`, and `Indent`,
plus `Ir::Fill` (per-gap greedy break decisions) and `Ir::PreferredFill`
(source-break-aware global minimum-cost decisions) for paragraph reflow. The
enum also carries `Align`, `IfBreak`, `ConditionalGroup` (`AllLines`),
`Verbatim`, `ColumnZero`, `MarginPrefix`, and `Nil`—see `ir.rs` for the
authoritative list.

### A group's fit measures the rest of the line in the mode it will print in

Deciding a `Group` measures its flat rendering *plus* the already-queued
commands up to the next line break (`printer::group_fits` → `rest_fits`, the
Wadler/Prettier "fits the rest of the line" rule). A later group in that rest is
measured in the mode it will actually print in: **flat when its own flat form
still fits from here, broken otherwise.** Measuring a doomed group flat charges
the group being decided for width that will never land on this line—and the
charge depends on where the doomed group's own body happens to break, which the
*previous formatting pass* decided.

That is a fixed-point hazard, not just a cosmetic one. In
`\EditInstance{block}{thm}{␣…long keyvals…␣}` inside an expl3 region
(`latex-lab-block.dtx`, issue #71) pass 1 broke `{block}` and `{thm}` out onto
their own lines because the trailing block measured flat and overflowed; pass 2
kept them inline because that block had by then acquired a hard break, and
idempotence failed. Deciding the rest group locally makes both passes agree—and
gives the trailing block the hug the expl3 `COMMAND` lowering always intended:
short leading arguments stay inline, only the over-long trailing one detonates.

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
identical in-region code lays out byte-identically under either flavor (pinned by
the `dtx_expl3_conditional` fixture against `expl_conditional_gold`). The "body
`{` at column 0" a bare `macrocode` block shows is *generic* LaTeX lowering: a
`macrocode` with no `\ExplSyntaxOn`/`\ProvidesExpl*` toggle is by design **not an
expl3 region** (`expl3_regions` keys on the toggle, not the environment), so its
body never reaches `lower_expl_code` and its inner spacing is not normalized.

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
loop. A *statement-leading* conditional—one that starts its logical line, nothing
accumulated before it—explodes **unconditionally**: the head and any leading
arguments on one line, then each `T`/`F` branch on its own line hung one indent
step (+6 inside a +4 body; a multi-line branch nests its interior +8, giving the
R5 progression). It breaks even when the whole conditional would fit on one line;
the l3styleguide's gold example puts a short true branch on its own line beside a
multi-line false branch, and the house style treats the branch list as structure,
not a width-fill.

The conditional is recognized by `expl_conditional_branches`: the argspec after
the final `:` in the command name, if it ends in a run of `T`/`F`, gives the
branch count (`:nTF` → 2, `:nT`/`:nF` → 1). In an expl3 argspec `T`/`F` mark only
the true/false slots, so this is exact—and it is a purely lexical fact of the
name (the whole name lexes as one `CONTROL_WORD` in-region), so meaning stays out
of the parser (decision #2), mirroring the R3 name rule. Each branch is lowered
as a *soft* `lower_expl_group`, so a short branch stays `{ … }` inline on its line
while a long one breaks internally.

Two scopes keep it precise. A conditional used **mid-line as a value**
(`,key = \tl_if_empty:nTF …`) is not statement-leading, so it stays on the
width-driven head-hug path (issue #71, `expl_trailing_block_hug`). And a
conditional whose branch groups do **not attach** to it—an `:NTF`/`:nNnTF` whose
single-token `N`/`V`/operator argument breaks greedy brace attachment, leaving the
branches on a following sibling—falls back to the width path rather than
mis-lower a partial shape. The break is width-independent, so it is a fixed point:
the exploded output re-parses to the same greedy `COMMAND` (brace arguments attach
across the inserted newlines) in statement position and re-explodes identically.
A LaTeX2e conditional (`\@ifpackageloaded`, no `:`-argspec) is never matched, so
issue #94's sticky-fill handling is untouched.

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

### Statement boundaries

Statement boundaries follow *source newlines* (the expl3 one-call-per-line
convention; a multi-token call like `\cs_new:Npn \foo:n #1 {…}` is several
sibling CST nodes, not one structural unit)—except *within one command's
attached arguments*, where a newline is inert whitespace and only the width fill
breaks (otherwise a fill-broken argument would read as a new statement on the
next pass and never reach a fixed point). A single inserted space at any
preserved token boundary keeps re-lexing from merging two tokens.

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
fires identically whether source newlines are statement boundaries
(`SplitAtNewlines`) or inert catcode-9 whitespace within one command's attached
arguments (`Statements::Ignore`). This is what gives an *attached* brace
argument the l3 hang: `\hbox_set:Nn \l_tmpa_box` / `␣␣{ … }`, or the `T`/`F`
branches of `\cs_if_exist:NTF` each hung one step. A directly-abutting argument
(`\EditInstance{a}{b}{…}`, no space) leaves `atom` non-empty and stays K&R-glued
instead—the same `atom` emptiness discriminates space from glue.

**Head-hug.** A detonating *non-group* child (a command subtree whose first line
is a head atom—e.g. the `N`-argument
`\__kernel_dependency_version_check:nn{T}{F}` of `\cs_if_exist:NTF`) that
follows a head on the current line, space-separated, is kept on that line by an
`Ir::group_hug` wrapping `[head, sep, block]`. The hug is rest-aware (`fits`
stops *successfully* at the block's first forced break), so it measures only the
prefix `head␣<block-first-line>`, never the block body—the issue-#71-safe
measurement, deliberately not the `step_fill` local `flat_width` cascade that
would split a short head off a detonating trailing block.

**Sibling coupling.** Within one command's attached arguments (`Ignore` only),
if any brace argument detonates on a *forced* break—a docstrip guard, comment,
or `.dtx` margin (`expl_group_forces_break`, a cheap token scan, no
lowering)—every brace sibling is forced to the broken (Allman) form via
`lower_expl_group`'s `force_break`, so a short false-branch
(`{ \tex_endinput:D }`) expands to match a multi-line true-branch. Keyed on the
*forced* trigger only, so the coupling is a pass-stable function of the arg-list
content; a sibling that would break solely from **width** does not couple (width
is a printer decision, invisible at build time).

### Sticky-break statement fills

Every expl3 statement line is committed as an `Ir::StickyFill`, not a plain
`Ir::Fill`. Both greedily fill atoms across the width; the difference is the
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
