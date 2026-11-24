# Badness performance measurements

| Reported cost | Starting point | Limit of the measurement |
| --- | --- | --- |
| LaTeX parsing or formatting | `task bench:micro` | Separates parse, format from a built CST, and full formatting; excludes CLI startup. |
| CLI latency, linting, or BibTeX | Release CLI on the actual file or project | Includes discovery, configuration, parsing, and output rendering. |
| Typing in the editor | `task bench:keystroke` | Splice, salsa upsert, edit staging, and parse; excludes diagnostics and request transport. |
| Incremental parser tiers | `task bench:reparse` | Direct reparse and full-parse comparison with explicit tier checks. |
| Warm LSP requests or process memory | External LSP harness | Request latency and process-tree RSS/PSS after a defined session. |
| Heap retained across edits or discovery | `task bench:lsp-memory` | Paired histories ending in the same state; live allocations, not RSS. |

## Measurement and profiling

Run commands from the repository root. Examples use Bash. Use the project's
devenv tools and go-task entry points where available.

### Baselines and wall time

- Match input, configuration, file flavor, build flags, and cache lifetime
  between baseline and candidate. A default-style micro-bench cannot explain a
  regression that depends on project declarations or a different wrap mode.
- Build and save the baseline before editing. If it must be recovered later,
  use an isolated worktree at the relevant revision; do not stash or reset the
  user's working tree. Preserve any pre-existing edits in the comparison.
- Check CPU topology, allowed CPUs (`taskset -pc $$`), and background load.
  For single-threaded work, set `perf_cpu` to a quiet allowed core and use it
  consistently. Do not assume CPU 0 is allowed or is a performance core. For a
  threaded LSP or project workload, keep a representative CPU allocation and
  report it rather than silently reducing it to one core.
- Warm both binaries, then collect repeated timings. Start with roughly 20
  measured runs for small latency differences; interleave baseline and candidate
  to reduce thermal or background-load drift. Report medians and spread, retain
  raw measurements, and do not assume a universal percentage noise floor.
- Time already-built binaries. Use the normal release build for the final
  user-visible comparison; profiling flags and instrumentation can affect cost.

For example, after building with `task build-release` and setting `perf_cpu`:

```bash
mkdir -p target/perf-investigation
hyperfine --shell=default --warmup 3 --runs 20 \
  --export-json target/perf-investigation/cli.json \
  "taskset -c $perf_cpu ./target/release/badness format --no-config \
    --stdin-filepath bench.tex < benches/documents/masters_dissertation.tex > /dev/null"
```

Use the reported project's configuration instead of `--no-config` when it is
part of the problem. For directory formatting, use `--check` or a disposable
copy so repeated runs see identical input. Inspect output and exit status once
before timing: `lint` and `format --check` can return 1 for expected findings or
changes. Use hyperfine's `--ignore-failure` only after confirming the reason.
A repeated CLI loop pays startup on every iteration; it does not isolate or
amortize the lint pass. `--version` is a startup control, not an exact amount to
subtract from another command.

### LaTeX parse and format

```bash
task bench:download
BADNESS_BENCH_DOC=masters_dissertation.tex BADNESS_BENCH_ITERATIONS=60 \
  BADNESS_BENCH_OUTPUT_JSON=target/perf-investigation/micro.json task bench:micro
```

Fetch only if the needed corpus is missing. `small.tex` is committed; larger
documents are downloaded at pinned revisions. Use the user's reproducer when
the standard corpus does not exercise the problem.

`benches/formatting.rs` always uses `LatexFlavor::Document` and default
`FormatStyle`. `BADNESS_BENCH_DOC` selects a file relative to
`benches/documents/`; it does not select a language or file flavor. Pointing it
at `.bib`, `.sty`, `.cls`, or `.dtx` does not reproduce their production paths.
Use the CLI with the correct `--stdin-filepath`, or a focused harness using the
matching public entry point and resolved configuration.

