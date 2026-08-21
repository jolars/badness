# Benchmarking and profiling

Five complementary tools, measuring different things:

  | Tool                                              | What it measures                                  | Includes startup floor?       |
  | ------------------------------------------------- | ------------------------------------------------- | ----------------------------- |
  | `benches/compare_format.sh` (`task bench`)        | wall-clock CLI speed vs tex-fmt/latexindent       | **yes** (whole process)       |
  | `benches/formatting.rs` (`task bench:micro`)      | in-process per-byte cost, split parse/format/full | **no** (library entry points) |
  | `benches/keystroke.rs` (`task bench:keystroke`)   | what one editor keystroke costs through salsa     | **no** (library entry points) |
  | `benches/reparse.rs` (`task bench:reparse`)       | what one incremental reparse costs, per tier      | **no** (library entry points) |
  | `benches/lsp_memory.rs` (`task bench:lsp-memory`) | live heap retained across LSP history             | **no** (in-process server)    |

The CLI script answers "how fast is the `badness` binary"; the formatting bench
answers "where does the per-byte work go, with no process startup in the way."
Use them together to separate the **fixed startup floor** from the **per-byte
cost** (the TODO's profiling task).

The last two answer a different question — not throughput but *latency per
edit*. The keystroke bench times the whole composition (`didChange` splice →
`upsert_file` → parse); the reparse bench times `parser::reparse` alone, which
is the only place the tier a scenario reaches is observable. Both have their own
sections below, and both carry a **gate** (`task bench:gate`).

The memory harness answers a third question: whether a long-running language
server retains *history* after arriving at the same current project state. It
uses a counting system allocator around the real in-process LSP server, so its
live-byte figure excludes allocator pages that are free but have not been
returned to the operating system.

## The gates

```bash
task bench:gate              # both gates
task bench:reparse-gate      # tiers, speedup floors, bail budget
task bench:keystroke-gate    # the write phase
task bench:lsp-memory-gate   # live heap after paired LSP histories
```

Each case declares what it claims and the gate checks it, printing every check
with its margin so a threshold can be watched drifting long before it fails. Off
by default: a plain `task bench:reparse` stays a measurement and never fails the
shell it was typed into.

Three rules the gates are built on, and one habit they need.

- **Thresholds live in the harness and nowhere else.** A number in `TODO.md`
  cannot be checked, and panache's drifted from its harness inside one phase.
  Read the gate's output, not a table in a document.
- **Every ratio rule carries an absolute-microsecond escape**, because a ratio
  on a 2 µs baseline measures noise. Speedup *floors* deliberately do not: an
  escape large enough to matter would forgive every result a small document can
  produce.
- **A gate must not pass by not measuring.** Every document but `small.tex` is
  gitignored, so the gates assert the corpus is present *and* the expected size
  — the floors are a function of document size, and `download.sh` pins release
  tags precisely so those sizes hold. They also assert that each pinned edit
  site still lands in the leaf kind it claims, which is simultaneously the check
  that the `verbatim` site is still injecting its `lstlisting`.
- **Calibrate on an idle machine**, at the default iteration count, taking
  floors \~5% under the lowest of three runs. A shortened run measures sampling
  noise, and a floor set against a loaded machine fails later for no reason.

The gates do **not** run in CI: they need the gitignored corpus and a release
build, and a timing assert on a shared runner would land flaky. Run them locally
before touching the reparse path.

## Quick start

```bash
# Fetch the larger corpus (small.tex is committed; the rest are gitignored)
task bench:download

# Wall-clock CLI comparison → benches/benchmark_results.json (feeds the docs
# benchmark page, docs/src/reference/benchmarks.md)
task bench

# In-process micro-bench (parse vs format vs full pipeline, throughput)
task bench:micro

# Machine-readable JSON from the micro-bench
BADNESS_BENCH_OUTPUT_JSON=benches/micro_results.json cargo bench --bench formatting

# LSP keystroke pipeline (didChange splice → salsa upsert → parse)
task bench:keystroke

# Incremental reparse, per tier
task bench:reparse

# Compare retained heap after equivalent LSP end states
task bench:lsp-memory

# Check both against their declared contracts (exits non-zero on a violation)
task bench:gate
```

## The keystroke bench

`benches/keystroke.rs` times the composition a real editor session runs on every
character, which nothing else here touches: `benches/formatting.rs` and the CLI
comparison never construct an `IncrementalDatabase`, and `tests/scaling.rs`
guards growth ratios rather than the pipeline. Four rows per document, of which
the first is a reference rather than a stage:

0. **`text copy (reference)`** — one allocation and one linear copy of the
   document. Nothing in the pipeline calls this; it is the machine-independent
   unit the write phase is measured in, since what a splice *should* cost is a
   small number of linear passes.
1. **`upsert, text unchanged`** — the staleness guard alone: what a no-op
   `upsert_file` costs to prove there is nothing to do. This is what the
   language server pays whenever a job re-writes text salsa already has, and the
   one row that must stay **flat** in the document size.
2. **`splice + upsert (write phase)`** — `lsp::apply_content_changes` on the
   live `TextBuffer`, handing the result to `upsert_file`, and staging the edit
   chain, no parse demanded. The per-keystroke *text copies* live here, so this
   is the row on which text-storage designs (`String`, `Arc<str>`, a rope) are
   comparable. It currently costs **\~52 document copies** on a large file,
   almost all of it `TextBuffer::new` rebuilding the whole `LineIndex`; with the
   parse now cheap, that is the largest single thing in a keystroke.
3. **`keystroke end-to-end (parse included)`** — the same plus `parsed_tree`.

There is deliberately **no reparse row here**. It used to be derived as row 3
minus row 2, which was fair while a full parse was 97% of the keystroke and
became a difference of two \~800 µs numbers once the leaf tiers landed: five
runs of one binary gave 150, 75, 67 and 30 µs, and one clamped to zero when row
3 came out *below* row 2. Use `benches/reparse.rs` instead.

## The reparse bench

`benches/reparse.rs` times `parser::reparse` directly, against a `ReparseBase`
it builds itself. That is the only way to observe which `ReparseTier` answered —
through the salsa layer the tier is computed and dropped, and the reparse side
channel may not grow an accessor for it.

Twenty-two cases: five for each of four documents, plus two region cases on the
small document. `word` (a letter typed into prose) and `math-word` (a
partition-preserving letter typed into math) must reach the token tier;
`verbatim` (a line typed into an injected `lstlisting`) must reach the
protected-body tier; `math-shape` (an edit that moves a scripted-word boundary)
must reach the math tier; and `decline` must reach none. The declining case
types a backslash at the *same offset* as the word case, so the pair isolates
the guard rather than confounding it with position, and it prices the full guard
cascade rather than bailing on the first check.

```bash
task bench:reparse
BADNESS_BENCH_CASE=phd_dissertation.tex/word cargo bench --bench reparse
```

Two things to know about the numbers. They are **not** comparable with the
keystroke bench's end-to-end row, which carries the splice, the upsert and the
cache lookups around this call — a thesis keystroke is \~0.71 ms end to end, of
which the reparse is \~37 µs. And a decline costs about what a splice does,
because both pay the same `O(top-level arity)` descent to the leaf; that is what
is left of a cheap reparse.

The bench **pre-warms the heap** before measuring. Without it the first case on
a large document reads \~40% high and no amount of warmup inside the measurement
fixes it: glibc trims the heap as each iteration's tree is freed and the next
faults those pages back in, so the number depended on where a case sat in the
list. `MALLOC_TRIM_THRESHOLD_=-1` collapses all three thesis cases onto the same
\~26 ms, which is how that was pinned down.

Each iteration alternates an insert and a delete of one character, so the text
genuinely changes every round: salsa sees a fresh revision and the number is one
keystroke, never a memoized no-op.

To A/B a text-storage change, edit `handoff` (and its return type) on each
branch to whatever that branch's `upsert_file` takes, and run both. Iteration
counts auto-calibrate to `BADNESS_BENCH_TARGET_MS` (500 ms per row) because the
corpus spans three orders of magnitude; on the largest document the end-to-end
row buys only a couple of dozen samples, so **run one document at a time on an
idle machine** when the difference you are chasing is under \~10%:

```bash
BADNESS_BENCH_DOC=phd_dissertation.tex cargo bench --bench keystroke
BADNESS_BENCH_OUTPUT_JSON=/tmp/head.json cargo bench --bench keystroke
```

## The LSP memory harness

`benches/lsp_memory.rs` runs the public `lsp::serve` entry point over an
in-memory protocol connection and compares two pairs of sessions:

- **query log:** real text changes followed by diagnostics and hover requests,
  versus the same protocol traffic with unchanged text;
- **project generations:** opening the same synthetic `.tex`/`.sty`/`.bib`
  project one directory at a time, versus seeding the entire tree before opening
  the same documents.

Each pair ends with byte-identical open buffers and the same project membership.
The difference in current live allocations is therefore retained history, not
the cost of the final project. Peak allocation is reported for context but never
gated. The default gate permits 1 MiB of query-history excess and the larger of
1 MiB or 10% of the one-shot project for progressive discovery. It is local-only
because it drives threaded LSP histories; it is not a shared-runner CI gate.

```bash
task bench:lsp-memory
task bench:lsp-memory-gate

BADNESS_MEMORY_SCENARIO=query-log \
BADNESS_MEMORY_QUERY_GENERATIONS=1000 \
BADNESS_MEMORY_OUTPUT_JSON=/tmp/badness-memory.json \
  cargo bench --bench lsp_memory
```

`BADNESS_MEMORY_SCENARIO` accepts `all`, `query-log`, or `project`.
`BADNESS_MEMORY_PROJECT_GENERATIONS` overrides the default 48-directory project,
and `BADNESS_MEMORY_ASSERT=1` enables the gates.

## Profiling

`benches/formatting.rs` is `harness = false` (a plain `main` with fixed
iteration counts, not criterion) so a flamegraph attaches cleanly to a single
hot document instead of criterion's sampling loop:

```bash
# Flamegraph the masters dissertation per-byte hot paths
task bench:profile          # → benches/flamegraph_masters.svg

# Or pick any corpus document explicitly:
BADNESS_BENCH_DOC=masters_dissertation.tex BADNESS_BENCH_ITERATIONS=60 \
    cargo flamegraph --bench formatting -o benches/flamegraph_masters.svg

# perf with call graphs for the selected document
BADNESS_BENCH_DOC=masters_dissertation.tex BADNESS_BENCH_ITERATIONS=60 \
    perf record --call-graph dwarf cargo bench --bench formatting
perf report
```

Env knobs for `benches/formatting.rs`:

- `BADNESS_BENCH_DOC`: profile only this document under `benches/documents/`.
- `BADNESS_BENCH_ITERATIONS`: iteration count for the selected document (10).
- `BADNESS_BENCH_OUTPUT_JSON`: write a machine-readable report to this path.

The micro-bench warms up before timing, so the one-time `LazyLock` signature-DB
init (see below) is excluded from the timed loops—it is reported separately at
the top of the run as a startup-floor component.

## Findings (2026-06, attribution round)

Numbers are from one dev machine; treat the *ratios*, not the absolutes, as the
finding. Reproduce with `task bench:micro` + `task bench:profile`.

### Startup floor vs per-byte

The CLI's small-document time is dominated by a **fixed startup floor**, not by
formatting:

  | Document                 | size   | CLI wall-clock | in-process full | implied floor |
  | ------------------------ | -----: | -------------: | --------------: | ------------: |
  | small.tex                | 1.2 KB |        ~4.5 ms |        ~0.11 ms |       ~4.4 ms |
  | cv.tex                   | 6.3 KB |        ~5.1 ms |        ~0.38 ms |       ~4.7 ms |
  | masters_dissertation.tex |  95 KB |       ~14.9 ms |         ~8.6 ms |       ~6.3 ms |

A bare `badness --version` is only \~0.8 ms, so the extra \~3.7 ms of the format
floor *was* **the one-time CWL signature-DB init**: `cwl()` used to decompress
and parse the embedded `cwl_signatures.json.gz` on first access (`~4.5 ms`), and
it is on the format hot path (`Signatures::command`/`environment` fall back to
`cwl()`, and the lexer consults it for verbatim-env detection).

**Fixed (2026-06):** the CWL tier is now baked into the binary at build time as
a `phf` perfect-hash map (`build.rs` → `phf_codegen`, values are `const fn`
constructor calls in `src/semantic/signature.rs`), so init is **\~0** (no
decompress, no JSON parse, no map build). CLI latency on `small.tex` dropped
from \~4.5 ms to \~1.3 ms, and `cv.tex` from \~5.1 ms to \~1.4 ms. The trade-off
is a larger binary (the data is now uncompressed read-only statics) and a
one-time build-time codegen step. The curated `builtin` DB
(`data/signatures.json`, \~8 KB, \~0.09 ms) stays a runtime `LazyLock` JSON
parse—negligible, not worth moving.

### Per-byte cost (masters dissertation, in-process)

Pipeline split: parse \~25 %, lower+print \~70 % of the full pipeline;
throughput \~10 MB/s. Flamegraph self-time, bucketed:

  | Bucket                          | self-time | notes                                                                                  |
  | ------------------------------- | --------: | -------------------------------------------------------------------------------------- |
  | rowan red-tree cursor traversal |  ~25–30 % | `PreorderWithTokens`/`SyntaxElementChildren` iteration, `NodeData::new`, sibling walks |
  | allocator (malloc/free)         |     ~17 % | `Ir` nodes, `Vec<Ir>`, `smol_str`, red nodes                                           |
  | parse + tree-build              |     ~13 % | lexer + `GreenNodeBuilder` + `smol_str` interning                                      |
  | lowering logic                  |     ~10 % | `lower_node`/`lower_element_stream` + `Ir` build                                       |
  | printing                        |      ~7 % | `Printer::run_with_mode` + `flat_width`                                                |

Most of the per-byte cost is **inherent to the lossless-CST + Doc-IR
architecture** (materializing/walking red cursors, allocating IR)—by design, and
the price of the LSP, incremental reparse, and losslessness. The printer itself
is modest. One concrete bit of *slack*: `lower_node` runs up to four
direct-children predicate scans per `ENVIRONMENT`
(`has_verbatim_body`/`is_margin_framed`/`is_alignment_env`/`is_list_env`); these
are bounded (direct children only) but redundant and could share one pass.
