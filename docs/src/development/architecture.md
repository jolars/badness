# Architecture

Badness parses LaTeX into a lossless concrete syntax tree (CST) and puts a
formatter, a linter, and a language server on top of it. The design follows
[rust-analyzer](https://rust-analyzer.github.io/): a generic, error-tolerant,
hand-written parser produces a lossless tree, semantics live in a separate layer
above it, and recomputation is incremental via
[salsa](https://github.com/salsa-rs/salsa).
[arity](https://github.com/jolars/arity), the same kind of tool for R, was the
other influence.

The [parser](parser.md), the [formatter](formatter.md), the [linter](linter.md),
and the [language server](lsp.md) have their own pages.

## What it does

Badness turns source text into a syntax tree, and the tree into diagnostics and
formatted text. It does not typeset, it does not run TeX, and, outside the
language server, it does not look at the machine it runs on.

The pipeline is:

```
text → lexer → token stream → parser → event stream → tree_builder → GreenNode
```

The parser emits events (`Start`, `Tok(idx)`, `Finish`) instead of building a
tree directly. Tokens are referred to by index and diagnostics travel on a side
channel keyed by byte range, so there is no `Error` event. The tree builder
re-attaches trivia and feeds rowan's `GreenNodeBuilder`.

Everything downstream reads that tree. The formatter lowers it to a `Doc` IR and
prints the IR, the linter walks it once and collects diagnostics, and the
language server answers requests from salsa queries over it.

The tree is a pure function of the file's text. Config, the signature database,
and the filesystem take no part in producing it. Determinism, error tolerance,
and incremental recomputation all rest on that.

## The crates

Badness is a four-crate Cargo workspace on edition 2024. The root package is the
CLI, LSP, and linter crate `badness`; two publishable library crates and one
unpublished wasm shim live under `crates/`.

`badness-parser` holds the syntax layer (`syntax`, `ast`), the parser, the
semantic layer, the BibTeX parsing and semantic layers, the `data/` signature
artifacts, and the `build.rs` that bakes them into phf tables.

`badness-formatter` holds the layout engine (`core`, `ir`, `printer`, `style`,
`context`, `colspec`, `sentence`, `perturb`) and the `.bib` formatter. It
depends on `badness-parser`.

`badness-wasm` is a `publish = false` wasm-bindgen shim over the two library
crates. It powers the [playground](../playground/index.html) and is built with
`wasm-pack` through `task playground:wasm`.

Both library crates build for `wasm32-unknown-unknown`, so nothing in them may
touch the filesystem, threads, or processes. The formatter is embedded by the
dprint Wasm plugin, and a CI job guards the target. Anything that needs the
outside world lives in the root crate.

The root crate keeps `linter/`, `lsp/`, `project/`, `text/`, plus
`incremental.rs` (salsa), `config.rs`, `cli.rs`, `completion.rs`, and
`file_discovery.rs`. It re-exports the member crates at their old module paths
through shim modules, so `src/parser.rs` is one
`pub use badness_parser::parser::*;` line and callers keep writing
`crate::parser::…`. Two modules are real bridges rather than shims:
`src/formatter.rs` holds the `check` batch driver and the disk-backed
`format_file_with_packages` entries, and `src/semantic.rs` holds `load`.

## The BibTeX side

`.bib` files get their own pipeline in `bib/`, a sibling of `parser/` rather
than a mode of it. It is built on the same lossless rowan CST and the same flat
event stream, but has its own grammar, `SyntaxKind`, `BibLang` marker, lexer,
parser, tree builder, typed AST, formatter, linter, semantic layer, completion,
and outline. The invariants below apply to it unchanged.

## Inputs and configuration

The CLI processes `.tex`, `.sty`, `.cls`, `.dtx`, `.ins`, and `.bib`.
Directories are walked with [`ignore`](https://docs.rs/ignore), honoring
`.gitignore` and `badness.toml` excludes.

The lexer's `LatexFlavor` picks the starting catcode regime. `Package` (`.sty`,
`.cls`, `.dtx`) begins with `@` already a letter, as if under `\makeatletter`;
`Document` does not. `.dtx` docstrip surface syntax is parsed.

Wrap mode is not a property of the file kind. Every kind defaults to
`WrapMode::Reflow`, and content that cannot be safely reflowed is refused
structurally in every mode. See
[Formatter](formatter.md#reflow-is-safe-by-construction-not-by-file-kind).

`badness.toml` is found by walking ancestors from each input. The CLI is its
only consumer; the library API takes a resolved `FormatStyle`. Sections are
`[format]` (`line-width`, `indent-width`, `wrap`, `math-wrap`, `lang`,
`no-break-abbreviations`), `[lint]` (`select`, `ignore`), and `[build]`
(`aux-dir`). Excludes follow Ruff: `exclude` replaces the built-in default,
`extend-exclude` adds to it. `wrap` is an `Option` so the LSP can tell "unset"
from "set" when merging editor settings over project config, not because the
fallback depends on the file.

TEXMF discovery is deliberately not a section here. Where a TeX installation
lives is machine state rather than project data, so it arrives through editor
settings. See [LSP](lsp.md).

## Two layers

The syntactic layer is the generic CST. It knows nothing about what a command
means.

The semantic layer is a signature database: a curated built-in table, a bulk
CWL-derived tier, and `\newcommand`/`\newenvironment` scanning. It assigns
arity, verbatim-ness, sectioning, and per-argument content kinds.

Meaning never leaks downward. The parser may read static lexical facts, never
signature data that config, package scopes, or scanned definitions can change.

## Tenets

1. Layout is decided solely by the formatter's rules and the layout engine. The
   formatter is the sole authority on layout, so push back against hard-coded
   special cases.
2. Autofixes are textual edits that never invoke the formatter. A fix decides
   what to rewrite, never how to lay it out, and owes correctness (the result
   still parses and is still lossless) but not line width. When a fix cannot
   meet that bar for some shape, make it correct by construction or withhold it
   for that shape while still reporting the finding. The pipeline is
   fix-then-format, and the mirror holds: content rewrites never run inside
   `format`.
3. Parser and CST work must keep the salsa reparse path viable.
4. Parsing is the parser's job. Never paper over a parser mistake in the
   formatter, and never let parsing logic creep into the formatter.
5. Losslessness is the parser's job. The formatter may assume a lossless CST.

## Invariants

These are held by construction and enforced as test oracles. Breaking one is a
bug, not a trade-off.

- Losslessness: `reconstruct(text) == text`, byte for byte.
- Idempotence: `fmt(fmt(x)) == fmt(x)`.
- The formatter is whitespace-only. It changes trivia (whitespace, newlines,
  comments, `.dtx` margins and guards) and nothing else. It never inserts,
  deletes, or rewrites a non-trivia token. Meaning-preserving content rewrites,
  such as `x^{2}` → `x^2` or `$$…$$` → `\[…\]`, are linter autofixes.
- Protected regions (`verbatim`, `lstlisting`, `\verb`, comments) are never
  altered, with one carve-out for line terminators; see
  [Formatter](formatter.md#line-endings).
- Trivia-invariant layout: layout may read only those trivia predicates the
  formatter itself preserves. This is being rolled out; see
  [Formatter](formatter.md#trivia-invariant-layout).

There is deliberately no parse-stability invariant. The formatter may change CST
shape: the math operator split re-groups a catcode-12 `WORD`, so `a+2` → `a + 2`
re-lexes into separate atoms. The whitespace-only invariant pins the non-trivia
content the tree carries, which is the part that matters. Running the formatter
over a corpus is a good way to find parser modeling gaps, so this freedom is
useful rather than merely tolerated.

We also run [texlab](https://github.com/latex-lsp/texlab)'s parser as a
differential oracle over a corpus, skeletonizing both trees and comparing. It is
a reference we measure against, not one we match.

## Technology choices

rowan for the CST, salsa for incremental queries,
[smol_str](https://docs.rs/smol_str) for token text, [insta](https://insta.rs/)
for snapshot tests, [annotate-snippets](https://docs.rs/annotate-snippets) for
diagnostic rendering, and [`clap`](https://docs.rs/clap) for the CLI, with
`build.rs` generating man pages, completions, and markdown.

Salsa stores green nodes, never red ones: `SyntaxNode` is not `Send`, `Eq`, or
`salsa::Update`. `incremental.rs` stores `rowan::GreenNode` under
`no_eq, unsafe(non_update_types)`, which is sound because the tree is a pure
function of the text, and materializes red cursors on demand. See
[Parser](parser.md#incrementality).

The LSP is built on `lsp-server` and `lsp-types` rather than `tower-lsp-server`;
see [LSP](lsp.md#why-lsp-server-not-tower-lsp). The formatter uses a
Wadler/Prettier-style `Doc` IR; see [Formatter](formatter.md#the-doc-ir).

## Non-goals

No macro expansion, no TeX evaluator, no execution of primitives or `\def`
semantics. Common `\newcommand`, `\newenvironment`, and xparse *signatures* may
feed the semantic database, but they are extracted, never executed.

No general `\catcode` handling beyond the bounded patterns listed under
[sanctioned lexer modes](parser.md#sanctioned-lexer-modes).

No typesetting.

The formatter never reads the environment. Its output is a function of the input
plus shipped data (the curated tables, CWL, and the tlpdb-derived name lists and
CTAN metadata). It resolves local `.sty` and `.cls` files sitting next to the
document, never the installed TEXMF tree, so output cannot depend on what
happens to be installed. The language server is allowed more latitude; see
[LSP](lsp.md).
