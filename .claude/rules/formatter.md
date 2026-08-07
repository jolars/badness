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
- **Keep `Gap` free of a `Newline` variant.** Deleting the information at the
  boundary is the enforcement; a rule cannot key on what it cannot see.
- **Tier 2 modes** (`WrapMode::Stable`/`Sentence`/`Semantic`,
  `ReflowKind::Statement`, the expl3 fallback statement) read the unsafe
  predicate by definition and **must carry a written fixed-point argument**
  showing every layout they can emit re-reads to itself.
- Four sites still read it; three are Tier 2, the fourth
  (`spans_multiple_lines`) is residue filed in `TODO.md`. Don't add a fifth.
- The convergence oracle (`formatter::perturb`, `badness debug format --checks
  trivia`) gates today; strict invariance is the end state.

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

## Optional arguments

- Width alone decides expansion.
- **A glued comma split (inside a `WORD`) is emitted only under
  `ContentKind::Keyval`**, because breaking there materializes a space token TeX
  will see. A gap split (comma already followed by whitespace) is free anywhere.
- **`Keyval` must never be set on an argument whose content is typeset.** It
  changes typeset output; hold it to the curated standard of math routing and
  verify with `task typeset:check`.
- Comma splits belong here, not in the lexer: a comma is catcode 12 and
  indistinguishable from `=` or `5`, so splitting on it in the lexer would encode
  keyval-ness into lexing.

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

## Line endings

The printer always emits `\n`; `line_ending` is a post-pass over finished text.
`auto` is the default so formatting never rewrites endings behind the author's
back. Only the `\r\n`/`\n` pair converts; a lone `\r` is left as authored.
