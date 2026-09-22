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

## LaTeX3 completion

Badness completes expl3 functions, variables, and constants inside
`\ExplSyntaxOn` regions, after `\ProvidesExplPackage`, `\ProvidesExplClass`, or
`\ProvidesExplFile`, and inside recognized expl3 macrocode regions in `.dtx`
files. For example, `\tl_` offers `\tl_set:Nn`, and `\l_tmpa_` offers scratch
variables. The built-in catalog ships with Badness and needs no TeX
installation.

Completion also includes literal definitions in the current file and loaded
local packages and classes, including `\cs_new:Npn` functions, variable and
constant declarations, conditional forms, and generated variants. Badness does
not expand macros to discover computed names. It skips incomplete definitions
and definitions stored as token-list data. These names support completion; expl3
definition navigation and argument signature help are not yet provided.

## Renaming source files

Invoke your editor's LSP rename command inside a literal source path, such as
`\input{chapters/introduction}`. Badness renames the file and updates its
references across the discovered workspace. This also works with `\include`,
`\subfile`, `\subfileinclude`, `\import`, `\subimport`, `\loadglsentries`, and
the parent path in `\documentclass[...]{subfiles}`. Literal `\includeonly` lists
are updated along with their targets.

The new name uses the same base directory as the original argument. For example,
renaming `chapters/introduction` to `appendix` moves the file to `appendix.tex`
beside the referring document. Use `chapters/appendix` to keep it in the same
directory. Import commands use their directory argument as the base. Badness
preserves the file extension when you omit it and keeps each reference's
extension spelling when it still resolves correctly. Renaming an imported file
preserves the import directory argument unless that directory itself moves.

Moves stay within the same workspace root and never overwrite existing files or
destination buffers. New parent directories are allowed when the editor creates
them while applying the file operation; Neovim supports this. Without workspace
folders, Badness uses the initiating document's directory as the boundary.

File explorers can also rename source files and folders through
`workspace/willRenameFiles` and `workspace/didRenameFiles`. Your explorer must
send these requests and notifications. When files move, Badness adjusts their
recognized references, including references to assets inside moved folders.
Cursor rename includes its reference edits even when explorer hooks are enabled.
Unsaved editor buffers take precedence over disk contents. Each referring file
uses its own project's declarations and exclusions, including nested projects
and other workspace folders.

Badness declines moves across directories when a moved source contains relative
file arguments. Their resolution can depend on the compilation directory or an
import context, which cannot be inferred from the source's location alone. It
also checks the compilation and import directories inherited through literal
source loads. If a reference requires different edits in those contexts, Badness
declines the rename, even when the referring file stays in place.

File rename requires a client that supports LSP resource-rename operations.
Dynamic paths, braceless inputs, `\graphicspath`, and symlink aliases are not
resolved for rename. Source and destination paths cannot pass through symlinks
inside the workspace, and new names cannot contain quotation marks or TeX
delimiters. Badness also declines names containing spaces when a reference uses
`\usepackage`, `\RequirePackage`, or `\bibliography`, which strip those spaces.
Unresolved references and files excluded from discovery remain unchanged.
Installed TEXMF files and navigation-only `.dtx` fallbacks are never renamed.
Renaming directly from bibliography, graphics, package, or class arguments is
not supported.

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

## Forward and inverse search

Jump between a source line and the matching place in the compiled PDF.

**Badness never typesets, and it never reads a `.synctex.gz`.** Forward search
works out three things — the file your cursor is in, the root document's PDF,
and the line number — and hands them to a viewer you configure. Every
SyncTeX-aware viewer (zathura, Okular, SumatraPDF, Skim) links libsynctex and
does the mapping itself, which is why they all want a file and a line rather
than a coordinate. Inverse search runs in the other direction and is started by
the viewer.

You need a PDF compiled with SyncTeX enabled — `latexmk -pdf -synctex=1`, or
`-synctex=1` passed to `pdflatex`/`lualatex` directly. Badness will not run that
for you; use your existing build setup, or an extension like LaTeX Workshop.

### Configuring the viewer

