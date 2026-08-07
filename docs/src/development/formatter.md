# Formatter

The formatter is the sole authority on layout. It lowers the CST into a
Wadler/Prettier-style `Doc` IR, and a printer lays that IR out under a
flat-or-break fit model.

## The formatter is whitespace-only

The layout engine changes trivia and nothing else: whitespace, newlines,
comments, and `.dtx` margins and guards. It never inserts, deletes, or rewrites
a non-trivia token. Everything else is emitted verbatim. Mechanically, each
maximal run of `WHITESPACE` and `NEWLINE` trivia is replaced by a break
primitive, and the printer computes indentation.

Meaning-preserving content rewrites therefore do not live here. Stripping
redundant braces around a single-token script (`x^{2}` → `x^2`) and rewriting
`$$…$$` → `\[…\]` are linter autofixes. This mirrors the fix-then-format rule:
just as the formatter never runs inside `--fix`, content rewrites never run
inside `format`. The payoff is a guarantee by construction, checked by the
non-trivia-content oracle in `assert_format_invariants`, instead of a
meaning-preservation argument defended one fixture at a time.

One subtlety. The formatter may still change CST *shape*. The parser's [math
operator split](parser.md#math-operator-atoms) re-groups a catcode-12 `WORD`, so
inserting insignificant math whitespace (`a+2` → `a + 2`) makes the output
re-lex into separate atoms. The oracle compares the concatenated text of
non-trivia tokens rather than their boundaries, so it tolerates the re-grouping
while still catching any inserted or deleted non-trivia character.

## Line endings

The printer always builds output with `\n` and is the sole authority on where
breaks go. `FormatStyle::line_ending` decides only how those breaks are spelled,
as a pass over the finished text:

- `auto` (the default) uses whatever the source used: CRLF if the document's
  first line break is `\r\n`, LF otherwise.
- `lf` and `crlf` are unconditional.
- `native` is CRLF on Windows and LF elsewhere.

`auto` is the default so that formatting never rewrites a file's line endings
behind the author's back. A CRLF repository does not get a whole-file diff the
first time it is formatted, and `format --check` does not flag every file.
Detection walks the tree's text chunk by chunk rather than materializing the
document as a `String`; `\r\n` is a single `NEWLINE` token, so the pair cannot
straddle a chunk boundary.

The pass is applied at the three entry points every other one routes through:
the two `format_node…_sentence` entries and the BibTeX `format_node`. A range
format must match the endings of the text it splices into, and a selected block
holding no line break of its own cannot answer for itself, so detection always
reads the whole document.

This is the one carve-out in the protected-regions rule. A `verbatim` or
`lstlisting` body is emitted from source token text, so before `line_ending`
existed a CRLF document came out with CRLF inside the protected region and LF
everywhere else. The conversion is therefore document-wide. Only the `\r\n` and
`\n` pair is touched; a lone `\r`, which the parser also lexes as a line break
but which can only reach the output through a protected region, is left exactly
as authored.

Idempotence survives because the conversion is a fixed point, and the
non-trivia-content oracle is unaffected because `NEWLINE` is trivia.

## Trivia-invariant layout

Whitespace-only says what the formatter may write. Trivia-invariant layout says
what the lowering may read:

> Layout is a function of non-trivia content, config, and only those trivia
> predicates the formatter itself preserves.

A predicate `P` is preserved when `P(fmt(x)) == P(x)`. Reading a preserved
predicate is safe, because the formatter cannot change the answer. Reading an
unpreserved one means pass 1's layout silently edits pass 2's input.

Three predicates are preserved and may be read: whether a blank line is present,
whether a comment is present and whether it is own-line or trailing, and whether
a `%` margin or `%<…>` guard sits at column 0. One is not, and must never be
read: whether a gap is a lone newline or a space. The formatter converts freely
in both directions, turning `alpha\nbeta` into `alpha beta` and writing a
newline where a width wrap needs one.

### Why this makes idempotence a theorem

The formatter changes only trivia, so `fmt(x)` is a trivia-perturbation of `x`.
If layout is invariant under trivia perturbation, `fmt(fmt(x)) == fmt(x)`
follows. Idempotence stops being an empirical property defended one decision at
a time.

The alternative does not scale. Every layout decision that reads the unsafe
predicate is an independent latent bug, and the supply of decisions is
unbounded. The whole K&R-versus-Allman family of bugs is that pattern: a soft
width break becomes a hard statement boundary on the reparse,
`contains_forced_break` flips, and the layout flips with it.

This is not parse stability, which we deliberately decline. The math operator
split re-granulates a `WORD` from non-trivia content and reads no trivia at all,
so it is unaffected.

### Two tiers

By default the lowering must not be able to read the unsafe predicate, and the
enforcement is to delete the information at the boundary. The lowering consumes
a normalized inter-token gap

```
Gap = Glued | Space | BlankLine | Comment(..) | Guard(..) | Margin(..)
```

with no `Newline` variant, rather than raw trivia tokens. A rule cannot key on
what it cannot see, which is the only form of this constraint that survives
contact with a large codebase.

Some modes are *defined* by reading authored breaks: `WrapMode::Stable`,
`Sentence`, and `Semantic`, and `ReflowKind::Statement`, where a lone newline
ends the line so `\draw …;` lists keep one statement per line. These take a
widened gap, and each owes a written fixed-point argument showing that every
layout it can emit re-reads to itself.

`ReflowKind::Statement` is the model: its continuation is flush, so a
width-wrapped tail re-parses as a line already at the body indent and lays out
identically. expl3's structural statements reach the same end differently, by
re-deriving a call unit from content on every pass, so a width wrap anywhere
inside a unit re-consumes to the same unit.

Four sites still read the unsafe predicate. Three are opt-in modes with written
arguments: `RunAtom::preferred_break_before` (the stable/sentence family),
`ReflowKind::Statement`, and the expl3 fallback statement, described under
[statement boundaries](#statement-boundaries-are-structural). The fourth,
`spans_multiple_lines` in `core.rs`, is accidental residue governing
block-versus-inline choice for multi-line brace `Opaque` groups outside expl3
regions; it is filed in `TODO.md`.

Statement boundaries for recognized expl3 heads are not on that list; they are
structural. Neither is the optional argument `[…]`, which used to share
`spans_multiple_lines` with the brace case and is now a group over its top-level
entries, so where the author broke the line decides nothing there.

### The oracle

`formatter::perturb::trivia_perturbations` generates TeX-identical trivia
perturbations of each input, swapping a lone newline for a space and back
wherever the swap preserves meaning. One generator feeds two oracles.

The convergence oracle, which gates today, requires every perturbed variant to
format to a fixed point whose output parses cleanly, round-trips losslessly, and
carries the same non-trivia content. That is strictly stronger than idempotence,
which only ever exercises the single trivia configuration `fmt` itself produces.
A layout hybrid needs a corpus file to land on exactly the right column
arithmetic, whereas the perturbations synthesize those configurations directly.
Deliberate preservation of authored breaks passes by construction, so a failure
is always a real bug, and the check is valid under every wrap mode: it validates
the opt-in modes' fixed-point arguments empirically rather than exempting them.

The strict-invariance oracle, `fmt(perturbed) == fmt(original)`, is the full
contract and the end state. Until the lowering stops reading the unsafe
predicate entirely, it fails wherever the formatter preserves an authored break,
so it gates nothing yet.

Per file the generator emits two bulk variants (every eligible lone newline to a
space, and every eligible single space to a newline) plus a few deterministic
single-flip reproducers. Eligibility encodes meaning safety only, never layout
ownership: blank lines, comment-adjacent gaps, margined and guarded `.dtx`
lines, and verbatim neighbors are never touched. Each variant is verified after
the fact for a clean parse, identical non-trivia content, and an identical
trivia-blind CST skeleton, so newline-sensitive parser shape gates are dropped
and counted rather than polluting the inventory. The convergence oracle runs as
`badness debug format --checks trivia`, with wrap pinned to `reflow` so an
explicit `--wrap preserve` cannot make it converge trivially, and inside the
corpus sweep in `tests/format.rs`.

## The Doc IR

The IR is `formatter::ir::Ir`. Its core variants are `Group`, `Line`,
`SoftLine`, `HardLine`, `EmptyLine`, and `Indent`, plus `Fill` (per-gap greedy
break decisions) and `PreferredFill` (source-break-aware global minimum-cost
decisions) for paragraph reflow. It also carries `Align`, `IfBreak`,
`ConditionalGroup`, `Verbatim`, `ColumnZero`, `MarginPrefix`, and `Nil`.

Between lowering and printing, `Ir::propagate_breaks` saturates every non-hug
group's `expand` flag from its content, so the flag becomes the single
representation of "forced open".

### `Mode::Flat` is an honest contract

The printer's layout mode is a verified claim, not a hint. A producer may
dispatch a subtree in `Mode::Flat` only after verifying that the subtree's whole
flat rendering fits, every line of it for subtrees that structural `HardLine`s
split. Consumers trust the claim, so a group or conditional group dispatched
flat honors it instead of re-deciding, and the parent's verification and the
print agree by construction.

A producer that cannot make the whole-subtree claim dispatches `Break` and lets
children decide for themselves. Choosing a conditional-group candidate by
first-line fit is one such decision, and a `PreferredFill` break plan places
atoms without verifying them, so those atoms inherit the fill's mode. Both
dispatch decisions measure from `Writer::current_col()`, the column the next
visible character lands at counting a pending indent, because a fit verified
from the wrong column is still a lie, and one the honest contract would then pin
rather than let nested re-decisions paper over.

Two calibrated exceptions keep the contract honest at its edges.

An `expand` group prints `Break` even under an incoming `Flat`. A subtree
carrying a hard break was never a flat claim, since its `HardLine`s fire in
either mode, and pinning its `Line`s flat would glue content onto a forced-open
line. This needs the saturated flag.

A trailing-block hug claims only `Mode::FlatPrefix`. Its prefix measurement
stops *successfully* at the block's first forced break, so only the prefix was
verified. Trivia-level layout renders flat exactly as under `Flat`, which is
what keeps the head glued, but a group may sit past the break the measurement
stopped at, so groups under `FlatPrefix` re-decide. In
`\@@_if_key_value:VTF {T}{F}` the hug verified up to `T`'s detonation; `F` was
never measured, and pinning it flat printed a 125-column line.

The honest contract is what finally delivers the trailing-hang rule's stated
intent. An all-lines candidate is verified by rendering the whole subtree flat
and checking every line, so the chosen candidate's nested groups keep exactly
the measured layout in the real print instead of re-deciding against the rest of
the line and detonating into a hybrid the measurement never saw.

### A group's fit measures the rest of the line

Deciding a group dispatched in `Break` mode measures its flat rendering plus the
already-queued commands up to the next line break, the Wadler "fits the rest of
the line" rule. A group dispatched flat skips the decision entirely.

A later group in that rest is measured in the mode it will actually print in:
flat when its own flat form still fits from here, broken otherwise. Measuring a
doomed group flat charges the group being decided for width that will never land
on this line, and the charge depends on where the doomed group's own body
happens to break, which the previous formatting pass decided.

That is a fixed-point hazard rather than a cosmetic one. In
`\EditInstance{block}{thm}{␣…long keyvals…␣}` inside an expl3 region, pass 1
broke `{block}` and `{thm}` onto their own lines because the trailing block
measured flat and overflowed, and pass 2 kept them inline because that block had
by then acquired a hard break. Deciding the rest group locally makes both passes
agree, and gives the trailing block the hug the expl3 lowering always intended:
short leading arguments stay inline and only the over-long trailing one
detonates.

The same rest-awareness extends to a fill's last atom. The lowering glues
trailing content after a statement's fill, so the last atom's flat claim has to
survive the rest of the line too. Without it the atom's folded hang break is
never taken: the atom alone fits, goes flat, and the glued tail overflows a line
the measurement never saw.

### Two measurement walkers

Every fit decision is served by exactly two walkers in `printer.rs`, each
carrying a small explicit policy. They replaced five overlapping predicates
whose hand-copied traversals had drifted apart.

`flat_end` simulates a subtree laid out flat and returns the column it ends at,
or `None` when the measurement failed. Its `FlatMeasure` policy names three
deliberate readings. `Footprint` is the unbounded flat width: a single-line
comment counts, since it shares the line and forces a break only after, and
`expand` is ignored. `Fits` asks whether the subtree lies fully flat within the
width, so any forced break or `expand` group fails. `HugPrefix` is the
trailing-block hug's claim: a forced line break stops the measurement
successfully, a comment still fails it, and an atom no break could rescue is
excused from overflow.

`line_fits` walks a pending work list, mode-aware, up to the first newline that
would actually be emitted. The conditional-group picker's probe and the
queued-commands adapter behind `group_fits` are thin seeds over it. Its one
policy knob records the single deliberate difference between those contexts: a
candidate carrying a standalone comment can never render flat, while a comment
in the rest of an already-committed line is there either way and counts its
width. Each work item also carries a verified flag, so the measurement honors
the honest flat contract exactly as the run loop does.

Sharing the traversal dissolved the drift. A later group in the rest is now
decided with its own hug flags, a later conditional group through the candidate
picker rather than assumed flat-most, and a `Break`-mode preferred fill by its
first atom rather than measured whole-flat. All of it measured behavior-neutral
on the gate corpora, because mode propagation had already removed every
reachable disagreement: a group inside a flat parent is never asked whether it
fits.

## Paragraph line breaks

Paragraph line breaks are controlled by `WrapMode`, modeled on the sibling
[panache](https://github.com/jolars/panache) formatter and mechanized through
the `Doc` IR rather than a separate line filler. All five modes are implemented.

`Reflow`, the default for every file kind, width-fills.

`Stable` keeps acceptable authored breaks while optimizing overflow, underflow,
change, displacement, and raggedness against a soft target
(`FormatStyle::stable_wrap_target`, `line-width - 15`, not yet configurable).
Its completed prose runs render through `PreferredFill`, so an
already-reasonable paragraph is left close to how it was authored.

`Preserve` keeps authored breaks.

`Sentence` and `Semantic` split one sentence per line and ignore width, through
the shared `reflow_elements` engine. `Semantic` additionally ends a line at
every authored newline, with no clause detection.

Sentence-boundary detection is a per-language abbreviation profile in
`formatter::sentence`, ported from panache, resolved from `[format] lang` and
`[format.no-break-abbreviations]` into a `SentenceOptions` threaded on
`FormatContext`. Babel and polyglossia auto-detection is deferred.

The `\\` line break, with a tightly bound `*` or `[len]`, is grouped by the
parser into a `LINE_BREAK` node, so the formatter sees `\\[2ex]` as one unit.

### Reflow is safe by construction, not by file kind

`WrapMode` used to be resolved per file extension, with `.tex` and `.bib`
reflowing while `.sty`, `.cls`, `.dtx`, `.ins`, and `*.code.tex` fell back to
`Preserve`. That default is gone. Whether content is safe to reflow is a
property of the content, not of the file name, and answering it by extension
left `--wrap reflow` on a `.dtx` free to corrupt the document.

The safety is now structural, and every gate below is independent of the wrap
mode, so an explicit `--wrap reflow` is exactly as safe as any other mode.

Every relayout arm in `lower_node` (environment, math, multi-line group,
optional argument, and a command with a managed argument) refuses a node whose
subtree carries a `DOC_MARGIN` or `GUARD`. Reflowing a managed argument breaks
its body onto fresh lines, which drops the `%` margin, and on an unmargined line
a `^^A` doc comment re-lexes as content, so the layout stops being
whitespace-only and pass 2 no longer parses.

The margin-escape detector is the residual backstop. Under
`ReflowKind::DtxProse` the per-line `DOC_MARGIN` is dropped and a canonical `%`
re-emitted. A forced-break block amid the prose whose interior lines all ride
their own column-0 margins is committed raw with the canonical margin
re-attached for its first line, and a column-0 guard whose physical line can be
isolated becomes its own unmargined single-line segment. Both stay inside the
layout, so the surrounding prose keeps reflowing. What remains escapes: a block
with an unmargined interior line, one opening on an unmargined line, or a guard
line that cannot be isolated sets `LineBuilder::margin_escaped`, and
`lower_dtx_doc_paragraph` re-lowers the paragraph on the byte-faithful preserve
path.

`is_dtx_doc_paragraph` reads the paragraph's *first* content token, descending
into child nodes. Walking only direct child tokens skipped an opening command
and read a margin from a later line, so a guarded `%<package>\def\x{1}` was
wrapped in a `%` margin that commented the code out.

Doc-margined runs between expl3 regions are never reflowed as generic prose.
`lower_expl_paragraph` splits a paragraph at the toggles, and an out-of-region
run carrying a margin or guard is documentation-layer text riding `%` margins
and margin-framed `macrocode` frames. A run that opens on a margined line, sits
under no enclosing environment, and contains at most margin-framed chunks does
reflow under the `%` margin: the doc prose rewraps while each chunk commits raw
behind its byte-exact source frame lead, since docstrip matches the
`%    \begin{macrocode}` line literally and a frame lead must never be
normalized to the canonical `%`. Any run the gate declines takes the
byte-faithful element stream in every wrap mode.

Two adjacency rules keep package code looking like package code under the new
default. A forced-break block glued to the command run in progress
(`\newcommand\cls@hook{%`) hugs it, since the source offered no break
opportunity there. And content still on a block's last physical line
(`\input docstrip.tex`, where a leading `%%` comment bound to `\input` and made
it a block) rides that line, the same rule the trailing `%` already had.

## Optional-argument layout

An optional argument is a plain Wadler group over its top-level comma-separated
entries: flat when it fits the width, one entry per line when it does not.

```latex
\usepackage[unicode, colorlinks, linkcolor=blue, citecolor=green]{hyperref}

\usepackage[
  unicode,
  colorlinks,
  linkcolor=blue,
  citecolor=green,
  urlcolor=magenta
]{hyperref}
```

Width alone decides. There is deliberately no "expand once the list has more
than N keys" rule and no knob for one: the group is already a pure function of
content and width, and a count threshold would need the comma count to proxy for
keyval-ness, exploding a comma-rich textual optional. Nor is there a Black-style
magic trailing comma, since content steering layout conflicts with the
sole-authority tenet.

The flat rendering is byte for byte what the older collapse produced, so
`\foo[a=1,\nb=2]` still comes out `\foo[a=1, b=2]`. What is new is that an
over-long bracket expands instead of silently overflowing, and that neither
outcome depends on where the author happened to break the line.

A body that is not safely segmentable, because of a blank-line `\par`, a `%`
comment that must end its line, or nested content carrying a forced break, falls
back to the indented block form. With no split point at all the bracket stays
inline and is allowed to overflow, since a breakable group would push
`[!htb]`-shaped brackets onto three lines to no gain. Padding at the body's
edges (`\baz [ me ]`) rides the flat rendering through an `IfBreak` and vanishes
when the delimiters take their own lines.

### Which commas are break opportunities

There are two kinds of split point, and they are not equally free.

A gap split is a comma the author already followed by whitespace. Flat it is a
space, broken a newline, which is the whitespace-to-newline exchange that is
TeX-identical anywhere. It needs no permission and applies to every `[…]`.

A glued split cuts inside a `WORD` at a comma with nothing after it
(`xmin=-5,xmax=5`). Broken, that materializes a space token TeX will see, so it
is emitted only for an argument the signature database proves is a key-value
list (`ContentKind::Keyval`). Its separator is a `SoftLine` rather than a
`Line`, so a bracket that fits stays byte-identical to the source and the space
appears only on the line the split created.

The distinction is not stylistic. Compiling both spellings and diffing the
typeset output splits cleanly along keyval-ness.
`\documentclass[a4paper,twoside]`, `\usepackage[english,french]{babel}`,
`\includegraphics[width=1cm,height=1cm]`, `\draw[thick,red]`, and
`\begin{lstlisting}[caption=one,label=two]` are all identical with and without
the space. `\item[red,green]`, `\newcommand{\x}[1][alpha,beta]`, a
`\caption[short,list]` list-of-figures entry, and `\cite[see,also]` all differ.

keyval, xkeyval, pgfkeys, and l3keys all strip spaces around entries, and LaTeX
runs `\zap@space` over class and package option lists. A textual optional
typesets them. So `Keyval` must never be set on an argument whose content is
typeset; hold it to the same curated standard as the math-environment routing.

The flag comes from two tiers. The CWL corpus marks it per argument, inline in
the placeholder name (`\begin{axis}[options%keyvals]`), and the generator
preserves that as `{"kind": "opt", "content": "keyval"}`. Unlike the `#V`,
`#\math`, and `#L0` classification suffixes, which are behavior claims the bulk
tier deliberately refuses to trust, this is a mechanical per-argument fact, and
it agreed with every row above. Curated `signatures.json` entries mask the CWL
tier wholesale, so the ones that need the flag (`\usepackage`,
`\includegraphics`, `\documentclass`, `lstlisting`, `minted`) carry it by hand.

Two details the segmentation has to get right. A comma is a split point only at
bracket depth 0: the parser closes an `OPTIONAL` at its first `]`, so a stray
`[` inside the body opens a region that never closes and everything after it
stays glued, which is what keeps `\foo[a=[1,2]` from breaking at the `1,2`. And
the lexer ends a `WORD` at every control sequence, so a key list routinely hands
the splitter a word that opens with the comma closing the previous token's entry
(`width=`, `\figurewidth`, `,xmin=-5,…`). That comma is a real split point, not
the empty entry a leading comma would otherwise look like.

### Why the split lives here and not in the parser

`is_word_char` excludes only TeX's special characters, and a comma is catcode 12
"other", indistinguishable from `=`, `-`, or `5`. Lexing `xmin=-5,xmax=5,` as
one `WORD` is the correct generic-TeX reading, and splitting on `,` would encode
"this is a keyval list" into the lexer.

The sub-`WORD` precedent, the math operator split, does not transfer. It is
licensed by a static fact the parser can see, that we are in math, plus a safety
property, that TeX ignores spaces in math. Neither holds in a bracket, whose
content is arbitrary text. So the comma split belongs in the formatter, gated on
a semantic fact.

## Display-math line breaks

Display-math line breaks have their own knob, `MathWrap` (`auto`, `preserve`,
`single-line`, `break`), scoped to single-formula display bodies: `\[…\]`,
`$$…$$`, and non-grid `equation`. Grids and inline math are untouched. `auto`,
the default, resolves against the effective `WrapMode` at `LowerCtx`
construction, so `Preserve` preserves authored breaks and anything else uses the
amsmath-style breaker, and one `wrap` setting carries over to math for free.

A body whose first non-trivia atom is a `\label{…}` splits that label onto its
own line and starts the formula fresh below it, since the label is equation
bookkeeping rather than part of the math. This runs under every `MathWrap`
policy, applied before the mode dispatch and then recursing so the remaining
formula lowers under its own policy, so `single-line` and `preserve` bodies
split the label too.

The split is otherwise deliberately narrow. It fires only on a leading `\label`,
so a trailing one stays glued to its line and the rule stays out of
`aligned`-style bodies, and it is keyed to the single `\label` command by name
rather than a wider bookkeeping set. A body that is nothing but a label stays on
one line rather than gaining a dangling break.

## Math operator spacing

Operator atoms are produced by the parser's [math operator
split](parser.md#math-operator-atoms). Their spacing is a formatter concern: a
single space around each binary and relation atom, with unary signs and scripts
tight.

## Table column alignment

Table column alignment is layout, so the formatter owns it. The `{lcr}` column
spec is parsed by `formatter::colspec` into per-column alignments, reading only
the static argument text. It is conservative, bailing to all-left on any token
it does not model. `p`, `m`, and `b` count as left, `*{n}{}` expands, and `>{}`,
`<{}`, `@{}`, `!{}`, and vertical rules add no column.

The grid renderer aligns each cell left, center, or right. A right- or
center-aligned last cell pads on the left only, so there is no trailing
whitespace and idempotence holds. A `\multicolumn{n}{spec}{…}` spans `n`
columns: it is excluded from single-column widths, aligned within its span by
its own spec, and left to overflow rather than ballooning narrow data columns.
The rule-line recognizer tolerates the booktabs `\cmidrule(lr){2-3}` paren trim,
consuming the `(lr)` word and the detached `{2-3}` group as part of the rule
line, and a same-line `\\ \hline` is normalized onto its own passthrough line.

### Which environments grid-align

Routing to the grid is primarily semantic. The curated `align` signature
(`tabular`, `array`, and every math grid) picks the grid path, and the parallel
`math` flag additionally routes the math-aware lowerer so cells get role-aware
operator spacing.

But the signature database cannot name a user-defined environment, so an
environment shaped exactly like an alignment would otherwise miss the grid.
After the curated arms have had their say, one more arm routes any remaining
environment whose body carries a top-level `&` to the non-math grid. A `&` at
catcode 4 is a column tab, a static CST-shape fact read exactly as the grid
builder defines a cell boundary: a direct child of the body or of its single
wrapping paragraph, so a nested `&` lives in a child node and stays invisible.
It is the same move the [environment group-boundary
gate](parser.md#the-environment-group-boundary-gate) makes, generalizing a
curated set to the package code it cannot name.

Three properties keep it safe. It keys on `&` and never on `\\`, because a
`\\`-only body is a line stack rather than a column alignment and gridding an
arbitrary `\begin{center}a \\ b\end{center}` would reflow it. It is
whitespace-only and self-correcting: the grid renderer touches only trivia, and
any shape it cannot lay out on aligned rows falls back to the plain indented
body. And it is placed after the curated arms, so a stray top-level `&` inside
an `itemize` body never reroutes it. Doc-margined bodies are excluded, since
grid padding would push a `%` margin off column 0.

An unknown environment takes the non-math grid, so its `&` columns align and
gain the single `" & "` spacing, but its cells do not get math operator spacing,
because the parser never entered math mode for it. `a&=b` becomes `a & =b`, not
`a & = b`. Inferring math mode for an unnamed environment is exactly the meaning
the parser declines to guess.

## expl3 code formatting

The expl3 letter mode is a lexer fact; see [expl3 regions are macro
code](parser.md#expl3-regions-are-macro-code). The matching whitespace catcodes
are a formatter concern: inside a region, source spaces and tabs are catcode 9
(ignored) and `~` is catcode 10 (a literal space). Because inter-token
whitespace is provably insignificant there, the formatter owns the layout of
in-region code, indentation and line breaks alike, regardless of `WrapMode`.

This is idempotent by construction. The inserted whitespace is itself
catcode-insignificant, so re-lexing the output yields the same token sequence
and the deterministic layout is a fixed point. It is the property a generic
hanging continuation indent could not get, supplied here at the catcode level.

Region membership is not recorded in the CST. The lexer's toggle is transient,
and the formatter recomputes in-region byte ranges in a read-only pre-pass over
the same fixed toggle-name set the lexer uses, storing them as a
`Vec<TextRange>` side channel on `LowerCtx`, the same byte-range pattern as
parser diagnostics. The CST, lexer, events, and tree builder are untouched, so
losslessness is unaffected; the reformatted output is a different valid text
with the same meaning.

### House style

The target is the LaTeX Project's own house style, "The LaTeX3 kernel: style
guide for code authors" (`l3styleguide.tex` in
[latex3/latex3](https://github.com/latex3/latex3), LPPL). Its mechanical,
formatter-enforceable rules are:

- **R1** lines under 80 characters where possible
- **R2** a two-space indent per level of code
- **R3** single spaces between everything, except "simple runs of parameter
  (`{#1}`, `#1#2`)", which stay tight
- **R4** each conceptually separate step on its own line
- **R5** canonical brace layout: the body `{` on its own line at +2, the body at
  +4, `nTF` branches at +6, nested groups at +8
- **R6** related variants may optionally be aligned
- **R7** no tabs

The guide's own worked example is the reference:

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

The formatter satisfies R1, R2, R6 (it normalizes any alignment away, which is
permitted since R6 is optional), and R7. R3 is the spacing rules below, and R4
and R5 are the conditional-break rule. The brace-column progression falls out of
the nested `Ir::indent` that the hang-group rule and the conditional lowering
emit.

There is no path divergence between file flavors for genuine expl3 code. `.sty`,
`.tex`, and a `.dtx` `macrocode` body all route through `lower_expl_paragraph`
and `lower_expl_code`, and the `.dtx` margin frame's column-0 base composes with
the same in-region rules, so identical code lays out byte-identically under
either flavor. The "body `{` at column 0" that a bare `macrocode` block shows is
generic LaTeX lowering: a `macrocode` with no toggle is by design not an expl3
region, since region detection keys on the toggle rather than the environment,
so its body never reaches `lower_expl_code`.

The style guide's non-layout rules, such as naming prefixes, `:D`-primitive
discipline, and expandability, are out of scope. They are meaning rather than
trivia, and belong to a linter.

### The space before an attached argument

R3's positive half applies outside the brace as well, so an expl3 function's
argument written flush against its head is respaced: `\clist_count:n{#1}`
becomes `\clist_count:n {#1}`. `lower_expl_code` synthesizes the gap by flushing
the atom exactly as a source gap would, so every branch below it sees the state
the spaced spelling already produced and there is no second spelling to
special-case.

The trigger is the same purely lexical fact used elsewhere: the parent command's
name contains `_` or `:`. It is deliberately not subject to the
simple-parameter-run exception below, which governs a group's inner padding
rather than the gap before it. l3kernel writes the space either way; a sweep of
its `.dtx` sources counts 2883 spaced against 9 glued for parameter-run
arguments, and 8447 against 14 over all expl3-named heads. An embedded 2e-named
command keeps its authored gap (`\eqref{#1}`, `\ProvidesExplPackage{demo}{…}`),
where upstream is genuinely mixed and the house style does not reach.

Inserting the space is a trivia edit and catcode-safe: in-region source spaces
are catcode 9, so the token stream is unchanged, and a real space is `~`. This
is also why it cannot resurrect the classic `\def\foo#1 {…}` delimiter hazard,
since the inserted space never becomes a token. Junk-glued statements are
exempt, their authored line shape being load-bearing.

### Simple parameter runs stay tight

The default for an expl3-named command's brace argument is the canonical inner
space, `{ value }`. A group whose body, ignoring outer padding, is a run of
adjacent parameter tokens (`{#1}`, `{#1#2}`, `{##1}`) stays tight instead,
overriding the command-name rule. A padded `{ #1 }` therefore normalizes to
tight `{#1}`, but whitespace between the parameters (`{ #1 #2 }`) or any
non-parameter token (`{ X #2 }`) keeps the inner spaces, which is exactly the
discrimination the guide's example draws.

The gate reads only token kinds and single-digit index text, so no signature or
meaning is involved, and the removed padding is catcode-9 whitespace, so the
rewrite is trivia-only and idempotent by construction.

### Conditional branches break structurally

R4 is enforced for expl3 conditionals by `lower_expl_conditional`. A
statement-leading conditional, one that starts its logical line with nothing
accumulated before it, explodes unconditionally: the head and any leading
arguments on one line, then each `T` or `F` branch on its own line hung one
indent step, +6 inside a +4 body, with a multi-line branch nesting its interior
+8. It breaks even when the whole conditional would fit on one line, because the
guide's example puts a short true branch on its own line beside a multi-line
false branch, treating the branch list as structure rather than a width fill.

The conditional is recognized from the argspec after the final `:` in the
command name: if it ends in a run of `T` and `F`, that gives the branch count,
so `:nTF` is 2 and `:nT` or `:nF` is 1. In an expl3 argspec `T` and `F` mark
only the true and false slots, so this is exact, and it is a purely lexical fact
of the name, since the whole name lexes as one `CONTROL_WORD` in-region. Each
branch is lowered as a soft group, so a short branch stays `{ … }` inline on its
line while a long one breaks internally.

Two scopes keep it precise. A conditional used mid-line as a value
(`,key = \tl_if_empty:nTF …`) is not statement-leading and stays on the
width-driven path. And a conditional whose branch groups do not attach to it, an
`:NTF` or `:nNnTF` whose single-token argument breaks greedy brace attachment
and leaves the branches on a following sibling, falls back to the width path
rather than mis-lowering a partial shape.

All-or-nothing is a property of the node, not of statement position. Both scopes
above are positional, and both are additionally gated off inside a fallback
statement. Left to the plain fill, a conditional's branch groups are independent
atoms, so an overflow hangs only the last one:

```tex
  ,begin-vspace:e = \tl_if_empty:nTF {#2} { \newtheoremstyle@vspace@default }
    {#2}
```

The two branches of one conditional sit at different indents, and the second
reads as a continuation of the key-value entry rather than as the false branch.
So `lower_node`'s in-region command arm wraps any recognized conditional in a
group choosing between the fill and the exploded form: flat when the whole call
fits, otherwise the explosion. That covers the fallback case the positional
paths decline, and it carries none of their pass-invariance question, since
whether a name sits trailing depends on where a fallback line's junk ends but
the node is the node on every pass.

Annotated branches stay on the path. A `%` among the branch children does not
force a fallback: a comment trailing a branch rides that branch's own line, and
an own-line one keeps its own line between branches. Own-line-ness is a
preserved predicate, so reading it is trivia-invariant and stable in both
directions, and relocating either way would rebind the comment under the trivia
attachment rule. Bailing to the width path instead was the real trigger for a
reported bug: one `% You might prefer \nobreakspace to ~` between two branches
dropped the whole call off the exploded shape, and the width path, then still
coupling siblings, blew every argument including `{ > }` and `{ 1 }` into a
three-line block. A comment after the whole call is not a child of the command,
so it leaves a fitting conditional flat.

The statement-leading break is width-independent, so it is a fixed point: the
exploded output re-parses to the same greedy command, since brace arguments
attach across the inserted newlines, in statement position, and re-explodes
identically. A LaTeX2e conditional such as `\@ifpackageloaded` has no
`:`-argspec and is never matched.

The mid-line value scope is itself width-conditional, and that is where
idempotence gets subtle. A trailing conditional, with head atoms before it and
only trivia after it in the statement, is not pass-stable if it simply joins the
sticky fill. When head and conditional overflow, the fill drops the head to its
own line, which on the next parse makes the conditional statement-leading, so it
then explodes unconditionally and the two passes disagree. `lower_expl_code`
therefore commits head and conditional as one group decided by the group's flat
width, head included, so it is neither fooled by a branch that detonates
internally nor evaluated apart from its head. If it fits, head and conditional
share a line; if it overflows, the head takes its own line and the conditional
explodes, which re-parses statement-leading and re-explodes to identical bytes.

### Positional gate on layout ownership

The shared toggle-name set is necessary but not sufficient to open a region. The
formatter additionally requires the toggle to be a top-level statement, because
the catcode-9 whitespace assumption only holds where TeX actually executes the
toggle at load. A toggle spelling that is never run is a false positive, and
mis-owning its layout rewrites real space tokens even though the byte-level
losslessness and idempotency oracles stay green. Two shapes are rejected.

In definee position, the toggle command's immediately preceding non-trivia
sibling is a `\def` or `\let`-family primitive, so the toggle is the control
sequence being defined rather than executed. `l3kernel/expl3.sty`'s loader
writes `\protected\def\ProvidesExplPackage{…}`.

Nested in a group or definition body, an ancestor of the toggle's command is a
`GROUP` or `OPTIONAL`, so the toggle is tokenized into a replacement text and
only ever executed, if at all, when that macro runs.

This deliberately splits the shared-toggle-set invariant. The name set stays
shared between lexer and formatter, so a new spelling is recognized in both, but
the positional layout-ownership rule is the formatter's alone. The lexer keeps
the naive name-only model on purpose: mis-lexing a name in letter mode only
splits CST tokens, which is lossless and cosmetic, whereas mis-owning layout
rewrites meaning, so only the higher-stakes side gates.

### Statement boundaries are structural

Statement boundaries are call units, not source newlines. A pure shape scan,
`semantic::expl3::segment_expl_statements`, runs over each in-region element
stream before layout and decides per gap whether a statement ends there, and the
layout loop commits logical lines where the map says. The formatter owns
one-call-per-line: authored same-line calls split, authored mid-call newlines
join, and `\cs_new:Npn \foo:n #1 {…}`, several sibling CST nodes, is one
statement however it was authored.

A unit is a head command whose name has derivable arity, with the argspec suffix
read letter by letter (`N` and `V` take one token, `n c v o x e f` one brace
group, trailing `T` and `F` branch groups, `p` parameter text), plus the
elements its slots consume. Consumption draws from the head's greedily attached
children first and then from following siblings, with four load-bearing rules.

Peel-back handles the fact that greedy attachment routinely gives an argument to
the wrong owner: in `\cs_new:Nn \foo:n {body}` the body attaches to `\foo:n`. A
command consumed into a single-token slot has its own attached children pushed
back onto the scan queue for the outer head's remaining slots. Only the head's
argspec ever drives consumption; an argument's own argspec is inert data,
exactly as TeX grabs it.

One token means one character. A single-token slot also takes a bare character,
so the relation in `\int_compare:nNnTF {…} = {1} {T} {F}` satisfies its `N`;
before that the whole conditional degraded to the fallback. The lexer packs a
run of characters into one `WORD`, so only a single-character token qualifies.
Consuming a longer run would take material TeX leaves for the next slot, and
that shape aborts to the fallback instead.

The p-scan ends parameter text at the first explicit `{`, TeX's own static rule,
scanning the flattened peeled order so a delimited text (`#1 \q_stop {body}`)
finds the body wherever attachment put it.

Only preserved trivia is read. A blank line ends the unit where it stands and
the partial unit commits, pass-stably. A comment is transparent to consumption.
A docstrip guard or doc margin aborts to the fallback, since guarded alternative
bodies make arity lie. The region toggles are recognized zero-arity units, so a
region's opening line is structural too.

Whatever the scan cannot resolve degrades to the fallback, which is the authored
physical line: the old newline rule demoted to a per-statement residue. A
completed unit also absorbs trailing same-line junk, such as punctuation, an
unrecognized command token, or a trailing comment, and a junk-bearing statement
renders with every top-level gap unbreakable so the authored line shape
survives.

Within one command's attached arguments there are no statements at all: a
newline is inert whitespace and only the width fill breaks. A single inserted
space at any preserved token boundary keeps re-lexing from merging two tokens.

The fallback carries its own fixed-point argument. Fallback lines commit as
plain greedy fills, so each printed line re-segments to a fallback statement
that re-fills to itself; gaps that could put a recognized head at a printed line
start are unbreakable; and junk-glued statements render with every top-level gap
hard, so their newline-keyed extent can never move. The strict-invariance oracle
cannot gate a stream containing a fallback head, so the convergence oracle
validates the argument empirically.

For recognized heads the K&R-versus-Allman root cause is gone by construction: a
width wrap inside a unit is harmless because the next pass re-derives the same
unit from content and re-runs the same width decisions.

#### Bracket re-attachment is stable in-region

Unit re-formation assumes the CST shape it consumes, meaning which `[…]` groups
attached where, is the same on every pass. Bracket attachment is shape-gated on
trivia facts, so this needs an argument, and it rests on two facts.

Only the `Greedy` and `Forbid` policies are reachable in-region. `Tight` rides
the curated math `\begin`, and inside a region `\begin` and `\end` are plain
commands, so the one policy that demands a directly abutting `[` never gates
in-region code. On `.dtx` doc-margin lines that carve-out lifts, but there the
ordinary environment-header layout preserves flushness in both directions.
`Forbid` never attaches. `Greedy` attaches across trivia, and every bracket gate
and scan treats a space and a lone newline identically, since both reset the
abutment flag and only a blank line bails, so the formatter's free
space-to-newline conversion is invisible to attachment.

The formatter never creates or removes a flush junction before a `[`, which is
the one perturbation the gates could see. The leading-space respace does not
apply to an `OPTIONAL`, the fill never breaks a flush junction because an
authored gap is where the break lands, a math command is lowered as one verbatim
atom so nothing can touch its `[` from inside, and the math sequence never
deletes an authored gap between atoms.

The second-order shape, where an outer `[`'s verdict depends on whether an inner
`[` abuts a command, is covered by the same invariant. A bare flush `[` with a
reachable closer cannot exist, since flush plus reachable implies attached, so
the counting only ever reads junctions inside attached nodes, whose flushness
the formatter preserves.

### Trailing comments

A trailing comment rides its statement line zero-width, rustfmt-style. The line
may overflow, but prose length never re-breaks code, and the comment is never
relocated: moving it would rebind it as the next statement's leading doc comment
on the second pass, changing its attachment. gofmt and rustfmt do not relocate
trailing comments either, and ruff exempts pragma comments from width for the
same reason.

### Continuation groups

A continuation group is a brace group that starts a fresh atom, with nothing
glued before it because some trivia flushed the atom. It indents one step under
its head statement, the l3styleguide shape:

```tex
\cs_new:Npn \foo:n #1
  { body }
```

The step wraps the break and the group alone in one `Indent`, and the rest of
the line stays at base. A width break re-reads its atoms as ordinary base-indent
statements on the next pass, so break and group body have to land at the same
column either way for the layout to be a fixed point.

The rule keys only on the group shape, not on the statement mode, so it fires
identically whether statement boundaries are structural or absent within one
command's attached arguments. That is what gives an attached brace argument the
l3 hang, as in `\hbox_set:Nn \l_tmpa_box` with `{ … }` below it, or the branches
of `\cs_if_exist:NTF` each hung one step. A directly abutting argument
(`\EditInstance{a}{b}{…}`, no space) leaves the atom non-empty and stays
K&R-glued; the same emptiness test discriminates space from glue.

A detonating non-group child that follows a head on the current line,
space-separated, is kept on that line by a head-hug wrapping head, separator,
and block. The hug is rest-aware, its prefix measurement stopping successfully
at the block's first forced break, so it measures only `head␣<block-first-line>`
and never the block body. That is deliberately not the local flat-width cascade,
which would split a short head off a detonating trailing block. Because only the
prefix was verified, a successful hug dispatches its inner as `Mode::FlatPrefix`
rather than `Flat`, so any group past the first forced break re-decides for
itself.

The hug is reached from the forced-break dispatch, which fires only on a
structural statement, because whether a child detonates is not pass-invariant on
a fallback line. There the same placement comes from the line's own fill, which
hugs, so pairs like `\vbox to \Gin@req@height{%` stay joined without any arm
reading the unsafe predicate.

Each brace argument breaks on its own body. A sibling's forced break, whether
from a guard, a comment, or a `.dtx` margin, is none of its business. This is
the style guide's own example, whose short true branch stays inline beside a
multi-line false branch, and l3kernel follows it throughout; a sweep of its
`.dtx` sources counts 338 places where a flat one-line braced argument sits
directly above a broken sibling at the same indent. An earlier sibling-coupling
rule forced every brace sibling to the Allman form when one detonated, and it
was removed after it turned each of `\int_compare:nNnTF`'s five arguments,
`{ > }` and `{ 1 }` included, into a three-line block because one branch held a
comment.

`Ir::propagate_breaks` gives "forced" a single representation. After lowering, a
bottom-up prepass marks every non-hug group whose inner contains an
unconditional forced break as `expand`, with `contains_forced_break`'s exact
semantics: an `IfBreak` shields its branches, a conditional group's flat-most
candidate decides, and every candidate and branch is still saturated inside. The
flag is thereafter the one representation of "forced open" the printer trusts,
and `contains_forced_break` survives as the query the lowering asks about
pre-pass sub-IR.

That makes the mode pin fall out rather than being a hand-made special case. A
group's body laid out as a bare concat inherits whatever mode the caller was
dispatched in, and inheriting `Flat` was the bug: `step_fill` in flat mode lays
every gap flat without measuring, while the groups hanging off those gaps still
decide their own break. The result was the K&R hybrid `\int_set:Nn \l_…_int {`
with the body wrapped below, which is not a fixed point, since the wrapped lines
re-parse as separate statements and the next pass lays the same group out
Allman. `lower_expl_group`'s forced form now differs from the soft form only in
its boundary separator, a `HardLine` instead of a `Line` or `SoftLine`, and
those in-shape hard lines are what the prepass reads to mark the group, which
pins the body's mode to `Break`.

Two carve-outs keep the flag honest. Hug groups are never marked, since their
inner holds a forced break by construction and their break decision is the hug
fit rather than the flag. And two measurements deliberately ignore `expand`: the
flat footprint recurses instead, because a group forced open only by a
single-line comment still has a flat width, and the hug-prefix measurement lets
the content decide, since a nested block's first hard break must stop the
measurement successfully while a prefix comment must fail it, a distinction the
flag cannot carry.

### Trailing hang groups

A trailing greedily hung `{body}`, meaning a brace group after head atoms with
only trivia after it whose body is a multi-command fill, would otherwise flip
between K&R and Allman across passes. A body authored on one source line hangs
K&R on pass 1, with `{` glued and the body's fill wrapping below, but those
wrapped lines re-parsed as several newline-split statements under the older
model, so the body then carried a forced break and the continuation branch
detonated it Allman on pass 2. Structural boundaries have since removed the
re-split for recognized heads; the three-way choice remains for the body-fit
flip a fallback body can still exhibit.

`lower_expl_code` commits such a group as one all-lines conditional group over
three candidates: flat (`head { body }` on one line), Allman-inline (head on its
own line, `{ body }` inline one step under it), and Allman-broken (`{` on its
own line, body a further step, `}` back). The choice is keyed on the body's real
one-line fit rather than its incidental source-line count. The all-lines
measurement forces each candidate's nested brace groups flat against a very wide
probe line, so a candidate is accepted only as a genuine one-liner and never as
a hybrid where an inner group detonated to keep each printed line short. Both
Allman forms re-parse to a head statement plus a statement-leading `{body}` that
the continuation branch re-emits identically, so each is a fixed point. Since
the accepted candidate is dispatched in honest `Mode::Flat`, the real print
keeps exactly the layout the measurement accepted.

Narrow guards keep this off the shapes the ordinary hang path already lays out
stably: a single-command or bare-value body, a body that already carries a
forced break and therefore wants the plain Allman block, and the multi-argument
and conditional-branch shapes whose head this branch cannot measure as one unit,
so intercepting them would detonate a preceding argument group.

### Sticky-break statement fills

Every structural expl3 statement line is committed as an `Ir::StickyFill` rather
than a plain `Ir::Fill`. Both greedily fill atoms across the width; the
difference is the break cascade. In a plain fill each gap decides independently,
so a long word breaks and the following words keep filling, which is correct for
prose reflow. In a sticky fill, once any atom lands on a broken line every later
atom breaks too.

That is what a width-broken sibling needs, and it is the whole of what a sibling
gets: the argument after a detonated one moves to its own line, but is not
itself broken open. When a true-branch block detonates purely from width, the
greedy fill would let a following empty false-branch `{}` glue back onto the
block's short closing line, because at that column two bytes fit. But whether
the block's own body broke hard, from a source newline, or soft, from a width
fill, is not pass-invariant, since the formatter's own reflow turns one into the
other. Pass 1 produced `} {}` and pass 2 put `{}` on its own line. The sticky
cascade defers the decision to the printer's actual column-aware break, so the
empty branch follows the block onto its own line on every pass, and it does so
without exploding any argument into a block.

A fallback or junk-glued line instead commits as a greedy fill that hugs,
`Ir::HugFill`. Greedy packing there is self-fulfilling: each printed line
re-segments to a fallback statement that re-fills to exactly itself, whereas a
sticky cascade would force atoms that fit onto broken lines, a shape the next
pass's shorter per-line statements do not reproduce.

#### A fallback line commits no interior lines

A fallback line has no cascade, so nothing defers the soft-versus-hard decision
to the printer, and the same non-invariance bites through a different door.
`lower_expl_code`'s node dispatch branches on the lowered child's
`contains_forced_break()`, and every arm of that branch reacts by committing the
line. Committing forces each later atom onto its own line, which is precisely
what the sticky cascade produces anyway, which is why structural and
attached-argument streams agree on the two paths. A plain greedy fill does not,
so on a fallback line the two paths render different bytes: the sibling after a
multi-line group glued onto its closing `}` on pass 1 and dropped to its own
line on pass 2. Committing mid-statement also falsifies the plain fill's own
fixed-point argument and silently drops the unbreakable leading space, since a
pending separator is emitted only when the atom is non-empty.

So no arm of that dispatch fires inside a fallback statement, and the predicate
is not read there at all. Nothing is lost, arm by arm. The hanging brace group
takes the soft continuation-hang path either way, since a forced body has no
flat width, so the fill dispatches that atom in `Mode::Break` at every width and
its leading `Line` breaks: the same bytes, minus the line commit. The
abutting-atom glue already glued, so only its line commit is dropped. The
no-head-to-hug commit stranded whatever the author had abutted onto the block's
closing brace (`}\@ehc`, `}.`, `}{`) on a line of its own, a gap the source
never had, and left to the fill the abutment survives. The head-hug is the one
that bought something: without it a detonating atom has no flat width, so the
fill's pair fit fails and the gap before it breaks, splitting `\vbox to` from
`\Gin@req@height{%`. So the fill itself hugs.

#### The hugging fill

`Ir::HugFill` is a plain greedy fill whose atoms are measured by their first
line, the prefix up to their first forced break, when they have no flat width.
That is exactly the claim a trailing-block hug makes. A hugged atom prints in
`Mode::FlatPrefix`, so its own body still breaks below.

This is pass-invariant where the dispatch was not. A soft atom's prefix is its
flat width, so the measurement is unchanged for it, and the atom that flips from
soft to forced across passes, because a width wrap inside fallback content mints
statement boundaries the reparse reads as hard breaks, is placed at the same
column both times, since the flip cannot change the first line. The
rest-awareness that keeps a flat last atom honest is deliberately not applied to
a hug claim: like the trailing-block hug's, it never covered the rest of the
line, and a statement that ends one atom earlier on the next pass must place
that atom identically.

Every early line commit must therefore build its head with the same fill kind
the line would have committed as. The trailing-command arm hands a head off as
one fill, and a plain `Ir::Fill` there would break the very atoms that hugged
mid-line.

### Interaction with `.dtx` doc margins

In a `.dtx` a region regularly spans several `macrocode` chunks, with
`\ExplSyntaxOn` in one and the `Off` several chunks later, so the doc-margined
lines in between, both doc prose and the frame lines themselves, are subtracted
from the regions. Only code lines are formatter-owned, and a `%` margin stays in
column 0.

More generally, the line-oriented `.dtx` tokens `DOC_MARGIN` and `GUARD` are
margins and guards only at line start, so no relayout may merge or re-indent
their lines. An in-region code group carrying such a token in its body is held
to the same rule: `lower_expl_group` forces the broken form so the guard or
margin rides its own line and `lower_loose_token` pins it to column 0.
Flattening it into `{ %<trace> … }` would re-lex the guard as an ordinary `%`
comment that swallows the closing brace, unbalancing the group on the next
parse.
