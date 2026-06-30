# badness—Roadmap

A LaTeX formatter, linter, and language server on a lossless rowan CST,
following **rust-analyzer's** architecture. See `AGENTS.md` for load-bearing
design decisions and invariants.

Single-crate package (not a workspace). Parser and formatter are **intentionally
interleaved**: the formatter is the primary tool for stress-testing the parser.

Status: `[ ]` todo · `[~]` in progress · `[x]` done

--------------------------------------------------------------------------------

## Parser

## Formatter

- [ ] `Sentence`/`Semantic` (sembr) wrap modes—both fall back to `Preserve`
  today. *Demoted, much later.*
- [ ] **Argument content-kind taxonomy.** `prose`/`collapse` are two ad-hoc
  bools on `ArgSpec`; the real model is a per-argument *content kind*
  (opaque, token-list, prose, document-body) the formatter dispatches
  whitespace and break policy on. Generalize once a third case appears. The
  non-determinism fix (`spans_multiple_lines` deciding block-vs-inline from
  incidental source newlines) is sidestepped for collapse-flagged args but
  still governs every *unflagged* multi-line group—revisit when the
  taxonomy lands.
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
- [ ] Join alignment-cell continuation lines (currently triggers the plain-body
  fallback).
- [ ] Column-spec-aware L/C/R cell alignment and `\multicolumn` for the table
  environments (`tabular`/`array` are now grid-aligned, but every column is
  left-aligned regardless of its `{lcr}` spec). Also: `\cmidrule(lr){2-3}`
  paren trim specs (the parenthesized part isn't recognized as part of the
  rule line, so such a line is treated as a cell and the table falls back),
  and the same-line `\\ \hline` form (only own-line rule commands become
  passthrough lines today).

## Linter

- [~] Wire the remaining report-only fixes onto the autofix infra:
  `deprecated-command`'s `\bf → \bfseries` is **done** (a `Safe` control-word swap,
  consumed by `lint --fix` and the new LSP code actions); `obsolete-environment`'s
  `eqnarray → align` is still report-only.
- [~] More stylistic lints. `missing-nonbreaking-space` (a tie before a cite/ref
  command, broad curated family, `\nocite` excluded; `Unsafe` autofix) is **done**.
  Remaining: general typography (quotes, dashes, …). *Follow-up:* the tie lint only
  covers a same-line `WORD WHITESPACE \cmd` shape; a *source line break* before the
  command (`Figure\n\ref{x}`) is also a breakable space but is left for a later pass
  (replacing the newline with `~` reflows the source and overlaps the formatter).
- [ ] `unused-label` (cross-file)—deferred: can false-positive on labels
  referenced from outside the analyzed set.

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
  - No on-disk config watching: the anchor-dir cache lives for the session and is
    cleared only on `didChangeConfiguration`, so editing `badness.toml` needs a
    config-change nudge (or restart) to take effect. Folds into the
    `didChangeWatchedFiles` work below.
  - Deliberately *not* done: plumbing `wrap` (or other knobs) through
    `EditorSettings` itself. A discovered config's `wrap` flows via `FormatConfig`,
    so no new editor knob was needed; `EditorSettings` stays `line_width`/`indent_width`.
- [ ] `workspace/diagnostic` (the workspace-wide pull)—deferred: it is a
  streaming/long-poll protocol (held-open request, per-uri result ids, partial
  results) that fits the one-shot id-bound read-job model poorly. Advertise
  `workspace_diagnostics: true` and add it once that plumbing exists; editors
  drive interactive diagnostics through `textDocument/diagnostic` meanwhile.
