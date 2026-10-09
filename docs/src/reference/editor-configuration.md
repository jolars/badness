# Editor Configuration

These settings configure the Badness language server. See [Editor
Setup](../guide/editor-setup.md) for editor-specific configuration examples and
[Language Server](../guide/language-server.md) for its capabilities. Project
settings belong in [badness.toml](configuration.md).

## Supplying settings

Settings can be supplied as `initializationOptions` at startup or through
`workspace/didChangeConfiguration`, either as a bare object or namespaced under
a `badness` key.

**Formatter widths**: `lineWidth` and `indentWidth`. They act as a fallback: a
discovered `badness.toml` always wins outright, and absent one, your editor's
tab size (sent with each formatting request) overrides the indent width.

The language server is also the sole consumer of the `[build]` section of
`badness.toml`, which locates the compile's `.aux` artifacts; see the
[Configuration reference](configuration.md#build).

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
section](configuration.md#build) instead.

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
[`root`](configuration.md#root) in the `[build]` reference.

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
