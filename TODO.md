# Badness TODO

A LaTeX formatter, linter, and language server on a lossless rowan CST,
following **rust-analyzer's** architecture. See `AGENTS.md` for load-bearing
design decisions and invariants.

Single-crate package (not a workspace). Parser and formatter are **intentionally interleaved**: the formatter is the primary tool for stress-testing the parser.

Status: `[ ]` todo · `[~]` in progress · `[x]` done

--------------------------------------------------------------------------------

## Parser

## Formatter

- [x] **Math operator spacing.** A single space around each binary/relation atom
  (`a+2*1^5` -> `a + 2 * 1^5`, `x=-b` -> `x = -b`); unary signs and `^`/`_` scripts
  stay tight; command operators (`\cdot`, `\leq`) join via `math_atom_role`.
  Group bodies normalized (`x^{a+b}` -> `x^{a + b}`). Scientific notation (`1e-5`)
  is a known non-special-cased limitation.
- [ ] **Opaque-group layout non-determinism.** The content-kind taxonomy has
  landed: `ArgSpec` now carries a `ContentKind` enum (`Opaque`/`Prose`/
  `TokenList`) the formatter dispatches whitespace and break policy on
  (`DocumentBody` stays an environment-body concept via
  `EnvironmentSig::no_indent`; add it when a command-arg case appears). What
  remains is the non-determinism fix: `spans_multiple_lines` decides
  block-vs-inline from incidental source newlines, sidestepped for the
  `TokenList` kind but still governing every `Opaque` multi-line group. Give
  `Opaque` groups a deterministic layout policy that does not depend on
  incidental whitespace.
- [ ] **Long collapsed cite list overflow.** A `collapse` arg folds to one line
  even when the key list exceeds the width; it never breaks *at commas* (one
  key per line) as a fallback. Needs the token-list content kind to break on
  its own separators rather than the paragraph fill.
- [ ] Widen the prose-argument table (CWL ingest could feed it); consider gluing
  a prose arg onto its command line when a source break separates them.
- [ ] **Hanging continuation indent for wrapped statements (B', deferred ---
  blocked on structure).** A wrapped brace-body line ideally hangs its continuation
  one step in (`\node[…] at (2,3)`/`····{…};`) to read as a continuation rather
  than a sibling. This **cannot be idempotent** under the generic CST: the wrap
  becomes a real source newline, and on re-parse the continuation is just a line at
  the body indent (no marker says "continuation"), so the next pass flushes it ---
  `fmt(fmt(x)) != fmt(x)`. Flush-B sidesteps this precisely because there is no
  indent delta. The real fix needs a node that *owns the whole statement*, so layout
  derives from structure (source newlines insignificant). For the motivating case
  (`\node[…] at (2,3) {…};`) that node is a **TikZ path statement**: `at` keyword,
  `(coord)`, `;` terminator, `{label}`—none of which are TeX-surface facts
  (`;`/`at`/`()` carry no special catcode in plain TeX), so grouping them is
  package-specific grammar, out of scope for the generic parser (decisions #1, #2;
  non-goals). Belongs in a future sanctioned **TikZ-aware mode** (its own grammar,
  corpus, and AGENTS.md amendment), not a formatter patch.
- [x] Column-spec-aware L/C/R cell alignment and `\multicolumn` for the table
  environments. The `{lcr}` spec is parsed (`formatter::colspec`, layout-only,
  bails to all-left on any unrecognized token; `p`/`m`/`b` read as left, `*{n}{}`
  expands, `>{}`/`<{}`/`@{}`/`!{}` and rules produce no column) and threaded into the
  grid renderer: each cell aligns L/C/R, a right/center last column pads on the left
  only (no trailing whitespace), and a `\multicolumn{n}{spec}{…}` spans `n` columns
  (excluded from single-column widths, aligned within its span by its own spec, wide
  markup overflows rather than ballooning the data columns). Also handled:
  `\cmidrule(lr){2-3}` paren trim specs (the `(lr)` `WORD` and detached `{2-3}` group
  are consumed as part of the rule line) and the same-line `\\ \hline` form (a rule
  sharing a physical line with the preceding `\\` is normalized onto its own
  passthrough line).