- [ ] `workspace/didChangeWatchedFiles` + dynamic `client/registerCapability`
  for `**/*.{tex,bib}` so on-disk edits to non-open includes/`.bib` files (the
  project graph's leaves) reanalyze—the deferred follow-up to LSP project
  assembly (re-read + re-upsert + `RelintAll`).

### Formatting

- [ ] Range formatting (`textDocument/rangeFormatting`)—format the smallest
  enclosing node(s) covering the selection; clamp to node boundaries so a
  partial selection never corrupts the tree.
- [ ] On-type formatting (`textDocument/onTypeFormatting`), e.g. re-indent on
  `}`/`\end{…}` close. *Lower priority; opt-in trigger characters.*

### Navigation & structure

- [ ] Selection ranges (`textDocument/selectionRange`)—expand-selection from
  the CST's node hierarchy (group → argument → command → environment).
- [ ] Workspace symbols (`workspace/symbol`)—labels and sectioning titles
  across the project include graph.

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

- [ ] **expl3 full catcode model (deferred).** Model `~` as a literal space
  (catcode 10) and spaces/tabs as ignored (catcode 9) inside expl3 regions. Formatter
  territory (insignificant-whitespace reflow), beyond the letters-only mechanism above.
- [ ] **expl3 implicit detection in toggle-less `.dtx` (deferred).** Real expl3
  package sources (e.g. `ltx-talk-structure.dtx`) carry no in-file `\ExplSyntaxOn`/
  `\ProvidesExpl*`; expl3 is declared in the parent `.dtx`/build, and `@@` is a
  docstrip module prefix (`%<@@=mod>`). Treat `macrocode` bodies as expl3 when the
  file carries a static expl3 signal (a `%<@@=mod>` guard or `\ProvidesExpl*`
  anywhere). Needs a file-level scan plus the `macrocode` save/restore interaction
  (mirror `at_letter`).

### Formatting

### Semantic and integration

- [ ] **Signature extraction from package sources.** Run the existing
  `semantic/define.rs` scanner across loaded `.sty`/`.cls` (and `macrocode`
  blocks of a `.dtx` when no generated `.sty` is present), extending it to
  `\DeclareRobustCommand`/`\DeclareDocumentCommand` and friends. Prefer a
  generated `.sty` over its `.dtx` source when both exist.
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
- [x] **Profile the formatter (separate startup floor from per-byte cost).** Done:
  in-process micro-bench (`benches/formatting.rs`, `task bench:micro`, split
  parse/format/full + throughput, with a single-doc flamegraph hook
  `task bench:profile`; modeled on panache's `benches/formatting.rs` rather than
  criterion so the flamegraph attaches to one hot doc). Findings (full writeup in
  `benches/README.md`):
  - **Startup floor was the one-time CWL signature-DB init, not binary load—now
    fixed.** A bare `--version` is ~0.8 ms, but the format path's floor was ~4.4 ms:
    `cwl()` decompressed+parsed the embedded `cwl_signatures.json.gz` once (~4.5 ms,
    `LazyLock`) and is on the hot path (`Signatures::command`/`environment` fall
    back to it; the lexer uses it for verbatim-env detection). **Fixed:** the CWL
    tier is now baked into the binary as a build-time `phf` perfect-hash map
    (`build.rs` + `phf_codegen`, values are `const fn` constructor calls;
    `CommandSig`/`EnvironmentSig` args became `Cow<'static,[ArgSpec]>`), so init is
    ~0—no decompress, no parse. CLI `small.tex` dropped ~4.5 ms → ~1.3 ms,
    `cv.tex` ~5.1 ms → ~1.4 ms. Trade-off: larger binary (uncompressed statics) and
    a build-time codegen step; `flate2` dropped, `phf`/`phf_codegen` added. The
    curated `builtin` DB (~0.09 ms) stays a runtime JSON `LazyLock`—negligible.
  - **Per-byte cost is mostly architectural.** masters_dissertation in-process:
    parse ~25 %, lower+print ~70 %, ~10 MB/s. Flamegraph self-time is dominated by
    rowan red-tree cursor traversal (~25–30 %) + allocator churn (~17 %)—inherent
    to the lossless CST + `Doc` IR, by design. Printer itself is ~7 %. Minor slack:
    `lower_node` runs up to four direct-children predicate scans per `ENVIRONMENT`
    (`has_verbatim_body`/`is_margin_framed`/`is_alignment_env`/`is_list_env`) that
    could share one pass. *(Speed-only; no correctness implication.)*
- [ ] Intra-file incremental reparse (reuse green subtrees on contained edits).
- [ ] `wasm32` build for a web playground.

## Tooling & infrastructure

- [x] `badness.toml` configuration (`src/config.rs`). Top-level
  `exclude`/`extend-exclude` (Ruff model: `exclude` replaces the built-in
  `DEFAULT_EXCLUDE`, `extend-exclude` adds on top), `[format]`
  (`line-width`/`indent-width`/`wrap`), and `[lint]` (`select`/`ignore`). Ancestor
  walk stopping at `.git`; `--config`/`--no-config` and additive
  `--exclude`/`--select`/`--ignore` CLI flags; `badness init` scaffolder.
  **CLI-only for now**—the LSP still reads `EditorSettings`, not `badness.toml`
  (see *Configuration & sync* below). No `[index]` section and no `line-ending`
  key (the formatter has no `LineEnding` type yet).
- [ ] `build.rs` man/completions/markdown
  (clap_mangen/\_complete/clap-markdown). **\[copy\]**—the `format`
  subcommand lives in `main.rs`; `build.rs` still deferred.

## BibTeX/BibLaTeX

- [ ] Cross-file `undefined-string`: a `@string` defined in one `.bib` and used
  in another resolves only once a project-level `@string` union exists (today
  single-file-sound, same caveat as `unused-string`).
- [x] Bib-aware LSP completion: `@string` macro names in value position, field
  names per entry type (type-scoped, hiding fields already present), and entry
  types after `@` (`src/bib/completion.rs`); plus `\cite` key completion on the
  `.tex` side, resolved cross-file via `ResolvedCitations` (`src/lsp.rs`
  `cite_completion_items`).
- [ ] Enrich `\cite` completion items: today they carry only `label` (the key) +
  `REFERENCE` kind. Populate `detail`/`documentation` from the resolved entry —
  `entry_type` is already on `Entry` (free), and title/author can be read from the
  entry's fields (`crate::bib::ast`). Purely additive in `cite_completion_items`.
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

--------------------------------------------------------------------------------

## Design notes

Extended rationale for the load-bearing decisions in `AGENTS.md`. These are the
*why* and the edge cases behind already-implemented sanctioned lexer modes; the
crisp rule lives in `AGENTS.md` (core architectural decisions).

### Why the parser stays a generic TeX surface lexer (decision #1)

It never *requires* resolving macros or catcodes to succeed, because in full
generality that is equivalent to running a TeX engine: catcodes are reassignable
at runtime and tokenization is entangled with execution (e.g. `\makeatletter`
changes whether `@` is part of a control word; a `\catcode` inside a conditional
depends on a runtime value). The sanctioned patterns below all read only *static*
facts—"we are inside region X", "the previous control word was Y"—and resolve no
macro meaning.

### expl3 syntax mode

Between `\ExplSyntaxOn` and `\ExplSyntaxOff` (and after a
`\ProvidesExplPackage`/`\ProvidesExplClass`/`\ProvidesExplFile` declaration, which
opens it for the rest of the file), `_` and `:` are catcode-11 *letters*, so expl3
names (`\seq_new:N`, `\__module_internal:nn`) lex as single control words and a
bare `_` is text, not a subscript. It is an independent boolean flag that
*composes* with `\makeatletter` (the `@@` module-prefix convention `\g_@@_x_tl`
needs both), threaded through the lexer exactly like `at_letter`. Scope is
deliberately letters-only: expl3's other catcode changes (`~`→space, spaces/tabs
ignored) and *implicit* detection in toggle-less `.dtx` sources are recorded
follow-ups, not yet modeled.

