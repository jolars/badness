# Badness TODO

A LaTeX formatter, linter, and language server on a lossless rowan CST,
following **rust-analyzer's** architecture. See `AGENTS.md` for load-bearing
design decisions and invariants.

A cargo workspace: the `badness` root crate (CLI, linter, language server,
project/configuration) plus `badness-parser`, `badness-formatter`, and
`badness-wasm`. Parser and formatter are **intentionally interleaved**: the
formatter is the primary tool for stress-testing the parser.

Status: `[ ]` todo · `[~]` in progress · `[x]` done

## Parser

- [ ] **Keep carving `grammar.rs`** (4,331 lines; the first cut took
  `grammar/facts.rs`, `grammar/trivia.rs`, and `grammar/prescan.rs`, and
  `grammar/expl3.rs` came out later). Two candidates remain, each its own
  commit:

  - The **math / `\left…\right` sublanguage** (`dollar_math` through
    `stray_right`, plus `split_math_word`), ~460 lines and highly
    self-contained. `math_environment_body` currently sits in the environment
    section and is the one routine the split has to decide about.

  - The **gate machinery** (`WalkKey`, `GateBatch`, `VerdictSink`, the policy
    vocabulary, `trait GatePolicy`, and the nine gate policies), ~805 lines.
    It drags the `scan_work` linearity tests along.

  The rest of the hygiene item is done: the shadow counters, the DOC_COMMENT
  precede dedup (`precede`/`extend_back`/`doc_comment_bind`), the `PreScan`
  extraction, the `math_atom` EOF tripwire, the environment-delimiter helpers,
  `BLANK_LINE_NEWLINES`, the `is_trivia` reuse, the borrowing `peek_end_name`,
  and the stale `parser.rs` module doc. Still open from that note: promoting
  `precede` into the event layer as a real rust-analyzer `Marker` with a
  `DropBomb`, which is a mechanical diff across every `open`/`close` site.

## Formatter

- [ ] **Extend inline-command argument glue to `.dtx` margin prose.** The
  `inline_command_argument_glue` rule deliberately preserves pre-argument trivia
  in margin-carrying and virtual documentation streams. Enabling it there adds
  idempotency and trivia failures for `bm.dtx` and `longtable.dtx`; reduce those
  interactions and fix the structural margin boundary before widening the rule.

- [ ] **Decide whether math control-word spacing should use the lexer's ASCII
  alphabet.** The formatter's `is_control_word_letter` (`formatter/core.rs`)
  uses `char::is_alphabetic`, while the lexer uses `is_ascii_alphabetic`, so
  `\alphaβ` becomes `\alpha β`. Unlike the mode-specific `@`, `_`, and `:`
  mismatch, this is a readability judgment rather than a defect.

- [ ] **Widen mandatory-keyval admission (follow-up to the `{…}` segmentation).**
  `ContentKind::Keyval` on a *mandatory* group is now consumed
  (`lower_segmented_group`; fixture `keyval_group_splits_entries`), so the setters
  `\pgfkeys`/`\tikzset`/`\lstset`/… take one entry per line instead of a prose
  reflow that wrapped mid-key. One lower-confidence admission question remains:

  - The bulk CWL tier still drops a `%keyvals` mark on a `{…}`
    (`scripts/gen_cwl_signatures.py`, `_parse_arg_shape`). The reason it gave —
    "nothing consumes the flag there" — has expired, but the other half has not: the
    mark is mechanical, and a wrong `Keyval` on a mandatory group changes typeset
    output where the same mistake on a bracket is contained. Lifting the scoping
    means first *measuring* which names would gain it (needs the pinned CWL
    source) and putting the textual ones through `task typeset:check`.

  The environment half is done: `lower_begin` now routes a matched mandatory
  keyval slot through `lower_segmented_group`, and the curated `tblr`, `longtblr`,
  and `talltblr` signatures declare their outer and inner specifications as
  keyval (`environment_keyval_group_splits_entries`). They deliberately do not
  set `align`, whose column reader expects a raw colspec group; top-level `&`
  still selects the structural grid path. The real tabularray case is covered by
  `tests/typeset/keyval_mandatory.tex`.

  This entry also sets the reach of the `blank-line-in-keyval` rule, which reads
  only the hand-curated tier: any name admitted here is a name that rule starts
  protecting.

