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
pre-pass (`formatter::core::expl3_regions`) over the same fixed toggle set the
lexer uses (`parser::lexer::expl_toggle`, shared so the two never drift), stored
as a `Vec<TextRange>` side channel in `LowerCtx`—the same byte-range pattern as
parser diagnostics. The CST, lexer, events, and tree_builder are untouched, so
losslessness is unaffected; the reformatted output is a different valid text
with the same meaning.

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

A *continuation group*—a brace group starting its statement line (a function
body, a `\tl_set:Nn` value on its own line)—indents **one step** under its head
statement (`\cs_new:Npn \foo:n #1` / `␣␣{ body }`, the l3styleguide shape). The
step wraps the break and *the group alone* in one `Indent` (as a folded
statement separator, or a folded fill gap for a mid-statement group): the rest
of the line stays at base, because a width break re-reads its atoms as ordinary
base-indent statements on the next pass—break and group body must land at the
same column either way for the layout to be a fixed point.

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