Which viewer is installed on your machine, and under what name, is a fact about
the machine rather than the project — so these settings come from the editor,
like [TEXMF discovery](#texmf-discovery), and not from `badness.toml`. Where the
*PDF* lives is project data and belongs to the [`[build]`
section](../reference/configuration.md#build) instead.

A `forwardSearch` object:

- `executable` (string): the viewer program. **Spawned directly, not through a
  shell**, so it is a program name and never a command line — putting flags here
  (`"zathura --synctex-forward"`) silently fails to launch. This is the most
  common misconfiguration.
- `args` (array of strings): the viewer's arguments. Required — there is no
  useful default, since every viewer spells forward search differently. Without
  it, forward search reports itself unconfigured.
- `ipcDir` (path, optional): where inverse-search servers advertise themselves.
  An escape hatch for containers and sandboxes; see below.

Each argument may carry:

  | Placeholder | Expands to                       |
  | ----------- | -------------------------------- |
  | `%f`        | the `.tex` file the cursor is in |
  | `%p`        | the **root document's** PDF      |
  | `%l`        | the line number, counting from 1 |
  | `%%f`       | a literal `%f`                   |

An argument wrapped entirely in `"` is passed through with the quotes stripped
and nothing substituted — the escape hatch when a viewer needs a literal `%`.

Recipes, matching texlab's, so an existing configuration ports unchanged:

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

The server handles `textDocument/forwardSearch`, a custom request taking the
standard `{ textDocument, position }` params — the same method name and shape
texlab uses, so a client written for texlab works unchanged. It never fails the
request; it answers with a status:

  | Status | Meaning                                                              |
  | ------ | -------------------------------------------------------------------- |
  | `0`    | the viewer was launched                                              |
  | `1`    | the viewer would not start                                           |
  | `2`    | no PDF on disk, or the buffer has no path — build the document first |
  | `3`    | no viewer configured                                                 |

The capability is advertised as `experimental.textDocumentForwardSearch`.

If forward search opens the wrong PDF, or reports status `2` on a project that
has been built, the root document is probably not being found — see
[`root`](../reference/configuration.md#root) in the `[build]` reference.

### Inverse search

Configure your viewer to run:

```sh
badness inverse-search --input "%f" --line "%l"
```

substituting the viewer's own placeholders. For zathura that is:

```sh
zathura --synctex-editor-command "badness inverse-search --input %{input} --line %{line}"
```

Use `--line0` instead if your viewer counts lines from zero. (`--line1` is
accepted as a synonym for `--line`, so a texlab configuration ports directly.)

The command finds the language server whose workspace contains the file and asks
it to reveal the position, so **an editor must already have that project open**,
and its LSP client must support `window/showDocument`. Servers whose client does
not support it never register, which is why inverse search silently does nothing
in an editor lacking it — the command says so when nothing is listening.

With several editor windows open, the server whose workspace root contains the
file wins; the longest matching root is preferred, so nested projects resolve
deterministically.

Servers advertise themselves in `$BADNESS_IPC_DIR`, else a per-user directory
under your runtime directory (`$XDG_RUNTIME_DIR`), else the temporary directory.
The `forwardSearch.ipcDir` setting overrides all of these — useful when the
viewer and the server see different filesystems, as in a container or a remote
development setup. Keep it short: a Unix socket path cannot exceed about 100
bytes, and badness says so explicitly in its log if yours does. On a system with
no `$XDG_RUNTIME_DIR` and a `/tmp` shared between users, that last fallback is
worth knowing about: the directory is created `0700`, the advertisements `0600`,
and badness ignores any advertisement it does not own, so another user can
neither read nor impersonate one.

One caveat inherent to SyncTeX: it maps the source **as it was compiled**. With
unsaved edits, buffer line numbers and PDF line numbers drift apart until you
rebuild.

## Table refactoring

With the cursor inside a statically understood `tabular`, `tabular*`, or `array`
environment, the **Add column at end** code action appends a centered `c` column
to the preamble and an empty trailing cell to every row. The action is withheld
when the preamble uses unknown column types, a row has an ambiguous width, or
the environment has been redefined, so it never applies a partial table rewrite.

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

### Using only some features

The formatter, linter, and language features share one server but can be turned
off independently, so you can adopt just the parts you want:

- `badness.formatting.enable` — use Badness as a formatter.
- `badness.diagnostics.enable` — show Badness diagnostics (the linter).
- `badness.languageFeatures.enable` — hover, completion, navigation, symbols,
  rename, code actions, and the rest.

All three default to `true`. They are client-side gates, so the server keeps
running and the toggles take effect without a reinstall. For a formatter-only
setup, turn off the other two:

```json
{
  "badness.diagnostics.enable": false,
  "badness.languageFeatures.enable": false
}
```

Turning off `badness.diagnostics.enable` this way suppresses **every**
diagnostic, including the syntax/parse errors that a `badness.toml` `[lint]`
selection [cannot silence](../reference/configuration.md#lint). The
`badness.toml` route stays the right tool when you want to keep parse errors but
mute specific lint rules across every editor and the CLI.

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
