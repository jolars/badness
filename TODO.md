# badness—Roadmap

A LaTeX formatter, linter, and language server on a lossless rowan CST,
following **rust-analyzer's** architecture. See `AGENTS.md` for load-bearing
design decisions and invariants.

Single-crate package (not a workspace). Parser and formatter are **intentionally interleaved**: the formatter is the primary tool for stress-testing the parser.

Status: `[ ]` todo · `[~]` in progress · `[x]` done

--------------------------------------------------------------------------------

## Parser

- [x] **Math operator atoms.** In math mode a `WORD` glued around `+ - * / = < >`
  is split into flat operator/operand atoms (byte-range `split_math_word`, trailing
  operand is the scriptable base). Needed a `SubTok` event. See `AGENTS.md`
  decisions #3/#4.

- [x] **Math environments parse in math mode.** An environment the built-in DB flags
  `math` (`equation`, `align`, `gather`, matrix, …) has its body wrapped in a `MATH`
  node (`math_environment_body`, gated by `is_math_environment`), so it enjoys the same
  math-aware layout as `\[…\]` instead of formatting as prose. No lexer changes. See
  `AGENTS.md` decision #1 ("Math environments").

## Formatter

- [x] **Math operator spacing.** A single space around each binary/relation atom
  (`a+2*1^5` -> `a + 2 * 1^5`, `x=-b` -> `x = -b`); unary signs and `^`/`_` scripts
  stay tight; command operators (`\cdot`, `\leq`) join via `math_atom_role`.
  Group bodies normalized (`x^{a+b}` -> `x^{a + b}`). Scientific notation (`1e-5`)
  is a known non-special-cased limitation.

- [x] **Math environment layout.** `lower_math_environment` routes a `math`-flagged
  environment's `MATH` body: a single formula (`equation`) through the relation-aware
  display breaker (`lower_display_math_body`); a grid (`align`/matrix, or a `gather`
  row stack) through `build_alignment_grid` in math mode (cells lower via
  `lower_math_seq`). A top-level `\\` is a hard break in a math body, so row stacks and
  grid-fallbacks keep each row on its own line. Long align rows stay flat (parity with
  the prior grid); per-cell relation breaking is a possible later refinement.

- [x] `Sentence`/`Semantic` (sembr) wrap modes. One sentence per line (width
  ignored); `Semantic` additionally preserves authored soft breaks. Boundary
  detection is a per-language abbreviation profile (`formatter::sentence`, ported
  from panache) driven by `[format] lang` + `[format.no-break-abbreviations]`.
  Babel/polyglossia language auto-detection is deferred (config-only for now).
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
- [x] **Brace-group body reflow (`ReflowKind::Statement`).** A multi-line brace
  body (a `\newcommand` definition body) now reflows as code-like *statements*: each
  source line stays its own logical line, but an over-long one wraps to the width
  (breaking before a trailing `{…}` atom) instead of forcing the printer to detonate
  the innermost nested prose group—the only soft break a rigid
  `lower_element_stream` body exposed. Continuation is **flush**, not hanging.
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
- [ ] Column-spec-aware L/C/R cell alignment and `\multicolumn` for the table
  environments (`tabular`/`array` are now grid-aligned, but every column is
  left-aligned regardless of its `{lcr}` spec). Also: `\cmidrule(lr){2-3}`
  paren trim specs (the parenthesized part isn't recognized as part of the
  rule line, so such a line is treated as a cell and the table falls back),
  and the same-line `\\ \hline` form (only own-line rule commands become
  passthrough lines today).

## Linter

Ships today: `duplicate-label`, `deprecated-command` (`\bf`→`\bfseries`),
`missing-nonbreaking-space` (tie before cite/ref), `obsolete-environment`
(`eqnarray`→`align`), `dollar-display-math` (`$$`→`\[…\]`), `mismatched-delimiter`
(`\left…\right` orientation), `undefined-ref`, `undefined-citation`.

