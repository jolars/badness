---
paths:
  - "crates/badness-parser/**/*.rs"
  - "crates/badness-parser/data/*.json"
  - "src/parser.rs"
  - "src/semantic.rs"
  - "src/semantic/**/*.rs"
  - "src/incremental.rs"
  - "src/syntax.rs"
  - "src/ast.rs"
---

# Parser rules

Narrative overview: `docs/src/development/architecture.md` § *The parser*.

## Hard invariants

- **Losslessness.** `reconstruct(text) == text`, byte for byte. Every new
  feature needs a losslessness assertion.
- **The tree is a pure function of the text and the declarations.** Attachment
  and grouping read the input plus compiled-in data only — never package scopes,
  the CWL tier, or scanned definitions (beyond the two-pass *self-definition*
  scan: user verbatim commands and environment aliases, both read from the file's
  own definitions). Consulting the signature database during grouping would
  invalidate every parse on every signature edit. The one non-text input is the
  explicit `Declarations` value of decision #12 (`badness.toml`, seeded into
  `ParseCtx`): a closed, hand-authored vocabulary at `Durability::HIGH` that
  *names constructs* and never directs attachment. **A declaration names a
  spelling, never a pairing** — every shape gate still runs unchanged, so a wrong
  declaration demotes like an inferred one instead of corrupting a tree. Never
  widen it into a general signature-DB read.
