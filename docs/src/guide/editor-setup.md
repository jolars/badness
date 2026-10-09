# Editor Setup

Badness ships a language server. Start it with:

```sh
badness lsp
```

The server uses the Language Server Protocol (LSP) over standard input and
output (**stdio**). Point your editor's LSP client at the `badness` binary with
the `lsp` argument and associate it with LaTeX (`.tex`) and BibTeX (`.bib`)
files.

<span id="command-completion"></span> <span id="latex3-completion"></span>
<span id="renaming-source-files"></span>
<span id="inlining-an-input-file"></span> <span id="table-refactoring"></span>

See [Language Server](language-server.md) for capabilities and feature behavior.

<span id="texmf-discovery"></span> <span id="forward-and-inverse-search"></span>
<span id="configuring-the-viewer"></span>
<span id="triggering-forward-search"></span> <span id="inverse-search"></span>

See the [Editor Configuration reference](../reference/editor-configuration.md)
for shared settings, their precedence, TEXMF discovery, and PDF search.

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

See the [Editor Configuration
reference](../reference/editor-configuration.md#supplying-settings) for shared
settings and their precedence. The `init_options` block is optional; omit it to
use the defaults or a `badness.toml`.

## VS Code

Install the [Badness
extension](https://marketplace.visualstudio.com/items?itemName=jolars.badness)
from the VS Code Marketplace or the [Open VSX
extension](https://open-vsx.org/extension/jolars/badness). It bundles a
platform-specific `badness` binary and starts the language server automatically
when you open a `.tex` file, so no separate CLI install is required.

Configure the extension through `badness.*` settings. By default, it uses the
bundled binary (`badness.executableStrategy: "bundled"`). Set the strategy to
`environment` to use `badness` from your `PATH`, or to `path` with
`badness.executablePath` to select a specific binary. See the extension's README
for the full list of settings.

### Using only some features

The formatter, linter, and language features share one server but can be turned
off independently, so you can adopt just the parts you want:

- `badness.formatting.enable`—use Badness as a formatter.
- `badness.diagnostics.enable`—show Badness diagnostics (the linter).
- `badness.languageFeatures.enable`—hover, completion, navigation, symbols,
  rename, code actions, and the rest.

All three default to `true`. The extension controls these features on the client
side, so the server keeps running and the changes take effect without a
reinstall. For a formatter-only setup, turn off the other two:

```json
{
  "badness.diagnostics.enable": false,
  "badness.languageFeatures.enable": false
}
```

Turning off `badness.diagnostics.enable` suppresses **every** diagnostic,
including the parse errors that a `[lint]` selection in `badness.toml` [cannot
silence](../reference/configuration.md#lint). Use `badness.toml` when you want
to keep parse errors but disable specific lint rules across every editor and the CLI.

### Using with LaTeX Workshop

Badness works alongside [LaTeX
Workshop](https://marketplace.visualstudio.com/items?itemName=James-Yu.latex-workshop).
LaTeX Workshop handles building, PDF preview, and SyncTeX, while Badness handles
formatting, linting, and navigation.

**Formatting.** The Badness extension registers itself as the default formatter
for LaTeX files. LaTeX Workshop's formatter integration is disabled by default
(`latex-workshop.formatting.latex` is `"none"`). Leave it disabled so only one
extension formats your LaTeX files. LaTeX Workshop ships a built-in BibTeX
formatter, so select Badness explicitly for BibTeX files:

```json
{
  "[bibtex]": {
    "editor.defaultFormatter": "jolars.badness"
  }
}
```

**Linting.** LaTeX Workshop's ChkTeX and lacheck integrations are disabled by
default (`latex-workshop.linting.chktex.enabled` and
`latex-workshop.linting.lacheck.enabled`). Leave them off to avoid overlapping
diagnostics from those tools and Badness.

**Completion.** Both extensions contribute completion items, so you may see
duplicate suggestions for commands, environments, or citations. This is
harmless, but if it bothers you, the `latex-workshop.intellisense.*` settings
let you turn off the overlapping parts on the LaTeX Workshop side.

## Zed

Install **LaTeX** and **Badness** from Zed's extensions view (`zed: extensions`
in the command palette). The LaTeX extension supplies the LaTeX and BibTeX
languages; Badness supplies the language server for both.

Badness uses a `badness` binary on your `PATH` when available and otherwise
downloads a release for your platform. On NixOS, install the Nixpkgs package so
the extension can use the native binary.

To use Badness for both languages and enable formatting on save, add this to
Zed's `settings.json`:

```json
{
  "languages": {
    "LaTeX": {
      "language_servers": ["badness-language-server"],
      "formatter": "language_server",
      "format_on_save": "on"
    },
    "BibTeX": {
      "language_servers": ["badness-language-server"],
      "formatter": "language_server",
      "format_on_save": "on"
    }
  }
}
```

To retain Texlab alongside Badness for LaTeX files, list `"texlab"` after
`"badness-language-server"` in the `LaTeX` settings.

Shared [editor
settings](../reference/editor-configuration.md#supplying-settings) go under the
server ID. For example, these formatting settings apply when the project has no
`badness.toml`:

```json
{
  "lsp": {
    "badness-language-server": {
      "settings": {
        "lineWidth": 100,
        "indentWidth": 2
      }
    }
  }
}
```

For a custom binary, set `binary.path` under `lsp.badness-language-server`. If
you also set `binary.arguments`, use `["lsp"]`: the arguments replace the
default command line.

## Other Editors

Any editor with an LSP client can run Badness. Configure it to use `badness lsp`
over stdio for LaTeX documents. Consult your editor's LSP client documentation
for the exact configuration syntax.