A new rule is a unit struct implementing `Rule` (`src/linter/rules.rs`), added in
four spots there (`mod`, `pub use`, `all_rules()`, `ALL_RULE_IDS`, kept in lockstep
by `registry_and_id_list_agree`) plus a new `src/linter/rules/<name>.rs`. Node-shape
rules declare `interests(&[SyntaxKind])` + `check`; whole-file/semantic rules use
`check_file` (reading `ctx.model`, `ctx.resolution`, `ctx.citations`, signature DB).
An optional `Fix::safe`/`Fix::unsafe_` is picked up by `--fix`, LSP code actions, and
`select`/`ignore` for free. The candidates below are all **type-(B)** lints
(content/semantic/typographic) a deterministic formatter would never make — type-(A)
layout items (whitespace, indentation, brace-on-scripts) are the formatter's job and
excluded. Sources: ChkTeX (numbered warnings), lacheck, textidote.

- [~] Wire the remaining report-only fixes onto the autofix infra:
  `deprecated-command`'s `\bf → \bfseries` is **done** (a `Safe` control-word swap,
  consumed by `lint --fix` and the new LSP code actions); `obsolete-environment`'s
  `eqnarray → align` is still report-only.
- [~] `missing-nonbreaking-space` (a tie before a cite/ref command, broad curated
  family, `\nocite` excluded; `Unsafe` autofix) is **done**. *Follow-up:* the tie lint
  only covers a same-line `WORD WHITESPACE \cmd` shape; a *source line break* before
  the command (`Figure\n\ref{x}`) is also a breakable space but is left for a later
  pass (replacing the newline with `~` reflows the source and overlaps the formatter).

### Tier 3 — structural / semantic / project-layer (curated subset)

Whole-file or cross-file (`check_file`), using the semantic model, signature DB, and
project resolution. Pure prose-opinion textidote rules (title capitalization, caption
period, section length) are skipped as grammar-tool territory.

- [ ] `missing-required-argument`—command invoked with fewer `{…}` groups than its
  signature-DB arity (ChkTeX 14, done precisely via the tree + DB, not line
  heuristics). Report-only.
- [ ] `verbatim-trailing-text`—text after `\end{verbatim}` on the same line, silently
  dropped (ChkTeX 31). Report-only.
- [x] `unbalanced-left-right`—`\left` with no matching `\right`. **Already covered by
  the parser:** `left_right` recovery emits an `unclosed \left` parse diagnostic on
  every unbalanced-`\left` path (EOF, `}`, `$`, `\]`, `\end`, paragraph break), so a
  dedicated lint rule would only duplicate the existing `parse` finding on the same
  span. No separate rule added.
- [ ] `unreferenced-label`—a `\label` never targeted by any `\ref`-family command,
  project-aware behind the same closed+rooted namespace gate as `undefined-ref`
  (**supersedes the old `unused-label` deferral**, whose open-namespace false-positive
  risk is exactly what that gate handles). Report-only.
- [x] `sectioning-level-jump`—a heading that skips a level (`\section` →
  `\subsubsection`), read from the semantic sectioning tree (textidote sh:secskip).
  Report-only.
- [x] `hard-coded-reference`—literal "Figure 3"/"Section 2"/"Table 1" in prose
  instead of `\ref`/`\cref` (textidote sh:hcfig/hctab/hcsec). Report-only, heuristic.

## Semantic layer & signatures

- [ ] How much of `\newcommand`/`xparse` to model for the signature DB. *(open
  decision)*

## Language server

### Feature status vs texlab

A deliberate diff against **texlab** (5.25.1) as the mature reference LSP. The
comparison is asymmetric, and the framing matters when triaging the items below.

