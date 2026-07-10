---
name: profile
description: Profile-driven performance work on the badness parser, formatter, or
  linter. Measure first with the in-process micro-bench + perf/flamegraph on a
  real document; classify hotspots into a small set of buckets; apply the matching
  cheap fix; verify the median wall-time moved AND the invariants still hold
  before committing. Use for "speed up formatting/parsing/linting", "close the gap
  with tex-fmt on <doc>", "profile the formatter/parser/linter", "why is
  masters_dissertation slow".
---

Use this skill when the task is *measure parser, formatter, or linter cost on a
real input and recover wall-time*—not to (re)generate the docs benchmark page
(that is the `bench` skill) and not to check parse concordance (that is
`parse-compat`).

The workflow is linear: **baseline → profile → classify → fix → measure →
verify invariants → commit → repeat.** The buckets and verification are shared
across all three pipelines; only the harness invocation and the hot-file map
differ (see §Which pipeline).

## The gap, and where it lives

The docs benchmark (`docs/src/reference/benchmarks.md`,
`benches/benchmark_results.json`) shows the standing gap vs tex-fmt. As of the
last run only **`masters_dissertation.tex` is a real per-byte gap** (~6×); the
smaller docs (`small`, `cv`, `project`) are ~1.15–1.4× and dominated by the
**fixed startup floor**, not formatting work. Confirm this split before
optimizing: a small-doc "win" is almost always startup-floor noise, not per-byte
work. `masters_dissertation.tex` is the doc to profile.

The startup floor was already attacked once (CWL tier baked into a build-time
`phf` map; see `benches/README.md` §Findings). Don't re-chase it without a fresh
`badness --version` vs `badness format` measurement showing new floor slack.

## Which pipeline (formatter / parser / linter / bib)

The buckets in §Classify and the verify/commit discipline are shared. What
differs is *which metric you read*, *which files are hot*, and *how you isolate
the work*:

- **Formatter.** Micro-bench `format only` metric. Hot files:
  `src/formatter/core.rs` (`lower_node`/`lower_element_stream`, the `ENVIRONMENT`
  predicate scans), `src/formatter/ir.rs` (`Ir`), `src/formatter/printer.rs`
  (`run_with_mode`, `flat_width`). Guarded by **idempotence** + protected regions.
- **Parser.** Micro-bench `parse only` metric (same binary, same run—the harness
  reports both). Hot files: `src/parser/lexer*`, `src/parser/grammar.rs`,
  `src/parser/events.rs`, `src/parser/tree_builder*`. Guarded by **losslessness**;
  any emitter/token-shape change also needs `parse-compat`.
- **Linter.** *Not* covered by the micro-bench split. The linter shares the parse
  path (so `parse only` already attributes the parse half); the lint-specific
  cost is `src/linter/` (`check.rs`, `rules/`, `fix.rs`, `render.rs`) layered on
  top. Isolate it by `perf record`ing a repeated-run loop of the `Lint`
  subcommand (below), or by extending the micro-bench with a lint arm. Guarded by
  **losslessness** of any autofix (fix output must still parse and reconstruct).
- **Bib.** The parallel `.bib` pipeline (`src/bib/`) has its own lexer, parser,
  formatter, and linter with the same invariants. Profile it by pointing the
  harness/CLI at a large `.bib` file; emitter-shape changes need
  `bib-parse-compat`.

For the linter and bib (no micro-bench arm), drive the CLI under `perf` on a
warm loop so the startup floor is amortized across iterations:

```sh
# many iterations of the lint pass over the stress doc, pinned, under perf
perf record --call-graph=dwarf -F 999 -o /tmp/badness_perf.data -- \
  sh -c 'for i in $(seq 1 200); do
    ./target/release/badness lint --no-config \
      benches/documents/masters_dissertation.tex >/dev/null 2>&1; done'
```

(A repeated-CLI loop re-pays the process floor each iteration; that floor shows
up in the profile as `main`/arg-parsing/`SignatureDb` init. Read past it to the
per-byte lint frames, or subtract it by profiling `badness --version` the same
way.)

