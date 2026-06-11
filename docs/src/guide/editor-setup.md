# Editor Setup

badness ships a language server. Start it with:

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

There is no dedicated extension yet. Any generic LSP bridge that lets you
register a stdio server command (`badness lsp`) for the `latex` language will
work.

## Other editors

Any LSP-capable editor can drive badness: configure a server whose command is
`badness lsp`, communicating over stdio, for LaTeX documents. Consult your
editor's LSP client documentation for the exact configuration shape.

> The language server is young; the set of supported LSP requests will expand.
> Track progress in the [Changelog](../changelog.md).
