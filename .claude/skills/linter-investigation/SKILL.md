---
name: linter-investigation
description: Investigate badness's linter (and, secondarily, its parser) against
  a real-world LaTeX codebase. Clone a target repo, lint it, and triage the
  diagnostics for false positives, incorrect spans, and unsafe autofixes (fixes
  that change typeset output); parse failures on valid LaTeX are caught along the
  way. Suspected bugs are confirmed against the texlab differential oracle (and a
  real compile via `lualatex`/`pdflatex` when needed) before being called bugs.
  Use when asked to stress-test, investigate, or triage the linter (or parser)
  over an external repo or corpus.
---

Point badness's linter at a large body of real LaTeX and hunt for **linter
quality bugs**: false positives, incorrect spans, and unsafe fixes. This is the
primary goal. **Parse failures are a secondary catch**—a parse error blocks
linting a file, so they surface naturally, and a parse failure on *genuinely
valid* LaTeX is a real parser bug worth reporting—but the center of gravity is
the linter, not a full parser audit.

This is **distinct from the `smoke-test-triage` skill.** That one reacts to the
weekly automated corpus scan's *formatter* regressions (losslessness,
idempotence, format-error, panic) filed as GitHub issues. This skill is
proactive and interactive: you choose a repo and go looking for linter/parser
quality problems. Formatter losslessness and idempotence are out of scope
here—leave them to `smoke-test-triage`.

## The core principle (read first)

**A finding is only a bug once you can show badness is wrong—and in LaTeX,
"wrong" is subtle.** Validity is *catcode- and package-dependent*: a file that is
perfectly valid inside its project (with its preamble, class, and `\catcode`
rebindings) may not parse standalone, and per badness's `AGENTS.md` non-goals,
that is *expected*, not a bug. Real corpora are full of `.dtx`/`.sty` internals,
`expl3` code, and conditional branches that a generic parser deliberately does
not fully resolve. Classify each suspicious finding into exactly one of:

- **True positive** — badness is right; move on.
- **False positive** — badness flags legitimate LaTeX. The highest-value find.
- **Incorrect span** — the finding is real but the caret underlines the wrong
  tokens.
- **Unsafe fix** — an autofix that changes *typeset output* (LaTeX is
  whitespace- and catcode-sensitive), breaks the source, or drops trivia. Test
  it; `--unsafe-fixes` fixes especially warrant scrutiny.
- **Parser bug** — *genuinely* valid LaTeX that badness fails to parse or
  mis-parses. Confirm with the oracle, and rule out the catcode/preamble
  caveats first.

## The oracle (texlab is not installed; a TeX engine is)

- **texlab differential oracle** — badness's parser is measured against texlab's
  (a dev-dependency; no separate install needed). Run `task parse-compat`
  (writes `PARSE_COMPAT.md`) for LaTeX and `task bib-parse-compat` (writes
  `BIB_PARSE_COMPAT.md`) for BibTeX. This is the primary ground truth for parse
  concordance; see `tests/parse_compat.rs` / `tests/bib_parse_compat.rs`.
- **badness's own view** — `badness parse <file>` prints the lossless CST for
  shape comparison. (Losslessness, `reconstruct(text) == text`, is asserted by
  the parse-compat and corpus test suites, not a CLI flag.)
- **A real compile (heavy, use sparingly)** — `lualatex`/`pdflatex` are
  installed. Compiling a *self-contained* minimal document is the only true
  semantic oracle, but it's slow and needs a full preamble, so reserve it for a
  claim you can't settle otherwise. Never compile a bare fragment lifted out of
  its project and conclude "invalid" from the failure—that's the catcode trap.

Most linter false positives are settled by *reading the LaTeX* plus badness's own
parse tree; the TeX engine is a last resort for genuine ambiguity.

## Workflow

1. **Target.** Take the repo from the user's argument (GitHub `owner/name`, clone
   URL, or local path). If none is given, propose a good default (`latex3/latex2e`
   or `pgf-tikz/pgf` for breadth; `HoTT/book` or `stacks/stacks-project` for
   large real documents) and confirm before cloning.