The three rows are separately timed loops: parse, format from a pre-built CST,
and full parse/lower/print. Their reported values are per-iteration means;
compare distributions across repeated runs. Their percentages are attribution
hints and need not sum to 100%. A perf recording of this executable includes
all three loops, warmup, and signature initialization, so its samples are not a
profile of the full pipeline alone. Isolate the relevant loop if that distinction
affects the diagnosis. Confirm the document was measured rather than skipped
for parser diagnostics.

### Symbolized profiles

The release profile strips symbols. Enable debug information **and** disable
stripping when profiling. Select the exact executable from Cargo's artifact
message; a `target/release/deps/formatting-*` glob can select an old build.

```bash
mkdir -p target/perf-investigation
CARGO_PROFILE_RELEASE_DEBUG=true CARGO_PROFILE_RELEASE_STRIP=false \
  cargo build --release --bench formatting --message-format=json \
  > target/perf-investigation/build.jsonl
perf_bench=$(jq -r 'select(.reason == "compiler-artifact" and
  .target.name == "formatting" and .executable != null) | .executable' \
  target/perf-investigation/build.jsonl)
readelf -SW "$perf_bench" | rg '\.symtab|\.debug_info'

BADNESS_BENCH_DOC=masters_dissertation.tex BADNESS_BENCH_ITERATIONS=200 \
  taskset -c "$perf_cpu" perf record -e cycles:u --user-callchains --call-graph dwarf -F 999 \
    -o target/perf-investigation/perf.data -- "$perf_bench"
perf report --stdio -i target/perf-investigation/perf.data \
  --no-children -g none --percent-limit 1
```

Confirm both symbol and debug sections exist. Apply the same build overrides
to the CLI or another bench when that is the target. `task bench:profile` is the
masters-dissertation flamegraph shortcut; pass the overrides there too.

Read self time alongside caller/callee attribution (`-g graph,caller` or
`-g graph,callee`, and `--inline` when useful). Optimized DWARF stacks may be
incomplete. If using frame-pointer unwinding, rebuild with
`-C force-frame-pointers=yes`; do not interpret collapsed stacks as phase costs.
On hybrid CPUs, inspect the event and sample count for the core actually used;
do not trust percentages from a handful of samples in another PMU block.
Use an allocation profiler such as heaptrack when CPU stacks cannot identify
the allocation site, and verify any gain without its instrumentation.

If perf cannot sample under the current permissions, continue with timings and
report the attribution limit. Cargo flamegraph also relies on perf on Linux;
switching frontends does not bypass that restriction.

### Incremental parsing and typing

```bash
BADNESS_BENCH_DOC=phd_dissertation.tex task bench:keystroke
BADNESS_BENCH_CASE=phd_dissertation.tex/word task bench:reparse
```

The keystroke harness alternates insert/delete edits through the live
`TextBuffer`, salsa upsert, edit staging, and parse. Its unchanged-text row
measures a guard, not an ordinary edit. `BADNESS_BENCH_SITE` selects `word`,
`verbatim`, or `decline`; `BADNESS_BENCH_TARGET_MS` controls the timing budget.
It excludes queueing, lint diagnostics, and protocol transport.

Use the direct reparse harness to observe tiers; do not subtract keystroke rows
to estimate reparse cost. Debug builds run a full-parse oracle, so measure in
release. The release harness checks tree and error equivalence outside timing.
Include declined edits and their full-parse fallback when assessing editor
latency. Both benches support `BADNESS_BENCH_OUTPUT_JSON`.

Run their gates with the complete pinned corpus, default timing budgets, and
no document/case filters. Thresholds and exact-tier expectations live in the
harnesses. Do not lower them to accommodate a regression or a loaded machine.

### LSP requests and memory

For the existing external session comparison, write an investigation artifact:

```bash
./benches/compare_lsp_memory.sh --out target/perf-investigation/lsp.json
```

Read the script and `benches/lsp_memory_compare.py` before adapting the session.
They measure warm symbols, hover, definition, references, and rename, plus
whole-process-tree RSS/PSS on Linux. The wrapper builds the current release
binary; A/B comparisons of saved binaries require configuring the Python
driver. Preserve returned work as well as latency so an empty result cannot
appear to be a speedup. Other request types need their own reproduction.

