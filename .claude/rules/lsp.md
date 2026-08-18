---
paths:
  - "src/lsp/**/*.rs"
  - "src/lsp.rs"
  - "src/project/**/*.rs"
  - "src/project.rs"
  - "src/completion.rs"
  - "src/text/**/*.rs"
---

# Language server rules

Narrative overview: `docs/src/development/architecture.md` § *The language
server*.

## Boundary and side effects

- LSP may use local environment data for navigation; formatter/linter remain
  hermetic.
- Forward search launching a user-configured PDF viewer is the sanctioned
  outbound side effect.
- Do not run TeX engines from LSP, and do not parse `.synctex.gz`.
- Keep TEXMF index disconnected from formatter signature resolution.

## Configuration ownership

- Machine-specific settings (TeX install, viewer path) belong to editor settings
  (`initializationOptions` / `didChangeConfiguration`), not `badness.toml`.
- Project-specific settings (build outputs, project config) stay in project
  config.
- Keep declaration publishing centralized in dispatcher/request flow, not ad hoc
  per-handler logic.

## Data sources and parsing

- Parse `.aux` with the dedicated line scanner, not the LaTeX parser.
- Discover TEXMF roots via `kpsewhich -var-value` rather than reimplementing
  kpathsea.
- Aux freshness is cache-based (mtime + length) and should remain lightweight.

## Live-buffer contract

- Carry document text as `Arc<TextBuffer>` through main loop and jobs.
- Use `TextBuffer::line_index()` for current-buffer indexing; avoid rebuilding
  per request.
- Avoid dual encoding sources in a handler (`&TextBuffer` + separate `enc`).
- Staleness checks must use `text_is_current` semantics (pointer-aware, then
  content fallback).
- Patch an initialized line table across edits with `LineTable::patch`; keep an
  uninitialized table lazy. Text and table must remain structurally paired in the
  same `TextBuffer`, and the patch must handle edits that split/join CRLF.
- `LineIndex::with_table` may only pair text with the table derived from those
  exact bytes; a mismatched table returns wrong positions without necessarily
  panicking.

## Feeding the reparse

**Every `upsert_file` call site pairs with a `reparse_stage_edits`** — the chain
`apply_content_changes` returned where the edits are known, `None` where the text
arrived by a route carrying none (`didOpen`, the re-lint sweep, sibling seeding, a
watched-file re-read). No exceptions: an exceptionless rule survives the next call
site, and the failure is silent, since a missing chain costs a full parse and
nothing else (`reparse_edits` rejects any chain that does not land on exactly the
text being asked about).

- **Stage *after* the upsert, never before.** Its `&mut db` is what proves no
  analyze is reading. A chain staged ahead of the write can be peeked by an
  in-flight `parsed_document`, which fails to verify it, full-parses, and then
  *drains* it — losing the edit for good.
- **Stage even when `upsert_file` skipped its write.** The chain is anchored at
  the reparse *base*, not at the db text, so a buffer that round-trips back to
  what salsa holds still took a transform to get there.
- **The offsets are the clamped ones the splice used**, never the raw client
  positions: the chain describes the transform the buffer took, not the one the
  client asked for.
- **Coalescing stays on the analyze side of the write.** `Worker::run` handles
  every `WorkerJob`, so N keystrokes are N upsert+stage pairs in order; only
  `AnalyzeRequest` coalesces, and it carries no text. A job kind that batched
  writes would have to carry the superseded chain forward instead.

## Concurrency/processes

- Never block read-pool threads waiting for spawned viewer processes.
- Spawn directly (no shell splitting fallback for misconfigured executable
  strings).

## Completion and paths

- Do not prefix-filter citations server-side; rely on client matching with
  `filterText`, keep `isIncomplete: true`.
- Convert paths/URIs only through `uri_to_fs_path` and `path_to_uri`, with
  Windows drive handling preserved.
