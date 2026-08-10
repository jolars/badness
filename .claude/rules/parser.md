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
- **The tree is a pure function of the text.** Attachment and grouping read the
  input plus compiled-in data only — never config, package scopes, or scanned
  definitions (beyond the two-pass verbatim scan). Consulting the signature
  database during grouping would invalidate every parse on every signature edit.
- **Meaning never enters the syntactic layer.** Static lexical facts only.
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
  or user tiers — a wrong route is a structural change.
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
  braces; a demoted `\begin` demotes its orphaned `\end` in step.
- **`$`, `\[`, `\(` open math only when a closer is reachable** before an
  unbalanced `}`, an unowed `\end`, a paragraph break, chunk end, or EOF. The
  paragraph-break anchor applies only between top-level atoms of the math body.
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
  than fails. Demotes silently. One forward scan per live opener; every ordinary
  anchor cuts it short, so real corpora are unaffected — see `TODO.md` for the
  pathological shape.
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
  claims. In text: mirroring the `$` gate.
- **Arity-directed expl3 attachment is a recorded candidate, deliberately
  unimplemented.** Do not implement without answering the three open questions
  in `TODO.md` (mixed-shape CST, false-positive blast radius moving into the
  tree, texlab divergence ledger).

## Trivia

- Comments bind forward, whitespace floats, a blank line breaks the bind.
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
