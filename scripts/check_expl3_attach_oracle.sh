#!/usr/bin/env bash
# The expl3 attachment migration oracle (AGENTS.md decision #8, stage 2):
# diff arity-directed grammar attachment against the semantic call-unit model
# over the gate corpora. Fails on any disagreement not recorded in
# crates/badness-parser/tests/expl3_attach_allowlist.toml.
#
# Usage: scripts/check_expl3_attach_oracle.sh [--dump]
#   --dump   also write per-head detail to target/expl3_attach_oracle.txt
set -euo pipefail

cd "$(dirname "$0")/.."

if [ ! -d corpora ]; then
  echo "gate corpora missing; run scripts/fetch_gate_corpora.sh first" >&2
  exit 1
fi

if [ "${1:-}" = "--dump" ]; then
  export EXPL3_ORACLE_DUMP=1
fi

cargo test --release -p badness-parser --test expl3_attach_oracle -- --ignored --nocapture
