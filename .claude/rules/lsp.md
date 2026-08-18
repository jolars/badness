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

## Concurrency/processes

- Never block read-pool threads waiting for spawned viewer processes.
- Spawn directly (no shell splitting fallback for misconfigured executable
  strings).

## Completion and paths

- Do not prefix-filter citations server-side; rely on client matching with
  `filterText`, keep `isIncomplete: true`.
- Convert paths/URIs only through `uri_to_fs_path` and `path_to_uri`, with
  Windows drive handling preserved.
