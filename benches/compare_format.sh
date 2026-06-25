#!/usr/bin/env bash
#
# Benchmark badness's formatter speed against other LaTeX formatters
# (tex-fmt, latexindent) on a corpus of real documents, using hyperfine.
#
# Usage:
#   ./benches/compare_format.sh                      # human report → BENCH.md
#   ./benches/compare_format.sh --json [--out PATH]  # structured JSON artifact
#   BADNESS_BENCH_INPUT=path/to/file.tex ./benches/compare_format.sh
#                                                    # benchmark one real file
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
BADNESS="$REPO_ROOT/target/release/badness"
HYPERFINE_MIN_RUNS=3

JSON_MODE=0
JSON_OUT="benches/benchmark_results.json"
BENCH_MD="BENCH.md"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --json) JSON_MODE=1; shift ;;
        --out)  JSON_OUT="$2"; shift 2 ;;
        -h|--help) sed -n '3,17p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; exit 2 ;;
    esac
done

# Progress goes to stderr in JSON mode so stdout/JSON stays clean.
if [ "$JSON_MODE" = "1" ]; then LOG_FD=2; else LOG_FD=1; fi
log() { echo -e "$@" >&$LOG_FD; }

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
        hyperfine --warmup 1 --min-runs "$HYPERFINE_MIN_RUNS" \
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
        for ((i=1; i<=iterations; i++)); do eval "$cmd" >/dev/null 2>&1; done
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

# Command template per tool, with FILE substituted at call time.
cmd_for() {
    local tool="$1" file="$2"
    case "$tool" in
        badness)     echo "$BADNESS format --no-config --stdin-filepath bench.tex < '$file'" ;;
        tex-fmt)     echo "tex-fmt --stdin < '$file'" ;;
        latexindent) echo "latexindent -g /dev/null - < '$file'" ;;
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

[ "${#DOC_ID[@]}" -gt 0 ] || { echo "error: no documents benchmarked (corpus missing or all gated out)" >&2; exit 1; }

# Look up a result mean for (doc, tool); empty if not present.
mean_of() {
    local d="$1" t="$2" i
    for i in "${!RES_DOC[@]}"; do
        if [ "${RES_DOC[$i]}" = "$d" ] && [ "${RES_TOOL[$i]}" = "$t" ]; then
            echo "${RES_MEAN[$i]}"; return
        fi
    done
}

# --- Render JSON -------------------------------------------------------------

if [ "$JSON_MODE" = "1" ]; then
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
    exit 0
fi

# --- Render BENCH.md ---------------------------------------------------------

ratio_cell() {
    # "$tool_mean" relative to "$badness_mean" → human ratio string.
    local tool_mean="$1" base_mean="$2"
    awk -v t="$tool_mean" -v b="$base_mean" 'BEGIN {
        if (b <= 0 || t <= 0) { print "—"; exit }
        r = t / b
        if (r >= 1) printf "%.1f× slower", r
        else        printf "%.1f× faster", 1 / r
    }'
}

{
    echo "# Formatter benchmark: badness vs tex-fmt & latexindent"
    echo
    echo "Wall-clock formatting speed of \`badness\` against"
    echo "[\`tex-fmt\`](https://github.com/wgunderwood/tex-fmt) and"
    echo "[\`latexindent\`](https://github.com/cmhughes/latexindent.pl), measured with"
    echo "[hyperfine]. Every tool formats stdin → stdout, so the comparison is free of"
    echo "file-mutation and exit-code noise."
    echo
    echo "**This is not a CI gate and not a parity target.** Timings are machine- and"
    echo "run-dependent, and this file measures *speed only*, never output equivalence."
    echo "The tools also do different work: \`latexindent\` only indents by default and"
    echo "does no line reflow, while \`badness\` and \`tex-fmt\` wrap — so a raw speed"
    echo "comparison is a snapshot of each tool at its defaults, not equal work."
    echo "Regenerate with \`task bench\`."
    echo
    echo "[hyperfine]: https://github.com/sharkdp/hyperfine"
    echo
    echo "## Setup"
    echo
    echo "- **badness**: \`$BADNESS_VER\`"
    if [ "$HAVE_TEXFMT" = "yes" ]; then echo "- **tex-fmt**: \`$TEXFMT_VER\`"; else echo "- **tex-fmt**: not measured (not installed)"; fi
    if [ "$HAVE_LATEXINDENT" = "yes" ]; then echo "- **latexindent**: \`$LATEXINDENT_VER\`"; else echo "- **latexindent**: not measured (not installed)"; fi
    echo "- **backend**: $BACKEND (min runs: $HYPERFINE_MIN_RUNS)"
    [ -n "$HOST_CPU" ] && echo "- **host**: $HOST_OS/$HOST_ARCH, $HOST_CPU"
    echo
    echo "Corpus is real LaTeX: a committed \`small.tex\` baseline plus documents fetched"
    echo "by \`benches/documents/download.sh\` (gitignored). Documents \`badness\` cannot"
    echo "yet format (parser diagnostics) are skipped."
    echo
    echo "## Results"
    for i in "${!DOC_ID[@]}"; do
        id="${DOC_ID[$i]}"
        base="$(mean_of "$id" badness)"
        echo
        echo "### ${DOC_LABEL[$i]} (${DOC_SIZE[$i]} bytes, ${DOC_LINES[$i]} lines)"
        echo
        echo "| Tool | Mean (ms) | Min (ms) | Max (ms) | Relative |"
        echo "| --- | ---: | ---: | ---: | --- |"
        for tool in "${TOOLS[@]}"; do
            m="$(mean_of "$id" "$tool")"
            [ -n "$m" ] || continue
            # Find the matching min/max for this (doc, tool).
            mn=""; mx=""
            for j in "${!RES_DOC[@]}"; do
                if [ "${RES_DOC[$j]}" = "$id" ] && [ "${RES_TOOL[$j]}" = "$tool" ]; then
                    mn="${RES_MIN[$j]}"; mx="${RES_MAX[$j]}"; break
                fi
            done
            [ "$mn" = "null" ] && mn="—"
            [ "$mx" = "null" ] && mx="—"
            if [ "$tool" = "badness" ]; then rel="baseline"; else rel="$(ratio_cell "$m" "$base")"; fi
            printf '| %s | %s | %s | %s | %s |\n' "$tool" "$m" "$mn" "$mx" "$rel"
        done
    done
} > "$BENCH_MD"

log "Report written to: $BENCH_MD"