- **Badness leads:** range formatting, on-type formatting, code-action quick-fixes
  (texlab's `codeAction` handler returns an empty array), a native linter (20
  LaTeX + 10 bib rules vs texlab's 4 built-ins plus a `chktex` shell-out),
  comment-run folding, the deterministic rule-based formatter, and a first-class
  bib formatter with entry sorting. texlab also ships **no** semantic tokens,
  signature help, selection ranges, or code lens—so those are not texlab gaps.
- **Badness trails (tracked below):** completion breadth (`### Completion`),
  macro/include navigation and matching-pair highlight (`### Navigation &
  structure`), label preview and macro references/rename
  (`### Labels & references`), package hover, and the change-environment refactor
  (`### Code actions`).
- **Deliberately not matched:** the typeset-adjacent features under
  `## Editor integration` (build, clean-aux, chktex passthrough, CSL rendering).

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
- [x] `workspace/didChangeWatchedFiles` + dynamic `client/registerCapability`
  for `**/*.{tex,bib}` and `badness.toml` so on-disk edits to non-open
  includes/`.bib` files (the project graph's leaves) and the config reanalyze.
  Watchers are registered post-handshake (`register_file_watchers`) when the client
  advertises `didChangeWatchedFiles.dynamicRegistration`; no `notify`-crate fallback
  otherwise. A `.tex`/`.bib` change re-reads + re-upserts the non-open file
  (`Worker::apply_watched_change`, scoped to seeded project dirs) and re-lints via
  `RelintAll`; a `badness.toml` change clears the config cache and re-lints
  (`relint_all_open`). Open buffers are skipped (the editor overlay is authoritative).

### Formatting

- [x] Range formatting (`textDocument/rangeFormatting`)—expand the selection to
  whole top-level blocks (children of `ROOT`), lower only those (a byte-range
  emission filter in `format_node_range_with_signatures`, so the formatter stays
  the sole layout authority and out-of-range blocks are never laid out), then
  diff the fragment against the original slice into minimal edits. LaTeX only;
  bib is a no-op for now.
- [x] On-type formatting (`textDocument/onTypeFormatting`): typing `}` re-indents
  the containing top-level block, but only when the `}` structurally closes a
  multi-line group or an `\end{…}` (a `closes_multiline_construct` guard);
  inline `\textbf{x}` and `\begin{…}` opens are skipped. Reuses the range path
  (`range_edits_for_root` with an empty selection at the cursor). Trigger `}`;
  client opt-in (e.g. `editor.formatOnType`).

### Navigation & structure

- [ ] Selection ranges (`textDocument/selectionRange`)—expand-selection from
  the CST's node hierarchy (group → argument → command → environment). Subsumes
  texlab's `findEnvironments` command: the enclosing-environment stack falls out
  of the CST-hierarchy expansion.
- [x] **Document links (`textDocument/documentLink`).** Clickable include edges:
  `\input`/`\include`/`\subfile`/`\import`/`\subimport`,
  `\usepackage`/`\RequirePackage`/`\documentclass`/`\LoadClass` (→ resolved
  `.sty`/`.cls`/`.dtx`), `\bibliography`/`\addbibresource`, and
  `\includegraphics` (extension guessed against the graphics image types). A
  self-contained single-file, positional walk (`src/lsp/document_link.rs`)
  bypasses the range-free project graph: it takes each command's precise
  argument span via `ast::nth_group_inner` (per comma-separated name) and is
  **disk-aware**—a link is emitted only when the resolved target exists on disk
  (first existing candidate wins for the graphics guess; a `.dtx` fallback
  covers literate `.sty`/`.cls` sources). A project-local `mypkg.sty` resolves
  locally; a system `\usepackage{amsmath}` now falls back to the **TEXMF index**
  (`project::texmf`, see "TEXMF index" below), linking to its installed source
  (local always wins). `\graphicspath` is unsupported (graphics resolve against
  `base_dir` only). texlab covers the include edges only (`crates/links`).
- [~] **Go-to-definition for includes and user macros.** **File targets done:** a
  file-referencing argument under the cursor (`\input`/`\include`/`\subfile`/
  `\import`, `\usepackage`/`\documentclass`, `\bibliography`/`\addbibresource`,
  `\includegraphics`) jumps to the resolved on-disk file. It reuses
  `document_link::document_links` (finding the link whose span covers the cursor), so
  it is disk-aware and TEXMF-aware for free—a system `\usepackage{amsmath}` jumps to
  its installed source (`file_target_under_cursor` in `src/lsp.rs`; no `CursorTarget`
  variant needed, the label/cite path is unchanged). *Still deferred:* a user command
  `\mycmd` jumping to its `\newcommand`/xparse definition (needs the definition *span*
  recorded in the signature DB, `semantic::define`—provenance is tracked, the span is
  not). texlab: `crates/definition` (command/include/label/citation/string_ref).
- [x] **Matching `\begin`/`\end` document highlight.** Highlight the paired
  begin/end of the environment under the cursor (highlight was label-key only
  before); the parser already pairs them structurally. texlab: `crates/highlights`
  (label-only) plus its `findEnvironments` command.
- [x] Workspace symbols (`workspace/symbol`)—the per-file outline (sections,
  labels, floats, theorems, macros, environments) aggregated across every tracked
  project file, filtered by the query string. LaTeX members only.

### Labels & references

- [x] **Label hover.** Hover a `\label`/`\ref`-family key to render a preview:
  kind + nearest heading/caption (`semantic::label_context`, classified at the
  definition site, cross-file like go-to-def) *and* the resolved number from the
  compile's `.aux` (`project::aux`, mirroring texlab's `\newlabel` extraction) —
  `Figure 3: A chart`, `Section 1.2 (Intro)`; degrades to numberless when never
  compiled. The same aux data feeds **document symbols**: section names get their
  toc numbers prefixed (`1.2 Intro`, via `\@writefile{toc}` title matching) and
  labels/floats their numbers as `detail`. LSP-only (AGENTS.md, "LSP environment
  awareness" tier 3); `[build] aux-dir` locates out-of-tree builds. Deferred:
  latexmkrc/`Tectonic.toml` aux-dir auto-detection; eager `**/*.aux` watching
  (numbers refresh on the next request).
- [ ] **References + rename for user macros and environment names.** Extend
  references/rename (label/citation keys only today) to command names (cross-file,
  via the signature-DB provenance in `semantic::define`) and to environment-name
  pairs. texlab: `crates/references` + `crates/rename`
  (command/entry/label/string_def).

### Completion

Badness offers command, environment, label, cite-key, bib field/type, and file
completion (`src/completion.rs`, `src/bib/completion.rs`). texlab's completion
breadth is its biggest lead (`crates/completion/providers/`); the specialized
sources below are missing.

- [x] **Package/class name completion** for `\usepackage{}` / `\documentclass{}`
  (`package_arg` in `src/completion.rs`, `PackageName` context). Three tiers in
  `package_completion_items`: local `.sty`/`.cls` files, then the **installed set**
  from the TEXMF index (`project::texmf`, see below), then the baked all-of-CTAN name
  list (`data/{package,class}_names.txt`, generated by `scripts/gen_package_names.py`
  from the pinned TeX Live tlpdb; names only, ranked namesake/common-first). Every
  item is enriched with the shipped CTAN one-line `desc` as `detail`
  (`data/package_metadata.json`, `semantic::signature::package_metadata`).
- [x] **Glossary/acronym key completion** (`\gls`/`\acrshort`/… ←
  `\newglossaryentry`/`\newacronym`). Definers are scanned into the
  `SemanticModel` (`GlossaryDef` in `semantic::builder`, mirroring label
  discovery—not `semantic::define`, which holds command *signatures*), projected
  through the `file_glossary_keys` firewall, and unioned cross-file by walking
  `ResolvedLabels::namespace_members` in `glossary_completion_items` (`lsp.rs`),
  the cite-completion shape. `\loadglsentries` is an `IncludeKind::GlsEntries`
  edge so a dedicated entries file joins the namespace. Covers base glossaries +
  glossaries-extra (`\newabbreviation`, `\glsxtr*`). *Deferred:* hover,
  goto-definition, and rename for keys (`GlossaryDef` carries the ranges).
- [ ] **Color name + model, TikZ/PGF library completion**—small static datasets
  for `\color`/`\textcolor`/`\definecolor` and
  `\usetikzlibrary`/`\usepgflibrary`.
- [ ] **Argument-value enum completion** for fixed enumerated argument choices—
  needs the signature DB to carry per-argument value enums.
- [x] **Package hover** (`\usepackage{amsmath}` → description). Hovering a
  package/class name inside `\usepackage`/`\RequirePackage`/`\documentclass`/
  `\LoadClass` renders the shipped CTAN one-line description plus a
  `https://ctan.org/pkg/<id>` link (`package_target_at`/`render_package` in
  `src/lsp/hover.rs`, backed by `data/package_metadata.json`). The metadata is the
  static dataset that was missing; unlike texlab's component DB it is tlpdb-derived,
  not a live query. *Deferred:* an "installed at `<path>`" line from the TEXMF index.
- [ ] *(Design decision)* **Package-scoped command completion.** texlab suggests
  only commands provided by the loaded packages (a package→command component
  model). Badness's signature DB is flat (curated + CWL + scanned); scoping
  completion to `\usepackage`-loaded packages needs package→command attribution.
  Open question, not a mechanical add.

### IntelliSense (signature DB)

- [x] Signature help (`textDocument/signatureHelp`)—show the active argument
  while typing a command's `{…}`/`[…]` arguments. (Not a texlab gap—texlab has no
  signature help—but a natural fit for the signature DB.) Triggered by `{`/`[`
  (retriggered by `}`/`]`); the cursor's `GROUP`/`OPTIONAL` is greedily aligned
  against the signature's slots (omitted optionals skipped, extraneous arguments
  suppress rather than mishighlight), rendered as `#n` placeholder labels
  (`\sqrt[#1]{#2}`) since the DB carries no argument names
  (`src/lsp/signature_help.rs`).

