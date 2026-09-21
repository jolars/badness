# Correctness and performance evidence

Use targeted tests while changing code, then `task check` for a substantial
change. It runs workspace tests, Clippy, formatting checks, smoke-script tests,
and the wasm build. Run `task fmt` before committing Rust changes.

## Match checks to the changed path

- **LaTeX parser:** `cargo test -p badness-parser`, including snapshots, round
  trips, property losslessness, and incremental-reparse tests. For parser/CST
  changes that can affect concordance, follow
  [parse-compat](../../parse-compat/SKILL.md). Preserve error vectors as well as
  source reconstruction.
- **BibTeX parser:** include the crate's `bib_parser`, `bib_roundtrip`, and
  `bib_parse_oracle` suites, and follow
  [bib-parse-compat](../../bib-parse-compat/SKILL.md) for parser/CST changes.
- **Formatter:** `cargo test -p badness-formatter` and representative real
  inputs. Compare baseline and candidate output byte for byte; retain trivia
  safety, protected regions, and idempotence. Run `task typeset:check` when
  changing keyval signature behavior or optional-argument lowering.
- **Linter:** `cargo test --test lint --test bib_lint --test cli_lint`, plus
  focused rule tests. Compare rule IDs, severity, spans, and offered/applied
  edits. Fix output must preserve meaning, not merely parse and reconstruct;
  check it without relying on formatting to repair it.
- **Incremental or LSP:** `cargo test --test incremental --test lsp` and
  `cargo test -p badness-parser --test incremental_reparse`. Keep staleness,
  invalidation, text/line-index pairing, and staged edit chains intact.

A changed output snapshot is a correctness failure by default in performance
work. Investigate it rather than bulk-accepting snapshots. If a separately
requested behavior change explains it, document and test that change explicitly.

## Incremental and memory gates

For reparse-path changes, run `task bench:gate` before and after on an idle
machine with the full pinned corpus. Run `task reparse-corpora:check` when
changing reparse admission, tiers, or locality proofs; investigate changed
splice tallies rather than automatically recording a new baseline. New tiers
also need the exact-tier, speedup-floor, release-equivalence, and seeded-corpus
evidence required by `AGENTS.md`.

For retained-history memory changes, run `task bench:lsp-memory-gate` with its
paired controls. These local gates are distinct from `task check`; keep their
timing and retention thresholds in the harnesses rather than adding duplicate
assertions elsewhere. Do not run them on shared CI as timing gates.

When adding a cache or changing its lifetime, verify retention with paired
histories that exercise the affected production path. Populate the cache and
trigger invalidation through repeated edits or project changes, as applicable.
Compare live allocations at equivalent final states with the cache populated in
both histories. An existing memory gate counts as this check only if its
scenario exercises that cache.

## Evidence to retain

Record the exact input and configuration, revisions, commands, compiler/build
settings, CPU allocation, warmups, run count, median, spread, and percentage
change. For LSP requests include p95 and returned work; for memory include
baseline, settled, and peak values or the paired live-heap delta, as applicable.
Check a contrasting workload for a plausible tradeoff, such as large versus
small files, accepted versus declined edits, or single-file versus project work.

Report checks that could not run and why. If committing, keep measured changes
separable and include the relevant before/after result in the commit body.
Update published benchmark artifacts through the bench workflow only when
requested; an investigation does not require rewriting those artifacts.
