# Benchmarking and profiling

Three complementary tools, measuring different things:

  | Tool                                            | What it measures                                  | Includes startup floor?       |
  | ----------------------------------------------- | ------------------------------------------------- | ----------------------------- |
  | `benches/compare_format.sh` (`task bench`)      | wall-clock CLI speed vs tex-fmt/latexindent       | **yes** (whole process)       |
  | `benches/formatting.rs` (`task bench:micro`)    | in-process per-byte cost, split parse/format/full | **no** (library entry points) |
  | `benches/keystroke.rs` (`task bench:keystroke`) | what one editor keystroke costs through salsa     | **no** (library entry points) |

The CLI script answers "how fast is the `badness` binary"; the formatting bench
answers "where does the per-byte work go, with no process startup in the way."
Use them together to separate the **fixed startup floor** from the **per-byte
cost** (the TODO's profiling task).

The keystroke bench answers a different question — not throughput but *latency
per edit*, on the composition (`didChange` splice → `upsert_file` → parse) that
neither of the other two touches. See its own section below.

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
```

## The keystroke bench

`benches/keystroke.rs` times the composition a real editor session runs on every
character, which nothing else here touches: `benches/formatting.rs` and the CLI
comparison never construct an `IncrementalDatabase`, and `tests/scaling.rs`
guards growth ratios rather than the pipeline. Three rows per document:

1. **`upsert, text unchanged`** — the staleness guard alone: what a no-op
   `upsert_file` costs to prove there is nothing to do. This is what the
   language server pays whenever a job re-writes text salsa already has, and the
   one row that must stay **flat** in the document size.
2. **`splice + upsert (write phase)`** — `lsp::apply_content_changes` on the
   live `TextBuffer` plus handing the result to `upsert_file`, no parse
   demanded. The per-keystroke *text copies* live here, so this is the row on
   which text-storage designs (`String`, `Arc<str>`, a rope) are comparable.
3. **`keystroke end-to-end (parse included)`** — the same plus `parsed_tree`.
   Badness has no intra-file reparse yet (AGENTS.md decision #6), so **row 3
   minus row 2 is the full-reparse cost**, printed as its own line.

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
