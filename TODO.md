# badness—Roadmap

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
- [ ] Column-spec-aware L/C/R cell alignment and `\multicolumn` for the table
  environments (`tabular`/`array` are now grid-aligned, but every column is
  left-aligned regardless of its `{lcr}` spec). Also: `\cmidrule(lr){2-3}`
  paren trim specs (the parenthesized part isn't recognized as part of the
  rule line, so such a line is treated as a cell and the table falls back),
  and the same-line `\\ \hline` form (only own-line rule commands become
  passthrough lines today).

## Linter

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

### Formatting

### Navigation & structure

- [ ] Selection ranges (`textDocument/selectionRange`)—expand-selection from
  the CST's node hierarchy (group → argument → command → environment). Subsumes
  texlab's `findEnvironments` command: the enclosing-environment stack falls out
  of the CST-hierarchy expansion.

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
- [ ] **Argument-value enum completion** for fixed enumerated argument choices—
  needs the signature DB to carry per-argument value enums.
- [ ] *(Design decision)* **Package-scoped command completion.** texlab suggests
  only commands provided by the loaded packages (a package→command component
  model). Badness's signature DB is flat (curated + CWL + scanned); scoping
  completion to `\usepackage`-loaded packages needs package→command attribution.
  Open question, not a mechanical add.

### IntelliSense (signature DB)

### Code actions

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