### Code actions

- [x] **Change-environment refactor.** Rewrites the `\begin{a}`/`\end{a}` name
  pair around the cursor (the innermost enclosing environment; an unclosed one
  rewrites just its `\begin`) to a new environment. A correctness-only textual
  edit (never invokes the formatter—tenet #1) built from the paired begin/end
  spans the parser already builds; declines when a delimiter name is not a plain
  token run rather than rewrite half a pair. Exposed as the
  `badness.changeEnvironment` execute-command with `texlab.changeEnvironment` as
  a wire-compatible alias (same single `RenameParams`-shaped argument; the edit
  is pushed via `workspace/applyEdit`), so texlab client keybindings work
  unchanged.

### Infrastructure

- [x] Client capability negotiation—gate advertised providers and
  UTF-8/UTF-16 position encoding on what `initialize` reports. The handshake is
  now two-step (`initialize_start`/`initialize_finish`) so `server_capabilities`
  can read the client's params: UTF-8 is advertised and served when
  `general.positionEncodings` offers it (columns become byte distances), else
  the mandatory UTF-16; the pull-diagnostics provider is advertised only to a
  client that reports `textDocument.diagnostic` support. The negotiated
  encoding lives in `text::PositionEncoding`, is carried by every `LineIndex`
  (`with_encoding`; `new` keeps the UTF-16 default for CLI `line_col` use), and
  is threaded from `GlobalState`/`Worker` into every conversion, including
  signature-help label offsets.
- [x] README editor-wiring docs (Neovim/VS Code `initializationOptions`,
  `badness lsp` invocation). Resolved as a README one-liner pointing at
  `docs/src/guide/editor-setup.md`, which carries the full story: Neovim
  `vim.lsp.config` with `init_options`, `lineWidth`/`indentWidth` accepted
  bare or under a `badness` key, file-config-wins precedence, `bib` filetype,
  VS Code via the extension's `badness.*` settings.

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

- [x] **TEXMF index (LSP-only, read-only) + static CTAN metadata.** `project::texmf`
  builds a `filename -> path` index over the installed tree: root discovery is
  *delegated* to `kpsewhich -var-value=TEXMF{HOME,LOCAL,DIST,MAIN}` (heuristic-path
  fallback when absent), enumeration reads each root's `ls-R` or walks it, cached to
  the OS cache dir keyed by a distro fingerprint, held in a lazy process-global
  (first `[texmf]` config wins). It powers three LSP consumers—document links, hover,
  and installed-set completion—for **system** packages, while a local file always
  wins. Paired with the shipped static CTAN metadata DB (`data/package_metadata.json`,
  `semantic::signature::package_metadata`) that supplies descriptions the scan can't
  cheaply derive. Gated by `[texmf]` config (`enabled`/`roots`/`use-kpsewhich`), and
  **never** wired into the formatter's signature scope (guard test
  `formatter_scope_never_reaches_the_texmf_tree`; see AGENTS.md "LSP environment
  awareness"). All three LSP consumers plus **go-to-definition** for a file argument
  (see the go-to-definition item under Navigation) resolve system packages through
  the index. *Deferred:* a MiKTeX `findtexmf` discovery path.
- [x] **Signature extraction from package sources.** The `semantic/define.rs`
  scanner already runs across loaded `.sty`/`.cls` (via `scope_signatures` and its
  db-less CLI mirror `collect_package_signatures`), and already recognizes
  `\DeclareRobustCommand` (`DefKind::Command`) and `\DeclareDocumentCommand` + the
  `\New/Renew/Provide...DocumentCommand/Environment` family (`DefKind::Xparse*`).
  Added: package resolution now falls back to a package's `.dtx` literate source
  when no generated `.sty`/`.cls` is a member (preferring the generated file when
  both exist), in both `PackageGraph::build` and the CLI `collect_loaded`
  (`project::package::dtx_source_of`). The `.dtx` is scanned whole-tree by the
  existing per-file `document_signatures` (its `macrocode` bodies already lex as
  real code). Broadening the definer set beyond the above (`\DeclareMathOperator`,
  etoolbox `\newrobustcmd`/`\csdef`, ...) is deferred.
- [ ] **Package metadata & options (recognize, never execute).**
  `\ProvidesPackage`/`\ProvidesClass` (name/date/version),
  `\NeedsTeXFormat`, `\DeclareOption`/`\ProcessOptions`/`\ExecuteOptions` ---
  surfaced as signatures/metadata for hover/diagnostics, never run.
- [ ] **Package-aware diagnostics.** Once the load graph exists: unknown-option,
  duplicate `\RequirePackage`, missing `\ProvidesPackage`, and resolving
  user-macro definitions to their defining package for hover/go-to-definition.

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
- [ ] `title-capitalization` refinement: the acronym heuristic flags mid-word
  capitals, so CamelCase names (`McDonald`, `DeForest`) are false positives ---
  a curated name-particle allowlist or a smarter word model would tighten it.
- [ ] Shared component-finder: `ResolvedCitations` duplicates the union-find +
  component assignment from `ResolvedLabels` (`project/citations.rs`); factor one
  helper when a third consumer appears.

--------------------------------------------------------------------------------

## Open decisions to revisit

- [ ] How much of `\newcommand`/`xparse` to model. *(Semantics)*
- [ ] Formatter opinionatedness: configurable vs. fixed. *(Formatter)*
- [ ] `.dtx` two-layer model: a preprocessor that splits doc/code layers, or a
  single lexer mode with margin-aware tokens? *(Package infrastructure)*