- [ ] **Formatter-owned trailing comma (parked; the last piece of issue #47).**
  A `[…]` — and, since the segmentation above, a proven-keyval `{…}` — is a
  width-driven group over its top-level entries, and a
  `ContentKind::Keyval` argument may also break at a glued comma
  (`docs/src/development/architecture.md` § *Optional arguments, tables, and math spacing*). What is left of the old parked
  item is the Black-style trailing comma: for a proven-keyval argument, add the
  `,` when expanded and drop it when collapsed — safe as *TeX*, because
  keyval/xkeyval/pgfkeys/l3keys and `\ProcessOptions` clists all ignore empty
  entries. **Blocked on an invariant, not on data:** inserting or deleting a `,`
  is a non-trivia token edit, which the whitespace-only invariant forbids and
  `assert_format_invariants` actively catches. Landing it means amending that
  invariant and its oracle to carve out this one insertion — a decision worth
  taking on its own, not as a ride-along. The count-based *expansion* half was
  declined: width alone is already canonical, and an N-key threshold would need
  the comma count to proxy for keyval-ness, exploding comma-rich textual
  optionals. The Black/Ruff *magic trailing comma* (a trailing `,` in the
  **source** forcing one-key-per-line) stays declined too — content steering
  layout conflicts with the formatter-is-sole-authority tenet.

- [ ] Widen the prose-argument table (CWL ingest could feed it). The block half
  of the signature widening landed as `CommandSig::block`; the inline half now
  glues every matched argument slot in ordinary prose and prose-argument reflow
  in `expand_inline_prose`, independent of whether the author used spaces or
  newlines
  (`inline_command_argument_glue`). CWL ingest could still broaden the table.

- [ ] **Key-value continuation indent in an expl3 fallback statement (open scope
  call).** A key whose value continues on the next line should indent the value
  one step, which is what an author writes and what upstream overwhelmingly does
  (a sweep of latex3's `.dtx` sources: of 87 code lines ending in `=` with the
  value on the following line, **65 indent it by +2**, the rest split between 0
  and incidental alignment):

  ```tex
  ,begin-vspace:e =
    \tl_if_empty:nTF {#2}
      { \newtheoremstyle@vspace@default }
      {#2}
  ```

  Badness neither produces nor preserves this: it emits the continuation *level
  with the key*, discarding an authored `+2`. The cause is structural, not a
  layout bug. `,begin-vspace:e = ` is a **fallback** statement (no derivable
  arity), and in a fallback stream a newline is a statement *boundary* — the
  Tier-2 residue. So `\tl_if_empty:nTF …` on the next line is a *sibling
  statement* at the same base indent, not a continuation of the entry; and
  indentation is always computed, never read, so the author's step is dropped.

  Emitting +2 needs the formatter to know that `,key = ` is an *incomplete*
  entry — key-value modelling inside a stream it explicitly declines to model.
  The narrowest form is a rule like "a fallback statement whose last non-trivia
  token is `=` hangs its successor +2", which reads only non-trivia token
  content and is therefore trivia-invariant and permissible. **The open call is
  whether to have it at all**: the l3styleguide is silent on key-value dialects,
  so this is badness inventing layout for a dialect it cannot name. Upstream's
  65/87 gives the rule an empirical basis; the tenet-#1 pressure is that a
  `Keyval` content kind (see the trailing-comma entry above) would be the
  principled carrier, not a token-shape heuristic in `lower_expl_code`.

  Surfaced while fixing the sibling-coupling and all-or-nothing conditional bugs
  (issue #101); the conditional fix removed the *worse* half of this shape (the
  branch list no longer splits across two indents), leaving only the
  continuation indent itself.

- [ ] **Fold the linter's pgf picture set into the `statementBody` flag.**
  `linter::rules::is_pgf_picture_environment` (which keeps `dash-length` off
  coordinate arithmetic) and `data/signatures.json`'s `statementBody` now curate
  the same family twice — and the flag now has *three* readers (formatter
  routing, the parser's statement mode, and the linter's duplicate set), which
  strengthens the case for the fold. Merging them needs the effective signature
  scope on `RuleContext`; its lazy `user_definitions` database contains only
  file-local scans, not built-ins, project declarations, or loaded-package
  signatures. Until then the two carry cross-references and must be edited in
  step. Note the sets are not quite identical by design: the flag also names
  `scope` and `pgfonlayer`, which the linter reaches through the enclosing
  `tikzpicture` on its ancestor walk.

## Linter

### Issues

- [ ] **Prose `dash-length` FPs on index-pair and term names.** `0-1 law`,
  `1-2 plane`, `1-1 function` (22 of 25 findings in the cam-notes sweep) use an
  intentional hyphen. These are `Unsafe`-gated, so `--fix` withholds them and
  they are noise rather than corruption. Distinguishing `0-1 law` from
  `pages 5-10` statically is the open part. (The *corrupting* half of this —
  pgf/TikZ coordinate arithmetic under `--unsafe-fixes` — is fixed by the shared
  `in_pgf_picture` and `in_pgfmath_argument` gates.)

## Language server

### Feature status vs LaTeX Workshop

A second reference diff, against **LaTeX Workshop** (the dominant VS Code LaTeX
extension). It is not an LSP: its intellisense, hover, and outline are
regex-driven extension code, and its formatting and linting shell out
(latexindent/tex-fmt, ChkTeX/lacheck), so badness already leads on language
smarts. Coexistence is the deliberate story (docs `guide/editor-setup.md`):
LaTeX Workshop keeps build, PDF preview, and SyncTeX. The features it has that
badness lacks and wants are filed in the sections below, tagged *(LW)*: command
argument placeholders, graphics hover preview, a texmf bib fallback, and
surround/promote-demote code actions.
Math-preview-on-hover is the one big item needing a design decision (see
`### Hover` and Open decisions). Not adopted: `@a`-style abbreviation snippets
and two-letter environment snippets (editor-snippet territory), graphics
thumbnails inside completion items (VS Code-only), and sub/superscript history
completion (niche).

### Configuration & sync

- [ ] `workspace/diagnostic` (the workspace-wide pull)—deferred: it is a
  streaming/long-poll protocol (held-open request, per-uri result ids, partial
  results) that fits the one-shot id-bound read-job model poorly. Advertise
  `workspace_diagnostics: true` and add it once that plumbing exists; editors
  drive interactive diagnostics through `textDocument/diagnostic` meanwhile.

### Completion

Badness offers command, environment, label, cite-key, bib field/type, and file
completion (`src/completion.rs`, `src/bib/completion.rs`). texlab's completion
breadth is its biggest lead (`crates/completion/providers/`); the specialized
sources below are missing.

- [ ] *(Design decision)* **Package-scoped command completion.** texlab suggests
  only commands provided by the loaded packages (a package→command component
  model). Badness's signature DB is flat (curated + CWL + scanned); scoping
  completion to `\usepackage`-loaded packages needs package→command attribution.
  Open question, not a mechanical add.

- [ ] **Command argument placeholder snippets *(LW)*, opt-in.** Environment
  completion already inserts snippet bodies with tab stops; commands could emit
  placeholders for required/optional arguments straight from the signature DB
  (`\frac{$1}{$2}`). Gate on the client's snippet capability and an editor
  setting—LaTeX Workshop's equivalent (`intellisense.argumentHint.enabled`) is
  off by default, since placeholder churn annoys as many users as it helps.

### Hover

- [ ] **Graphics preview on hover *(LW)*.** Hovering an `\includegraphics`
  argument returns hover markdown embedding the image itself
  (`![](file:///…/fig.png)`)—VS Code renders images in hover markdown. No
  rendering on our side, just a file reference: reuse the target resolution
  from `lsp/document_link.rs`; png/jpg/svg only, degrading to the resolved
  path for `.pdf`/`.eps`.

- [ ] *(Design decision)* **Math preview on hover *(LW)*.** LaTeX Workshop's
  most-loved language feature: hovering math renders it (MathJax,
  client-side); texlab lacks it too, so it is also a differentiator. Options:
  (a) skip—LaTeX Workshop covers it, and coexistence is the story; (b) render
  in the VS Code extension—breaks the thin-client principle and is VS
  Code-only; (c) server-side SVG via a Rust math renderer (ReX or similar) as
  a data-URI image in hover markdown—editor-agnostic, but ships a math layout
  engine, which is typesetting in all but name. Lean (a) for now, but leave the
  choice open until implementation constraints justify closing it; record the
  eventual rationale in the architecture documentation rather than turning
  `AGENTS.md` into a decision log.

### Code actions

- [ ] **Surround selection with environment/command *(LW)*.** LaTeX Workshop
  ships these as client-side commands; badness can host them editor-agnostically
  as code actions or `executeCommand`s alongside `changeEnvironment`
  (`lsp/code_action.rs`).

- [ ] **Section promote/demote *(LW)*.** Recursively shift sectioning levels
  across a selection (`\section` ↔ `\subsection`); the sectioning hierarchy is
  already in the signature DB, so this is a mechanical rewrite.

## Performance & hardening

- [ ] **Borrowed token text, maybe.** Tokens are `SmolStr`
  (`Token` in `parser/lexer.rs`, same in `bib/lexer.rs`), so short tokens are
  already allocation-free and the fatou-sized win (-60% lexing from `&'src str`
  tokens) does not transfer wholesale. The payoff concentrates in the
  pathologically long tokens (`VERBATIM_BODY`, `VERB`), and the conversion is
  blocked on the half-dozen sites that push constant literals instead of input
  slices (`push_env_delimiter`, the braced-verb path, the synthesized `%`
  pushes) — each provably corresponds to contiguous input bytes, so they can be
  rewritten to slice. Measure with a lexer-only bench first (arity's
  workload-stratified `benches/lex.rs` is the template); do this last, if at all.

- [ ] **Deeply nested CST construction is quadratic in rowan's cache
  rehashing.** Parser-only timing on `{` repeated around `x` grows from 60.5 ms
  at depth 1000 to 231.2 ms at depth 2000 (3.82x, debug build, enlarged test
  stack). A perf profile attributes 17.5% self-time to
  `FxHasher::add_to_hash` and 15.0% to `rowan::green::node_cache::node_hash`.
  Rowan 0.17's `NodeCache::node` carries a precomputed child hash for ordinary
  lookup, but the `insert_with_hasher` callback recursively hashes a stored
  green subtree when hashbrown grows the table; a depth chain therefore
  rehashes progressively longer prefixes. Check newer rowan releases and raise
  this upstream before adding a local cache fork. Do not disable green-node
  interning without measuring the memory regression. Once fixed, add a
  parser-only brace-depth case to `tests/scaling.rs` at `MAX_RATIO`, using a
  larger thread stack so the guard reaches its asymptotic regime.

- [ ] **Split `crates/badness-parser/tests/parser.rs` by area.** It is now 2,970
  lines and 233 tests; separate math, verbatim, comments, conditionals, and
  aliases into focused integration-test targets.

- [ ] **Mine the `latexindent` corpus for construct coverage** (human-in-the-loop,
  ongoing). Skill: `.agents/skills/formatter-fixture/`. The corpus is read as a
  coverage map — which constructs occur and in what shapes — and **latexindent
  itself is the taste reference we check each construct against**: 711 of its
  test files are named for the upstream issue that produced them, across 127
  distinct issues, so its answers carry a decade of real user pushback.
  Never a byte-target: it is an indenter that preserves author breaks and
  never touches intra-line spacing, where we reflow and own layout. But every
  divergence gets a verdict (corroborates / explained / no opinion /
  unexplained), and an *unexplained* one blocks the fixture until it is worked
  out — that is where our rule is usually wrong. Run it at default settings
  (`latexindent probe.tex`, no `-s`) on a hand-authored probe; the committed
  `*-mod*.tex` files are one YAML stack's answer with `-m` on, not its own
  judgment. Measured gaps against the 281 existing
  slugs: `items` (157 files) and bare/named brace groups are no longer thin;
  re-measure before trusting any gap list here. Beamer item
  overlays are covered by `list_item_overlay_prefix`. `mand-args` /
  `opt-and-mand-args` / `environments` yielded `begin_tail_is_body` — content the
  greedy parser attaches to `BEGIN` past the declared arity is body, not header —
  which closed a Tier-1 lone-newline read *and* a column-0 indentation bug; the
  rest of those three families is still open. `filecontents` is
  done (`filecontents_protected_body`) — it was purely a protected-region
  question, and the survey found no defect: the sharp edge it now pins is that a
  verbatim-body environment's `\begin` line must never break under width
  pressure, since it defines where the protected body starts and `filecontents`'s
  optional is `Keyval` (which elsewhere licenses a comma split).
  `environment_leading_body_command` pins that a command after a completed
  `BEGIN` header starts the indented body even when authored on the header line;
  `environment_argument_comment_barrier` pins that a trailing comment between
  declared environment arguments forces a safe, indented mandatory-argument
  continuation while a following optional remains unindented;
  `environment_special_character_names` pins full-name pairing and ordinary
  framing for names containing `@` and `*`, or spanning lexer tokens at `_`;
  `environment_omitted_optional_slots` pins positional `BEGIN` header matching:
  omitted optional slots are skipped, and the first separated group after the
  supplied arguments is body rather than another header argument;
  `environment_inline_prose_boundaries` pins that an environment expanded as a
  block closes its line before following prose, removing the prior space-versus-
  newline dependency while keeping a trailing comment on the closer;
  `display_math_prose_boundaries` pins the analogous boundary for display math
  in ordinary and proven prose, while opaque arguments preserve a glued suffix
  because inserting whitespace could change their token sequence;
  `environment_keyval_group_splits_entries` pins that tabularray's curated
  mandatory inner specification segments at top-level commas under width while
  nested commas stay sealed and comment-bearing groups take the shared keyval
  block fallback;
  `inline_command_argument_glue` pins that collapsible trivia before matched
  arguments of a curated inline prose command is removed under ordinary prose
  and prose-argument reflow, while a trailing comment remains a hard barrier;
  the remaining environment and argument shapes are still open.
  Sectioning/`headings` is done (two slugs, and the Tier-1 lone-newline
  bug that lived there). `ifelsefi` (402 files) is done too, via the
  `CONDITIONAL` node under *Parser* and eight fixtures — do not re-derive a
  formatter-only rule for it, the survey already showed every such rule is
  trivia-reading, typeset-unsafe, or lopsided.

## Editor integration

texlab bundles PDF-workflow features. Only position mapping (no typesetting by
badness) is admissible; the rest are explicit non-goals recorded here so they are
not re-proposed. Forward and inverse search ship today
(`lsp/forward_search.rs`, `ipc.rs`, `badness inverse-search`) without badness
parsing SyncTeX at all — every SyncTeX-aware viewer links libsynctex and so takes
a file and a line, never a coordinate.

- [ ] *(Design decision)* **Native `.synctex.gz` reader.** Would let forward
  search drive viewers with *no* SyncTeX support at all by resolving a page
  number (qpdfview, a browser), and report an honest `Failure` when a line
  produces no output instead of launching a viewer onto nothing. Costs a gzip
  dependency, a parser with real traps (compressed `,=` points, `Input:` lines
  interleaved mid-file, `./`-segment path matching, leaf-vs-enclosing-box lookup
  semantics), and a fixture corpus validated against the `synctex` CLI with no
  existing oracle to lean on. The seam is already in place: `SearchTarget` in,
  `ForwardSearchStatus` out — a backend behind it changes no LSP surface, no
  `[build]` key, and no config. Not worth it until a page-only viewer is a real
  target.

## BibTeX/BibLaTeX

- [ ] **`unexplained-suppression`.** Report a suppression directive with no
  `: <reason>`. Requiring reasons is a project convention rather than a defect,
  while badness currently enables every registered rule by default; either
  accept the default-on policy or first add explicit opt-in rule metadata.

- [ ] **Format suppression in `.bib`.** The `% badness-format` axis parses in a
  `.bib` `@comment{…}` and deliberately does nothing. The bib formatter is a
  canonical re-emitter rather than a trivia-only pass, so "reproduce this span
  byte for byte" is a genuinely different mechanism there, not a matter of
  routing the resolved ranges through. Until it exists, `inert-suppression`
  reports the unsupported axis.

- [ ] **A `%`-comment directive carrier inside a `.bib` entry.** Now that a `%`
  comment exists inside an entry, the LaTeX-side carrier could work there too;
  today only the `@comment{…}` entry form does (`bib/linter/suppression.rs`).
  The grammar is already shared, so this is only a decision about what an
  in-entry comment attaches to (the field below it, presumably, matching the
  formatter's forward bind).

- [ ] **`task bib-error-compat`: biber as a `.bib` *error* oracle.** The gap the
  `%`-comment bug exposed — `bib-parse-compat` cannot see over-strictness at all,
  because texlab's bib parser has no error channel (the skill says so outright),
  so a whole family of files we wrongly refused sat in a gate baseline instead of
  failing a gauge. biber does have one: btparse reports real syntax errors, e.g.
  `ERROR - BibTeX subsystem: …, line 2, syntax error: found "author", expected
  end of entry ("}" or ")") (skipping to next "@")`, plus an `INFO - ERRORS: n`
  tally. Cross-tabulate per corpus file, `badness has diagnostics` × `biber has
  ERRORS`: agreement on the diagonal, **badness dirty + biber clean = over-strict**
  (this bug's class), badness clean + biber dirty = under-strict (we would format
  something biber rejects).

  Two constraints, both learned by trying it:

  - **Boolean per file only** — never error counts or positions. biber recovers by
    skipping to the next `@`, so it under-counts badly; in a three-entry probe it
    never reported an unterminated `@misc` at EOF at all, having swallowed it
    during recovery.
  - **Do not project `biber --tool` output onto a skeleton.** Tool mode exposes
    biber's *data model*, not its parse, and the transformation would swamp any
    real divergence: `author = {Ann Author and Bo Beispiel}` comes back as
    `{Author, Ann and Beispiel, Bo}`, `year` + `month` merge into `DATE = {2021-11}`
    (both source field names *gone*), `#` concatenation is resolved, and
    `--output-format=biblatexml` additionally explodes names into
    `<bltx:namepart>` and resolves `@string` uses away. texlab stays the right
    *structural* oracle precisely because it is coarse and syntactic; biber's job
    here is only "is this legal BibTeX".

  Placement: biber is an external binary, not a crate, so this cannot be an
  in-process dev-dependency like `texlab-parser` (and Text::BibTeX is not
  separately installed — biber bundles it). Same bucket as `task typeset:check`:
  needs a local install, runs on demand, never in CI.

- [ ] Cross-file `undefined-string`: a `@string` defined in one `.bib` and used
  in another resolves only once a project-level `@string` union exists (today
  single-file-sound, same caveat as `unused-string`).

- [ ] `unused-entry`: a `.bib` entry never targeted by any `\cite`-family
  command, project-aware behind the same closed+rooted namespace gate as
  `unreferenced-label`/`undefined-ref` (the bib linter has `unused-string` but no
  `unused-entry`). Report-only. texlab: `UnusedEntry`.

- [ ] Bib document-symbol outline completeness: `src/bib/outline.rs` surfaces
  regular entries only; consider `@string`/`@preamble`/`@comment` blocks (and a
  richer `SymbolKind`/detail).

- [ ] Shared component-finder: `ResolvedCitations` duplicates the union-find +
  component assignment from `ResolvedLabels` (`project/citations.rs`); factor one
  helper when a third consumer appears.

- [ ] **`subfiles`' `\subfix` wrapper opens the citation namespace.** The
  package's path fixer is the idiomatic way a subfile names a shared resource
  (`\addbibresource{\subfix{references.bib}}`), but a macro inside the group
  makes `nth_group_text` return `None`, so the target is `BibTarget::Dynamic`
  and the whole component goes open — silently disabling `undefined-citation`
  project-wide for exactly the projects issue #112 was about. Conservative (a
  loss of coverage, never a false positive), hence not urgent. Unwrapping it is
  a *shape* fact, not meaning — `\subfix{p}` is transparent by construction, the
  same class of static recognition as the `subfiles` class-option gate — so it
  keeps parser text-purity; the open part is where the unwrap belongs so
  `include.rs`, `document_link`, and completion cannot drift on it.

- [ ] **`project::package` does not collapse `.`/`..` in load targets.**
  `include.rs`'s resolvers now normalize lexically (`resolve_against`), so
  `\input{../shared}` and a `subfiles` parent resolve; `package.rs`'s `resolve`
  still does not, so `\usepackage{../mypkg}` never matches a member and its
  signatures stay out of scope. Benign (a missing local scope, not a wrong one)
  and a separate subsystem, so it was left out of the #112 fix. Lift the helper
  to `project.rs` when touching it.

- [ ] **Central-bib fallback via the texmf index *(LW)*.** LaTeX Workshop
  resolves `\bibliography{refs}` through `kpsewhich` (plus a `bibDirs`
  setting) for users who keep one master `.bib` in their texmf tree. Extend
  citation resolution to fall back to the read-only texmf index
  (`project::texmf`) for bib paths that don't resolve project-locally.
  LSP-only, sanctioned by the AGENTS.md environment-awareness tiers
  (completion, hover, go-to-definition); the `undefined-citation` lint and the
  CLI stay hermetic and project-local.

## CST / AST / trivia

- [ ] **[low, latent] No `SyntaxNodePtr`/`AstPtr`.** RA stashes stable node
  pointers in salsa data to re-resolve across reparses; badness sidesteps this by
  storing the `GreenNode` directly and carrying diagnostics as byte-ranges, so
  the need has not arisen. Latent: a future feature that must stash a *stable
  node identity* in a salsa query (resolving a completion/hover target to a
  specific node across edits) has no primitive for it, and byte-ranges alone do
  not survive edits.

- [ ] **Mark the free-function AST shims `#[deprecated]`** (or file the
  removal issue) once the formatter/linter call sites migrate — two parallel
  APIs for the same reads with no forcing function is a standing invitation
  for new code to pick the wrong one.
