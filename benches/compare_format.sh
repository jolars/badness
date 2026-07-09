#!/usr/bin/env bash
#
# Benchmark badness's formatter speed against other LaTeX formatters
# (tex-fmt, latexindent) on a corpus of real documents, using hyperfine.
#
# Usage:
#   ./benches/compare_format.sh             # → benches/benchmark_results.json
#   ./benches/compare_format.sh --out PATH  # write the JSON artifact elsewhere
#   BADNESS_BENCH_INPUT=path/to/file.tex ./benches/compare_format.sh
#                                           # benchmark one real file
#
# The JSON artifact feeds the docs benchmark page (docs/src/reference/benchmarks.md),
# rendered at mdbook-build time by the doc-utils preprocessor. Regenerate it
# manually with `task bench`; it is never rebuilt at site-generation time or in CI.
#
# This is a *visibility* tool, not a CI gate and not an output-parity target.
# It measures wall-clock formatting speed only, never output equivalence — the
# three tools have very different layout philosophies (notably latexindent only
# indents by default and does no line reflow, so it does less work).
#
# Timing backend: prefers `hyperfine` (warmup + stddev/min/max) with `jq` to read
# its JSON; falls back to a plain shell timing loop (mean only) when either is
# missing. Comparison tools absent from PATH are skipped silently. Every tool is
# run stdin → stdout so the comparison is free of file-mutation noise.
#
# Mirrors the sibling project panache's `benches/compare_all.sh`.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

DOCS_DIR="benches/documents"
PROJECT_SRC="$DOCS_DIR/project"
BADNESS="$REPO_ROOT/target/release/badness"
HYPERFINE_MIN_RUNS=3
PROJECT_ITERS=5

JSON_OUT="benches/benchmark_results.json"

# The folder benchmark runs against a throwaway copy of the project corpus so
# both tools walk an identical, un-gitignored file set (see the project block
# below). Clean it up on any exit.
PROJECT_STAGE=""
cleanup() { [ -n "$PROJECT_STAGE" ] && rm -rf "$PROJECT_STAGE"; }
trap cleanup EXIT

while [[ $# -gt 0 ]]; do
    case "$1" in
        --out)  JSON_OUT="$2"; shift 2 ;;
        -h|--help) sed -n '3,17p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; exit 2 ;;
    esac
done

# Progress goes to stderr so the JSON artifact path is the only thing on stdout.
log() { echo -e "$@" >&2; }

have() { command -v "$1" >/dev/null 2>&1; }

HAVE_TEXFMT=$(have tex-fmt && echo yes || echo no)
HAVE_LATEXINDENT=$(have latexindent && echo yes || echo no)
HAVE_HYPERFINE=$(have hyperfine && echo yes || echo no)
HAVE_JQ=$(have jq && echo yes || echo no)

BACKEND="shell-loop"
if [ "$HAVE_HYPERFINE" = "yes" ] && [ "$HAVE_JQ" = "yes" ]; then
    BACKEND="hyperfine"
fi

# --- Build the release binary ------------------------------------------------

log ">> Building release binary..."
cargo build --release --quiet 2>&1 | grep -v "warning:" >&2 || true

# --- Tool versions + host metadata -------------------------------------------

BADNESS_VER=$("$BADNESS" --version | awk '{print $2}')
TEXFMT_VER=""; LATEXINDENT_VER=""
[ "$HAVE_TEXFMT" = "yes" ] && TEXFMT_VER=$(tex-fmt --version 2>/dev/null | awk '{print $2}')
[ "$HAVE_LATEXINDENT" = "yes" ] && LATEXINDENT_VER=$(latexindent --version 2>/dev/null | head -1 | awk '{print $1}' | tr -d ',')

HOST_OS=$(uname -s | tr '[:upper:]' '[:lower:]')
HOST_ARCH=$(uname -m)
HOST_CPU=""
[ -f /proc/cpuinfo ] && HOST_CPU=$(grep -m1 "model name" /proc/cpuinfo | sed 's/.*: //')

