# Editor Setup

Badness ships a language server. Start it with:

```sh
badness lsp
```

The server speaks the Language Server Protocol over **stdio**. Point your
editor's LSP client at the `badness` binary with the `lsp` argument and
associate it with LaTeX (`.tex`) and BibTeX (`.bib`) files.

Settings can be supplied as `initializationOptions` at startup or through
`workspace/didChangeConfiguration`, either as a bare object or namespaced under
a `badness` key.

**Formatter widths**: `lineWidth` and `indentWidth`. They act as a fallback: a
discovered `badness.toml` always wins outright, and absent one, your editor's
tab size (sent with each formatting request) overrides the indent width.

The language server is also the sole consumer of the `[build]` section of
`badness.toml`, which locates the compile's `.aux` artifacts; see the
[Configuration reference](../reference/configuration.md#build).

## TEXMF discovery

How the language server discovers the installed TeX tree for package resolution:
document links, package hover, go-to-definition, and installed-set completion.
Where a TeX installation lives is a fact about the machine, not the project, so
these settings come from the editor rather than `badness.toml`, and they never
affect `badness format` or `badness lint`, whose output stays a pure function of
the input regardless of what is installed.

A `texmf` object with three keys, all optional:

- `enabled` (boolean, default `true`): whether to scan the TEXMF tree at all.
  When `false`, package resolution stays local to the document's directory.
- `roots` (array of paths, default `[]`): extra TEXMF root directories to index
  in addition to (and ahead of) the discovered ones. Useful for a non-standard
  install that `kpsewhich` can't see.
- `useKpsewhich` (boolean, default `true`): whether to shell out to `kpsewhich`
  to discover the TEXMF tree roots. When `false`, discovery falls back to
  default-path heuristics only.

```json
{ "texmf": { "enabled": true, "roots": ["/opt/texmf"], "useKpsewhich": true } }
```

## Neovim

With the built-in `vim.lsp` client (Neovim 0.11+):

```lua
vim.lsp.config.badness = {
  cmd = { "badness", "lsp" },
  filetypes = { "tex", "latex", "plaintex", "bib" },
  root_markers = { "badness.toml", ".git" },
  init_options = { lineWidth = 80, indentWidth = 2 },
}
vim.lsp.enable("badness")
```

The `init_options` block is optional; omit it to use the defaults or a
`badness.toml`.

## VS Code

Install the [Badness
extension](https://marketplace.visualstudio.com/items?itemName=jolars.badness)
from the VS Code Marketplace or the [Open VSX
extension](https://open-vsx.org/extension/jolars/badness). It bundles a
platform-specific `badness` binary and starts the language server automatically
when you open a `.tex` file, so no separate CLI install is required.

The extension is configured through `badness.*` settings. By default it uses the
bundled binary (`badness.executableStrategy: "bundled"`); set the strategy to
`environment` to use a `badness` on your `PATH`, or `path` with
`badness.executablePath` to point at a specific binary. See the extension's
README for the full list of settings.

### Using with LaTeX Workshop

Badness works alongside [LaTeX
Workshop](https://marketplace.visualstudio.com/items?itemName=James-Yu.latex-workshop)
rather than replacing it. The two divide cleanly: LaTeX Workshop handles
building, PDF preview, and SyncTeX, while badness handles formatting, linting,
and navigation. Run both, and let each own its half.

**Formatting.** The badness extension registers itself as the default formatter
for LaTeX files. LaTeX Workshop's own formatter integration is disabled by
default (`latex-workshop.formatting.latex` is `"none"`); leave it that way so
there is a single formatting authority. For BibTeX files, LaTeX Workshop ships a
built-in formatter, so pick badness explicitly:

```json
{
  "[bibtex]": {
    "editor.defaultFormatter": "jolars.badness"
  }
}
```

**Linting.** LaTeX Workshop's ChkTeX and lacheck integrations are disabled by
default (`latex-workshop.linting.chktex.enabled` and
`latex-workshop.linting.lacheck.enabled`). Leave them off; enabling them
alongside badness produces overlapping diagnostics for many common issues.

**Completion.** Both extensions contribute completion items, so you may see
duplicate suggestions for commands, environments, or citations. This is
harmless, but if it bothers you, the `latex-workshop.intellisense.*` settings
let you turn off the overlapping parts on the LaTeX Workshop side.

## Other Editors

Any LSP-capable editor can run badness: configure a server whose command is
`badness lsp`, communicating over stdio, for LaTeX documents. Consult your
editor's LSP client documentation for the exact configuration shape.
