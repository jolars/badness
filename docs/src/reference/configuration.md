# Configuration reference

Use kebab-case for configuration keys in `badness.toml`. Badness reports an
error for an unknown key or section.

<!-- Preserve bookmarks to sections moved into the configuration guide. -->
<span id="discovery"></span> <span id="editor-support"></span>
<span id="vs-code-even-better-toml"></span>
<span id="inline-schema-directive"></span> <span id="other-editors"></span>

For configuration
[discovery](../guide/configuration.md#choose-the-configuration-source), project
setup, and [editor support](../guide/configuration.md#editor-support), see the
configuration guide.

## Top level

### `extend`

Load another config as a base. Relative `extend` paths start in the directory of
the file that sets `extend`. Absolute paths and paths starting with `~/` also
work. You can chain several files. Missing files and cycles are errors.

The child overrides individual settings, including arrays. `extend-exclude` and
`[lint] extend-select` append to the inherited lists. Entries in `[commands]`
and `[environments]` merge by name. In a project config, relative paths in
inherited settings use the selected config's directory.

**Default value**: unset

**Type**: string

**Example**:

```toml
extend = "../shared/badness.toml"

[format]
line-width = 100
```

### `exclude`

Gitignore-style patterns for files and directories that `format` and `lint` skip
during discovery. In a project config, patterns are relative to the
`badness.toml` directory. This list replaces the defaults. The CLI flag
`--exclude` adds patterns to the list.

Use a pattern such as `plots/*.pgf` to skip generated figures during discovery.

**Default value**: `[".git/"]`

**Type**: array of strings

**Example**:

```toml
exclude = ["vendor/", "old-drafts/"]
```

### `extend-exclude`

Extra patterns to add to [`exclude`](#exclude) or to the built-in defaults.

**Default value**: `[]`

**Type**: array of strings

**Example**:

```toml
extend-exclude = ["build/"]
```

## `[format]`

Options for `badness format`. CLI flags such as `--line-width` override their
settings for a single run.

### `line-width`

Target width for wrapping, from `0` to `1000`. This applies to prose, display
math, command arguments, and BibTeX values. Set `0` to turn off width-based
wrapping. The formatter still applies structural breaks and the breaks required
by the selected [`wrap`](#wrap) mode.

**Default value**: `80`

**Type**: integer

**Example**:

```toml
[format]
line-width = 100
```

### `indent-width`

Spaces per indent step. Must be between 1 and 1000.

**Default value**: `2`

**Type**: integer

**Example**:

```toml
[format]
indent-width = 4
```

### `item-indent`

How continuation lines in list items are indented relative to the `\item`
command.

  | Mode     | Behavior                                                  |
  | -------- | --------------------------------------------------------- |
  | `hang`   | Align below the start of the body after a plain `\item `. |
  | `indent` | Add one `indent-width` step from the `\item` column.      |
  | `none`   | Align with the `\item` command.                           |

With `hang`, labels and Beamer overlays leave the alignment column fixed.

**Default value**: `"hang"`

**Type**: string

**Example**:

```toml
[format]
item-indent = "indent"
```

### `wrap`

Controls line breaks within paragraphs.

  | Mode       | Behavior                                                                                      |
  | ---------- | --------------------------------------------------------------------------------------------- |
  | `reflow`   | Fill each line up to `line-width`.                                                            |
  | `stable`   | Favor existing line breaks while fitting the text to `line-width`.                            |
  | `preserve` | Keep existing line breaks.                                                                    |
  | `sentence` | Put each sentence on one line, even if it exceeds `line-width`.                               |
  | `semantic` | Keep existing breaks, start each sentence on a new line, and wrap long lines at `line-width`. |

With `line-width = 0`, `stable` keeps existing breaks and `semantic` adds only
sentence breaks. For sentence detection, see [`lang`](#lang) and
[`no-break-abbreviations`](#no-break-abbreviations).

**Default value**: unset (`reflow`)

**Type**: `"reflow" | "stable" | "sentence" | "semantic" | "preserve"`

**Example**:

```toml
[format]
wrap = "stable"
```

### `math-wrap`

Controls line breaks in single-formula displays such as `\[…\]`, `$$…$$`, and
`equation`.

  | Mode          | Behavior                                                                                     |
  | ------------- | -------------------------------------------------------------------------------------------- |
  | `auto`        | Use `preserve` when [`wrap`](#wrap) is `preserve`. Use `break` for other wrap modes.         |
  | `preserve`    | Keep the existing breaks.                                                                    |
  | `single-line` | Keep the formula on one line, even if it exceeds `line-width`.                               |
  | `break`       | Wrap long formulas before top-level relations and binary operators. Align chained relations. |

**Default value**: `"auto"`

**Type**: `"auto" | "preserve" | "single-line" | "break"`

**Example**:

```toml
[format]
math-wrap = "preserve"
```

### `line-ending`

Line endings in the formatted output.

  | Mode     | Behavior                                                            |
  | -------- | ------------------------------------------------------------------- |
  | `auto`   | Use CRLF if the input's first line break is CRLF. Otherwise use LF. |
  | `lf`     | Use `\n`.                                                           |
  | `crlf`   | Use `\r\n`.                                                         |
  | `native` | Use `\r\n` on Windows and `\n` elsewhere.                           |

Protected BibTeX regions keep their original line endings. When one appears at
the start of the output, `auto` uses its first line break if it has one. See
[BibTeX format suppression](../guide/formatting.md#bibtex-format-suppression).

**Default value**: `"auto"`

**Type**: `"auto" | "lf" | "crlf" | "native"`

**Example**:

```toml
[format]
line-ending = "lf"
```

### `lang`

Language code for detecting sentence breaks in `sentence` and `semantic` modes.
Badness uses the first part of the code, such as `de` in `de-AT`. Built-in
profiles cover Czech, English, French, German, and Spanish. Other languages use
the English profile.

**Default value**: unset (English)

**Type**: string

**Example**:

```toml
[format]
lang = "de"
```

### `no-break-abbreviations`

Extra abbreviations to skip when looking for sentence endings in `sentence` and
`semantic` modes. Use a language code for a specific language or `default` for
all languages.

**Default value**: `{}`

**Type**: table of string arrays, keyed by language code or `default`

**Example**:

```toml
[format.no-break-abbreviations]
default = ["ibid."]         # applied to every document
de = ["bzw.", "Abb."]       # applied only when lang resolves to German
```

## `[lint]`

Choose which rules `badness lint` runs. For rule IDs and defaults, see the
[LaTeX](linter-rules.md) and [BibTeX](bib-linter-rules.md) rule references.

### `select`

Base rule ids to enable. When set, this replaces the default rule set. An empty
`select = []` disables the defaults. [`extend-select`](#extend-select) can still
add rules, and [`ignore`](#ignore) takes precedence over both lists.

**Default value**: unset (all default-enabled rules run)

**Type**: array of strings

**Example**:

```toml
[lint]
select = ["deprecated-command", "dollar-display-math"]
```

### `extend-select`

Additional rules to run alongside [`select`](#select) or the defaults. Entries
accumulate across files linked by [`extend`](#extend). The repeatable CLI flag
`--extend-select` adds more entries.

**Default value**: `[]`

**Type**: array of strings

**Example**:

```toml
[lint]
extend-select = ["dash-length"]
```

### `ignore`

Rule ids to disable, including those added by [`extend-select`](#extend-select).
Applied after [`select`](#select) or the defaults and `extend-select`.

**Default value**: `[]`

**Type**: array of strings

**Example**:

```toml
[lint]
ignore = ["missing-nonbreaking-space"]
```

## `[build]`

Tell the language server where to find the main document, `.aux` files for label
and section numbers, and the compiled PDF for [forward
search](../guide/language-server.md#forward-and-inverse-search).

### `aux-dir`

Directory containing `.aux` files, such as latexmk's `-auxdir` or `-outdir`.
Relative paths start in the root document's directory.

**Default value**: unset (sibling `.aux` files)

**Type**: path

**Example**:

```toml
[build]
aux-dir = "out"
```

### `pdf-dir`

Directory containing the compiled PDF, such as latexmk's `-outdir`. Relative
paths start in the root document's directory.

**Default value**: unset (the root document's own directory)

**Type**: path

**Example**:

```toml
[build]
pdf-dir = "out"
```

### `pdf-filename`

Name of the compiled PDF, for example when using latexmk's `-jobname`. Use a
file name here and `pdf-dir` for its directory. Badness adds `.pdf` if you omit
the extension.

**Default value**: unset (`<root document stem>.pdf`)

**Type**: string

**Example**:

```toml
[build]
pdf-filename = "thesis.pdf"
```

### `root`

Path to the main `.tex` file you compile. Relative paths start in the
`badness.toml` directory. Set this when the language server needs help finding
the main file, for example when you edit chapters in a separate directory.

**Default value**: unset (infer from files the language server has loaded)

**Type**: path

**Example**:

```toml
[build]
root = "main.tex"
```

## `[commands]`

Tell Badness which custom commands take label or citation keys in their first
braced argument. This enables lint checks, navigation, rename, and key
completion. Use a custom control word without its leading backslash as the table
name.

### Command `like`

Required for each entry. Name a reference or citation command that Badness
already recognizes. Use `ref` or `eqref` for a single label, `cref` for a
comma-separated list of labels, or `parencite` for a comma-separated list of
bibliography keys. `nocite` also accepts `*`.

**Type**: string

**Example**:

```toml
[commands.eqrefs]
like = "cref"
```

## `[environments]`

Describe custom environments and the commands that open or close them. Name each
table after the environment as it appears in `\begin{…}`. Declarations apply to
formatting, linting, and the language server.

Use [`like`](#like) to copy a built-in environment's behavior. Use
[`begin`](#begin) or [`end`](#end) to add aliases to an argument-free,
non-verbatim environment. To add aliases to a custom environment, set `like`
too.

Each alias must be a unique custom control word, such as `'\bea'` or `'bea'`.
Use single quotes to write a literal backslash. You can mix aliases with the
usual `\begin{…}` and `\end{…}` and list any number on either side.

### `like`

The built-in environment whose behavior to copy. For example, `like = "align"`
uses math alignment, and `like = "lstlisting"` treats the body as verbatim.

**Default value**: unset

**Type**: string

**Example**:

```toml
[environments.mycode]
like = "lstlisting"
```

### `begin`

Commands that open the environment in place of `\begin{…}`. With the example
below, `\bsplit` opens an environment you can close with `\end{split}`.

**Default value**: `[]`

**Type**: array of strings (control words)

**Example**:

```toml
[environments.split]
begin = ['\bsplit']
```

### `end`

Commands that close the environment in place of `\end{…}`. A closing alias works
with `\begin{…}` or any opening alias for the same environment.

**Default value**: `[]`

**Type**: array of strings (control words)

**Example**:

```toml
[environments.eqnarray]
begin = ['\bea']
end = ['\eea']
```
