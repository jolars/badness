# Getting Started

Start with [Installation](installation.md) if Badness is not yet available on
your `PATH`.

This page walks through formatting and linting your first file from the command
line. For editor integration, see [Editor Setup](editor-setup.md).

## Formatting a File

Format a file in place:

```sh
badness format paper.tex
```

See [Formatting](formatting.md) for multiple files and standard input.

## Checking Without Writing

Verify formatting without writing, for example in CI:

```sh
badness format --check paper.tex
```

The command prints a diff and exits with a nonzero status if any file needs
formatting. See [Checking Without
Writing](formatting.md#checking-without-writing) for reporting options.

## Linting

Report problems in your source:

```sh
badness lint paper.tex
```

The command exits with a nonzero status when it reports a diagnostic. See
[Linting](linting.md) for rules, fixes, and output formats.

## Adjusting Layout

See [Style Options](formatting.md#style-options) for command-line overrides and
[Configuration](configuration.md) for shared project settings. Continue with
[Integrations](integrations.md) to automate the checks.
