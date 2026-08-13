#!/usr/bin/env bash
#
# Fetch the trivia-invariant-layout gate corpora into `corpora/` at the
# repository root, pinned to exact commits so every gate run
# (`badness debug format --checks all|trivia --report .`) is reproducible.
# The directory is gitignored; re-running is a fast no-op when a corpus is
# already checked out at its pin.
#
# The pins are the SHAs the recorded sets in tests/gate_baselines are measured
# against — never bump one without re-recording, or the two-sided ratchet in
# check_gate_baselines.sh compares against a different corpus.
#
# `latexindent` is the odd one out: not package source but latexindent.pl's own
# test suite, ~5.3k small hand-written files of deliberately adversarial LaTeX
# (blank lines in display math, verbatim-argument commands, unmatched braces,
# alignment torture). Its *outputs* are not a target — latexindent is an
# indenter driven by its own YAML config model, so every committed `*-mod1.tex`
# is that tool's answer to a different question. We mine it purely as oracle
# input: the median file is ~200 bytes, so a failure is a near-minimal repro.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CORPORA_DIR="${REPO_ROOT}/corpora"

# name|owner/repo|commit
PINS="
latex3|latex3/latex3|3d1d347d8937863c0786988b14d307a6091ee397
latex2e|latex3/latex2e|3a9fdd88bdc53f16a0c2158aa70d259607de333a
pgf|pgf-tikz/pgf|1c7fc0fdc3ec8a6bdcfd68785c6bbd43ec110178
latexindent|cmhughes/latexindent.pl|748f0f68397793b4646fa48762b0041b889cfcb4
"

mkdir -p "${CORPORA_DIR}"

while IFS='|' read -r name repo sha; do
  [ -z "${name}" ] && continue
  dir="${CORPORA_DIR}/${name}"
  if [ -d "${dir}/.git" ] && [ "$(git -C "${dir}" rev-parse HEAD)" = "${sha}" ]; then
    echo "${name}: already at ${sha}"
    continue
  fi
  echo "${name}: fetching ${repo} @ ${sha}"
  rm -rf "${dir}"
  mkdir -p "${dir}"
  git -C "${dir}" init --quiet
  git -C "${dir}" remote add origin "https://github.com/${repo}.git"
  # A shallow single-commit fetch: reproducible, no history download.
  git -C "${dir}" fetch --quiet --depth 1 origin "${sha}"
  git -C "${dir}" checkout --quiet FETCH_HEAD
  echo "${name}: checked out $(git -C "${dir}" rev-parse HEAD)"
done <<EOF
${PINS}
EOF

echo "Gate corpora ready under ${CORPORA_DIR}"
