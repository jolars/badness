# AGENTS.md

Guidance for AI agents working with Badness, a formatter, linter, and language
server for LaTeX.

This file is the **rules** you work under: the tenets, the load-bearing
architectural decisions, and the invariants and conventions to respect.

Two other places carry design guidance:

- **`.claude/rules/*.md`** — per-subsystem directives (parser, formatter,
  linter, lsp), path-scoped in frontmatter so each loads only when you touch
  that subsystem's files. Terse by design: a rule, the one clause that keeps it
  from looking arbitrary, and a pointer. Keep each under 200 lines.
- **`docs/src/development/architecture.md`** — the narrative tour of the whole
  system, one section per subsystem. This is where a decision gets *explained*.

When a decision below changes, update this file (the rule), the matching
`.claude/rules/` file if it carries a directive, and the Architecture page if
the change is visible at the level of the tour. Contributor-facing process lives
in `CONTRIBUTING.md`. Extended roadmap rationale is threaded through TODO.md.

Worked examples and issue archaeology deliberately do **not** live in any of
these. They live in the issue tracker, in `git log`, and above all in named
tests and fixtures — those are the artifacts that fail when a rule is violated,
so that is where a regression case belongs.

## What this project is

Badness follows **rust-analyzer's** architecture: a generic, error-tolerant,
hand-written parser produces a **lossless concrete syntax tree (CST)**; semantics are
layered on top as a separate concern; recomputation is incremental via salsa. On the
CST sit a **formatter** (`badness format`), a **linter** (diagnostics), and a
**language server** (LSP). (We were also inspired by
[arity](https://github.com/jolars/arity), the same kind of tool for R.)

It is a **four-crate Cargo workspace** (edition 2024) whose root package is the
CLI/LSP/linter crate `badness`, with two publishable, **wasm-clean** library
crates and one non-published wasm shim under `crates/`:

- **`badness-parser`** — `syntax`, `ast`, `parser`, `semantic` (minus the
  disk/salsa-backed `load`), the BibTeX parsing+semantic layers, the `data/`
  signature artifacts, and the phf codegen `build.rs`.
- **`badness-formatter`** — the layout engine (`core`, `ir`, `printer`, `style`,
  `context`, `colspec`, `sentence`, `perturb`) and the `.bib` formatter.
  Embedded by the dprint Wasm plugin (`jolars/dprint-plugin-badness`, its own
  repo so `plugin.wasm` stays out of this one's `v*` release stream), so it (and
  the parser it depends on) must keep building for `wasm32-unknown-unknown` — a
  CI job guards this (and covers `badness-wasm` too). Anything touching the
  filesystem, threads, or processes stays in the root crate. The plugin is
  sandboxed with no filesystem, so it passes an empty `SignatureDb` where the
  CLI folds in signatures scanned from sibling `.sty`/`.cls` files — the one
  sanctioned divergence from `badness format`. Its optional `serde`/`schema`
  features (off by default, exercised by their own CI steps) put `badness.toml`'s
  kebab-case spellings on `FormatStyle` and its enums so the plugin's published
  config schema can `$ref` them instead of restating the accepted values; the CLI
  keeps its own `*Config` mirrors in `src/config.rs` regardless. **The wire
  spellings are public API** — `style.rs`'s `serde_tests` pin them, and a new
  enum variant will not compile until it is listed there.
- **`badness-wasm`** — `publish = false` wasm-bindgen shim over the two library
  crates, powering the docs playground (`docs/src/playground/index.html`); built
  with `wasm-pack` via `task playground:wasm`, never released.

The root crate keeps `linter/`, `lsp/`, `project/`, `incremental` (salsa),
`completion`, `config`, `file_discovery`, `text/`, and the CLI, and re-exports
the member crates at the old module paths via **shim modules** (`src/parser.rs`
is `pub use badness_parser::parser::*;`, etc.), so intra-repo consumers write
`crate::parser::…` as before. Bridge modules host the CLI-side halves of split
concerns: `formatter::check` + the disk-backed `format_file_with_packages`
entries, and `semantic::load`. The CLI
processes `.tex`, `.sty`/`.cls`, `.dtx`, `.ins`, and `.bib`; `badness.toml` is
local project config consumed only by the CLI. See the Architecture page for the full
tour.

## Tenets

1. **Deterministic, rule-based formatting.** Layout is decided solely by the
   formatter's rules and the layout engine—the formatter is the **sole authority on
   layout**. Push back against hard-coding special cases. Autofixes are textual edits
   that never invoke the formatter: a fix decides *what* to rewrite, never *how to lay
   it out*, and owes only correctness (the result still parses and is still lossless),
   never line-width. When a fix can't meet that bar for some shape, make it correct by
   construction or withhold it for that shape (still report the finding). The pipeline
   is fix-then-format; don't run the formatter inside `--fix`—and, mirrored, content
   rewrites never run inside `format`: the layout engine changes only trivia (see
   Invariants).
2. **Incremental parsing is first-class.** Parser/CST work must keep the salsa-based
   reparse path (`incremental.rs`) viable.
3. **Parsing is the parser's job.** Never paper over parser mistakes in the formatter,
   and never let parsing logic creep into the formatter. If the formatter hits
   something the parser got wrong, fix it in the parser.
4. **Losslessness is the parser's job.** `reconstruct(text) == text`, always. The
   formatter may assume a lossless CST.

## Core architectural decisions

Load-bearing. If a change pushes against one, raise it explicitly. Each rule below
links to the `.claude/rules/` file carrying its full rationale, examples, and
provenance.

1. **The parser treats input as generic TeX surface syntax and always produces a
   lossless tree.** It never *requires* resolving macros or catcodes; we do **not**
   implement macro expansion or a TeX evaluator. Anything we cannot statically resolve
   degrades to generic nodes (plus a diagnostic where useful), never a crash or
   corruption. We **do** handle a bounded, growing set of *statically recognizable*
   patterns—letter modes, verbatim, `\left`/`\right` isolation, math environments,
   definition bodies, short verbs, macrocode chunks, `^^A` doc comments, expl3
   regions, char-constant isolation, signatures—as lexer modes or grammar routing, all
   reading static facts only (no macro meaning). The catalog, with examples and issue
   references, is in `docs/src/development/architecture.md`
   (§ *Sanctioned lexer modes*). The related expl3 *code formatting* is a
   formatter concern (§ *expl3 code formatting*).

   - **expl3 toggles: shared name set, formatter-only positional gate.** The lexer and
     the formatter read the *same* fixed toggle *name* set (`parser::lexer::expl_toggle`)
     so a new spelling is recognized in both and they never drift. But only the
     *formatter* additionally requires a toggle to be a **top-level statement**
     (`toggle_is_top_level`: not a `\def`/`\let` definee, not stored inside a
     group/definition body) before it opens a layout-owned region. Rationale (issue #69,
     `l3kernel/expl3.sty`): a toggle spelling TeX never executes is a false positive of
     the static model, and the byte-level losslessness/idempotency oracles cannot catch
     the resulting damage. The lexer keeps the naive name-only model on purpose—mis-lexing
     a name in letter mode only splits CST tokens (lossless, cosmetic), whereas mis-owning
     layout rewrites real space tokens (meaning). So only the higher-stakes side gates.
     Detail in `docs/src/development/architecture.md`
     (§ *expl3 code formatting*).

   - **Environment pairing is shape-gated on brace structure, not a command set.**
     An environment can never outlive the brace group its `\begin` opened in: braces
     are catcode structure, `\begin`/`\end` are only macros, so a `}` closing a group
     opened before the `\begin` always wins. A `\begin` whose `\end` is unreachable
     before that `}` — and, mirrored, an `\end` reached *inside* a group — is a plain
     command with **no diagnostic**, like a gated `$`/`\[` (issue #71). This
     generalizes the curated definition-body set (#45/#55) to the package code it
     cannot name. Two scope limits keep it precise: only a *group* boundary
     suppresses the environment (a `\begin` that just runs out of file still
     diagnoses), and `.dtx` doc-margin lines are exempt from *stranded* braces so
     the doc layer keeps pairing across `macrocode` chunks that leave braces open
     on purpose — an exemption that lifts when the enclosing `{` opened on a
     doc-margin line itself, since that group is the documentation layer's own and
     locally visible (`theorem.dtx`'s `% \def\deflist#1{\begin{list}…}` split
     definition). A `\begin` the gate demotes leaves an orphan `\end` the gate
     made, so that closer is demoted in step rather than unwinding every enclosing
     environment on its way to the root (`amsldoc.tex`'s
     `\lowercase{…\begin{error}{…}}`); the mirror is scoped to demoted names, so a
     genuine typo still reports. Detail in
     `docs/src/development/architecture.md` (§ *Sanctioned lexer modes*).

   - **Environment aliases are inferred from the file's own definitions, never
     configured.** A command whose replacement body is exactly `\begin{X}` (or
     `\end{X}`) stands in for that delimiter, so `\bea … \eea` pairs as an
     `ENVIRONMENT` of `X` (issue #109). Discovery rides the *existing* second
     parse pass (`parser::core::parse_ctx`), so the tree stays a pure function of
     that file's text — no config, no directive, no cross-file input, and an alias
     defined in a sibling `.sty` deliberately does not pair. Admission is narrow
     because a wrong pairing rewrites layout: the target must be a **curated
     built-in** environment (an alias declares a *spelling*, never a *semantic*),
     **non-verbatim** (`\newcommand{\bv}{\begin{verbatim}}` does not work in TeX),
     and **argument-free** on both sides (attaching from the target's signature
     would be arity-directed grouping from scanned data, decision #8); **both
     halves** must be defined in the file. Two things carry the risk: the opener
     index must exclude every *name being bound*, as a slot countdown rather than
     a one-word test (`\def\bea{…}` leaves the definee at brace depth 0 with
     `in_def_body` unset, so unfiltered the two definition lines pair with each
     other; `\let\oldbea\bea` binds *two* names, and left live the source operand
     pairs with the next stray closer and swallows the prose between), and the
     gate is **positive** — modelled on `conditional_closer`, not on the `\begin`
     gate — with no paragraph anchor, since an `itemize` alias body legitimately
     spans blank lines. Behavior then resolves **from the node, never the name**
     (`Signatures::environment_at` vs `::environment`), so a literal `\begin{bea}`
     beside an alias `\bea` stays an unrelated environment. Detail in
     `docs/src/development/architecture.md` (§ *Environment aliases*).

   - **Conditionals pair behind a shape gate, and the gate must mirror the walk.**
     `\if…\else…\or…\fi` becomes a `CONDITIONAL` of `CONDITIONAL_BRANCH`es (the
     `\fi` last, positionally) only when the closer is reachable; otherwise the
     opener is a plain command with **no diagnostic**, since a `\fi` is routinely
     assembled elsewhere (`\def\stopit{\fi}`, `\expandafter\fi`, `\iffalse…\fi`
     comment tricks leave 268 of 6205 corpus files unbalanced). Recognition is
     pair-and-trust over the `if`-prefix minus two curated families —
     `NOT_FI_TERMINATED` and the `\newif`/`\let` operand slots
     (`parser::conditional`, the recognizer *and* its state machine shared with the
     linter's `ConditionalIndex`, so the two can never disagree about what an
     opener is; each still layers its own filter on top — the parser suppresses
     expl3 regions, the linter withholds `\def` bodies). Subtracting the
     brace-argument family is load-bearing, not cosmetic: shape alone *mis-pairs*
     rather than fails, stealing an enclosing `\fi`
     (`test-cases/ifelsefi/issue-250.tex`).

     The load-bearing constraint is that **the token scan must never promise a
     closer past the one the recursive walk reaches**. Counting a `\fi` the walk
     consumes inside some other construct promises a pairing it cannot honor, and
     the walk then runs on looking for a closer that is gone — `ltboxes.dtx` puts
     three `\fi`s inside a `$…$` and ran the construct over 160 lines and every
     `macrocode` chunk between, stranding the cursor past `macrocode_end` for every
     chunk-bounded scan downstream. So the closer counts only at the opener's own
     *brace, environment, and math* level, a `macrocode` frame is a hard boundary
     in both directions, and the walk is bounded by the located closer index. The
     guarantee is one-directional by design: the walk may still close *earlier*
     (a nested opener the scan counted by name may be demoted when the walk
     re-gates it, and its `\fi` is then reached first), which is why
     `ast::Conditional::closer` is fallible and nothing downstream may assume the
     two indices agree. The `\if` *test*'s extent stays unresolvable by design
     (`\ifnum\radius>5` scans ⟨number⟩⟨rel⟩⟨number⟩ by TeX's own scanner), so there
     is no head node and no body indent. Detail in
     `docs/src/development/architecture.md` (§ *Sanctioned lexer modes*).

2. **Two layers: syntactic vs. semantic.** The syntactic CST knows nothing about what
   a command means; the semantic layer is a signature database assigning arity,
   verbatim-ness, and sectioning. **Meaning never leaks into the parser.** See
   `docs/src/development/architecture.md`.

   - **`ContentKind` is where a *whitespace-safety* claim lives**
     (`Opaque`/`Prose`/`TokenList`/`Keyval` on `ArgSpec`). `Keyval` is the
     strongest: it asserts a keyval-family processor strips spaces around entries,
     which is what lets the formatter break a `[…]` at a comma the author *glued*
     (`docs/src/development/architecture.md` § *Optional arguments, tables, and math spacing*). Compiling both spellings shows
     the claim is real for `\usepackage`/`\includegraphics`/tikz/`lstlisting` and
     false for every *textual* optional (`\item`, `\caption`, `\cite`, a
     `\newcommand` default), so a wrong flag changes typeset output — hold it to
     the curated standard of the math-env routing. Sourced from CWL's mechanical
     per-argument `%keyvals` placeholder mark plus hand-curated entries; never
     from scanned user definitions.

3. **Hand-written recursive descent is the spine; Pratt is local to math**
   (sub/superscript binding and `\left…\right` only). Math operator atoms and the
   `$`/`\[`/`\(` shape gates are bounded, sanctioned widenings of this rule, still
   producing no expression tree. See `docs/src/development/architecture.md`.

4. **The parser emits an event stream, not a tree directly**
   (`Start`/`Tok(idx)`/`Finish`); diagnostics ride a byte-range side channel (no
   `Error` event), and a `SubTok` event attaches `WORD` sub-slices for the math split.
   See `docs/src/development/architecture.md`.

5. **Errors travel alongside the tree, never abort it.** A single syntactic error
   never fails the whole parse. Recovery anchors: `\end{…}`, `\begin`, blank line,
   `}`, `$`, `&`, `\\`. Always make progress; never infinite-loop. See `docs/src/development/architecture.md`.

6. **Incrementality is salsa-first.** Cross-file/cross-query incrementality via salsa
   is the v1 story; intra-file reparse is a later optimization. See `docs/src/development/architecture.md`.

7. **Store green nodes in salsa, never red (`SyntaxNode`).** Red trees aren't
   `Send`/`Eq`/`salsa::Update`; the tree is a pure function of the text, materialized
   to red cursors on demand. See `docs/src/development/architecture.md`.

8. **Argument grouping is text-pure: greedy and generic by default, deviating only on
   static lexical facts.** Greedy attachment (texlab-style) is the only total strategy
   where the text carries no arity protocol; arity is refined by the semantic layer,
   never consulted during attachment—grouping from mutable signature data (config,
   package scopes, scanned definitions beyond the two-pass *self-definition* scan)
   would make the tree a function of inputs other than the text (decision #7). That
   scan covers user verbatim commands and environment aliases; both read only the
   file's own definitions, so the tree stays a pure function of that file's text. Sanctioned
   deviations read static facts only: **`[…]` attachment is shape-gated**—a bracket is
   an argument only when it reads as one, from static shape facts, never meaning—and
   the **expl3 argspec suffix** (the one dialect whose arity rides in the token
   itself, so arity-directed attachment would be as text-pure as greed) is the
   recorded *candidate* deviation, deliberately unimplemented: today the semantic
   layer derives the arity (`semantic::expl3`) and the formatter consumes it, and
   promoting attachment into the grammar is a migration with recorded open questions
   (mixed-shape CST, false-positive blast radius, differential-oracle divergence),
   not a patch. See `docs/src/development/architecture.md`
   (§ *Argument grouping and bracket policy*).

9. **Trivia attachment follows the rust-analyzer rule:** comments bind *forward* (a
   run of own-line `%` before a `COMMAND`/`ENVIRONMENT` becomes a `DOC_COMMENT`),
   whitespace floats, a blank line breaks the bind. See `docs/src/development/architecture.md`.

10. **Typed AST wrappers are a read-only view, never a re-model of the tree.** They
    expose structure, never meaning; accessors are positional and tolerate
    over-attachment. The formatter stays raw for structural work, adopting wrappers
    only for field access. See `docs/src/development/architecture.md`.

The **formatter engine** (Wadler-style `Doc` IR, `WrapMode`, `MathWrap`, table
alignment, expl3 layout), the **linter** (Rule trait, autofix model,
registration), and the **LSP's** sanctioned environment awareness are all
covered in `docs/src/development/architecture.md`.

## Invariants (test oracles—enforce them)

- **Losslessness:** `reconstruct(text) == text`, byte-for-byte.
- **Idempotence:** `fmt(fmt(x)) == fmt(x)`.
- **Whitespace-only formatter:** the formatter changes only *trivia* (whitespace,
  newlines, comments, `.dtx` margins/guards); it never inserts, deletes, or rewrites a
  non-trivia token. Content normalizations (stripping redundant single-token script
  braces `x^{2}` → `x^2`, `$$…$$` → `\[…\]`) are *linter autofixes*, never layout. Pinned
  by the non-trivia-content oracle in `assert_format_invariants` (`tests/format.rs`).
- **Protected regions** (`verbatim`, `lstlisting`, `\verb`, comments) are never altered
  by the formatter—with one carve-out: **line terminators are normalized
  document-wide**, protected regions included (`FormatStyle::line_ending`; `Auto`, the
  default, keeps whatever the source used). A protected body is emitted from source
  token text, so without the carve-out a CRLF document came out CRLF *inside* verbatim
  and LF everywhere else. Only the `\r\n`/`\n` pair converts; every other byte of the
  region is still untouched. Detail in `docs/src/development/architecture.md` (§ *Line endings*).
- **Reflow safety is structural, never config-derived.** Every file kind defaults to
  `WrapMode::Reflow`; there is no per-extension default. Whether content may be
  relaid is decided by the content, in *every* wrap mode — the `contains_doc_margin`
  gates on the relayout arms, and the `.dtx` margin-escape detector as the residual
  backstop (`LineBuilder::margin_escaped` → `lower_dtx_doc_paragraph` falls back to
  the preserve path when a probe-gated reflow would commit content outside the `% `
  margin; margin-riding blocks, isolable guard lines, and macrocode chunks behind
  their byte-exact frame leads stay *inside* the reflow, for paragraphs and
  doc-margined out-of-region expl3 runs alike — `dtx_run_reflows_safely`). So a
  user asking for `--wrap reflow` on a `.dtx` cannot corrupt it. Never re-introduce a
  file-kind wrap default to paper over a layout bug; fix the gate. Detail in
  `docs/src/development/architecture.md`
  (§ *Reflow is safe by construction*).
- **Trivia-invariant layout**: layout is a function of
  non-trivia content, config, and only those trivia predicates the formatter itself
  *preserves*. A predicate `P` is preserved when `P(fmt(x)) == P(x)`. Blank-line
  presence, comment presence and own-line-ness, and a column-0 `.dtx` margin/guard are
  preserved, so layout may read them. **Whether a gap is a lone newline or a space is
  not** — the formatter converts freely in both directions (`alpha\nbeta` → `alpha beta`)
  — so layout must never key on it. **No Tier-1 read remains**: under `Reflow`,
  opaque brace groups (`lower_opaque_group`) and optionals (`lower_optional`) are
  width-driven, and the surviving `spans_multiple_lines` readers are Tier-2
  residues behind the non-`Reflow` modes and the doc-margined corner. The `Gap`
  boundary normalization that would *enforce* this is still open (TODO.md).
  Detail, the predicate classification, and the
  widened-gap escape hatch for modes *defined* by authored breaks in
  `docs/src/development/architecture.md` (§ *Trivia-invariant layout*); the Tier 1
  vs. Tier 2 vocabulary is in `.claude/rules/formatter.md`.

  This subsumes idempotence: `fmt(x)` is by construction a trivia-perturbation of `x`
  (the whitespace-only invariant), so a layout invariant under trivia perturbation is
  idempotent *by proof*, not by corpus luck. Every bug in the K&R↔Allman family (issues
  \#71, \#94, \#96, \#97) is one decision keyed on the unsafe predicate.

  expl3 statement boundaries are **structural**: a call unit is the head plus the
  arguments its argspec arity consumes (`semantic::expl3::expl3_slots`, decision #2's
  "semantic layer assigns arity"; segmentation in
  `semantic::expl3::segment_expl_statements`), so the formatter owns
  one-call-per-line and a width wrap re-derives the same unit on every pass. The old
  newline rule survives only as the per-statement **fallback** for underivable heads
  (no `:` suffix, `w`/`D`/unknown letters, shape mismatches, guards mid-unit) and for
  a unit's same-line trailing junk — Tier 2, with its fixed-point argument written in
  `semantic::expl3` (greedy self-refilling lines, no wrap before a recognized
  head, junk-glued statements all-hard).

There is deliberately **no parse-stability invariant**: the formatter may still change
CST *shape* (the math operator split re-groups a catcode-12 `WORD`, so `a+2` → `a + 2`
re-lexes into separate atoms), but the whitespace-only invariant above pins the
non-trivia *content* it carries. The formatter is intentionally used to stress the
parser—any formatter ambiguity should surface a parser modeling gap. Trivia-invariant
layout is **not** parse stability and does not reintroduce it: the math split is driven
by non-trivia *content*, reads no trivia, and stays sanctioned.

**Differential oracle:** use **texlab's parser** as a differential *parse* oracle over
a corpus—skeletonize both trees and compare. It is a reference we measure against,
never match.

## Repo conventions

- Edition 2024; the toolchain is pinned by `rust-toolchain.toml` (single source of
  truth), consumed by `devenv.nix` and honored by CI. A `wasm32-unknown-unknown`
  target is configured.
- **Run `cargo fmt` before committing**—the rustfmt git hook rewrites unformatted
  files and aborts the commit otherwise. `clippy` warnings are errors:
  `cargo clippy --all-targets --all-features -- -D warnings`.
- **Typeset stability is not a CST property.** The CST oracles cannot see the one
  risk `ContentKind::Keyval` takes — a space token is trivia to the CST and content
  to TeX — so `task typeset:check` compiles `tests/typeset/*.tex` before and after
  formatting and diffs the typeset output. It needs a TeX install and never runs in
  CI; run it when touching keyval signature data or the optional-argument lowering.
- Task runner is `go-task` (`Taskfile.yml`). Performance is first-class (`perf`,
  `cargo-flamegraph`, `hyperfine`, `cargo-show-asm`, `cargo-llvm-cov` are in the dev
  shell)—benchmark before optimizing, never regress losslessness for speed.
- New parser features need corpus + snapshot tests **and** a losslessness assertion.
- **`CHANGELOG.md` is autogenerated by
  [versionary](https://github.com/jolars/versionary)** from the conventional-commit
  history—never hand-edit it. Write good conventional commit messages instead.
  Each workspace crate is its own versionary package with its own changelog and
  version: the root CLI tags bare `v*`, the members tag `badness-parser-v*` and
  `badness-formatter-v*`. Only the bare `v*` stream may carry GitHub release
  assets (binaries, npm/PyPI/AUR feed off it); the member streams publish to
  crates.io only, via `cargo workspaces publish --skip-published`
  (`publish-crates.yml`).
- **Windows CI bites twice:**
  - *Line endings.* The formatter emits **LF** and tests compare bytes against
    checked-in fixtures. When you add a fixture in a new extension under
    `crates/*/tests/fixtures/**` or `crates/badness-parser/tests/corpus/**`, add
    a matching `… eol=lf` line to
    `.gitattributes` (the `*_crlf_*`/`*_lf_*` line-ending fixtures are the deliberate
    `-text` exceptions). Never normalize line endings in code to pass a test—fix the
    attribute.
  - *URIs.* Decode LSP URIs to filesystem paths only through
    `uri_to_fs_path`/`path_to_uri` (`lsp.rs`), which strips the `/` before a Windows
    drive letter, keeps the Unix root, and spells separators natively (a URI-spelled
    path is already a usable `Path` key, but the spelling leaks wherever a decoded
    path is rendered back to text — forward search's `%f` beside an on-disk `%p`).
    Keep `uri_to_fs_path_handles_unix_and_windows` green; tests and snapshots must
    not assume `/` vs `\`.
- **Generated `data/` artifacts** (in `crates/badness-parser/data/`). Several data files are generated from pinned
  upstream sources by `scripts/gen_*.py` and guarded by paired `task …:check`/`:sync`
  targets: `cwl_signatures.json` (`cwl:check`/`:sync`),
  `package_names.txt`+`class_names.txt`+`package_metadata.json` (`pkg-names:check`/`:sync`),
  and `bib_fields.json` (`bib-fields:check`/`:sync`, tracking biblatex's `blx-dm.def`).
  `signatures.json`, `colors.json`, and `tikz_libraries.json` are curated by hand.
  Re-sync generated files via their model/task; don't hand-edit the mechanical facts.
  The CWL tier carries *names, arity, and the `%keyvals` argument mark* only —
  every behavior-classification suffix (`#V`, `#\math`, `#L0`-`#L5`, …) is
  reported for human promotion, never applied. Note `signatures.json` **masks** the
  CWL entry for a name wholesale (`.or_else`, not a field merge), so a curated
  command that CWL marks keyval needs the flag added by hand too.

## Working agreements for agents

- Keep the syntactic layer free of semantic knowledge.
- Read/navigate the CST through the typed AST wrappers (decision #10): typed accessors
  and `child`/`children`/`child_token` over raw `children().find(|c| c.kind()==X)`. Add
  a wrapper struct when a node kind gains a field-extraction consumer; keep accessors
  positional and meaning-free.
- **Never key layout on a lone source newline** (trivia-invariant layout, above). A
  width wrap and an authored newline are the same bytes to the next parse, so any rule
  that reads one is a latent idempotency bug. Blank lines and comments are fair game.
  A rule that genuinely needs the unsafe predicate (`WrapMode::Stable`, `Sentence`,
  `Semantic`, `ReflowKind::Statement`, the expl3 fallback statement, the
  command-only-line residue, the delimited-group block residue on
  `spans_multiple_lines`) is Tier 2: it must carry a written fixed-point argument
  showing every layout it can emit re-reads to itself, as `ReflowKind::Statement`'s
  flush continuation, the expl3 fallback's greedy self-refilling lines, the
  command-only residue's preservation-only hardening (`line_is_command_only`), and
  the delimited-group residue's block-re-reads-multi-line argument
  (`spans_multiple_lines`) do. A fallback line's fill also *hugs*
  (`Ir::HugFill`): an atom that carries a forced break is measured by its first
  line, so where it lands never depends on forced-ness — which is why no arm of
  the forced-break dispatch fires inside a fallback statement. A Tier-2 rule must
  also never mint a forced break an upstream `contains_forced_break` reader can
  see on one pass but not the other — that is why the command-only residue is
  off inside a prose argument body (`ReflowKind::ProseArg`).
- Don't add intra-file incremental reparse, macro expansion, or catcode logic beyond
  decision #1 without recording the decision here and in the relevant
  `.claude/rules/` file.
- New salsa **inputs** carrying rarely-changing data (config, package/class metadata)
  must be constructed at `Durability::HIGH`/`MEDIUM`; per-file `text` stays `LOW`
  (salsa's default). Otherwise every keystroke's `LOW`-revision bump invalidates them.
  Detail in `docs/src/development/architecture.md` (§ *Incrementality*).
- Update TODO.md as phases progress; update this file when a decision changes, and keep
  the matching `.claude/rules/` file in sync.
