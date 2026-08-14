---
paths:
  - "crates/badness-formatter/**/*.rs"
  - "src/formatter.rs"
  - "src/formatter/**/*.rs"
---

# Formatter rules

Narrative overview: `docs/src/development/architecture.md` § *The formatter*.

## Hard invariants

- **Whitespace-only.** The layout engine changes trivia (whitespace, newlines,
  comments, `.dtx` margins and guards) and nothing else. It never inserts,
  deletes, or rewrites a non-trivia token. Pinned by the non-trivia-content
  oracle in `assert_format_invariants`.
- **Content rewrites are linter autofixes, never layout.** `x^{2}` → `x^2`,
  `$$…$$` → `\[…\]`. Mirror of fix-then-format: the formatter never runs inside
  `--fix`, and content rewrites never run inside `format`.
- **Idempotence.** `fmt(fmt(x)) == fmt(x)`.
- **Protected regions are never altered**, with one carve-out: line terminators
  normalize document-wide (`FormatStyle::line_ending`).
- **The formatter is the sole authority on layout.** Push back on hard-coded
  special cases. Content must not steer layout (no magic trailing comma, no
  "expand past N entries").
- **Never paper over a parser bug here.** Fix it in the parser.

## Trivia-invariant layout

Layout is a function of non-trivia content, config, and only those trivia
predicates the formatter *preserves*.

- **May read:** blank-line presence; comment presence and own-line-ness; a
  column-0 `%` margin or `%<…>` guard.
- **Must never read:** whether a gap is a lone newline or a space. The formatter
  converts freely in both directions, so any rule keying on it is a latent
  idempotency bug — this is the root cause of the whole K&R/Allman family
  (#71, #94, #96, #97).
- **The boundary enforces this.** A consumed trivia run arrives as a normalized
  `Gap` (`Glued | Space { flat } | Blank | Comment`) with **no `Newline`
  variant** — inline whitespace and a lone newline are one variant, so a rule
  cannot key on what it cannot see. `Gap::flat` is the one-line spelling (a
  single space wherever the run held a newline, else the authored whitespace);
  `Gap::separator` is the split point (`Ir::Line` at a gap, `Ir::SoftLine` where
  the author glued). Take `consume_gap`; reach for `consume_gap_widened` /
  `consume_widened_gap_slice` only as a Tier-2 site, and write the fixed-point
  argument when you do.
- **Tier 2 sites** (`WrapMode::Stable`/`Sentence`/`Semantic`,
  `ReflowKind::Statement`, the expl3 fallback statement, the
  command-only-line residue, the delimited-group block residue on
  `spans_multiple_lines`, and the preservation-only boundaries —
  `classify_trivia`, `lower_prose_stream`, `MathWrap::Preserve`) read the unsafe
  predicate by definition and **must carry a written fixed-point argument**
  showing every layout they can emit re-reads to itself. The preservation-only
  ones have the easy version: their output is their own input, and they never
  convert between the two spellings.
- **The command-only-line residue is Tier 2, not Tier 1.** Curated block
  commands are intercepted upstream via `CommandSig::block` and never reach it,
  so `line_is_command_only` decides only for un-signatured and
  scanned-definition commands (block-ness undecidable without meaning) and for
  block commands glued to adjacent content — and retiring that would glue every
  authored `\mymacro`-on-its-own-line into the fill, a policy change. Its
  fixed-point argument is written on the function: preservation-only — a kept
  break re-reads to itself in place, and a hardened width break coincides with
  the first-fit fill's own (`reflow_command_stranded_by_width`). It does
  **not** fire inside a signature-proven prose argument body
  (`ReflowKind::ProseArg`): there the hardening mints a forced break only
  pass 2 can see, and the bit leaks upward through `contains_forced_break`
  readers, flipping the enclosing group between its forms across passes.
- **No Tier-1 site remains.** Under `Reflow`, opaque brace groups
  (`lower_opaque_group`) and optionals (`lower_optional`) are width-driven:
  flat is byte-identical to the generic path except a lone-newline run renders
  as one space, break opportunities are exactly the perturbation-eligible gaps,
  a glued junction never breaks, and only preserved predicates (interior blank
  line, comment) or forced-break content decline to the block form. Edge
  padding joins the vanish-when-broken protocol only when its flat spelling is
  a single space — the one spelling a break reproduces. The surviving
  `spans_multiple_lines` readers are the delimited-group residue behind the
  non-`Reflow` modes and the doc-margined corner, Tier 2 with the fixed-point
  argument written on the predicate. Don't add a Tier-1 read.
- Count *decisions that differ*, not call sites. Several places branch on
  `newlines == 1` yet emit `" "` either way — normalizations, not decisions, and
  the oracle collapses a run to one character so it cannot see them. Nobody
  should "fix" those.
- The convergence oracle (`formatter::perturb`, `badness debug format --checks
  trivia`) gates today. Strict invariance is the end state, and already has a
  surveying surface (`--checks trivia-strict`) — the only mechanical way to find
  a Tier-1 read, since both spellings are self-consistent fixed points that
  every other check passes.

## The printer

- **`Mode::Flat` is a verified claim, not a hint.** Dispatch flat only after
  verifying the whole subtree's flat rendering fits — every line of it. Consumers
  trust it and will not re-decide.
- A hug that verified only a prefix claims **`Mode::FlatPrefix`**, so groups past
  the first forced break re-decide. Claiming `Flat` there prints overflow.
- Measure from `Writer::current_col()`. A fit verified from the wrong column is
  a lie the honest contract will then pin.
- A group's fit measures **the rest of the line**, and a later group in that rest
  is measured in the mode it will actually print in — measuring a doomed group
  flat charges width that never lands, and the charge depends on the previous
  pass's output.
- **Exactly two measurement walkers exist (`flat_end`, `line_fits`). Do not add
  a third.** They replaced five hand-copied traversals that had drifted apart;
  differences belong in their policy enums, not in a new walker.
- `Ir::propagate_breaks` makes `expand` the single representation of "forced
  open". Hug groups are never marked; flat-footprint and hug-prefix measurements
  deliberately ignore it.

## Wrapping and reflow

- **Reflow safety is structural, never file-kind-derived.** Every file kind
  defaults to `WrapMode::Reflow`; every gate is wrap-mode independent, so
  `--wrap reflow` on a `.dtx` is as safe as any other mode. **Never re-introduce
  a file-kind wrap default to paper over a layout bug — fix the gate.**
- Every relayout arm refuses a subtree carrying `DOC_MARGIN` or `GUARD`.
  `LineBuilder::margin_escaped` is the residual backstop; on escape,
  `lower_dtx_doc_paragraph` falls back to the byte-faithful preserve path.
- A `.dtx` `macrocode` frame lead is matched literally by docstrip — commit it
  byte-exact, never normalized to the canonical `%`.
- **A curated block-level command is a block-level statement:** a break before
  it and after it, from `CommandSig::sectioning` or `CommandSig::block`
  (`command_is_sectioning`/`command_is_block`), never from the trivia the
  author wrote. Routing these through the command-only-line rule instead read
  the lone-newline predicate. A non-sectioning block command **glued** to
  adjacent non-trivia keeps its authored adjacency and falls to the residual
  rule — breaking there materializes a space token (`\ProcessOptions\relax`,
  the glued-divider principle); a heading splits even glued, since its own
  `\par` discards the materialized glue. Neither fires under
  `ReflowKind::Statement`, whose Tier-2 contract is the authored line. A
  trailing `%` still rides the statement's line (`prev_block_closes_line` lets
  the comment ride but not content); blank lines around it are preserved,
  never synthesized.
