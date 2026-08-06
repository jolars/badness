# Parser & Lexer Modes

The parser is a hand-written recursive-descent parser over a flat token stream.
It treats input as **generic TeX surface syntax** and **always produces a
lossless tree**. This page collects the parser's load-bearing design decisions,
the catalog of statically-recognizable patterns it handles, and the shape gates
that keep it from over-committing to meaning it can't statically know.

For the high-level pipeline and the two-layer split, start with
[Architecture](architecture.md).

## Generic TeX, always lossless

The parser never *requires* resolving macros or catcodes—in full generality that
is equivalent to running a TeX engine, and we do **not** implement macro
expansion or a TeX evaluator. Anything we cannot statically resolve degrades to
generic nodes (plus a diagnostic where useful), never a crash or corruption.

What we *do* handle is a bounded, growing set of *statically recognizable*
patterns, either as lexer modes or as semantic enrichment. All of them read
static facts only; none resolves what a macro means.

## Sanctioned lexer modes

Each entry below is a pattern the lexer or grammar recognizes from static shape
alone. They are deliberately conservative: when in doubt, a construct stays
generic.

### Letter modes

`\makeatletter`/`\makeatother` make `@` a letter, and
`\ExplSyntaxOn`/`\ExplSyntaxOff` open expl3 (where `_` and `:` are letters; also
opened by `\ProvidesExplPackage`/`Class`/`File`). These are independent flags
that compose.

