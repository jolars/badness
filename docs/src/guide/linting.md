# Linting

`badness lint` parses each file and reports diagnostics, rendered with source
snippets pointing at the offending range. It exits non-zero when there is at
least one diagnostic, which makes it usable as a CI gate.

```sh
badness lint paper.tex
cat paper.tex | badness lint   # stdin
```

## Parse diagnostics

Alongside the rules, the linter surfaces **parse diagnostics**: places where the
parser recovered from malformed input. Because the parser is error-tolerant, a
single problem never aborts the parse—badness anchors recovery on clean LaTeX
boundaries (`\end{…}`, `\begin`, a blank line, `}`, `$`, `&`, `\\`) and keeps
going, so one file can report several independent diagnostics in one run. Parse
diagnostics carry the rule id `parse` and are never silenced by
`select`/`ignore`.

## Rules

Beyond parse recovery, badness ships a growing set of built-in rules
(`deprecated-command`, `dollar-display-math`, `undefined-ref`, and more). Each
has a stable id used in diagnostics, config, and suppression comments. See the
[Linter Rules](../reference/linter-rules.md) reference for the full catalogue,
or print a single rule's description and examples from the terminal:

```sh
badness lint --explain deprecated-command
```

Every rule is on by default. Narrow the active set through the `[lint]` table in
`badness.toml` or the matching `--select`/`--ignore` CLI flags; see the
[Configuration reference](../reference/configuration.md#lint).

Suppress a rule at one site with a comment directive:

```tex
% badness-lint skip deprecated-command: legacy code
{\bf here}
```

The verb carries the scope, and there are three:

  | Scope              | Directive                                                |
  | ------------------ | -------------------------------------------------------- |
  | The next construct | `% badness-lint skip <rule>: <reason>`                   |
  | A region           | `% badness-lint off <rule>` … `% badness-lint on <rule>` |
  | The whole file     | `% badness-lint skip-file <rule>: <reason>`              |

Naming the `<rule>` is optional---leave it out and the directive covers every
rule over that same span. An `off` with no matching `on` runs to the end of the
file. The `: <reason>` tail is optional everywhere and is never interpreted.

Each has a bare counterpart that turns off the **formatter** at the same time:
`% badness skip`, `% badness off` / `% badness on`, and `% badness skip-file`.
For layout only, use the `% badness-format` spellings described in
[Formatting](formatting.md#turning-the-formatter-off).

In `.bib` files the same grammar rides an `@comment` entry, since BibTeX has no
line-comment token:

```bib
@comment{badness-lint skip missing-required-field: publisher long gone}
@book{oldbook, title = {An Orphaned Book}}
```

Some rules ship an **auto-fix**. `badness lint --fix` applies the
meaning-preserving (Safe) ones; `--unsafe-fixes` also applies fixes that may
change output, such as `missing-nonbreaking-space` (inserting a tie changes line
breaking), `abbreviation-spacing` (inserting `\` or `\@` changes sentence
spacing), or `space-before-command` (deleting a space before `\footnote` changes
spacing).

## Machine-readable output

`badness lint --output json` emits the findings as a JSON array on **stdout**
(the human-readable `pretty` and `concise` modes write to stderr). A clean run
emits `[]`, so consumers always receive valid JSON; the exit code still signals
whether findings exist. This is the contract external tools consume, e.g.
panache when linting `latex` code blocks in Markdown documents.

```json
[
  {
    "rule": "ellipsis",
    "severity": "warning",
    "path": "paper.tex",
    "start": 5,
    "end": 8,
    "message": "literal `...` ellipsis; use `\\dots`",
    "fix": {
      "edits": [{ "content": "\\dots", "start": 5, "end": 8 }],
      "applicability": "safe",
      "description": "Replace `...` with `\\dots`"
    },
    "related": []
  }
]
```

Ranges are 0-indexed byte offsets into the named file (no line/column
resolution). `severity` is one of `error`, `warning`, `info`, or `hint`;
`applicability` is `safe` or `unsafe` (the `--fix`/`--unsafe-fixes` split). The
`fix` key is omitted when a finding has no auto-fix. An edit carries a `path`
key only when it targets a *different* file than the diagnostic (a cross-file
fix); `related` lists secondary "see also" locations.

Compared to the sibling tools arity and fatou, the schema differs in two ways:
offsets are flat `start`/`end` keys rather than a `range` object, and `message`
is a plain string rather than a structured object.