log "Formatters:"
log "  badness: $BADNESS_VER"
if [ "$HAVE_TEXFMT" = "yes" ]; then log "  tex-fmt: $TEXFMT_VER"; else log "  tex-fmt: (not on PATH — skipped)"; fi
if [ "$HAVE_LATEXINDENT" = "yes" ]; then log "  latexindent: $LATEXINDENT_VER"; else log "  latexindent: (not on PATH — skipped)"; fi
log "  backend: $BACKEND"
[ "$BACKEND" = "shell-loop" ] && log "  (hint: install hyperfine + jq for stddev/min/max stats)"
log

# --- JSON helpers ------------------------------------------------------------

json_escape() { printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'; }

# Run one command; echo "mean stddev min max runs" in milliseconds. For the
# shell-loop backend stddev/min/max are the literal "null".
run_one() {
    local iterations="$1" cmd="$2"
    if [ "$BACKEND" = "hyperfine" ]; then
        local tmp; tmp=$(mktemp)
        # --ignore-failure: the folder benchmark's `--check` commands exit non-zero
        # when files "would reformat" (the normal case); that is not a run error.
        hyperfine --warmup 1 --min-runs "$HYPERFINE_MIN_RUNS" --ignore-failure \
            --export-json "$tmp" --style=none "$cmd" >/dev/null 2>&1
        local mean stddev min max runs
        mean=$(jq -r '.results[0].mean' "$tmp")
        stddev=$(jq -r '.results[0].stddev' "$tmp")
        min=$(jq -r '.results[0].min' "$tmp")
        max=$(jq -r '.results[0].max' "$tmp")
        runs=$(jq -r '.results[0].times | length' "$tmp")
        rm -f "$tmp"
        awk -v m="$mean" -v s="$stddev" -v lo="$min" -v hi="$max" -v r="$runs" \
            'BEGIN { printf "%.4f %.4f %.4f %.4f %d\n", m*1000, s*1000, lo*1000, hi*1000, r }'
    else
        local start end
        start=$(date +%s%N)
        local i
        # `|| true`: a `--check` command exits non-zero when files would reformat.
        for ((i=1; i<=iterations; i++)); do eval "$cmd" >/dev/null 2>&1 || true; done
        end=$(date +%s%N)
        awk -v t="$((end - start))" -v n="$iterations" \
            'BEGIN { printf "%.4f null null null %d\n", (t/n)/1e6, n }'
    fi
}

# --- Corpus ------------------------------------------------------------------
# Each entry: "id|file|label|iterations". `file` is relative to DOCS_DIR unless
# the single-file override is in effect.

declare -a CORPUS=()
if [ -n "${BADNESS_BENCH_INPUT:-}" ]; then
    [ -f "$BADNESS_BENCH_INPUT" ] || { echo "error: BADNESS_BENCH_INPUT='$BADNESS_BENCH_INPUT' is not a file" >&2; exit 1; }
    CORPUS+=("override|$BADNESS_BENCH_INPUT|$(basename "$BADNESS_BENCH_INPUT")|10")
else
    CORPUS+=("small|$DOCS_DIR/small.tex|small.tex (baseline)|50")
    CORPUS+=("cv|$DOCS_DIR/cv.tex|cv.tex|30")
    CORPUS+=("masters|$DOCS_DIR/masters_dissertation.tex|masters_dissertation.tex|8")
    CORPUS+=("phd|$DOCS_DIR/phd_dissertation.tex|phd_dissertation.tex|3")
fi

# --- Active tool list --------------------------------------------------------

declare -a TOOLS=("badness")
[ "$HAVE_TEXFMT" = "yes" ]      && TOOLS+=("tex-fmt")
[ "$HAVE_LATEXINDENT" = "yes" ] && TOOLS+=("latexindent")

# Command template per tool, with FILE substituted at call time (single-file,
# stdin → stdout).
cmd_for() {
    local tool="$1" file="$2"
    case "$tool" in
        badness)     echo "$BADNESS format --no-config --stdin-filepath bench.tex < '$file'" ;;
        tex-fmt)     echo "tex-fmt --stdin < '$file'" ;;
        latexindent) echo "latexindent -g /dev/null - < '$file'" ;;
    esac
}

