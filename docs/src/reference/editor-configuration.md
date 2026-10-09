# Editor Configuration

These settings configure the Badness language server. See [Editor
Setup](../guide/editor-setup.md) for editor-specific configuration examples and
[Language Server](../guide/language-server.md) for its capabilities. Project
settings belong in [badness.toml](configuration.md).

## Supplying settings

Supply settings as `initializationOptions` at startup or through
`workspace/didChangeConfiguration`, either as a bare object or under a `badness`
key.

**Formatter widths**: `lineWidth` and `indentWidth` serve as fallbacks. A
discovered `badness.toml` takes precedence over both. Without one, your editor's
tab size, sent with each formatting request, overrides the indent width.

The language server is also the sole consumer of the `[build]` section of
`badness.toml`, which locates the `.aux` files produced by compilation; see the
[Configuration reference](configuration.md#build).

## TEXMF discovery

The language server discovers the installed TeX tree to resolve packages for
document links, package hover, go-to-definition, and completion of installed
package names. The TeX installation's location depends on the machine, so these
settings come from the editor. They never affect `badness format` or
`badness lint`, whose output depends only on the input, regardless of what is
installed.

The `texmf` object has three optional keys:

- `enabled` (boolean, default `true`): whether to scan the TEXMF tree at all.
  When `false`, package resolution stays local to the document's directory.
- `roots` (array of paths, default `[]`): extra TEXMF root directories to index
  in addition to (and ahead of) the discovered ones. Useful for a nonstandard
  installation that `kpsewhich` can't see.
- `useKpsewhich` (boolean, default `true`): whether to run `kpsewhich` to
  discover the TEXMF tree roots. When `false`, discovery falls back to
  default-path heuristics only.

```json
{ "texmf": { "enabled": true, "roots": ["/opt/texmf"], "useKpsewhich": true } }
```

## Forward and inverse search

Jump between a source line and the matching place in the compiled PDF.

**Badness never typesets, and it never reads a `.synctex.gz`.** Forward search
identifies the file your cursor is in, the root document's PDF, and the line
number, then passes them to your configured viewer. Every SyncTeX-aware viewer
(zathura, Okular, SumatraPDF, Skim) links libsynctex and does the mapping
itself, which is why they all want a file and a line rather than a coordinate.
Inverse search runs in the other direction and is started by the viewer.

You need a PDF compiled with SyncTeX enabled. Run `latexmk -pdf -synctex=1`, or
pass `-synctex=1` directly to `pdflatex` or `lualatex`. Use your existing build
setup or an extension such as LaTeX Workshop; Badness does not compile the
document.

### Configuring the viewer

The viewer program and its name depend on the machine, so configure them through
the editor, as with [TEXMF discovery](#texmf-discovery). Where the *PDF* lives
is project data and belongs to the [`[build]` section](configuration.md#build)
instead.

The `forwardSearch` object has these keys:

- `executable` (string): the viewer program. **Spawned directly, not through a
  shell**, so it is a program name and never a command line—putting flags here
  (`"zathura --synctex-forward"`) silently fails to launch. This is the most
  common misconfiguration.
- `args` (array of strings): the viewer's arguments. Required—there is no useful
  default, since every viewer spells forward search differently. Without it,
  forward search reports itself unconfigured.
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
and nothing substituted—the escape hatch when a viewer needs a literal `%`.

These settings match texlab's, so you can reuse an existing configuration
unchanged:

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

The server handles `textDocument/forwardSearch`, a custom request with the
standard `{ textDocument, position }` parameters. It uses the same method name
and parameters as texlab, so a client written for texlab works unchanged. The
server always answers the request with a status:

  | Status | Meaning                                                            |
  | ------ | ------------------------------------------------------------------ |
  | `0`    | the viewer was launched                                            |
  | `1`    | the viewer would not start                                         |
  | `2`    | no PDF on disk, or the buffer has no path—build the document first |
  | `3`    | no viewer configured                                               |

The capability is advertised as `experimental.textDocumentForwardSearch`.

If forward search opens the wrong PDF, or reports status `2` on a project that
has been built, the server probably cannot find the root document. See
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
and its LSP client must support `window/showDocument`. Servers register only
when their client supports `window/showDocument`. Inverse search cannot reveal a
position without that support, and the command reports when no server is
listening.

With several editor windows open, the server whose workspace root contains the
file wins; the longest matching root is preferred, so nested projects resolve
deterministically.

Servers advertise themselves in `$BADNESS_IPC_DIR`, else a per-user directory
under your runtime directory (`$XDG_RUNTIME_DIR`), else the temporary directory.
The `forwardSearch.ipcDir` setting overrides these locations, which is useful
when the viewer and server see different filesystems, as in a container or
remote development setup. Keep it short: a Unix socket path cannot exceed about
100 bytes, and Badness logs an error if yours does. On a system with no
`$XDG_RUNTIME_DIR` and a `/tmp` shared between users, that last fallback is
worth knowing about: the directory is created `0700`, the advertisements `0600`,
and Badness ignores any advertisement it does not own, so another user can
neither read nor impersonate one.

SyncTeX maps the source **as it was compiled**. With unsaved edits, buffer line
numbers and PDF line numbers drift apart until you rebuild.
