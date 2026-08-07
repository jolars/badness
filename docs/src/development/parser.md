# Parser & Lexer Modes

The parser is hand-written recursive descent over a flat token stream. It treats
its input as generic TeX surface syntax and always produces a lossless tree.

Resolving macros and catcodes in full generality means running a TeX engine, and
we do not do that. Anything we cannot resolve statically degrades to a generic
node, with a diagnostic where one is useful, never to a crash or to corrupted
output. What we do handle is a bounded and growing set of patterns that can be
recognized from static shape alone.

## Sanctioned lexer modes

Each pattern below is recognized by the lexer or the grammar from static facts.
They are deliberately conservative: when in doubt, a construct stays generic.

### Letter modes

`\makeatletter` and `\makeatother` make `@` a letter. `\ExplSyntaxOn` and
`\ExplSyntaxOff` open expl3, where `_` and `:` are letters;
`\ProvidesExplPackage`, `\ProvidesExplClass`, and `\ProvidesExplFile` open it
too. The two flags are independent and compose.

Real expl3 package sources never run an in-file `\ExplSyntaxOn`: expl3 is
declared in the parent `.dtx` or the build, and the module prefix `@@` comes
from a docstrip `%<@@=mod>` guard. So in a `.dtx` the lexer also reads a
file-level signal, a line-leading `%<@@=…>` guard or a `\ProvidesExpl*`
anywhere. When it is present, every `macrocode` body lexes under expl3 catcodes;
`expl_syntax` is forced on at each frame entry and restored on exit, mirroring
the `at_letter` save and restore. Since `macrocode` never nests, one saved slot
is enough.

The scan is coarse and name-only on purpose. A false positive only joins `_` and
`:` into a control word, which is lossless and cosmetic, and reading the whole
file makes the signal order-independent, so a body above the declaration is
covered too. This widens catcodes only: `\begin`, `\end`, and braces are
untouched, so environment pairing is unaffected.

### Verbatim

`\verb`, verbatim-like environments, and verbatim-argument commands capture
their body or final argument as a single token, using `data/signatures.json` for
the argument shape. Built-ins are curated. User-defined verbatim commands are
found by the definition scanner in a bounded two-pass parse: pass 1 fingerprints
catcode-changing definitions, pass 2 re-lexes with those names. A false positive
suppresses real diagnostics, so the scan prefers false negatives.

Verbatim environments are recognized two ways in the same pass. Either a
`\newenvironment` or `\NewDocumentEnvironment` whose begin-code fingerprints a
catcode-othering signal (`\dospecials`, `\catcode…=12`, `\@makeother`), or the
identity of the defining command: `listings`' `\lstnewenvironment` and
`fancyvrb`'s `\DefineVerbatimEnvironment` leave no scannable signal because the
raw-body machinery lives inside the package. Both mark the environment
`verbatim_body`, so pass 2 collects the body as one opaque `VERBATIM_BODY` and
literal environment tokens inside it are never parsed as structure. A definer's
name is a static fact, not macro meaning.

The same scan handles the inverse collision. A built-in braced-verbatim command
name (`\code`, `\url`, `\path`) that the file *redefines* to an ordinary macro
is recorded as a suppression, so pass 2 lexes `\code{…}` as an ordinary group.
Only the braced form is affected. Because the scan reads a single file's text, a
redefinition living in another file stays a tolerated false negative.

### `\left` and `\right` delimiter isolation

The delimiter following `\left` or `\right` is emitted as its own token. The
parser then builds the `LEFT_RIGHT` pair.

### Math environments

An environment the curated signature table flags `math` (`equation`, `align`,
`gather`, the matrix family) has its body parsed in math mode and wrapped in a
`MATH` node exactly as `\[…\]`, so `^` and `_` build `SCRIPTED` nodes, the math
operator split fires, and `\left…\right` pair. The set includes environments
whose shape is not obviously math but whose contents always are: `array`, the
math-only analog of `tabular`, and `tikzcd`, whose cells typeset in math. Both
were `\left…\right` false positives for the linter while their bodies stayed in
text mode.