## Linter

- [ ] **Mine the ChkTeX warning catalog (~44 warnings) for missing rules.**
  LaTeX Workshop adds no lint rules of its own (it only shells out to
  ChkTeX/lacheck, both off by default), so ChkTeX's catalog is the source to
  compare against. Badness already covers the high-value territory (ellipsis,
  dash length, straight quotes, `$$`, space-before-`\footnote`, intersentence
  spacing); remaining candidates include space before punctuation or
  parentheses and missing italic correction (`\/`).

## Semantic layer & signatures

- [ ] How much of `\newcommand`/`xparse` to model for the signature DB. *(open
  decision)*

## Language server

### Feature status vs texlab

A deliberate diff against **texlab** (5.25.1) as the mature reference LSP. The
comparison is asymmetric, and the framing matters when triaging the items below.

- **Badness leads:** range formatting, on-type formatting, code-action quick-fixes
  (texlab's `codeAction` handler returns an empty array), a native linter (25
  LaTeX + 9 bib rules vs texlab's 4 built-ins plus a `chktex` shell-out),
  comment-run folding, the deterministic rule-based formatter, and a first-class
  bib formatter with entry sorting. texlab also ships **no** semantic tokens,
  signature help, selection ranges, or code lens—so those are not texlab gaps.
- **Badness trails (tracked below):** completion breadth (`### Completion`) and
  matching-pair highlight (`### Navigation & structure`). (Macro references/
  rename, package hover, and the change-environment refactor have since shipped;
  see the `[x]` items below.)
- **Deliberately not matched:** the typeset-adjacent features under
  `## Editor integration` (build, clean-aux, chktex passthrough, CSL rendering).

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
  `wrap-target`, `indent-width`, `wrap`) and `[lint]` (`select`/`ignore`, applied via
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

### Formatting

- [x] Minimal-diff paragraph wrapping: `wrap = "minimal"` retains authored
  equilibrium breaks and uses `wrap-target` as a soft target under the hard
  `line-width`; implemented as source-aware `PreferredFill` IR with global
  lexicographic layout selection, preserving the formatter-engine boundary.

### Navigation & structure

- [x] Selection ranges (`textDocument/selectionRange`)—expand-selection from
  the CST's node hierarchy (group → argument → command → environment). Subsumes
  texlab's `findEnvironments` command: the enclosing-environment stack falls out
  of the CST-hierarchy expansion. (`lsp/selection_range.rs`: leaf token +
  `parent_ancestors` up to `ROOT`, consecutive-equal ranges collapsed; single-file
  read-pool job mirroring folding.)

### Completion

Badness offers command, environment, label, cite-key, bib field/type, and file
completion (`src/completion.rs`, `src/bib/completion.rs`). texlab's completion
breadth is its biggest lead (`crates/completion/providers/`); the specialized
sources below are missing.

- [x] **Color name + model, TikZ/PGF library completion**—small static datasets
  (`data/colors.json`, `data/tikz_libraries.json`) for
  `\color`/`\textcolor`/`\definecolor` and `\usetikzlibrary`/`\usepgflibrary`.
  Color-name completion also merges document `\definecolor`/`\colorlet` names.
  (Model completion is brace-arg only; the optional-arg form `\color[rgb]{…}` is
  not yet classified.)
- [x] **Argument-value enum completion** for fixed enumerated argument choices
  (`\pagestyle`, `\pagenumbering`, `\bibliographystyle`, `\theoremstyle`,
  `\mathversion`, `\fontshape`/`\fontseries`). A curated side dataset
  (`data/arg_enums.json`) keyed by command name then *brace*-group index, consumed
  by completion only (`semantic::signature::arg_enum_values`)—it is *not* a field
  on the formatter's `ArgSpec` (cold completion data stays out of the hot `Copy`
  struct), mirroring the color/TikZ datasets. Static built-ins only; merging
  document-defined values (e.g. `\fancypagestyle` names into `\pagestyle`,
  `\newtheoremstyle` into `\theoremstyle`) the way colors merge `\definecolor` is a
  clean follow-up on top of the table.
