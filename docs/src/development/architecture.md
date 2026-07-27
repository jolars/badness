# Architecture

Badness parses LaTeX into a **lossless concrete syntax tree (CST)** and builds a
**formatter** (`badness format`), a **linter** (diagnostics), and a **language
server** (LSP) on top. The architecture follows
[rust-analyzer](https://rust-analyzer.github.io/): a generic, error-tolerant,
hand-written parser produces a lossless tree; semantics are layered on top as a
separate concern; recomputation is incremental via [salsa](https://github.com/salsa-rs/salsa).
(We were also inspired by [arity](https://github.com/jolars/arity), the same kind
of tool for R.)

This page is the orientation map. The parser internals, the formatter internals,
and the LSP's environment awareness each have their own page:

- [Parser & Lexer Modes](parser.md)
- [Formatter](formatter.md)
- [LSP & Environment Awareness](lsp.md)

## What the project is

**A single-crate Cargo package** (`badness`, edition 2024), *not* a workspace.
The source is organized into module folders: `parser/`, `formatter/`, `linter/`,
`semantic/`, `project/`, `text/`, `ast/`, `lsp/`, and `bib/` (the parallel BibTeX
pipeline, below), plus top-level `syntax.rs`, `incremental.rs`, `config.rs`,
`cli.rs`, `completion.rs`, and `file_discovery.rs`.

### Supported inputs

The CLI processes `.tex`, `.sty`/`.cls`, `.dtx`, `.ins`, and `.bib` files.
Directories are walked with the [`ignore`](https://docs.rs/ignore) crate, honoring
`.gitignore` plus `badness.toml` excludes (see `file_discovery.rs`).

The lexer's `LatexFlavor` (`Document` vs `Package`) picks the starting catcode
regime: `.sty`/`.cls`/`.dtx` begin with `@` already a letter (implicit
`\makeatletter`). `.dtx` docstrip surface syntax is parsed; `.ins` install scripts
default to the `Preserve` wrap mode. Each `FileKind` carries its own default
`WrapMode`.

### The BibTeX subsystem

`.bib` files get their own full pipeline in `bib/`—a sibling of `parser/` built on
the *same* lossless rowan CST plus flat event-stream architecture, but with a
distinct grammar, its own `SyntaxKind`/`BibLang` marker, and its own lexer, parser,
`tree_builder`, `ast/`, formatter, linter, semantic layer, completion, and outline.
The same invariants apply (losslessness, idempotence), and the bib CST has typed AST
wrappers too (`bib/ast.rs`).

### Configuration

`badness.toml` is discovered by an ancestor walk from each input (`config.rs`), and
the **CLI is its only consumer**—the library API takes a fully-resolved
`FormatStyle`. Sections include `[format]` (`line-width`, `indent-width`, `wrap`,
`math-wrap`, `lang`, `no-break-abbreviations`) and `[build]` (`aux-dir`). Excludes
follow the [Ruff](https://docs.astral.sh/ruff/) model (`exclude` *replaces* the
built-in `DEFAULT_EXCLUDE`; `extend-exclude` is additive). `wrap` is optional and
resolves per file kind when omitted.

This keeps the formatter hermetic: config is local project data, not the
environment. TEXMF discovery is deliberately **not** a section here—where a TeX
installation lives is machine state, not project data, so it arrives via the LSP
editor settings, never `badness.toml`. See [LSP & Environment
Awareness](lsp.md).

## The pipeline

```
lexer → flat token stream → parser emits events → tree_builder → rowan GreenNode
                              (Start / Tok(idx) / Finish)   (re-attaches trivia)
```

The parser emits an **event stream**, not a tree directly. Tokens are referenced by
index, and diagnostics ride a side channel keyed by byte range—there is no `Error`
event. The tree builder re-attaches trivia and feeds
[rowan](https://github.com/rust-analyzer/rowan)'s `GreenNodeBuilder`. This is the
rust-analyzer event-stream pattern; the details, including the one extra `SubTok`
event, are on the [Parser](parser.md) page.

## Tenets

1. **Deterministic, rule-based formatting.** Layout is decided solely by the
   formatter's rules and the layout engine—the formatter is the **sole authority on
   layout**. Push back against hard-coding special cases. Autofixes are textual
   edits that never invoke the formatter: a fix decides *what* to rewrite, never
   *how to lay it out*, and owes only correctness (the result still parses and is
   still lossless), never line-width. When a fix can't meet that bar for some shape,
   make it correct by construction or withhold it for that shape (still report the
   finding). The pipeline is fix-then-format; the formatter never runs inside
   `--fix`.
2. **Incremental parsing is first-class.** Parser/CST work must keep the salsa-based
   reparse path (`incremental.rs`) viable.
3. **Parsing is the parser's job.** Never paper over parser mistakes in the
   formatter, and never let parsing logic creep into the formatter. If the formatter
   hits something the parser got wrong, fix it in the parser.
4. **Losslessness is the parser's job.** `reconstruct(text) == text`, always. The
   formatter may assume a lossless CST.

## Two layers: syntactic vs. semantic

The **syntactic** layer is the generic CST and knows nothing about what a command
means. The **semantic** layer is a *signature database*—a built-in table plus
CWL-style data plus `\newcommand`/`\newenvironment` scanning—that assigns arity,
verbatim-ness, and sectioning.

**Meaning never leaks into the parser.** The parser has one narrow, sanctioned
window onto static data (argument-shape facts used for verbatim capture), described
under [sanctioned lexer modes](parser.md#sanctioned-lexer-modes); it never resolves
what a macro *does*.

## Invariants (test oracles)

Three properties are held by construction and enforced as test oracles. A change
that breaks any of them is a bug, not a trade-off:

- **Losslessness:** `reconstruct(text) == text`, byte-for-byte.
- **Idempotence:** `fmt(fmt(x)) == fmt(x)`.
- **Whitespace-only formatter:** the formatter changes only *trivia* (whitespace,
  newlines, comments, `.dtx` margins/guards); it never inserts, deletes, or rewrites
  a non-trivia token. Meaning-preserving *content* normalizations—stripping redundant
  single-token script braces (`x^{2}` → `x^2`), rewriting `$$…$$` → `\[…\]`—are
  *linter autofixes*, not layout (see [Linter](linter.md) and [Formatter](formatter.md#the-formatter-is-whitespace-only)).
  Pinned by the non-trivia-content oracle in `assert_format_invariants`
  (`tests/format.rs`).
- **Protected regions** (`verbatim`, `lstlisting`, `\verb`, comments) are never
  altered by the formatter.

There is deliberately **no parse-stability invariant.** The formatter may still
change CST *shape*—the math operator split re-groups a catcode-12 `WORD`, so
`a+2` → `a + 2` re-lexes into separate atoms—but the whitespace-only invariant above
pins the non-trivia *content* it carries. The formatter is intentionally used to
stress the parser—any formatter ambiguity should surface a parser modeling gap.

**Differential oracle.** We use [texlab](https://github.com/latex-lsp/texlab)'s
parser as a differential *parse* oracle over a corpus—skeletonize both trees and
compare. It is a reference we measure against, never one we match.

## Technology choices

- **rowan** for the CST; **salsa** for incremental queries;
  [**smol_str**](https://docs.rs/smol_str) for interned token text;
  [**insta**](https://insta.rs/) for snapshot tests;
  [**annotate-snippets**](https://docs.rs/annotate-snippets) for diagnostics
  rendering.
- **Storing green nodes, never red.** Red trees (`SyntaxNode`) aren't
  `Send`/`Eq`/`salsa::Update`. `incremental.rs` uses `#[salsa::input] SourceFile {
  text }` and a `parsed_document` query returning `rowan::GreenNode` plus
  diagnostics under `no_eq, unsafe(non_update_types)`—sound because the tree is a
  pure function of the text—materializing red cursors on demand. See
  [Parser](parser.md#incrementality) for the incrementality story.
- **LSP:** `lsp-server` + `lsp-types` (rust-analyzer's stack), *not*
  `tower-lsp-server`. See [LSP & Environment
  Awareness](lsp.md#why-lsp-server-not-tower-lsp).
- **Formatter engine:** a Wadler/Prettier-style `Doc` IR. See
  [Formatter](formatter.md#the-doc-ir).
- **CLI:** [`clap`](https://docs.rs/clap) + `build.rs` generating man pages,
  completions, and markdown (`clap_mangen`, `clap_complete`, `clapdown`).

## Non-goals

- No general macro expansion, no TeX evaluator, no execution of TeX primitives or
  arbitrary `\def` semantics. (Common `\newcommand`/`\newenvironment`/xparse
  *signatures* may feed the semantic DB—extracted, never executed.)
- No general `\catcode` handling beyond the bounded, statically-recognizable
  patterns described on the [Parser](parser.md#sanctioned-lexer-modes) page.
- We never typeset.
- **The formatter never reads the environment.** `badness format` output is a pure
  function of the input plus shipped data (curated tables, CWL, the tlpdb-derived
  name lists and CTAN metadata). It resolves only *local* `.sty`/`.cls` next to the
  document (`semantic::load::DiskPackageSource`)—never the installed TEXMF tree—so
  output can't depend on what's installed. This is load-bearing for the
  deterministic-formatting tenet and the idempotence/losslessness oracles. The
  language server is allowed more latitude; see [LSP & Environment
  Awareness](lsp.md).
