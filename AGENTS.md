# AGENTS.md

Guidance for AI agents (and humans) working in **badness**, a formatter, linter, and
language server for LaTeX.

badness is a sibling of **arity** (`../arity`), the same kind of tool for R. arity is
a mature single-crate implementation of this exact architecture (rowan CST +
event-stream parser + salsa + Wadler formatter IR + tower-lsp-server). **When in
doubt, read arity**—badness deliberately mirrors its shape, and bootstraps by
copying its language-agnostic parts (see *Relationship to arity* below).

## What this project is

badness parses LaTeX into a **lossless concrete syntax tree (CST)** and builds three
tools on top of it: a **formatter** (`badness format`), a **linter** (diagnostics), and a
**language server** (LSP). The architecture follows **rust-analyzer**: a generic,
error-tolerant, hand-written parser producing a lossless tree, semantics layered on
top as a separate concern, and incremental recomputation via salsa.

**Single-crate Cargo package** (`badness`, edition 2024), *not* a workspace—same as
arity. Module folders: `parser/`, `formatter/`, `linter/`, `semantic/`, `project/`,
`text/`, plus `syntax.rs` and `incremental.rs`.

## Tenets

(Adapted from arity; these are language-neutral and load-bearing.)

1. **Deterministic, rule-based formatting.** Output is decided solely by the
   formatter's rules and the layout engine. Push back against hard-coding special
   cases for specific constructs. Because the formatter is the **sole authority on
   layout**, autofixes are textual edits that never invoke it: a fix decides
   *what* to rewrite, never *how to lay it out*. A fix owes only correctness —
   applying it must leave a tree that still parses and is still lossless—never
   line-width or any other formatting property; producing well-formatted output
   after a fix is a separate format pass's job (the pipeline is fix-then-format).
   When an edit can't meet that parses-and-lossless bar for some shape, make it
   correct by construction (tight span, atom-guarded) or withhold the fix for that
   shape (the finding is still reported). Don't run the formatter inside `--fix`.
2. **Incremental parsing is first-class**, not an afterthought. Parser/CST work must
   keep the salsa-based reparse path (`incremental.rs`) viable.
3. **Parsing is the parser's job.** Never paper over parser mistakes in the
   formatter, and never let parsing logic creep into the formatter. If the formatter
   hits something the parser got wrong, fix it in the parser.
4. **Losslessness is the parser's job.** The parser preserves all text so that
   `reconstruct(text) == text`, always. The formatter may assume a lossless CST.

## Core architectural decisions

Load-bearing. If a change pushes against one of these, raise it explicitly.

