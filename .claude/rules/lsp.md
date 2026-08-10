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
- Delegate TEXMF root discovery to `kpsewhich -var-value`; reimplementing
  kpathsea is out of scope and MiKTeX doesn't use it.
- Aux freshness is an mtime+length cache, so a recompile is picked up without a
  watcher.

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