## Scope boundaries

- **Performance never justifies breaking an invariant.** These are test
  oracles, not aspirations (AGENTS.md §Invariants):
  - **Losslessness:** `reconstruct(text) == text`, byte-for-byte. Any parser
    perf change touching lexer/event/`tree_builder` shape must keep it; any
    linter autofix must still parse and reconstruct.
  - **Idempotence:** `fmt(fmt(x)) == fmt(x)`. Any formatter perf change touching
    emitter/IR shape must keep it.
  - **Protected regions** (verbatim, `lstlisting`, `\verb`, comments) are never
    altered.
- A change that drops a symbol from the perf top-25 but does not move median
  wall time is **sample relocation, not work elimination**—revert it.
- Keep the layers clean (AGENTS.md): no semantic knowledge into the parser, no
  parsing logic into the formatter, no new catcode/macro logic. A perf fix that
  needs any of these is out of scope—raise it instead.
- Don't pool the rowan `GreenNodeBuilder`/`NodeCache` across parses: it holds
  Arc'd green nodes (LSP memory leak) and a warm cache after iteration 1
  produces a misleading flat benchmark that real CLI usage never sees.

## Related files to read first

- `AGENTS.md` — tenets (deterministic formatting, parsing-is-the-parser's-job)
  and the Invariants section; the perf work must not push against a load-bearing
  decision without recording it.
- `benches/README.md` — the harness contract and the prior attribution rounds.
  The bucket table there is the starting map; update it if a round changes the
  picture.

## Machine setup (portable — do this first, every machine)

The wins this skill hunts are small (~3–5%), so measurement hygiene is
non-negotiable and must be re-checked on each box. Four things, in order:

1. **Know your CPU topology.** `lscpu` (or check whether `perf report` shows a
   `cpu_core` *and* a `cpu_atom` block). On a **hybrid P/E-core CPU** (recent
   Intel Core Ultra, Alder Lake+, Apple-style big.LITTLE under Linux VMs):
   - **Pin every timing run to one performance core** — `taskset -c 0` (core 0 is
     a P-core on current Intel hybrids; confirm with `lscpu -e` if unsure). Without
     pinning the scheduler migrates the process across P/E cores and the jitter
     swamps the win.
   - **In `perf report`, read the `cpu_core` block, not `cpu_atom`.** On a hybrid
     CPU `cpu_atom` captures a handful of stray samples and its percentages are
     noise.
   On a **homogeneous CPU** there is one event block and no P/E split, but still
   pin with `taskset -c <n>` — single-core pinning cuts migration jitter
   regardless of topology.
2. **Check the machine is quiet before trusting any number.** `uptime` /
   `cat /proc/loadavg`. For ~3–5% deltas you want load well under ~1.0 and no
   other CPU hog on your pinned core. **The most common self-inflicted polluter:
   editing a source file wakes rust-analyzer, which reindexes and pins a core for
   tens of seconds** — your "after" run then looks slower for reasons that have
   nothing to do with your change. Either wait for load to settle, or use the
   **interleaved A/B** below (which cancels slow drift).
3. **Check `perf` permissions.** `cat /proc/sys/kernel/perf_event_paranoid`. At
   `2` (common default) user-space call-graph sampling works but some hardware
   events don't; **`dwarf` call graphs are the portable choice** and are assumed
   throughout this skill. If it's `>2` or perf refuses, lower it for the session
   (`sudo sysctl kernel.perf_event_paranoid=1`) or fall back to `cargo flamegraph`.
4. **Record the machine in the commit.** CPU model + whether pinned + load state.
   A delta is only comparable against another delta measured the same way, and
   the corpus is run on more than one desktop.

## Build for profiling — the strip trap (READ THIS)

`[profile.release]` in `Cargo.toml` sets **`strip = "symbols"`**. That strips the
debuginfo back out *even when you pass `CARGO_PROFILE_RELEASE_DEBUG=true`*, so
`perf` shows your own functions as raw hex addresses (`[.] 0x00194dc2`) and the
profile is useless. You must **disable strip as well**:

```sh
CARGO_PROFILE_RELEASE_DEBUG=true CARGO_PROFILE_RELEASE_STRIP=false \
  cargo build --release --bench formatting
```

- `CARGO_PROFILE_RELEASE_DEBUG=true` only *adds* DWARF sections; it does **not**
  lower `opt-level` or change codegen/inlining. The machine code that runs is
  identical to a plain release build (the binary is just larger on disk), so
  timing is unaffected. Neither env var is ever committed — the shipped binary
  keeps the normal stripped release profile.
- **Verify symbols are present before profiling**, or you'll waste a whole record
  on a stripped binary:
  ```sh
  BIN=$(ls -t ./target/release/deps/formatting-* | grep -v '\.d$' | head -1)
  readelf -SW "$BIN" | grep -E '\.symtab|\.debug_info'   # both must be present
  ```
- **Pick the *newest* binary, not the first glob match.** `cargo` leaves stale
  `formatting-<hash>` binaries in `target/release/deps/`; a bare
  `formatting-*[!.d]` glob can grab an old *stripped* one. Always
  `ls -t … | head -1`.

## Harness — baseline & measurement

The in-process micro-bench (`benches/formatting.rs`, `harness = false`) is the
primary tool for parser+formatter—it splits **parse / format-only / full** and
has no process-startup floor. Task shortcut: `task bench:micro`. For focused
single-doc work drive it directly and pin the core.

**Note on shells:** your interactive shell is fish, but the agent automation
shell (the Bash tool) is **bash** — use bash syntax in tool calls. Both forms:

```sh
# bash (agent automation)
BIN=$(ls -t ./target/release/deps/formatting-* | grep -v '\.d$' | head -1)
for i in $(seq 1 12); do
  env BADNESS_BENCH_DOC=masters_dissertation.tex BADNESS_BENCH_ITERATIONS=60 \
    taskset -c 0 "$BIN" 2>/dev/null | grep -E 'full \(|parse only|format only'
done
```

```fish
# fish (your interactive shell)
set BIN (ls -t ./target/release/deps/formatting-* | grep -v '\.d$' | head -1)
for i in (seq 1 12)
  env BADNESS_BENCH_DOC=masters_dissertation.tex BADNESS_BENCH_ITERATIONS=60 \
    taskset -c 0 $BIN 2>/dev/null | grep -E 'full \(|parse only|format only'
end
```

Discard the first 2–3 warmup runs; take the median of the remaining ~9–10.
Per-run variance on a warm, pinned, *quiet* machine is ~3–5%; demand a delta at
least that big before declaring a fix worked.

The split tells you *which half to profile*: `parse_pct` vs `format_pct`. On
`masters` the last round was ~parse 26% / lower+print 72%—so the formatter is the
bigger lever, but confirm per round.

### Interleaved A/B (use when the machine won't sit still)

Absolute µs numbers drift with thermals and background load, so a "before" run
and an "after" run taken minutes apart (e.g. across a rebuild that woke
rust-analyzer) are **not comparable**. Cancel the drift by building *both*
binaries up front and alternating runs:

```sh
# build FIXED, stash it aside
CARGO_PROFILE_RELEASE_DEBUG=true CARGO_PROFILE_RELEASE_STRIP=false \
  cargo build --release --bench formatting
cp "$(ls -t ./target/release/deps/formatting-* | grep -v '\.d$' | head -1)" /tmp/bench_fixed
# revert the change, build BASELINE, stash it aside
git stash push src/…            # or git checkout the file
CARGO_PROFILE_RELEASE_DEBUG=true CARGO_PROFILE_RELEASE_STRIP=false \
  cargo build --release --bench formatting
cp "$(ls -t ./target/release/deps/formatting-* | grep -v '\.d$' | head -1)" /tmp/bench_base
git stash pop                    # restore the change
# alternate base/fixed so drift hits both equally; compare medians
run() { env BADNESS_BENCH_DOC=masters_dissertation.tex BADNESS_BENCH_ITERATIONS=60 \
  taskset -c 0 "$1" 2>/dev/null | grep 'format only' | grep -oE '[0-9]+\.[0-9]+' | head -1; }
for i in $(seq 1 14); do echo "$(run /tmp/bench_base) $(run /tmp/bench_fixed)"; done
```

Discard any pair where either side is a gross outlier (a load spike hits one run,
not the pair symmetrically). If the paired medians differ by less than the noise
floor, the fix is flat — revert it.

End-to-end CLI wall-time (includes the startup floor—use only to check the floor
itself, or to reproduce the docs-benchmark gap):

```sh
CARGO_PROFILE_RELEASE_DEBUG=true CARGO_PROFILE_RELEASE_STRIP=false cargo build --release
hyperfine --warmup 3 \
  'taskset -c 0 ./target/release/badness format --no-config \
     --stdin-filepath bench.tex < benches/documents/masters_dissertation.tex > /dev/null'
```

## Capture a profile

Flamegraph (the repo target writes an SVG):

```sh
task bench:profile   # masters_dissertation.tex, 60 iters → benches/flamegraph_masters.svg
# or any doc explicitly:
env BADNESS_BENCH_DOC=cv.tex BADNESS_BENCH_ITERATIONS=400 \
  cargo flamegraph --bench formatting -o benches/flamegraph_cv.svg
```

`perf` for the flat self-time view and caller/callee graphs (build with the
strip trap disabled first, and pick the newest binary):

```sh
BIN=$(ls -t ./target/release/deps/formatting-* | grep -v '\.d$' | head -1)
env BADNESS_BENCH_DOC=masters_dissertation.tex BADNESS_BENCH_ITERATIONS=200 \
  taskset -c 0 perf record --call-graph=dwarf -F 999 -o /tmp/badness_perf.data -- "$BIN"
perf report --stdio -i /tmp/badness_perf.data \
  --no-children -g none --percent-limit 0.8 | head -40
```

`--no-children` for flat self-time; `-g graph,caller` / `,callee` to find who
calls a hot leaf; `--inline` for inlined-frame visibility. Always read the
`cpu_core` block on a hybrid CPU.

**Trust the flat self-time view over the caller graph here.** In an optimized
release build with heavy inlining, `dwarf` caller-graph attribution is often weak
(children ≈ self, chains collapse) — don't spend a round doing caller-graph
archaeology when the flat view already names the hot leaf. To attribute an
allocation *site* specifically, reach for `heaptrack` rather than trying to read
malloc's callers out of the perf tree.

## Classify each hotspot

Every badness parser/formatter hotspot recovered so far falls into one of these
buckets (from `benches/README.md` §Findings, masters dissertation). Identify the
bucket BEFORE editing:

- **rowan red-tree cursor traversal (~20–30%, the dominant bucket).**
  `PreorderWithTokens` / `SyntaxElementChildren` iteration, `NodeData::new`,
  sibling walks, `cursor::free`. Mostly *inherent* to the lossless-CST +
  red-cursor design (the price of LSP/incremental/losslessness). Recoverable
  slack: **redundant re-traversal**—walking the same children more than once.
  ⚠️ The obvious instance (`lower_node` re-running `has_verbatim_body` per
  `ENVIRONMENT` arm) was **measured flat** (~0.5%, in the noise): most
  environments have too few direct children for the saved scans to matter. Treat
  "share one children pass" as a hypothesis to *measure*, not a guaranteed win;
  it only pays where the re-scanned node is genuinely wide (many direct children,
  scanned many times).
- **allocator churn (~17–21%).** malloc/free for `Ir` nodes, `Vec<Ir>`,
  `smol_str`, red nodes. Levers: pre-size a `Vec<Ir>` whose length is known; avoid
  a throwaway intermediate `Vec` in a hot lower/print loop (hoist + `clear()` +
  reuse, or borrow instead of collect). Attribute the *site* with `heaptrack`, not
  the perf caller tree. Do NOT invent a new pooling abstraction before
  measuring—an extra pass often costs more than the alloc it saves (see §Don't
  redo).
- **per-node predicate re-scan (subset of the traversal bucket).** A
  `lower_*`/classification/rule function that scans a node's children to answer
  several independent yes/no questions with separate passes. Fold into one pass
  that returns a small struct/bitset — *if* the node is wide enough to pay (see
  the flat-measurement warning above).
- **parse + tree-build (~13%).** lexer + `GreenNodeBuilder` + `smol_str`
  interning. Reducing it means emitting fewer tokens/nodes—**invasive**, gated by
  losslessness. Verify CST snapshots and a losslessness assertion before shipping
  any emitter-shape change. Don't pool the builder (see boundaries).
- **char-walk where bytes would do.** A scanner stepping char-by-char
  (`chars().next()?.len_utf8()`) through text that is structurally ASCII.
  Replace with a `memchr`-style byte scan. LaTeX structural bytes (`\`, `{`,
  `}`, `%`, `$`, `&`, `^`, `_`) are ASCII, so byte scans are losslessness-safe—
  but a token's *text* is arbitrary UTF-8, so only scan for ASCII delimiters,
  never index into the middle of a char.
- **Unicode ops on ASCII.** `to_uppercase`/`to_lowercase`/`trim` (Unicode
  whitespace) on what is always ASCII. Fold case a byte at a time (`b & !0x20`)
  or use a byte-level blank/trim check. ⚠️ `str::chars().count()` is **not** in
  this bucket — std already lowers it to the SWAR `char_count_general_case`
  (counts non-continuation bytes a word at a time); a hand-rolled byte loop won't
  beat it. Only redundant *re*-counting of the same string is recoverable there,
  and that's a memoization change, not a byte-loop swap.
- **printing (~7%, modest).** `Printer::run_with_mode` + `flat_width`. Usually
  not the lever; touch only if the profile actually points here for the doc at
  hand. `flat_width` re-measuring the same `Ir::Text` across ancestor group-fit
  checks is the one memoization candidate, but it's an IR-shape change gated by
  idempotence — bigger, not a quick win.

## Apply the smallest matching fix

- **Don't theorize before measuring.** In the sibling panache, several
  "should-have-helped" changes (LF pre-count to pre-size a vec; a redundant
  byte-gate) *regressed* wall time and were reverted. In badness the
  `has_verbatim_body` consolidation *looked* obviously good and came out flat. The
  intuition wasn't wrong; the cost model was. Measure.
- **One change per commit.** Prevents one regression masking another's win.
  Re-run tests + clippy + fmt + a fresh pinned measurement per change.
- **Revert promptly.** If 12 pinned runs (or an interleaved A/B) don't show a
  median shift larger than the ~3–5% noise floor, the fix doesn't pay—revert and
  pick another lever. Don't ship pretty-but-flat refactors as perf.

## Verify and commit

Invariants first, then wall-time. For every commit:

```sh
cargo test                                                        # incl. losslessness + idempotence oracles
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt -- --check
```

Snapshot suites (insta): a perf change must NOT change output. If `cargo test`
reports snapshot diffs, the change altered layout—that is a regression, not a
refresh. Only `cargo insta accept` after confirming, byte for byte, that a diff
is intended (formatter output shape must not drift for a perf fix). If the change
touched parser emitter shape, also run the differential gauge via the
`parse-compat` skill (and `bib-parse-compat` for `.bib` changes).

Then a fresh pinned measurement (`taskset -c 0`, 12 runs or interleaved A/B,
median). Commit message names the bucket and quotes the delta (conventional-commit,
imperative, `<60` char subject per the repo style):

```
perf(formatter): <bucket> on <call site>

<one paragraph: profile pointed here, what was wasteful, what the fix
replaces it with>

Median on `<harness command>` (12 runs, taskset -c 0, <CPU model>, load <n>):
~X ms -> ~Y ms (~Z%).
```

Cite the number even when it's "in the noise"—that's the honest record, and it
lets a reviewer decide whether to ship a noise-floor change at all. Record the
CPU and load state so a delta from another desktop is comparable.

## Key files

- `benches/formatting.rs` — the micro-bench harness (`BADNESS_BENCH_DOC` /
  `BADNESS_BENCH_ITERATIONS` / `BADNESS_BENCH_OUTPUT_JSON`; parse/format/full
  split; startup-floor report at the top). Covers parser + formatter; **no lint
  arm** (profile the linter via the CLI loop in §Which pipeline).
- `benches/README.md` — harness contract + the attribution findings table.
- `benches/documents/` — corpus (`small.tex` committed; the rest fetched by
  `download.sh`, gitignored). `masters_dissertation.tex` is the per-byte stress
  doc; `phd_dissertation.tex` is the larger stress doc.
- `Taskfile.yml` — `bench:micro`, `bench:profile`, `bench` targets.
- `src/formatter/core.rs`, `src/formatter/ir.rs`, `src/formatter/printer.rs` —
  formatter hot paths (idempotence-guarded).
- `src/parser/lexer*`, `src/parser/grammar.rs`, `src/parser/events.rs`,
  `src/parser/tree_builder*` — parser hot paths (losslessness-guarded).
- `src/linter/check.rs`, `src/linter/rules/`, `src/linter/fix.rs`,
  `src/linter/render.rs` — linter hot paths (autofix losslessness-guarded).
- `src/bib/` — the parallel `.bib` pipeline (own lexer/parser/formatter/linter).

## Don't redo / known traps

- **The `strip = "symbols"` release profile hides your symbols.** Pass
  `CARGO_PROFILE_RELEASE_STRIP=false` alongside `…_DEBUG=true`, and verify
  `.symtab` is present before recording. (See §Build for profiling.)
- **The `formatting-*` glob can grab a stale stripped binary.** Use
  `ls -t … | head -1` and verify symbols.
- **Editing a source file wakes rust-analyzer and pollutes the next run.** Check
  `/proc/loadavg` before trusting a number; prefer the interleaved A/B when the
  machine won't sit still.
- **The agent automation shell is bash, not fish.** The `for … in (…)` fish loop
  won't parse under the Bash tool — use `for i in $(seq …); do … done`.
- **Trust flat self-time over the dwarf caller graph in release.** Heavy inlining
  collapses the call chains; use `heaptrack` to pin an allocation site.
- **Don't optimize the small docs' wall time.** It's startup floor, not per-byte
  work; the micro-bench `full` number for `small.tex`/`cv.tex` is ~0.1–0.4 ms
  while the CLI is ~1.3–1.4 ms. `masters` is the only real per-byte gap.
- **Don't pool the rowan `NodeCache`/`GreenNodeBuilder` across parses.** Memory
  leak (Arc'd green nodes) + misleading warm-cache benchmark.
- **Don't trust `cpu_atom` perf samples; don't run unpinned.** (Hybrid CPU — see
  §Machine setup; re-check topology on each box.)
- **Don't `cargo insta accept` a snapshot diff to make a perf commit pass.** A
  layout change under a perf commit is a bug in the change.
- **A pre-count pass to pre-size a `Vec` often loses.** The extra scan over the
  input can cost more than the resize-grow it saves (panache precedent). Only try
  it if you can prove the win by measurement.
- **"Share one children pass" is not a free win.** The `has_verbatim_body`
  per-`ENVIRONMENT` consolidation measured flat (2026-07); only pays on genuinely
  wide, repeatedly-scanned nodes. Measure before shipping.

## Report-back format

1. Hotspot (function + approximate self-%) and which pipeline (parse/format/lint).
2. Bucket (from §Classify).
3. Median wall-time delta on the relevant harness (12 runs or A/B, `taskset -c 0`,
   with CPU model + load state).
4. Invariants: losslessness + idempotence oracles green, snapshots unchanged,
   clippy/fmt clean (or the specific exception).
5. What was tried and reverted, with the reason.
6. Suggested next hotspot, ranked by likely shared root cause.
