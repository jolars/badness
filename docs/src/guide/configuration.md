# Configuration

Put a `badness.toml` file at the root of your project to share formatting and
linting settings between the CLI and language server. Every key is optional and
spelled in kebab-case. Badness reports an error for an unknown key or section.
This guide covers common setup tasks; the [configuration
reference](../reference/configuration.md) lists all keys, defaults, and accepted
values.

## Create a project config

Run this in a project that does not yet have a `badness.toml`:

```sh
badness init
```

The starter file contains commented defaults and examples. A small active
configuration might look like this:

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
support](#editor-support) for other ways to associate the schema.

## Choose the configuration source

For each input, Badness walks from the file's directory upward and uses the
first `badness.toml` it finds. The walk stops at a directory containing a `.git`
entry (the repository root), so a config file outside your repository is never
picked up.

If no project file is found, Badness next checks the `BADNESS_CONFIG`
environment variable. When set (and non-empty), it names a config file to use
instead of the global user config below—handy for keeping one config on a synced
drive and pointing every machine at it. A set `BADNESS_CONFIG` shadows the
global config entirely.

If `BADNESS_CONFIG` is unset, Badness falls back to a global user config: the
first existing file among

1. `$XDG_CONFIG_HOME/badness/config.toml`
2. `~/.config/badness/config.toml`
3. the platform config directory (`%APPDATA%\badness\config.toml` on Windows,
   `~/Library/Application Support/badness/config.toml` on macOS)

The `BADNESS_CONFIG` and global files use the same schema as a project
`badness.toml` and are whole-file fallbacks, never merged with a project config.
Relative `exclude` patterns in them resolve against the working directory (CLI)
or the document's directory (language server) rather than the config's own
directory. The language server also uses these files for editor-wide defaults
such as `wrap = "preserve"`. See [Editor
configuration](../reference/editor-configuration.md#supplying-settings) for
width precedence.

Two global CLI flags override discovery:

- `--config <PATH>` uses that file instead of discovering one.
- `--no-config` ignores any project, `BADNESS_CONFIG`, or global file and uses
  built-in defaults.

CLI flags for individual options (`--line-width`, `--wrap`, `--select`, etc.)
override the corresponding config values for a single run.

To choose a file explicitly or try the built-in defaults:

```sh
badness --config path/to/badness.toml format paper.tex
badness --no-config lint paper.tex
```

## Editor support

Badness publishes a [JSON Schema](https://json-schema.org/) for `badness.toml`
so editors with TOML support can provide key and value completion, hover
documentation, and validation.

**Schema URL:** <https://badness.dev/badness.schema.json>

The schema is generated from the configuration types and checked against them in
the test suite. The version at the URL tracks the latest released version of
Badness.

### VS Code (Even Better TOML)

With the [Even Better
TOML](https://marketplace.visualstudio.com/items?itemName=tamasfe.even-better-toml)
extension installed, add this association to your user or workspace
`settings.json`:

```jsonc
{
  "evenBetterToml.schema.associations": {
    "^(.*/)?badness\\.toml$": "https://badness.dev/badness.schema.json"
  }
}
```

### Inline `#:schema` directive

TOML tools that support inline schema directives can select the schema from the
configuration file itself:

```toml
#:schema https://badness.dev/badness.schema.json
```

This is also useful for the global user configuration, whose generic
`config.toml` file name should not be associated with Badness automatically.

### Other editors

Any editor or language server that consumes JSON Schemas—including Helix, Neovim
with `taplo-lsp`, Zed, and IntelliJ—can use the same URL through its TOML schema
settings.

## Share settings between projects

Use `extend` to inherit another file and override the settings this project
needs:

```toml
extend = "../shared/badness.toml"

[format]
line-width = 100
```

The path is relative to the file that declares it. Tables merge by key, and the
project's values override inherited values. `extend-exclude` adds patterns to
inherited exclusions, and `[lint] extend-select` adds to inherited rule lists.
See the [inheritance reference](../reference/configuration.md#extend) for the
other merge rules.

## Exclude files

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

## Adjust formatting and linting

Use `[format]` for layout and `[lint]` for rule selection. The
[formatting](formatting.md) and [linting](linting.md) guides show the usual
workflows. The reference also covers [build
paths](../reference/configuration.md#build), [custom
commands](../reference/configuration.md#commands), and
[environments](../reference/configuration.md#environments) when a LaTeX project
needs additional context.

The [dprint plugin](integrations.md#dprint) uses `dprint.json` and does not load
`badness.toml`.

Machine-specific settings, such as TEXMF-tree discovery and the PDF viewer,
belong in your editor's LSP settings rather than a shared `badness.toml`. See
[editor configuration](../reference/editor-configuration.md).
