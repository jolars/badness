# AGENTS.md

Guidance for AI agents (and humans) working in **badness**, a formatter, linter, and
language server for LaTeX.

badness is a sibling of **ravel** (`../ravel`), the same kind of tool for R. ravel is
a mature single-crate implementation of this exact architecture (rowan CST +
event-stream parser + salsa + Wadler formatter IR + tower-lsp-server). **When in
doubt, read ravel** — badness deliberately mirrors its shape, and bootstraps by
copying its language-agnostic parts (see *Relationship to ravel* below).

## What this project is

badness parses LaTeX into a **lossless concrete syntax tree (CST)** and builds three
tools on top of it: a **formatter** (`badness format`), a **linter** (diagnostics), and a
**language server** (LSP). The architecture follows **rust-analyzer**: a generic,
error-tolerant, hand-written parser producing a lossless tree, semantics layered on
top as a separate concern, and incremental recomputation via salsa.

**Single-crate Cargo package** (`badness`, edition 2024), *not* a workspace — same as
ravel. Module folders: `parser/`, `formatter/`, `linter/`, `semantic/`, `project/`,
`text/`, plus `syntax.rs` and `incremental.rs`.

## Tenets

(Adapted from ravel; these are language-neutral and load-bearing.)

1. **Deterministic, rule-based formatting.** Output is decided solely by the
   formatter's rules and the layout engine. Push back against hard-coding special
   cases for specific constructs.
2. **Incremental parsing is first-class**, not an afterthought. Parser/CST work must
   keep the salsa-based reparse path (`incremental.rs`) viable.
3. **Parsing is the parser's job.** Never paper over parser mistakes in the
   formatter, and never let parsing logic creep into the formatter. If the formatter
   hits something the parser got wrong, fix it in the parser.
4. **Losslessness is the parser's job.** The parser preserves all text so that
   `reconstruct(text) == text`, always. The formatter may assume a lossless CST.
5. **Autofixes never introduce formatting errors.** `format → lint --fix →
   format --check` must pass. Make fixes format-clean by construction, or withhold
   the fix (still report the finding); don't run the formatter inside `--fix`.

## Core architectural decisions

Load-bearing. If a change pushes against one of these, raise it explicitly.

1. **The parser backbone treats input as generic TeX surface syntax and always
   produces a lossless tree** — it never *requires* resolving macros or catcodes to
   succeed, because in full generality that is equivalent to running a TeX engine:
   catcodes are reassignable at runtime and tokenization is entangled with execution
   (e.g. `\makeatletter` changes whether `@` is part of a control word; a
   `\catcode` inside a conditional depends on a runtime value). We do **not**
   implement general macro expansion or a TeX evaluator.

   We **do** handle a bounded, growing set of *statically recognizable* patterns as
   lexer modes or semantic enrichment — `\makeatletter`/`\makeatother`,
   `\verb`/verbatim-like environments, and `\newcommand`/`xparse` *signatures*
   (extracted, never executed). Anything we cannot statically resolve degrades to
   generic nodes (plus a diagnostic where useful), never a crash or corruption.

2. **Two layers: syntactic vs. semantic.**
   - *Syntactic layer:* the generic CST. Knows nothing about what a command means.
   - *Semantic layer:* a **signature database** (built-in table + CWL-style data,
     later `\newcommand`/`\newenvironment` scanning) assigning meaning — arity,
     verbatim-ness, sectioning. This is the structural analog of ravel's `rindex/`
     R-package index. Meaning never leaks into the parser.

3. **Hand-written recursive descent is the spine. Pratt is local to math.**
   Operator precedence in LaTeX essentially only exists in math mode. Use a small
   precedence-climbing routine *only* for sub/superscript binding (`^`, `_`) and
   `\left…\right` matching, and only once we build a structured math model. The
   text-level parser has no precedence. (Contrast ravel, whose R `parser/expr.rs`
   *is* a full Pratt expression grammar — that's the main place the two parsers
   diverge.)

