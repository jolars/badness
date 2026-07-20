# Configuration

badness is configured through a `badness.toml` file. All keys are optional and
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
# wrap = "reflow"  # reflow | sentence | semantic | preserve

[lint]
# select = ["..."]  # if set, only these rules run
# ignore = []       # rules to disable
```

## Discovery

For each input, badness walks from the file's directory upward and uses the
first `badness.toml` it finds. The walk stops at a directory containing a
`.git` entry (the repository root), so a config outside your repository is
never picked up. If no file is found, built-in defaults apply.

Two global CLI flags override discovery:

- `--config <PATH>` uses that file instead of discovering one.
- `--no-config` ignores any discovered file and uses built-in defaults.

CLI flags for individual options (`--line-width`, `--wrap`, `--select`, …)
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

Maximum line width before the formatter breaks a line. Must be between 1
and 1000.

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

### `wrap`

How the formatter lays out line breaks *inside a paragraph*. It does not affect
structure—only where soft line breaks fall.

  | Mode       | Behavior                                                                                                        |
  | ---------- | --------------------------------------------------------------------------------------------------------------- |
  | `reflow`   | Greedy fill: pack words up to `line-width`, breaking only where the next word would overflow.                   |
  | `preserve` | Leave the authored line breaks untouched.                                                                       |
  | `sentence` | One sentence per line. Line width is ignored—a long sentence stays on one line.                                 |
  | `semantic` | [Semantic line breaks](https://sembr.org): keep the author's soft breaks *and* add a break after each sentence. |

Both `sentence` and `semantic` split a paragraph at sentence boundaries,
one sentence per line. Boundary detection is a small per-language rule engine
over the words: a `.`, `!`, or `?` ends a sentence *unless* the word is a known
abbreviation (`e.g.`, `Fig.`, `Dr.`, …), an ellipsis (`...`, `…`), or a
contextual abbreviation whose following word signals that the sentence
continues (`U.S. Government` stays together, `U.S. However` splits). The
abbreviation profile is chosen by [`lang`](#lang) and extended by
[`no-break-abbreviations`](#no-break-abbreviations).

`semantic` additionally **preserves the author's own line breaks** on top of
the sentence breaks (the [sembr](https://sembr.org) convention). It does not
detect clause boundaries itself—a break after a comma or `and` survives only
where the author placed a newline. A run-on sentence on a single source line is
still sentence-split.

When omitted, the formatter uses each file kind's default: `.tex` and `.bib`
files reflow, while code-heavy `.sty`, `.cls`, `.dtx`, and `.ins` files
preserve authored line breaks. Setting `wrap` applies the same mode to every
file kind.

**Default value**: unset (per file kind: `.tex`/`.bib` → `reflow`,
`.sty`/`.cls`/`.dtx`/`.ins` → `preserve`)

**Type**: `"reflow" | "sentence" | "semantic" | "preserve"`

**Example**:

```toml
[format]
wrap = "sentence"
```

### `lang`

Document language as a BCP-47-style code (`en`, `de`, `pt-BR`, …), used by the
`sentence` and `semantic` wrap modes to pick the sentence-boundary abbreviation
profile. Built-in profiles cover English (default), Czech, German, Spanish, and
French; the region subtag is folded away, and an unknown or unset language
falls back to English. (Automatic detection from `babel`/`polyglossia` is not
yet implemented.)

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
[BibTeX](bib-linter-rules.md) rule sets. Every rule is on by default. An
unknown rule id is reported at lint time, not rejected at config-parse time.

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

Rule ids to disable, applied on top of either [`select`](#select) or the
default rule set.

**Default value**: `[]`

**Type**: array of strings

**Example**:

```toml
[lint]
ignore = ["missing-nonbreaking-space"]
```

## `[build]`

Where the TeX compiler leaves its artifacts. Read by the **language server**
only, which pulls resolved label and section numbers from the `.aux` files for
hover and document symbols; never by the formatter or linter.

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

> **Note**: TEXMF-tree discovery (the former `[texmf]` section) is configured
> through your editor's LSP settings, not `badness.toml`. Where a TeX
> installation lives is a fact about the machine, not the project, so it does
> not belong in a file shared across contributors. See [Editor
> Setup](../guide/editor-setup.md#texmf-discovery).
