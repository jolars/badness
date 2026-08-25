#!/usr/bin/env bash
# Scan one acquired project and append debug-format failures to a shared result set.

set -euo pipefail

if [ "$#" -ne 7 ]; then
  echo "usage: $0 RESULTS_DIR RESULT_PREFIX SOURCE REVISION PROJECT_DIR FILE_MODE CONFIG_MODE" >&2
  exit 2
fi

RESULTS_DIR="$1"
RESULT_PREFIX="$2"
SOURCE="$3"
SOURCE_REVISION="$4"
PROJECT_DIR="$5"
FILE_MODE="$6"
CONFIG_MODE="$7"

: "${BADNESS_BIN:?BADNESS_BIN must name the badness executable}"
: "${BADNESS_SHA:?BADNESS_SHA must be set}"
: "${BADNESS_VERSION:?BADNESS_VERSION must be set}"

case "$FILE_MODE" in
  tracked | all) ;;
  *) echo "error: FILE_MODE must be 'tracked' or 'all'" >&2; exit 2 ;;
esac
case "$CONFIG_MODE" in
  project | none) ;;
  *) echo "error: CONFIG_MODE must be 'project' or 'none'" >&2; exit 2 ;;
esac
if [ ! -d "$PROJECT_DIR" ]; then
  echo "error: project directory does not exist: $PROJECT_DIR" >&2
  exit 2
fi

LOGS_DIR="$RESULTS_DIR/logs"
FAILURES_TSV="$RESULTS_DIR/failures.tsv"
SKIPPED_TSV="$RESULTS_DIR/skipped.tsv"
mkdir -p "$LOGS_DIR"
if [ ! -f "$FAILURES_TSV" ]; then
  printf 'source\tfailure_type\tfile\tlog_path\treport_path\tsource_revision\tbadness_sha\tbadness_version\tidempotency_input_path\tidempotency_once_path\tidempotency_twice_path\n' > "$FAILURES_TSV"
fi
if [ ! -f "$SKIPPED_TSV" ]; then
  printf 'source\tfile\treason\n' > "$SKIPPED_TSV"
fi

ALLOWLIST_ENTRIES="$(printf '%s\n' "${ALLOWLIST:-}" | sed 's/[[:space:]]*$//' | grep -Ev '^[[:space:]]*(#|$)' || true)"

read_counter() {
  local path="$1"
  if [ -f "$path" ]; then
    cat "$path"
  else
    echo 0
  fi
}

increment_counter() {
  local name="$1"
  local path="$RESULTS_DIR/$name"
  local value
  value="$(read_counter "$path")"
  echo "$((value + 1))" > "$path"
}

is_allowlisted() {
  grep -Fxq "$1|$2|$3" <<< "$ALLOWLIST_ENTRIES"
}

add_failure_type() {
  local candidate="$1"
  if ! grep -Fxq "$candidate" <<< "$failure_types"; then
    if [ -z "$failure_types" ]; then
      failure_types="$candidate"
    else
      failure_types+=$'\n'"$candidate"
    fi
  fi
}

