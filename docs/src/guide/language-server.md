# Language server

Badness brings formatting, linting, and language features to LaTeX and BibTeX
editors through the Language Server Protocol. See [Editor
setup](editor-setup.md) to connect and configure your editor.

## Command completion

Command suggestions include short signatures, such as `\section[]{}` and
`\vspace{}`, before you select an item. Clients that support completion label
details can display the argument suffix beside the command name. Other clients
receive the full signature in the completion item's `detail` field. Full
documentation loads when the client resolves the selected item.

Signatures use the document's definitions, loaded local packages, and Badness's
built-in data. They show the known argument slots delimited by braces or
brackets. They do not describe every TeX argument protocol. The signature
display does not insert arguments.

Known math and text symbols and logos, such as `\omega`, `\hbar`, `\copyright`,
and `\LaTeX`, have the completion kind `Constant`. Known argument-free control,
spacing, and declaration commands, such as `\newpage`, `\par`, `\quad`, and
`\bfseries`, have kind `Keyword`. Argument-taking commands such as `\vspace`
retain `Function`. Editors can use these distinctions for icons and automatic
brackets; for example, blink.cmp can insert braces after `\vspace` while leaving
`\omega` and `\newpage` bare. Recognized definitions in the document or loaded
local packages override the built-in classification, as do explicit project
declarations. Commands without a curated classification keep their existing
completion kinds; an empty signature alone does not establish zero arguments.

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
Renaming from the cursor includes edits to references even when explorer hooks
are enabled. Unsaved editor buffers take precedence over disk contents. Each
referring file uses its own project's declarations and exclusions, including
nested projects and other workspace folders.

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

## Inlining an input file

With the cursor on `\input{filename}`, **Inline input file** replaces the
statement with the referenced file's contents. This `refactor.inline` action
resolves paths relative to the current file and defaults a missing extension to
`.tex`. It uses unsaved editor contents when available and otherwise reads the
file from disk. It preserves following groups and adds a final newline when
needed to separate inserted tokens and comments from following text. The
referenced file stays in place.

The action requires a complete, literal brace argument. It is withheld for
missing files, self-inputs, comments between the command and its argument, and
locally redefined or project-declared `\input` commands. It expands only the
selected input. Nested inputs remain as written.

## Table refactoring

With the cursor inside a statically understood `tabular`, `tabular*`, or `array`
environment, the **Add column at end** code action appends a centered `c` column
to the preamble and an empty trailing cell to every row. The action is withheld
when the preamble uses unknown column types, a row has an ambiguous width, or
the environment has been redefined, so it never applies a partial table rewrite.

## Forward and inverse search

Forward search jumps from a source line to the matching place in the compiled
PDF. Badness passes the viewer the source file, the root document's PDF, and the
line number. The viewer reads the SyncTeX data and maps the position. Inverse
search starts in the viewer and sends the resolved source position to your
editor. For build instructions and viewer settings, see [Forward and inverse
search in Editor setup](editor-setup.md#forward-and-inverse-search).

### Triggering forward search

The server handles the custom request `textDocument/forwardSearch` with
`{ textDocument, position }` parameters. It uses the same method name and
parameters as Texlab, so a client written for Texlab works unchanged. The
capability is advertised as `experimental.textDocumentForwardSearch`. The
response includes a `status` field.

- `0` means Badness launched the viewer.
- `1` means the viewer failed to start.
- `2` means the PDF is missing or the buffer has no path.
- `3` means the viewer is unconfigured.

For status `2`, save the source file and build the document. If the PDF already
exists or forward search opens the wrong PDF, check how Badness identifies the
root document. See [`root` in the configuration
reference](../reference/configuration.md#root).

### Inverse search

The `badness inverse-search` command finds a running language server whose
workspace contains the file and asks it to reveal the position using
`window/showDocument`. Servers register for inverse search when their LSP
clients support that request. The command reports when no server is listening.
If several workspaces contain the file, the server with the longest matching
workspace root wins, so nested projects resolve deterministically.

Servers advertise themselves in `$BADNESS_IPC_DIR` when set. Otherwise they use
a per-user directory under `$XDG_RUNTIME_DIR` or fall back to the temporary
directory. The `forwardSearch.ipcDir` setting overrides the server's location.
If you use it for a container or remote development setup, point the viewer's
`badness inverse-search` command at the same directory with `--ipc-dir` or
`$BADNESS_IPC_DIR`.

Keep the directory path short on Unix. Badness logs an error if the socket path
exceeds the platform limit of about 100 bytes. In the shared `/tmp` fallback,
Badness uses a per-user directory with `0700` permissions and advertisements
with `0600` permissions. It accepts only advertisements owned by the current
user.

SyncTeX maps the source as it was compiled. Save and rebuild after edits to
bring source and PDF positions back into sync.
