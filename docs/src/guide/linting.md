# Linting

`badness lint` parses each file and reports diagnostics, rendered with source
snippets pointing at the offending range. It exits non-zero when there is at
least one diagnostic, which makes it usable as a CI gate.

```sh
badness lint paper.tex
cat paper.tex | badness lint   # stdin
```

## What it reports today

badness is early, and the linter currently surfaces **parse diagnostics**:
places where the parser recovered from malformed input. Because the parser is
error-tolerant, a single problem never aborts the parse—badness anchors recovery
on clean LaTeX boundaries (`\end{…}`, `\begin`, a blank line, `}`, `$`, `&`,
`\\`) and keeps going, so one file can report several independent diagnostics in
one run.

Rule-based lints beyond parse recovery will grow over time; see the
[Changelog](../changelog.md).
