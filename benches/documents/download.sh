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
# A real, pinned multi-file LaTeX thesis (kks32/phd-thesis-template): a main
# `thesis.tex` that `\input`s per-chapter/appendix/front-matter fragments. This
# is the corpus for the recursive folder benchmark (badness vs tex-fmt only —
# latexindent has no recursive directory mode). Only the `.tex` fragments are
# fetched, and `compare_format.sh` benchmarks a clean temp copy of them, so the
# two tools walk an identical `.tex`-only file set (`badness format` is
# `.tex`-only, tex-fmt would otherwise also touch `.bib`/`.cls`). The generated
# 175 kB `Classes/glyphtounicode.tex` glyph map is intentionally skipped: it is a
# machine-written table, not representative document prose.

PROJECT_REF="v2.4"  # https://github.com/kks32/phd-thesis-template
PROJECT_RAW="https://raw.githubusercontent.com/kks32/phd-thesis-template/${PROJECT_REF}"
PROJECT_DIR="project"

echo
echo "Downloading project corpus (phd-thesis-template @ ${PROJECT_REF})..."
echo

fetch_project() {
    local path="$1"
    echo "📄 ${PROJECT_DIR}/${path}"
    curl -sSL --create-dirs -o "${PROJECT_DIR}/${path}" "${PROJECT_RAW}/${path}"
}

for f in \
    thesis.tex \
    thesis-info.tex \
    Preamble/preamble.tex \
    Abstract/abstract.tex \
    Acknowledgement/acknowledgement.tex \
    Dedication/dedication.tex \
    Declaration/declaration.tex \
    Chapter1/chapter1.tex \
    Chapter2/chapter2.tex \
    Chapter3/chapter3.tex \
    Appendix1/appendix1.tex \
    Appendix2/appendix2.tex; do
    fetch_project "$f"
done

echo
echo "✅ Done. File sizes:"
du -h ./*.tex 2>/dev/null || true
du -sh "./${PROJECT_DIR}" 2>/dev/null || true
echo
echo "Run the benchmark with: task bench  (or ./benches/compare_format.sh)"
