#!/usr/bin/env bash
# Machine check for tests/reparse_baselines: re-run the incremental-reparse corpus
# sweep over the pinned corpora (fetched by scripts/fetch_gate_corpora.sh) and
# compare the recorded tallies against the baselines.
#
# The ratchet is two-sided, like scripts/check_gate_baselines.sh: a splice rate that
# fell is a REGRESSION (a guard narrowed a tier), one that rose means the baseline is
# STALE (a tier widened and the record has to catch up), and a row whose counts moved
# without the rate moving is CHANGED — most often a workload shifting tier, which no
# floor can catch because declining is always sound. All three fail, so the recorded
# sets always match reality.
#
# The invariant itself is *not* baselined and never will be: a reparse that diverges
# from a full parse is a bug, so the sweep asserts it directly and this script only
# ever sees the tallies. Same for the per-driver splice floors the sweep carries —
# those survive a careless re-record of these files.
#
# Usage: check_reparse_baselines.sh [corpus...]   (default: latex3 latex2e pgf latexindent)
# Env:   RECORD=1   write the baselines instead of diffing them
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CORPORA_DIR="${REPO_ROOT}/corpora"
BASELINE_DIR="${REPO_ROOT}/tests/reparse_baselines"

CORPORA=("$@")
if [ "${#CORPORA[@]}" -eq 0 ]; then
  CORPORA=(latex3 latex2e pgf latexindent)
fi

for corpus in "${CORPORA[@]}"; do
  if [ ! -d "${CORPORA_DIR}/${corpus}" ]; then
    echo "error: ${CORPORA_DIR}/${corpus} not found — fetch the corpora first:" >&2
    echo "  task gate-corpora:fetch" >&2
    exit 2
  fi
done

TAB="$(printf '\t')"
tmp="$(mktemp -d)"
trap 'rm -rf "${tmp}"' EXIT

# The sweep exits non-zero when a splice floor trips. That is a real failure, but it
# still printed every row before asserting (deliberately), so the diff below is worth
# showing first — it is what says whether a guard narrowed or a workload moved.
sweep_status=0
BADNESS_REPARSE_SWEEP_CORPORA="${CORPORA[*]}" \
  cargo test --release -p badness-parser --test reparse_corpus_sweep \
  -- --ignored --nocapture >"${tmp}/raw" 2>&1 || sweep_status=$?

# Distill the run into baseline lines: every `sweep<TAB>…` line with the marker
# dropped, so a line is `corpus<TAB>row<TAB>fields…`.
awk -F'\t' '$1 == "sweep" {
  line = $2
  for (i = 3; i <= NF; i++) line = line "\t" $i
  print line
}' "${tmp}/raw" >"${tmp}/got"

if [ ! -s "${tmp}/got" ]; then
  echo "error: the sweep produced no rows — its output follows:" >&2
  cat "${tmp}/raw" >&2
  exit 2
fi

mkdir -p "${BASELINE_DIR}"
failed=0

for corpus in "${CORPORA[@]}"; do
  baseline="${BASELINE_DIR}/${corpus}.txt"
  grep "^${corpus}${TAB}" "${tmp}/got" >"${tmp}/${corpus}.got" || true

  if [ -n "${RECORD:-}" ]; then
    cp "${tmp}/${corpus}.got" "${baseline}"
    echo "recorded: ${baseline} ($(wc -l <"${baseline}" | tr -d ' ') rows)"
    continue
  fi

  if [ ! -f "${baseline}" ]; then
    echo "error: missing baseline ${baseline} — record it with RECORD=1" >&2
    failed=1
    continue
  fi

  if diff -q "${baseline}" "${tmp}/${corpus}.got" >/dev/null; then
    echo "ok: ${corpus} matches the baseline ($(wc -l <"${baseline}" | tr -d ' ') rows)"
    continue
  fi

  failed=1
  echo "${corpus}: the recorded tallies moved."
  # Classify each moved row by direction, then show the raw diff. The direction is
  # what says which of the three cases this is; the diff is what says by how much.
  awk -F'\t' -v corpus="${corpus}" '
    function rate(field,   parts) {
      # field looks like `spliced=1590/3940`; a `corpus` header row has none.
      if (field !~ /^spliced=/) return -1
      sub(/^spliced=/, "", field)
      split(field, parts, "/")
      return parts[2] == 0 ? 0 : parts[1] / parts[2]
    }
    NR == FNR { was[$2] = $0; wasrate[$2] = rate($3); seen[$2] = 1; next }
    {
      now[$2] = $0; nowrate[$2] = rate($3)
      if (!($2 in seen)) { printf "  NEW ROW      %s\n", $2; next }
      if (was[$2] == $0) next
      if (nowrate[$2] < wasrate[$2])      printf "  REGRESSION   %s: splice rate fell\n", $2
      else if (nowrate[$2] > wasrate[$2]) printf "  STALE        %s: splice rate rose\n", $2
      else                                printf "  CHANGED      %s: same rate, different counts\n", $2
    }
    END {
      for (row in was) if (!(row in now)) printf "  MISSING ROW  %s\n", row
    }
  ' "${baseline}" "${tmp}/${corpus}.got"
  diff -u "${baseline}" "${tmp}/${corpus}.got" | sed '1,2d;s/^/  /' || true
  echo "  Re-record with: RECORD=1 ./scripts/check_reparse_baselines.sh ${corpus}"
done

if [ "${sweep_status}" -ne 0 ]; then
  echo
  echo "The sweep itself failed (exit ${sweep_status}) — a splice floor tripped or an" >&2
  echo "edit diverged from a full parse. Its output follows:" >&2
  cat "${tmp}/raw" >&2
  exit "${sweep_status}"
fi

exit "${failed}"
