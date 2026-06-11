# badness --- Roadmap

A LaTeX formatter, linter, and language server on a lossless rowan CST,
mirroring **ravel** (`../ravel`, the same tool for R). See `AGENTS.md` for
load-bearing design decisions, invariants, and the copy-from-ravel strategy.

Single-crate package (not a workspace). Parser and formatter are **intentionally
interleaved**: the formatter is the primary tool for stress-testing the parser.

Strategy: **copy ravel's language-agnostic skeleton to bootstrap, extract a
shared crate later** once badness's formatter works and boundaries are proven.
Files marked **[copy]** are lifted \~wholesale from ravel; **[rewrite]** are
LaTeX-specific; **[diverge]** intentionally differs from ravel.

Status: `[ ]` todo · `[~]` in progress · `[x]` done

--------------------------------------------------------------------------------

## Session handoff (resume here)

**Where we are:** Phase 0 ✅, Phase 1 ✅, the **Phase 2 formatter MVP** ✅, the
first three real format rules ✅ --- **whitespace normalization**, **environment
indentation**, and **group/argument indentation** --- Phase 3 (salsa + semantic
+ project graph) ✅, and now the **Phase 4 Minimal LSP MVP** ✅ (`src/lsp.rs`, a
`badness lsp` subcommand, `tests/lsp.rs`): a single-threaded, salsa-backed
`lsp-server` doing full-document formatting + pushed parser diagnostics. The
parser is a lossless, error-tolerant recursive-descent grammar over a rowan CST;
`badness format` parses → lowers to a Wadler IR → prints. The lowering: runs of
`WHITESPACE`/`NEWLINE` trivia collapse to a single break (trailing whitespace
trimmed, 2+ blank lines → one, exactly one final newline); the body of every
`\begin{…} … \end{…}` is indented one step (nesting recursively, `\begin`/`\end`
flush); and the body of a *multi-line* `{…}` (`GROUP`) or `[…]` (`OPTIONAL`) is
indented the same way (delimiters flush, single-line groups left inline,
existing breaks respected). All indentation is computed by the printer
(`Ir::indent`), never preserved from input, so re-indentation is idempotent.
Protected regions (`\verb`, verbatim bodies, comments) are preserved.
**Paragraph reflow now landed** (default): a `WrapMode` (`Reflow`/`Sentence`/
`Semantic`/`Preserve`, modeled on **panache**) selects the intra-paragraph
line-break policy; `Reflow` greedily word-wraps to the line width through a new
Wadler `Ir::Fill` node (per-gap break decisions; the printer stays the layout
authority), `Preserve` keeps authored breaks (the pre-reflow behavior), and
`Sentence`/`Semantic` are scaffolded but fall back to `Preserve`. A `--wrap` CLI
flag selects the mode. The next opinionated step is the `Sentence`/`Semantic`
bodies (port panache's sentence rules / sembr).

**Build & verify** (everything is green as of this commit):

```sh
cargo test          # 46 tests: parser/lexer + printer engine + formatter invariants
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt -- --check
task snapshots      # regenerate insta snapshots (INSTA_UPDATE=always cargo test)

# CLI smoke checks:
printf '\\section{Hi}   \n\n\n\ntext.  ' | cargo run -- format   # → \section{Hi}\n\ntext.\n
printf '\\begin{itemize}\n\\item a\n\\end{itemize}\n' | cargo run -- format  # body indented 2 sp
cargo run -- format --check tests/corpus/*.tex                   # basic/math/edge now report
                                                             # (indentation + edge's final
                                                             # newline) — corpus is a parser
                                                             # fixture set, not pre-formatted
```

**Code map:** - `src/syntax.rs` --- `SyntaxKind` (tokens + nodes) + rowan
`Language` binding. - `src/parser/lexer.rs` --- total lossless lexer; modes:
`\verb`, verbatim envs, `\makeatletter`. - `src/parser/grammar.rs` --- the
recursive-descent grammar (events + errors). Key methods: `parse_block`
(paragraphs), `environment`/`finish_environment` (mismatch recovery), `command`,
`group`, `optional`, `dollar_math`, `delim_math`, `verbatim_body`. -
`src/parser/events.rs` + `tree_builder.rs` --- events → green tree. -
`src/parser/core.rs` --- `parse()` / `reconstruct()` /
`Parse { green, errors }`. - `src/formatter.rs` + `formatter/` --- the
formatter. **[copy]** engine: `ir.rs`, `printer.rs`, `style.rs`, `context.rs`
(lifted \~wholesale from ravel, each marked `EXTRACTION CANDIDATE`).
**[rewrite]** `core.rs` --- `format`/ `format_with_style` + the LaTeX lowering:
`lower_node` dispatches `ENVIRONMENT` to `lower_environment` (body wrapped in
`Ir::indent`, leading/trailing breaks trimmed via
`trim_leading_break`/`trim_trailing_break`, verbatim envs kept on the generic
path via `has_verbatim_body`); everything else goes through
`lower_element_stream` where runs of `WHITESPACE`/`NEWLINE` collapse to one
break (`classify_trivia`: 0 newlines → inline ws kept; 1 → `hard_line`; 2+ →
`empty_line`; indentation dropped --- the printer owns it). A final
unconditional fixup trims the trailing edge to exactly one newline. `check.rs`
--- `--check` over explicit paths (ravel's, minus `file_discovery`). -
`src/main.rs` --- clap CLI:
`badness format [paths] [--check] [--line-width]   [--indent-width] [--wrap]`;
stdin→stdout when no paths. `--wrap` (clap `ValueEnum` `WrapArg`, mapped to the
clap-free `WrapMode`) selects the paragraph line-break policy. - `src/text/line_index.rs` --- byte ↔ line/col (UTF-16) for later
LSP. - `tests/parser.rs` --- tree snapshots + recovery assertions (asserts
losslessness). - `tests/format.rs` --- fixture pairs
(`tests/fixtures/formatter/<name>/{input,   expected}.tex`) + idempotence,
parse-stability (trivia-elided), and losslessness-of-output over the unit cases
and corpus, plus an error-refusal case and a snapshot.

**Next step** --- candidates, pick by priority: - *Formatter:* the
**`Sentence`/`Semantic` wrap modes** (port panache's sentence-boundary rules and
sembr semantic line breaks; both currently fall back to `Preserve`), or **reflow
inside argument groups** (today reflow is `PARAGRAPH`-only; `{…}`/`[…]` bodies
still keep authored breaks). - *LSP follow-up:* a `format_node(tree)` entry so
formatting reuses the cached salsa tree (today `textDocument/formatting` reparses
via `format_with_style`); exposing `--wrap` over LSP / a config file; README
editor-wiring docs. - *Hardening:* the `latexindent` differential formatter
oracle (now more useful with reflow landed).

Each rule is a small diff; use formatter ambiguities to drive parser fixes
(AGENTS.md). The differential oracles --- `latexindent` (formatter) and
texlab/tree-sitter-latex (parse) --- remain available as hardening tracks.

**Decisions recorded:** - *(whitespace)* the final-newline fixup is
*unconditional* --- for any non-empty document the formatter trims the trailing
edge (ASCII ws/newlines only, so trailing Unicode content survives) and appends
exactly one `\n`; empty input stays empty. - *(indentation)* all indentation is
computed by the printer; leading whitespace in the input is dropped (not
preserved), which is what makes re-indentation idempotent. Environment
indentation is **uniform** --- `document` and math environments (`align`,
`equation`) indent like any other; a `document`/per-name opt-out belongs in a
future config, not a special case (Tenet 1). - *(group indentation)* a
`GROUP`/`OPTIONAL` is indented iff it has a **direct** `NEWLINE` token child
(`spans_multiple_lines`), so single-line `{Hi}` stays inline and a newline
inside a *nested* group is attributed to that child --- which keeps
re-indentation idempotent (a reformatted multi-line group still owns its
newline). Existing line breaks are respected (`hard_line`, like environments);
no reflow yet. An empty multi-line group collapses to bare delimiters (`{\n}` →
`{}`). The OPTIONAL opener is captured only once, since a stray `[` inside `[…]`
is body, not a delimiter. - *(resolved)* argument-taking environments
(`\begin{tabular}{cc}`) now keep the declared argument groups on the `\begin`
header line: `lower_begin` queries the signature DB for the environment's arity
and glues that many trailing groups onto `\begin{name}` (dropping any source line
break between them), so a `{cc}` on its own line reflows up to the header.
Environments the DB doesn't know take the generic path unchanged. Verbatim nested
in an environment: `\begin{verbatim}`
indents but the body and `\end` stay column-0 (body is byte-preserved). Both are
lossless and idempotent today. - *(paragraph reflow)* the line-break policy is a
`WrapMode` (`Reflow` default / `Sentence` / `Semantic` / `Preserve`), modeled on
**panache**'s mode taxonomy but mechanized through the Wadler `Doc` IR: a new
`Ir::Fill` node does *per-gap* greedy break decisions (Prettier's `fill`: a
separator stays a space iff `atom + sep + next-atom` fits flat), so the printer
remains the single layout authority --- *not* panache's separate streaming
line-filler. Reflow keys on the `PARAGRAPH` node: adjacent non-whitespace elements
glue into one unbreakable atom (so `word~word` ties and `\emph{x}` never split),
inter-word whitespace/single newline is a break opportunity, and an explicit `\\`,
a `%` comment, or a nested block (forced-break IR) ends the current line and starts
a fresh fill. The `\\` line break is grouped by the *parser* into a `LINE_BREAK`
node (with its tightly-bound `*` / `[len]`, recognized only when directly abutting,
no trivia crossed) --- a formatter ambiguity (`\\[2ex]` orphaning the `[2ex]`)
driven back into the parser per tenet 3, so the formatter keeps `\\[2ex]`/`\\*`
intact instead of stranding the modifier on the next line. `Preserve` is exactly the pre-reflow generic path;
`Sentence`/`Semantic` fall back to it for now. Width uses `chars().count()` (matches
the existing printer; not Unicode display width --- a follow-up). Idempotence quirk
fixed in passing: a trailing `\`-at-EOL is lexed as a control symbol that absorbs
the newline; reflow emits the pre-newline part as a flat atom and lets the line
break supply the newline, so it reparses identically.

Parser-adjacent ambiguities to watch (no parser change needed now): (1)
indentation after a newline lives in the *same* trivia run as the newline ---
the run classifier, not the parser, splits them; (2) a `COMMENT` breaks a trivia
run, so blank-line collapsing around comments is a future paragraph/semantic
concern, not a formatter hack.

**Known deferred (not blockers, all lossless today):** arg-taking verbatim envs
(`lstlisting`/`minted`/`Verbatim`); block-vs-inline paragraph refinement (a lone
block env is wrapped in a `PARAGRAPH`); structured math (Phase 3); `build.rs`
man/completions and directory-walking file discovery for `format`. See the Phase 1
follow-ups list below.

--------------------------------------------------------------------------------

## Phase 0 --- Foundations ✅

Bootstrap milestone --- complete. The two umbrella items below are scoped to
what bootstrap actually required; the rest of ravel's module/dep list is created
by the phase that first needs it (`incremental.rs` + salsa → Phase 4, `lsp.rs` +
lsp-server/lsp-types → Phase 4.5, `linter/` + annotate-snippets → Phase 5).

- [x] Module layout bootstrapped: `parser/`, `formatter/`, `text/`, `syntax.rs`.
      (`linter/`, `semantic/`, `project/`, `incremental.rs` come with their
      phases; the CLI currently lives in `main.rs`, not a separate `cli.rs`.)
- [x] Core `Cargo.toml` deps in place: rowan 0.16, smol_str, insta, clap.
      (salsa, annotate-snippets, **lsp-server + lsp-types** *(not
      tower-lsp-server)*, and the clap build-deps land with the phases that need
      them.)
- [x] `syntax.rs`: `SyntaxKind` (token + node kinds) + rowan `Language` impl.
      **[rewrite]**
- [x] `text/line_index.rs`: byte ↔ (line, col) / UTF-16. **[copy]** (swap
      `Position` type)
- [x] `parser/events.rs` (`Start`/`Tok(idx)`/`Finish`) + `tree_builder.rs`.
      **[copy]**
- [x] Lossless lexer skeleton; trivia (whitespace, comments, blank lines)
      preserved but separable. **[rewrite]**
- [x] Round-trip harness: `reconstruct(text) == text`, byte-for-byte.
- [x] `insta` snapshot scaffolding + initial `.tex` corpus.
- [x] `Taskfile.yml` mirroring ravel's targets (build, test, fmt, lint, bench).

## Phase 1 --- Core parser

- [ ] Event-stream recursive-descent parser → green tree via `tree_builder`.
- [x] Diagnostics on a side channel by byte range (no `Error` event), carried
      alongside the tree (`Parse { green, errors }`, `parser/grammar.rs`).
- [x] Grammar coverage:
      - [x] Text runs grouped into `PARAGRAPH` nodes delimited by blank lines
            (`parse_block` / `trivia_run_is_separator`).
      - [x] Control sequences (`\foo` → `COMMAND`, control symbols as tokens);
            `\makeatletter`/`\makeatother` letter-mode in the lexer.
      - [x] Groups `{ … }` with unbalanced-brace recovery.
      - [x] Comments (`% …` to end of line) --- handled in the lexer.
      - [x] Environments `\begin{name} … \end{name}`; mismatch recovery unwinds
            the implicit stack with one diagnostic per unclosed env.
      - [x] Generic greedy argument grouping: trailing `{…}` → `GROUP`, `[…]` →
            `OPTIONAL`, stopping at a paragraph break.
      - [x] Inline `$ … $`, display `$$ … $$`, `\[ … \]`, `\( … \)`.
      - [x] `~` ties, `\\`, `&`, `^`, `_`, `#` as distinct tokens.
      - [x] `\verb`/`\verb*` (one `VERB` token) and verbatim-like environments
            (`verbatim`, `verbatim*` → one `VERBATIM_BODY` token) as lexer
            modes. *Argument-taking verbatims (`lstlisting`/`minted`/`Verbatim`)
            deferred --- need signature-aware arg handling.*
- [x] Recovery anchors: `\end`, `\begin`, blank line, `}`, `]`, `$`, EOF.
- [x] Progress guarantee: every grammar loop bumps ≥1 token or breaks
      (`debug_assert` in `bump`; `pos` only advances there).
- [x] **Enforce losslessness** --- asserted per-case in `tests/parser.rs` and
      over the corpus in `tests/roundtrip.rs`.
- [x] Differential parse oracle: cross-check against **texlab** over a corpus
      (ravel's `air_parser_harness` analog). Two layers, both in `tests/`:
      `parse_oracle.rs` --- hard acceptance gate (badness-clean ⟹ texlab no
      `ERROR`); `parse_compat.rs` (`task parse-compat`, `#[ignore]`d) --- soft
      structural- concordance gauge that projects both rowan CSTs onto one
      coarse skeleton (`tests/parse_skeleton/`) and writes `PARSE_COMPAT.md`.
      Picked texlab over tree-sitter-latex: the latter has no working pure-cargo
      packaging (crates.io `0.1.0` omits `scanner.c`; git lacks the generated
      `parser.c`). *Open follow-ups: tree-sitter-latex as a second oracle; a
      larger / external corpus (env-var `BADNESS_PARSE_CORPUS`); growing the
      projector's name-extraction as the corpus exercises more node kinds.*

**Phase 1 follow-ups:** - [x] `PARAGRAPH` node grouping over
blank-line-delimited runs. - [x] `\makeatletter`/`\makeatother` letter-mode in
the lexer (Core decision #1). - [x] Verbatim lexer mode for `\verb` and
verbatim-like environments. - \[ \] Argument-taking verbatim envs
(`lstlisting`/`minted`/`Verbatim`) --- needs the signature DB to know where the
raw body starts. - \[ \] Structured math model (scripts/delimiters) ---
currently flat tokens (Phase 3). - \[ \] Block-vs-inline refinement: a lone
block environment is currently wrapped in a `PARAGRAPH`; the signature DB can
later avoid that.

## Phase 2 --- CLI + formatter MVP (interleaved with Phase 1)

- [~] `cli.rs` + `build.rs` (man/completions/markdown via
  clap_mangen/\_complete/ clap-markdown). **[copy]** --- clap `format` subcommand
  lives in `src/main.rs`; `build.rs` man/completions still deferred.
- [x] `badness format`: parse → re-emit; first milestone is identity (round-trip).
- [x] `formatter/ir.rs` + `printer.rs`: Wadler IR + layout engine. **[copy]**
      (extract first)
- [~] LaTeX format rules: **whitespace normalization done** (trailing-ws trim,
  blank-line collapse, single final newline), **environment indentation done**
  (printer-owned, idempotent re-indent, verbatim-protected), **group/argument
  indentation done** (multi-line `{…}`/`[…]` bodies indented one step, single-line
  groups left inline), and **paragraph reflow done** (default `WrapMode::Reflow`:
  greedy word-wrap to the line width via a new `Ir::Fill` engine node; `Preserve`
  keeps authored breaks; `Sentence`/`Semantic` scaffolded, fall back to `Preserve`).
  **[rewrite]** --- replaced the identity `lower_node`.
- [x] Protected regions never touched (`verbatim`, `\verb`, comments) ---
      verified by the `protected_verbatim` / `protected_comment_trailing_space`
      fixtures now that rules touch surrounding text. (`lstlisting`/arg-taking
      verbatims still deferred.)
- [x] **Invariants:** idempotence `fmt(fmt(x)) == fmt(x)`; stability
      `parse(fmt(x)) ≅       parse(x)` (trivia-elided); losslessness of
      formatted output --- asserted per fixture and over the unit/corpus cases
      in `tests/format.rs`.
- [ ] Use formatter ambiguities to drive parser fixes.

## Phase 3 --- Salsa + semantic layer

- [x] `incremental.rs`: `#[salsa::input] SourceFile { text }`, `parsed_document`
      query storing `GreenNode` (`no_eq, unsafe(non_update_types)`). **[copy]**
- [x] `semantic_model` tracked query; linter/LSP reuse it (no re-parse from
      text). **[rewrite]** Per-file label/reference def-use model
      (`src/semantic/`): one CST walk collects `\label` defs + the
      reference-command family (`\ref`/`\pageref`/
      `\eqref`/`\autoref`/`\nameref`/`\cref`/`\Cref`/`\vref`/`\Vref`/`\cpageref`),
      then a flat name-match resolve marks defs `referenced` / refs `resolved`.
      The query is `returns(ref)` **without** `no_eq` (`SemanticModel: Eq`), so
      it backdates on a model-preserving edit. Tested in `src/semantic.rs`
      (builder) and `tests/semantic.rs` (memoization + value stability).
- [x] Signature DB (analog of ravel `rindex/`): built-in command/environment
      table. **[rewrite]** — `semantic/signature.rs`: a `LazyLock<SignatureDb>`
      deserialized from one curated JSON file (`data/signatures.json`,
      `include_str!`-ed, serde) that co-locates *all* metadata per name —
      argument shapes plus sectioning level / verbatim-ness / math-ness. This is
      the hand-maintained high-precision tier (the analog of ravel's
      `PackageIndex` schema). Lower-precision sources layer underneath later
      (ravel's `installed > base > bundled`): the TeXstudio/Kile **CWL corpus**
      (ingested *into* this schema by a converter — CWL is an import format, never
      the source of truth) and per-file `\newcommand` scanning. First consumer:
      the formatter glues an environment's declared argument groups onto the
      `\begin` header line (closes the `\begin{tabular}{cc}` gap below).
- [ ] `\newcommand`/`\newenvironment`/`xparse` signature scanning (signatures
      only, no execution).
- [x] Project graph: `\input` / `\include` / `\import` resolution. **[rewrite]**
      Purely-syntactic include extraction (`project/include.rs`) --- `\input`,
      `\include`, `\import`/`\subimport`, `\subfile`; literal brace-group
      targets with `.tex` defaulting + base-dir joining, non-literal/missing →
      `Dynamic`. Salsa firewall `include_edges` (range-free, backdates) feeds
      the interned `Project` → `project_graph` query building `IncludeGraph`
      (resolved edges, reverse map, unresolved, reachability, cycle detection).
      Tested in `src/project/` (extraction + pure graph) and `tests/project.rs`
      (firewall).
- [x] Label/reference model (`\label` / `\ref` / `\cref`). Landed as the first
      tenant of `semantic_model` (above).

**Phase 3 decisions / follow-ups (semantic model / label-ref):** - *(flat, not
scoped)* LaTeX labels are one document/project-**global** namespace, so the
model is a flat `Vec<LabelDef>` + `Vec<LabelRef>` resolved by name --- **no
scope tree** (contrast ravel's `semantic/scope.rs`, which lexically scopes R
bindings). We mirror ravel's *shape* (Vec + newtype ids + build/resolve) but
adapt the semantics. - *(ast.rs extracted)* `command_name` / `nth_group_text`
moved from `project/include.rs` into `src/ast.rs` (generic, purely-syntactic CST
accessors) now that the semantic builder is their second consumer --- the
extraction TODO flagged below. Both `project/` and `semantic/` build on them. -
*(known limitations)* `\label{\foo}` (nested-macro key) → no def (conservative,
like an unresolvable include); `\cref{a,b,c}` splits into per-key refs that
share the command range (per-key sub-ranges deferred to go-to-def in Phase 7). -
*(per-file only / no consumer yet)* resolution is within one file ---
`unreferenced_labels`/`unresolved_refs` are *facts*, not lints: a label
referenced from an `\input`-ed file looks unreferenced here. Cross-file
resolution (a `file_labels` firewall → project-level `resolved_labels`, ravel's
`visible_symbols` analog) and the duplicate-label / undefined-ref diagnostics
are deferred; the signature DB and `\newcommand` scanning the model will later
consume are deferred too. The model lands "harness + model only," like
`incremental.rs` and the project graph did --- and its `Eq`-backdating becomes
*observable* once that cross-file resolver consumes it.

**Phase 3 decisions / follow-ups (project graph):** - *(ordering)* Include
extraction is **purely syntactic** (reads the generic CST, no
`semantic_model`/signature DB), so it landed ahead of those items --- consistent
with AGENTS.md decision #2 (meaning never leaks into the syntactic layer). -
*(out of scope)* `\includegraphics`, `\graphicspath`, `\bibliography`/
`\addbibresource`, `\usepackage`/`\RequirePackage`, `\documentclass` ---
non-`.tex` assets / packages, not source includes. - *(known limitations, all
conservative)* bare plain-TeX `\input foo` (no braces) → `Dynamic` (the greedy
arg grammar only attaches `{…}`/`[…]`); `\include`'s main-document-relative base
dir and `\includeonly` filtering deferred (we resolve `\include` like `\input`,
but keep it a distinct `IncludeKind`); cycle **diagnostics** deferred to the
linter (the graph only *exposes* `cycles()`). - *(no consumer yet)*
`project_graph` passes `root: None`, so reachability is left to a future caller
of `IncludeGraph::build` that designates the main document. (The "no `ast.rs`
yet" note here is now resolved --- see the semantic-model follow-ups above.) No
`visible_symbols` analog --- graph lands "harness + graph only," like
`incremental.rs` did.

## Phase 4 --- Minimal LSP (editor integration) ✅

**Goal: get badness into an editor as soon as salsa lands** --- a thin server
doing just formatting + diagnostics, deferring the rich features to Phase 7.
Rides the `parse_diagnostics`/`parsed_document` salsa query; precedes the linter
because its diagnostics are the parser's existing byte-range errors, no lints
required.

Landed as `src/lsp.rs` (\~250 lines, `pub mod lsp`), plus a `badness lsp` CLI
subcommand and a `tests/lsp.rs` stdio smoke test. **Single-threaded,
salsa-backed** (per the recorded decisions below).

- [x] Add `lsp-server` + `lsp-types` (+ `serde_json`) deps (rust-analyzer's
      stack, **not** tower-lsp-server --- see AGENTS.md LSP note).
      **[diverge from ravel]** `lsp-server 0.7`, `lsp-types 0.97`,
      `serde_json 1`.
- [x] `lsp.rs`: **single-threaded** sync main loop owning one
      `IncrementalDatabase` by value (`serve(Connection)` split out from `run()`
      so tests drive it over `Connection::memory()`). **[diverge from ravel]**
      *(The ra-style writer/threadpool + `salsa::Cancelled` model is deferred to
      Phase 7 --- a whole-file reparse is sub-ms, AGENTS.md #6.)*
- [x] Lifecycle: `initialize` (advertises `documentFormattingProvider` + FULL
      sync) / `initialized` / `shutdown` / `exit` (via
      `Connection::handle_shutdown`).
- [x] Document sync: `didOpen` / `didChange` (full sync) / `didClose` →
      `upsert_file` into salsa, keyed by the **URI string**
      (`PathBuf::from(uri.as_str())`).
- [x] `textDocument/formatting`: full-document single replacing `TextEdit`,
      backed by `format_with_style`; honors client `tab_size` → `indent_width`;
      replies `null` on no-op / unknown doc / format refusal (parse errors).
- [x] `publishDiagnostics`: maps the parser's byte-range errors to LSP ranges
      via `text/line_index.rs` (`utf16_position`, already UTF-16-aware);
      `didClose` publishes an empty list to clear squiggles.
- [x] Smoke test (`tests/lsp.rs`): drives `initialize`→`initialized`→`didOpen`
      (parse error → diagnostics)→`didChange` (valid → diagnostics
      clear)→`formatting` (edit == formatter output)→`shutdown`→`exit` over
      `Connection::memory()`.

**Phase 4 decisions / deferred:** - *(single-threaded + salsa-backed)* chosen
over the threadpool model; salsa still backs the **diagnostics** path.
Formatting calls `format_with_style(&str)`, which reparses internally ---
badness has no public `format_node(tree)` entry yet (ravel does), so the cached
green tree is not reused on the format path. Adding a
`format_tree`/`format_node` entry is a clean, optional follow-up. - *(URI as
salsa key)* sidesteps URI↔filesystem-path conversion (`lsp-types`' `Uri` has no
`to_file_path`). Text always comes from `didOpen`/`didChange`, never disk; real
path resolution (for cross-file `\input` features) is a Phase 7 concern. -
*(deferred to Phase 7)* writer/threadpool + `salsa::Cancelled` cancellation,
incremental `didChange` sync, client-config `line_width`, a `didClose`
salsa-eviction API, range formatting, symbols, folding, hover, completion,
definition/rename. README editor-wiring docs still to write.

*Deferred to Phase 7:* range formatting, symbols, folding, hover, completion,
definition/rename.

## Phase 5 --- Math

- [ ] Structured math model over the generic math tree.
- [ ] Precedence-climbing for `^` / `_` binding and primes (the one Pratt site).
- [ ] `\left … \right` delimiter matching.
- [ ] Alignment-aware formatting: `align`, `matrix`/`pmatrix`, `&` columns, `\\`
      rows.

## Phase 6 --- Linter

- [x] Diagnostics framework over CST + semantics (reuse parse error channel).
      `badness lint` surfaces parse diagnostics; `linter/{diagnostic,render}`.
- [ ] `linter/suppression` (`% badness-ignore` style) + annotate-snippets
      render. **[copy shape]** — annotate-snippets render done; suppression TODO.
- [ ] Lints: unmatched delimiters, undefined/duplicate refs, deprecated
      commands, stylistic checks.
- [ ] Autofix infra; enforce "autofixes never introduce formatting errors"
      (Tenet 5).


## Phase 7 --- Full LSP

Builds on the minimal server (Phase 4.5); adds the semantics-backed features.

- [ ] Range formatting (`textDocument/rangeFormatting`).
- [ ] Linter diagnostics (Phase 5) published alongside parse errors.
- [ ] Document symbols, folding ranges.
- [ ] Hover + completion from the signature DB.
- [ ] Go-to-definition / rename for labels and refs.
- [ ] Incremental (`didChange`) document sync, replacing full sync.

## Phase 8 --- Performance & hardening

- [ ] Extract shared crate(s) from the **[copy]** files (IR engine first),
      depended on by both badness and ravel.
- [ ] Intra-file incremental reparse (reuse green subtrees on contained edits).
- [ ] Fuzzing (losslessness must hold on arbitrary input).
- [ ] Large-doc benchmarks (`hyperfine`, criterion); flamegraph hot paths.
- [ ] `wasm32` build for a web playground.

## Phase 9 --- BibTeX / BibLaTeX parsing, linting, formatting, and LSP support.

- [ ] BibTeX / BibLaTeX parser (probably a separate `bib.rs` module, maybe a
      separate crate if it's big enough).
- [ ] Formatter rules for BibTeX / BibLaTeX entries.
- [ ] Linter rules for BibTeX / BibLaTeX entries (e.g missing required fields, invalid field values).
- [ ] LSP support for BibTeX / BibLaTeX files (e.g `textDocument/formatting`, diagnostics, hover, etc).
- [ ] Salsa incremental parsing and semantic model for BibTeX / BibLaTeX files,
      integrated with the main LaTeX project graph (e.g. to resolve `\bibliography`
      references).

--------------------------------------------------------------------------------

## Open questions / decisions to revisit

- [ ] Trivia-attachment policy (leading vs. trailing) --- pick one, document it.
- [ ] How much of `\newcommand` / `xparse` to model for the signature DB.
- [ ] Formatter opinionatedness: which choices are configurable vs. fixed.
- [~] CWL data sourcing/licensing for the built-in signature DB. Decided: the
      built-in DB is a hand-maintained `data/signatures.json` (our own granular
      schema), *not* CWL — so no external files and no licensing question now. The
      CWL corpus stays a future *ingest* source, converted into this schema when
      ecosystem-wide breadth (e.g. LSP completion) needs it; licensing is
      revisited only if/when that corpus is vendored.
- [ ] Whether ravel should also migrate tower-lsp-server → lsp-server (separate
      decision; out of scope for badness, but the rationale in `AGENTS.md`
      applies).