2. **Setup (parallel/background).** Build the release binary and shallow-clone
   into the **session scratchpad directory** (not bare `/tmp`), at once:

   ```sh
   cargo build --release
   git clone --depth 1 https://github.com/<owner>/<name>.git "$SCRATCH/<name>"
   ```

3. **Lint the tree, capture everything.** Capture both streams (`lint` exits
   non-zero when it reports anything):

   ```sh
   target/release/badness lint "$SCRATCH/<name>" >lint.out 2>lint.err
   ```

   badness lints `.tex` (LaTeX pipeline: `.tex/.sty/.cls/.dtx/.ins`) and `.bib`
   (BibTeX pipeline). If an unreadable file aborts the run, move it aside and
   re-run.

4. **Summarize by rule.** Count findings per rule to prioritize the high-volume
   and high-risk buckets:

   ```sh
   grep -oE '(warning|error): [a-z-]+' lint.err | sort | uniq -c | sort -rn
   ```

5. **Triage (the heart of the work).** For each priority rule, pull real findings
   (`grep -B1 -A6 'warning: <rule>' lint.err`), open the cited source line, and
   **reduce each suspect to a minimal reproducer** piped to the tool:

   ```sh
   printf '...\n' | target/release/badness lint
   printf '...\n' | target/release/badness parse            # inspect the CST
   ```

   For a suspected parser bug, **isolate the trigger by bisecting context**
   (which environment, math vs text mode, which macro/catcode), varying one axis
   at a time until the minimal failing shape is pinned—then confirm the shape is
   valid *without* relying on hidden preamble state.

6. **Verify against the oracle.** Promote a suspicion to a bug only after texlab
   concordance (or, rarely, a self-contained compile) agrees, and only after the
   catcode/preamble caveats are ruled out.

7. **Fan out for volume (recommended).** For a big finding set, spawn parallel
   triage subagents—one per rule-bucket—each given the absolute
   `target/release/badness` path, the `lint.err` path, the classification scheme
   (with the LaTeX validity caveats spelled out), and the oracle recipe. Each
   returns minimal reproducers, per-category verdicts, and an FP-rate assessment.

8. **Fix or record.** For the cleanest, well-isolated bugs, fix TDD-style,
   honoring badness's tenets (parser bugs fixed in the parser; losslessness
   sacred; the formatter must not alter verbatim/protected content):

   - Add a failing fixture first and **watch it fail**, following badness's
     `add-lint-rule` / parser-fixture conventions (reduce from the corpus).
   - Fix at the root cause; re-verify against the oracle.
   - Run the gates: `cargo test`, `cargo clippy --all-targets --all-features --
     -D warnings`, `cargo fmt -- --check`; `cargo insta accept` after reviewing
     new snapshots; re-run `task parse-compat` if the parser changed.

   Record everything you don't fix as follow-ups in `TODO.md` in the house style,
   each with a minimal reproducer and why it's valid LaTeX. Commit only if the
   user asks—atomic, Conventional Commits.

9. **Report back.** State plainly: bugs found (fixed vs. documented) with
   copy-pasteable reproducers; false-positive categories per rule; incorrect-span
   issues; which rules you verified clean; and follow-ups recorded. Be explicit
   about which suspects you *dismissed* as catcode/preamble-dependent rather than
   real bugs.

## badness-specific notes

- **The catcode trap is the whole game.** Most "parse failures" over a real
  corpus are legitimately-skipped conditionals, `\catcode` rebindings, or
  balance hidden in an unexpanded macro—badness's documented non-goals, not bugs.
  Confirm a fragment is valid *standalone* before blaming the parser.
- **Unsafe fixes change typeset output.** LaTeX is whitespace- and
  catcode-sensitive; a fix that looks cosmetic can shift spacing or a line break
  in the PDF. `--unsafe-fixes` edits deserve the most scrutiny; test by applying
  and re-parsing, and reason about typeset impact.
- **Two pipelines.** `.tex`-family vs `.bib`; they have separate parsers and
  separate oracles (`parse-compat` vs `bib-parse-compat`). Keep findings and
  reproducers on the right side.
- **texlab is not installed as a binary**—the differential oracle runs *inside*
  the cargo test via the dev-dependency, so use `task parse-compat`, not a
  `texlab` CLI. A TeX engine (`lualatex`/`pdflatex`) *is* installed for the rare
  self-contained compile.