### `\left`/`\right` delimiter isolation

The single delimiter following `\left`/`\right` is emitted as its own token, so a
word-character delimiter (`(`, `)`, `|`, `/`, `.`, `<`, `>`) does not glue into the
following word run and become un-splittable downstream (the same surface-lexing
problem `\verb` has). The mode reads only "the previous control word was
`\left`/`\right`"; the matched `LEFT_RIGHT` pair is then built by the parser
(decision #3).

### Verbatim environments and commands

For *argument-taking* verbatim environments (`lstlisting`, `minted`, `Verbatim`)
the raw body begins only after the `\begin` arguments, so the lexer consults the
built-in signature DB (`semantic::signature::builtin`) to read each environment's
static arg shape and find where the opaque body starts. This is the single source
of truth (`data/signatures.json`), kept in lockstep with `grammar.rs` via
`is_verbatim_environment`. It reads only static argument-shape data—a recorded
exception to "meaning never leaks into the parser" (decision #2), no macro meaning
resolved. User-defined verbatim environments stay out of scope (their definitions
aren't known until after parsing).

Verbatim-argument *commands* (`\verb` generalized) flagged `verbatim` in the
signature DB (`\verb`, `\lstinline`, `\url`, `\code`, …) have their final argument
captured as a single `VERB` token—a balanced `{…}` group or a `\verb`-style
delimiter run, chosen by the argument's first character, with any leading
non-verbatim args read from the DB's static arg shape (e.g. `\mintinline`'s
language). A curated set of well-known *class*-defined commands is allowed as
built-ins (e.g. jss's `\code`, whose `\@makeother\$` makes `$` literal—a runtime
catcode fact we cannot derive, so we record it as data).

### User-defined verbatim-argument commands via definition scanning

Beyond the curated built-ins, the definition scanner (`semantic::define`) flags an
*arbitrary* command verbatim when its `\newcommand`/xparse/`\def` replacement
**body** reassigns a special char's catcode to "other"—the static fingerprint
`\@makeother`, `\catcode…12`, `\dospecials`, `\@sanitize`, possibly one or more
hops away through a chained helper macro it calls (followed across the scanned
definition set, with a cycle guard). The `\def`/`\edef`/`\gdef`/`\xdef` forms have
no `[n]` arity optional, so their arity is counted from the `#1#2…` **parameter
text** between the name and the body group (`scan_def`/`def_params_and_body`); a
`\def` helper's body is scanned like any other, so chains resolve through it. Only
the command's *own* arity gates it (it must take an argument to capture); the final
argument becomes the implicit verbatim one.

This **reads replacement-body surface text**—a deliberate step past "signatures
only"—but executes nothing, expands nothing, and evaluates no catcode arithmetic;
it matches static substrings. It is **conservative by construction**: a false
positive *suppresses* real diagnostics (the worse failure), so we flag only on a
clear catcode signal and prefer false negatives (e.g. a `\let`-aliased helper, or a
definition visible only after re-tokenization, is not followed).

Because the lexer must know a verbatim command *before* it tokenizes call sites,
but such commands are only discoverable from the parsed tree, `parser::parse` runs
a **bounded two-pass parse**: pass 1 with built-ins only, a definition scan, and—
only when it finds a user verbatim command—pass 2 re-lexing with those names fed
into the lexer (a lexer `pending_def` state keeps a command's own definition site
from being mis-lexed as a call). Two passes is the bound; a definition visible only
after re-tokenization is a tolerated false negative. Reparse cost is paid only when
such a definition exists. `\def`-defined verbatim *environments* and
delimited-parameter `\def` macros stay out of scope.

### Trivia attachment (decision #9), full policy

Trivia (`WHITESPACE`, `NEWLINE`, `COMMENT`) is never dropped—losslessness forces
every trivia token to be a leaf under *some* node—so the only decision is *which*
node owns it. Mirrors rust-analyzer's `n_attached_trivias`.

- The leading-comment bind is exactly ra's rule (comments attach forward to
  item-like nodes). The binding run is the *maximal blank-line-free suffix* of the
  preceding trivia that starts at an own-line comment, so in `%a \n\n %b \foo`,
  `%a` floats and `%b` binds. Mirrors ra's `"\n\n"` cutoff.
- The bound run is grouped into a `DOC_COMMENT` node (the construct's first child)
  so downstream (LSP/formatter) sees the doc comment as one unit; a margin/guard or
  an unbound floating comment is never wrapped. Implemented **grammar-locally**
  (`grammar.rs` `binding_run` + the `precede` idiom, the run wrapped via
  `open(DOC_COMMENT)`/`close`), so `tree_builder` stays a mechanical replay; the
  construct self-opens and its `Start` is pulled back over the `DOC_COMMENT`.
- The doc/ltxdoc *semantic* association of a doc comment with the macro it
  documents in a `.dtx` (where documentation lives behind floating `DOC_MARGIN`
  trivia, not `COMMENT` tokens, so nothing binds) is a deferred semantic-layer
  query, not a parser concern—keeping decision #2's no-meaning-in-the-parser rule
  intact.