- **The `\begin` header ends at the last element glued to it.** Declared
  arguments glue whatever the author wrote between them (`Signatures`
  arity — `\begin{tabular}\n{cc}` joins); past that, the header continues only
  while each boundary is `Gap::Glued`, and everything from the first gap is
  *body* (`lower_begin` → `BeginParts::tail`, spliced by `lower_env_body`).
  Attachment past the declared arity is an accident of greed (decision #8), not
  an argument claim, so rendering it as one strands it at the `\begin` column
  (`\begin{center}\n{\bfseries A}`) while gluing it up dresses body content as an
  argument. Only `Glued`-versus-not is read, so a lone newline never reaches the
  decision. **The tail must be spliced into the leading paragraph's reflow, not
  concatenated ahead of it** — a paragraph trims its own leading newline, so
  concatenation abuts the two with no separator and deletes a space TeX typesets.
  A header `%` keeps the whole node byte-faithful when an arity is declared:
  gluing the argument across it comments it out, and bodying it takes a
  `tabular`'s colspec from the grid. Pinned by `begin_tail_is_body`.
- **A `CONDITIONAL` is all-or-nothing:** flat when the whole construct fits, else
  every divider opens a line. Offered as two whole candidates
  (`Ir::conditional_group_all_lines`), **never as one `Ir::group` of `Ir::Line`s**
  — a group saturates its break state from the subtree, and a branch interior
  carries a forced break for every line the command-only-line rule keeps, so the
  group would decide the dividers from the interior's authored newlines. The flat
  candidate is collapsed from *content* (`collapse_conditional`), so the choice
  reads width and content only. No body indent: the `\if` test's extent is not
  statically resolvable, so there is no head/body split to hang one off.
- **A glued divider is never broken.** `Ir::Line` is a space flat and a newline
  broken, and TeX contributes both to the horizontal list, so breaking where the
  author glued (`\ifmmode y\else z\fi`) materializes a space token — a typeset
  change no CST oracle can see. Any glued boundary sends the whole construct down
  the byte-faithful path; breaking only the unglued siblings is the lopsided form.
- **The relayout runs only where prose is laid out** (`cx.wraps_prose()`).
  `WrapMode::Preserve` promises authored breaks are untouched, and the
  all-or-nothing choice would rejoin a conditional the author spread over lines.
- **A branch interior is lowered by its *enclosing* context, not by itself.** No
  `PARAGRAPH` nests in a branch (the gate keeps a conditional inside one), so the
  ancestor decides: paragraph → prose reflow, group/argument → byte-faithful
  stream. Feeding macro code to the prose reflow oscillates — `\ifx\\#1\\` puts a
  `LINE_BREAK` node in an operand slot and the "a `\\` ends its line" rule flips
  it every pass (`pagesel.sty`).
- **A `CONDITIONAL` child is not always a branch or the closer.** An own-line `%`
  run before the opener is reparented *into* the node as a `DOC_COMMENT`. Walk
  the expected children only and it is deleted — invisibly, since a comment is
  trivia to the non-trivia-content oracle. The comment oracle in
  `assert_format_invariants` is what catches this; keep it green.

## Optional arguments

- Width alone decides expansion.
- **A glued comma split (inside a `WORD`) is emitted only under
  `ContentKind::Keyval`**, because breaking there materializes a space token TeX
  will see. A gap split (comma already followed by whitespace) is free anywhere.
- **`Keyval` must never be set on an argument whose content is typeset.** It
  changes typeset output; hold it to the curated standard of math routing and
  verify with `task typeset:check`.
- **Comma segmentation is delimiter-agnostic; the *proof* is what gates it**
  (`lower_segmented_group` / `segment_delimited_body` take the open/close kinds;
  `lower_optional` is the bracket entry point). A `{…}` reaches the segmented
  layout only through `ContentKind::Keyval`, so the keyval-family setters
  (`\pgfkeys`, `\tikzset`, `\lstset`, `\setlist`, …) get one entry per line
  instead of a prose reflow that wrapped mid-key. Everything else keeps the
  opaque lowering — a mandatory group is the ordinary home of typeset text, and
  a wrong flag there is far worse than on a bracket.
- **Mandatory `Keyval` comes from the curated tier only.** The CWL generator
  still drops a `%keyvals` mark on a `{…}` (`gen_cwl_signatures.py`,
  `_parse_arg_shape`): the consumer now exists, but the mark is mechanical and
  unvalidated, and its blast radius on mandatory groups is unmeasured. Lifting
  that scoping is a separate, measured change — not a side effect of curating a
  name. Pinned by `keyval_group_splits_entries`.
- **The keyval proof also lifts `lower_bracketed`'s `open_glued` guard.** That
  guard exists because a break after a glued `{` materializes a space token
  (TeX state M), and it was scoped by *delimiter* — sound only while keyval
  lived on brackets. A proven-keyval body carries the same license under a
  different name, so it takes the Allman break whatever its delimiter;
  otherwise the group glues its opener while its closer still takes its own
  line. Any new keyval-shaped exemption keyed on `open ==` is that same proxy
  coming back.
- **A mandatory `Keyval` is deliberately *not* wired on the `\begin` path**
  (`lower_begin` keeps `keyval && is_bracket`). No environment is curated with
  one, and an environment header answers to rules a `[…]` does not: the grid
  router reads the colspec group, and a verbatim-body header line may never
  break at all.
- Comma splits belong here, not in the lexer: a comma is catcode 12 and
  indistinguishable from `=` or `5`, so splitting on it in the lexer would encode
  keyval-ness into lexing.
- **Never break the `\begin` line of a verbatim-body environment**, however wide
  it gets and whatever its optional's `ContentKind` licenses. That line *defines
  where the protected body starts*, so a break moves the first body byte —
  and for `filecontents`, those bytes are written to a file. `filecontents`'s
  optional is `Keyval`, so this is the one place the comma-split rule above must
  not reach; it falls out of `has_verbatim_body` routing the environment to the
  verbatim arm, not from a width special case. Pinned by
  `filecontents_protected_body`.

## Comments

- **A `%` ends its line, so nothing the formatter emits may follow one there.**
  Every lowering that owns a delimiter encodes this: `lower_bracketed` and
  `lower_prose_group` put the closer on its own line, `collapse_arg_group` /
  `segment_delimited_body` / `lower_opaque_group` decline their flat form
  outright. The
  cost of forgetting is a *deleted closing delimiter* (`\caption{x%}`), which the
  whitespace-only oracle sees only as a comment that grew a `}`.
- **A comment glued to an open delimiter rides that delimiter's line.** Moving it
  down converts the newline the formatter writes after `{` into a real space
  token inside the group — `\caption{%\n}` (empty) becomes `\caption{ }`. Glued
  is the test, and the parser makes it a cheap one: leading whitespace is its own
  trivia token, so a `COMMENT` first in the body means the author glued it.
- The mirror on the other side is the *trailing* comment rule: it rides the line
  it was authored on and is **never relocated**, because an own-line `%` rebinds
  as the next construct's `DOC_COMMENT` on reparse (issue #38).

## Protected regions

- **The protected region extends through the `\end` marker's own indentation.**
  `VERBATIM_BODY` spans from the newline after the `\begin` args to the newline
  before `\end`, so that leading whitespace is *content*, not layout. Hence the
  deliberate asymmetry in a nested verbatim-family environment: the `\begin` line
  reindents with its parent, the body and the `\end` line stay byte-exact where
  the author put them. This is not a bug to be "fixed" into symmetry — indenting
  the `\end` would rewrite a protected token. Pinned by
  `verbatim_in_environment`, `verbatim_argument_environment`, and
  `filecontents_protected_body`.

## Tables

- Grid routing is curated-first; the top-level-`&` arm runs **after** the curated
  arms so a stray `&` in an `itemize` never reroutes it.
- Key on `&`, never `\\` — a `\\`-only body is a line stack, not an alignment.
- Exclude doc-margined bodies; grid padding would push a `%` off column 0.
- `colspec` bails to all-left on any token it does not model.

## expl3

- The formatter owns in-region layout regardless of `WrapMode`, because
  in-region spaces are catcode 9. This is idempotent by construction.
- **Layout ownership is positionally gated** (`toggle_is_top_level`): reject a
  toggle in definee position or nested in a group/definition body. The
  byte-level oracles cannot catch mis-ownership (#69).
- **Statement boundaries are structural** (`semantic::expl3::segment_expl_statements`),
  not newline-keyed. The fallback (authored physical line) is Tier 2 and carries
  its own fixed-point argument.
- **No arm of the forced-break dispatch may fire inside a fallback statement.**
  Committing a line mid-statement there is not pass-invariant.
- Structural statement lines commit as `Ir::StickyFill`; fallback and junk-glued
  lines as `Ir::HugFill`. **An early line commit must build its head with the
  same fill kind the line would have committed as.**
- **Each brace argument breaks on its own body.** No sibling coupling — a
  sibling's forced break is none of its business (l3styleguide's own example).
- **A conditional's branches are resolved from the call *unit*, not the head
  node** (`semantic::expl3::expl3_unit` records each `T`/`F` slot's range). Where
  greedy attachment put a branch group is an accident of the surrounding tokens
  — an `N`/`V` slot hands it to a sibling — so it must never decide the layout.
  A *statement-leading* conditional explodes on that, unconditionally.
- **Only statement-leading position may use the unit rescan.** Mid-statement the
  conditional is an argument being passed as a token, not a call
  (`\@@_patch_check:NNnn \cs_if_exist:NTF #1 { undef }`); resolving a unit headed
  there claims the *outer* call's arguments as branches. The trailing arm reads
  the node's own attached children only.
- Trailing comments ride their line zero-width and are **never relocated**;
  moving one rebinds it as the next statement's doc comment.
- A group whose body carries a `DOC_MARGIN`/`GUARD` must take the broken form;
  flattening re-lexes the guard as a `%` comment that swallows the closing brace.
- Target is `l3styleguide.tex`. Its non-layout rules (naming, expandability) are
  meaning, not trivia — out of scope, linter territory.

## BibTeX

- The bib formatter is a canonical re-emitter, not a trivia-only pass: it
  reorders entries and fields and rebuilds every line. **So anything it does not
  explicitly emit is deleted.** Adding a node kind to the bib CST means teaching
  `lower_entry` (and the `@string`/`@preamble` bails) to emit it, or the
  content silently disappears.
- **A value carrying an unescaped `%` never reflows.** `%` is an ordinary
  character to BibTeX and a comment to the LaTeX that typesets the value, so its
  line breaks are content. Neither the bib CST oracles nor the gate can see
  this — joining two lines there is byte-legal and typeset-wrong.
- **A `%` comment binds to a field, never to an offset** — that is what carries
  it through the canonical sort. Same-line comments ride their field's line
  (never relocated, as on the LaTeX side); every other one hoists above the
  field below it. Pinned by the comment multiset oracle in `bib_format.rs`.

## Line endings

The printer always emits `\n`; `line_ending` is a post-pass over finished text.
`auto` is the default so formatting never rewrites endings behind the author's
back. Only the `\r\n`/`\n` pair converts; a lone `\r` is left as authored.
