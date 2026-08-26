# AGENTS.md

Operational guidance for AI agents working on Badness, a parser, formatter,
linter, and language server for LaTeX.

This file is organized by architectural responsibility, not filesystem path.
Follow the section for the behavior being changed, including when its code lives
in another crate or at a cross-subsystem call site.

## Project architecture

Badness follows a rust-analyzer-style architecture:

- A lossless, error-tolerant parser produces a CST.
- Semantics are layered separately from syntax.
- Salsa provides incremental recomputation.
- The formatter, linter, and language server build on the CST.

Workspace layout:

- `badness` (root crate): CLI, linter, language server, project/configuration,
  and file discovery.
- `badness-parser`: syntax, AST, parser, semantic core, and generated parser
  data.
- `badness-formatter`: LaTeX and BibTeX formatting.
- `badness-wasm`: wasm shim for the documentation playground; it is not
  published.

Keep these boundaries intact:

- Fix parser mistakes in the parser; do not compensate for them in the formatter
  or linter.
- Keep formatter layout deterministic and formatter-owned.
- Put content rewrites in linter fixes, not formatter rules.
- Keep parser and formatter runtime code wasm-clean: do not use the filesystem,
  threads, or child processes there. Code that needs the outside world belongs
  in the root crate; build scripts and tests are outside this boundary.
- Keep the dprint plugin and wasm targets working.

## Development workflow

- Prefer test-driven development.
- Rust edition 2024 is used throughout. Keep the supported Rust 1.89 floor in
  `workspace.package.rust-version`, its per-crate inheritance, and the MSRV CI
  job in lockstep; `rust-toolchain.toml` separately pins the current development
  and release compiler.
- Use `go-task` through `Taskfile.yml` for project tasks.
- Run targeted tests while developing and `cargo fmt` before committing.
- Keep Clippy warning-free. Run `task check` before handing off a substantial
  change; it mirrors CI, including the wasm build.
- Keep fixture line endings aligned with `.gitattributes`, especially on
  Windows.

## Parser

### Core contract

- **Losslessness:** `reconstruct(text) == text` byte-for-byte.
- **Tree purity:** parse shape is a function of source text plus explicit
  project declarations only.
- **Error tolerance:** never abort; recover, make progress on malformed input,
  and degrade unresolved shapes to generic nodes without silent corruption.
- **Reparse equivalence:** a successful incremental reparse produces the same
  green tree and `SyntaxError` vector as a full parse. Every failed proof falls
  back to a full parse. Route every tier through the shared oracle and length
  check; return `None` for an unproved edit, and add a bail instead of weakening
  the oracle.

Use texlab as a differential parse oracle over corpora. It is a reference for
comparison, not a byte target.

### Syntax, semantics, and grouping

- Badness is not a TeX interpreter. Do not execute or generally expand macros.
- Do not implement general `\catcode` evaluation. Support only bounded,
  statically recognizable lexer modes.
- Prefer typed AST wrappers for structural reads where available; keep wrappers
  positional and meaning-free.
- Keep the parser pure with respect to text and declarations. Do not use ambient
  package scope, CWL data, or the signature database to direct attachment.
- The only non-text parser input is explicit declarations from `badness.toml`
  through `Declarations` in `ParseCtx`.
- A declaration names a spelling; it must not force impossible pairing.
- Parser-side semantic facts must be curated or declarative and falsifiable by
  source shape, so that a failed gate can demote them to generic syntax.
- Gate mismatches demote to generic syntax. Do not emit low-confidence parser
  diagnostics for routine macro patterns.
- A shape gate must mirror the parse path it guards. Test both permissive and
  rejecting directions when changing a gate.
- Prefer false negatives over false positives in lexer-mode and gate admission.
- Use greedy grouping where text carries no arity protocol. Do not use signature
  arity to attach parser arguments.
- Keep bracket and optional-argument attachment shape-driven, not
  meaning-driven.
- expl3 argspec-driven grouping is a sanctioned dialect-specific exception;
  retain greedy fallback for underivable heads.

### Cross-subsystem parser contracts

- Keep `name_group` and `peek_end_name` aligned over the complete flat
  environment name. Names may contain punctuation such as `@` and `*`, or span
  lexer tokens at `_`, without changing environment pairing or formatter
  framing.
- Share expl3 toggle-name recognition with the formatter through
  `parser::lexer::expl_toggle`; the formatter owns positional layout gating.
- Environment, conditional, and math pairing must preserve formatter safety and
  must not overpromise closers.
- Environment aliases may come from the self-definition scan and declarations,
  but not cross-file or package inference.

### Incremental reparse

- Build incremental tiers on ordinary `parse` and `lex`: use leaf or node
  splices, or fixed-context fragment parses with explicit locality proofs. Lexer
  checkpoints, token-stream reuse, or grammar restart state require a new
  architecture decision.
- Relex fragments under the base parse's `ParseCtx` and full-file `.dtx` facts.
  Keep the mutable previous-parse side channel out of salsa inputs.
