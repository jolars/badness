# Linting

`badness lint` parses each file and reports diagnostics with source snippets
that point to the offending range. It exits with a nonzero status when it
reports at least one diagnostic, so you can use it to fail a CI check.

```sh
badness lint paper.tex
cat paper.tex | badness lint -   # stdin
```

Pass `-` to read standard input, or omit paths when piping:
`cat paper.tex | badness lint` works too. At an interactive prompt with no
paths, the command reports a usage error.

## Parse diagnostics

Alongside lint findings, the linter reports **parse diagnostics** where the
parser recovered from malformed input. A single problem never aborts the parse.
Badness resumes at LaTeX boundaries such as `\end{…}`, `\begin`, a blank line,
`}`, `$`, `&`, or `\\`, so it can report several independent diagnostics for one
file in one run. Parse diagnostics carry the rule id `parse` and are never
silenced by `select`/`ignore`.

## Rules

Beyond parse recovery, Badness ships a growing set of built-in rules
(`deprecated-command`, `dollar-display-math`, `undefined-ref`, and more). Each
has a stable id used in diagnostics, config, and suppression comments. See the
[Linter rules](../reference/linter-rules.md) reference for the full catalog, or
print a single rule's description and examples from the terminal:

```sh
badness lint --explain deprecated-command
```

Most rules are on by default; the rule reference marks opt-in rules. Use
`extend-select` in the `[lint]` table in `badness.toml` to enable additional
rules while keeping the defaults:

```toml
[lint]
extend-select = ["dash-length"]
```

The CLI flag `--extend-select` adds to the configured list and can be repeated.
Set `select` or use `--select` to replace the default set, then add rules with
`extend-select`. An empty `select = []` leaves only those additional rules. Use
`ignore` or `--ignore` to disable rules from either list; see the [Configuration
reference](../reference/configuration.md#lint).

Suppress a rule at one site with a comment directive:

```tex
% badness-lint skip deprecated-command: legacy code
{\bf here}
```

The directive's verb determines its scope:

  | Scope              | Directive                                                |
  | ------------------ | -------------------------------------------------------- |
  | The next construct | `% badness-lint skip <rule>: <reason>`                   |
  | A region           | `% badness-lint off <rule>` … `% badness-lint on <rule>` |
  | The whole file     | `% badness-lint skip-file <rule>: <reason>`              |

The `<rule>` is optional. Omitting it suppresses every rule over the same span.
An `off` with no matching `on` runs to the end of the file. The `: <reason>`
tail is optional everywhere and is never interpreted.

To require explanations for suppressions, add
`extend-select = ["unexplained-suppression"]` under `[lint]`. This opt-in rule
requires a nonempty `: <reason>` on `skip`, `off`, and `skip-file`, including
format-only and combined directives. Closing `on` directives need no reason.
Inline suppressions cannot hide this check once enabled.

Each has a bare counterpart that turns off the **formatter** at the same time:
`% badness skip`, `% badness off` / `% badness on`, and `% badness skip-file`.
For layout only, use the `% badness-format` spellings described in
[Formatting](formatting.md#turning-the-formatter-off).

In `.bib` files, use an `@comment` entry to suppress lint for an entire entry,
a region, or the whole file.

```bib
@comment{badness-lint skip missing-required-field: publisher long gone}
@book{oldbook, title = {An Orphaned Book}}
```

Inside a regular bibliography entry, use `% badness-lint skip` to suppress lint
for one field. A comment above a field applies to that field, and a comment at
the end of a field's line applies to the field on that line. A comment within
an assignment applies to that assignment. The formatter keeps the comment
with its field when it sorts fields.

```bib
@misc{example,
  title = {An Example},
  author = {Jane Doe},
  year = 2020,
  % badness-lint skip unknown-field: used by the bibliography exporter
  localid = {123},
  note = {}, % badness-lint skip empty-field: reserved for a later annotation
}
```

Use `@comment` entries for `off`, `on`, `skip-file`, and directives that also
affect formatting. The `inert-suppression` rule reports these directives when
they appear in a `%` comment inside an entry. It also reports a `skip` with no
target, an unmatched `on`, an unclosed `off`, and directives written as typeset
prose on a `.dtx` documentation line.

Some rules ship an **autofix**. `badness lint --fix` applies the
meaning-preserving (Safe) ones; `--unsafe-fixes` also applies fixes that may
change output, such as `missing-nonbreaking-space` (inserting a tie changes line
breaking), `abbreviation-spacing` (inserting `\` or `\@` changes sentence
spacing), or `space-before-command` (deleting a space before `\footnote` changes
spacing).

## Human-readable output

The default `pretty` output renders diagnostics with source snippets on stderr.
The snippets are colorized when stderr is a terminal; `--color always|never`
overrides that, and `NO_COLOR` is honored.

Use `--output concise` for plain, compact diagnostics on stderr or
`--output json` for structured output on stdout. Both stay plain regardless of
`--color`.

## Machine-readable output

`badness lint --output json` emits the findings as a JSON array on **stdout**
(the human-readable `pretty` and `concise` modes write to stderr). A clean run
emits `[]`, so consumers always receive valid JSON; the exit code still signals
whether findings exist. External tools such as Panache use this output to lint
`latex` code blocks in Markdown documents.

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

Ranges use zero-based byte offsets into the named file rather than line and
column numbers. `severity` is one of `error`, `warning`, `info`, or `hint`;
`applicability` is `safe` or `unsafe` (the `--fix`/`--unsafe-fixes` split). The
`fix` key is omitted when a finding has no autofix. An edit carries a `path` key
only when it targets a *different* file than the diagnostic (a cross-file fix);
`related` lists secondary "see also" locations.

The schema differs from those of arity and fatou in two ways: offsets are flat
`start`/`end` keys rather than a `range` object, and `message` is a plain string
rather than a structured object.
