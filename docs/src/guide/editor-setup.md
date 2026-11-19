# Editor setup

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

See [Language server](language-server.md) for capabilities and feature behavior.

## Neovim

Badness is included in
[nvim-lspconfig](https://github.com/neovim/nvim-lspconfig/blob/master/lsp/badness.lua)
and the [Mason
registry](https://github.com/mason-org/mason-registry/blob/main/packages/badness/package.yaml).
If you use [mason.nvim](https://github.com/mason-org/mason.nvim), install it
with:

```vim
:MasonInstall badness
```

With nvim-lspconfig installed and `badness` on your `PATH`, add this to your
`init.lua` (Neovim 0.11.3+):

```lua
vim.lsp.enable("badness")
```

Without nvim-lspconfig, [install Badness](installation.md) and define the
configuration with the built-in `vim.lsp` client (Neovim 0.11+):

```lua
vim.lsp.config.badness = {
  cmd = { "badness", "lsp" },
  filetypes = { "tex", "latex", "plaintex", "bib" },
  root_markers = { "badness.toml", ".git" },
  init_options = { lineWidth = 80, indentWidth = 2 },
}
vim.lsp.enable("badness")
```

The `init_options` block is optional. Omit it to use the defaults or a
`badness.toml`. See [Supplying settings](#supplying-settings) for shared
settings and their precedence.

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

Shared editor settings go under the server ID. This example sets formatting
widths for projects without a `badness.toml`.

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

See [Supplying settings](#supplying-settings) for width precedence.

For a custom binary, set `binary.path` under `lsp.badness-language-server`. If
you also set `binary.arguments`, use `["lsp"]`: the arguments replace the
default command line.

## Other editors

Any editor with an LSP client can run Badness. Configure it to use `badness lsp`
over stdio for LaTeX documents. Consult your editor's LSP client documentation
for the exact configuration syntax.

<span id="editor-configuration"></span>

## Supplying settings

Supply settings as `initializationOptions` at startup or through
`workspace/didChangeConfiguration`, either as a bare object or under a `badness`
key.

`lineWidth` and `indentWidth` serve as fallbacks. A discovered project
`badness.toml` takes precedence over both, using built-in defaults for widths it
omits. Without a project config, a global or `BADNESS_CONFIG` file overrides
only the widths it explicitly sets, including values inherited through `extend`.
If that file omits `indent-width`, your editor's tab size, sent with each
formatting request, overrides `indentWidth`. Omitted widths otherwise fall back
to these editor settings, then built-in defaults.

The language server also reads the `[build]` section of `badness.toml` to locate
the `.aux` files and PDF produced by compilation. See the [Configuration
reference](../reference/configuration.md#build).

### TEXMF discovery

The language server discovers the installed TeX tree to resolve packages for
document links, package hover, go-to-definition, and completion of installed
package names. The TeX installation's location depends on the machine, so these
settings come from the editor and apply only to language-server features.

The `texmf` object has three optional keys.

- `enabled` is a boolean and defaults to `true`. Set it to `false` to keep
  package resolution local to the document's directory.
- `roots` is an array of paths and defaults to `[]`. It adds TEXMF directories
  ahead of the discovered ones. Use it for an installation that `kpsewhich`
  can't see.
- `useKpsewhich` is a boolean and defaults to `true`. Set it to `false` to
  discover TEXMF roots using default paths only.

```json
{ "texmf": { "enabled": true, "roots": ["/opt/texmf"], "useKpsewhich": true } }
```

## Forward and inverse search

Build your PDF with SyncTeX enabled. Run `latexmk -pdf -synctex=1`, or pass
`-synctex=1` directly to `pdflatex` or `lualatex`. Use your existing build setup
or an extension such as LaTeX Workshop.

### Configuring the viewer

Configure your PDF viewer through your editor's LSP settings. If your build
places the PDF in a custom location, configure that path in
[`[build]`](../reference/configuration.md#build).

The `forwardSearch` object has these keys.

- `executable` is the viewer program name or path. Badness spawns it directly,
  so put flags and arguments in `args`.
- `args` is a required array of strings. Each viewer uses its own arguments for
  forward search.
- `ipcDir` is an optional directory where inverse-search servers advertise
  themselves. See [Inverse search in the language server
  guide](language-server.md#inverse-search).

Each argument may contain these placeholders.

  | Placeholder | Expands to                       |
  | ----------- | -------------------------------- |
  | `%f`        | the `.tex` file the cursor is in |
  | `%p`        | the root document's PDF          |
  | `%l`        | the line number, counting from 1 |
  | `%%f`       | a literal `%f`                   |

To pass an argument through without substitution, wrap the entire argument in
double quotes. Badness strips the enclosing quotes before passing it on.

Badness uses the same viewer settings as Texlab.

  | Viewer     | `executable`     | `args`                                                     |
  | ---------- | ---------------- | ---------------------------------------------------------- |
  | zathura    | `zathura`        | `["--synctex-forward", "%l:1:%f", "%p"]`                   |
  | Okular     | `okular`         | `["--unique", "file:%p#src:%l%f"]`                         |
  | SumatraPDF | `SumatraPDF`     | `["-reuse-instance", "%p", "-forward-search", "%f", "%l"]` |
  | Skim       | `displayline`    | `["%l", "%p", "%f"]`                                       |
  | Evince     | `evince-synctex` | `["-f", "%l", "%p", "\"code -g %f:%l\""]`                  |
  | qpdfview   | `qpdfview`       | `["--unique", "%p#src:%f:%l:1"]`                           |

```json
{
  "forwardSearch": {
    "executable": "zathura",
    "args": ["--synctex-forward", "%l:1:%f", "%p"]
  }
}
```

### Triggering forward search

Invoke your editor's forward-search command from the source line you want to
find in the PDF. For status codes and troubleshooting, see [Triggering forward
search](language-server.md#triggering-forward-search).

### Inverse search

Configure your viewer to run the command below. Substitute the viewer's own
placeholders for the input file and line number.

```sh
badness inverse-search --input "%f" --line "%l"
```

For zathura, use this command.

```sh
zathura --synctex-editor-command "badness inverse-search --input %{input} --line %{line}"
```

Use `--line0` if your viewer counts lines from zero. `--line1` is a synonym for
`--line`, so a Texlab configuration ports directly.

Open the project in an editor whose LSP client supports `window/showDocument`
before jumping back to the source. For workspace selection and IPC setup, see
[Inverse search](language-server.md#inverse-search).