This is a grammar decision and needs no lexer math state. The math-relevant
tokens (`&`, `\\`, `^`, `_`, `\left`/`\right` isolation) are emitted regardless
of mode; only which grammar function runs changes. It reads the curated `math`
flag and never the CWL or user tiers, because a wrong route is a structural
change. A user or unknown environment stays in text mode. A blank line inside
such a body is trivia rather than a paragraph break; the matching `\end`
terminates.

### Definition bodies

The argument groups of the environment definers (`\newenvironment` and
relatives, plus the xparse `\NewDocumentEnvironment` family), the command
definers (the `\newcommand` family, `\DeclareRobustCommand`, and the xparse
`\NewDocumentCommand` family), and the LaTeX2e hooks (`\AtBeginDocument`,
`\AtEndDocument`, `\AtEndOfClass`, `\AtEndOfPackage`, `\AddToHook`) are
macro-code bodies. TeX does not require `\begin` and `\end` to balance within a
single group:

```latex
\newenvironment{wrap}{\begin{center}}{\end{center}}
\AtBeginDocument{\begin{stretchpage}}
```

Inside these bodies `\begin` and `\end` parse as plain commands. There is no
environment pairing, no stray-`\end` or unclosed diagnostic, and they stop being
bail anchors for `[…]` optionals. The set of definer names is closed and
curated, the flag is scoped to the attached arguments, and the bodies stay
generic macro code.

### The environment group-boundary gate

The curated set above covers only the bodies we can name. The same splitting
happens throughout real package code under commands we cannot and should not
enumerate:

```latex
% array.sty: the two halves land in sibling groups
\newcolumntype{w}[2]{>{\begin{lrbox}…}c<{\end{lrbox}…}}
% multicol.sty: split across two \@namedef s
\@namedef{multicols*}…  \@namedef{endmulticols*}…
% amstex.sty: prose inside an error message that never runs as structure
\PackageError{…}{… \begin{split} …}
```

So pairing is gated on brace structure instead, which needs no command list at
all: an environment can never outlive the brace group its `\begin` opened in.
Braces are catcode-level structure while `\begin` and `\end` are only macros, so
a `}` closing a group opened before the `\begin` always wins. A `\begin` whose
`\end` is not reachable before that `}` is ordinary macro code, a plain command
with no diagnostic, exactly as a gated `$` or `\[` stays a plain token. The
mirror holds for the closer: an `\end` reached inside a group has its `\begin`
outside it, so it is macro code rather than stray.

Without the gate the environment swallowed the enclosing `}` and cascaded into
unmatched-brace noise. Parser diagnostics gate the formatter, so that refused
the whole file.

Two limits keep the gate precise.

Only a group boundary suppresses the environment. A `\begin` that merely runs
out of file still opens one, so a genuinely forgotten `\end` in prose keeps its
diagnostic, and an `\end` at the outer level is still stray.

