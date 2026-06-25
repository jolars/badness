# Benchmarking and profiling

Two complementary tools, measuring different things:

| Tool | What it measures | Includes startup floor? |
| --- | --- | --- |
| `benches/compare_format.sh` (`task bench`) | wall-clock CLI speed vs tex-fmt/latexindent | **yes** (whole process) |
| `benches/formatting.rs` (`task bench:micro`) | in-process per-byte cost, split parse/format/full | **no** (library entry points) |

The CLI script answers "how fast is the `badness` binary"; the in-process bench
answers "where does the per-byte work go, with no process startup in the way."
Use them together to separate the **fixed startup floor** from the **per-byte
cost** (the TODO's profiling task).

## Quick start

```bash
# Fetch the larger corpus (small.tex is committed; the rest are gitignored)
task bench:download

# Wall-clock CLI comparison → BENCH.md
task bench

# In-process micro-bench (parse vs format vs full pipeline, throughput)
task bench:micro

# Machine-readable JSON from the micro-bench
BADNESS_BENCH_OUTPUT_JSON=benches/micro_results.json cargo bench --bench formatting
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

- `BADNESS_BENCH_DOC` — profile only this document under `benches/documents/`.
- `BADNESS_BENCH_ITERATIONS` — iteration count for the selected document (10).
- `BADNESS_BENCH_OUTPUT_JSON` — write a machine-readable report to this path.

The micro-bench warms up before timing, so the one-time `LazyLock` signature-DB
init (see below) is excluded from the timed loops — it is reported separately at
the top of the run as a startup-floor component.

## Findings (2026-06, attribution round)

Numbers are from one dev machine; treat the *ratios*, not the absolutes, as the
finding. Reproduce with `task bench:micro` + `task bench:profile`.

### Startup floor vs per-byte

The CLI's small-document time is dominated by a **fixed startup floor**, not by
formatting:

| Document | size | CLI wall-clock | in-process full | implied floor |
| --- | ---: | ---: | ---: | ---: |
| small.tex | 1.2 KB | ~4.5 ms | ~0.11 ms | ~4.4 ms |
| cv.tex | 6.3 KB | ~5.1 ms | ~0.38 ms | ~4.7 ms |
| masters_dissertation.tex | 95 KB | ~14.9 ms | ~8.6 ms | ~6.3 ms |

A bare `badness --version` is only ~0.8 ms, so the extra ~3.7 ms of the format
floor is **the one-time CWL signature-DB init**: `cwl()` decompresses
(`flate2`) and parses the embedded `cwl_signatures.json.gz` on first access
(`~4.5 ms`), and it is on the format hot path (`Signatures::command`/
`environment` fall back to `cwl()`, and the lexer consults it for verbatim-env
detection). The curated `builtin` DB (`data/signatures.json`) is cheap by
comparison (~0.09 ms). This is the biggest lever for small-doc latency and is
*implementation slack worth chasing* (e.g. a faster-to-decode embedded format,
or deferring/streaming the CWL tier) — distinct from the architectural per-byte
cost below.

### Per-byte cost (masters dissertation, in-process)

Pipeline split: parse ~25 %, lower+print ~70 % of the full pipeline; throughput
~10 MB/s. Flamegraph self-time, bucketed:

| Bucket | self-time | notes |
| --- | ---: | --- |
| rowan red-tree cursor traversal | ~25–30 % | `PreorderWithTokens`/`SyntaxElementChildren` iteration, `NodeData::new`, sibling walks |
| allocator (malloc/free) | ~17 % | `Ir` nodes, `Vec<Ir>`, `smol_str`, red nodes |
| parse + tree-build | ~13 % | lexer + `GreenNodeBuilder` + `smol_str` interning |
| lowering logic | ~10 % | `lower_node`/`lower_element_stream` + `Ir` build |
| printing | ~7 % | `Printer::run_with_mode` + `flat_width` |

Most of the per-byte cost is **inherent to the lossless-CST + Doc-IR
architecture** (materializing/walking red cursors, allocating IR) — by design,
and the price of the LSP, incremental reparse, and losslessness. The printer
itself is modest. One concrete bit of *slack*: `lower_node` runs up to four
direct-children predicate scans per `ENVIRONMENT`
(`has_verbatim_body`/`is_margin_framed`/`is_alignment_env`/`is_list_env`); these
are bounded (direct children only) but redundant and could share one pass.
