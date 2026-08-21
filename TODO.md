# Badness TODO

A LaTeX formatter, linter, and language server on a lossless rowan CST,
following **rust-analyzer's** architecture. See `AGENTS.md` for load-bearing
design decisions and invariants.

Single-crate package (not a workspace). Parser and formatter are **intentionally interleaved**: the formatter is the primary tool for stress-testing the parser.

Status: `[ ]` todo · `[~]` in progress · `[x]` done

## Parser

- [ ] **Keep carving `grammar.rs`** (3,959 lines after the first cut, which
  took `grammar/facts.rs` and `grammar/trivia.rs`; `grammar/prescan.rs` came
  out with it). Two candidates remain, each its own commit:

  - The **math / `\left…\right` sublanguage** (`dollar_math` through
    `stray_right`, plus `split_math_word`), ~460 lines and highly
    self-contained. `math_environment_body` currently sits in the environment
    section and is the one routine the split has to decide about.

  - The **gate machinery** (`WalkKey`, `GateBatch`, `VerdictSink`, the policy
    vocabulary, `trait GatePolicy`, and the nine gate policies), ~805 lines.
    Postdates the original audit note. It drags the `scan_work` linearity
    tests along, so `AGENTS.md` ("pinned linear by the tests in
    `grammar.rs`") needs a matching one-word update.

  The rest of the hygiene item is done: the shadow counters, the DOC_COMMENT
  precede dedup (`precede`/`extend_back`/`doc_comment_bind`), the `PreScan`
  extraction, the `math_atom` EOF tripwire, the environment-delimiter helpers,
  `BLANK_LINE_NEWLINES`, the `is_trivia` reuse, the borrowing `peek_end_name`,
  and the stale `parser.rs` module doc. Still open from that note: promoting
  `precede` into the event layer as a real rust-analyzer `Marker` with a
  `DropBomb`, which is a mechanical diff across every `open`/`close` site.

- [ ] **Comment consolidation (consolidate, never purge).** Comment density
  in the parser crate is 30–39% per file and overwhelmingly the house-style
  constraint-and-provenance kind — keep that. The cuttable part is
  *restatement*: the lexer states the short-verb semantics in four places
  and the macrocode-frame rules twice; call sites restate 25-line helper
  docs. Cut each fact to one canonical location (the helper's doc) with
  one-line call-site pointers — roughly a third of the comment mass, zero
  information loss. The per-gate re-explanations of the shared scan skeleton
  die with the closer-map work. `catcode_signal` (under *Semantic layer &
  signatures*) is the cautionary tale for why this matters: the real hazard
  at this density is a comment asserting something the code stopped doing.

## Formatter

- [ ] **Long collapsed cite list overflow.** A `collapse` arg folds to one line
  even when the key list exceeds the width; it never breaks *at commas* (one
  key per line) as a fallback. Needs the token-list content kind to break on
  its own separators rather than the paragraph fill.

- [ ] **Widen mandatory-keyval admission (follow-up to the `{…}` segmentation).**
  `ContentKind::Keyval` on a *mandatory* group is now consumed
  (`lower_segmented_group`; `keyval_group_splits_entries`), so the setters
  `\pgfkeys`/`\tikzset`/`\lstset`/… take one entry per line instead of a prose
  reflow that wrapped mid-key. Two halves were deliberately left out and neither
  is a bug:

  - The bulk CWL tier still drops a `%keyvals` mark on a `{…}`
    (`gen_cwl_signatures.py`, `_parse_arg_shape`). The reason it gave — "nothing
    consumes the flag there" — has expired, but the other half has not: the mark
    is mechanical, and a wrong `Keyval` on a mandatory group changes typeset
    output where the same mistake on a bracket is contained. Lifting the scoping
    means first *measuring* which names would gain it (needs the pinned CWL
    source) and putting the textual ones through `task typeset:check`.
  - Environments are unwired: `lower_begin` keeps `keyval && is_bracket`. The
    corpus case is tabularray's `\begin{tblr}{hlines={white},…}` (latexindent's
    `keyEqualsValueBraces/issue-378`), and it pulls in two things a command does
    not have — the grid router reads the colspec group, and a verbatim-body
    environment's `\begin` line may never break at all.

- [ ] **Formatter-owned trailing comma (parked; the last piece of issue #47).**
  A `[…]` — and, since the segmentation above, a proven-keyval `{…}` — is a
  width-driven group over its top-level entries, and a
  `ContentKind::Keyval` argument may also break at a glued comma
  (`docs/src/development/architecture.md` § *Optional arguments, tables, and math spacing*). What is left of the old parked
  item is the Black-style trailing comma: for a proven-keyval argument, add the
  `,` when expanded and drop it when collapsed — safe as *TeX*, because
  keyval/xkeyval/pgfkeys/l3keys and `\ProcessOptions` clists all ignore empty
  entries. **Blocked on a tenet, not on data:** inserting or deleting a `,` is a
  non-trivia token edit, which the whitespace-only invariant forbids and
  `assert_format_invariants` actively catches. Landing it means amending that
  invariant and its oracle to carve out this one insertion — a decision worth
  taking on its own, not as a ride-along. The count-based *expansion* half was
  declined: width alone is already canonical, and an N-key threshold would need
  the comma count to proxy for keyval-ness, exploding comma-rich textual
  optionals. The Black/Ruff *magic trailing comma* (a trailing `,` in the
  **source** forcing one-key-per-line) stays declined too — content steering
  layout conflicts with the formatter-is-sole-authority tenet.

- [ ] Widen the prose-argument table (CWL ingest could feed it); consider gluing
  a prose arg onto its command line when a source break separates them. (The
  block half of the signature widening landed as `CommandSig::block`; the
  gluing clause is now a proposal to narrow the command-only-line rule's
  *residue*, see the Tier-2 entry above.)

- [ ] **Key-value continuation indent in an expl3 fallback statement (open scope
  call).** A key whose value continues on the next line should indent the value
  one step, which is what an author writes and what upstream overwhelmingly does
  (a sweep of latex3's `.dtx` sources: of 87 code lines ending in `=` with the
  value on the following line, **65 indent it by +2**, the rest split between 0
  and incidental alignment):

  ```tex
  ,begin-vspace:e =
    \tl_if_empty:nTF {#2}
      { \newtheoremstyle@vspace@default }
      {#2}
  ```

  Badness neither produces nor preserves this: it emits the continuation *level
  with the key*, discarding an authored `+2`. The cause is structural, not a
  layout bug. `,begin-vspace:e = ` is a **fallback** statement (no derivable
  arity), and in a fallback stream a newline is a statement *boundary* — the
  Tier-2 residue. So `\tl_if_empty:nTF …` on the next line is a *sibling
  statement* at the same base indent, not a continuation of the entry; and
  indentation is always computed, never read, so the author's step is dropped.

  Emitting +2 needs the formatter to know that `,key = ` is an *incomplete*
  entry — key-value modelling inside a stream it explicitly declines to model.
  The narrowest form is a rule like "a fallback statement whose last non-trivia
  token is `=` hangs its successor +2", which reads only non-trivia token
  content and is therefore trivia-invariant and permissible. **The open call is
  whether to have it at all**: the l3styleguide is silent on key-value dialects,
  so this is badness inventing layout for a dialect it cannot name — the same
  objection recorded against the 2e-brace-tightening entry below. Upstream's
  65/87 gives the rule an empirical basis; the tenet-#1 pressure is that a
  `Keyval` content kind (see the parked keyval entry above) would be the
  principled carrier, not a token-shape heuristic in `lower_expl_code`.

  Surfaced while fixing the sibling-coupling and all-or-nothing conditional bugs
  (issue #101); the conditional fix removed the *worse* half of this shape (the
  branch list no longer splits across two indents), leaving only the
  continuation indent itself.

- [ ] **Fold the linter's pgf picture set into the `statementBody` flag.**
  `linter::rules::is_pgf_picture_environment` (which keeps `dash-length` off
  coordinate arithmetic) and `data/signatures.json`'s `statementBody` now curate
  the same family twice — and the flag now has *three* readers (formatter
  routing, the parser's statement mode, and the linter's duplicate set), which
  strengthens the case for the fold. Merging them needs a `SignatureDb` on `RuleContext`,
  which carries none today — the same plumbing the user-declared ref/cite
  families entry below wants, and which the Declarations plan (Semantic layer
  § *Declarations*) has to build anyway. Until then the two carry cross-references and must
  be edited in step. Note the sets are not quite identical by design: the flag
  also names `scope` and `pgfonlayer`, which the linter reaches through the
  enclosing `tikzpicture` on its ancestor walk.

## Linter

### Issues

- [ ] **Config knob for user-declared ref/cite command families (grew out of
  issue #104).** The #104 example still draws `unreferenced-label` on every
  label referenced only through a custom wrapper (`\eqrefs{thm:eq1,thm:eq4}`
  expanding to `\eqref` calls): the semantic builder's ref-family name set
  (`semantic::builder::ref_command`) is fixed, and seeing through the wrapper
  would take macro expansion (out of scope, decision #1). A `badness.toml`
  knob declaring extra ref-family (and cite-family) command names — the
  analog of the parked user-declared-verbatim-envs knob above — would let a
  project name its wrappers; the declared names feed the builder's name sets
  (semantic layer only, never argument attachment, so decision #8's
  text-purity is untouched). Needs plumbing: config does not currently flow
  into `SemanticModel::build`, and the shared name sets also serve completion
  and the LSP, which should honor the same declarations. **Land it as
  `[commands.eqrefs] like = "eqref"`** on the Declarations mechanism (Semantic
  layer § *Declarations*) — the deferred `[commands.*]` half of it — rather than
  as a bespoke list-of-names knob.

- [x] **`codeexample` unknown to the signature DB.** pgfmanual's `codeexample`
  env holds verbatim-like example source that is *also* executed. Because it was
  not in `data/signatures.json` (which lists `verbatim`, `lstlisting`, `minted`,
  `Sinput`, …), the prose rules fired inside it: on the pgf corpus this drove
  ~1900 `straight-quotes`, ~370 `ellipsis`, and ~100 `dash-length` findings, and
  — worse — the *default* (`Safe`) `ellipsis` fix rewrote `...`→`\dots` inside
  executed code (`\immediate\write\w{...}` → `{\dots}`). Resolved by curating
  `codeexample` into the built-in DB as a `verbatimBody` env, following the
  precedent of the equally package-specific Sweave (`Sinput`/`Soutput`/`Scode`)
  and `Code`/`CodeInput`/`CodeOutput` entries; its body now lexes to one opaque
  `VERBATIM_BODY` token, so the prose rules never see it.

  - [ ] *Follow-up (open):* a project-config knob for user-declared verbatim envs
    would generalize this to package-specific envs badness cannot name. Config
    does not currently flow into the signature DB or the lexer's `VerbCtx`, so
    this is a separable feature, not a data edit. **Subsumed by** the Declarations
    plan (Semantic layer § *Declarations*): `[environments.x] like = "lstlisting"`
    is exactly this knob, and falls out of steps 3–5 rather than needing its own.

  - [ ] *Out of scope (catcode limitation):* the sibling `|…|` active-char
    shortverb (`\catcode`\|=13` + `\gdef|{…\verb|…}`) drives the same class of
    FP (`straight-quotes`, `unclosed-math-delimiter`, `sectioning-level-jump` on
    `|\part|`, `missing-nonbreaking-space` on `\ref` inside `|…|`) but is a
    genuine catcode limitation, not statically resolvable.

The remaining linter findings from the cam-notes sweep are recorded below as open
follow-ups (each with a minimal reproducer); none is fixed yet.

- [x] **`dash-length` corrupts pgf/TikZ coordinate arithmetic under
  `--unsafe-fixes`.** The `in_math` guard covered only `$…$`, not a pgfplots
  expression in `{…}`: `printf '\\addplot3 {(y^2-1)^2};\n' | badness lint --fix
  --unsafe-fixes` yielded `{(y^2--1)^2}`, a meaning-bearing minus turned into an
  en-dash. Resolved by two shared pgf gates: `in_pgf_picture` (a `tikzpicture`/
  `pgfpicture`/pgfplots-`axis`-family ancestor, so coordinate arithmetic like
  `(2-1,3)` is skipped) and `in_pgfmath_argument` (the `\addplot`/`\pgfmath…`
  expression argument, attached or detached past the numeric `\addplot3` variant),
  where a `-` between numbers is a pgfmath subtraction, not a typeset range.

  - [ ] *Follow-up (open):* prose FPs on index-pair/term names (`0-1 law`,
    `1-2 plane`, `1-1 function` — 22 of 25 findings) where the hyphen is
    intentional; these are `Unsafe`-gated so `--fix` withholds them, so they are
    noise rather than corruption. Distinguishing `0-1 law` from `pages 5-10`
    statically is the open part.

- [ ] **`makeat-macro` residual on plain-`.tex` package internals.** Recognizing
  `*.code.tex` as package flavor fixed 98.9% of the pgf `makeat-macro` FPs, but
  generic-implementation files named plainly (`pgfutil-common.tex`,
  `support/pgf-regression-test.tex` — `\input` under `\makeatletter`, no
  `\makeatletter` of their own, no `.code.tex` signal) still emit ~590 findings.
  There is no clean static signal distinguishing these from a document that
  genuinely forgot `\makeatletter`, so this is a known limitation rather than a
  fixable gap; noted for completeness.

### Rules

- [ ] **Lint malformed `.dtx` `macrocode` closing frames.** The `doc` package
  terminates a code chunk by scanning for the literal physical line
  `%    \end{macrocode}` (or its starred form), with exactly four spaces after
  the column-one `%`; Badness currently parses near misses with a different
  space count as ordinary, clean `ENVIRONMENT` nodes. Add a `.dtx`-only
  `invalid-macrocode-frame` file check that reports such near matches as errors.
  The opener's four-space spelling is conventional rather than the critical
  delimiter, so do not report it at error severity. Start without an autofix,
  or restrict a `Safe` fix to a column-one `%` whose horizontal space is the
  only malformed part. A related, separate candidate is an indented `%<...>`
  marker: docstrip recognizes a guard only at column zero.

- [x] **Blank line inside a `ContentKind::Keyval` argument.** Shipped as
  `blank-line-in-keyval` (`Error`, node-shape, `Safe` fix). A blank line is a
  `\par` and a keyval processor walks its entries with non-`\long` macros, so
  the call aborts -- and TeX names the *processor*, not the command the author
  wrote (`\kv@processor@default`, `\pgfkeys@addpath`, `\enit@setlist@i`,
  `\caption@setup@options@`; `\geometry` fails differently again), which is
  what makes the finding worth more than the compiler's message.

  Three scope limits, each measured rather than assumed: a blank line *nested*
  in a value's brace group compiles clean and is not flagged; an unclosed `{` is
  left to the parse error it already draws; and a `Keyval` *optional* cannot
  reach the shape at all, since the parser's bracket gate refuses across a
  paragraph break. Swept over all four gate corpora (6205 files): **zero
  findings in latex2e, latex3, and pgf**, and 780 in latexindent's own
  `modifyLineBreaks` output fixtures -- true positives, confirmed by compiling
  the shape.

  Still open, and deliberately so: the rule reads only the hand-curated tier, so
  its reach grows exactly with the mandatory-keyval admission entry below. Any
  name that entry adds is a name this rule starts protecting.

- [ ] **Mine the ChkTeX warning catalog (~44 warnings) for missing rules.**
  LaTeX Workshop adds no lint rules of its own (it only shells out to
  ChkTeX/lacheck, both off by default), so ChkTeX's catalog is the source to
  compare against. Badness already covers the high-value territory (ellipsis,
  dash length, straight quotes, `$$`, space-before-`\footnote`, intersentence
  spacing); remaining candidates include space before punctuation or
  parentheses and missing italic correction (`\/`).

Follow-ups from `label-before-caption` (floats only, shipped). All three are
scope limits recorded at implementation time, not regressions.

- [ ] **Extend `label-before-caption` to list items.** `\label` before `\item`
  is the same `\@currentlabel` bug: in `\begin{enumerate}\label{i:a}\item
  A\end{enumerate}` the label captures the enclosing counter, so `\ref{i:a}`
  prints a number unrelated to the item. Left out of the initial rule because
  the shapes are more varied than a float's — a label may legitimately sit
  between two `\item`s and belong to the earlier one, and `description`/
  `enumitem` custom labels widen the surface — so the statement-level gate that
  makes the float case safe has to be re-derived before it can fire here.

- [ ] **`label-before-caption` is silent outside floats.** `\captionof` in a
  `minipage` fails the same way (`\begin{minipage}{\textwidth}\label{mp}
  \captionof{figure}{C}\end{minipage}`), but `minipage` is not an
  `OutlineKind::Float`, so the rule never looks. Widening the container set
  means deciding which environments may host a `\captionof` without inventing
  findings on ordinary layout environments; the float set is curated signature
  data precisely so this stays a data question.

- [ ] **`label-before-caption` misses the nested-subfigure case.** The detection
  cutoff is the first counter-stepping command at *any* depth, so a `subfigure`'s
  own `\caption` silences a later statement-level `\label` in the outer float —
  which really does capture the sub-counter. Deliberate: the liberal cutoff is
  what keeps `\subcaptionbox` and the `\caption{Text\label{x}}` idiom from
  producing false positives. Recovering the miss needs a per-scope stepper model
  that knows *which* counter each caption stepped, so it is a modeling change
  rather than a gate tweak.

## Semantic layer & signatures

- [ ] How much of `\newcommand`/`xparse` to model for the signature DB. *(open
  decision)*

- [ ] **Make `EnvironmentSig::reflow`/`block` computed, not stored.** Both
  are derivations of other fields, and mutation sites must hand-sync them —
  `define.rs` writes `sig.reflow = false` manually after setting
  `verbatim_body` at two sites, and a forgotten sync is silent. Computed
  methods remove the field, the hand-sync, and the derivation duplicated
  across the const fns and `From<RawEnvironment>`.

- [ ] **`is_cite_command` accepts any `\cite*`-prefixed name**
  (`semantic/builder.rs`): `\citebox` or `\citecolor` gets its argument
  recorded as citation keys — an open-ended false-positive surface, unlike
  the neighboring closed-table predicates, and nothing documents the choice.
  Either write down why open-prefix recall is intended or close the set.

- [ ] **Semantic-layer hygiene (audit follow-up).**

  - `ast::command_name` (and `ControlWord::name`, `nth_group_text`) return
    `SmolStr`/`&str` instead of `String` — called per command node in every
    tree walk and in expl3's segmentation hot loops; the cheapest real
    allocation win the audit found.

  - Split the completion word-list tiers (package/class names, colors, tikz
    libraries, CTAN metadata, `arg_enums`) out of `signature.rs` into their
    own module — they have nothing to do with signatures, and the file drops
    to ~1,100 lines.

  - Collapse `merge_from`/`merge_from_package` into one origin-parametrized
    helper; table-ize `builder::build`'s four identical key-family arms (the
    layer's only 100+-line function); extract expl3's `is_recognized_head`
    predicate (spelled three ways today); consider per-index flags for
    `StatementMap`'s four parallel `Vec<bool>` so illegal states are
    unrepresentable; hash-map `builder::resolve` (currently O(refs ×
    labels)); move `define.rs`'s private `is_trivia` mirror into `syntax`
    beside `is_collapsible_trivia`.

## Language server

### Feature status vs LaTeX Workshop

A second reference diff, against **LaTeX Workshop** (the dominant VS Code LaTeX
extension). It is not an LSP: its intellisense, hover, and outline are
regex-driven extension code, and its formatting and linting shell out
(latexindent/tex-fmt, ChkTeX/lacheck), so badness already leads on language
smarts. Coexistence is the deliberate story (docs `guide/editor-setup.md`):
LaTeX Workshop keeps build, PDF preview, and SyncTeX. The features it has that
badness lacks and wants are filed in the sections below, tagged *(LW)*:
citation filter-by-title, command argument placeholders, keyval `label={…}`
scanning, graphics hover preview, package-doc hover links, a texmf bib
fallback, and surround/promote-demote code actions. Math-preview-on-hover is
the one big item needing a design decision (see `### Hover` and Open
decisions). Not adopted: `@a`-style abbreviation snippets and two-letter
environment snippets (editor-snippet territory), graphics thumbnails inside
completion items (VS Code-only), and sub/superscript history completion
(niche).

### Configuration & sync

- [x] config over LSP—the LSP now discovers `badness.toml` per document
  (`GlobalState::resolve_settings`, cached by anchor dir, cleared on
  `didChangeConfiguration`). A discovered config wins outright
  (file-wins); editor settings are the fallback. Both `[format]` (`line-width`,
  `indent-width`, `wrap`) and `[lint]` (`select`/`ignore`, applied via
  `RuleSelection` in the analyze/diagnostic/code-action paths) are honored. Two
  follow-ups remain:

  - Deliberately *not* done: plumbing `wrap` (or other knobs) through
    `EditorSettings` itself. A discovered config's `wrap` flows via `FormatConfig`,
    so no new editor knob was needed; `EditorSettings` stays `line_width`/`indent_width`.

- [ ] `workspace/diagnostic` (the workspace-wide pull)—deferred: it is a
  streaming/long-poll protocol (held-open request, per-uri result ids, partial
  results) that fits the one-shot id-bound read-job model poorly. Advertise
  `workspace_diagnostics: true` and add it once that plumbing exists; editors
  drive interactive diagnostics through `textDocument/diagnostic` meanwhile.

### Completion

Badness offers command, environment, label, cite-key, bib field/type, and file
completion (`src/completion.rs`, `src/bib/completion.rs`). texlab's completion
breadth is its biggest lead (`crates/completion/providers/`); the specialized
sources below are missing.

- [ ] *(Design decision)* **Package-scoped command completion.** texlab suggests
  only commands provided by the loaded packages (a package→command component
  model). Badness's signature DB is flat (curated + CWL + scanned); scoping
  completion to `\usepackage`-loaded packages needs package→command attribution.
  Open question, not a mechanical add.

- [ ] **Command argument placeholder snippets *(LW)*, opt-in.** Environment
  completion already inserts snippet bodies with tab stops; commands could emit
  placeholders for required/optional arguments straight from the signature DB
  (`\frac{$1}{$2}`). Gate on the client's snippet capability and an editor
  setting—LaTeX Workshop's equivalent (`intellisense.argumentHint.enabled`) is
  off by default, since placeholder churn annoys as many users as it helps.

- [ ] **Labels from keyval options *(LW)*.** LaTeX Workshop scans `label={…}`
  inside environment option blocks (`lstlisting`, beamer frames) and
  configurable custom label commands (`\linelabel`). Check whether the label
  scanner catches the keyval form; if not, it is a bounded static pattern for
  the semantic layer, feeding completion, navigation, and the
  `undefined-ref`/`unreferenced-label`/`duplicate-label` rules alike.

### IntelliSense (signature DB)

### Hover

- [ ] **Graphics preview on hover *(LW)*.** Hovering an `\includegraphics`
  argument returns hover markdown embedding the image itself
  (`![](file:///…/fig.png)`)—VS Code renders images in hover markdown. No
  rendering on our side, just a file reference: reuse the target resolution
  from `lsp/document_link.rs`; png/jpg/svg only, degrading to the resolved
  path for `.pdf`/`.eps`.

- [x] **Documentation link in package hover *(LW)*.** LaTeX Workshop's
  `\usepackage` hover offers a "View documentation" link via `texdoc`. The
  package hover now pairs a texdoc documentation link
  (`https://texdoc.org/pkg/<name>`, keyed on the package name texdoc resolves,
  serving the documentation PDF) with the existing CTAN catalogue link (keyed
  on the `ctan` catalogue id).

- [ ] *(Design decision)* **Math preview on hover *(LW)*.** LaTeX Workshop's
  most-loved language feature: hovering math renders it (MathJax,
  client-side); texlab lacks it too, so it is also a differentiator. Options:
  (a) skip—LaTeX Workshop covers it, and coexistence is the story; (b) render
  in the VS Code extension—breaks the thin-client principle and is VS
  Code-only; (c) server-side SVG via a Rust math renderer (ReX or similar) as
  a data-URI image in hover markdown—editor-agnostic, but ships a math layout
  engine, which is typesetting in all but name. Lean (a) for now, but leave the
  choice open until implementation constraints justify closing it; record the
  eventual rationale in the architecture documentation rather than turning
  `AGENTS.md` into a decision log.

### Code actions

- [ ] **Surround selection with environment/command *(LW)*.** LaTeX Workshop
  ships these as client-side commands; badness can host them editor-agnostically
  as code actions or `executeCommand`s alongside `changeEnvironment`
  (`lsp/code_action.rs`).

- [ ] **Section promote/demote *(LW)*.** Recursively shift sectioning levels
  across a selection (`\section` ↔ `\subsection`); the sectioning hierarchy is
  already in the signature DB, so this is a mechanical rewrite.

## Performance & hardening

- [ ] **Gate the query log behind an observation window.** `record_query`
  (`src/incremental.rs:1388`) pushes an entry per executed tracked query for the
  life of the process, and nothing outside tests ever reads or clears the log —
  the long-running language server grows it without bound. Arity had the same
  leak (jolars/arity#116) and fixed it by making `clear_query_log` the "start
  observing" switch: recording is off until the first call, and every test
  already clears before asserting, so no test changes. Port that commit
  (`fix(lsp): record query log only during observation`, arity `9959129`)
  essentially verbatim.

- [ ] **Stop leaking interned `Project` snapshots.** `Project`
  (`src/project/graph.rs:451`) is a `#[salsa::interned]` struct, and salsa never
  collects an interned value created above `Durability::LOW` — every distinct
  membership interns an immortal snapshot plus the project-level memos keyed on
  its id (`project_graph`, `package_graph`, `resolved_labels`,
  `resolved_citations` with the whole bibliography, `resolved_package_options`,
  and per-`(project, file)` `scope_signatures` `SignatureDb`s, the
  generations-times-members multiplier). The key is
  `members_of(snapshot) = snapshot.tracked_files()` (`src/lsp.rs:1070`), which
  grows on every first touch of a file — opening one, or a sibling seeded by
  `seed_disk_file` for cross-file resolution — and the intern sites are the hot
  read paths (hover, signature help, completion resolve, the diagnostics
  passes). Progressively navigating a large project therefore ratchets memory
  for the whole session. Arity's fix (jolars/arity#116, commit `8cf322f`) is the
  template: seed membership as a salsa input in the write phase, return the
  snapshot as plain `Eq` data from a keyless tracked query so it backdates
  instead of interning, and make the project-level queries keyless — each then
  keeps exactly one replaced memo. Measured there: about 1.3 MB leaked per
  membership/metadata change in a 180-file workspace, flat after the fix.

- [ ] **Borrowed token text, maybe.** Tokens are `SmolStr`
  (`crates/badness-parser/src/parser/lexer.rs:42`, same in `bib/lexer.rs:29`),
  so short tokens are already allocation-free and the fatou-sized win (-60%
  lexing from `&'src str` tokens) does not transfer wholesale. The payoff
  concentrates in the pathologically long tokens (`VERBATIM_BODY`, `VERB`),
  and the conversion is blocked on the half-dozen sites that push constant
  literals instead of input slices (`push_env_delimiter` at
  `lexer.rs:1646-1662`, the braced-verb path at `lexer.rs:1344-1356`,
  `lexer.rs:1457`) — each provably corresponds to contiguous input bytes, so
  they can be rewritten to slice. Measure with a lexer-only bench first
  (arity's workload-stratified `benches/lex.rs` is the template); do this
  last, if at all.

- [ ] **95 corpus files change their `%` comments** (`comment-change` in the gate
  baselines). Surfaced by the comment oracle added alongside the conditional node —
  the `content-change` check compares `nontrivia_content`, and a comment is trivia
  to the CST, so this whole class was invisible. All 95 predate the conditional
  work (verified file by file against `main`; that change fixes 12 of them and
  regresses none). Two shapes so far:

  - **Adjacent comments merge.** `%\n% just backwards compatibility…` comes out as
    `% % just backwards compatibility…`, the empty comment's `%` swallowed onto the
    next line (`pgfrcs.code.tex`, `latexrelease.sty`). Byte-identical meaning to
    TeX, but the formatter still rewrote a protected region.

  - **`.dtx` guards re-lex as comments.** A `%<+debug>` that no longer opens its
    line is a comment, not a docstrip guard, so the extracted file changes — a
    meaning change, not a cosmetic one. Every `.dtx` in the list is this shape.
    Likely the same margin/guard column-0 pinning the reflow already backstops.

- [ ] **`Ir::contains_forced_break` is a per-child subtree walk at lowering
  time**, so nesting depth is still superlinear — 64% of the run on `{{{x}}}`
  nested 4000 deep, the residue after the `contains_doc_margin` gate above.
  `saturate` (`ir.rs`) already computes the identical bit bottom-up in one O(n)
  pass, precisely so it is "computed on the way up, never by re-traversal", but
  it runs once at the printer seam while lowering asks the question repeatedly
  on partial sub-IR — which `core.rs` explicitly sanctions today. So this is a
  documented decision to revisit, not a bug to patch: the bit changes as the IR
  is rebuilt during lowering, so a memo has to be keyed on something that
  cannot go stale. `Ir::contains_group` has the same shape. Deep brace nesting
  is the only shape that reaches it (both bench documents are unaffected), so
  it is not urgent. `tests/scaling.rs` tolerates the residue at a 3.4x bound;
  tighten it to 3.0x when this lands.

- [ ] **Split `tests/parser.rs` by area.** It is now 2,923 lines and 230 tests;
  separate math, verbatim, comments, conditionals, and aliases into focused
  integration-test targets.

- [ ] **`build.rs` renders positional same-typed bool lists** in the
  generated constructor calls (`command(&[…], None, false, false, false)`;
  nine positional args for environments), so a swapped `verbatim`/`rule`
  compiles silently. Named-struct constructors, or `/*verbatim*/`-style
  inline comments in the rendered source, make the generated code
  self-checking.

- [ ] **Mine the `latexindent` corpus for construct coverage** (human-in-the-loop,
  ongoing). Skill: `.agents/skills/formatter-fixture/`. The corpus is read as a
  coverage map — which constructs occur and in what shapes — and **latexindent
  itself is the taste reference we check each construct against**: 711 of its
  test files are named for the upstream issue that produced them, across 127
  distinct issues, so its answers carry a decade of real user pushback.
  Never a byte-target: it is an indenter that preserves author breaks and
  never touches intra-line spacing, where we reflow and own layout. But every
  divergence gets a verdict (corroborates / explained / no opinion /
  unexplained), and an *unexplained* one blocks the fixture until it is worked
  out — that is where our rule is usually wrong. Run it at default settings
  (`latexindent probe.tex`, no `-s`) on a hand-authored probe; the committed
  `*-mod*.tex` files are one YAML stack's answer with `-m` on, not its own
  judgment. Measured gaps against the 265 existing
  slugs: `items` (157 files) and bare/named brace groups are no longer thin (12
  and 33 slugs); re-measure before trusting any gap list here. Beamer item
  overlays are covered by `list_item_overlay_prefix`. `mand-args` /
  `opt-and-mand-args` / `environments` yielded `begin_tail_is_body` — content the
  greedy parser attaches to `BEGIN` past the declared arity is body, not header —
  which closed a Tier-1 lone-newline read *and* a column-0 indentation bug, and
  surfaced the paragraph-reflow glued-split entry above; the rest of those three
  families is still open. `filecontents` is
  done (`filecontents_protected_body`) — it was purely a protected-region
  question, and the survey found no defect: the sharp edge it now pins is that a
  verbatim-body environment's `\begin` line must never break under width
  pressure, since it defines where the protected body starts and `filecontents`'s
  optional is `Keyval` (which elsewhere licenses a comma split).
  Sectioning/`headings` is done (two slugs, and the Tier-1 lone-newline
  bug that lived there). `ifelsefi` (402 files) is done too, via the
  `CONDITIONAL` node under *Parser* and eight fixtures — do not re-derive a
  formatter-only rule for it, the survey already showed every such rule is
  trivia-reading, typeset-unsafe, or lopsided.

## Editor integration

texlab bundles PDF-workflow features. Only position mapping (no typesetting by
badness) is admissible; the rest are explicit non-goals recorded here so they are
not re-proposed.

- [x] **Forward/inverse SyncTeX search (no typesetting).**
  `textDocument/forwardSearch` (a custom LSP method, texlab-wire-compatible)
  resolves the root document's PDF from `[build]` and launches a viewer
  configured through editor settings, with `%f`/`%p`/`%l` substituted
  (`lsp/forward_search.rs`). Inverse search receives a viewer position over IPC
  and answers with `window/showDocument` (`ipc.rs`, `badness inverse-search`).
  Badness never typesets. It also never *maps*: investigating texlab showed it
  parses no SyncTeX either, because every SyncTeX-aware viewer links libsynctex
  and so takes a file and a line, never a coordinate. Servers publish per-process
  advertisements rather than sharing texlab's single fixed socket, which a second
  editor window silently steals.

- [ ] *(Design decision)* **Native `.synctex.gz` reader.** Would let forward
  search drive viewers with *no* SyncTeX support at all by resolving a page
  number (qpdfview, a browser), and report an honest `Failure` when a line
  produces no output instead of launching a viewer onto nothing. Costs a gzip
  dependency, a parser with real traps (compressed `,=` points, `Input:` lines
  interleaved mid-file, `./`-segment path matching, leaf-vs-enclosing-box lookup
  semantics), and a fixture corpus validated against the `synctex` CLI with no
  existing oracle to lean on. The seam is already in place: `SearchTarget` in,
  `ForwardSearchStatus` out — a backend behind it changes no LSP surface, no
  `[build]` key, and no config. Not worth it until a page-only viewer is a real
  target.

## BibTeX/BibLaTeX

- [ ] **`deprecated-suppression-syntax`: report the retired `% badness-ignore`
  family.** `% badness-ignore <rule>` and `% badness-ignore-file [<rule>]` are
  undocumented but still resolve (permanently — a directive spelling is
  user-facing API). Nothing tells a user their file carries the old spelling, so
  a warning with a **safe** autofix rewriting to `% badness-lint skip <rule>` /
  `% badness-lint skip-file <rule>` is the missing half of the deprecation. The
  rewrite is entirely inside a comment, so it is textual, trivially lossless,
  and needs no layout decision — exactly the fix contract. The fact is already
  computed: `directives::Directive::deprecated` marks these at parse time, so the
  rule needs `Suppressions` to retain the directives it saw (with the comment
  token's range) rather than only the resolved ranges. Covers both carriers —
  the `%` comment and the `.bib` `@comment{…}` entry.

- [ ] **A meta rule for inert suppression directives.** Ruff's documented wart is
  that a misplaced `# fmt: off` does nothing and says nothing; badness now has
  the same hole. Report a `% badness…` directive that suppresses nothing: an
  `on` with no open region, a `skip` with no following construct, an `off` left
  unclosed at EOF (which runs to end of file on purpose, but is worth saying), a
  `% badness-format` directive in a `.bib` (parsed, deliberately inert), and a
  directive written on a `.dtx` doc-margin line, where the leading `%` is a
  margin rather than a comment so the directive is inert by construction. Wants
  the same retained-directive list as the rule above, so do them together —
  fatou's `meta/*-suppression` rules are the model. A natural companion is
  `unexplained-suppression` (no `: <reason>`).

- [ ] **Format suppression in `.bib`.** The `% badness-format` axis parses in a
  `.bib` `@comment{…}` and deliberately does nothing. The bib formatter is a
  canonical re-emitter rather than a trivia-only pass, so "reproduce this span
  byte for byte" is a genuinely different mechanism there, not a matter of
  routing the resolved ranges through. Until it exists, the axis is silently
  inert, which is the meta rule above's job to report.

- [ ] **A `%`-comment directive carrier inside a `.bib` entry.** Now that a `%`
  comment exists inside an entry, the LaTeX-side carrier could work there too;
  today only the `@comment{…}` entry form does (`bib/linter/suppression.rs`).
  The grammar is already shared, so this is only a decision about what an
  in-entry comment attaches to (the field below it, presumably, matching the
  formatter's forward bind).

- [ ] **`task bib-error-compat`: biber as a `.bib` *error* oracle.** The gap the
  `%`-comment bug exposed — `bib-parse-compat` cannot see over-strictness at all,
  because texlab's bib parser has no error channel (the skill says so outright),
  so a whole family of files we wrongly refused sat in a gate baseline instead of
  failing a gauge. biber does have one: btparse reports real syntax errors, e.g.
  `ERROR - BibTeX subsystem: …, line 2, syntax error: found "author", expected
  end of entry ("}" or ")") (skipping to next "@")`, plus an `INFO - ERRORS: n`
  tally. Cross-tabulate per corpus file, `badness has diagnostics` × `biber has
  ERRORS`: agreement on the diagonal, **badness dirty + biber clean = over-strict**
  (this bug's class), badness clean + biber dirty = under-strict (we would format
  something biber rejects).

  Two constraints, both learned by trying it:

  - **Boolean per file only** — never error counts or positions. biber recovers by
    skipping to the next `@`, so it under-counts badly; in a three-entry probe it
    never reported an unterminated `@misc` at EOF at all, having swallowed it
    during recovery.
  - **Do not project `biber --tool` output onto a skeleton.** Tool mode exposes
    biber's *data model*, not its parse, and the transformation would swamp any
    real divergence: `author = {Ann Author and Bo Beispiel}` comes back as
    `{Author, Ann and Beispiel, Bo}`, `year` + `month` merge into `DATE = {2021-11}`
    (both source field names *gone*), `#` concatenation is resolved, and
    `--output-format=biblatexml` additionally explodes names into
    `<bltx:namepart>` and resolves `@string` uses away. texlab stays the right
    *structural* oracle precisely because it is coarse and syntactic; biber's job
    here is only "is this legal BibTeX".

  Placement: biber is an external binary, not a crate, so this cannot be an
  in-process dev-dependency like `texlab-parser` (and Text::BibTeX is not
  separately installed — biber bundles it). Same bucket as `task typeset:check`:
  needs a local install, runs on demand, never in CI.

- [ ] Cross-file `undefined-string`: a `@string` defined in one `.bib` and used
  in another resolves only once a project-level `@string` union exists (today
  single-file-sound, same caveat as `unused-string`).

- [ ] `unused-entry`: a `.bib` entry never targeted by any `\cite`-family
  command, project-aware behind the same closed+rooted namespace gate as
  `unreferenced-label`/`undefined-ref` (the bib linter has `unused-string` but no
  `unused-entry`). Report-only. texlab: `UnusedEntry`.

- [ ] Bib document-symbol outline completeness: `src/bib/outline.rs` surfaces
  regular entries only; consider `@string`/`@preamble`/`@comment` blocks (and a
  richer `SymbolKind`/detail).

- [ ] Shared component-finder: `ResolvedCitations` duplicates the union-find +
  component assignment from `ResolvedLabels` (`project/citations.rs`); factor one
  helper when a third consumer appears.

- [ ] **`subfiles`' `\subfix` wrapper opens the citation namespace.** The
  package's path fixer is the idiomatic way a subfile names a shared resource
  (`\addbibresource{\subfix{references.bib}}`), but a macro inside the group
  makes `nth_group_text` return `None`, so the target is `BibTarget::Dynamic`
  and the whole component goes open — silently disabling `undefined-citation`
  project-wide for exactly the projects issue #112 was about. Conservative (a
  loss of coverage, never a false positive), hence not urgent. Unwrapping it is
  a *shape* fact, not meaning — `\subfix{p}` is transparent by construction, the
  same class of static recognition as the `subfiles` class-option gate — so it
  fits decision #8; the open part is where the unwrap belongs so `include.rs`,
  `document_link`, and completion cannot drift on it.

- [ ] **`project::package` does not collapse `.`/`..` in load targets.**
  `include.rs`'s resolvers now normalize lexically (`resolve_against`), so
  `\input{../shared}` and a `subfiles` parent resolve; `package.rs`'s `resolve`
  still does not, so `\usepackage{../mypkg}` never matches a member and its
  signatures stay out of scope. Benign (a missing local scope, not a wrong one)
  and a separate subsystem, so it was left out of the #112 fix. Lift the helper
  to `project.rs` when touching it.

- [ ] **Central-bib fallback via the texmf index *(LW)*.** LaTeX Workshop
  resolves `\bibliography{refs}` through `kpsewhich` (plus a `bibDirs`
  setting) for users who keep one master `.bib` in their texmf tree. Extend
  citation resolution to fall back to the read-only texmf index
  (`project::texmf`) for bib paths that don't resolve project-locally.
  LSP-only, sanctioned by the AGENTS.md environment-awareness tiers
  (completion, hover, go-to-definition); the `undefined-citation` lint and the
  CLI stay hermetic and project-local.

## CST / AST / trivia

- [ ] **[low, latent] No `SyntaxNodePtr`/`AstPtr`.** RA stashes stable node
  pointers in salsa data to re-resolve across reparses; badness sidesteps this by
  storing the `GreenNode` directly (decision #7) and carrying diagnostics as
  byte-ranges (decision #4), so the need has not arisen. Latent: a future feature
  that must stash a *stable node identity* in a salsa query (resolving a
  completion/hover target to a specific node across edits) has no primitive for
  it, and byte-ranges alone do not survive edits.

- [ ] **Collapse the four near-identical token walks in `ast/nodes.rs`**
  (`Group::inner_text`/`inner`, `NameGroup::text`/`range`): all four walk
  `children_with_tokens`, skip the delimiters, bail on nested nodes, and
  accumulate text and/or a range. The drift risk is demonstrated, not
  hypothetical — the issue-#104 `HASH` rejection made it into two of the
  four. One shared helper.

- [ ] **Mark the free-function AST shims `#[deprecated]`** (or file the
  removal issue) once the formatter/linter call sites migrate — two parallel
  APIs for the same reads with no forcing function is a standing invitation
  for new code to pick the wrong one.

- [ ] **Share the cross-language boilerplate that is past due**: one
  `SyntaxError` for `parser::core` and `bib::core` (two identical structs
  today, a type-level fork consumers handle twice), an `impl_rowan_lang!`
  macro for the duplicated `Language`/transmute boilerplate (leaves one
  audited `unsafe` instead of two), and a compile-time `ROOT`-is-last
  assertion making the "do not add variants after `ROOT`" comment
  mechanical. Leave the rest of the bib parallel alone — it is disciplined,
  self-labeled duplication with the unification path recorded in place, and
  genericizing events/tree_builder/`Parser` at n=2 would be a premature
  abstraction.

## Open decisions to revisit

- [ ] How much of `\newcommand`/`xparse` to model. *(Semantics)*

- [ ] Formatter opinionatedness: configurable vs. fixed. *(Formatter)*

- [x] Model `.dtx` as one lossless CST with margin-aware tokens; project safe,
  fully margined documentation environments into virtual LaTeX in the formatter.
  *(Package infrastructure)*

- [ ] Math preview on hover: skip (LaTeX Workshop covers it), render in the
  VS Code extension, or a server-side Rust renderer? *(Language server; see
  `### Hover`)*
