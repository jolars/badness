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
- **Non-goal:** any runtime query of the TeX distribution feeding the
  *formatter*.
- **Guarded by tests — keep them green:** the TEXMF index is never wired into
  `scope_signatures` or `DiskPackageSource`, and the formatter never reads an
  `.aux` file.

## Environment awareness

- **Parse `.aux` with the dedicated line scanner, never the LaTeX parser.** Aux
  files are written under `\makeatletter`, so `\@input`/`\@writefile` mis-lex.
- **Machine state belongs in editor settings, not `badness.toml`.** Where a TeX
  installation lives is not project data, so the `texmf` settings arrive as
  `initializationOptions`/`didChangeConfiguration`.
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

## Paths

**Decode LSP URIs only through `uri_to_fs_path`/`path_to_uri`** — they strip the
`/` before a Windows drive letter and keep the Unix root. Keep
`uri_to_fs_path_handles_unix_and_windows` green; tests and snapshots must not
assume `/` vs `\`.