- Decline when effects outside a fragment lack an explicit locality proof. Fix
  debug-oracle divergences by adding a bail.
- Keep the token tier's text-read classification complete and backed by its
  source-scanning test. Guard text reads where they are reachable, including
  reads performed by lexer predicates.
- Relex protected and mode-only bodies with their enclosing delimiters, which
  establish lexer mode. Do not reproduce catcode or capture rules in the
  reparser; admit a splice only when locality and token-sequence checks pass.
- A new tier needs a direct-reparse benchmark that asserts the exact tier, a
  speedup floor, a release full-parse comparison, and a seeded corpus baseline
  with splice-rate floors and exact per-tier tallies. Keep protocol details in
  the architecture documentation and harness.

### Parser validation and data

- New parser features need corpus and snapshot coverage with explicit
  losslessness assertions.
- Regenerate mechanical data with `task cwl:sync`, `task pkg-names:sync`,
  `task bib-fields:sync`, or `task math-symbols:sync`, as appropriate; do not
  edit generated artifacts by hand. `signatures.json`, `colors.json`, and
  `tikz_libraries.json` are curated data and may be edited directly.
- When parser behavior changes, update tests, snapshots, losslessness checks,
  these operational instructions when needed, and the parser rationale in
  `docs/src/development/architecture.md`.

## Formatter

### Core contract

- **Trivia only:** change whitespace, newlines, comments, and `.dtx`
  margin/guard trivia, never non-trivia tokens.
- **Idempotence:** `fmt(fmt(x)) == fmt(x)`.
- **Protected regions:** preserve `verbatim`, `lstlisting`, `\verb`, and
  comments, except configured line-ending normalization.
- CST shape may change during formatting as long as non-trivia content
  invariants hold; parse stability is not an invariant.
- `Mode::Flat` is a verified claim, not a preference. Measure it using the
  correct current-column context, and keep measurement logic centralized.

### Layout and trivia

- Derive layout deterministically from content, configuration, and permitted
  preserved-trivia predicates. Do not add hard-coded content-meaning expansion
  heuristics.
- Never branch on “single newline versus space” for consumed gaps. Permitted
  predicates are blank-line presence, comment presence or own-line status, and
  `.dtx` margin/guard structure.
- Use normalized `Gap` APIs for width paths.
- Keep colon-prefixed relations indivisible in math lowering, including when a
  script makes the equals sign a separate structural atom; coalesce only across
  a trivia-free boundary.
- Intentional newline-shape reads are Tier 2 and require an explicit fixed-point
  argument and tests.
- Keep paragraph-level sectioning commands paragraph-separated in
  `reflow_elements`: emit one blank line before the whole command, including any
  leading bound comment, and after any immediately following `\label` run. Keep
  those labels directly below the heading; do not synthesize `\par` inside nested
  argument or conditional structure.

### Reflow and structural safety

- Make reflow safety structural and gate-driven; never use a file-kind default
  workaround.
- Keep a complete Beamer `<overlay>` prefix glued to a list item's structural
  marker in `item_overlay_marker_suffix`; the ordinary body still hangs from the
  bare `\item ` column.
- `.dtx` margin and guard escapes must fall back safely to preservation, and
  required `macrocode` framing bytes must remain literal.
- Derive structural statement boundaries from parse structure under reflow.
- In `reflow_elements`, make an environment close its line before following
  prose; a trailing comment still rides the closer because moving it changes
  TeX spacing and comment binding.
- In `lower_environment`, keep commands after a completed `BEGIN` header in the
  indented body, even when the author wrote them on the header line.
- In `lower_begin`'s ordinary path, advance declared headers by positional
  signature slots, not attached-node count; skip omitted optionals, but demote a
  delimiter mismatch to ordinary glue boundaries so incomplete signatures
  cannot reclassify text.
- In `lower_begin`, route a declared `ContentKind::Keyval` slot through the
  delimiter-appropriate segmented layout. Keep `tblr`/`longtblr`/`talltblr`
  unmarked as `align`: their required group is a keyval list, not the raw column
  specification that `column_alignments` reads; the structural ampersand router
  still owns their grids.
- In `lower_commented_begin`, keep a declared mandatory argument after a
  trailing header comment in the `BEGIN` header and indent its continuation;
  never add indentation before a following optional argument, where whitespace
  can change argument recognition.
- In `lower_environment`, keep an empty environment's `BEGIN` and `END` on
  separate lines; collapsible body whitespace must not select another layout.
- Keep interior statement wrapping unit-aware and meaning-safe. Underivable
  fallbacks may preserve authored lines but must remain idempotent.
- Anchor a multiline environment used as a math atom at its rendered start
  column with `Ir::align_current`; its closer must not fall back to the enclosing
  display's base indentation.
- In `render_alignment_rows`, pad terminated grid rows to the full grid width so
  their `\\` markers align; leave unterminated rows unpadded.

### Formatter validation and linter boundary

- Run `task typeset:check` when changing keyval signature behavior or optional
  argument lowering. This typeset-safety oracle is not part of default CI.
