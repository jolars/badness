#!/usr/bin/env bash
# Survey strict trivia invariance (`fmt(perturbed) == fmt(original)`) over the
# pinned gate corpora and print a per-corpus histogram of reproducer kinds.
#
# This is deliberately NOT a gate and has no baseline in tests/gate_baselines.
# Strict invariance is the *end-state* contract: until the lowering stops
# reading the lone-newline-vs-space predicate, it fails wherever the formatter
# deliberately preserves an authored break, which is most files. A near-total
# set makes a useless ratchet.
#
# What it is good for is the one thing nothing else can do: a layout decision
# that reads the unsafe predicate is self-consistent on both spellings, so it is
# invisible to idempotence and to the convergence gate alike. This survey is the
# only mechanical way to enumerate those sites. The counts belong in
# tests/gate_baselines/README.md as a number the residual TODO entries shrink.
#
# Usage: strict_trivia_survey.sh [corpus...]   (default: latex3 latex2e pgf latexindent)
# Env:   BADNESS=/path/to/badness   (default: target/release/badness)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BADNESS="${BADNESS:-${REPO_ROOT}/target/release/badness}"
CORPORA_DIR="${REPO_ROOT}/corpora"

CORPORA=("$@")
if [ "${#CORPORA[@]}" -eq 0 ]; then
  CORPORA=(latex3 latex2e pgf latexindent)
fi

if [ ! -x "${BADNESS}" ]; then
  echo "error: ${BADNESS} not found or not executable — build it first:" >&2
  echo "  cargo build --release" >&2
  exit 2
fi

for corpus in "${CORPORA[@]}"; do
  if [ ! -d "${CORPORA_DIR}/${corpus}" ]; then
    echo "error: ${CORPORA_DIR}/${corpus} not found — fetch the corpora first:" >&2
    echo "  task gate-corpora:fetch" >&2
    exit 2
  fi

  # The report exits 1 whenever failures exist — that is the expected state here.
  report="$(cd "${CORPORA_DIR}/${corpus}" \
    && "${BADNESS}" --no-config debug format --checks trivia-strict --report . || true)"

  checked="$(printf '%s\n' "${report}" | sed -n 's/^- Files checked: //p')"
  failures="$(printf '%s\n' "${report}" | sed -n 's/^- Failures: //p')"
  echo "== ${corpus}: ${failures:-0} of ${checked:-0} files violate strict invariance"

  # Reproducer kinds: `flip@…` means a single gap localized the divergence;
  # a bulk label means every eligible gap had to move before it showed.
  # shellcheck disable=SC2016  # the backticks are literal report syntax, not expansion
  printf '%s\n' "${report}" \
    | sed -n 's/^- Variant: `.*reported: \([^`]*\)`.*/\1/p' \
    | sed 's/^flip@.*/flip@ (localized)/' \
    | LC_ALL=C sort | uniq -c | sed 's/^/   /'
done