# Command template for the whole-project (folder) benchmark: format a directory
# recursively in read-only `--check` mode (the folder analog of stdin → stdout —
# full formatting work, nothing written). Only badness and tex-fmt support this;
# latexindent has no recursive directory mode and is excluded from the folder
# comparison.
dir_cmd_for() {
    local tool="$1" dir="$2"
    case "$tool" in
        badness) echo "$BADNESS format --no-config --check '$dir'" ;;
        tex-fmt) echo "tex-fmt --check --recursive '$dir'" ;;
    esac
}

# --- Run ---------------------------------------------------------------------

# Accumulators, indexed in lockstep so the renderers can walk them.
declare -a DOC_ID=() DOC_LABEL=() DOC_FILE=() DOC_SIZE=() DOC_LINES=() DOC_ITERS=()
declare -a RES_DOC=() RES_TOOL=() RES_MEAN=() RES_STDDEV=() RES_MIN=() RES_MAX=() RES_RUNS=()

for entry in "${CORPUS[@]}"; do
    IFS='|' read -r id file label iters <<< "$entry"

    if [ ! -f "$file" ]; then
        log "⚠️  skip $label — not found ($file; run $DOCS_DIR/download.sh?)"
        continue
    fi

    # Sanity gate: badness must format the doc (it refuses input with parser
    # diagnostics). Skip any doc it cannot handle yet, with a note.
    if ! "$BADNESS" format --no-config --stdin-filepath bench.tex < "$file" >/dev/null 2>&1; then
        log "⚠️  skip $label — badness cannot format it yet (parser diagnostics)"
        continue
    fi

    size=$(wc -c < "$file"); lines=$(wc -l < "$file")
    DOC_ID+=("$id"); DOC_LABEL+=("$label"); DOC_FILE+=("$file")
    DOC_SIZE+=("$size"); DOC_LINES+=("$lines"); DOC_ITERS+=("$iters")

    log "━━ $label ($size bytes, $lines lines) ━━"
    for tool in "${TOOLS[@]}"; do
        cmd="$(cmd_for "$tool" "$file")"
        log "  $tool..."
        read -r mean stddev min max runs < <(run_one "$iters" "$cmd")
        RES_DOC+=("$id"); RES_TOOL+=("$tool"); RES_MEAN+=("$mean")
        RES_STDDEV+=("$stddev"); RES_MIN+=("$min"); RES_MAX+=("$max"); RES_RUNS+=("$runs")
    done
    log
done

# --- Whole-project (folder) benchmark ----------------------------------------
# A recursive `--check` over a real multi-file project (badness vs tex-fmt only;
# latexindent has no recursive mode). We benchmark a throwaway copy so the walk
# sees an un-gitignored, `.tex`-only tree — `benches/documents/.gitignore` hides
# `*.tex`, and `badness format` is `.tex`-only while tex-fmt would also touch
# `.bib`/`.cls`, so staging just the `.tex` fragments keeps the compared set
# identical. Any file badness cannot format yet is dropped from *both* tools via
# a generated `.ignore` (both honor it), so the comparison stays symmetric.