- [ ] *(Design decision)* **Package-scoped command completion.** texlab suggests
  only commands provided by the loaded packages (a package→command component
  model). Badness's signature DB is flat (curated + CWL + scanned); scoping
  completion to `\usepackage`-loaded packages needs package→command attribution.
  Open question, not a mechanical add.
- [ ] **Citation completion filterable by title/author *(LW)*.** LaTeX
  Workshop's `\cite{` completion matches on the entry's title and other fields,
  not just the key (`intellisense.citation.filterText`)—type a word from the
  paper's title instead of remembering the key. Badness already resolves
  entries cross-file and lazily (`lsp/completion_resolve.rs`); widen
  `filter_text` (and `sort_text`) on citation items to key + title + authors.
  VS Code truncates `filterText` at 128 chars, so field order matters (key
  first).
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
- [ ] **Documentation link in package hover *(LW)*.** LaTeX Workshop's
  `\usepackage` hover offers a "View documentation" link via `texdoc`. The
  shipped CTAN metadata (`data/package_metadata.json`) already carries a
  `ctan` field—append a documentation link (`https://ctan.org/pkg/<name>`) to
  the package hover markdown.
- [ ] *(Design decision)* **Math preview on hover *(LW)*.** LaTeX Workshop's
  most-loved language feature: hovering math renders it (MathJax,
  client-side); texlab lacks it too, so it is also a differentiator. Options:
  (a) skip—LaTeX Workshop covers it, and coexistence is the story; (b) render
  in the VS Code extension—breaks the thin-client principle and is VS
  Code-only; (c) server-side SVG via a Rust math renderer (ReX or similar) as
  a data-URI image in hover markdown—editor-agnostic, but ships a math layout
  engine, which is typesetting in all but name (pressure on the AGENTS.md
  non-goal). Lean (a) for now; whichever way, record the decision in
  AGENTS.md.

### Code actions

- [ ] **Surround selection with environment/command *(LW)*.** LaTeX Workshop
  ships these as client-side commands; badness can host them editor-agnostically
  as code actions or `executeCommand`s alongside `changeEnvironment`
  (`lsp/code_action.rs`).
- [ ] **Section promote/demote *(LW)*.** Recursively shift sectioning levels
  across a selection (`\section` ↔ `\subsection`); the sectioning hierarchy is
  already in the signature DB, so this is a mechanical rewrite.

## Package & class infrastructure (`.sty`/`.cls`/`.dtx`/`.ins`)

The document-level tools are mature; the next frontier is the **package
ecosystem**—class and package sources, and the literate `.dtx` format they
ship in. This is a large, multi-area subproject (parser + formatter + semantic).
It stays inside the AGENTS.md non-goals: bounded, statically-recognizable
patterns only, signatures *extracted, never executed*, no docstrip run, no TeX
engine.

### Parsing

