# AGENTS.md

Guidance for AI agents working with Badness, a formatter, linter, and language
server for LaTeX.

This file is the **rules** you work under: the tenets, the load-bearing
architectural decisions (stated tersely), and the invariants and conventions to
respect. The **why**—worked examples, issue provenance, and the full catalog of
statically-recognized patterns—lives in the book's Development section, which is the
source of truth for design detail:

- Architecture (`docs/src/development/architecture.md`)
- Parser & lexer modes (`docs/src/development/parser.md`)
- Formatter (`docs/src/development/formatter.md`)
- Linter (`docs/src/development/linter.md`)
- LSP & environment awareness (`docs/src/development/lsp.md`)

When a decision below changes, update both this file (the rule) and the relevant
Development page (the detail). Extended roadmap rationale is threaded through TODO.md.

## What this project is

Badness follows **rust-analyzer's** architecture: a generic, error-tolerant,
hand-written parser produces a **lossless concrete syntax tree (CST)**; semantics are
layered on top as a separate concern; recomputation is incremental via salsa. On the
CST sit a **formatter** (`badness format`), a **linter** (diagnostics), and a
**language server** (LSP). (We were also inspired by
[arity](https://github.com/jolars/arity), the same kind of tool for R.)

It is a **single-crate Cargo package** (`badness`, edition 2024), *not* a workspace,
with module folders `parser/`, `formatter/`, `linter/`, `semantic/`, `project/`,
`text/`, `ast/`, `lsp/`, and `bib/` (a parallel BibTeX pipeline—its own lexer, parser,
grammar, AST, formatter, linter, and semantic layer, same invariants). The CLI
processes `.tex`, `.sty`/`.cls`, `.dtx`, `.ins`, and `.bib`; `badness.toml` is
local project config consumed only by the CLI. See the Architecture page for the full
tour.

## Tenets

1. **Deterministic, rule-based formatting.** Layout is decided solely by the
   formatter's rules and the layout engine—the formatter is the **sole authority on
   layout**. Push back against hard-coding special cases. Autofixes are textual edits
   that never invoke the formatter: a fix decides *what* to rewrite, never *how to lay
   it out*, and owes only correctness (the result still parses and is still lossless),
   never line-width. When a fix can't meet that bar for some shape, make it correct by
   construction or withhold it for that shape (still report the finding). The pipeline
   is fix-then-format; don't run the formatter inside `--fix`—and, mirrored, content
   rewrites never run inside `format`: the layout engine changes only trivia (see
   Invariants).
2. **Incremental parsing is first-class.** Parser/CST work must keep the salsa-based
   reparse path (`incremental.rs`) viable.
3. **Parsing is the parser's job.** Never paper over parser mistakes in the formatter,
   and never let parsing logic creep into the formatter. If the formatter hits
   something the parser got wrong, fix it in the parser.
4. **Losslessness is the parser's job.** `reconstruct(text) == text`, always. The
   formatter may assume a lossless CST.

## Core architectural decisions

Load-bearing. If a change pushes against one, raise it explicitly. Each rule below
links to the Development page carrying its full rationale, examples, and provenance.

1. **The parser treats input as generic TeX surface syntax and always produces a
   lossless tree.** It never *requires* resolving macros or catcodes; we do **not**
   implement macro expansion or a TeX evaluator. Anything we cannot statically resolve
   degrades to generic nodes (plus a diagnostic where useful), never a crash or
   corruption. We **do** handle a bounded, growing set of *statically recognizable*
   patterns—letter modes, verbatim, `\left`/`\right` isolation, math environments,
   definition bodies, short verbs, macrocode chunks, `^^A` doc comments, expl3
   regions, char-constant isolation, signatures—as lexer modes or grammar routing, all
   reading static facts only (no macro meaning). The catalog, with examples and issue
   references, is in `parser.md` (§ *Sanctioned lexer modes*). The related expl3 *code
   formatting* is a formatter concern (`formatter.md`).

   - **expl3 toggles: shared name set, formatter-only positional gate.** The lexer and
     the formatter read the *same* fixed toggle *name* set (`parser::lexer::expl_toggle`)
     so a new spelling is recognized in both and they never drift. But only the
     *formatter* additionally requires a toggle to be a **top-level statement**
     (`toggle_is_top_level`: not a `\def`/`\let` definee, not stored inside a
     group/definition body) before it opens a layout-owned region. Rationale (issue #69,
     `l3kernel/expl3.sty`): a toggle spelling TeX never executes is a false positive of
     the static model, and the byte-level losslessness/idempotency oracles cannot catch
     the resulting damage. The lexer keeps the naive name-only model on purpose—mis-lexing
     a name in letter mode only splits CST tokens (lossless, cosmetic), whereas mis-owning
     layout rewrites real space tokens (meaning). So only the higher-stakes side gates.
     Detail in `formatter.md` (§ *Positional gate on layout ownership*).

   - **Environment pairing is shape-gated on brace structure, not a command set.**
     An environment can never outlive the brace group its `\begin` opened in: braces
     are catcode structure, `\begin`/`\end` are only macros, so a `}` closing a group
     opened before the `\begin` always wins. A `\begin` whose `\end` is unreachable
     before that `}` — and, mirrored, an `\end` reached *inside* a group — is a plain
     command with **no diagnostic**, like a gated `$`/`\[` (issue #71). This
     generalizes the curated definition-body set (#45/#55) to the package code it
     cannot name. Two scope limits keep it precise: only a *group* boundary
     suppresses the environment (a `\begin` that just runs out of file still
     diagnoses), and `.dtx` doc-margin lines are exempt from *stranded* braces so
     the doc layer keeps pairing across `macrocode` chunks that leave braces open
     on purpose — an exemption that lifts when the enclosing `{` opened on a
     doc-margin line itself, since that group is the documentation layer's own and
     locally visible (`theorem.dtx`'s `% \def\deflist#1{\begin{list}…}` split
     definition). A `\begin` the gate demotes leaves an orphan `\end` the gate
     made, so that closer is demoted in step rather than unwinding every enclosing
     environment on its way to the root (`amsldoc.tex`'s
     `\lowercase{…\begin{error}{…}}`); the mirror is scoped to demoted names, so a
     genuine typo still reports. Detail in `parser.md` (§ *The environment
     group-boundary gate*).

2. **Two layers: syntactic vs. semantic.** The syntactic CST knows nothing about what
   a command means; the semantic layer is a signature database assigning arity,
   verbatim-ness, and sectioning. **Meaning never leaks into the parser.** See
   `architecture.md`.

3. **Hand-written recursive descent is the spine; Pratt is local to math**
   (sub/superscript binding and `\left…\right` only). Math operator atoms and the
   `$`/`\[`/`\(` shape gates are bounded, sanctioned widenings of this rule, still
   producing no expression tree. See `parser.md`.

4. **The parser emits an event stream, not a tree directly**
   (`Start`/`Tok(idx)`/`Finish`); diagnostics ride a byte-range side channel (no
   `Error` event), and a `SubTok` event attaches `WORD` sub-slices for the math split.
   See `parser.md`.

5. **Errors travel alongside the tree, never abort it.** A single syntactic error
   never fails the whole parse. Recovery anchors: `\end{…}`, `\begin`, blank line,
   `}`, `$`, `&`, `\\`. Always make progress; never infinite-loop. See `parser.md`.

6. **Incrementality is salsa-first.** Cross-file/cross-query incrementality via salsa
   is the v1 story; intra-file reparse is a later optimization. See `parser.md`.

7. **Store green nodes in salsa, never red (`SyntaxNode`).** Red trees aren't
   `Send`/`Eq`/`salsa::Update`; the tree is a pure function of the text, materialized
   to red cursors on demand. See `parser.md`.

8. **Argument grouping is greedy and generic** (texlab-style); arity is refined by the
   semantic layer. **`[…]` attachment is shape-gated**—a bracket is an argument only
   when it reads as one, from static shape facts, never meaning. See `parser.md`.

9. **Trivia attachment follows the rust-analyzer rule:** comments bind *forward* (a
   run of own-line `%` before a `COMMAND`/`ENVIRONMENT` becomes a `DOC_COMMENT`),
   whitespace floats, a blank line breaks the bind. See `parser.md`.

10. **Typed AST wrappers are a read-only view, never a re-model of the tree.** They
    expose structure, never meaning; accessors are positional and tolerate
    over-attachment. The formatter stays raw for structural work, adopting wrappers
    only for field access. See `parser.md`.

The **formatter engine** (Wadler-style `Doc` IR, `WrapMode`, `MathWrap`, table
alignment, expl3 layout) is documented in `formatter.md`; the **linter** (Rule trait,
autofix model, registration) in `linter.md`; the **LSP's** sanctioned environment
awareness in `lsp.md`.

## Invariants (test oracles—enforce them)

- **Losslessness:** `reconstruct(text) == text`, byte-for-byte.
- **Idempotence:** `fmt(fmt(x)) == fmt(x)`.
- **Whitespace-only formatter:** the formatter changes only *trivia* (whitespace,
  newlines, comments, `.dtx` margins/guards); it never inserts, deletes, or rewrites a
  non-trivia token. Content normalizations (stripping redundant single-token script
  braces `x^{2}` → `x^2`, `$$…$$` → `\[…\]`) are *linter autofixes*, never layout. Pinned
  by the non-trivia-content oracle in `assert_format_invariants` (`tests/format.rs`).
- **Protected regions** (`verbatim`, `lstlisting`, `\verb`, comments) are never altered
  by the formatter.

There is deliberately **no parse-stability invariant**: the formatter may still change
CST *shape* (the math operator split re-groups a catcode-12 `WORD`, so `a+2` → `a + 2`
re-lexes into separate atoms), but the whitespace-only invariant above pins the
non-trivia *content* it carries. The formatter is intentionally used to stress the
parser—any formatter ambiguity should surface a parser modeling gap.

**Differential oracle:** use **texlab's parser** as a differential *parse* oracle over
a corpus—skeletonize both trees and compare. It is a reference we measure against,
never match.

## Repo conventions

- Edition 2024; the toolchain is pinned by `rust-toolchain.toml` (single source of
  truth), consumed by `devenv.nix` and honored by CI. A `wasm32-unknown-unknown`
  target is configured.
- **Run `cargo fmt` before committing**—the rustfmt git hook rewrites unformatted
  files and aborts the commit otherwise. `clippy` warnings are errors:
  `cargo clippy --all-targets --all-features -- -D warnings`.
- Task runner is `go-task` (`Taskfile.yml`). Performance is first-class (`perf`,
  `cargo-flamegraph`, `hyperfine`, `cargo-show-asm`, `cargo-llvm-cov` are in the dev
  shell)—benchmark before optimizing, never regress losslessness for speed.
- New parser features need corpus + snapshot tests **and** a losslessness assertion.
- **`CHANGELOG.md` is autogenerated by
  [versionary](https://github.com/jolars/versionary)** from the conventional-commit
  history—never hand-edit it. Write good conventional commit messages instead.
- **Windows CI bites twice:**
  - *Line endings.* The formatter emits **LF** and tests compare bytes against
    checked-in fixtures. When you add a fixture in a new extension under
    `tests/fixtures/**` or `tests/corpus/**`, add a matching `… eol=lf` line to
    `.gitattributes` (the `*_crlf_*`/`*_lf_*` line-ending fixtures are the deliberate
    `-text` exceptions). Never normalize line endings in code to pass a test—fix the
    attribute.
  - *URIs.* Decode LSP URIs to filesystem paths only through
    `uri_to_fs_path`/`path_to_uri` (`lsp.rs`), which strips the `/` before a Windows
    drive letter and keeps the Unix root. Keep `uri_to_fs_path_handles_unix_and_windows`
    green; tests and snapshots must not assume `/` vs `\`.
- **Generated `data/` artifacts.** Several data files are generated from pinned
  upstream sources by `scripts/gen_*.py` and guarded by paired `task …:check`/`:sync`
  targets: `cwl_signatures.json` (`cwl:check`/`:sync`),
  `package_names.txt`+`class_names.txt`+`package_metadata.json` (`pkg-names:check`/`:sync`),
  and `bib_fields.json` (`bib-fields:check`/`:sync`, tracking biblatex's `blx-dm.def`).
  `signatures.json`, `colors.json`, and `tikz_libraries.json` are curated by hand.
  Re-sync generated files via their model/task; don't hand-edit the mechanical facts.

## Working agreements for agents

- Keep the syntactic layer free of semantic knowledge.
- Read/navigate the CST through the typed AST wrappers (decision #10): typed accessors
  and `child`/`children`/`child_token` over raw `children().find(|c| c.kind()==X)`. Add
  a wrapper struct when a node kind gains a field-extraction consumer; keep accessors
  positional and meaning-free.
- Don't add intra-file incremental reparse, macro expansion, or catcode logic beyond
  decision #1 without recording the decision here and on the relevant Development page.
- New salsa **inputs** carrying rarely-changing data (config, package/class metadata)
  must be constructed at `Durability::HIGH`/`MEDIUM`; per-file `text` stays `LOW`
  (salsa's default). Otherwise every keystroke's `LOW`-revision bump invalidates them.
  Detail in `parser.md` (§ *Incrementality*).
- Update TODO.md as phases progress; update this file when a decision changes, and keep
  the matching Development page in sync.