1. **The parser backbone treats input as generic TeX surface syntax and always
   produces a lossless tree**—it never *requires* resolving macros or catcodes to
   succeed, because in full generality that is equivalent to running a TeX engine:
   catcodes are reassignable at runtime and tokenization is entangled with execution
   (e.g. `\makeatletter` changes whether `@` is part of a control word; a
   `\catcode` inside a conditional depends on a runtime value). We do **not**
   implement general macro expansion or a TeX evaluator.

   We **do** handle a bounded, growing set of *statically recognizable* patterns as
   lexer modes or semantic enrichment—`\makeatletter`/`\makeatother`,
   `\ExplSyntaxOn`/`\ExplSyntaxOff` (expl3 letter mode), `\verb`/verbatim-like
   environments, `\left`/`\right` delimiter isolation, and `\newcommand`/`xparse`
   *signatures* (extracted, never executed). Anything we cannot statically resolve
   degrades to generic nodes (plus a diagnostic where useful), never a crash or
   corruption.

   **expl3 syntax mode (a sanctioned lexer mode):** between `\ExplSyntaxOn` and
   `\ExplSyntaxOff` (and after a `\ProvidesExplPackage`/`\ProvidesExplClass`/
   `\ProvidesExplFile` declaration, which opens it for the rest of the file), `_`
   and `:` are catcode-11 *letters*, so expl3 names (`\seq_new:N`,
   `\__module_internal:nn`) lex as single control words and a bare `_` is text, not
   a subscript. The mode reads only the static fact "we are inside an expl3 region";
   no macro meaning is resolved. It is an independent boolean flag that *composes*
   with `\makeatletter` (the `@@` module-prefix convention `\g_@@_x_tl` needs both),
   threaded through the lexer exactly like `at_letter`. Scope is deliberately
   letters-only: expl3's other catcode changes (`~`→space, spaces/tabs ignored) and
   *implicit* detection in toggle-less `.dtx` sources are recorded follow-ups in
   `TODO.md`, not yet modeled.

   **`\left`/`\right` delimiter isolation (a sanctioned lexer mode):** the single
   delimiter following `\left`/`\right` is emitted as its own token, so a
   word-character delimiter (`(`, `)`, `|`, `/`, `.`, `<`, `>`) does not glue into
   the following word run and become un-splittable downstream (the same
   surface-lexing problem `\verb` has—control-symbol/control-word/bracket
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
   argument-shape data—no macro meaning is resolved—so it stays inside this
   decision's sanctioned lexer modes. User-defined verbatim environments stay out of
   scope (their definitions aren't known until after parsing).

   **Verbatim-argument *commands* (`\verb` generalized):** commands flagged
   `verbatim` in the signature DB (`\verb`, `\lstinline`, `\url`, `\code`, …) have
   their final argument captured as a single `VERB` token—a balanced `{…}` group
   or a `\verb`-style delimiter run, chosen by the argument's first character, with
   any leading non-verbatim args read from the DB's static arg shape (e.g.
   `\mintinline`'s language). Same rationale as verbatim environments: reads only
   static argument-shape data, no macro meaning resolved. A curated set of
   well-known *class*-defined commands is allowed as built-ins (e.g. jss's `\code`,
   whose `\@makeother\$` makes `$` literal—a runtime catcode fact we cannot
   derive, so we record it as data).

   **User-defined verbatim-argument commands via definition scanning (a bounded
   two-pass parse):** beyond the curated built-ins, the definition scanner
   (`semantic::define`) flags an *arbitrary* command verbatim when its
   `\newcommand`/xparse/`\def` replacement **body** reassigns a special char's catcode
   to "other"—the static fingerprint `\@makeother`, `\catcode…12`, `\dospecials`,
   `\@sanitize`, possibly one or more hops away through a chained helper macro it
   calls (followed across the scanned definition set, with a cycle guard). The
   `\def`/`\edef`/`\gdef`/`\xdef` forms have no `[n]` arity optional, so their arity is
   counted from the `#1#2…` **parameter text** between the name and the body group
   (`scan_def`/`def_params_and_body`); a `\def` helper's body is scanned like any other,
   so chains resolve through it. Only the command's *own* arity gates it (it must take
   an argument to capture); the final argument becomes the implicit verbatim one. This
   **reads replacement-body surface text**—a deliberate, recorded step past
   "signatures only"—but executes nothing, expands nothing, and evaluates no catcode
   arithmetic; it matches static substrings. It is **conservative by construction**: a
   false positive *suppresses* real diagnostics (the worse failure), so we flag only on
   a clear catcode signal and prefer false negatives (e.g. a `\let`-aliased helper, or a
   definition visible only after re-tokenization, is not followed). Because the lexer
   must know a verbatim command *before* it tokenizes call sites, but such commands are
   only discoverable from the parsed tree, `parser::parse` runs a **bounded two-pass
   parse**: pass 1 with built-ins only, a definition scan, and—only when it finds a
   user verbatim command—pass 2 re-lexing with those names fed into the lexer (a lexer
   `pending_def` state keeps a command's own definition site from being mis-lexed as
   a call). Two passes is the bound; a definition visible only after re-tokenization
   is a tolerated false negative. Reparse cost is paid only when such a definition
   exists (decision #6). `\def`-defined verbatim *environments* and delimited-parameter
   `\def` macros stay out of scope (see `TODO.md`).

2. **Two layers: syntactic vs. semantic.**
   - *Syntactic layer:* the generic CST. Knows nothing about what a command means.
   - *Semantic layer:* a **signature database** (built-in table + CWL-style data,
     later `\newcommand`/`\newenvironment` scanning) assigning meaning—arity,
     verbatim-ness, sectioning. This is the structural analog of arity's `rindex/`
     R-package index. Meaning never leaks into the parser.

3. **Hand-written recursive descent is the spine. Pratt is local to math.**
   Operator precedence in LaTeX essentially only exists in math mode. Use a small
   precedence-climbing routine *only* for sub/superscript binding (`^`, `_`) and
   `\left…\right` matching, and only once we build a structured math model. The
   text-level parser has no precedence. (Contrast arity, whose R `parser/expr.rs`
   *is* a full Pratt expression grammar—that's the main place the two parsers
   diverge.)

4. **Parser emits an event stream, not a tree directly** (arity/ra shape):
   `lexer → flat token stream → parser emits events (Start/Tok(idx)/Finish) →
   tree_builder re-attaches trivia and feeds rowan's GreenNodeBuilder`. Tokens are
   referenced by index; there is **no `Error` event**—diagnostics ride a side
   channel keyed by byte range (copy arity's `parser/events.rs` exactly).

5. **Errors travel alongside the tree, never abort it.** A single syntactic error
   never fails the whole parse. Recovery anchors for LaTeX are clean: `\end{…}`,
   `\begin`, blank line, `}`, `$`, `&`, `\\`. Always make progress; never
   infinite-loop on unexpected input.

6. **Incrementality is salsa-first.** Cross-file/cross-query incrementality via
   salsa is the v1 story. Intra-file incremental reparse (reusing green subtrees) is
   a *later optimization*—a whole-file reparse of a typical `.tex` is sub-ms.

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
   `NEWLINE`, `COMMENT`) is never dropped—losslessness forces every trivia token
   to be a leaf under *some* node—so the only decision is *which* node owns it.
   The policy:
   - **Default: float at the nearest enclosing node.** Inter-sibling whitespace and
     newlines stay direct children of the tightest block/group that contains the
     boundary (never hoisted to `ROOT`), owned by neither neighbor. This is what the
     inline trivia bumps in `grammar.rs` already do (`skip_trivia` between siblings).
   - **Exception: a contiguous run of `%` comments immediately preceding a
     documentable construct attaches *leading* into that construct**, so the comment
     binds to the command/environment/sectioning node it annotates—exactly ra's
     `n_attached_trivias` (comments attach forward to item-like nodes). "Documentable"
     is decided **purely on node kind**—any `COMMAND` or `ENVIRONMENT`—so no
     signature-DB lookup leaks into the parser (sectioning commands are `COMMAND`
     nodes, covered without special-casing). A *same-line trailing* comment
     (`\foo % x`, no newline before it) is **not** leading and never binds.
   - **A blank line (`≥2` newlines, the `\par` boundary) breaks the bind:** comments
     past a blank line stay floating, never leading. Mirrors ra's `"\n\n"` cutoff.
     The binding run is the *maximal blank-line-free suffix* of the preceding trivia
     that starts at an own-line comment (so in `%a \n\n %b \foo`, `%a` floats and
     `%b` binds).

   Plain whitespace trivia stays **bare leaf tokens**, never wrapped in a node —
   the token *kind* already marks it skippable (`Parser::is_trivia`), matching
   arity/ra and keeping `tree_builder` a mechanical replay. The one *named*-node
   exception is the bound leading-comment run: it is grouped into a `DOC_COMMENT`
   node (the construct's first child) so downstream (LSP/formatter) sees the doc
   comment as one unit. The node groups only the contiguous *bound* run; a margin/
   guard or an unbound floating comment is never wrapped. There is no
   parse-stability invariant, so this policy is a CST-shape *convention* enforced
   by tests, not a hard oracle. The leading comment-bind is implemented
   **grammar-locally** (`grammar.rs` `binding_run` + the `precede` idiom, the run
   wrapped via `open(DOC_COMMENT)`/`close`), so `tree_builder` stays a mechanical
   replay; the construct self-opens and its `Start` is pulled back over the
   `DOC_COMMENT`. The doc/ltxdoc *semantic* association of a doc comment with the
   macro it documents in a `.dtx` (where the documentation lives behind floating
   `DOC_MARGIN` trivia, not `COMMENT` tokens, so nothing binds) is a deferred
   semantic-layer query, not a parser concern—keeping decision #2's no-meaning-
   in-the-parser rule intact.

## Invariants (these are test oracles—enforce them)

- **Losslessness:** `reconstruct(text) == text`, byte-for-byte. Enforced day one.
- **Idempotence:** `fmt(fmt(x)) == fmt(x)`.
- **Protected regions** (`verbatim`, `lstlisting`, `\verb`, comments) are never
  altered by the formatter.

There is deliberately **no** parse-stability invariant (`parse(fmt(x))`
structurally equal to `parse(x)`). The formatter is allowed to *normalize*
structure—e.g. stripping redundant braces around a single-token math script
(`x^{2}` → `x^2`) changes the CST shape on purpose. Such rewrites must preserve
*meaning* (a correctness requirement carried by fixtures and the corpus, 
but they are not held to structural equality with the input.

The formatter is intentionally used to stress the parser: any formatter ambiguity
should surface a parser modeling gap. Lean on this loop.

**Differential oracle** (steal arity's `air_compat` pattern): use **texlab's
parser** as a differential *parse* oracle over a corpus. It is an external
reference implementation we measure against, never match.

## Technology choices (aligned with arity's Cargo.toml)

- **rowan** (`0.16`): lossless CST. **salsa** (`0.26`): incremental queries.
- **smol_str** for interned token text; **insta** for snapshot tests.
- **LSP:** `lsp-server` + `lsp-types` (rust-analyzer's own stack), **not**
  `tower-lsp-server`. This is the one place badness deliberately diverges from arity.
  Reason: salsa cancellation is a synchronous unwind (`salsa::Cancelled`) under a
  single-writer/snapshot-readers model—it composes cleanly with `lsp-server`'s
  sync main loop + threadpool (exactly how ra does it) and fights tower-lsp's async
  `&self` model. Reuse arity's `text/line_index.rs` logic but swap its
  `tower_lsp_server::ls_types::Position` for `lsp_types::Position`.
- **Formatter engine:** a Wadler/Prettier-style `Doc` IR
  (`Group`/`Line`/`SoftLine`/`HardLine`/`EmptyLine`/`Indent`, flat/break fit)—copy
  arity's `formatter/ir.rs` + `printer.rs` nearly wholesale. **Addition over arity:**
  an `Ir::Fill` node (Wadler/Prettier *fill*: per-gap greedy break decisions) for
  paragraph reflow—arity formats R and has no prose-wrapping, so this primitive is
  badness-specific. Keep the rest of the engine close to arity's.
- **Paragraph line breaks** are controlled by a `WrapMode` (`Reflow` default,
  `Sentence`, `Semantic`/sembr, `Preserve`), modeled on the sibling **panache**
  formatter's mode taxonomy. badness mechanizes it through the `Doc` IR (`Fill`),
  *not* panache's separate streaming line-filler—the printer stays the single
  layout authority. `Reflow` and `Preserve` are implemented; `Sentence`/`Semantic`
  currently fall back to `Preserve`. The `\\` line break (with a tightly-bound
  `*`/`[len]`) is grouped by the *parser* into a `LINE_BREAK` node—a formatter
  ambiguity (the orphaned `[2ex]`) driven back into the parser per tenet 3, so the
  formatter sees `\\[2ex]` as one unit instead of splitting it.
- **CLI:** `clap` + `build.rs` generating man pages, completions, and markdown
  (`clap_mangen`, `clap_complete`, `clap-markdown`)—copy arity's scaffolding.
- **Diagnostics rendering:** `annotate-snippets`.

## Relationship to arity (copy now, extract later)

Decision: **bootstrap badness by copying arity's language-agnostic skeleton, then
extract a shared crate once badness's formatter works and the boundaries are proven.**
Premature extraction is the bigger risk while badness is empty.

**Copy ~wholesale (language-agnostic)—mark each as an EXTRACTION CANDIDATE:**
- `formatter/ir.rs` + `formatter/printer.rs` (the Wadler engine—extract first)
- `text/line_index.rs` (swap the LSP `Position` type—see LSP note above)
- `parser/events.rs` + `parser/tree_builder.rs` (generic given SyntaxKind + Token)
- `incremental.rs` salsa harness shape
- `config`, `file_discovery`, `linter/suppression`, diagnostic rendering, `build.rs`

**Rewrite for LaTeX (the genuinely different part):** `parser/lexer.rs`,
`parser/expr.rs` + `structural.rs`, `syntax.rs` kinds, `ast/`, `semantic/` scoping,
and the signature DB (analog of `rindex/`).

**Diverge from arity on purpose:** `lsp.rs`—badness uses `lsp-server` (see LSP
note); do *not* copy arity's tower-lsp-server loop.

When you touch a copied file, keep it close to arity's version so the eventual
extraction stays a mechanical lift, not a merge.

## Non-goals (keep these true)

- No general macro expansion; no TeX evaluator; no execution of TeX primitives or
  arbitrary `\def` semantics. (Common `\newcommand`/`\newenvironment`/`xparse`
  *signatures* may feed the semantic DB—extracted, never executed.)
- No general `\catcode` handling beyond the bounded, statically-recognizable
  patterns named in decision #1.
- We are not a TeX engine; we never typeset.

## Repo conventions

- Rust edition 2024; the toolchain is defined by `rust-toolchain.toml` (the
  single source of truth, currently `stable`), which `devenv.nix` consumes via
  `toolchainFile` and CI honors as the override.
- A `wasm32-unknown-unknown` target is configured (web/playground is in scope).
- `rustfmt` runs as a git hook; keep code rustfmt-clean. **Run `cargo fmt`
  before committing**—the hook rewrites unformatted files and aborts the
  commit, so committing dirty means a second attempt. `clippy` warnings are
  errors (`cargo clippy --all-targets --all-features -- -D warnings`).
- Performance is first-class: `perf`, `cargo-flamegraph`, `hyperfine`,
  `cargo-show-asm`, `cargo-llvm-cov` are in the dev shell. Benchmark before
  optimizing; never regress losslessness for speed.
- Task runner: `go-task` (`Taskfile.yml`, mirror arity's targets).
- **Windows CI (line endings + URIs)—these bite repeatedly:**
  - *Line endings.* The formatter always emits **LF**, and tests compare its
    output byte-for-byte against checked-in `expected.*` fixtures. With Git's
    `* text=auto`, any fixture extension not pinned to `eol=lf` in `.gitattributes`
    is checked out **CRLF** on Windows, so the comparison fails there but passes on
    Linux/macOS. **When you add a fixture in a new language (a new extension under
    `tests/fixtures/**` or `tests/corpus/**`), add a matching `… eol=lf` line to
    `.gitattributes`** (currently `.tex`, `.bib`, `.sty`, `.cls`, `.dtx`, `.ins`,
    plus the corpus and snapshots). The deliberate exceptions are the
    `*_crlf_*`/`*_lf_*`/`crlf_*`/`lf_*` line-ending fixtures, pinned `-text` so they
    keep their bytes. Never normalize line endings *in code* to make a test pass —
    fix the attribute.
  - *URIs.* LSP document URIs are decoded to filesystem paths through
    `uri_to_fs_path` (`lsp.rs`), which handles the Windows `file:///C:/…` form:
    the leading `/` before a drive letter is URI syntax, not part of the path, and
    is stripped (`strip_drive_letter_slash`); on Unix the leading `/` is the root
    and stays. Keep both the Unix and Windows cases in
    `uri_to_fs_path_handles_unix_and_windows` green when touching URI/path code, and
    don't hand-roll `file://` parsing elsewhere—go through `uri_to_fs_path`/
    `path_to_uri`. Paths in tests/snapshots must not assume `/` vs `\\`.
- The bib field/entry DB (`data/bib_fields.json`) tracks **biblatex's canonical
  data model** (`blx-dm.def`). `scripts/gen_bib_fields.py` keeps the *mechanical*
  facts (entry-type set, field categories, `required` constraints) in sync with the
  installed biblatex, preserving the hand-curated `optional` ordering and the
  classic-BibTeX overlay. `task bib-fields:check` reports drift (run after a biblatex
  bump); `task bib-fields:sync` applies it. Don't hand-edit those mechanical facts —
  change them via the model and re-sync.

## Working agreements for agents

- Match surrounding (and arity's) idioms, naming, comment density.
- New parser features need corpus + snapshot tests *and* a losslessness assertion.
- Keep the syntactic layer free of semantic knowledge.
- Don't add intra-file incremental reparse, macro expansion, or catcode logic beyond
  decision #1 without recording the decision here.
- Update TODO.md as phases progress; update this file when a decision changes.
