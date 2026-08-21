#!/usr/bin/env bash
# Manual gate: prove that formatting a document does not change what it typesets.
#
# Most invariants (losslessness, whitespace-only, trivia convergence) are
# checked against the CST, which by construction cannot prove that changing
# whitespace leaves TeX's output alone. A keyval break can materialize a space
# token, while math ancestry can hide a space captured and inspected by a macro.
# A space is trivia to the CST but may be content to TeX. Only a real compile can
# tell the difference.
#
# So: compile each input, format it, compile again, and diff the extracted text.
# Any difference is a formatter bug—most likely an over-broad content license
# applied to whitespace that TeX observes.
#
# Needs a TeX installation, so it never runs in CI. Run it when touching keyval
# or math signatures and their lowering rules.
#
# Usage: check_typeset_stability.sh [file.tex...]   (default: tests/typeset/*.tex)
# Env:   BADNESS=/path/to/badness   (default: target/release/badness)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BADNESS="${BADNESS:-${REPO_ROOT}/target/release/badness}"

INPUTS=("$@")
if [ "${#INPUTS[@]}" -eq 0 ]; then
  shopt -s nullglob
  INPUTS=("${REPO_ROOT}"/tests/typeset/*.tex)
  shopt -u nullglob
fi
if [ "${#INPUTS[@]}" -eq 0 ]; then
  echo "error: no inputs given and tests/typeset/ is empty" >&2
  exit 2
fi
for tool in pdflatex pdftotext; do
  command -v "${tool}" >/dev/null || { echo "error: ${tool} not found" >&2; exit 2; }
done
[ -x "${BADNESS}" ] || { echo "error: ${BADNESS} not found — cargo build --release" >&2; exit 2; }

work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT
export SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1

failed=0
for input in "${INPUTS[@]}"; do
  name="$(basename "${input}" .tex)"
  dir="${work}/${name}"
  mkdir -p "${dir}"
  cp "${input}" "${dir}/before.tex"
  cp "${input}" "${dir}/after.tex"
  "${BADNESS}" --no-config format "${dir}/after.tex"
  if cmp -s "${dir}/before.tex" "${dir}/after.tex"; then
    echo "skip: ${name} (already formatted — nothing to compare)"
    continue
  fi
  for side in before after; do
    # Two passes so the LOF/LOT and other cross-references settle.
    #
    # `.stdout`, never `.out`: hyperref writes its bookmarks to `\jobname.out`, so
    # capturing the terminal log there hands the *next* pass its own log as the
    # bookmark file — which typesets the log into the PDF and can hang the run.
    (cd "${dir}" && pdflatex -interaction=nonstopmode "${side}.tex" >"${side}.stdout" 2>&1) || true
    (cd "${dir}" && pdflatex -interaction=nonstopmode "${side}.tex" >"${side}.stdout" 2>&1) || true
    if [ ! -f "${dir}/${side}.pdf" ]; then
      echo "error: ${name} (${side}) failed to compile; see ${dir}/${side}.stdout" >&2
      failed=1
      continue 2
    fi
  done
  if diff -q <(pdftotext -layout "${dir}/before.pdf" -) \
             <(pdftotext -layout "${dir}/after.pdf" -) >/dev/null; then
    echo "ok: ${name} typesets identically after formatting"
  else
    echo "TYPESET CHANGE: ${name}"
    diff <(pdftotext -layout "${dir}/before.pdf" -) \
         <(pdftotext -layout "${dir}/after.pdf" -) | sed 's/^/  /'
    failed=1
  fi
done

exit "${failed}"
