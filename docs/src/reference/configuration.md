# Configuration

Badness is configured through a `badness.toml` file. All keys are optional and
spelled in kebab-case; an unknown key or section is a hard error, not a silent
no-op. Run `badness init` to write a commented starter file showing every key at
its default.

```toml
# Gitignore-style patterns to skip during directory discovery.
# exclude = [".git/"]
# extend-exclude = []

[format]
# line-width = 80
# indent-width = 2
# item-indent = "hang"  # hang | indent | none
# wrap = "reflow"  # reflow | stable | sentence | semantic | preserve
# line-ending = "auto"  # auto | lf | crlf | native

[lint]
# select = ["..."]  # if set, only these rules run
# ignore = []       # rules to disable
```

## Discovery

For each input, Badness walks from the file's directory upward and uses the
first `badness.toml` it finds. The walk stops at a directory containing a `.git`
entry (the repository root), so a config file outside your repository is never
picked up.

If no project file is found, Badness next checks the `BADNESS_CONFIG`
environment variable. When set (and non-empty), it names a config file to use
instead of the global user config below—handy for keeping one config on a synced
drive and pointing every machine at it. A set `BADNESS_CONFIG` shadows the
global config entirely.

If `BADNESS_CONFIG` is unset, badness falls back to a global user config: the
first existing file among

1. `$XDG_CONFIG_HOME/badness/config.toml`
2. `~/.config/badness/config.toml`
3. the platform config directory (`%APPDATA%\badness\config.toml` on Windows,
   `~/Library/Application Support/badness/config.toml` on macOS)

The `BADNESS_CONFIG` and global files use the same schema as a project
`badness.toml` and are whole-file fallbacks, never merged with a project config.
Relative `exclude` patterns in them resolve against the working directory (CLI)
or the document's directory (language server) rather than the config's own
directory. The language server uses the same resolution, so both are easy ways
to set editor-wide defaults such as `wrap = "preserve"` (an edit is picked up
when the server restarts). If none of these files is found, built-in defaults
apply.

Two global CLI flags override discovery:

- `--config <PATH>` uses that file instead of discovering one.
- `--no-config` ignores any project, `BADNESS_CONFIG`, or global file and uses
  built-in defaults.

CLI flags for individual options (`--line-width`, `--wrap`, `--select`, etc.)
override the corresponding config values for a single run.

## Top level

### `exclude`

Gitignore-style patterns to exclude from directory discovery, resolved relative
to the directory containing the `badness.toml`. Excludes apply to both `format`
and `lint`, which share one file walk, so this is a top-level key rather than a
`[format]` option.

