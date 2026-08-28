#!/usr/bin/env bash
# Compare speed and whole-process-tree RSS/PSS for fresh Badness and TexLab
# language-server sessions. The committed JSON artifact feeds the docs page.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROJECT="$REPO_ROOT/benches/documents/project"
BADNESS="$REPO_ROOT/target/release/badness"
PROJECT_REPOSITORY="https://github.com/kks32/phd-thesis-template"
PROJECT_TAG="v2.4"
PROJECT_COMMIT="3ce347686d75747f69d9e736acd46a9393a1b332"

RUNS="${RUNS:-3}"
SAMPLE_INTERVAL="${SAMPLE_INTERVAL:-0.15}"
QUIET_SECONDS="${QUIET_SECONDS:-5}"
SETTLE_TIMEOUT="${SETTLE_TIMEOUT:-60}"
LSP_LATENCY_RUNS="${LSP_LATENCY_RUNS:-20}"
LSP_LATENCY_WARMUPS="${LSP_LATENCY_WARMUPS:-2}"
JSON_OUT="$REPO_ROOT/benches/memory_results.json"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --out) JSON_OUT="$2"; shift 2 ;;
        -h|--help) sed -n '2,4p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

if [ "$(uname -s)" != "Linux" ] || [ ! -r /proc/self/smaps_rollup ]; then
    echo "error: the LSP benchmark requires Linux with readable /proc smaps_rollup" >&2
    exit 1
fi
for tool in cargo git python3 texlab; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "error: required tool '$tool' is not on PATH" >&2
        exit 1
    }
done

if [ ! -f "$PROJECT/.benchmark-commit" ] || \
   [ "$(cat "$PROJECT/.benchmark-commit")" != "$PROJECT_COMMIT" ]; then
    "$REPO_ROOT/benches/documents/download.sh"
fi

OPEN_FILES=(
    "$PROJECT/thesis.tex"
    "$PROJECT/Chapter1/chapter1.tex"
    "$PROJECT/Chapter2/chapter2.tex"
    "$PROJECT/Preamble/preamble.tex"
    "$PROJECT/Chapter3/chapter3.tex"
)
for path in "${OPEN_FILES[@]}"; do
    [ -f "$path" ] || {
        echo "error: corpus is missing ${path#"$PROJECT/"}" >&2
        exit 1
    }
done

echo ">> Building release binary..."
cargo build --release --quiet --manifest-path "$REPO_ROOT/Cargo.toml"

TEXLAB=$(command -v texlab)
BADNESS_VERSION=$("$BADNESS" --version | awk '{print $2}')
TEXLAB_VERSION=$("$TEXLAB" --version | awk '{print $2}')

BENCH_TMP=$(mktemp -d)
trap 'rm -rf "$BENCH_TMP"' EXIT

python3 "$REPO_ROOT/benches/lsp_memory_compare.py" \
    --project "$PROJECT" \
    --files "${OPEN_FILES[@]}" \
    --server "badness=$BADNESS lsp" \
    --server "texlab=$TEXLAB run" \
    --runs "$RUNS" \
    --sample-interval "$SAMPLE_INTERVAL" \
    --quiet-seconds "$QUIET_SECONDS" \
    --settle-timeout "$SETTLE_TIMEOUT" \
    --latency-runs "$LSP_LATENCY_RUNS" \
    --latency-warmups "$LSP_LATENCY_WARMUPS" \
    --stderr-dir "$BENCH_TMP/stderr" \
    --scratch-dir "$BENCH_TMP/scratch" \
    --badness-version "$BADNESS_VERSION" \
    --texlab-version "$TEXLAB_VERSION" \
    --corpus-repository "$PROJECT_REPOSITORY" \
    --corpus-tag "$PROJECT_TAG" \
    --corpus-commit "$PROJECT_COMMIT" \
    --out "$BENCH_TMP/memory_results.json"

install -Dm 0644 "$BENCH_TMP/memory_results.json" "$JSON_OUT"

echo ">> Wrote $JSON_OUT"
