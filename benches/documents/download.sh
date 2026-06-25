#!/usr/bin/env bash
#
# Fetch the real-world LaTeX corpus for the formatter benchmark
# (`benches/compare_format.sh`). The small baseline `small.tex` is committed, so
# the benchmark runs with zero network; these larger documents add a size
# gradient and are gitignored (see `.gitignore` in this directory).
#
# Sources are pinned to a tex-fmt release tag for reproducibility. They are real
# LaTeX documents from tex-fmt's own test corpus — a CV, a master's
# dissertation, and a PhD dissertation — so the benchmark measures realistic
# input, not synthetic filler. (tex-fmt is also one of the tools we compare
# against, and benchmarks itself on the same dissertations.)
#
# Note: `badness format` only formats fully parseable input, so any document it
# cannot parse yet is skipped by the benchmark's sanity gate (with a note),
# regardless of whether it downloads here. The large PhD dissertation currently
# falls into that bucket; it is fetched anyway so it is picked up automatically
# once the parser covers it.

set -euo pipefail

DOCS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$DOCS_DIR"

# Pinned tex-fmt release tag (https://github.com/wgunderwood/tex-fmt).
TEXFMT_REF="v0.5.7"
RAW="https://raw.githubusercontent.com/wgunderwood/tex-fmt/${TEXFMT_REF}"

echo "Downloading benchmark documents (tex-fmt @ ${TEXFMT_REF})..."
echo

fetch() {
    local out="$1" path="$2"
    echo "📄 $out"
    curl -sSL -o "$out" "${RAW}/${path}"
}

# small  → committed baseline (small.tex), no download
fetch cv.tex                   tests/cv/source/cv.tex
fetch masters_dissertation.tex tests/masters_dissertation/source/masters_dissertation.tex
fetch phd_dissertation.tex     tests/phd_dissertation/source/phd_dissertation.tex

echo
echo "✅ Done. File sizes:"
du -h ./*.tex 2>/dev/null || true
echo
echo "Run the benchmark with: task bench  (or ./benches/compare_format.sh)"