For memory that grows with edit or project history, use
`task bench:lsp-memory`. `benches/lsp_memory.rs` compares paired histories with
identical final buffers and project membership using a counting allocator.
`BADNESS_MEMORY_SCENARIO` selects `all`, `query-log`, or `project`;
`BADNESS_MEMORY_OUTPUT_JSON` saves results. Its live-heap delta excludes free
pages retained by the allocator and is not interchangeable with RSS or PSS.
Check both controlled retention and real-session memory when the report calls
for both.

## Correctness and performance evidence

Use targeted tests while changing code, then `task check` for a substantial
change. It runs workspace tests, Clippy, formatting checks, smoke-script tests,
and the wasm build. Run `task fmt` before committing Rust changes.

### Match checks to the changed path

- **LaTeX parser:** `cargo test -p badness-parser`, including snapshots, round
  trips, property losslessness, and incremental-reparse tests. For parser/CST
  changes that can affect concordance, follow
  [parse-compat](../.agents/skills/parse-compat/SKILL.md). Preserve error vectors as well as
  source reconstruction.
- **BibTeX parser:** include the crate's `bib_parser`, `bib_roundtrip`, and
  `bib_parse_oracle` suites, and follow
  [bib-parse-compat](../.agents/skills/bib-parse-compat/SKILL.md) for parser/CST changes.
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

### Incremental and memory gates

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

### Evidence to retain

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

## Badness hotspot leads

Read after locating the hot phase. Historical percentages in
`benches/README.md` describe an earlier run, not the current ranking.

| Measured cost | Candidate explanation and next check |
| --- | --- |
| Rowan cursor traversal | Repeated child scans or materialization in lowering and lint dispatch. Confirm the same wide node is visited repeatedly before consolidating passes. |
| Allocation or collection growth | Trace to the allocation site. Borrow, remove an intermediate collection, or reuse local capacity when its lifetime fits. A size pre-count adds a scan and can cost more than growth. |
| Lexer scanning or string operations | ASCII delimiter scans may use bytes, but token contents are arbitrary UTF-8. Preserve Unicode whitespace and character boundaries wherever the existing semantics require them. |
| Green-tree construction | Check lexer/events/tree-builder work and token sizes before changing CST shape or interning. Investigate the nested-tree rehashing note in `TODO.md` if cost grows with depth. |
| Formatter lowering or printing | Check repeated IR construction and width measurement. Width depends on current column, gaps, and print mode; memoizing by text alone can invalidate flat-fit proofs. |
| Lint dispatch or rules | Attribute work to a rule or shared index. Keep shared node-shape, whole-file, and streaming dispatch; do not introduce a tree walk per rule. |
| Salsa or text updates | Check unnecessary writes, line-table rebuilds, declaration publication, and invalidation. Preserve paired text/index storage, edit staging, and ordered writes. |
| Growing LSP heap | Distinguish current project data, retained query/history data, and allocator pages using the two memory harnesses. A smaller single-shot parse does not establish bounded retention. |

Do not pool rowan `NodeCache` or `GreenNodeBuilder` across independent parses
as a benchmark shortcut. Retained green nodes change memory behavior, and a
warm cache across iterations does not model fresh CLI processes.

The previous `has_verbatim_body` scan consolidation measured within noise.
Treat it as a hypothesis to revisit only with evidence of repeated expensive
walks. Likewise, replacing `chars().count()` with a byte loop is not inherently
faster; removing repeated counts may be the actual opportunity.

### Source map

- LaTeX parser: `crates/badness-parser/src/parser/`, especially `lexer.rs`,
  `grammar.rs`, `events.rs`, `tree_builder.rs`, and `reparse.rs`.
- LaTeX formatter: `crates/badness-formatter/src/formatter/`, especially
  `core.rs`, `ir.rs`, and `printer.rs`.
- Linter: `src/linter/check.rs`, `rules/`, `fix.rs`, and `render.rs`.
- BibTeX: `crates/badness-parser/src/bib/`,
  `crates/badness-formatter/src/bib/`, and `src/bib/linter/`.
- Incremental analysis and LSP: `src/incremental.rs`, `src/text/`, and
  `src/lsp/`. Follow the measured request to its handler before editing.