- [x] **expl3 full catcode model.** `~` is a literal space (catcode 10) and spaces/tabs
  are ignored (catcode 9) inside expl3 regions, so the formatter owns in-region layout
  regardless of `WrapMode`: one statement per source line, brace-group code blocks indent,
  inter-token whitespace collapses to a single space, and `~` is a breakable space. The
  formatter recomputes region membership read-only (`formatter::core::expl3_regions`, sharing
  the lexer's `expl_toggle` set); the CST/lexer are untouched, so losslessness holds and the
  layout is idempotent by construction (catcode-9 whitespace is insignificant, so it supersedes
  the flush-B deferral for expl3 — see "Hanging continuation indent" above). See the expl3
  code-formatting note in `AGENTS.md` decision #1.
- [ ] **expl3 implicit detection in toggle-less `.dtx` (deferred).** Real expl3
  package sources (e.g. `ltx-talk-structure.dtx`) carry no in-file `\ExplSyntaxOn`/
  `\ProvidesExpl*`; expl3 is declared in the parent `.dtx`/build, and `@@` is a
  docstrip module prefix (`%<@@=mod>`). Treat `macrocode` bodies as expl3 when the
  file carries a static expl3 signal (a `%<@@=mod>` guard or `\ProvidesExpl*`
  anywhere). Needs a file-level scan plus the `macrocode` save/restore interaction
  (mirror `at_letter`).

### Formatting

### Semantic and integration

## Performance & hardening

- [ ] Fuzzing (losslessness must hold on arbitrary input).
- [~] Large-doc benchmarks (`hyperfine`, criterion); flamegraph hot paths.
  Formatter speed bench vs `tex-fmt`/`latexindent` landed (`benches/compare_format.sh`,
  `task bench`, writes `benches/benchmark_results.json`, which feeds the docs
  benchmark page `docs/src/reference/benchmarks.md`). In-process parse/format micro-bench +
  flamegraph hot paths landed (`benches/formatting.rs`, `task bench:micro`/`bench:profile`;
  see the profiling item below). Still pending: bib + lint benchmarks.
- [ ] Intra-file incremental reparse (reuse green subtrees on contained edits).
- [ ] `wasm32` build for a web playground.

## Editor integration

texlab bundles PDF-workflow features. Only position mapping (no typesetting by
badness) is admissible; the rest are explicit non-goals recorded here so they are
not re-proposed.

- [ ] **Forward/inverse SyncTeX search (no typesetting).**
  `textDocument/forwardSearch` (a custom LSP method) locates a configured PDF and
  drives an external viewer; inverse search receives a viewer position over IPC
  and answers with `window/showDocument`. Badness never typesets—it only maps
  source↔PDF positions via SyncTeX and shells the viewer. texlab:
  `crates/commands/fwd_search` + the `ipc` crate.
- **Non-goals (do not re-propose):** build/latexmk orchestration
  (`textDocument/build`), clean-aux/artifacts, `chktex` passthrough (the native
  linter supersedes it), and citeproc/CSL bibliography rendering (badness shows a
  lightweight entry summary; full CSL is out of scope). These sit inside the
  AGENTS.md "We never typeset" non-goal. **Note (boundary split):** the
  kpsewhich/distro package DB is *no longer* a blanket non-goal—a **read-only TEXMF
  file index** feeding *LSP navigation only* now exists (`project::texmf`; see
  "TEXMF index" under Package & class infrastructure and the AGENTS.md "LSP
  environment awareness" section). What stays a non-goal is a distro query feeding
  the **formatter** (it would break formatting determinism).

## BibTeX/BibLaTeX

- [ ] Cross-file `undefined-string`: a `@string` defined in one `.bib` and used
  in another resolves only once a project-level `@string` union exists (today
  single-file-sound, same caveat as `unused-string`).
- [ ] `unused-entry`: a `.bib` entry never targeted by any `\cite`-family
  command, project-aware behind the same closed+rooted namespace gate as
  `unreferenced-label`/`undefined-ref` (the bib linter has `unused-string` but no
  `unused-entry`). Report-only. texlab: `UnusedEntry`.
- [x] Bib-aware LSP completion: `@string` macro names in value position, field
  names per entry type (type-scoped, hiding fields already present), and entry
  types after `@` (`src/bib/completion.rs`); plus `\cite` key completion on the
  `.tex` side, resolved cross-file via `ResolvedCitations` (`src/lsp.rs`
  `cite_completion_items`).
- [ ] Bib document-symbol outline completeness: `src/bib/outline.rs` surfaces
  regular entries only; consider `@string`/`@preamble`/`@comment` blocks (and a
  richer `SymbolKind`/detail).
- [x] `title-capitalization` refinement: a single mid-word capital now counts only
  when it is the *first* capital of a lowercase-initial word (the camelCase brand
  pattern, `iPhone`/`eBay`/`pH`). A later capital in a capital-initial word is a
  surname particle (`McDonald`, `DeForest`, `MacArthur`) or style token (`LaTeX`),
  so it is left alone---no curated name list needed, at the cost of an occasional
  miss (a capital-initial acronym like the cell line `HeLa`). `[A-Z]{2,}` runs
  (`DNA`) still flag regardless of word shape.
- [ ] Shared component-finder: `ResolvedCitations` duplicates the union-find +
  component assignment from `ResolvedLabels` (`project/citations.rs`); factor one
  helper when a third consumer appears.
- [ ] **Central-bib fallback via the texmf index *(LW)*.** LaTeX Workshop
  resolves `\bibliography{refs}` through `kpsewhich` (plus a `bibDirs`
  setting) for users who keep one master `.bib` in their texmf tree. Extend
  citation resolution to fall back to the read-only texmf index
  (`project::texmf`) for bib paths that don't resolve project-locally.
  LSP-only, sanctioned by the AGENTS.md environment-awareness tiers
  (completion, hover, go-to-definition); the `undefined-citation` lint and the
  CLI stay hermetic and project-local.

--------------------------------------------------------------------------------

## rust-analyzer conformance audit

A structured audit of badness against **rust-analyzer** (its architectural
inspiration), across five layers: parser/event-stream, CST/AST/trivia, salsa
incrementality, the LSP server, and the diagnostics/linter model. A read-only
rust-analyzer checkout for triage lives at `.rust-analyzer-ref/` (git-ignored;
`git clone --depth 1 https://github.com/rust-lang/rust-analyzer` to recreate) so
line references stay stable while we work through the items.

**Verdict:** the overwhelming majority of divergences are deliberate,
AGENTS.md-sanctioned, or forced by the LaTeX/catcode domain, and are sound. The
green-node `no_eq` soundness argument, the byte-range error side channel, the
SubTok math split, the catcode-in-lexer modes, the recovery-anchor set, the
firewall-layered cross-file salsa queries, the read-snapshot/threadpool split,
cancellation-via-salsa, version-gated diagnostic publish, incremental document
sync, UTF-16/UTF-8 column math, and comment-suppression coverage were all checked
and found faithful (or better). One agent-reported concern was a **false
positive**: cross-file lint rules are *not* inert in the editor — `analyze_tex`
(`lsp.rs:3023`) and `compute_lint_findings` (`lsp.rs:3278`) thread full project
resolution; only the salsa cancellation/cache-miss fallbacks (`fallback_*`,
`lsp.rs:3186`/`3311`) pass `None`, by design.

The items below are the genuine divergences worth a separate look. None is a
known-live bug; they are latent gaps, hardening opportunities, and editor-UX
capabilities RA has that badness does not. Severity in brackets.

### Robustness / hardening

- [x] **[high] Worker thread panic guard** (`lsp.rs`, `Worker::handle_job_guarded`).
  A panic in the single write-phase worker (`seed_dir`, `apply_watched_change`,
  `project_members`, a poisoned-mutex `.expect`) used to unwind and kill the
  worker thread; the main loop kept running but every `job_tx.send` silently
  no-ops (`let _ = …`), so the server became a quiet zombie. Now the run loop
  routes every job through `handle_job_guarded`, which `catch_unwind`s the panic
  and logs it (mirroring the read pool's per-job isolation, `task_pool.rs:47`), so
  one bad job degrades to a single logged error instead of killing the server.
  (Follow-up still open: recovering from a *poisoned mutex* — see next item — so a
  read-pool panic while holding `files`/`query_log` can't leave the worker unable
  to touch the db.)
- [x] **[med] Mutex poisoning no longer cascades a read panic into worker death**
  (`incremental.rs`, `recover_poison`). The `files`/`query_log` locks now recover
  the inner guard on poison (`.lock().unwrap_or_else(recover_poison)`) instead of
  `.expect("… poisoned")`, so a read-pool job that panics while holding one can't
  cascade into the writer panicking on its next `.lock()`. Sound because each lock
  guards a plain map/vec mutated atomically per access, with no cross-call
  invariant a panic could leave half-updated. Guarded by
  `poisoned_files_lock_recovers`.
- [x] **[med] Global parser step/loop limiter** (`grammar.rs`,
  `Parser::step` + `PARSER_STEP_LIMIT`). The lookahead primitives (`kind`,
  `nth_kind`) now tick a peek budget that resets on every cursor advance (via
  `bump` *or* the math-split `pos += 1` fast path — keyed on `pos`, not `bump`),
  so the surviving count is the number of *consecutive* peeks with no token
  consumed. Exceeding the ceiling aborts loudly instead of hanging — the RA
  catch-all (`parser.rs`), converting "provably terminating by reading the code"
  into "cannot hang on adversarial or malformed input" (fuzzing, a corrupt corpus
  file). A **release-mode** guard (real `assert!`, not `debug_assert`); the async
  callers already recover from a parse panic, so a wedged parse degrades to a
  logged error. Measured overhead on the 95 KB parse bench: within run-to-run
  noise (~1-2%). Guarded by `step_guard_trips_when_wedged` and
  `step_budget_resets_on_cursor_progress`. (Complements the Fuzzing item under
  Performance & hardening.)
- [x] **[low] `--fix` post-application losslessness/parse guard**
  (`main.rs`, `debug_assert_fixes_preserved`). Before `fix_file` writes the
  fixpoint result back, a debug-only, kind-aware (LaTeX + bib) guard asserts the
  output (1) reconstructs losslessly and (2) carries no *new* parse errors vs. the
  original (`errors_before`), so a mis-built fix span that corrupts structure is
  caught before it reaches disk. Compiled out of release builds.
- [x] **[low] Debug open/close balance assertion**
  (`grammar.rs`, `debug_assert_balanced`). After `parse()` builds the event
  stream, a debug-only pass walks it (+1 `Start`, -1 `Finish`) and asserts it
  never goes negative and ends at zero — catching a leaked `open()` or an
  unbalanced `precede` splice at parse time (counting *all* start/finish events
  regardless of how emitted), rather than as an opaque rowan `finish_node` panic
  later. The cheap post-hoc analog of RA's per-`Marker` `DropBomb`; compiled out
  of release builds.

### Incrementality (salsa)

- [ ] **[med, latent] No input durability tier.** RA sets
  `Durability::HIGH/MEDIUM/LOW` per source root (`base-db/change.rs`); badness's
  setters (`incremental.rs:622`, `SourceFile::new`) never call
  `.with_durability(...)`, so every input is implicitly `LOW`. Harmless *today*
  because badness has zero rarely-changing salsa inputs (config, the built-in
  signature DB, and CWL/package/texmf/aux data all live in `LazyLock`/`OnceLock`
  or a plain `HashMap`, deliberately outside the db per the hermeticism tenet).
  But the moment config or package data is promoted into salsa — the natural
  direction for cross-file work — it must be `HIGH`/`MEDIUM` or every keystroke
  will invalidate it. Record the requirement now.
- [ ] **[low] `Project` re-interned from a fresh member `Vec` per request**
  (`incremental.rs:889`, `Analysis::resolve_project`/`scope_signatures`/…).
  Interning dedups by value, so an unchanged sorted membership yields the same id
  and the memo survives — correct *provided* member construction is always
  identically sorted and deduped. It is today (via `tracked_files`,
  `incremental.rs:672`); flag as fragile if member-list construction order ever
  drifts, since a silent id churn would recompute every cross-file query.

### CST / AST / trivia

- [ ] **[low, perf] `LineIndex` re-scans the buffer per call and requires the
  caller to pass `text` back in** (`text/line_index.rs`). Wide-char *correctness*
  is right (the common bug — verified: UTF-16/UTF-8 column math, astral chars,
  CRLF all handled). But RA's standalone `line-index` precomputes a per-line
  wide-char table at construction and owns no text, so queries are O(wide-chars)
  with no re-walk and no "hand the same buffer back" misuse hazard. Consider
  precomputing the table if line-index shows up in a profile.
- [ ] **[low, latent] No `SyntaxNodePtr`/`AstPtr`.** RA stashes stable node
  pointers in salsa data to re-resolve across reparses; badness sidesteps this by
  storing the `GreenNode` directly (decision #7) and carrying diagnostics as
  byte-ranges (decision #4), so the need has not arisen. Latent: a future feature
  that must stash a *stable node identity* in a salsa query (resolving a
  completion/hover target to a specific node across edits) has no primitive for
  it, and byte-ranges alone do not survive edits.

### Diagnostics / linter model (editor-UX capabilities RA has)

- [x] **[med] LSP diagnostic tags.** `lint_to_lsp` now sets `tags` via
  `lint_diagnostic_tags`, a presentational rule-id→tag map (kept out of the
  `Diagnostic` struct so the CLI renderer is untouched): `unreferenced-label` →
  `Unnecessary` (editor dim), and `deprecated-command`/`obsolete-environment`/
  `primitive-command` → `Deprecated` (strike-through). Extend the match as more
  rules earn a tag.
- [x] **[med] `related_information` / secondary spans.** `Diagnostic` now carries
  a `related: Vec<RelatedInfo>` (`{path, start, end, message}`), populated by
  `duplicate-label`: the intra-file branch points at the first definition's key
  range (precise, same file), the cross-file branch at each other definer. It
  renders as `DiagnosticRelatedInformation` in LSP (`lint_to_lsp` →
  `lint_related_to_lsp`) and as annotate-snippets context annotations in the CLI
  (`render_pretty`, cross-file loaded via `source_for`). Messages stay unchanged
  (additive), so concise output and `related`-blind clients are unaffected.
  **Cross-file secondaries are file-level (`0..0`, the document start), not the
  exact `\label` byte range:** `ResolvedLabels` / the `file_labels` firewall
  deliberately store names→paths only so a label move backdates and doesn't
  rebuild cross-file resolution (tenet #2). Precise cross-file carets would need a
  separate per-file name→range query resolved lazily at the render/LSP layer —
  a possible follow-up.
- [x] **[low] `code_description` (rule doc URL).** `lint_to_lsp` now sets
  `code_description.href` to `https://badness.dev/reference/linter-rules.html#<rule>`
  (the mdBook anchor equals the rule id), so editors deep-link the rule's docs.
  Gated by a `link_docs` flag threaded from the analyze/code-action sites: the
  LaTeX arms pass `true`, the bib arms `false` (bib rules aren't catalogued on that
  page yet — the `code` still carries the rule id, just without a link). Wire the
  bib rules in once they get a reference page.
- [ ] **[low, latent] Fix model can't express cross-file fixes.** `Fix` now
  carries a set of disjoint edits in the diagnostic's own file
  (`linter/diagnostic.rs`), applied atomically (`linter/fix.rs`: a fix lands
  with all its edits or none), so multi-location rewrites are expressible —
  `dollar-display-math` and `obsolete-environment` swap both delimiters/names
  in place with the body untouched. What remains of RA's `SourceChange` is the
  *per-file* dimension: a fix spanning files (e.g. rename a label and its
  `\ref`s across a multi-file project) has no representation, and the CLI's
  per-file fixpoint (`main.rs::fix_file`) has no way to apply one. Defer until
  a rule needs it; when it comes, key edit sets by path (the `RelatedInfo`
  pattern: real paths stamped at rule time) and teach both apply paths
  (`lint --fix`, LSP `WorkspaceEdit`) to honor them atomically.

### Maintainability (not a conformance gap, surfaced by the audit)

- [x] **[low] Factor the duplicated trivia/blank-line scanners.** The blank-line
  and comment-bind logic was re-implemented across ~five methods in `grammar.rs`
  (`peek_meaningful`, `at_paragraph_break`, `trivia_run_is_separator`,
  `binding_run`, `at_script`), each re-walking trivia with slightly different
  newline-counting and a near-identical `.dtx` margin/guard comment block. RA
  concentrates the equivalent in one `n_attached_trivias`. Now a single
  `scan_trivia(from, CommentMode)` helper returns a `TriviaScan` (`next`,
  `next_kind`, `saw_blank_line`, `comment_start`); all five methods are thin
  wrappers over it. `CommentMode::Stop` folds in `at_script`'s narrower regime (a
  comment ends the line and stops the scan) without a second walker.

--------------------------------------------------------------------------------

## Open decisions to revisit

- [ ] How much of `\newcommand`/`xparse` to model. *(Semantics)*
- [ ] Formatter opinionatedness: configurable vs. fixed. *(Formatter)*
- [ ] `.dtx` two-layer model: a preprocessor that splits doc/code layers, or a
  single lexer mode with margin-aware tokens? *(Package infrastructure)*
- [ ] Math preview on hover: skip (LaTeX Workshop covers it), render in the
  VS Code extension, or a server-side Rust renderer? *(Language server; see
  `### Hover`)*
