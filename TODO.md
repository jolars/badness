# Badness TODO

A LaTeX formatter, linter, and language server on a lossless rowan CST,
following **rust-analyzer's** architecture. See `AGENTS.md` for load-bearing
design decisions and invariants.

Single-crate package (not a workspace). Parser and formatter are **intentionally interleaved**: the formatter is the primary tool for stress-testing the parser.

Status: `[ ]` todo · `[~]` in progress · `[x]` done

--------------------------------------------------------------------------------

## Parser

## Formatter

- [ ] **Math operator spacing is inconsistent between script args and command
  args** (surfaced by issue #42's examples). A braced script argument is lowered
  through the math seq path and gets operator spacing (`\sum_{i=1}^m` ->
  `\sum_{i = 1}^m`, `\Big \}^{1/2}` -> `\}^{1 / 2}`), while a command argument in
  math mode (`\frac{1}{n^{m+1}}`) is left untouched — the two should agree.
  Related conventions question: `/` (and arguably `*`) is conventionally set
  tight (`1/2`, per Knuth), and script-size content is conventionally tight
  overall, so the likely resolution is tight `/` everywhere and no operator
  spacing inside `^`/`_` arguments — decide, then make both paths agree.
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
- [ ] **Keyval-aware optional-argument layout (parked; grew out of issue #47).**
  The landed fix only collapses a fitting multi-line `[…]`; commas pass through
  wherever the author put them. The Black/Ruff *magic trailing comma* (a trailing
  `,` before `]` forcing one-key-per-line) was prototyped and **declined by
  design**: it is deterministic but not canonical — content steering layout
  conflicts with the formatter-is-sole-authority tenet. The parked replacement,
  two independent pieces:
  - *Count-based expansion:* expand one key per line when a `[…]` has more than N
    top-level keys (or overflows the width); else collapse. Canonical — layout is
    a pure function of content + width. Splits must stay meaning-safe: only at a
    top-level comma already followed by whitespace (whitespace ↔ newline is
    TeX-identical); a glued `[a=1,b=2,…]` has no safe split point and stays
    inline. Semantics to settle: N (default, knob or fixed), and that comma count
    is a proxy for keyval-ness (a comma-rich textual optional would expand too).
  - *Formatter-owned trailing comma, signature-gated:* for an argument the
    signature DB can *prove* keyval (a new `ContentKind::Keyval` or flag), add
    the trailing comma when expanded and drop it when collapsed, Black-style —
    safe because keyval/xkeyval/pgfkeys/l3keys and `\ProcessOptions` clists all
    ignore empty entries. Data: curated built-ins first (`\usepackage`,
    `\includegraphics`, `tcolorbox`, `minted`, …); the CWL corpus marks these via
    `#keyvals:` sections, which `gen_cwl_signatures.py` currently skips. Content
    insertion on a wrong flag changes typeset output, so hold it to the curated
    standard of the math-env routing; never for scanned user definitions, never
    for unknown commands.
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

## Linter

- [x] **`makeat-macro` fires on xypic syntax (false positive).** `\ar@{=}`,
  `\xymatrix@C=3pc` (97/97 findings on the HoTT book). The `@` is intentional
  xypic DSL, not a forgotten `\makeatletter`. Fix: a curated exact-name gate in
  `src/linter/rules/makeat_macro.rs` skipping the control words `ar`/`xymatrix`
  before the `@`, mirroring `space_before_command`'s `NO_SPACE_COMMANDS`.
- [ ] **`hard-coded-reference` fires inside citation locators and theorem titles
  (false positive).** `\cite[Section~8.1]{…}` (an external-work locator) and
  `\begin{thm}[Conway's Theorem 0]` (a proper-name title). Fix: skip when
  `in_key_argument(tok)` (reuses the existing gate — `cite` is already a
  key-argument command) and add a small gate for a token inside an `OPTIONAL`
  whose parent is a `BEGIN` node (an environment title, not a cross-reference).
- [ ] **DSL false positives: `dash-length` in `\texttt`, `ellipsis` in
  `\foreach`.** `\texttt{03-02}` (an MSC code — the hyphen is literal in
  monospace) and `\foreach \x in {0,20,...,350}` (pgffor range syntax; `\dots`
  would break the loop). Fixes: a `\texttt`-family gate for `dash-length`; an
  `in_foreach_range` gate for `ellipsis` — note the range list `{…}` is a
  *sibling* of `\foreach`, not a direct argument, so it needs a previous-sibling
  walk.

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
- [x] **Citation completion filterable by title/author *(LW)*.** LaTeX
  Workshop's `\cite{` completion matches on the entry's title and other fields,
  not just the key (`intellisense.citation.filterText`)—type a word from the
  paper's title instead of remembering the key. Badness already resolves
  entries cross-file and lazily (`lsp/completion_resolve.rs`); widen
  `filter_text` (and `sort_text`) on citation items to key + title + authors.
  VS Code truncates `filterText` at 128 chars, so field order matters (key
  first). Done: `cite_completion_items` returns the full namespace (no
  server-side key filter) with `filterText` = key + title + authors and
  `sortText` = key; `title`/`authors` cached on the bib semantic `Entry`.
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
- [ ] Bib document-symbol outline completeness: `src/bib/outline.rs` surfaces
  regular entries only; consider `@string`/`@preamble`/`@comment` blocks (and a
  richer `SymbolKind`/detail).
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

### CST / AST / trivia

- [ ] **[low, latent] No `SyntaxNodePtr`/`AstPtr`.** RA stashes stable node
  pointers in salsa data to re-resolve across reparses; badness sidesteps this by
  storing the `GreenNode` directly (decision #7) and carrying diagnostics as
  byte-ranges (decision #4), so the need has not arisen. Latent: a future feature
  that must stash a *stable node identity* in a salsa query (resolving a
  completion/hover target to a specific node across edits) has no primitive for
  it, and byte-ranges alone do not survive edits.

--------------------------------------------------------------------------------

## Open decisions to revisit

- [ ] How much of `\newcommand`/`xparse` to model. *(Semantics)*
- [ ] Formatter opinionatedness: configurable vs. fixed. *(Formatter)*
- [ ] `.dtx` two-layer model: a preprocessor that splits doc/code layers, or a
  single lexer mode with margin-aware tokens? *(Package infrastructure)*
- [ ] Math preview on hover: skip (LaTeX Workshop covers it), render in the
  VS Code extension, or a server-side Rust renderer? *(Language server; see
  `### Hover`)*
