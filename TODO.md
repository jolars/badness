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

## Formatter

- [x] **Math operator spacing.** A single space around each binary/relation atom
  (`a+2*1^5` -> `a + 2 * 1^5`, `x=-b` -> `x = -b`); unary signs and `^`/`_` scripts
  stay tight; command operators (`\cdot`, `\leq`) join via `math_atom_role`.
  Group bodies normalized (`x^{a+b}` -> `x^{a + b}`). Scientific notation (`1e-5`)
  is a known non-special-cased limitation.

- [ ] `Sentence`/`Semantic` (sembr) wrap modes—both fall back to `Preserve`
  today. *Demoted, much later.*
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

### Tier 1 — typographic & math substitutions (node-shape, clear autofix)

- [x] `ellipsis`—`...` → `\dots` (text) or `\ldots`/`\cdots` by math context
  (ChkTeX 11, lacheck). `Safe` for the unambiguous text `\dots`; `Unsafe` in math
  where `\ldots` vs `\cdots` depends on neighbors.
- [x] `straight-quotes`—ASCII `"` → `` `` ``/`''`; plus quote-direction and mixing
  checks (ChkTeX 18/32-34, lacheck). `Unsafe` (open/close direction inferred from
  context).
- [x] `dash-length`—classify `-`/`--`/`---` by neighbors: number--number → en dash,
  word---word → em dash (ChkTeX 8). `Unsafe`; needs a curated exception list
  (hyphenated compounds, option flags).
- [x] `times-variable`—literal `x` between digits (`640x200`) → `\times` (ChkTeX 29).
  `Unsafe` (intent heuristic).
- [x] `math-operator-name`—bare `sin`/`cos`/`log`/`lim`… in math typeset as italic
  variables → `\sin` etc. (ChkTeX 35). `Unsafe` (changes typeset glyph and operator
  spacing, and a bare `sin` is occasionally a real product—same reasoning as
  `times-variable`, unlike the truly output-identical `\bf`→`\bfseries` swap). Matches an
  operator-name prefix at a word boundary (so `sin(x)` too), only in math mode, and never
  inside a sub/superscript (the `x_{max}` label case).
- [ ] `primitive-command`—raw TeX primitives discouraged in LaTeX: `\over`→`\frac`,
  `\centerline`→`center`/`\centering`, `\eqno`, `\bgroup`… (ChkTeX 41, lacheck).
  Extends the `deprecated-command` family (merge vs. sibling rule: open). Report-only
  where the rewrite restructures arguments (`\over`); `Safe` control-word swaps where a
  1:1 replacement exists.

### Tier 2 — spacing/correctness semantics (not layout)

These edit *significant* whitespace or command shape, so they do not overlap the
formatter (which only normalizes redundant layout whitespace). Verify the overlap
notes when implementing.

- [ ] `swallowed-space`—control word directly followed by a space TeX eats before
  text (`\LaTeX is` renders "LaTeXis") (ChkTeX 1). `Unsafe` (insert `{}` or `\ `).
- [ ] `abbreviation-spacing`—inter-word `\ ` after `e.g.`/`i.e.`/`et al.`, and
  forgotten `\@` before sentence-final punctuation after a capital (`UFO.` → `UFO\@.`)
  unless `\frenchspacing` (ChkTeX 12/13, lacheck, textidote sh:010/011). `Unsafe`.
  *Overlap note:* verify the formatter does not touch this.
- [ ] `space-before-command`—spurious space before `\footnote`/`\index`/`\label`
  where the space is semantically wrong, not layout (ChkTeX 24/42). `Safe` (delete the
  space). *Overlap note:* confirm the formatter preserves this inter-token space; if it
  already strips it, drop this rule.
- [ ] `makeat-macro`—`@` inside a macro name used outside a
  `\makeatletter`/`\makeatother` (or expl3) region (lacheck). badness can decide this
  **exactly** from its letter-mode tracking rather than heuristically. Report-only.

### Tier 3 — structural / semantic / project-layer (curated subset)

Whole-file or cross-file (`check_file`), using the semantic model, signature DB, and
project resolution. Pure prose-opinion textidote rules (title capitalization, caption
period, section length) are skipped as grammar-tool territory.