**Implicit expl3 in toggle-less `.dtx`.** Real expl3 package sources
(`ltx-talk-structure.dtx`) never run an in-file `\ExplSyntaxOn`: expl3 is
declared in the parent `.dtx`/build, and the module prefix `@@` is set by a
docstrip `%<@@=mod>` guard. So the lexer additionally reads a **file-level
static signal** (`lexer::dtx_has_expl_signal`): a line-leading `%<@@=…>` module
guard, or a `\ProvidesExpl*` anywhere. When present in a `.dtx`, every
`macrocode` body lexes under expl3 catcodes—`expl_syntax` is forced on at each
frame entry and restored on exit, mirroring the `at_letter` save/restore
(`macrocode` never nests, so one saved slot suffices). The scan is deliberately
coarse and name-only, consistent with the lexer's other expl handling: a false
positive only *joins* `_`/`:` into a control word (lossless, AGENTS.md decision
#1), and reading the whole file makes the signal order-independent, so a body
*above* the declaration is flagged too. This is a **catcode-only** widening:
`\begin`/`\end` and braces are untouched, so environment pairing is unaffected
(a macrocode body is already blanket-exempt from pairing via `in_def_body`);
making the grammar's `in_expl_region` implicit-aware is a deliberate non-goal.

### Verbatim

`\verb`, verbatim-like environments, and verbatim-argument commands capture
their opaque body or final argument as a single token, using the signature DB
(`crates/badness-parser/data/signatures.json`) for argument shape. Built-ins are
curated; **user-defined** verbatim commands are discovered by the definition
scanner (`semantic::define`) via a **bounded two-pass parse**—pass 1
fingerprints catcode-changing definitions, pass 2 re-lexes with those names.
This is conservative by construction: a false positive suppresses real
diagnostics, so we prefer false negatives.

Verbatim **environments** are recognized two ways in the same pass-1 scan: a
`\newenvironment`/`\NewDocumentEnvironment` whose begin-code fingerprints a
catcode-othering signal (`\dospecials`, `\catcode…=12`, `\@makeother`), *and*—by
the identity of the defining command, since the raw-body machinery lives inside
the package and leaves no scannable signal—`listings`' `\lstnewenvironment` and
`fancyvrb`'s `\DefineVerbatimEnvironment`. Both mark the environment
`verbatim_body`, so pass 2 collects its body as one opaque `VERBATIM_BODY` and
literal environment tokens inside it (`\begin{tabular}`, a stray `\end`) are
never parsed as structure (issue #93). This is the same bounded, statically
recognizable pattern set as decision #1: a definer's name is a static fact, not
macro meaning.

The same pass-1 scan handles the inverse collision (follow-up to issue #53): a
built-in braced-verbatim command name (`\code`, `\url`, `\path`, …) that the
file **redefines** to an ordinary, non-verbatim macro is recorded as a
*suppression* in the lexer's `VerbCtx`, so pass 2 lexes `\code{…}` as an
ordinary group rather than an opaque `VERB`. Only the braced form is affected;
the built-in capture still applies when the file does not redefine the name.
Because the scan is a pure function of a single file's text (decision #7), a
redefinition living in another file (e.g. HoTT's `\code` in `macros.tex`)
remains a tolerated false negative.

### `\left`/`\right` delimiter isolation

The delimiter following `\left` or `\right` is emitted as its own token; the
parser then builds the `LEFT_RIGHT` pair.

### Math environments

An environment the *built-in* signature DB flags `math` (`equation`, `align`,
`gather`, the matrix family, …) has its body parsed in **math mode**, wrapped in
a `MATH` node exactly as `\[…\]`—so `^`/`_` build `SCRIPTED` nodes, the math
operator split fires, and `\left…\right` pair. The set includes environments
whose *shape* is not obviously math but whose *contents* always are: `array`
(the math-mode-only analog of `tabular`) and `tikzcd` (tikz-cd commutative
diagrams, whose cells typeset in math). Both were `\left…\right` false positives
for the linter's `unclosed-math-delimiter` while their bodies stayed in text
mode (dalcde/cam-notes).

This is a *grammar* decision (`parser::grammar::math_environment_body`, gated by
`parser::lexer::is_math_environment`), needing **no lexer math state**: the
math-relevant tokens (`&`, `\\`, `^`/`_`, `\left`/`\right` isolation) are
emitted regardless of mode; only *which grammar function runs* changes. It reads
the curated `math` flag only (never CWL or user tiers), mirroring
`is_block_environment` and `is_verbatim_environment`—a wrong route is a
structural change, so it rests on curated data, and a user or unknown
environment stays in text mode. A blank line inside such a body stays trivia in
the `MATH` node (no paragraph split); the matching `\end` is the terminator.

### Definition bodies

The argument groups of the environment-defining commands
(`\newenvironment`/`\renewenvironment`/`\provideenvironment` and the xparse
`\NewDocumentEnvironment` family), the command-defining commands (the
`\newcommand` family, `\DeclareRobustCommand`, and the xparse
`\NewDocumentCommand` family), and the LaTeX2e document/package hooks
(`\AtBeginDocument`/`\AtEndDocument`/`\AtEndOfClass`/`\AtEndOfPackage`/`\AddToHook`)
are *macro-code bodies*. TeX does not require `\begin`/`\end` to balance within
an individual group—`\newenvironment{wrap}{\begin{center}}{\end{center}}` (issue
#45), or `\AtBeginDocument{\begin{stretchpage}}` balanced by a matching
`\AtEndDocument` (issue #55).

Inside these bodies `\begin`/`\end` parse as plain `COMMAND`s—no `ENVIRONMENT`
pairing, no stray-`\end` or unclosed diagnostics—and they stop being bail
anchors for `[…]` optionals. This is a grammar routing decision on a closed,
curated command-name set (`parser::grammar::is_definition_body_command`, a
parser flag scoped to the attached arguments); the bodies stay generic macro
code, never executed.

### The environment group-boundary gate

The curated command set above only covers bodies we can name. But the same
splitting happens all over real package code under commands we do not (and
should not) enumerate: `array.sty`'s
`\newcolumntype{w}[2]{>{\begin{lrbox}…}c<{\end{lrbox}…}}` puts the two halves in
*sibling* groups, `rotex.tex` pairs
`\newcommand\BeginExample{…\begin{VerbatimOut}…}` against a separate
`\EndExample`, `multicol.sty` splits
`\@namedef{multicols*}`/`\@namedef{endmulticols*}`, and `amstex.sty` writes
`\begin{split}` as *prose* inside a `\PackageError` message that never runs as
structure (all issue #71).

So environment pairing is shape-gated on brace structure instead, which needs no
command list at all: **an environment can never outlive the brace group its
`\begin` opened in.** Braces are catcode-level structure while `\begin`/`\end`
are only macros, so a `}` closing a group opened *before* the `\begin` always
wins. A `\begin` whose matching `\end` is not reachable before that `}` is
therefore ordinary macro code—a plain `COMMAND`, **no diagnostic**, exactly as a
gated `$` or `\[` stays a plain token
(`parser::grammar::environment_escapes_group`). The mirror case holds for the
closer: an `\end` reached *inside* a group has its `\begin` outside it, so it is
macro code rather than stray—ltxdoc's `\StopEventually{\end{document}}`.

Without the gate the environment swallowed the enclosing `}` and cascaded into
unmatched-brace and unclosed-`{` noise, which—since parser diagnostics gate the
formatter—refused the whole file.

Two scope limits keep the gate high-precision:

- **Only a group boundary suppresses the environment.** A `\begin` that merely
  runs out of file still opens one, so a genuinely forgotten `\end` in prose
  keeps its unclosed-environment diagnostic. Likewise an `\end` at the outer
  level is still stray.

- **`.dtx` doc-margin lines are exempt from *stranded* braces**, much as they
  are from the expl3 carve-out: `\begin{macro}` and friends are the
  *documentation* layer and must keep pairing across the `macrocode` chunks
  between them. Those chunks routinely leave a brace open on purpose (a
  `\iffalse}\fi` editor-balance hack, a ``\char`}`` constant), which strands the
  group depth above zero for the rest of the file and would otherwise unnest the
  whole doc layer behind it. A paragraph-break bound cannot stand in here the
  way it does for the math gates—a blank `.dtx` doc line is still a `%` margin,
  so it never reads as a `\par`.

  The exemption lifts when the enclosing group opened on a doc-margin line too
  (`parser::grammar::doc_margin_exempt`): that `{` is the documentation layer's
  own, locally visible, and the `\begin` really is inside it. `theorem.dtx`
  writes the split definition as doc prose—`% \def\deflist#1{\begin{list}…}`
  paired with `% \def\enddeflist{\end{list}}`—and gets the same gate the code
  layer does (issue #71).

A `\begin` the gate demotes leaves its `\end` an orphan *the gate made*, not one
the author wrote, so the closer is demoted in step
(`parser::grammar::end_orphans_a_demoted_begin`): an `\end` whose name was gated
earlier and which closes no open environment is a plain `COMMAND` too. Left as a
stray `\end` it unwinds every enclosing environment on its way to the root—in
`amsldoc.tex` a single `\lowercase{…\begin{error}{…}}` (the classic trick for
smuggling a literal `}` into text) un-closed the whole `document` and stranded
`\end{document}` behind it. The mirror is scoped to demoted names only, so a
genuine typo (`\end{itemiz}`) still reports.

- **`.dtx` doc-margin lines are exempt**, exactly as they are from the expl3
  carve-out: `\begin{macro}` and friends are the *documentation* layer and must
  keep pairing across the `macrocode` chunks between them. Those chunks
  routinely leave a brace open on purpose (a `\iffalse}\fi` editor-balance hack,
  a ``\char`}`` constant), which strands the group depth above zero for the rest
  of the file and would otherwise unnest the whole doc layer behind it. A
  paragraph-break bound cannot stand in here the way it does for the math
  gates—a blank `.dtx` doc line is still a `%` margin, so it never reads as a
  `\par`.

### Short verbs

The `doc` package's `\MakeShortVerb{\|}`/`\DeleteShortVerb{\|}` toggle a
character's short-verb catcode; while enabled, `<c>…<c>` on one line captures as
a single opaque `VERB` token, exactly like `\verb<c>…<c>`. This is a lexer mode
toggled left-to-right like `\makeatletter` (issue #57).

It is also enabled by loading a curated doc class
(`\documentclass{ltxdoc|ltxguide|ltnews|l3doc|amsldoc}`—those classes enable `|`
themselves, `amsldoc.cls` with an active `\gdef|{\protect\activevert{}}`;
without it `amsldoc.tex`'s `|\begin{alignat}|` prose read as real structure,
issue #71) and is on for `|` from the start in `.dtx` mode (dtx files are
typeset under ltxdoc; the driver may live elsewhere). It is gated *off* inside
`macrocode` bodies (a code layer) and after `\left`/`\right` (the `|` is a
delimiter). A span with no closing character on its line falls back to an
ordinary lone character.

It is gated off once more after a primitive that grabs the *next token*
unexpanded (`parser::lexer::is_literal_token_command`:
`\string`/`\noexpand`/`\meaning`/`\expandafter`/`\show`). `\string|` prints the
bar—the active character is the token being printed, not a capture opener. The
doc layer writes that in prose, and capturing there runs the span on to the
*next* `|` and swallows whatever braces lie between: lthooks.dtx's
`\meta{first\texttt{\string|}last}\verb|):|` lost the `}` closing `\texttt{` and
`\meta{`, unnesting the `quote` environment around it (issue #71). This is the
same family as the `\left`/`\right` gate one paragraph up.

### Macrocode chunk bodies (`.dtx`)

A frame-lexed `macrocode`/`macrocode*` body is *macro code* whose only
terminator is the frame line (`%    \end{macrocode}`, a line-oriented docstrip
fact; the frame `\begin` is fingerprinted by its `DOC_MARGIN` and attaches no
arguments—the next line's `{` is body code).

The two frames are deliberately asymmetric about column 0. A **begin** frame may
be indented: `\DocInput` runs the documentation part under `\MakePercentIgnore`
(``\catcode`\%=9``, doc.dtx), so a `%` there is an *ignored* character at any
column and `␣␣%␣␣␣␣\begin{macrocode}` opens a chunk exactly like the column-0
spelling—`multicol.dtx` and `latex-lab-block.dtx` both do it, and lexing the
line as a comment made the frame vanish and its `\end{macrocode}` unwind the doc
layer behind it (issue #71). The indent rides as a `WHITESPACE` token before the
margin, so the line stays lossless and the formatter re-pins the frame at column 0.
An **end** frame stays column-0 strict: inside the body `%` is a comment again,
and doc.sty terminates on a delimited match against the literal
`%    \end{macrocode}` line. As with definition bodies, `\begin`/`\end` inside
parse as plain `COMMAND`s (kernel code uses the `\end` *primitive*), and
chunk-unmatched braces are plain tokens with no diagnostics (a `\def` regularly
opens `{` in one chunk and closes it chunks later, issue #57). Matched pairs
still form `GROUP`s, via a per-chunk brace pre-scan
(`parser::grammar::macrocode_body`), and a `[` attaches as an optional only when
its `]` closes inside the chunk.

### Docstrip guard lines are content, not blank space (`.dtx`)

A line-leading `%<…>` lexes as a `GUARD` trivia leaf in any layer. For the
layout rules it *floats* like a `DOC_MARGIN`, so the blank-line test still sees
`%\n%\n` as a `\par`. But a guard-only line (`%<*dtx>`) is not blank: docstrip
**deletes** it outright when it strips the file, so the lines around it are
adjacent, not parted. The shape gates that ask only "did my construct's source
run out mid-shape?" therefore read `TriviaScan::saw_blank_line_outside_guards`—a
second blank-line tally that a guard resets—rather than the layout one.
rotating.dtx is the case: it splits `\ProvidesPackage{rotating}`'s `[…date…]`
optional across `%<package>` and `%<*dtx>`/`%</dtx>` variants, and reading the
guard pair as a paragraph break bailed out of the `[` mid-argument (issue #71).

### `^^A` doc comments (`.dtx`)

ltxdoc/l3doc set ``\catcode`\^^A=14``, and the l3 sources lean on it for
editor-balance hacks in doc-margin prose (`^^A{` paired with a verb `|}|`, a
commented-out `^^A\end{function}`—issue #60). So on a *doc-margin line* the
literal `^^A` lexes as a comment to end of line. This is scoped to doc lines
only: inside `macrocode` bodies `^^A` is live code
(``\char_set_catcode:nn { `\^^A }`` must keep its line), and unmargined driver
lines lex normally.

### l3doc verbatim name arguments

l3doc's `macro`/`function`/`variable` take an xparse `v`-type name argument
(`{ O{} +v }`), curated as `verbatimArg` in the signature DB. The delimited form
(`\begin{macro}+\@@_compile_{:+`) is chosen by upstream precisely when the name
holds unbalanced braces, so the lexer captures the span as one opaque `VERB`
token (same-line, punctuation delimiter, directly abutting). The braced form
keeps its `{`/`}` as real brace tokens (the parser still builds the ordinary
name `GROUP`) with the balanced content between them as one `VERB`: a v-arg is
raw data either way, so `\begin{macro}{\]}` never draws an orphan-closer
diagnostic (issue #60). Both forms are same-line only; a multi-line braced name
falls back to normal lexing. The parser attaches the abutting `VERB` or name
group into the `BEGIN` node like any verbatim command argument.

### expl3 regions are macro code

Inside an expl3 region (`\ExplSyntaxOn`…`\ExplSyntaxOff`, or `\ProvidesExpl*` to
EOF), token lists pass `\begin`/`\end` around as data—l3prefixes.tex builds a
longtable across two `\tl_set:Nn`/`\tl_put_right:Nn` bodies (issue #60). So
in-region they parse as plain `COMMAND`s exactly as in a definition body, and an
orphan `\]`/`\)` is data with no diagnostic (`\char_set_catcode_letter:N \)`;
the rule also applies in definition bodies and macrocode chunks). The parser
pre-scans the same fixed toggle set the lexer flips
(`parser::lexer::expl_toggle`, so the two never drift) and gates by token
position (`parser::grammar::in_macro_code`). `.dtx` doc-margin lines are exempt:
a region regularly spans macrocode chunks, and the doc-layer markup between them
(`\begin{macro}` prose, the frame lines) must keep pairing—the same doc-line
subtraction the formatter applies to region ownership.

The matching *whitespace* catcodes inside an expl3 region are a **formatter**
concern, not a parser one; see [Formatter](formatter.md#expl3-code-formatting).
That page's *positional gate* narrows which toggles license formatter layout
ownership: the toggle *name* set stays shared here (so the two never drift), but
only the formatter also requires the toggle to be a top-level statement, because
mis-lexing a name is lossless and cosmetic whereas mis-owning layout rewrites
meaning (issue #69).

### Char-constant isolation

After a numeric-context primitive (a closed curated set,
`parser::lexer::is_char_constant_command`: the `\char`/`\catcode` code-tables
plus the number producers `\number`/`\the`/`\romannumeral`/`\numexpr`/`\dimexpr`
and the numeric conditionals `\ifnum`/`\ifodd`/`\ifdim`), a backtick opens TeX's
char-constant number notation: the next character is *data* (``\char`$``,
``\char`}``—issue #60), lexed with its backtick as one plain `WORD` token so it
can never open math or close a group. The escaped single-character form
(``\number`\[``) is isolated the same way—backtick plus the whole control
symbol—so a `\[`/`\]` there is the character `[`/`]`, not a math delimiter
(encguide.tex's char-code table pairs `\relax[ … ]` across table rows, issue
#71). This is the same family as the `\left`/`\right` delimiter isolation.

A bare `{`/`}` is the one exception, and only *inside* a brace group. TeX's
balanced-text scans—a `\def` body, a macro argument—count brace **tokens**, and
they run long before `\char` ever would, so at depth > 0 the brace is structure
and the backtick cannot hide it: ``\def\v{\char`}`` closes at that `}`
(longtable.dtx), and the ``\ifnum`}=0\fi`` / ``\ifnum`{=\z@\fi`` brace-balance
idiom keeps its braces structural instead of stranding the group it sits in
(longtable, amsmath-2018-12-01.sty—issue #71). At depth 0 there is no such scan
and the constant reading stands: *a close-group character is written*
``\char`}`` *in running text*. The escaped form `` `\} `` is a control symbol,
never a group delimiter, so it stays data at any depth.

### Signatures

`\newcommand`/xparse *signatures* are extracted into the semantic DB, never
executed.

## Recursive descent, with Pratt local to math

Hand-written recursive descent is the spine. Precedence-climbing (Pratt) is used
*only* for sub/superscript binding (`^`, `_`) and `\left…\right` matching. The
text-level parser has no precedence.

### Math operator atoms

Arithmetic operators (`+ - * / = < >`) are catcode-12 "other" characters, so the
catcode-faithful lexer globs them into `WORD` runs (`a+2*1` is one token).
Operator-ness is a *math-semantic* fact assigned after catcode lexing, so it is
the parser's job, not the lexer's.

Inside math mode (`math_scripted`, `grammar.rs`) a `WORD` glued around operators
is split at operator boundaries into flat sibling atoms via a **byte-range split
of its text** (not a re-lex—no catcode machinery; see `split_math_word`). Only
the *trailing* operand piece is the scriptable base, so `a+2*1^5` binds `^5` to
`1` (matching TeX). This is a bounded widening of "Pratt is local to math":
operators become atoms so the formatter can space them and the display breaker
can break long chains—there is **no arithmetic-precedence expression tree**.

The split rule: `+ - * /` each stand alone (so a leading `+`/`-` reads as
unary), `= < >` coalesce into one relation piece (`<=`), never merging with a
sign (`=-` → `=`,`-`). Bare unbraced script arguments (`x_i+y`) are left glued
(a pre-existing whole-`WORD` script-binding behavior). The resulting operator
*spacing* is a formatter concern; see
[Formatter](formatter.md#math-operator-spacing).

### The `$` shape gate

`$`/`$$` are data in macro code at least as often as they are math delimiters (a
tabular preamble's `>{$}`, an expl3 token list's `{ $ }`, catcode comparisons in
`\def` bodies). So a dollar opens math only when it *reads* as math: a matching
closer must be reachable before an unbalanced `}`, an `\end` not owed to an
intervening `\begin` (plain commands in definition bodies, so neither anchors
nor nests there), a paragraph break, the macrocode chunk end, or EOF
(`parser::grammar::dollar_closes`, mirroring the `[…]` gates below).

A gated dollar stays an ordinary token—no math node and **no diagnostic**: in
code the shape is routine, so it is not statically an error. (Parser diagnostics
gate the formatter, so they must be high-precision; a likely-typo lone `$` in
prose is linter territory.) A closing `$` counts only outside `{…}` nesting
(`math_group` consumes a nested dollar as an ordinary atom).

Both gates must mirror the parse they guard, and the paragraph-break anchor is
tested only *between top-level atoms* of the math body: once the body descends
into a `{…}` group or a nested environment, a blank line is ordinary body trivia
and the math runs on (`paragraph_break_blocks`, issue #70). Scanning those blank
lines as blockers made the gate stricter than the parse, so a display equation
laid out from `tikzpicture` cells— `\[ \begin{array}… \begin{tikzpicture}`,
blank line, `…\]`, the standard Feynman-diagram idiom—lost its math node and
reported its own `\]` as unmatched, which in turn refused the whole file to the
formatter.

`\[`/`\(` are gated the same way (`delim_math_closes`, issue #65): macro code
passes the delimiters around as data (`\expandafter\@tempa\[\@nil`), so an
opener with no reachable closer stays an ordinary token, no diagnostic. An
orphan `\]`/`\)` still diagnoses, so a prose `\[…\]` typo'd across a paragraph
break is caught on its closer; only a fully unmatched opener goes silent (linter
territory, as for `$`).

Relatedly, the control *symbol* after a `\def`-family primitive
(`\def`/`\gdef`/`\edef`/`\xdef`) is the sequence being (re)defined, never
syntax: `\def\[{…}` is no math opener and `\def\\{…}` no line break—the name is
consumed as a plain token inside the `\def`'s node, and the attached body is a
macro-code body (stacks-project opens `trivlist` in `\def\[`'s body and closes
it in `\def\]`'s; `is_def_prefix_command`, another closed curated set). A
control-*word* name (`\def\foo…`) keeps its benign generic-command shape.

## Argument grouping and bracket policy

The CST greedily attaches trailing `{…}`/`[…]` groups as argument nodes
(texlab-style). Arity is unknown at parse time; the semantic layer refines it.

### Why greedy: text purity, not uniformity

Decision #8 is really two claims, and only one of them is load-bearing. The deep
claim is **database independence**: attachment must read the input text (plus
compiled-in data), never mutable signature inputs—config, package scopes, or
scanned definitions beyond the two-pass verbatim scan. Consulting the signature
DB during grouping would make the tree a function of inputs other than the text,
breaking decision #7's "the tree is a pure function of the text" property that
salsa storage, determinism, and error tolerance all lean on: every signature
edit would invalidate every parse. For generic LaTeX this forces
greed—`\foo{a}{b}` is a 2-argument call or a 0-argument command followed by two
groups, and nothing in the text says which—so greedy is the only *total,
text-pure* strategy available.

The shallow claim—that attachment is therefore *uniform*—was never quite true
(the bracket shape gates below, the `#`/control-word run breaks, and the
starred-variant fold all deviate on static facts) and has one systematic
counterexample: **expl3**. The argspec suffix rides in the `CONTROL_WORD` token
itself (in-region, `:`/`_` are letters), so arity-directed attachment for
derivable specs would be exactly as text-pure as greed. Greedy is not neutral
there; it is a systematically wrong guess—every single-token slot (`N`/`V`)
breaks the attachment run, so every `\tl_set:Nn \l_a {x}` attaches `{x}` to the
definee. The measured cost of that guess is the formatter's S4 machinery (the
peel-back queue and p-scan in `semantic::expl3`, which exist only to undo greedy
ownership after the fact) and the compensating heuristics that accumulated
before it (`head_command_has_grouped_sibling_arg`,
`statement_has_preceding_group`, `lower_expl_conditional`'s bail-out when
branches attach to the wrong sibling).

Arity-directed expl3 attachment is therefore the **recorded candidate
deviation**, deliberately unimplemented (see TODO.md). It would key on token
shape alone—a colon-suffixed name only lexes as one token in-region, so the
grammar needs no region awareness—and unknown or underivable specs (`w`, `D`,
colonless names) would fall back to greed. The open questions the migration must
answer before it lands: the **mixed-shape CST** (consumers must handle
arity-attached and greedy nodes side by side, where today the in-region tree is
uniformly greedy and every consumer opts into the arity view); the
**false-positive blast radius** (a never-executed `\foo:n` spelling in data
position gets a wrong *tree*, visible to the linter and LSP, where today the
static model's misfires cost only layout); and **differential-oracle
divergence** (texlab will not group this way, so the parse-compat skeletons need
a divergence ledger). The semantic statement model (`semantic::expl3` driving
the formatter's segmentation) doubles as the migration's differential oracle:
grammar attachment can be tested against it over the gate corpora before any
consumer flips.

**`[…]` attachment is shape-gated** (issue #43). `[`/`]` are not real grouping
in TeX, so a bracket is an argument only when it reads as one, decided from
static shape facts, never meaning (`parser::grammar`, `BracketPolicy` +
`attach_arguments`):

- *Lexically inside math* (a `math_depth` that persists into text-mode bodies of
  unknown environments nested in math), a `[` attaches only when it directly
  abuts the command (`\sqrt[3]{x}`; a spaced `\bE [ x ]` is a delimiter) and its
  `]` closes before the math ends (open-interval notation `$]0;\num{0.5}[$`)—net
  of the `]`s claimed by intervening command-abutting `[`s, so in
  `\P[\gamma[0, \infty) \cap A =   \emptyset]` the lone `]` belongs to `\gamma[`
  and the outer `\P[` stays an ordinary atom (issue #55). How a `$` inside the
  bracket reads depends on the *innermost enclosing math's flavor*
  (`math_dollar`, pushed in lockstep with `math_depth`): inside `\[…\]`/`\(…\)`
  a `$` opens a genuine nested inline region, so a balanced inline `$…$` pair is
  transparent and `\inferrule*[right=$\Pi$-eq]` attaches its optional and wraps
  the label's `$\Pi$` in an `INLINE_MATH` node (an *unbalanced* `$` leaves no
  reachable `]`, so the bracket stays plain); inside `$…$`/`$$…$$` a `$` cannot
  nest, so the first one is the math's *closer* and bounds the search—a stray
  `[` with its `]` only in a *later* `$…$` (a missing-`]` typo like
  `$\mathcal{N}[\mathcal{S}$`, issue #99) stays an ordinary atom instead of an
  optional swallowing the following math.
- *In text mode* (issue #60, mirroring the `$` shape gate), a `[` attaches only
  when its `]` is reachable before an unbalanced `}`, a `\begin`/`\end` outside
  a definition body, a paragraph break, or EOF—net of the `]`s claimed by
  intervening command-abutting `[`s, as in the math gate. Macro code tests for
  and re-emits lone brackets (`\@ifnextchar [\@xmpar\@ympar`) at least as often
  as prose writes real optionals, so a gated bracket stays an ordinary token
  with **no diagnostic**. (In a macrocode chunk the chunk-scoped gate applies
  instead.)
- A *curated math environment's* `\begin` likewise attaches only a
  directly-abutting bracket (`\begin{aligned}[t]`; a detached `[a]_1` on the
  next line is body content). Non-math environments stay greedy across
  trivia—the xparse-signature glue relies on a next-line `[Warning]` still
  attaching to `\begin{note}`.
- The *delimiter-size commands* (`\big`…`\Bigg` + `l`/`m`/`r`) never take a
  bracket argument: their `[` is the delimiter being sized (`\Big[ x \Big]`),
  mirroring the `\left`/`\right` special case.

**Starred-variant `*` folds into the invocation.** A lone `*` tight to a command
and *followed by an argument* (`[`/`{`) is a starred-variant marker (`\@ifstar`
commands: `\section*{…}`, mathpartir's `\inferrule*[…]`, the `\\*[2pt]` line
break), so `attach_arguments` folds it in as a child token and keeps scanning,
letting the following arguments attach instead of the `*` breaking the run
(`at_star_variant_marker`). Gating on a *following argument* keeps a math
operator (`\pi*r`, `\Gamma * x`)—a `*` with nothing after it—from being mistaken
for a marker, and a spaced `\foo *` or a glued `\foo*bar` (the lexer merges
`*bar` into one word) is not a marker either. The semantic layer's star probes
read the folded child (`pkgmeta::has_trailing_star` for `\DeclareOption*`), with
the pre-fold sibling shape kept as a fallback.

## Trivia attachment

Trivia attachment follows the rust-analyzer rule: comments bind *forward*,
whitespace floats, blank lines break the bind. Trivia is never dropped, so the
only question is which node owns it:

- **Default: float at the nearest enclosing node.** Inter-sibling whitespace and
  newlines stay direct children of the tightest containing block/group, owned by
  neither neighbor.
- **A contiguous run of own-line `%` comments immediately preceding a `COMMAND`
  or `ENVIRONMENT` binds *leading* into it**, grouped as a `DOC_COMMENT` node.
  "Documentable" is decided purely on node kind—no signature-DB lookup leaks
  into the parser. A same-line trailing comment (`\foo % x`) never binds.
- **A blank line (`≥2` newlines, the `\par` boundary) breaks the bind:**
  comments past it stay floating. This is a **deliberate divergence** from
  rust-analyzer's `n_attached_trivias`, which peeks *past* a blank line and
  keeps attaching when the next comment is an outer doc comment (`///`/`//!`).
  That peek keys on the `///`-vs-`//` distinction—a marker of documentation
  intent that LaTeX's single catcode-14 `%` has no equivalent for. Applied to
  `%` it would wrongly glue a license or copyright header into the following
  command's doc comment, so we bind only the maximal blank-line-free suffix.
  Pinned by `comment_after_blank_line_still_binds` (`tests/parser.rs`).

Whitespace stays a bare leaf token (never wrapped); the bound leading-comment
run is the one named-node exception. This is a CST-shape convention enforced by
tests, not a hard oracle.

## The event stream

The parser emits an event stream, not a tree directly:

```
lexer → flat token stream → parser emits events (Start / Tok(idx) / Finish)
      → tree_builder re-attaches trivia and feeds rowan's GreenNodeBuilder
```

Tokens are referenced by index. There is **no `Error` event**—diagnostics ride a
side channel keyed by byte range (the rust-analyzer event-stream pattern). One
extra event, `SubTok { idx, start, end }`, attaches a `WORD` sub-slice of
`tokens[idx]` (the math operator split above); losslessness holds because a
token's `SubTok` pieces cover its full byte range contiguously.

## Error recovery

A single syntactic error never fails the whole parse; errors travel alongside
the tree. Recovery anchors are `\end{…}`, `\begin`, a blank line, `}`, `$`, `&`,
and `\\`. The parser always makes progress and never infinite-loops on
unexpected input.

## Incrementality

Incrementality is **salsa-first.** Cross-file and cross-query incrementality via
salsa is the v1 story; intra-file incremental reparse (reusing green subtrees)
is a *later optimization*—a whole-file reparse of a typical `.tex` is
sub-millisecond.

Green nodes are stored in salsa, never red (`SyntaxNode`), because red trees
aren't `Send`/`Eq`/`salsa::Update`. `incremental.rs` uses
`#[salsa::input] SourceFile { path, text }` and a `parsed_document` query
returning `rowan::GreenNode` plus diagnostics under
`no_eq, unsafe(non_update_types)`—sound because the tree is a pure function of
the text—materializing red cursors on demand.

**Durability.** Salsa's default input durability is `LOW`, so every input is
implicitly `LOW` unless constructed otherwise. `SourceFile.path` is built at
`Durability::HIGH` (via `SourceFile::builder(path, text).path_durability(HIGH)`)
because it is set once and never mutated; `text` keeps the `LOW` default, since
a keystroke `set_text` rewrites it. The interned `Project` is already
`NEVER_CHANGE` (interned values always are), so it needs nothing. Per-field
revision tracking already stops a path-only query from re-running on a text
edit; durability adds the coarse global short-circuit that only starts to matter
once a genuinely rarely-changing input exists. So the rule for growth: **any
future salsa input promoted from config or package/class metadata must be
constructed at `HIGH`/`MEDIUM`**—leave it `LOW` and every keystroke's global
`LOW`-revision bump would invalidate it. (Today that data deliberately lives in
`LazyLock`/`OnceLock`, outside the db.)

## Typed AST wrappers

Typed AST wrappers are a **read-only view, never a re-model of the tree.** On
top of the untyped rowan CST sits a thin typed layer (`ast.rs` + `ast/nodes.rs` +
`ast/tokens.rs`, and the bib parallel `bib/ast.rs`): rust-analyzer-style
`AstNode`/`AstToken` traits (`can_cast`/`cast`/`syntax`), an `ast_node!`
identity macro (a 12-line `macro_rules!`, *not* codegen—the accessors are
hand-written), and one wrapper struct per node kind (`Command`, `Group`,
`Optional`, `NameGroup`, `Begin`, `End`, `Environment`, `ControlWord`; add more
only when a field-extraction consumer appears—`Math`/`Scripted`/… stay unwrapped
until then).

Wrappers expose **structure** (a command's name token, its positional argument
groups, an environment's `\begin`/`\end`), never **meaning**—no signature-DB
lookup lives here. Because the CST is greedy and generic, accessors are
**positional** (`Command::nth_group(n)` filters `GROUP` only, so an `OPTIONAL`
never shifts brace indexing) and tolerate over-attachment by construction; they
never pretend arity is fixed (`Command::title()` would be a lie—a `\section` and
a `\newcommand` share the `COMMAND` shape). Navigation uses the generic helpers
`child::<N>`/`children::<N>`/`child_token::<T>`, which replace the raw
`children().find(|c| c.kind()==X)` idiom at *field-extraction* sites.

The wrappers are read-only, so they can't threaten losslessness or idempotence.
The **formatter deliberately stays raw** for structural work: the `lower_node`
`match node.kind()` dispatch and the token-classification loops (trivia walks,
`L_BRACE`/`R_BRACE` matching) are idiomatic tree-walking that wrappers would
only obscure—the formatter adopts wrappers *only* for field access
(argument/name extraction). The pre-wrapper **free functions** (`command_name`,
`environment_name`, `nth_group_text`, …) remain as thin **kind-agnostic shims**
over the wrapper bodies: they read whatever relevant child a node has without
gating on the node's own kind, because callers rely on that latitude (a dtx
`\begin{macro}{\foo}` reads a `GROUP` off a `BEGIN`; an xparse default body
handed to `group_inner_source` may be an `OPTIONAL`). The typed methods are
kind-checked at `cast`; the shims are not.
