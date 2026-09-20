# Measurement and profiling

Run commands from the repository root. Examples use Bash. Use the project's
devenv tools and go-task entry points where available.

## Baselines and wall time

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
hyperfine --warmup 3 --runs 20 \
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

## LaTeX parse and format

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

## Symbolized profiles

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
  taskset -c "$perf_cpu" perf record --call-graph=dwarf -F 999 \
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

## Incremental parsing and typing

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

## LSP requests and memory

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