- `lint --fix` is fix-first; formatting does not run inside fix application.
  Never rely on formatter output to make a potentially unsafe fix safe.

## Linter

### Dispatch and rule declarations

- Do not walk the tree independently per rule. Use shared node-shape
  (`interests()` plus `check()`), whole-file (`check_file()`), or streaming
  (`stream()`) dispatch.
- Reuse shared indexes such as `math_regions` and `conditionals`. Extract an
  index once two rules need the same derived view.
- Treat missing project-resolution context as inert (`None`), not automatically
  wrong.
- Keep each user-facing rule `id` stable and kebab-case; a rename is breaking.
- Ensure `emits_fix` matches behavior because fix-loop scheduling depends on it.
- Keep descriptions and examples non-empty and runnable in documentation
  generation.
- Register rules in every lockstep registry in `rules.rs`.
- Apply `select` and `ignore` as a `RuleSelection` post-filter over shared
  dispatch; keep the driver configuration-agnostic.

### Fixes and suppression

- A fix chooses what to rewrite, not final layout.
- A `Safe` fix preserves parseability and meaning. Withhold autofix for unproved
  shapes, but still report the diagnostic. Unsafe fixes require explicit opt-in.
- Keep fix edits atomic, including cross-file sets.
- Keep `apply_fixes` pure over source, fixes, and flags, with no filesystem
  effects.
- Use `badness_parser::directives`: `% badness-lint <verb> [<rule>]` is the lint
  axis; `% badness <verb>` suppresses lint and formatting over the same region;
  `% badness-format` does not suppress lint.
- Preserve compatibility with retired `% badness-ignore` spellings. BibTeX
  directives use `@comment{...}`.

## CLI, configuration, and project discovery

- Treat CLI flags, output streams, output formats, exit codes, and configuration
  keys as user-facing compatibility surfaces.
- Keep configuration and filesystem discovery centralized in the root crate.
  Runtime parser and formatter APIs receive resolved configuration and
  declarations; they do not discover `badness.toml` or inspect the environment.
- Keep per-command behavior on the shared discovery path instead of
  reimplementing directory walking, excludes, or file-kind dispatch.
- Put project behavior in `badness.toml` and machine-specific settings, such as
  TeX installation and viewer paths, in editor configuration.
- Update the CLI and configuration references, generated command documentation,
  and starter configuration when their public behavior changes.

## Language server

### Boundaries and configuration

- The language server may use local environment data for navigation; the
  formatter and linter remain hermetic.
- Launching a configured PDF viewer for forward search is the sole sanctioned
  outbound effect. Do not run TeX engines or parse `.synctex.gz`.
- Keep the TEXMF index disconnected from formatter signature resolution.
- Keep declaration publishing centralized in dispatcher and request flow.
- Parse `.aux` with its line scanner, not the LaTeX parser.
- Discover TEXMF roots with `kpsewhich -var-value`.

### Live buffers and reparse staging

- Carry text as `Arc<TextBuffer>` through the main loop and jobs. Use its
  `line_index()`; do not rebuild indexes or pass a separate encoding source.
- Use `text_is_current` for staleness checks: pointer-aware first, then content
  fallback.
- Keep text and its line table paired in one `TextBuffer`. Patch initialized
  tables with `LineTable::patch`, leave uninitialized tables lazy, and never
  combine text with a table derived from different bytes.
- Pair every `upsert_file` call with `reparse_stage_edits`: pass the chain from
  `apply_content_changes` when available and `None` otherwise.
- Stage after the upsert, even when it skipped its write, and use the clamped
  offsets actually applied by the splice.
- Keep coalescing on the analysis side of writes. `Worker::run` processes every
  write job in order; only text-free `AnalyzeRequest` coalesces.

### Concurrency, completion, and paths

- Never block read-pool threads while waiting for viewer processes. Spawn them
  directly; do not shell-split a misconfigured executable string.
- Convert paths and URIs only with `uri_to_fs_path` and `path_to_uri`,
  preserving Windows drive handling.

## Documentation and generated files

- Keep architectural rationale and history in
  `docs/src/development/architecture.md`.
- Keep active roadmap and debugging notes in `TODO.md`.
- Keep contributor processes and rule-authoring instructions in
  `CONTRIBUTING.md`.
- `CHANGELOG.md` is generated by versionary; do not edit it by hand.
- Regenerate the linter-rules reference with `task docs:rules` rather than
  editing generated output.
- Regenerate the benchmark page with `task bench` rather than editing its data
  by hand.
- Update user-facing documentation when CLI or configuration behavior changes.
  Run `task docs` when generated documentation or the playground changes.

## Maintaining agent instructions

- Keep this file action-oriented and below the 32 KiB project-instruction limit.
- Do not turn it into a decision log, issue log, tutorial, or substitute for
  architecture documentation.
- If a rule needs extended rationale, state the operational rule briefly here
  and link to the relevant architecture section.
- Edit instructions in place; avoid append-only growth and duplicate rules.
