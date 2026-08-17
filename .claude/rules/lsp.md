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

## The hermetic boundary

The formatter's output is a function of the input plus shipped data. The LSP
gets more latitude because navigation is inherently about the local environment.

- **Sanctioned:** a read-only index or shipped metadata feeding LSP navigation.
- **Sanctioned:** launching a user-configured PDF viewer from
  `textDocument/forwardSearch` — the one *outbound* side effect. It is an
  explicit user action, never speculative, and feeds nothing back into the
  formatter or linter.
- **Non-goal:** any runtime query of the TeX distribution feeding the
  *formatter*.
- **Non-goal:** typesetting. Never run an engine, and never parse a
  `.synctex.gz` — the mapping is delegated to the viewer, which links libsynctex
  itself. The seam for revisiting that is `SearchTarget` → `ForwardSearchStatus`
  in `lsp::forward_search`; it would change no LSP surface.
- **Guarded by tests — keep them green:** the TEXMF index is never wired into
  `scope_signatures` or `DiskPackageSource`, and the formatter never reads an
  `.aux` file.

## Environment awareness

- **Parse `.aux` with the dedicated line scanner, never the LaTeX parser.** Aux
  files are written under `\makeatletter`, so `\@input`/`\@writefile` mis-lex.
- **Machine state belongs in editor settings, not `badness.toml`.** Where a TeX
  installation lives is not project data, so the `texmf` settings arrive as
  `initializationOptions`/`didChangeConfiguration`. Same rule, same reason, for
  `forwardSearch` — which viewer is installed is a fact about the machine, while
  where the PDF lands is project data and belongs to `[build]`.
  Read these off `state.editor_settings` directly: `ResolvedSettings` is built
  from the `Config` whenever a `badness.toml` exists, so a field populated only
  in `from_editor` is silently blank for every workspace that has one.
- **The declarations are republished by the *dispatcher*, not by each handler.**
  The salsa input carrying them is a project-wide singleton, so any job that
  reads a tree must find the cell holding its own document's block — and a
  per-handler call is a rule the next handler forgets, leaving that one feature
  parsing under whichever document was analyzed last. `publish_declarations_for_request`
  runs once in the request loop and reads `textDocument.uri` off the raw params,
  so it covers handlers not yet written; the notification sites that publish
  ahead of an `Edit` job go through `GlobalState::analysis_settings`, which wants
  the settings anyway. **Do not move this back into the handlers**, and do not
  narrow it with a method allowlist — a request whose job reads no tree
  (`forwardSearch`) pays at most one redundant write, which is cheaper than a
  list that can go stale. Every job rides the same FIFO channel as the write that
  precedes it, so ordering needs no handshake.
- Delegate TEXMF root discovery to `kpsewhich -var-value`; reimplementing
  kpathsea is out of scope and MiKTeX doesn't use it.
- Aux freshness is an mtime+length cache, so a recompile is picked up without a
  watcher.

## The live buffer

**A document buffer is an `Arc<TextBuffer>` — never a `String` — from the main
loop through the job to the read pool.** The main loop is on the keystroke path,
so capturing a buffer for a job must be a refcount bump; `TextBuffer` carries
the negotiated encoding and a `OnceLock<LineIndex>`, so the index is built once
per document version rather than once per request (1.8 ms over 1 MB).

- **A handler that indexes the cursor buffer takes `&TextBuffer` and calls
  `line_index()`.** Never `LineIndex::with_encoding` over it — that is the
  rebuild the type exists to remove. Handlers walking *other* project members
  still build their own; those texts come off the snapshot and have no buffer.
- **A `&TextBuffer` handler must not also take an `enc`**, since the buffer
  knows its own encoding and two sources can disagree. Keep `enc` only where it
  feeds a cross-file index.
- **The buffer is immutable**: an edit yields a new one (`with_replacement`), so
  a job that captured the previous version keeps a consistent text *and* index
  with no lock — and the pointer identity stays meaningful.
- **Staleness is `text_is_current`, never a `file_text(file) == text` compare.**
  It pointer-tests first and falls back to the content compare; both halves are
  load-bearing (a disk re-read is a fresh allocation that may still be equal).
  Same rule for `upsert_file`'s skip-the-write guard — salsa's setter does no
  equality check of its own.
- The line table is **rebuilt, not patched**, across an edit: a keystroke still
  pays a full reparse, which dwarfs the scan. Revisit with incremental reparse,
  in `TextBuffer`.

## Transport

`lsp-server` + `lsp-types`, not tower-lsp: salsa cancellation is a synchronous
unwind (`salsa::Cancelled`) that composes with a sync main loop plus threadpool
and fights tower-lsp's async `&self` model.

## Completion

- **Do not prefix-filter citations server-side.** Return the whole namespace
  (deduped by folded key, first definer wins) and let the client match on
  `filterText` (key, title, authors — key first, capped at 128 chars). This is
  LSP-standard, so it needs no client-specific code; a server-side key filter is
  the one thing that would break title-word matching.
- Keep `isIncomplete: true` so the client re-queries as the prefix narrows.

## Processes

**Never block a pool thread on a spawned child.** The read pool can be a single
thread wide, so waiting on a viewer that lives for hours stalls diagnostics and
formatting behind it. Spawn, reap on a short detached thread, and report that the
*launch* succeeded — not that the user eventually closed the window. (texlab uses
a blocking `.status()` here; that is the thing not to copy.)

Spawn directly, never through a shell: an `executable` carrying flags is a
misconfiguration to document, not to paper over with word splitting.

## Paths

**Decode LSP URIs only through `uri_to_fs_path`/`path_to_uri`** — they strip the
`/` before a Windows drive letter, keep the Unix root, and spell separators the
platform's way. Keep `uri_to_fs_path_handles_unix_and_windows` green; tests and
snapshots must not assume `/` vs `\`.

`Path` compares and hashes by component, so a URI-spelled path is already a
usable key — the separator normalization is for the places a decoded path is
rendered back to *text*. Forward search is the one that bites: `%f` comes off the
document URI and `%p` off a root discovered on disk, so a viewer used to receive
one spelling of each.
