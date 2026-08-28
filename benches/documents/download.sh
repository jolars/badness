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
    curl -sSL --create-dirs -o "$out" "${RAW}/${path}"
}

# small  → committed baseline (small.tex), no download
fetch cv.tex                   tests/cv/source/cv.tex
fetch masters_dissertation.tex tests/masters_dissertation/source/masters_dissertation.tex
fetch phd_dissertation.tex     tests/phd_dissertation/source/phd_dissertation.tex

# --- Multi-file project corpus (folder / whole-project benchmark) -------------
#
# A real, pinned multi-file LaTeX thesis (kks32/phd-thesis-template). The full
# checkout is the workspace for the external LSP speed and memory benchmark, including
# its class, style, bibliography, and image assets. `compare_format.sh` stages
# its own explicit `.tex` subset so expanding this checkout cannot silently
# change the formatter speed corpus.

PROJECT_REPOSITORY="https://github.com/kks32/phd-thesis-template.git"
PROJECT_REF="v2.4"
PROJECT_COMMIT="3ce347686d75747f69d9e736acd46a9393a1b332"
PROJECT_DIR="project"

echo
echo "Downloading project corpus (phd-thesis-template @ ${PROJECT_REF}, ${PROJECT_COMMIT})..."
echo

PROJECT_TMP=$(mktemp -d)
trap 'rm -rf "$PROJECT_TMP"' EXIT
git init --quiet "$PROJECT_TMP/checkout"
git -C "$PROJECT_TMP/checkout" remote add origin "$PROJECT_REPOSITORY"
git -C "$PROJECT_TMP/checkout" fetch --quiet --depth 1 origin \
    "refs/tags/${PROJECT_REF}:refs/tags/${PROJECT_REF}"
ACTUAL_COMMIT=$(git -C "$PROJECT_TMP/checkout" rev-parse "${PROJECT_REF}^{commit}")
if [ "$ACTUAL_COMMIT" != "$PROJECT_COMMIT" ]; then
    echo "error: ${PROJECT_REF} resolved to ${ACTUAL_COMMIT}, expected ${PROJECT_COMMIT}" >&2
    exit 1
fi
git -C "$PROJECT_TMP/checkout" checkout --quiet --detach "$PROJECT_COMMIT"

mkdir "$PROJECT_TMP/staged"
git -C "$PROJECT_TMP/checkout" archive "$PROJECT_COMMIT" | tar -x -C "$PROJECT_TMP/staged"
printf '%s\n' "$PROJECT_COMMIT" > "$PROJECT_TMP/staged/.benchmark-commit"
rm -rf -- "$DOCS_DIR/$PROJECT_DIR"
mv "$PROJECT_TMP/staged" "$DOCS_DIR/$PROJECT_DIR"

echo
echo "✅ Done. File sizes:"
du -h ./*.tex 2>/dev/null || true
du -sh "./${PROJECT_DIR}" 2>/dev/null || true
echo
echo "Run the benchmark with: task bench  (or ./benches/compare_format.sh)"
