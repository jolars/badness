# AGENTS.md

Guidance for AI agents (and humans) working in **badness**, a formatter, linter, and
language server for LaTeX.

badness is a sibling of **arity** (`../arity`), the same kind of tool for R. arity is
a mature single-crate implementation of this exact architecture (rowan CST +
event-stream parser + salsa + Wadler formatter IR + tower-lsp-server). **When in
doubt, read arity** — badness deliberately mirrors its shape, and bootstraps by
copying its language-agnostic parts (see *Relationship to arity* below).

## What this project is

badness parses LaTeX into a **lossless concrete syntax tree (CST)** and builds three
tools on top of it: a **formatter** (`badness format`), a **linter** (diagnostics), and a
**language server** (LSP). The architecture follows **rust-analyzer**: a generic,
error-tolerant, hand-written parser producing a lossless tree, semantics layered on
top as a separate concern, and incremental recomputation via salsa.

**Single-crate Cargo package** (`badness`, edition 2024), *not* a workspace — same as
arity. Module folders: `parser/`, `formatter/`, `linter/`, `semantic/`, `project/`,
`text/`, plus `syntax.rs` and `incremental.rs`.

## Tenets

(Adapted from arity; these are language-neutral and load-bearing.)

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
   `\verb`/verbatim-like environments, `\left`/`\right` delimiter isolation, and
   `\newcommand`/`xparse` *signatures* (extracted, never executed). Anything we
   cannot statically resolve degrades to generic nodes (plus a diagnostic where
   useful), never a crash or corruption.

   **`\left`/`\right` delimiter isolation (a sanctioned lexer mode):** the single
   delimiter following `\left`/`\right` is emitted as its own token, so a
   word-character delimiter (`(`, `)`, `|`, `/`, `.`, `<`, `>`) does not glue into
   the following word run and become un-splittable downstream (the same
   surface-lexing problem `\verb` has — control-symbol/control-word/bracket
   delimiters already lex as single tokens). The mode reads only the static fact
   "the previous control word was `\left`/`\right`"; no macro meaning is resolved,
   so it stays inside this decision's sanctioned set. The matched pair
   (`LEFT_RIGHT`) is then built by the parser per decision #3.

   **Exception to "meaning never leaks into the parser" (decision #2), recorded
   deliberately:** for *argument-taking* verbatim environments (`lstlisting`,
   `minted`, `Verbatim`) the raw body begins only after the `\begin` arguments, so
   the lexer consults the built-in signature DB (`semantic::signature::builtin`) to
   read each environment's static arg shape and find where the opaque body starts.
   This is the single source of truth (`data/signatures.json`), keeps the lexer and
   `grammar.rs` in lockstep via `is_verbatim_environment`, and reads only static
   argument-shape data — no macro meaning is resolved — so it stays inside this
   decision's sanctioned lexer modes. User-defined verbatim environments stay out of
   scope (their definitions aren't known until after parsing).

   **Verbatim-argument *commands* (`\verb` generalized):** commands flagged
   `verbatim` in the signature DB (`\verb`, `\lstinline`, `\url`, `\code`, …) have
   their final argument captured as a single `VERB` token — a balanced `{…}` group
   or a `\verb`-style delimiter run, chosen by the argument's first character, with
   any leading non-verbatim args read from the DB's static arg shape (e.g.
   `\mintinline`'s language). Same rationale as verbatim environments: reads only
   static argument-shape data, no macro meaning resolved. A curated set of
   well-known *class*-defined commands is allowed as built-ins (e.g. jss's `\code`,
   whose `\@makeother\$` makes `$` literal — a runtime catcode fact we cannot
   derive, so we record it as data); arbitrary user-defined verbatim commands stay
   out of scope until definition-scanning lands (see `TODO.md`).

2. **Two layers: syntactic vs. semantic.**
   - *Syntactic layer:* the generic CST. Knows nothing about what a command means.
   - *Semantic layer:* a **signature database** (built-in table + CWL-style data,
     later `\newcommand`/`\newenvironment` scanning) assigning meaning — arity,
     verbatim-ness, sectioning. This is the structural analog of arity's `rindex/`
     R-package index. Meaning never leaks into the parser.

3. **Hand-written recursive descent is the spine. Pratt is local to math.**
   Operator precedence in LaTeX essentially only exists in math mode. Use a small
   precedence-climbing routine *only* for sub/superscript binding (`^`, `_`) and
   `\left…\right` matching, and only once we build a structured math model. The
   text-level parser has no precedence. (Contrast arity, whose R `parser/expr.rs`
   *is* a full Pratt expression grammar — that's the main place the two parsers
   diverge.)

4. **Parser emits an event stream, not a tree directly** (arity/ra shape):
   `lexer → flat token stream → parser emits events (Start/Tok(idx)/Finish) →
   tree_builder re-attaches trivia and feeds rowan's GreenNodeBuilder`. Tokens are
   referenced by index; there is **no `Error` event** — diagnostics ride a side
   channel keyed by byte range (copy arity's `parser/events.rs` exactly).

5. **Errors travel alongside the tree, never abort it.** A single syntactic error
   never fails the whole parse. Recovery anchors for LaTeX are clean: `\end{…}`,
   `\begin`, blank line, `}`, `$`, `&`, `\\`. Always make progress; never
   infinite-loop on unexpected input.

6. **Incrementality is salsa-first.** Cross-file/cross-query incrementality via
   salsa is the v1 story. Intra-file incremental reparse (reusing green subtrees) is
   a *later optimization* — a whole-file reparse of a typical `.tex` is sub-ms.

7. **Store green nodes in salsa, never red (`SyntaxNode`).** Red trees aren't `Send`
   and aren't `Eq`/`salsa::Update`. Copy arity's `incremental.rs`: `#[salsa::input]
   SourceFile { text }`, a `parsed_document` query returning `rowan::GreenNode` +
   diagnostics under `no_eq, unsafe(non_update_types)` (sound because the tree is a
   pure function of the text), and materialize red cursors on demand.

8. **Argument grouping is greedy and generic.** The CST greedily attaches trailing
   `{…}`/`[…]` groups to a command as argument nodes (texlab-style). Arity is not
   known at parse time; the semantic layer refines it.

9. **Trivia attachment follows the rust-analyzer rule: comments bind *forward*,
   whitespace floats, blank lines break the bind.** Trivia (`WHITESPACE`,
   `NEWLINE`, `COMMENT`) is never dropped — losslessness forces every trivia token
   to be a leaf under *some* node — so the only decision is *which* node owns it.
   The policy:
   - **Default: float at the nearest enclosing node.** Inter-sibling whitespace and
     newlines stay direct children of the tightest block/group that contains the
     boundary (never hoisted to `ROOT`), owned by neither neighbor. This is what the
     inline trivia bumps in `grammar.rs` already do (`skip_trivia` between siblings).
   - **Exception: a contiguous run of `%` comments immediately preceding a
     documentable construct attaches *leading* into that construct**, so the comment
     binds to the command/environment/sectioning node it annotates — exactly ra's
     `n_attached_trivias` (comments attach forward to item-like nodes). "Documentable"
     is decided **purely on node kind** — any `COMMAND` or `ENVIRONMENT` — so no
     signature-DB lookup leaks into the parser (sectioning commands are `COMMAND`
     nodes, covered without special-casing). A *same-line trailing* comment
     (`\foo % x`, no newline before it) is **not** leading and never binds.
   - **A blank line (`≥2` newlines, the `\par` boundary) breaks the bind:** comments
     past a blank line stay floating, never leading. Mirrors ra's `"\n\n"` cutoff.
     The binding run is the *maximal blank-line-free suffix* of the preceding trivia
     that starts at an own-line comment (so in `%a \n\n %b \foo`, `%a` floats and
     `%b` binds).

   Trivia stays **bare leaf tokens**, never wrapped in a node — the token *kind*
   already marks it skippable (`Parser::is_trivia`), matching arity/ra and keeping
   `tree_builder` a mechanical replay. A *named* trivia node (e.g. a `DOC_COMMENT`
   grouping) is reserved for a later semantic enrichment, not the default for plain
   whitespace. There is no parse-stability invariant, so this policy is a CST-shape
   *convention* enforced by tests, not a hard oracle. The leading comment-bind is
   implemented **grammar-locally** (`grammar.rs` `binding_run` + the `precede`
   idiom), so `tree_builder` stays a mechanical replay; the construct self-opens and
   its `Start` is pulled back over the bound comments.

## Invariants (these are test oracles — enforce them)

- **Losslessness:** `reconstruct(text) == text`, byte-for-byte. Enforced day one.
- **Idempotence:** `fmt(fmt(x)) == fmt(x)`.
- **Protected regions** (`verbatim`, `lstlisting`, `\verb`, comments) are never
  altered by the formatter.

There is deliberately **no** parse-stability invariant (`parse(fmt(x))`
structurally equal to `parse(x)`). The formatter is allowed to *normalize*
structure — e.g. stripping redundant braces around a single-token math script
(`x^{2}` → `x^2`) changes the CST shape on purpose. Such rewrites must preserve
*meaning* (a correctness requirement carried by fixtures and the corpus, 
but they are not held to structural equality with the input.

The formatter is intentionally used to stress the parser: any formatter ambiguity
should surface a parser modeling gap. Lean on this loop.

**Differential oracles** (steal arity's `air_compat` pattern): use **texlab's
parser / tree-sitter-latex** as a differential *parse* oracle over a corpus.
Both are external reference implementations we measure against, never match.

## Technology choices (aligned with arity's Cargo.toml)

- **rowan** (`0.16`) — lossless CST. **salsa** (`0.26`) — incremental queries.
- **smol_str** for interned token text; **insta** for snapshot tests.
- **LSP:** `lsp-server` + `lsp-types` (rust-analyzer's own stack), **not**
  `tower-lsp-server`. This is the one place badness deliberately diverges from arity.
  Reason: salsa cancellation is a synchronous unwind (`salsa::Cancelled`) under a
  single-writer/snapshot-readers model — it composes cleanly with `lsp-server`'s
  sync main loop + threadpool (exactly how ra does it) and fights tower-lsp's async
  `&self` model. Reuse arity's `text/line_index.rs` logic but swap its
  `tower_lsp_server::ls_types::Position` for `lsp_types::Position`.
- **Formatter engine:** a Wadler/Prettier-style `Doc` IR
  (`Group`/`Line`/`SoftLine`/`HardLine`/`EmptyLine`/`Indent`, flat/break fit) — copy
  arity's `formatter/ir.rs` + `printer.rs` nearly wholesale. **Addition over arity:**
  an `Ir::Fill` node (Wadler/Prettier *fill*: per-gap greedy break decisions) for
  paragraph reflow — arity formats R and has no prose-wrapping, so this primitive is
  badness-specific. Keep the rest of the engine close to arity's.
- **Paragraph line breaks** are controlled by a `WrapMode` (`Reflow` default,
  `Sentence`, `Semantic`/sembr, `Preserve`), modeled on the sibling **panache**
  formatter's mode taxonomy. badness mechanizes it through the `Doc` IR (`Fill`),
  *not* panache's separate streaming line-filler — the printer stays the single
  layout authority. `Reflow` and `Preserve` are implemented; `Sentence`/`Semantic`
  currently fall back to `Preserve`. The `\\` line break (with a tightly-bound
  `*` / `[len]`) is grouped by the *parser* into a `LINE_BREAK` node — a formatter
  ambiguity (the orphaned `[2ex]`) driven back into the parser per tenet 3, so the
  formatter sees `\\[2ex]` as one unit instead of splitting it.
- **CLI:** `clap` + `build.rs` generating man pages / completions / markdown
  (`clap_mangen`, `clap_complete`, `clap-markdown`) — copy arity's scaffolding.
- **Diagnostics rendering:** `annotate-snippets`.

## Relationship to arity (copy now, extract later)

Decision: **bootstrap badness by copying arity's language-agnostic skeleton, then
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

**Diverge from arity on purpose:** `lsp.rs` — badness uses `lsp-server` (see LSP
note); do *not* copy arity's tower-lsp-server loop.

When you touch a copied file, keep it close to arity's version so the eventual
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
- Task runner: `go-task` (`Taskfile.yml`, mirror arity's targets).

## Working agreements for agents

- Match surrounding (and arity's) idioms, naming, comment density.
- New parser features need corpus + snapshot tests *and* a losslessness assertion.
- Keep the syntactic layer free of semantic knowledge.
- Don't add intra-file incremental reparse, macro expansion, or catcode logic beyond
  decision #1 without recording the decision here.
- Update TODO.md as phases progress; update this file when a decision changes.
