# Editor Setup

Badness ships a language server. Start it with:

```sh
badness lsp
```

The server speaks the Language Server Protocol over **stdio**. Point your
editor's LSP client at the `badness` binary with the `lsp` argument and
associate it with LaTeX (`.tex`) files.

## Neovim

With the built-in `vim.lsp` client:

```lua
vim.lsp.config.badness = {
  cmd = { "badness", "lsp" },
  filetypes = { "tex", "latex", "plaintex" },
  root_markers = { ".git" },
}
vim.lsp.enable("badness")
```

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

## Other Editors

Any LSP-capable editor can drive badness: configure a server whose command is
`badness lsp`, communicating over stdio, for LaTeX documents. Consult your
editor's LSP client documentation for the exact configuration shape.

> The language server is young; the set of supported LSP requests will expand.
> Track progress in the [Changelog](../changelog.md).