if [ -z "${BADNESS_BENCH_INPUT:-}" ] && [ -d "$PROJECT_SRC" ]; then
    PROJECT_STAGE=$(mktemp -d)
    # The source holds only .tex fragments (download.sh fetches nothing else), so
    # a plain recursive copy stages the tree, preserving its subdirectory layout.
    cp -r "$PROJECT_SRC/." "$PROJECT_STAGE/"

    proj_bytes=0; proj_lines=0; proj_files=0; proj_excluded=0
    while IFS= read -r f; do
        if "$BADNESS" format --no-config --stdin-filepath bench.tex < "$f" >/dev/null 2>&1; then
            proj_files=$((proj_files + 1))
            proj_bytes=$((proj_bytes + $(wc -c < "$f")))
            proj_lines=$((proj_lines + $(wc -l < "$f")))
        else
            # Exclude from both walks (relative path, gitignore semantics).
            printf '/%s\n' "${f#"$PROJECT_STAGE"/}" >> "$PROJECT_STAGE/.ignore"
            proj_excluded=$((proj_excluded + 1))
        fi
    done < <(find "$PROJECT_STAGE" -name '*.tex' | sort)

    if [ "$proj_files" -eq 0 ]; then
        log "⚠️  skip project — badness cannot format any of its files yet"
    else
        label="project ($proj_files files)"
        [ "$proj_excluded" -gt 0 ] && \
            log "   ($proj_excluded file(s) excluded from both tools — badness cannot format them yet)"
        DOC_ID+=("project"); DOC_LABEL+=("$label"); DOC_FILE+=("$PROJECT_SRC")
        DOC_SIZE+=("$proj_bytes"); DOC_LINES+=("$proj_lines"); DOC_ITERS+=("$PROJECT_ITERS")

        log "━━ $label ($proj_bytes bytes, $proj_lines lines; recursive --check) ━━"
        for tool in badness tex-fmt; do
            [ "$tool" = "tex-fmt" ] && [ "$HAVE_TEXFMT" != "yes" ] && continue
            cmd="$(dir_cmd_for "$tool" "$PROJECT_STAGE")"
            log "  $tool..."
            read -r mean stddev min max runs < <(run_one "$PROJECT_ITERS" "$cmd")
            RES_DOC+=("project"); RES_TOOL+=("$tool"); RES_MEAN+=("$mean")
            RES_STDDEV+=("$stddev"); RES_MIN+=("$min"); RES_MAX+=("$max"); RES_RUNS+=("$runs")
        done
        log
    fi
fi

[ "${#DOC_ID[@]}" -gt 0 ] || { echo "error: no documents benchmarked (corpus missing or all gated out)" >&2; exit 1; }

# --- Render JSON -------------------------------------------------------------

mkdir -p "$(dirname "$JSON_OUT")"
{
        printf '{\n'
        printf '  "schema_version": 1,\n'
        printf '  "meta": {\n'
        printf '    "generated_at": "%s",\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
        printf '    "host": {"os": "%s", "arch": "%s", "cpu": "%s"},\n' \
            "$(json_escape "$HOST_OS")" "$(json_escape "$HOST_ARCH")" "$(json_escape "$HOST_CPU")"
        printf '    "backend": "%s",\n' "$BACKEND"
        printf '    "min_runs": %d,\n' "$HYPERFINE_MIN_RUNS"
        printf '    "tools": {\n'
        printf '      "badness": {"version": "%s"}' "$(json_escape "$BADNESS_VER")"
        [ "$HAVE_TEXFMT" = "yes" ]      && printf ',\n      "tex-fmt": {"version": "%s"}' "$(json_escape "$TEXFMT_VER")"
        [ "$HAVE_LATEXINDENT" = "yes" ] && printf ',\n      "latexindent": {"version": "%s"}' "$(json_escape "$LATEXINDENT_VER")"
        printf '\n    }\n'
        printf '  },\n'

        printf '  "documents": [\n'
        for i in "${!DOC_ID[@]}"; do
            printf '    {"id":"%s","name":"%s","file":"%s","size_bytes":%d,"lines":%d,"iterations":%d}' \
                "${DOC_ID[$i]}" "$(json_escape "${DOC_LABEL[$i]}")" "$(basename "${DOC_FILE[$i]}")" \
                "${DOC_SIZE[$i]}" "${DOC_LINES[$i]}" "${DOC_ITERS[$i]}"
            [ "$i" -lt $((${#DOC_ID[@]} - 1)) ] && printf ','
            printf '\n'
        done
        printf '  ],\n'

        printf '  "results": [\n'
        for i in "${!RES_DOC[@]}"; do
            printf '    {"document":"%s","formatter":"%s","mean_ms":%s,"stddev_ms":%s,"min_ms":%s,"max_ms":%s,"runs":%d}' \
                "${RES_DOC[$i]}" "${RES_TOOL[$i]}" "${RES_MEAN[$i]}" \
                "${RES_STDDEV[$i]}" "${RES_MIN[$i]}" "${RES_MAX[$i]}" "${RES_RUNS[$i]}"
            [ "$i" -lt $((${#RES_DOC[@]} - 1)) ] && printf ','
            printf '\n'
        done
        printf '  ]\n'
        printf '}\n'
} > "$JSON_OUT"

log "JSON written to: $JSON_OUT"
echo "$JSON_OUT"