4. **Parser emits an event stream, not a tree directly** (ravel/ra shape):
   `lexer → flat token stream → parser emits events (Start/Tok(idx)/Finish) →
   tree_builder re-attaches trivia and feeds rowan's GreenNodeBuilder`. Tokens are
   referenced by index; there is **no `Error` event** — diagnostics ride a side
   channel keyed by byte range (copy ravel's `parser/events.rs` exactly).

5. **Errors travel alongside the tree, never abort it.** A single syntactic error
   never fails the whole parse. Recovery anchors for LaTeX are clean: `\end{…}`,
   `\begin`, blank line, `}`, `$`, `&`, `\\`. Always make progress; never
   infinite-loop on unexpected input.

6. **Incrementality is salsa-first.** Cross-file/cross-query incrementality via
   salsa is the v1 story. Intra-file incremental reparse (reusing green subtrees) is
   a *later optimization* — a whole-file reparse of a typical `.tex` is sub-ms.

7. **Store green nodes in salsa, never red (`SyntaxNode`).** Red trees aren't `Send`
   and aren't `Eq`/`salsa::Update`. Copy ravel's `incremental.rs`: `#[salsa::input]
   SourceFile { text }`, a `parsed_document` query returning `rowan::GreenNode` +
   diagnostics under `no_eq, unsafe(non_update_types)` (sound because the tree is a
   pure function of the text), and materialize red cursors on demand.

8. **Argument grouping is greedy and generic.** The CST greedily attaches trailing
   `{…}`/`[…]` groups to a command as argument nodes (texlab-style). Arity is not
   known at parse time; the semantic layer refines it.

## Invariants (these are test oracles — enforce them)

- **Losslessness:** `reconstruct(text) == text`, byte-for-byte. Enforced day one.
- **Idempotence:** `fmt(fmt(x)) == fmt(x)`.
- **Stability:** `parse(fmt(x))` is structurally equivalent to `parse(x)`.
- **Protected regions** (`verbatim`, `lstlisting`, `\verb`, comments) are never
  altered by the formatter.

The formatter is intentionally used to stress the parser: any formatter ambiguity
should surface a parser modeling gap. Lean on this loop.

**Differential oracles** (steal ravel's `air_compat` pattern): use **`latexindent`**
as a free differential *formatter* oracle (measure the fixed point
`latexindent(badness(x)) == badness(x)`, treat divergences as triage, not gates) and
**texlab's parser / tree-sitter-latex** as a differential *parse* oracle over a
corpus. Both are external reference implementations we measure against, never match.

## Technology choices (aligned with ravel's Cargo.toml)

- **rowan** (`0.16`) — lossless CST. **salsa** (`0.26`) — incremental queries.
- **smol_str** for interned token text; **insta** for snapshot tests.
- **LSP:** `lsp-server` + `lsp-types` (rust-analyzer's own stack), **not**
  `tower-lsp-server`. This is the one place badness deliberately diverges from ravel.
  Reason: salsa cancellation is a synchronous unwind (`salsa::Cancelled`) under a
  single-writer/snapshot-readers model — it composes cleanly with `lsp-server`'s
  sync main loop + threadpool (exactly how ra does it) and fights tower-lsp's async
  `&self` model. Reuse ravel's `text/line_index.rs` logic but swap its
  `tower_lsp_server::ls_types::Position` for `lsp_types::Position`.
- **Formatter engine:** a Wadler/Prettier-style `Doc` IR
  (`Group`/`Line`/`SoftLine`/`HardLine`/`EmptyLine`/`Indent`, flat/break fit) — copy
  ravel's `formatter/ir.rs` + `printer.rs` nearly wholesale. **Addition over ravel:**
  an `Ir::Fill` node (Wadler/Prettier *fill*: per-gap greedy break decisions) for
  paragraph reflow — ravel formats R and has no prose-wrapping, so this primitive is
  badness-specific. Keep the rest of the engine close to ravel's.
- **Paragraph line breaks** are controlled by a `WrapMode` (`Reflow` default,
  `Sentence`, `Semantic`/sembr, `Preserve`), modeled on the sibling **panache**
  formatter's mode taxonomy. badness mechanizes it through the `Doc` IR (`Fill`),
  *not* panache's separate streaming line-filler — the printer stays the single
  layout authority. `Reflow` and `Preserve` are implemented; `Sentence`/`Semantic`
  currently fall back to `Preserve`.
- **CLI:** `clap` + `build.rs` generating man pages / completions / markdown
  (`clap_mangen`, `clap_complete`, `clap-markdown`) — copy ravel's scaffolding.
- **Diagnostics rendering:** `annotate-snippets`.

## Relationship to ravel (copy now, extract later)

Decision: **bootstrap badness by copying ravel's language-agnostic skeleton, then
extract a shared crate once badness's formatter works and the boundaries are proven.**
Premature extraction is the bigger risk while badness is empty.

**Copy ~wholesale (language-agnostic) — mark each as an EXTRACTION CANDIDATE:**
- `formatter/ir.rs` + `formatter/printer.rs` (the Wadler engine — extract first)
- `text/line_index.rs` (swap the LSP `Position` type — see LSP note above)
- `parser/events.rs` + `parser/tree_builder.rs` (generic given SyntaxKind + Token)
- `incremental.rs` salsa harness shape
- `config`, `file_discovery`, `linter/suppression`, diagnostic rendering, `build.rs`

**Rewrite for LaTeX (the genuinely different part):** `parser/lexer.rs`,
`parser/expr.rs` + `structural.rs`, `syntax.rs` kinds, `ast/`, `semantic/` scoping,
and the signature DB (analog of `rindex/`).

**Diverge from ravel on purpose:** `lsp.rs` — badness uses `lsp-server` (see LSP
note); do *not* copy ravel's tower-lsp-server loop.

When you touch a copied file, keep it close to ravel's version so the eventual
extraction stays a mechanical lift, not a merge.

## Non-goals (keep these true)

- No general macro expansion; no TeX evaluator; no execution of TeX primitives or
  arbitrary `\def` semantics. (Common `\newcommand`/`\newenvironment`/`xparse`
  *signatures* may feed the semantic DB — extracted, never executed.)
- No general `\catcode` handling beyond the bounded, statically-recognizable
  patterns named in decision #1.
- We are not a TeX engine; we never typeset.

## Repo conventions

- Rust edition 2024; toolchain pinned in `devenv.nix` (currently 1.94.1).
- A `wasm32-unknown-unknown` target is configured (web/playground is in scope).
- `rustfmt` runs as a git hook; keep code rustfmt-clean. `clippy` warnings are
  errors (`cargo clippy --all-targets --all-features -- -D warnings`).
- Performance is first-class: `perf`, `cargo-flamegraph`, `hyperfine`,
  `cargo-show-asm`, `cargo-llvm-cov` are in the dev shell. Benchmark before
  optimizing; never regress losslessness for speed.
- Task runner: `go-task` (`Taskfile.yml`, mirror ravel's targets).

## Working agreements for agents

- Match surrounding (and ravel's) idioms, naming, comment density.
- New parser features need corpus + snapshot tests *and* a losslessness assertion.
- Keep the syntactic layer free of semantic knowledge.
- Don't add intra-file incremental reparse, macro expansion, or catcode logic beyond
  decision #1 without recording the decision here.
- Update TODO.md as phases progress; update this file when a decision changes.
