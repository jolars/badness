# Configuration

Put a `badness.toml` file at the root of your project to share formatting and
linting settings between the CLI and language server. Every key is optional.
This guide covers common setup tasks; the [configuration
reference](../reference/configuration.md) lists all keys, defaults, and
resolution rules.

## Create a Project Config

Run this in a project that does not yet have a `badness.toml`:

```sh
badness init
```

The starter file contains commented settings. A small active configuration might
look like this:

```toml
extend-exclude = ["vendor/", "build/"]

[format]
line-width = 100
wrap = "sentence"

[lint]
ignore = ["dollar-display-math"]
```

Commit the file so contributors use the same settings. Add
`#:schema https://badness.dev/badness.schema.json` at the top for completion and
validation in editors that support TOML schema directives. See [editor
support](../reference/configuration.md#editor-support) for other ways to
associate the schema.

## Choose the Configuration Source

For each input file, Badness uses the nearest `badness.toml`, searching upward
to the repository root. If there is none, it uses `BADNESS_CONFIG` when set,
then the global user config. Badness uses each fallback as a complete
configuration file. It does not automatically merge project and user settings.

To choose a file explicitly or try the built-in defaults:

```sh
badness --config path/to/badness.toml format paper.tex
badness --no-config lint paper.tex
```

CLI options such as `--line-width` override individual settings for one run. See
[discovery](../reference/configuration.md#discovery) for global config locations
and language-server behavior.

## Share Settings Between Projects

Use `extend` to inherit another file and override the settings this project
needs:

```toml
extend = "../shared/badness.toml"

[format]
line-width = 100
```

The path is relative to the file that declares it. Tables merge by key, and the
project's values override inherited values. `extend-exclude` adds patterns to
inherited exclusions. See the [inheritance
reference](../reference/configuration.md#extend) for the other merge rules.

## Exclude Files

Use `extend-exclude` to add gitignore-style patterns without replacing the
built-in exclusions:

```toml
extend-exclude = ["vendor/", "generated/"]
```

Both the formatter and linter apply these patterns when walking directories.
They still process files named explicitly. Add `--force-exclude` when a runner
passes filenames that should respect the exclusions:

```sh
badness format --force-exclude generated/tables.tex
```

Use `exclude` to replace the default list entirely. For an exception within a
document, use the directives in the
[formatting](formatting.md#turning-the-formatter-off) and [linting](linting.md)
guides.

## Adjust Formatting and Linting

Use `[format]` for layout and `[lint]` for rule selection. The
[formatting](formatting.md) and [linting](linting.md) guides show the usual
workflows. The reference also covers [build
paths](../reference/configuration.md#build), [custom
commands](../reference/configuration.md#commands), and
[environments](../reference/configuration.md#environments) when a LaTeX project
needs additional context.

The [dprint plugin](integrations.md#dprint) uses `dprint.json` and does not load
`badness.toml`.
