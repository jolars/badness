# AGENTS.md

Guidance for AI agents working with Badness, a formatter, linter, and
language server for LaTeX.

Badness follows **rust-analyzer's** architecture: rowan CST + event-stream parser
+ salsa + a Wadler-style formatter IR. (We were also inspired by
[arity](https://github.com/jolars/arity), the same kind of tool for R.) Extended
rationale for the decisions below is threaded through TODO.md's roadmap sections.

## What this project is

Badness parses LaTeX into a **lossless concrete syntax tree (CST)** and builds a
**formatter** (`badness format`), a **linter** (diagnostics), and a **language
server** (LSP) on top. The architecture follows **rust-analyzer**: a generic,
error-tolerant, hand-written parser producing a lossless tree; semantics layered on
top as a separate concern; incremental recomputation via salsa.

**Single-crate Cargo package** (`badness`, edition 2024), *not* a workspace. Module
folders: `parser/`, `formatter/`, `linter/`, `semantic/`, `project/`, `text/`, `ast/`,
`lsp/`, and `bib/` (the parallel BibTeX pipeline, below), plus top-level `syntax.rs`,
`incremental.rs`, `config.rs`, `cli.rs`, `completion.rs`, and `file_discovery.rs`.

**Supported inputs.** The CLI processes `.tex`, `.sty`/`.cls`, `.dtx`, `.ins`, and
`.bib` files (directories are walked with the `ignore` crate, honoring `.gitignore`
plus `badness.toml` excludes; see `file_discovery.rs`). The lexer's `LatexFlavor`
(`Document` vs `Package`) picks the starting catcode regime—`.sty`/`.cls`/`.dtx` begin
with `@` already a letter (implicit `\makeatletter`). `.dtx` docstrip surface syntax is
parsed; `.ins` install scripts default to the `Preserve` wrap mode. Each `FileKind`
carries its own default `WrapMode`.

**Parallel BibTeX subsystem (`bib/`).** `.bib` files get their own full pipeline—a
sibling of `parser/` built on the *same* lossless rowan CST + flat event-stream
architecture, but with a distinct grammar, its own `SyntaxKind`/`BibLang` marker, and
its own lexer, parser, `tree_builder`, `ast/`, formatter, linter, semantic layer,
completion, and outline. The same invariants apply (losslessness, idempotence). The bib
CST also has typed AST wrappers (`bib/ast.rs`, decision #10). Note: the `bib.rs`
module-header comment calling the formatter/linter/LSP "later increments" is stale—they
are implemented.

**Configuration (`badness.toml`).** Discovered by an ancestor walk from each input
(`config.rs`); the **CLI is the only consumer**—the library API takes a fully-resolved
`FormatStyle`. Sections include `[format]` (`line-width`, `wrap-target`, `indent-width`,
`wrap`, `lang`, `no-break-abbreviations`) and `[build]` (`aux-dir`). Excludes follow
the Ruff model (`exclude` *replaces* the built-in `DEFAULT_EXCLUDE`; `extend-exclude` is
additive). `wrap` is optional and resolves per file kind when omitted. This keeps the
formatter hermetic (config is local project data, not the environment). TEXMF discovery
is deliberately **not** a section here: where a TeX installation lives is machine state,
not project data, so it arrives via the LSP editor settings (below), never `badness.toml`.

## Tenets

1. **Deterministic, rule-based formatting.** Layout is decided solely by the
   formatter's rules and the layout engine—the formatter is the **sole authority on
   layout**. Push back against hard-coding special cases. Autofixes are textual edits
   that never invoke the formatter: a fix decides *what* to rewrite, never *how to lay
   it out*, and owes only correctness (the result still parses and is still lossless),
   never line-width. When a fix can't meet that bar for some shape, make it correct by
   construction or withhold it for that shape (still report the finding). The pipeline
   is fix-then-format; don't run the formatter inside `--fix`.
2. **Incremental parsing is first-class.** Parser/CST work must keep the salsa-based
   reparse path (`incremental.rs`) viable.
3. **Parsing is the parser's job.** Never paper over parser mistakes in the formatter,
   and never let parsing logic creep into the formatter. If the formatter hits
   something the parser got wrong, fix it in the parser.
4. **Losslessness is the parser's job.** `reconstruct(text) == text`, always. The
   formatter may assume a lossless CST.

## Core architectural decisions

Load-bearing. If a change pushes against one, raise it explicitly. Extended rationale
for the sanctioned lexer modes is threaded through TODO.md's `## Parser` and
`## Formatter` roadmap sections.

1. **The parser treats input as generic TeX surface syntax and always produces a
   lossless tree.** It never *requires* resolving macros or catcodes—in full
   generality that is equivalent to running a TeX engine. We do **not** implement
   macro expansion or a TeX evaluator. Anything we cannot statically resolve degrades
   to generic nodes (plus a diagnostic where useful), never a crash or corruption.

   We **do** handle a bounded, growing set of *statically recognizable* patterns as
   lexer modes or semantic enrichment, all reading static facts only (no macro meaning
   resolved):
   - **Letter modes.** `\makeatletter`/`\makeatother` (`@` is a letter) and
     `\ExplSyntaxOn`/`\ExplSyntaxOff` (expl3: `_` and `:` are letters; also opened by
     `\ProvidesExplPackage`/`Class`/`File`). Independent flags that compose.
   - **Verbatim.** `\verb`/verbatim-like environments and verbatim-argument commands
     capture their opaque body or final argument as a single token, using the
     signature DB (`data/signatures.json`) for argument shape. Built-ins are curated;
     **user-defined** verbatim commands are discovered by the definition scanner
     (`semantic::define`) via a **bounded two-pass parse** (pass 1 fingerprints
     catcode-changing definitions, pass 2 re-lexes with those names). Conservative by
     construction—a false positive suppresses real diagnostics, so prefer false
     negatives.
   - **`\left`/`\right` delimiter isolation.** The following delimiter is emitted as
     its own token; the `LEFT_RIGHT` pair is then built by the parser.
   - **Math environments.** An environment the *built-in* signature DB flags `math`
     (`equation`, `align`, `gather`, matrix, …) has its body parsed in **math mode**,
     wrapped in a `MATH` node exactly as `\[…\]`—so `^`/`_` build `SCRIPTED` nodes, the
     math operator split fires, and `\left…\right` pair. This is a *grammar* decision
     (`parser::grammar::math_environment_body`, gated by
     `parser::lexer::is_math_environment`), needing **no lexer math state**: the
     math-relevant tokens (`&`, `\\`, `^`/`_`, `\left`/`\right` isolation) are already
     emitted regardless of mode; only *which grammar function runs* changes. Reads the
     curated `math` flag only (never CWL/user tiers), mirroring `is_block_environment`/
     `is_verbatim_environment`: a wrong route is a structural change, so it rests on
     curated data, and a user/unknown environment stays in text mode. A blank line
     inside such a body stays trivia in the `MATH` node (no paragraph split); the
     matching `\end` is the terminator.
   - **Signatures.** `\newcommand`/xparse *signatures* are extracted into the semantic
     DB, never executed.

   **expl3 code formatting (formatter-side, sanctioned).** The expl3 letter mode above is
   a *lexer* fact. The matching *whitespace* catcodes—inside an expl3 region (`\ExplSyntaxOn`
   …`\ExplSyntaxOff`, or `\ProvidesExpl*` to EOF) source spaces/tabs are catcode 9 (ignored)
   and `~` is catcode 10 (a literal space)—are a **formatter** concern: since inter-token
   whitespace is provably insignificant, the formatter owns the layout of in-region code
   (indentation + line breaks), **regardless of `WrapMode`**. This is **idempotent by
   construction**: the inserted whitespace is itself catcode-insignificant, so re-lexing the
   output yields the same token sequence and the deterministic layout is a fixed point. It is
   the property the generic "hanging continuation indent" (TODO.md, the flush-B/TikZ problem)
   could not get, supplied here at the catcode level. Region membership is **not** recorded in
   the CST: the lexer's expl3 toggle stays transient, and the formatter recomputes in-region
   byte ranges in a read-only pre-pass (`formatter::core::expl3_regions`) over the same fixed
   toggle set the lexer uses (`parser::lexer::expl_toggle`, shared so the two never drift),
   stored as a `Vec<TextRange>` side channel in `LowerCtx`—the same byte-range pattern as
   parser diagnostics (decision #4). The CST, lexer, events, and tree_builder are untouched, so
   losslessness is unaffected; the reformatted output is a different valid text with the same
   meaning. Statement boundaries follow *source newlines* (the expl3 one-call-per-line
   convention; a multi-token call like `\cs_new:Npn \foo:n #1 {…}` is several sibling CST
   nodes, not one structural unit), and a single inserted space at any preserved token boundary
   keeps re-lexing from merging two tokens.

2. **Two layers: syntactic vs. semantic.** The *syntactic* layer is the generic CST
   and knows nothing about what a command means. The *semantic* layer is a
   **signature database** (built-in table + CWL-style data + `\newcommand`/
   `\newenvironment` scanning) assigning arity, verbatim-ness, and sectioning.
   **Meaning never leaks into the parser** (the verbatim-body exception in
   decision #1 reads static argument-shape data only).

3. **Hand-written recursive descent is the spine; Pratt is local to math.** Use
   precedence-climbing *only* for sub/superscript binding (`^`, `_`) and `\left…\right`
   matching. The text-level parser has no precedence.

   **Math operator atoms (sanctioned).** Arithmetic operators (`+ - * / = < >`) are
   catcode-12 "other" characters, so the catcode-faithful lexer globs them into `WORD`
   runs (`a+2*1` is one token); operator-ness is a *math-semantic* fact assigned after
   catcode lexing, so it is the parser's job, not the lexer's. Inside math mode
   (`math_scripted`, `grammar.rs`) a `WORD` glued around operators is split at operator
   boundaries into flat sibling atoms via a **byte-range split of its text** (not a
   re-lex—no catcode machinery; see `split_math_word`). Only the *trailing* operand
   piece is the scriptable base, so `a+2*1^5` binds `^5` to `1` (matching TeX). This is
   a bounded widening of "Pratt is local to math": operators become atoms so the
   formatter can space them and the display breaker can break long chains—there is **no
   arithmetic-precedence expression tree**. The split rule: `+ - * /` each stand alone
   (so a leading `+`/`-` reads as unary), `= < >` coalesce into one relation piece
   (`<=`), never merging with a sign (`=-` → `=`,`-`). Bare unbraced script arguments
   (`x_i+y`) are left glued (a pre-existing whole-`WORD` script-binding behavior). The
   resulting operator *spacing* is a formatter concern (tenet #1): a single space
   around each binary/relation atom, unary signs and scripts tight.

4. **Parser emits an event stream, not a tree directly.** `lexer → flat token stream →
   parser emits events (Start/Tok(idx)/Finish) → tree_builder re-attaches trivia and
   feeds rowan's GreenNodeBuilder`. Tokens are referenced by index; there is **no
   `Error` event**—diagnostics ride a side channel keyed by byte range (the
   rust-analyzer event-stream pattern). One extra event, `SubTok { idx, start, end }`,
   attaches a `WORD` sub-slice of `tokens[idx]` (the math operator split, decision #3);
   losslessness holds because a token's `SubTok` pieces cover its full byte range
   contiguously.

5. **Errors travel alongside the tree, never abort it.** A single syntactic error
   never fails the whole parse. Recovery anchors: `\end{…}`, `\begin`, blank line, `}`,
   `$`, `&`, `\\`. Always make progress; never infinite-loop on unexpected input.

6. **Incrementality is salsa-first.** Cross-file/cross-query incrementality via salsa
   is the v1 story. Intra-file incremental reparse (reusing green subtrees) is a
   *later optimization*—a whole-file reparse of a typical `.tex` is sub-ms.

7. **Store green nodes in salsa, never red (`SyntaxNode`).** Red trees aren't
   `Send`/`Eq`/`salsa::Update`. See `incremental.rs`: `#[salsa::input]
   SourceFile { text }`, a `parsed_document` query returning `rowan::GreenNode` +
   diagnostics under `no_eq, unsafe(non_update_types)` (sound because the tree is a
   pure function of the text), materializing red cursors on demand.

8. **Argument grouping is greedy and generic.** The CST greedily attaches trailing
   `{…}`/`[…]` groups as argument nodes (texlab-style). Arity is unknown at parse time;
   the semantic layer refines it.

9. **Trivia attachment follows the rust-analyzer rule: comments bind *forward*,
   whitespace floats, blank lines break the bind.** Trivia is never dropped, so the
   only question is which node owns it:
   - **Default: float at the nearest enclosing node**—inter-sibling whitespace and
     newlines stay direct children of the tightest containing block/group, owned by
     neither neighbor.
   - **A contiguous run of own-line `%` comments immediately preceding a `COMMAND` or
     `ENVIRONMENT` binds *leading* into it**, grouped as a `DOC_COMMENT` node.
     "Documentable" is decided purely on node kind—no signature-DB lookup leaks into
     the parser. A same-line trailing comment (`\foo % x`) never binds.
   - **A blank line (`≥2` newlines, the `\par` boundary) breaks the bind:** comments
     past it stay floating. This is a **deliberate divergence** from RA's
     `n_attached_trivias`, which peeks *past* a blank line and keeps attaching when the
     next comment is an outer doc comment (`///`/`//!`). That peek keys on the
     `///`-vs-`//` distinction—a marker of documentation intent that LaTeX's single
     catcode-14 `%` has no equivalent for. Applied to `%` it would wrongly glue a
     license or copyright header into the following command's doc comment, so we only
     bind the maximal blank-line-free suffix. Pinned by `comment_after_blank_line_still_binds`
     (`tests/parser.rs`).

   Whitespace stays a bare leaf token (never wrapped); the bound leading-comment run
   is the one named-node exception. This is a CST-shape convention enforced by tests,
   not a hard oracle.

10. **Typed AST wrappers are a read-only view, never a re-model of the tree.** On top
    of the untyped rowan CST sits a thin typed layer (`ast.rs` + `ast/nodes.rs` +
    `ast/tokens.rs`, and the bib parallel `bib/ast.rs`): rust-analyzer-style `AstNode`/
    `AstToken` traits (`can_cast`/`cast`/`syntax`), an `ast_node!` identity macro (a
    12-line `macro_rules!`, *not* codegen—the accessors are hand-written), and one
    wrapper struct per node kind (`Command`, `Group`, `Optional`, `NameGroup`, `Begin`,
    `End`, `Environment`, `ControlWord`; add more only when a field-extraction consumer
    appears—`Math`/`Scripted`/… stay unwrapped until then). Wrappers expose **structure**
    (a command's name token, its positional argument groups, an environment's
    `\begin`/`\end`), never **meaning**—no signature-DB lookup lives here (composing with
    decision #2). Because the CST is greedy and generic (decision #8), accessors are
    **positional** (`Command::nth_group(n)` filters `GROUP` only, so an `OPTIONAL` never
    shifts brace indexing) and tolerate over-attachment by construction; they never
    pretend arity is fixed (`Command::title()` would be a lie—a `\section` and a
    `\newcommand` share the `COMMAND` shape). Navigation uses the generic helpers
    `child::<N>`/`children::<N>`/`child_token::<T>`, which replace the raw
    `children().find(|c| c.kind()==X)` idiom at *field-extraction* sites.

    The wrappers are read-only, so they can't threaten losslessness or idempotence. The
    **formatter deliberately stays raw** for structural work: the `lower_node`
    `match node.kind()` dispatch and the token-classification loops (trivia walks,
    `L_BRACE`/`R_BRACE` matching) are idiomatic tree-walking that wrappers would only
    obscure—the formatter adopts wrappers *only* for field access (argument/name
    extraction). The pre-wrapper **free functions** (`command_name`, `environment_name`,
    `nth_group_text`, …) remain as thin **kind-agnostic shims** over the wrapper bodies:
    they read whatever relevant child a node has without gating on the node's own kind,
    because callers rely on that latitude (dtx `\begin{macro}{\foo}` reads a `GROUP` off a
    `BEGIN`; an xparse default body handed to `group_inner_source` may be an `OPTIONAL`).
    The typed methods are kind-checked at `cast`; the shims are not.

## Invariants (test oracles—enforce them)

- **Losslessness:** `reconstruct(text) == text`, byte-for-byte.
- **Idempotence:** `fmt(fmt(x)) == fmt(x)`.
- **Protected regions** (`verbatim`, `lstlisting`, `\verb`, comments) are never altered
  by the formatter.

There is deliberately **no parse-stability invariant**: the formatter may *normalize*
structure (e.g. stripping redundant braces around a single-token math script,
`x^{2}` → `x^2`), changing CST shape on purpose. Such rewrites must preserve *meaning*
(carried by fixtures and the corpus) but are not held to structural equality with the
input. The formatter is intentionally used to stress the parser—any formatter
ambiguity should surface a parser modeling gap.

**Differential oracle:** use **texlab's parser** as a differential *parse* oracle over
a corpus—skeletonize both trees and compare. It is a reference we measure against,
never match.

## Technology choices

- **rowan** for the CST; **salsa** for incremental queries;
  **smol_str** for interned token text; **insta** for snapshot tests;
  **annotate-snippets** for diagnostics rendering.
- **LSP:** `lsp-server` + `lsp-types` (rust-analyzer's stack), **not**
  `tower-lsp-server`. salsa cancellation is a synchronous unwind
  (`salsa::Cancelled`) that composes with `lsp-server`'s sync main loop + threadpool
  and fights tower-lsp's async `&self` model. `text/line_index.rs` uses
  `lsp_types::Position`.
- **Formatter engine:** a Wadler/Prettier-style `Doc` IR (`formatter::ir::Ir`), whose
  core variants are `Group`/`Line`/`SoftLine`/`HardLine`/`EmptyLine`/`Indent` plus
  `Ir::Fill` (per-gap greedy break decisions) and `Ir::PreferredFill`
  (source-break-aware global minimum-cost decisions) for paragraph reflow. The enum also
  carries `Align`, `IfBreak`, `ConditionalGroup`(`AllLines`), `Verbatim`, `ColumnZero`,
  `MarginPrefix`, and `Nil`—see `ir.rs` for the authoritative list.
- **Paragraph line breaks** are controlled by a `WrapMode` (`Reflow` default,
  `Minimal`, `Sentence`, `Semantic`/sembr, `Preserve`), modeled on the sibling
  **panache** formatter and mechanized through the `Doc` IR, not a separate line-filler.
  All five are implemented: `Reflow` width-fills, `Minimal` keeps acceptable authored
  breaks while optimizing overflow/underflow/change/displacement/raggedness against
  `[format] wrap-target` (default `line-width - 10`), `Preserve` keeps authored breaks,
  and `Sentence`/`Semantic` split one sentence per line (width ignored) through the
  shared `reflow_elements` engine—each completed prose run is rendered as a `Fill`
  (reflow), a `PreferredFill` (minimal), or as space-joined sentences
  (sentence/semantic). `Semantic` additionally
  ends a line at every authored newline (sembr; no clause detection). Sentence-boundary
  detection is a per-language abbreviation profile (`formatter::sentence`, ported from
  panache) resolved from `[format] lang` + `[format.no-break-abbreviations]` into a
  `SentenceOptions` threaded on `FormatContext`; babel/polyglossia auto-detection is
  deferred. The `\\` line break (with a tightly-bound `*`/`[len]`) is grouped by the
  *parser* into a `LINE_BREAK` node so the formatter sees `\\[2ex]` as one unit.
- **Table column alignment** (`tabular`/`array`) is a formatter concern (layout, so
  the formatter owns it—tenet #1). The `{lcr}` column spec is parsed by
  `formatter::colspec` into per-column `ColAlign`s, reading only the static argument
  text (no macro meaning); it is **conservative**, bailing to all-left on any token it
  does not model (`p`/`m`/`b` count as left, `*{n}{}` expands, `>{}`/`<{}`/`@{}`/`!{}`
  and vertical rules add no column). The grid renderer aligns each cell L/C/R; a
  right/center *last* cell pads on the left only (no trailing whitespace, so
  idempotence holds—padding re-trims on re-parse). A `\multicolumn{n}{spec}{…}` spans
  `n` columns: excluded from single-column widths, aligned within its span by its own
  spec, and left to overflow rather than ballooning narrow data columns. The rule-line
  recognizer (`non_row_line`) tolerates the booktabs `\cmidrule(lr){2-3}` paren trim
  (the `(lr)` `WORD` and detached `{2-3}` group are consumed as part of the rule line),
  and a same-line `\\ \hline` is normalized onto its own passthrough line.
- **CLI:** `clap` + `build.rs` generating man pages, completions, and markdown
  (`clap_mangen`, `clap_complete`, `clapdown`).

## Non-goals

- No general macro expansion, no TeX evaluator, no execution of TeX primitives or
  arbitrary `\def` semantics. (Common `\newcommand`/`\newenvironment`/xparse
  *signatures* may feed the semantic DB—extracted, never executed.)
- No general `\catcode` handling beyond the bounded patterns in decision #1.
- We never typeset.
- **The formatter never reads the environment.** `badness format` output is a pure
  function of the input plus shipped data (curated tables, CWL, the tlpdb-derived
  name lists and CTAN metadata). It resolves only *local* `.sty`/`.cls` next to the
  document (`semantic::load::DiskPackageSource`)—never the installed TEXMF tree—so
  output can't depend on what's installed. This is load-bearing for the deterministic-
  formatting tenet and the idempotence/losslessness oracles.

## LSP environment awareness (sanctioned, LSP-only)

The **formatter** stays hermetic (above), but the **language server** may look past
the document's directory into the installed TeX tree, because navigation is inherently
about the local environment. Two tiers, both read static facts only (no macro meaning,
no typesetting):

1. **Shipped static CTAN metadata** (`data/package_metadata.json`, generated by
   `scripts/gen_package_names.py` from the pinned tlpdb): a stem→`{desc, ctan}` map,
   the same read-only posture as the name lists and CWL. Drives package hover and
   completion detail (`semantic::signature::package_metadata`).
2. **A read-only TEXMF file index** (`project::texmf`): the installed `.sty`/`.cls`/
   `.dtx` files, discovered by *delegating* root discovery to `kpsewhich -var-value`
   (reimplementing kpathsea's `texmf.cnf` resolution is out of scope; MiKTeX doesn't
   use it) and enumerating via `ls-R`/walk. Cached to the OS cache dir keyed by a
   distro fingerprint; a lazy process-global (first config wins). Powers document
   links, go-to-definition, and installed-set completion for system packages.
3. **The compile's `.aux` artifacts** (`project::aux`): a dedicated line-oriented
   scanner (never the LaTeX parser — aux files are written under `\makeatletter`,
   so `\@input`/`\@writefile` would mis-lex) extracting `\newlabel` numbers and
   `\@writefile{toc}` entries, following `\@input` per-chapter chains. Freshness is
   a per-file `(mtime, len)`-keyed process cache — a recompile is picked up on the
   next request, no watcher. Located per label namespace (sibling `.aux`, or
   `[build] aux-dir` for out-of-tree builds; latexmkrc/Tectonic auto-detection
   deferred). Powers label hover (`Figure 3: A chart`) and document-symbol number
   enrichment (`1.2 Intro`; toc titles matched whitespace-normalized, consumed in
   document order). Guarded by `format_never_reads_the_aux_file`
   (`tests/format_packages.rs`).

The distinction the old TODO conflated: a **runtime distro query feeding the
formatter** stays a non-goal (it would break the hermeticism above); a **read-only
index/metadata feeding LSP navigation** is sanctioned. The index is gated by
the `texmf` editor settings (`enabled`/`roots`/`useKpsewhich`, supplied as
`initializationOptions` or via `didChangeConfiguration`—machine config, so it lives in
the editor, not `badness.toml`; `project::texmf::TexmfConfig`) and is **never** wired into
`scope_signatures`/`DiskPackageSource` (guarded by
`formatter_scope_never_reaches_the_texmf_tree`).

## Repo conventions

- Edition 2024; the toolchain is pinned by `rust-toolchain.toml` (single source of
  truth), consumed by `devenv.nix` and honored by CI. A `wasm32-unknown-unknown`
  target is configured.
- **Run `cargo fmt` before committing**—the rustfmt git hook rewrites unformatted
  files and aborts the commit otherwise. `clippy` warnings are errors:
  `cargo clippy --all-targets --all-features -- -D warnings`.
- Task runner is `go-task` (`Taskfile.yml`). Performance is
  first-class (`perf`, `cargo-flamegraph`, `hyperfine`, `cargo-show-asm`,
  `cargo-llvm-cov` are in the dev shell)—benchmark before optimizing, never regress
  losslessness for speed.
- New parser features need corpus + snapshot tests **and** a losslessness assertion.
- **`CHANGELOG.md` is autogenerated by [versionary](https://github.com/jolars/versionary)
  from the conventional-commit history—never hand-edit it.** Write good conventional
  commit messages instead; versionary derives the entries at release time.
- **Windows CI bites twice:**
  - *Line endings.* The formatter emits **LF** and tests compare bytes against
    checked-in fixtures. When you add a fixture in a new extension under
    `tests/fixtures/**` or `tests/corpus/**`, add a matching `… eol=lf` line to
    `.gitattributes` (the `*_crlf_*`/`*_lf_*` line-ending fixtures are the deliberate
    `-text` exceptions). Never normalize line endings in code to pass a test—fix the
    attribute.
  - *URIs.* Decode LSP URIs to filesystem paths only through `uri_to_fs_path`/
    `path_to_uri` (`lsp.rs`), which strips the `/` before a Windows drive letter and
    keeps the Unix root. Keep `uri_to_fs_path_handles_unix_and_windows` green; tests
    and snapshots must not assume `/` vs `\`.
- **Generated `data/` artifacts.** Several data files are generated from pinned
  upstream sources by `scripts/gen_*.py` and guarded by paired `task …:check`/`:sync`
  targets: `cwl_signatures.json` (TeXstudio CWL corpus, `cwl:check`/`:sync`),
  `package_names.txt`+`class_names.txt`+`package_metadata.json` (TeX Live tlpdb,
  `pkg-names:check`/`:sync`), and `bib_fields.json` (below). `signatures.json` (curated
  built-in command signatures), `colors.json`, and `tikz_libraries.json` are curated by
  hand. Re-sync generated files via their model/task; don't hand-edit the mechanical
  facts.
- **Bib field DB** (`data/bib_fields.json`) tracks biblatex's canonical data model
  (`blx-dm.def`). `scripts/gen_bib_fields.py` syncs the mechanical facts (entry-type
  set, field categories, `required` constraints), preserving the hand-curated
  `optional` ordering and classic-BibTeX overlay. `task bib-fields:check`/`:sync`
  report/apply drift after a biblatex bump. Don't hand-edit the mechanical
  facts—change them via the model and re-sync.

## Working agreements for agents

- Keep the syntactic layer free of semantic knowledge.
- Read/navigate the CST through the typed AST wrappers (decision #10): typed accessors
  and `child`/`children`/`child_token` over raw `children().find(|c| c.kind()==X)`. Add a
  wrapper struct when a node kind gains a field-extraction consumer; keep accessors
  positional and meaning-free.
- Don't add intra-file incremental reparse, macro expansion, or catcode logic beyond
  decision #1 without recording the decision here.
- Update TODO.md as phases progress; update this file when a decision changes.
