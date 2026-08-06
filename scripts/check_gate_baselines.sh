#!/usr/bin/env bash
# Machine check for tests/gate_baselines: re-run both gates over the pinned
# corpora (fetched by scripts/fetch_gate_corpora.sh) and compare the distilled
# failure sets against the recorded baselines.
#
# The ratchet is two-sided, like the in-repo KNOWN_INVARIANT_FAILURES registry:
# an ADDED line is a regression and fails the check; a REMOVED line means the
# baseline is stale (something got fixed) and also fails, with instructions to
# re-record — so the recorded sets always match reality.
#
# Usage: check_gate_baselines.sh [corpus...]   (default: latex3 latex2e pgf)
# Env:   BADNESS=/path/to/badness   (default: target/release/badness)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BADNESS="${BADNESS:-${REPO_ROOT}/target/release/badness}"
CORPORA_DIR="${REPO_ROOT}/corpora"
BASELINE_DIR="${REPO_ROOT}/tests/gate_baselines"

CORPORA=("$@")
if [ "${#CORPORA[@]}" -eq 0 ]; then
  CORPORA=(latex3 latex2e pgf)
fi

if [ ! -x "${BADNESS}" ]; then
  echo "error: ${BADNESS} not found or not executable — build it first:" >&2
  echo "  cargo build --release" >&2
  exit 2
fi

# Distill a --report stream into baseline lines on stdout.
#   mode=all    -> "path<TAB>kind"
#   mode=trivia -> "path<TAB>kind<TAB>class", the class read from the failure's
#                  `- Variant:` reason (format-error findings have none and
#                  class as themselves).
distill() {
  local mode="$1"
  awk -v mode="${mode}" '
    function flush() {
      if (path == "") return
      if (mode == "all") print path "\t" kind
      else print path "\t" kind "\t" cls
      path = ""
    }
    /^### [0-9]+\. `/ {
      flush()
      line = $0
      sub(/^### [0-9]+\. `/, "", line)
      path = line; sub(/`.*/, "", path)
      kind = line; sub(/.*` \(/, "", kind); sub(/\)$/, "", kind)
      cls = (kind == "format-error") ? "format-error" : "unclassified"
    }
    /^- Variant: / {
      if ($0 ~ /non-trivia content/) cls = "content-change"
      else if ($0 ~ /did not reach a fixed point/) cls = "non-fixed-point"
      else if ($0 ~ /parse without diagnostics/) cls = "parse-error"
      else if ($0 ~ /round-trip losslessly/) cls = "lossless-error"
      else if ($0 ~ /failed to (re-)?format/) cls = "format-error"
    }
    END { flush() }
  ' | LC_ALL=C sort -u
}

failed=0

check_one() {
  local corpus="$1" gate="$2" # gate: all | trivia
  local baseline="${BASELINE_DIR}/${corpus}.${gate}.txt"
  if [ ! -f "${baseline}" ]; then
    echo "error: missing baseline ${baseline}" >&2
    failed=1
    return
  fi
  local current
  # The report exits 1 whenever failures exist — that is the recorded state,
  # not an error of this check.
  current="$(cd "${CORPORA_DIR}/${corpus}" \
    && "${BADNESS}" --no-config debug format --checks "${gate}" --report . || true)"
  local got added removed
  got="$(printf '%s\n' "${current}" | distill "${gate}")"
  added="$(LC_ALL=C comm -13 <(LC_ALL=C sort -u "${baseline}") <(printf '%s\n' "${got}") || true)"
  removed="$(LC_ALL=C comm -23 <(LC_ALL=C sort -u "${baseline}") <(printf '%s\n' "${got}") || true)"
  if [ -n "${added}" ]; then
    echo "REGRESSION: ${corpus}.${gate} grew (new failures not in the baseline):"
    printf '%s\n' "${added}" | sed 's/^/  + /'
    failed=1
  fi
  if [ -n "${removed}" ]; then
    echo "STALE BASELINE: ${corpus}.${gate} shrank (recorded failures now pass):"
    printf '%s\n' "${removed}" | sed 's/^/  - /'
    echo "  Re-record ${baseline} (remove these lines) so the sets match reality."
    failed=1
  fi
  if [ -z "${added}" ] && [ -z "${removed}" ]; then
    echo "ok: ${corpus}.${gate} matches the baseline ($(printf '%s\n' "${got}" | grep -c . || true) entries)"
  fi
}

for corpus in "${CORPORA[@]}"; do
  if [ ! -d "${CORPORA_DIR}/${corpus}" ]; then
    echo "error: ${CORPORA_DIR}/${corpus} not found — fetch the corpora first:" >&2
    echo "  task gate-corpora:fetch" >&2
    exit 2
  fi
  check_one "${corpus}" all
  check_one "${corpus}" trivia
done

exit "${failed}"
