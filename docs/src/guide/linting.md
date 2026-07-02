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
diagnostics carry the rule id `parse` and are never silenced by `select`/`ignore`.

## Rules

Beyond parse recovery, badness ships a growing set of built-in rules
(`deprecated-command`, `dollar-display-math`, `undefined-ref`, and more). Each
has a stable id used in diagnostics, config, and suppression comments. See the
[Linter Rules](../reference/linter-rules.md) reference for the full catalogue, or
print a single rule's description and examples from the terminal:

```sh
badness lint --explain deprecated-command
```

Every rule is on by default. Narrow the active set through the `[lint]` table or
the matching CLI flags:

```toml
[lint]
select = ["deprecated-command", "dollar-display-math"]  # allowlist
ignore = ["missing-nonbreaking-space"]                  # turn off
```

Suppress a rule at one site with a comment directive:

```tex
% badness-ignore deprecated-command: legacy code
{\bf here}
```

Some rules ship an **auto-fix**. `badness lint --fix` applies the meaning-preserving
(Safe) ones; `--unsafe-fixes` also applies fixes that may change output, such as
`missing-nonbreaking-space` (inserting a tie changes line breaking) or
`abbreviation-spacing` (inserting `\ ` or `\@` changes sentence spacing).