scan_file() {
  local rel_file="$1"
  case "$(basename "$rel_file")" in
    .*)
      printf '%s\t%s\tdotfile-basename\n' "$SOURCE" "$rel_file" >> "$SKIPPED_TSV"
      increment_counter skipped_count
      return
      ;;
  esac
  if ! iconv -f UTF-8 -t UTF-8 "$PROJECT_DIR/$rel_file" >/dev/null 2>&1; then
    printf '%s\t%s\tnon-utf8\n' "$SOURCE" "$rel_file" >> "$SKIPPED_TSV"
    increment_counter skipped_count
    return
  fi

  increment_counter scanned_count
  local file_key log_path report_path pass_dir safe_rel_file status retry_status matched
  local idempotency_input_rel idempotency_once_rel idempotency_twice_rel failure_type
  local -a config_args
  file_key="$(printf '%s' "${SOURCE}:${rel_file}" | sha256sum | awk '{print $1}')"
  log_path="$LOGS_DIR/$file_key.log"
  report_path="$LOGS_DIR/$file_key.report.md"
  pass_dir="$LOGS_DIR/$file_key.passes"

  config_args=()
  if [ "$CONFIG_MODE" = none ]; then
    config_args=(--no-config)
  fi
  status=0
  (cd "$PROJECT_DIR" && timeout 120 "$BADNESS_BIN" debug format --checks all "${config_args[@]}" --dump-dir "$pass_dir" "$rel_file") > "$log_path" 2>&1 || status=$?
  if [ "$status" -eq 0 ]; then
    return
  fi

  if [ "$CONFIG_MODE" = project ] && grep -Fq 'badness.toml' "$log_path"; then
    config_args=(--no-config)
    retry_status=0
    (cd "$PROJECT_DIR" && timeout 120 "$BADNESS_BIN" debug format --checks all "${config_args[@]}" --dump-dir "$pass_dir" "$rel_file") > "$log_path" 2>&1 || retry_status=$?
    if [ "$retry_status" -eq 0 ]; then
      echo "notice: $SOURCE has an invalid badness config; $rel_file passed with --no-config"
      return
    fi
    status=$retry_status
  fi

  (cd "$PROJECT_DIR" && timeout 120 "$BADNESS_BIN" debug format --checks all "${config_args[@]}" --report "$rel_file") > "$report_path" 2>&1 || true
  matched=0
  failure_types=""
  idempotency_input_rel=""
  idempotency_once_rel=""
  idempotency_twice_rel=""

  safe_rel_file="$(printf '%s' "$rel_file" | sed 's/[^[:alnum:]._-]/_/g')"
  if [ -f "$pass_dir/$safe_rel_file.idempotency.input.txt" ]; then
    idempotency_input_rel="$RESULT_PREFIX/logs/$file_key.passes/$safe_rel_file.idempotency.input.txt"
  fi
  if [ -f "$pass_dir/$safe_rel_file.idempotency.once.txt" ]; then
    idempotency_once_rel="$RESULT_PREFIX/logs/$file_key.passes/$safe_rel_file.idempotency.once.txt"
  fi
  if [ -f "$pass_dir/$safe_rel_file.idempotency.twice.txt" ]; then
    idempotency_twice_rel="$RESULT_PREFIX/logs/$file_key.passes/$safe_rel_file.idempotency.twice.txt"
  fi

  if [ "$status" -eq 124 ]; then add_failure_type timeout; fi
  if grep -Eiq 'idempot' "$log_path" "$report_path" 2>/dev/null; then add_failure_type idempotency; fi
  if grep -Eiq 'lossless' "$log_path" "$report_path" 2>/dev/null; then add_failure_type losslessness; fi
  if grep -Eiq 'format-error' "$log_path" "$report_path" 2>/dev/null; then add_failure_type format-error; fi
  if grep -Eiq 'content-change' "$log_path" "$report_path" 2>/dev/null; then add_failure_type content-change; fi
  if grep -Eiq 'comment-change' "$log_path" "$report_path" 2>/dev/null; then add_failure_type comment-change; fi
  if [ -f "$report_path" ]; then
    while IFS= read -r parsed_type; do
      [ -n "$parsed_type" ] && add_failure_type "$parsed_type"
    done < <(grep -Eo '\((idempotency|losslessness|format-error|content-change|comment-change)\)' "$report_path" | tr -d '()' | sort -u)
  fi

  if [ -n "$failure_types" ]; then
    matched=1
    while IFS= read -r failure_type; do
      [ -z "$failure_type" ] && continue
      if is_allowlisted "$SOURCE" "$rel_file" "$failure_type"; then
        echo "notice: allowlisted $failure_type for $SOURCE:$rel_file (record-only)"
        increment_counter suppressed_count
        continue
      fi
      printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$SOURCE" "$failure_type" "$rel_file" \
        "$RESULT_PREFIX/logs/$file_key.log" \
        "$RESULT_PREFIX/logs/$file_key.report.md" \
        "$SOURCE_REVISION" "$BADNESS_SHA" "$BADNESS_VERSION" \
        "$idempotency_input_rel" "$idempotency_once_rel" "$idempotency_twice_rel" \
        >> "$FAILURES_TSV"
    done <<< "$failure_types"
  fi

  if [ "$matched" -eq 0 ]; then
    printf '%s\tunknown\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
      "$SOURCE" "$rel_file" \
      "$RESULT_PREFIX/logs/$file_key.log" \
      "$RESULT_PREFIX/logs/$file_key.report.md" \
      "$SOURCE_REVISION" "$BADNESS_SHA" "$BADNESS_VERSION" \
      "$idempotency_input_rel" "$idempotency_once_rel" "$idempotency_twice_rel" \
      >> "$FAILURES_TSV"
  fi
}

if [ "$FILE_MODE" = tracked ]; then
  while IFS= read -r -d '' rel_file; do
    scan_file "$rel_file"
  done < <(git -C "$PROJECT_DIR" ls-files -z -- '*.tex' '*.sty' '*.cls' '*.dtx' '*.ins' '*.bib')
else
  while IFS= read -r -d '' rel_file; do
    scan_file "${rel_file#./}"
  done < <(
    cd "$PROJECT_DIR"
    find . -type f \( -name '*.tex' -o -name '*.sty' -o -name '*.cls' -o -name '*.dtx' -o -name '*.ins' -o -name '*.bib' \) -print0 | sort -z
  )
fi