`.dtx` doc-margin lines are exempt from *stranded* braces. `\begin{macro}` and
its relatives are the documentation layer and must keep pairing across the
`macrocode` chunks between them, and those chunks routinely leave a brace open
on purpose (an `\iffalse}\fi` editor-balance hack, a ``\char`}`` constant). That
strands the group depth above zero for the rest of the file and would otherwise
unnest the whole doc layer behind it. A paragraph-break bound cannot stand in
here the way it does for the math gates, because a blank `.dtx` doc line is
still a `%` margin and never reads as a `\par`. The exemption lifts when the
enclosing group opened on a doc-margin line itself: that `{` belongs to the
documentation layer, is locally visible, and the `\begin` really is inside it.
`theorem.dtx` writes a split definition as doc prose,
`% \def\deflist#1{\begin{list}…}` paired with `% \def\enddeflist{\end{list}}`,
and gets the same gate the code layer does.

A `\begin` the gate demotes leaves an `\end` that the gate itself orphaned, so
the closer is demoted in step: an `\end` whose name was gated earlier and which
closes no open environment becomes a plain command too. Left stray it unwinds
every enclosing environment on its way to the root. In `amsldoc.tex` a single
`\lowercase{…\begin{error}{…}}`, the classic trick for smuggling a literal `}`
into text, un-closed the whole `document`. The mirror is scoped to demoted names
only, so `\end{itemiz}` still reports.

### Short verbs

`doc`'s `\MakeShortVerb{\|}` and `\DeleteShortVerb{\|}` toggle a character's
short-verb catcode. While enabled, `<c>…<c>` on one line captures as a single
opaque `VERB` token, exactly like `\verb<c>…<c>`. This is a lexer mode toggled
left to right like `\makeatletter`.

It is also enabled by a curated doc class (`ltxdoc`, `ltxguide`, `ltnews`,
`l3doc`, `amsldoc`), which enable `|` themselves, and it is on for `|` from the
start in `.dtx` mode, since dtx files are typeset under ltxdoc even when the
driver lives elsewhere. It is gated off inside `macrocode` bodies, which are a
code layer, and after `\left` and `\right`, where `|` is a delimiter. A span
with no closing character on its line falls back to an ordinary character.

It is gated off once more after a primitive that grabs the next token unexpanded
(`\string`, `\noexpand`, `\meaning`, `\expandafter`, `\show`). `\string|` prints
the bar; the active character is the token being printed, not a capture opener.
The doc layer writes that in prose, and capturing there runs the span on to the
next `|` and swallows whatever braces lie between. In `lthooks.dtx`,
`\meta{first\texttt{\string|}last}\verb|):|` lost the `}` closing `\texttt{` and
`\meta{`, unnesting the `quote` environment around it. This is the same family
as the `\left`/`\right` gate above.

### Macrocode chunk bodies

A frame-lexed `macrocode` or `macrocode*` body is macro code whose only
terminator is the frame line `%    \end{macrocode}`, a line-oriented docstrip
fact. The frame `\begin` is fingerprinted by its `DOC_MARGIN` and attaches no
arguments, so the next line's `{` is body code.

The two frames are deliberately asymmetric about column 0. A begin frame may be
indented, because `\DocInput` runs the documentation part under
`\MakePercentIgnore` (``\catcode`\%=9``), so a `%` there is an ignored character
at any column and `␣␣%␣␣␣␣\begin{macrocode}` opens a chunk exactly like the
column-0 spelling. `multicol.dtx` and `latex-lab-block.dtx` both do it, and
lexing the line as a comment made the frame vanish and its `\end{macrocode}`
unwind the doc layer behind it. The indent rides as a `WHITESPACE` token before
the margin, so the line stays lossless and the formatter re-pins the frame at
column 0. An end frame stays column-0 strict: inside the body `%` is a comment
again, and `doc.sty` terminates on a delimited match against the literal line.

As in definition bodies, `\begin` and `\end` inside a chunk parse as plain
commands (kernel code uses the `\end` primitive), and chunk-unmatched braces are
plain tokens with no diagnostics, since a `\def` regularly opens `{` in one
chunk and closes it several chunks later. Matched pairs still form groups via a
per-chunk brace pre-scan, and a `[` attaches as an optional only when its `]`
closes inside the chunk.

### Docstrip guard lines are content, not blank space

A line-leading `%<…>` lexes as a `GUARD` trivia leaf in any layer. For layout it
floats like a `DOC_MARGIN`, so the blank-line test still sees `%\n%\n` as a
`\par`. But a guard-only line such as `%<*dtx>` is not blank: docstrip deletes
it outright when it strips the file, so the lines around it are adjacent rather
than parted. The shape gates that ask only whether a construct's source ran out
mid-shape therefore read a second blank-line tally that a guard resets.
`rotating.dtx` splits `\ProvidesPackage{rotating}`'s date optional across
`%<package>` and `%<*dtx>`/`%</dtx>` variants, and reading the guard pair as a
paragraph break bailed out of the `[` mid-argument.

### `^^A` doc comments

ltxdoc and l3doc set ``\catcode`\^^A=14``, and the l3 sources lean on it for
editor-balance hacks in doc-margin prose: `^^A{` paired with a verb `|}|`, or a
commented-out `^^A\end{function}`. So on a doc-margin line the literal `^^A`
lexes as a comment to end of line. This is scoped to doc lines: inside
`macrocode` bodies `^^A` is live code, since ``\char_set_catcode:nn { `\^^A }``
must keep its line, and unmargined driver lines lex normally.

### l3doc verbatim name arguments

l3doc's `macro`, `function`, and `variable` take an xparse `v`-type name
argument, curated as `verbatimArg`. Upstream chooses the delimited form
(`\begin{macro}+\@@_compile_{:+`) precisely when the name holds unbalanced
braces, so the lexer captures the span as one opaque `VERB` token: same line,
punctuation delimiter, directly abutting.

The braced form keeps its `{` and `}` as real brace tokens, so the parser still
builds the ordinary name group, with the balanced content between them as one
`VERB`. A v-argument is raw data either way, so `\begin{macro}{\]}` never draws
an orphan-closer diagnostic. Both forms are same-line only; a multi-line braced
name falls back to normal lexing.

### expl3 regions are macro code

Inside an expl3 region, token lists pass `\begin` and `\end` around as data:
`l3prefixes.tex` builds a longtable across two `\tl_set:Nn` and
`\tl_put_right:Nn` bodies. So in-region they parse as plain commands exactly as
in a definition body, and an orphan `\]` or `\)` is data with no diagnostic
(`\char_set_catcode_letter:N \)`; the rule also applies in definition bodies and
macrocode chunks). The parser pre-scans the same fixed toggle-name set the lexer
flips, so the two cannot drift, and gates by token position. `.dtx` doc-margin
lines are exempt, because a region regularly spans macrocode chunks and the
doc-layer markup between them must keep pairing.

The matching whitespace catcodes inside a region are a formatter concern; see
[Formatter](formatter.md#expl3-code-formatting). The formatter also narrows
which toggles may take ownership of layout. The toggle-name set stays shared,
but only the formatter additionally requires the toggle to be a top-level
statement, because mis-lexing a name is lossless and cosmetic whereas mis-owning
layout rewrites meaning.

### Char-constant isolation

After a numeric-context primitive (a closed curated set: the `\char` and
`\catcode` code tables, the number producers `\number`, `\the`, `\romannumeral`,
`\numexpr`, `\dimexpr`, and the numeric conditionals `\ifnum`, `\ifodd`,
`\ifdim`), a backtick opens TeX's char-constant notation. The next character is
data, so ``\char`$`` and ``\char`}`` lex with their backtick as one plain `WORD`
token and can never open math or close a group. The escaped single-character
form (``\number`\[``) is isolated the same way, backtick plus the whole control
symbol, so a `\[` there is the character `[` rather than a math delimiter. This
is the same family as the `\left`/`\right` isolation.

A bare `{` or `}` is the one exception, and only inside a brace group. TeX's
balanced-text scans, a `\def` body or a macro argument, count brace tokens and
run long before `\char` ever would, so at depth greater than zero the brace is
structure and the backtick cannot hide it. ``\def\v{\char`}`` closes at that `}`
(longtable.dtx), and the ``\ifnum`}=0\fi`` brace-balance idiom keeps its braces
structural instead of stranding the group it sits in. At depth 0 there is no
such scan and the constant reading stands, which is how running text writes "a
close-group character is ``\char`}``". The escaped form `` `\} `` is a control
symbol, never a group delimiter, so it stays data at any depth.

### Signatures

`\newcommand` and xparse signatures are extracted into the semantic database,
never executed.

## Recursive descent, with Pratt local to math

Hand-written recursive descent is the spine. Precedence climbing is used only
for sub- and superscript binding and for `\left…\right` matching. The text-level
parser has no precedence.

### Math operator atoms

Arithmetic operators (`+ - * / = < >`) are catcode-12 "other" characters, so a
catcode-faithful lexer globs them into `WORD` runs: `a+2*1` is one token.
Operator-ness is a math-semantic fact assigned after catcode lexing, which makes
it the parser's job rather than the lexer's.

Inside math mode a `WORD` glued around operators is split at operator boundaries
into flat sibling atoms. This is a byte-range split of the token's text, not a
re-lex, so no catcode machinery is involved. Only the trailing operand piece is
the scriptable base, so `a+2*1^5` binds `^5` to `1`, matching TeX.

The split rule: `+ - * /` each stand alone, so a leading `+` or `-` reads as
unary; `= < >` coalesce into one relation piece (`<=`) and never merge with a
sign, so `=-` splits into `=` and `-`. Bare unbraced script arguments (`x_i+y`)
are left glued, a pre-existing whole-`WORD` script-binding behavior.

Operators become atoms so the formatter can space them and the display breaker
can break long chains. There is no arithmetic-precedence expression tree. The
resulting spacing is a formatter concern; see
[Formatter](formatter.md#math-operator-spacing).

### The `$` shape gate

`$` and `$$` are data in macro code at least as often as they are math
delimiters: a tabular preamble's `>{$}`, an expl3 token list's `{ $ }`, a
catcode comparison in a `\def` body. So a dollar opens math only when it reads
as math, meaning a matching closer is reachable before an unbalanced `}`, an
`\end` not owed to an intervening `\begin`, a paragraph break, the end of a
macrocode chunk, or EOF.

A gated dollar stays an ordinary token, with no math node and no diagnostic. In
code the shape is routine, so it is not statically an error, and parser
diagnostics gate the formatter and must therefore be high precision. A
likely-typo lone `$` in prose is linter territory. A closing `$` counts only
outside `{…}` nesting.

Both gates must mirror the parse they guard. The paragraph-break anchor is
tested only between top-level atoms of the math body: once the body descends
into a group or a nested environment, a blank line is ordinary body trivia and
the math runs on. Scanning those blank lines as blockers made the gate stricter
than the parse, so a display equation laid out from `tikzpicture` cells, the
standard Feynman-diagram idiom, lost its math node and reported its own `\]` as
unmatched, which refused the whole file to the formatter.

`\[` and `\(` are gated the same way, since macro code passes the delimiters
around as data (`\expandafter\@tempa\[\@nil`). An orphan `\]` or `\)` still
diagnoses, so a prose `\[…\]` typo'd across a paragraph break is caught on its
closer; only a fully unmatched opener goes silent.

Relatedly, the control symbol after a `\def`-family primitive (`\def`, `\gdef`,
`\edef`, `\xdef`) is the sequence being defined, never syntax. `\def\[{…}` is no
math opener and `\def\\{…}` no line break; the name is consumed as a plain token
inside the `\def`'s node and the attached body is a macro-code body. The Stacks
Project opens `trivlist` in `\def\[`'s body and closes it in `\def\]`'s. A
control-word name (`\def\foo…`) keeps its ordinary generic-command shape.

## Argument grouping and bracket policy

The CST greedily attaches trailing `{…}` and `[…]` groups as argument nodes,
texlab-style. Arity is unknown at parse time; the semantic layer refines it.

### Why greedy

The load-bearing claim is database independence: attachment reads the input text
plus compiled-in data, never mutable signature inputs such as config, package
scopes, or scanned definitions beyond the two-pass verbatim scan. Consulting the
signature database during grouping would make the tree a function of something
other than the text, and every signature edit would invalidate every parse. For
generic LaTeX that forces greed. `\foo{a}{b}` is either a two-argument call or a
zero-argument command followed by two groups, and nothing in the text says
which, so greedy is the only total, text-pure strategy available.

The weaker claim, that attachment is therefore uniform, was never quite true.
The bracket gates below, the `#` and control-word run breaks, and the
starred-variant fold all deviate on static facts. It also has one systematic
counterexample: expl3. The argspec suffix rides in the `CONTROL_WORD` token
itself, since in-region `:` and `_` are letters, so arity-directed attachment
for derivable specs would be exactly as text-pure as greed. Greedy is not
neutral there, it is a systematically wrong guess: every single-token slot (`N`,
`V`) breaks the attachment run, so `\tl_set:Nn \l_a {x}` attaches `{x}` to the
definee.

The cost of that guess is real. The formatter's peel-back queue and p-scan exist
only to undo greedy ownership after the fact, and several heuristics accumulated
before them. Arity-directed expl3 attachment is therefore the recorded candidate
deviation, deliberately unimplemented. It would key on token shape alone, since
a colon-suffixed name only lexes as one token in-region, and underivable specs
(`w`, `D`, colonless names) would fall back to greed.

Three questions have to be answered before it lands. The CST becomes mixed
shape, so consumers must handle arity-attached and greedy nodes side by side,
where today the in-region tree is uniformly greedy and every consumer opts into
the arity view. The false-positive blast radius grows: a never-executed `\foo:n`
spelling in data position gets a wrong tree, visible to the linter and the LSP,
where today a misfire of the static model costs only layout. And texlab will not
group this way, so the parse-compat skeletons need a divergence ledger. The
semantic statement model doubles as the migration's differential oracle: grammar
attachment can be tested against it over the gate corpora before any consumer
flips.

### Bracket attachment is shape-gated

`[` and `]` are not real grouping in TeX, so a bracket is an argument only when
it reads as one, decided from static shape.

Lexically inside math (a `math_depth` that persists into text-mode bodies of
unknown environments nested in math), a `[` attaches only when it directly abuts
the command (`\sqrt[3]{x}`; a spaced `\bE [ x ]` is a delimiter) and its `]`
closes before the math ends, so open-interval notation `$]0;\num{0.5}[$` stays
plain. The `]` count is net of those claimed by intervening command-abutting
`[`s, so in `\P[\gamma[0, \infty) \cap A = \emptyset]` the lone `]` belongs to
`\gamma[` and the outer `\P[` stays an ordinary atom.

How a `$` inside the bracket reads depends on the innermost enclosing math's
flavor. Inside `\[…\]` or `\(…\)` a `$` opens a genuine nested inline region, so
a balanced `$…$` pair is transparent and `\inferrule*[right=$\Pi$-eq]` attaches
its optional and wraps the label's math in an `INLINE_MATH` node; an unbalanced
`$` leaves no reachable `]`, so the bracket stays plain. Inside `$…$` or `$$…$$`
a `$` cannot nest, so the first one is the math's closer and bounds the search.
A stray `[` whose `]` appears only in a later `$…$`, as in the missing-`]` typo
`$\mathcal{N}[\mathcal{S}$`, stays an ordinary atom instead of an optional
swallowing the following math.

In text mode, mirroring the `$` gate, a `[` attaches only when its `]` is
reachable before an unbalanced `}`, a `\begin` or `\end` outside a definition
body, a paragraph break, or EOF, again net of intervening claims. Macro code
tests for and re-emits lone brackets (`\@ifnextchar [\@xmpar\@ympar`) at least
as often as prose writes real optionals, so a gated bracket stays an ordinary
token with no diagnostic. In a macrocode chunk the chunk-scoped gate applies
instead.

A curated math environment's `\begin` likewise attaches only a directly abutting
bracket: `\begin{aligned}[t]` yes, a detached `[a]_1` on the next line no.
Non-math environments stay greedy across trivia, because the xparse-signature
glue relies on a next-line `[Warning]` still attaching to `\begin{note}`.

The delimiter-size commands (`\big` through `\Bigg`, with the `l`, `m`, and `r`
variants) never take a bracket argument. Their `[` is the delimiter being sized,
as in `\Big[ x \Big]`, mirroring the `\left`/`\right` case.

### Starred variants

A lone `*` tight to a command and followed by an argument (`[` or `{`) is a
starred-variant marker: `\section*{…}`, mathpartir's `\inferrule*[…]`, the
`\\*[2pt]` line break. Attachment folds it in as a child token and keeps
scanning, so the following arguments attach instead of the `*` breaking the run.

Requiring a following argument keeps a math operator (`\pi*r`, `\Gamma * x`)
from being mistaken for a marker, and neither a spaced `\foo *` nor a glued
`\foo*bar` counts, the latter because the lexer merges `*bar` into one word. The
semantic layer's star probes read the folded child, with the pre-fold sibling
shape as a fallback.

## Trivia attachment

Comments bind forward, whitespace floats, and a blank line breaks the bind.
Trivia is never dropped, so the only question is which node owns it.

By default trivia floats at the nearest enclosing node: inter-sibling whitespace
and newlines stay direct children of the tightest containing block or group,
owned by neither neighbor.

A contiguous run of own-line `%` comments immediately preceding a `COMMAND` or
`ENVIRONMENT` binds leading into it, grouped as a `DOC_COMMENT` node.
"Documentable" is decided on node kind alone, so no signature lookup leaks into
the parser. A same-line trailing comment (`\foo % x`) never binds.

A blank line breaks the bind, and comments past it stay floating. This diverges
from rust-analyzer's `n_attached_trivias`, which peeks past a blank line and
keeps attaching when the next comment is an outer doc comment. That peek keys on
the `///` versus `//` distinction, a marker of documentation intent that LaTeX's
single catcode-14 `%` has no equivalent for. Applied to `%` it would glue a
license header into the following command's doc comment, so we bind only the
maximal blank-line-free suffix.

Whitespace stays a bare leaf token and is never wrapped. The bound
leading-comment run is the one named-node exception. This is a CST-shape
convention enforced by tests, not a hard oracle.

## The event stream

The parser emits events rather than a tree:

```
lexer → token stream → parser events (Start / Tok(idx) / Finish)
      → tree_builder re-attaches trivia and feeds rowan's GreenNodeBuilder
```

Tokens are referred to by index. There is no `Error` event; diagnostics ride a
side channel keyed by byte range. One extra event, `SubTok { idx, start, end }`,
attaches a `WORD` sub-slice for the math operator split. Losslessness holds
because a token's `SubTok` pieces cover its full byte range contiguously.

## Error recovery

A single syntactic error never fails the whole parse; errors travel alongside
the tree. The recovery anchors are `\end{…}`, `\begin`, a blank line, `}`, `$`,
`&`, and `\\`. The parser always makes progress and never loops on unexpected
input.

## Incrementality

Incrementality is salsa-first. Cross-file and cross-query incrementality is the
v1 story; intra-file reparse that reuses green subtrees is a later optimization,
since a whole-file reparse of a typical `.tex` is sub-millisecond.

Green nodes are stored in salsa, never red ones, because red trees are not
`Send`, `Eq`, or `salsa::Update`. `incremental.rs` uses
`#[salsa::input] SourceFile { path, text }` and a `parsed_document` query
returning `rowan::GreenNode` plus diagnostics under
`no_eq, unsafe(non_update_types)`, sound because the tree is a pure function of
the text, and materializes red cursors on demand.

Salsa's default input durability is `LOW`, so every input is `LOW` unless
constructed otherwise. `SourceFile.path` is built at `Durability::HIGH` because
it is set once and never mutated; `text` keeps `LOW`, since a keystroke rewrites
it. The interned `Project` is already `NEVER_CHANGE`. Per-field revision
tracking already stops a path-only query from re-running on a text edit, so
durability only adds the coarse global short-circuit, which starts to matter
once a genuinely rarely-changing input exists.

Hence the rule for growth: any future salsa input promoted from config or
package metadata must be constructed at `HIGH` or `MEDIUM`. Left at `LOW`, every
keystroke's global revision bump would invalidate it. Today that data
deliberately lives in `LazyLock` and `OnceLock`, outside the database.

## Typed AST wrappers

On top of the untyped rowan CST sits a thin typed layer: rust-analyzer-style
`AstNode` and `AstToken` traits, a twelve-line `ast_node!` identity macro (not
codegen, the accessors are hand-written), and one wrapper struct per node kind
(`Command`, `Group`, `Optional`, `NameGroup`, `Begin`, `End`, `Environment`,
`ControlWord`). Add more only when a field-extraction consumer appears; `Math`
and `Scripted` stay unwrapped until then.

Wrappers are a read-only view, never a re-model of the tree. They expose
structure (a command's name token, its positional argument groups, an
environment's `\begin` and `\end`) and never meaning, so no signature lookup
lives here. Because the CST is greedy and generic, accessors are positional:
`Command::nth_group(n)` filters `GROUP` only, so an `OPTIONAL` never shifts
brace indexing, and over-attachment is tolerated by construction. They never
pretend arity is fixed. `Command::title()` would be a lie, since a `\section`
and a `\newcommand` share the `COMMAND` shape. Navigation uses the generic
`child`, `children`, and `child_token` helpers, which replace the raw
`children().find(|c| c.kind() == X)` idiom at field-extraction sites.

Being read-only, the wrappers cannot threaten losslessness or idempotence.

The formatter deliberately stays raw for structural work: the `lower_node`
dispatch and the token-classification loops are ordinary tree walking that
wrappers would only obscure. It adopts wrappers for field access alone.

The pre-wrapper free functions (`command_name`, `environment_name`,
`nth_group_text`) remain as kind-agnostic shims over the wrapper bodies. They
read whatever relevant child a node has without gating on the node's own kind,
because callers rely on that latitude: a `.dtx` `\begin{macro}{\foo}` reads a
`GROUP` off a `BEGIN`, and an xparse default body handed to `group_inner_source`
may be an `OPTIONAL`. The typed methods are kind-checked at `cast`; the shims
are not.