- **Meaning enters the syntactic layer only through the admission test**
  (AGENTS.md decision #2). A fact may shape the tree when every entry is
  individually vetted (curated built-in or declared) **and** its misapplication
  is falsifiable from the text, so a gate can demote it. Routing and pairing
  facts qualify; database arity never does — a wrong arity mis-attaches
  byte-identically, past every oracle. The bulk CWL tier fails both bars.
- **Errors never abort the parse.** Recovery anchors: `\end{…}`, `\begin`, blank
  line, `}`, `$`, `&`, `\\`. Always make progress; never loop.

## Degradation

- Anything not statically resolvable degrades to a generic node — never a crash,
  never corruption.
- **A gated construct gets no diagnostic.** Parser diagnostics gate the
  formatter, so they must be high precision; a shape that is routine in macro
  code is not statically an error. Lone-`$`-in-prose typos are linter territory.
- **A shape gate must mirror the parse it guards.** A gate stricter than the
  parse drops the node and refuses the whole file to the formatter. Test both
  directions when touching one.

## Lexer modes

New modes read static facts only and go in the catalog in `architecture.md`.
Prefer false negatives; when in doubt a construct stays generic.

- **Math environment routing reads the curated `math` flag only**, never the CWL
  or user tiers — a wrong route is a structural change. A *declaration* counts as
  curated (`like` copies a built-in entry and resolves against nothing else), so
  `ParseCtx::is_math_environment` answers from the declared signature when there
  is one. A declared entry is **authoritative for its name**: every routing
  predicate answers from it alone, never merged with the built-in or the scan.
- **The verbatim definition scan prefers false negatives**; a false positive
  suppresses real diagnostics.
- **expl3 toggle names are a set shared with the formatter**
  (`parser::lexer::expl_toggle`) so the two cannot drift. The *positional* gate
  on layout ownership is the formatter's alone — do not move it here. Mis-lexing
  a name only splits CST tokens (lossless, cosmetic); mis-owning layout rewrites
  meaning (#69).
- **Environment pairing is gated on brace structure**, not a command set: an
  environment cannot outlive the brace group its `\begin` opened in (#71,
  generalizing #45/#55). `.dtx` doc-margin lines are exempt from stranded
  braces; a demoted `\begin` demotes its orphaned `\end` in step. Batched as
  `EnvGate` (container-stack C2.2), the driver's first **demotion** gate, so its
  verdict reads inverted (`Some` = escapes = demote) and two policies flip with
  it: a stray `}` *closes* rather than refutes, and math is not an anchor —
  refusing there would keep an environment the scan cannot vouch for. The
  `group_depth` and doc-margin pre-checks are per-opener walk state and stay
  outside the batch.
- **`$`, `\[`, `\(` open math only when a closer is reachable** before an
  unbalanced `}`, an unowed `\end`, a paragraph break, chunk end, or EOF. The
  paragraph-break anchor applies only between top-level atoms of the math body.
  Both run on the shared batch driver (container-stack C2.3) as `DollarGate` and
  `DelimMathGate`, **single-entry**: they open no nested entry, so a batch
  settles its seed alone — a delimiter that pairs swallows every opener up to
  its closer, so there is no same-frame neighbor to settle. Four policies invert
  against the pairing gates: a `}` refuses whether or not a group encloses the
  opener (`StrayBrace::RefutesAlways`, mirroring `dollar_math`/`delim_math`,
  which bail at any unbalanced `}`), a foreign math delimiter is content rather
  than an anchor, environments count at *any* brace depth, and the closer needs
  no environment balance. They also read a `macrocode` frame as an ordinary
  environment rather than a hard boundary — preserved from the pre-batch scans,
  not chosen; nothing in the corpora depends on it. The `$` gate is
  **unmemoized**: a demoted `$$` re-enters on its second `$` with
  `display: false`, a different question about the same index under the same
  walk state, which a walk-state-keyed slot would answer from the first verdict
  (`demoted_display_dollar_regates_its_second_dollar_as_inline`).
- **`\left` opens a pair only when its `\right` is reachable**; otherwise it is a
  plain command with no diagnostic (#77), a likely typo being linter territory.
  Runs on the driver as `LeftRightGate` (container-stack C2.4), the only gate
  whose entries **stack** instead of counting (`Nesting::Interleaved`): a pair
  closes by count wherever it sits, so `{`, `\begin`, and `\left` share one LIFO
  stack, and the two halves of that read differently. A frame **mismatch** (an
  `\end` or a `\right` meeting a frame of the wrong kind) refuses the whole scan,
  since the innermost frame is common to every outer entry
  (`an_end_inside_a_nested_left_refuses_the_whole_scan`); the *absence* of frames
  the blank-line anchor tests is seen only by the innermost entry, so a nested
  pair **shields** the ones around it
  (`a_nested_left_shields_the_outer_pair_from_a_paragraph_break` — where the gate
  is looser than the walk, which bails at the break; pre-existing, preserved by
  the migration). Its math anchor is the *closing* side (`MathAnchor::Closing`):
  a `\left` lives inside math already, so `$`/`\]`/`\)` end it while a `\[` is
  content. **Opener and closer recognition ignores `in_macro_code` on purpose**
  where the driver's `\begin`/`\end` counting does not — the pair is
  catcode-neutral and a `\def` body or `macrocode` chunk is exactly where
  `$\left#2\right#4$` lives (#95). Do not "fix" that asymmetry.
- **`\if…\else…\or…\fi` pairs only when the `\fi` is reachable at the opener's
  own brace, environment, *and* math level**; a `macrocode` frame bounds it both
  ways, and the walk is bounded by the located closer index. A gate that counts a
  `\fi` the walk will consume inside another construct promises a pairing it
  cannot honor, and the walk overruns (`ltboxes.dtx`, three `\fi`s inside `$…$`).
  The bound is **one-directional on purpose**: the walk never runs past the
  located closer, but may close earlier when it demotes a nested opener the scan
  counted by name — so `Conditional::closer` is fallible and no consumer may
  assume the two indices agree. Opener recognition lives in `parser::conditional`,
  **shared with the linter's `ConditionalIndex`**, recognizer and state machine
  both, so the two can never disagree about what an opener *is*; each still
  layers its own filter on the result (parser: no expl3 regions, since the
  formatter owns layout there; linter: `\def` bodies withheld entirely, since a
  carried `\let` must not arm the operand countdown). Subtracting the
  brace-argument `if*` family is load-bearing, since shape alone mis-pairs rather
  than fails. Demotes silently. Verdicts come in **batches** (container-stack
  C1): one scan, bounded by the last `\fi`-flavored word in the file, settles
  every same-frame opener it passes, memoized against the walk state it read.
  The batch's load-bearing rule: a refuted entry is **settled, never removed** —
  the per-opener model counts nested openers by name and never un-counts one, so
  a later `\fi` must still be consumed by the refuted entry's slot
  (`a_refuted_nested_opener_still_consumes_a_fi`). The batch is a **shared
  driver** (`Parser::gate_batch`), not this gate's own machinery: it owns the
  bookkeeping every gate repeats and takes a `GatePolicy` per gate. All nine
  gates run on it (container-stack C2 is complete); never average two gates'
  policies into the loop — where two read the same token differently, add a
  named axis with its reason, and say whether it was *chosen* or *preserved*.
- **Environment aliases pair behind a *positive* gate** (#109). A command whose
  body is exactly `\begin{X}`/`\end{X}` stands in for that delimiter. Target must
  be curated built-in, non-verbatim, argument-free; alias must be arity 0; both
  halves must be defined in the same file. A pair may instead be **declared**
  (`[environments.<target>] begin/end`, decision #12), which drops the
  same-file and inference-shape requirements but keeps the target rules and the
  gate: declared and inferred aliases land in the same `ParseCtx` maps and are
  indistinguishable downstream. Non-verbatim is *TeX truth* for a declared pair
  too — the closer alias is never expanded, because verbatim already swallowed
  it — so a declared `begin`/`end` on a verbatim target is a config error, while
  a name-only `[environments.x] like = "lstlisting"` is fine. The opener index
  **must exclude every name being bound**, as a slot countdown
  (`definition_name_slots`) and not a
  one-word test — `\def\bea{…}` leaves the definee at brace depth 0 with
  `in_def_body` unset, so unfiltered the two definition lines pair with each
  other, and `\let\oldbea\bea` leaves the *source* operand live to pair with the
  next stray closer. Gate is modelled on `conditional_closer` (locate the closer,
  bound the walk by it, EOF does not pair), **not** on the `\begin` demotion
  gate, and has no paragraph anchor. Demotes silently. Bound the walk by the last
  closer in the file and memoize the verdict — the caller asks twice, and openers
  that never pair are otherwise quadratic. Runs on the shared batch driver as
  `AliasGate` (container-stack C2.1); its only policy divergences from
  `ConditionalGate` are the absent paragraph anchor and the closer's name match
  (`GatePolicy::pairs`). Not extended to `math_atom` in v1.
- **Picture-body statements wrap retrospectively, with no gate.** In a curated
  `statementBody` environment body (`ParseCtx::is_statement_environment` — the
  `is_math_environment` template: curated built-ins plus declarations, never
  CWL or the scan), `parse_block`'s run loop wraps each run up to a top-level
  `;`-carrying `WORD` in a `STATEMENT` node via the `PARAGRAPH` `precede`
  idiom. Recognition is retrospective pure shape, so there is no `GatePolicy`,
  no `PreScan` index, and no scan-work cost — the gate-mirrors-walk concern
  cannot arise. A run that never reaches a `;` (blank line, `\end`, alias
  closer, EOF first) stays plain paragraph content, silently; a genuine
  `\begin` or pairing alias opener is a **statement boundary**
  (`statement_boundary`, mirroring the `element` dispatch gate verdicts —
  a *demoted* `\begin` stays statement content); recognition never reaches
  `group()`/`conditional()` element loops, which is what "top-level" means.
  The `;` is catcode-12 and lexes inside `WORD`s (`(1,1);` is one token), so
  the terminator test is per-token text; a multi-`;` or `;(2,2)`-glued WORD
  over-extends one statement — degradation, not corruption (a `SubTok` split
  is the recorded v2 upgrade). No coordinate/`at`/path-operator grammar:
  statement *extent* is the whole model, which is what keeps this inside
  decision #1.
- **Alias behavior resolves from the node, never the name.**
  `Signatures::environment_at` reads the alias map only for a `Begin::is_alias`
  delimiter; the name-keyed `Signatures::environment` never reads it. A literal
  `\begin{bea}` beside an alias `\bea` is an unrelated environment and inherits
  nothing — that is why aliases are a side map, not a cloned `EnvironmentSig`.
- **Every shape gate is bounded by its last-closer index** (the
  `last_alias_closer` treatment, container-stack C0). A gate can only succeed at
  one closer token shape, so truncating its scan at the last occurrence in the
  file is verdict-preserving — past it only refusals remain — and a file with
  none refuses without scanning. Recording may over-approximate, never
  under-approximate. A bound helps only a file with *no* closer, so a single
  reachable closer at EOF defeats it for every gate — which is what the batch
  driver, not the bound, is for. **Every** gate names one, since the driver
  takes it as `GatePolicy::last_closer` and refuses without scanning when it is
  absent — including the two where it rarely bites: `dollar_closes` (its closer
  is its opener's own token kind, so a file of openers ends at one) and
  `environment_escapes_group` (every `\begin{…}` carries a `}` in its own name
  group, so the index sits near EOF; batched in C2.2). Scan work is metered
  (`Parser::scan_work`) and pinned linear by the tests in `grammar.rs` — extend
  them when touching a gate. Two shapes stay quadratic **by design** and say so
  in their tests rather than pretending otherwise: a `${` per line (the brace
  depth ratchets upward, so no later opener sits at the seed's level) and a
  `macrocode` chunk of `\cmd[` openers whose only `]` is past the frame (that
  gate is single-entry by policy).

- **`.dtx` frames are asymmetric about column 0**: a begin frame may be indented
  (`\MakePercentIgnore`), an end frame is column-0 strict.
- **`.bib` `%` comment-ness is the grammar's call, never the lexer's.** The
  lexer emits a bare `PERCENT`; the grammar wraps `%`…EOL in a `COMMENT` node
  only where it skips trivia inside an entry. A brace group, a quoted string, an
  `@comment` body, and junk never do, so `{50% off}` keeps its `%`. We follow
  biber/btparse here, not classic `bibtex` (which has no comment at all);
  texlab models none either, hence the recorded gauge deviation.

## Argument grouping

- **Greedy is the default and the only text-pure total strategy.** Deviations
  read static facts only: bracket shape gates, `#` and control-word run breaks,
  the starred-variant fold.
- **A bracket attaches only when it reads as an argument.** In math: directly
  abutting with its `]` reachable before the math ends, net of intervening
  claims. In text: mirroring the `$` gate. All three run on the batch driver
  (container-stack C2.5) as `TextBracketGate`/`MathBracketGate`/
  `MacrocodeBracketGate`. The `]`-claim countdown of #55 **is** the driver's
  nested-opener stack once an opener is a command-abutting `[` — no new nesting
  model — and both of the family's anchors are depth-**blind**
  (`EnvAnchor::Refutes`, `ParagraphAnchor::AnyDepth`) because `optional` bails
  wherever the cursor stands. A `$` in math reads the enclosing math's *flavor*
  (`dollar_anchor`, the one runtime policy): inside `\[…\]` it opens a
  **transparent** region where the entries' own brackets stop counting; inside
  `$…$` it is that math's closer and refuses (#99). Flavor is walk state, so it
  rides `WalkKey`.
- **Two bracket divergences are preserved, not chosen** — flipping one is its
  own commit with its own test. The in-math gate's environment anchor ignores
  `in_macro_code` and its braces ignore `plain_braces`; both only ever *decline*
  to attach, and the second is arguably the faithful reading, since `optional`
  bails at any `R_BRACE` without consulting `plain_braces` (so its two siblings
  are the loose ones: an attached optional holding a chunk-plain `}` still
  reports "unclosed `[`").
- **Arity-directed expl3 attachment is landed** (decision #8's sanctioned
  deviation; TODO.md records the migration). In-region colon-suffixed heads
  attach by argspec arity via `grammar/expl3.rs`: a pure `&self` token scan
  produces an `Expl3Plan` the walk **replays exactly** — the scan mirrors the
  walk by construction, and the per-arg `debug_assert`s are the tripwire.
  `w`/`D`/colonless heads and the `\::n` drivers stay greedy. The scan aborts
  to greed, with no diagnostic, wherever it cannot mirror the walk: in-math
  heads (an `N` slot would swallow the enclosing closer — `xo-grid.dtx`), a
  `GUARD`/`DOC_MARGIN` mid-unit (#78), a candidate that would form a node
  (gated `\begin`, live conditional opener, bound `DOC_COMMENT` run), an
  unreachable group closer, or a paragraph separator (which ends the walk's
  stream — `latex-lab-sec.dtx`); a blank-line gap inside a brace group instead
  commits the consumed prefix. An `N` argument keeps its `COMMAND` node
  (`command_bare`) so name-keyed consumers still see it. Mis-attachment is
  byte-invisible, so the migration's oracle was a diff of grammar attachment
  against `semantic::expl3`'s independent consumption over the gate corpora
  (67k statement-leading heads, zero disagreements outside the benign
  greedy-leftover class); the corpus fixtures (`corpus/expl3_arity.tex`) are
  the net since.

## Trivia

- Comments bind forward, whitespace floats, a blank line breaks the bind.
- **A docstrip guard breaks a shape gate's paragraph run; a `.dtx` doc margin
  floats through it.** Docstrip deletes a guard-only line outright, so `%<*dtx>`
  between two lines does not part them (#71): the guard breaks the newline run
  without being a newline — `TriviaScan::saw_blank_line_outside_guards`. This is
  the **driver's** model, shared by every gate, not a per-gate policy;
  `MacrocodeBracketGate` was alone in it until the `DOC_TRIVIA_FLOATS` knob went
  away. A margin-only line is still the documentation layer's blank line, so
  margins keep floating.
- Bind only the **maximal blank-line-free suffix**. Do not adopt
  rust-analyzer's peek past a blank line: it keys on `///` vs `//`, and `%` has
  no equivalent, so peeking glues license headers into doc comments.
- Whitespace stays a bare leaf; the bound comment run is the only named-node
  exception.

## Typed AST

- Wrappers are a **read-only view, never a re-model**. Structure only, no
  meaning, no signature lookups.
- Accessors are **positional** and tolerate over-attachment (`nth_group` filters
  `GROUP` so an `OPTIONAL` never shifts indexing). Never write an accessor that
  presumes fixed arity — `Command::title()` would be a lie.
- Add a wrapper only when a field-extraction consumer appears.
- Navigate with `child`/`children`/`child_token`, not
  `children().find(|c| c.kind() == X)`.

## Incrementality

- **Store green nodes in salsa, never red.** `SyntaxNode` is not `Send`, `Eq`,
  or `salsa::Update`.
- **New inputs carrying rarely-changing data (config, package metadata) must be
  constructed at `Durability::HIGH`/`MEDIUM`.** Per-file `text` stays `LOW`.
  Left at `LOW`, every keystroke's revision bump invalidates them.
- **The declarations ride a singleton input, written only on a change.**
  `incremental::DeclarationsInput` (`HIGH`, created eagerly so every reader can
  use `get` rather than a dependency-free `try_get` fallback) is read by
  `parsed_document` and `scope_signatures`. Writing it reparses the whole
  database, so `set_declarations` no-ops on an equal value and the language
  server mirrors the last block it published, republishing from the request
  dispatcher rather than per handler (see `lsp.md`). A read job that misses the
  cache takes them off the snapshot (`Analysis::declarations`) — never re-derives
  them, and never answers declaration-blind.
- Don't add intra-file reparse without recording the decision in `AGENTS.md`.

## Generated `data/` artifacts

- `cwl_signatures.json`, the package/class name lists, `package_metadata.json`,
  and `bib_fields.json` are generated. Re-sync via `task <name>:sync`; never
  hand-edit the mechanical facts. `signatures.json`, `colors.json`, and
  `tikz_libraries.json` are hand-curated.
- The CWL tier carries **names, arity, and the `%keyvals` mark only**. Every
  behavior-classification suffix (`#V`, `#\math`, `#L0`–`#L5`) is reported for
  human promotion, never applied.
- **`signatures.json` masks the CWL entry wholesale** (`.or_else`, not a field
  merge), so a curated command that CWL marks keyval needs the flag re-added by
  hand.

## Testing

New parser features need corpus + snapshot tests **and** a losslessness
assertion. `task parse-compat` runs texlab as a differential oracle — a
reference to explain divergences against, not to match.