- [ ] `missing-required-argument`—command invoked with fewer `{…}` groups than its
  signature-DB arity (ChkTeX 14, done precisely via the tree + DB, not line
  heuristics). Report-only.
- [ ] `verbatim-trailing-text`—text after `\end{verbatim}` on the same line, silently
  dropped (ChkTeX 31). Report-only.
- [ ] `unbalanced-left-right`—`\left` with no matching `\right` (complements the
  orientation-only `mismatched-delimiter`); likely surfaceable from the parser's
  `LEFT_RIGHT` recovery. Report-only.
- [ ] `unreferenced-label`—a `\label` never targeted by any `\ref`-family command,
  project-aware behind the same closed+rooted namespace gate as `undefined-ref`
  (**supersedes the old `unused-label` deferral**, whose open-namespace false-positive
  risk is exactly what that gate handles). Report-only.
- [ ] `sectioning-level-jump`—a heading that skips a level (`\section` →
  `\subsubsection`), read from the semantic sectioning tree (textidote sh:secskip).
  Report-only.
- [ ] `hard-coded-reference`—literal "Figure 3"/"Section 2"/"Table 1" in prose
  instead of `\ref`/`\cref` (textidote sh:hcfig/hctab/hcsec). Report-only, heuristic.

### Deferred / out of scope

- Italic-correction cluster (ChkTeX 4/5/6/28)—legacy font-command era, needs brittle
  font-state tracking, low modern value.
- Pure-whitespace items (ChkTeX 30/36/37/39, lacheck double-space, paren spacing)—the
  formatter's job (type A), never lint findings.
- Brace-on-scripts (ChkTeX 25)—already normalized by the formatter.
- ChkTeX 19 (acute-accent-as-quote)—obsolete per ChkTeX's own author.
- ConTeXt `\start…/\stop…` matching (ChkTeX 47/48)—not a badness target.
- Prose-opinion rules (title caps, caption-ends-with-period, section length,
  cite-command-mixing)—grammar-checker territory.
- *(Note only)* a user-defined regex escape hatch (ChkTeX 44) as possible future infra.

## Semantic layer & signatures

- [ ] How much of `\newcommand`/`xparse` to model for the signature DB. *(open
  decision)*

## Language server

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
- [ ] On-type formatting (`textDocument/onTypeFormatting`), e.g. re-indent on
  `}`/`\end{…}` close. *Lower priority; opt-in trigger characters.*

### Navigation & structure

- [ ] Selection ranges (`textDocument/selectionRange`)—expand-selection from
  the CST's node hierarchy (group → argument → command → environment).
- [x] Workspace symbols (`workspace/symbol`)—the per-file outline (sections,
  labels, floats, theorems, macros, environments) aggregated across every tracked
  project file, filtered by the query string. LaTeX members only.

### Labels & references

### IntelliSense (signature DB)

- [ ] Signature help (`textDocument/signatureHelp`)—show the active argument
  while typing a command's `{…}`/`[…]` arguments.

### Code actions

### Infrastructure

- [ ] Client capability negotiation—gate advertised providers and
  UTF-8/UTF-16 position encoding on what `initialize` reports.
- [ ] README editor-wiring docs (Neovim/VS Code `initializationOptions`,
  `badness lsp` invocation).

## Package & class infrastructure (`.sty`/`.cls`/`.dtx`/`.ins`)

The document-level tools are mature; the next frontier is the **package
ecosystem**—class and package sources, and the literate `.dtx` format they
ship in. This is a large, multi-area subproject (parser + formatter + semantic).
It stays inside the AGENTS.md non-goals: bounded, statically-recognizable
patterns only, signatures *extracted, never executed*, no docstrip run, no TeX
engine. Local project files only—a `texmf`/CTAN/`kpsewhich` search is out of
scope (the same boundary the include graph and CWL ingest keep).

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

## BibTeX/BibLaTeX

- [ ] Cross-file `undefined-string`: a `@string` defined in one `.bib` and used
  in another resolves only once a project-level `@string` union exists (today
  single-file-sound, same caveat as `unused-string`).
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