When set, this **replaces** the built-in default set (`[".git/"]`); use
[`extend-exclude`](#extend-exclude) to add patterns without restating the
defaults. Patterns given with the `--exclude` CLI flag are always added on top.

**Default value**: `[".git/"]`

**Type**: array of strings

**Example**:

```toml
exclude = ["vendor/", "old-drafts/"]
```

### `extend-exclude`

Gitignore-style patterns added *in addition to* the base set selected by
[`exclude`](#exclude) (the built-in defaults when `exclude` is unset). Use this
to skip a few extra paths without replacing the defaults.

**Default value**: `[]`

**Type**: array of strings

**Example**:

```toml
extend-exclude = ["build/"]
```

## `[format]`

Options for `badness format`. Each mirrors a CLI flag of the same name, which
takes precedence for a single run.

### `line-width`

Maximum line width before the formatter breaks a line. Must be between 1 and 1000.

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

  | Mode     | Behavior                                                      |
  | -------- | ------------------------------------------------------------- |
  | `hang`   | Align under the body following a bare `\item ` (the default). |
  | `indent` | Add one `indent-width` step from the `\item` column.          |
  | `none`   | Align with the `\item` command.                               |

Labels and Beamer overlays do not widen the `hang` offset, so items retain one
continuation edge regardless of marker width.

**Default value**: `"hang"`

**Type**: string

**Example**:

```toml
[format]
item-indent = "indent"
```

### `wrap`

How the formatter lays out line breaks *inside a paragraph*. It does not affect
structure, only where soft line breaks fall.

  | Mode       | Behavior                                                                                                        |
  | ---------- | --------------------------------------------------------------------------------------------------------------- |
  | `reflow`   | Greedy fill: pack words up to `line-width`, breaking only where the next word would overflow.                   |
  | `stable`   | Preserve acceptable authored breaks and rebalance only text that no longer fits (keeps revision diffs small).   |
  | `preserve` | Leave the authored line breaks untouched.                                                                       |
  | `sentence` | One sentence per line. Line width is ignored—a long sentence stays on one line.                                 |
  | `semantic` | [Semantic line breaks](https://sembr.org): keep the author's soft breaks *and* add a break after each sentence. |

Both `sentence` and `semantic` split a paragraph at sentence boundaries, one
sentence per line. Boundary detection is a small per-language rule engine over
the words: a `.`, `!`, or `?` ends a sentence *unless* the word is a known
abbreviation (`e.g.`, `Fig.`, `Dr.`, etc.) an ellipsis (`...`, `…`), or a
contextual abbreviation whose following word signals that the sentence continues
(`U.S. Government` stays together, `U.S. However` splits). The abbreviation
profile is chosen by [`lang`](#lang) and extended by
[`no-break-abbreviations`](#no-break-abbreviations).

`semantic` additionally *preserves the author's own line breaks* on top of the
sentence breaks (the [sembr](https://sembr.org) convention). It does not detect
clause boundaries itself—a break after a comma or `and` survives only where the
author placed a newline. A run-on sentence on a single source line is still
sentence-split.

`stable` also preserves authored line breaks, but treats them as preferred
anchors rather than hard boundaries. It is aimed at keeping revision diffs
small: a small prose edit perturbs the smallest possible region. Each prose run
is solved as one global layout problem. Candidate layouts are compared
lexicographically by total overflow, underflow below a soft target
(`line-width - 15`), changed authored breaks, displacement from the nearest
authored break, raggedness around that target, and line count. This makes the
hard width non-negotiable before minimizing source churn, while a short final
line remains unpenalized. Blank lines and command-only lines bound each
independently optimized run, and code-like statement bodies retain ordinary
greedy fill. (The soft target is not currently configurable.)

When omitted, every file kind reflows—`.tex`, `.bib`, `.sty`, `.cls`, `.dtx`,
and `.ins` alike. A file's extension is not a layout input.

That is safe because reflow is never the thing that decides whether content may
move. The formatter declines to reflow anything it cannot lay out without
changing meaning, in *every* wrap mode and regardless of what you configure:
verbatim bodies and `\verb`, comments, `.dtx` documentation margins and docstrip
guards (which must stay at column 0), and any documentation block whose
rewrapping would push a `%` off column 0. Asking for `wrap = "reflow"` on a
`.dtx` cannot corrupt it; asking for `wrap = "preserve"` on a `.tex` is a
stylistic choice, not a safety one.

Code, in practice, has little to reflow: expl3 regions
(`\ExplSyntaxOn`…`\ExplSyntaxOff`) are laid out by their own rules whatever
`wrap` says, and a source line consisting only of commands keeps its own line.
So a package or class body formats much as it did before, and `preserve` remains
available if you want authored breaks kept verbatim.

**Default value**: unset (`reflow`, for every file kind)

**Type**: `"reflow" | "stable" | "sentence" | "semantic" | "preserve"`

**Example**:

```toml
[format]
wrap = "stable"
```

### `math-wrap`

How the formatter lays out line breaks inside *display math*: `\[…\]`, `$$…$$`,
and single-formula math environments such as `equation`. Alignment-grid
environments (`align`, `gather`, matrices) and inline `$…$` math are not
affected.

  | Mode          | Behavior                                                                                                              |
  | ------------- | --------------------------------------------------------------------------------------------------------------------- |
  | `auto`        | Derive from the effective [`wrap`](#wrap): `preserve` keeps authored math breaks, every other mode breaks (amsmath).  |
  | `preserve`    | Keep the authored line breaks inside the body. Spacing within each line is still normalized.                          |
  | `single-line` | Never insert breaks: the body stays on one line, overflowing `line-width` if too long (like inline math).             |
  | `break`       | Break a too-long body before its top-level relations and binary operators, aligning a relation chain (amsmath style). |

**Default value**: `"auto"`

**Type**: `"auto" | "preserve" | "single-line" | "break"`

**Example**:

```toml
[format]
math-wrap = "preserve"
```

### `line-ending`

How the line breaks in formatted output are spelled. The layout engine always
decides *where* breaks go; this decides only the bytes they render as, and it
applies to the whole document — including inside `verbatim`-style protected
regions, which would otherwise keep their authored endings and leave the file
mixed.

  | Mode     | Behavior                                                                                       |
  | -------- | ---------------------------------------------------------------------------------------------- |
  | `auto`   | Keep the endings the file was written with: CRLF if its first line break is one, LF otherwise. |
  | `lf`     | Always `\n`.                                                                                   |
  | `crlf`   | Always `\r\n`.                                                                                 |
  | `native` | The platform's convention: `\r\n` on Windows, `\n` elsewhere.                                  |

The default is `auto`, so formatting never rewrites a repository's line endings
on its own — set `lf` (or add a `.gitattributes` rule) if you want them
normalized.

**Default value**: `"auto"`

**Type**: `"auto" | "lf" | "crlf" | "native"`

**Example**:

```toml
[format]
line-ending = "lf"
```

### `lang`

Document language as a BCP-47-style code (`en`, `de`, `pt-BR`, …), used by the
`sentence` and `semantic` wrap modes to pick the sentence-boundary abbreviation
profile. Built-in profiles cover English (default), Czech, German, Spanish, and
French; the region subtag is folded away, and an unknown or unset language falls
back to English. (Automatic detection from `babel`/`polyglossia` is not yet
implemented.)

**Default value**: unset (English)

**Type**: string

**Example**:

```toml
[format]
lang = "de"
```

### `no-break-abbreviations`

User-supplied no-break abbreviations for the `sentence` and `semantic` wrap
modes, keyed by language code or the literal `default` bucket (applied to every
document). An abbreviation listed here never ends a sentence, so no line break
is inserted after it. Merged on top of the built-in per-language lists.

**Default value**: `{}`

**Type**: table of string arrays, keyed by language code or `default`

**Example**:

```toml
[format.no-break-abbreviations]
default = ["ibid."]         # applied to every document
de = ["bzw.", "Abb."]       # applied only when lang resolves to German
```

## `[lint]`

Rule selection for `badness lint`, shared by the [LaTeX](linter-rules.md) and
[BibTeX](bib-linter-rules.md) rule sets. Every rule is on by default. An unknown
rule id is reported at lint time, not rejected at config-parse time.

### `select`

Explicit allowlist of rule ids. When set, only these rules run.

**Default value**: unset (all rules run)

**Type**: array of strings

**Example**:

```toml
[lint]
select = ["deprecated-command", "dollar-display-math"]
```

### `ignore`

Rule ids to disable, applied on top of either [`select`](#select) or the default
rule set.

**Default value**: `[]`

**Type**: array of strings

**Example**:

```toml
[lint]
ignore = ["missing-nonbreaking-space"]
```

## `[build]`

Where the TeX compiler leaves its artifacts, and which file it was run on. Read
by the **language server** only — it pulls resolved label and section numbers
from the `.aux` files for hover and document symbols, and locates the compiled
PDF for [forward search](../guide/editor-setup.md#forward-and-inverse-search).
Never read by the formatter or linter.

### `aux-dir`

Directory holding the build's `.aux` files (latexmk's `-auxdir`/`-outdir`),
resolved relative to the root document's directory when not absolute. When
unset, each document's `.aux` is expected next to it, as in plain
`latex`/`pdflatex` runs.

**Default value**: unset (sibling `.aux` files)

**Type**: path

**Example**:

```toml
[build]
aux-dir = "out"
```

### `pdf-dir`

Directory holding the build's PDF output (latexmk's `-outdir`), resolved
relative to the root document's directory when not absolute. When unset, the PDF
is expected next to the root document.

**Default value**: unset (the root document's own directory)

**Type**: path

**Example**:

```toml
[build]
pdf-dir = "out"
```

### `pdf-filename`

The compiled PDF's file name, when the build does not name it after the root
document (latexmk's `-jobname`). A **bare file name**, never a path — use
`pdf-dir` for the directory — and `.pdf` is appended when it carries no
extension, so `"thesis"` and `"thesis.pdf"` mean the same thing.

**Default value**: unset (`<root document stem>.pdf`)

**Type**: string

**Example**:

```toml
[build]
pdf-filename = "thesis.pdf"
```

### `root`

The project's root document — the file the compiler was run on — resolved
relative to this `badness.toml`'s directory when not absolute.

Normally the root is found by scanning the project for a file carrying
`\documentclass` or `\begin{document}`, and you do not need this key. But that
scan only sees files the server has already loaded, and it loads them one
directory at a time: editing `chapters/ch1.tex` in a project rooted at
`../main.tex` never loads `main.tex`, so the scan finds no root at all and
forward search resolves the wrong PDF. Set `root` for that layout.

**Default value**: unset (scan the project for a document root)

**Type**: path

**Example**:

```toml
[build]
root = "main.tex"
```

## `[commands]`

Declares project commands whose first braced argument contains label or citation
keys. This covers wrappers that Badness cannot recognize without macro
expansion, which remains deliberately out of scope.

Entries are keyed by the command name without its leading backslash:

```toml
# A comma-separated list of label keys.
[commands.eqrefs]
like = "cref"

# A comma-separated list of bibliography keys.
[commands.projectcite]
like = "parencite"
```

The `like` target must be a curated reference or citation command. It determines
the key behavior: `ref`/`eqref` accept one label, `cref` and its list-valued
siblings split on commas, citation commands split on commas, and `nocite`
preserves the special `*` wildcard.

Command declarations affect linting, label/citation navigation, rename, and key
completion. They do not expand the macro, declare its arity, change argument
attachment, or lend formatter layout. Use the command whose observable key
behavior matches the wrapper: an `eqrefs` command that accepts several labels is
`like = "cref"`, even if its implementation calls `\eqref` once per key.

Anything that would silently do nothing is a configuration error: an empty
entry, an invalid control-word name, an unknown or non-ref/cite `like` target,
or an attempt to reclassify a curated built-in command.

### Command `like`

The curated reference or citation command whose key behavior this project
command copies.

**Type**: string

**Example**:

```toml
[commands.eqrefs]
like = "cref"
```

## `[environments]`

Declares environments Badness cannot recognize from the file alone: one that
behaves like a built-in but has no built-in counterpart, one whose body is
verbatim, and one reached through command spellings rather than `\begin`/`\end`.
This is the only section that changes how your files are *parsed*, so it is read
by `format`, `lint`, and the language server alike; editing it makes the server
reparse the project.

Entries are keyed by the environment's own name, whether or not Badness already
knows it:

```toml
# \begin{myenv} … \end{myenv}, with no built-in counterpart
[environments.myenv]
like = "align"

# extra delimiter spellings for an environment Badness already knows
[environments.eqnarray]
begin = ['\bea']
end = ['\eea']

# one side alone: `\bsplit` expands to `\begin{split}`, so a written-out
# `\end{split}` closes it — there is no closing command to declare
[environments.split]
begin = ['\bsplit']

# both at once: an environment reached only through commands
[environments.mytheorem]
like = "theorem"
begin = ['\startmyenv']
end = ['\endmyenv']
```

Write control words as TOML **literal** strings (single quotes) so the backslash
needs no escaping: `'\bea'`, not `"\\bea"`. Both spellings are accepted, and so
is a name with no backslash at all — a control word can never contain one, so
there is nothing to disambiguate.

A declaration names a **spelling**, never a pairing. Every structural rule still
applies, so a declared `\bea` whose `\eea` is unreachable — stranded inside a
brace group, or simply missing — stays an ordinary command, exactly as it would
without the declaration. A wrong declaration therefore does nothing to your
document; it cannot corrupt it.

What a declaration *cannot* do is invent behavior. It only ever points at an
environment Badness already curates, so there is no way to spell out "this one
is math, takes two arguments, and has a verbatim body" key by key. If nothing
built in resembles yours, that is worth an issue rather than a workaround.

Anything a declaration cannot satisfy is an error at config load, reported
against the key you wrote, rather than a block that parses and quietly does
nothing:

- an entry with no keys under it, which would declare nothing at all
- `like` naming an environment Badness does not know
- delimiter spellings for a verbatim environment (the closing command is never
  seen — the verbatim body has already swallowed it)
- delimiter spellings for an environment that takes arguments (a bare command
  carries none)
- delimiter spellings for an environment with no `like` and no built-in of that
  name, so its behavior is unknown
- one spelling claimed by two entries, or listed twice by one
- a spelling that is the delimiter itself (`'\end{split}'`) rather than a
  command standing in for one — the written-out delimiter already pairs with a
  declared spelling, so the key can just be removed
- a spelling that could never be a single control word (`'\b ea'`, `'\bea2'`)
- a spelling that is already a LaTeX command Badness knows (`'\emph'`), which
  would change what that command means throughout the project

### `like`

The built-in environment whose behavior this one copies: whether its body is
math, whether it aligns on `&`, whether it is verbatim, and every such property
at once. This is also how you name a verbatim environment defined by machinery
no scan can follow — `like = "lstlisting"` protects its body from reflowing and
from lint findings.

The target is looked up among the environments Badness curates by hand; a
misspelled one is an error rather than a silent no-op.

**Default value**: unset

**Type**: string

**Example**:

```toml
[environments.mycode]
like = "lstlisting"
```

### `begin`

Command spellings that stand in for this environment's `\begin{…}`. Any of them
opens it, and any spelling in [`end`](#end) closes it — pairing is by side, not
by position, so the two lists need not be the same length.

The written-out `\end{…}` closes it too, which is why `end` is optional. A
command defined as `\def\bsplit{\begin{split}}` *expands to* `\begin{split}`, so
`\bsplit … \end{split}` is a perfectly ordinary environment and there may be no
closing command to name at all.

Use this when the definition is somewhere Badness cannot see: a sibling `.sty`,
or one built by machinery no scan follows. A definition written with a plain
`\newcommand` or `\def` in the *same* file —
`\newcommand{\bea}{\begin{eqnarray}}`, or `\def\bsplit{\begin{split}}` on its
own — is already recognized without any configuration.

A spelling must be a command of your own. Naming one Badness already knows
(`'\emph'`, `'\section'`) is an error rather than a redefinition: the
declaration would apply everywhere that command appears, which is never what a
delimiter declaration means.

**Default value**: `[]`

**Type**: array of strings (control words)

**Example**:

```toml
[environments.eqnarray]
begin = ['\bea', '\beqa']
end = ['\eea']
```

### `end`

Command spellings that stand in for this environment's `\end{…}`, the mirror of
[`begin`](#begin) in every respect — including that it stands alone. A command
defined as `\def\eeq{\end{equation}}` closes a written-out `\begin{equation}`,
so an entry may name a closing spelling without naming an opening one.

**Default value**: `[]`

**Type**: array of strings (control words)

**Example**:

```toml
[environments.eqnarray]
begin = ['\bea']
end = ['\eea']
```

> **Note**: TEXMF-tree discovery (the former `[texmf]` section) is configured
> through your editor's LSP settings, not `badness.toml`. Where a TeX
> installation lives is a fact about the machine, not the project, so it does
> not belong in a file shared across contributors. See [Editor
> Setup](../guide/editor-setup.md#texmf-discovery).
