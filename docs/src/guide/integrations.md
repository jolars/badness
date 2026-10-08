# Integrations

Use Badness in CI, Git hooks, and other formatting tools. For editor setup, see
[Editor Setup](editor-setup.md).

## GitHub Actions

Use [badness-action](https://github.com/jolars/badness-action) to check LaTeX
and BibTeX files in CI. It installs and caches a prebuilt Badness binary on
GitHub-hosted Linux, macOS, and Windows runners. Create
`.github/workflows/badness.yml`:

```yaml
name: Badness

on:
  pull_request:
  push:
    branches: [main]

permissions:
  contents: read

jobs:
  badness:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - uses: jolars/badness-action@v1
        with:
          version: v0.26.0
```

By default, the action runs `badness format --check` and `badness lint` from the
repository root, checking `.tex`, `.sty`, `.cls`, `.dtx`, `.ins`, and `.bib`
files without changing them. Formatting differences or lint findings fail the
check. It uses the CLI's [configuration
discovery](../reference/configuration.md); directory walks honor `.gitignore`
and the exclusions in `badness.toml`.

The `@v1` tag selects the action version; `version` selects the Badness CLI
release. Keep the latter aligned with your local installation and pre-commit
revision. Omitting `version` selects the latest release with a binary for the
runner.

For a formatting-only check under `chapters/`:

```yaml
- uses: jolars/badness-action@v1
  with:
    version: v0.26.0
    path: chapters/
    lint: "false"
```

Set `format: "false"` to run only linting, or `config: path/to/badness.toml` to
select a configuration file. `quiet: "true"` lists files needing formatting
without printing a diff. See the [action
reference](https://github.com/jolars/badness-action/tree/v1#inputs) for all
supported inputs and outputs.

## pre-commit

[badness-pre-commit](https://github.com/jolars/badness-pre-commit) runs Badness
on staged files. The hooks install a prebuilt binary wheel from PyPI, so no
separate Badness, Rust, or LaTeX installation is needed.

[Install pre-commit](https://pre-commit.com/#installation), then add this entry
to `.pre-commit-config.yaml`:

```yaml
repos:
  - repo: https://github.com/jolars/badness-pre-commit
    rev: v0.26.0
    hooks:
      - id: badness-lint
      - id: badness-format
```

Install the Git hook and run it once over all tracked files:

```sh
pre-commit install
pre-commit run --all-files
```

On subsequent commits, both hooks select staged `.tex`, `.sty`, `.cls`, `.dtx`,
`.ins`, and `.bib` files. `badness-lint` reports findings, and `badness-format`
formats files in place. When a hook changes a file, review and stage the
changes, then commit again.

To apply safe lint fixes, add `--fix` to the lint hook and keep it before
formatting:

```yaml
hooks:
  - id: badness-lint
    args: [--fix]
  - id: badness-format
```

To check formatting without changing files, add `args: [--check]` to
`badness-format`. Add `--quiet` alongside it to list files needing formatting
without printing a diff.

Both hooks pass `--force-exclude`, so `exclude` and `extend-exclude` in
`badness.toml` apply even though pre-commit supplies filenames explicitly. See
[file exclusions](../reference/configuration.md#exclude).

The `rev` selects the Badness release: `v0.26.0` installs Badness 0.26.0. Run
`pre-commit autoupdate` to update hook revisions, then review and commit the
changes to `.pre-commit-config.yaml`.

## dprint

Format LaTeX and BibTeX alongside other languages with
[dprint](https://dprint.dev) and
[dprint-plugin-badness](https://github.com/jolars/dprint-plugin-badness). The
plugin bundles Badness's formatter as WebAssembly, so no Badness CLI or LaTeX
installation is needed.

[Install dprint](https://dprint.dev/install/). If your project has no
`dprint.json`, create one with `dprint init`, then add Badness:

```sh
dprint config add jolars/badness
```

Commit the versioned, checksummed plugin URL that the command adds to
`dprint.json`. Run `dprint fmt` to format files in place or `dprint check` to
check without changing files. The check fails when files need formatting. Both
commands run all configured plugins; Badness handles `.tex`, `.sty`, `.cls`,
`.dtx`, `.ins`, and `.bib` files.

Add settings under `badness`, keeping the `plugins` array created above:

```json
{
  "badness": {
    "lineWidth": 80,
    "indentWidth": 2
  }
}
```

The plugin reads `dprint.json` and does not load `badness.toml`. Its keys use
camelCase; see the [plugin's configuration
reference](https://github.com/jolars/dprint-plugin-badness#configuration) for
the supported settings. Leaving `wrap` unset preserves the default for each file
kind: prose in `.tex` files reflows, while code files such as `.sty` and `.cls`
preserve authored line breaks.

Use dprint's top-level `includes` and `excludes` for file selection; Badness's
TOML exclusions do not apply. dprint also respects `.gitignore`. See [dprint's
configuration guide](https://dprint.dev/config/).

The plugin cannot read sibling `.sty` and `.cls` files to discover local command
signatures. A document that depends on those signatures can therefore format
differently from the CLI. The plugin provides formatting; use the CLI or
pre-commit for linting.

The plugin is released independently of the CLI. Run `dprint config update`,
then review and commit the configuration changes. For comparable output, use
releases with the same `badness-formatter` version and equivalent settings,
allowing for the difference in local package context.

## Using with Panache

[Panache](https://panache.bz) can format and lint LaTeX code blocks inside
Markdown and Quarto documents. Install both CLIs on your `PATH`, then add this
to the document project's `panache.toml`:

```toml
[formatters]
latex = "badness"
bib = "badness"

[linters]
latex = "badness"
tex = "badness"
```

The formatter covers `latex`/`tex` and `bib`/`bibtex` blocks. The linter preset
covers `latex`/`tex` blocks; it does not lint BibTeX blocks.

Run `panache format document.qmd` to format the document and its code blocks, or
`panache lint document.qmd` to report findings without changing files. The
linter also runs through Panache's language server. To apply safe fixes before
formatting, run `panache lint --fix document.qmd`, then format the document.

Panache's Badness linter preset passes `--no-config`, so it does not load
`badness.toml` and uses Badness's default lint configuration. The formatter
preset runs separately and retains its CLI configuration behavior. See Panache's
[formatter preset](https://panache.bz/reference/formatter-presets.html#badness),
[linter preset](https://panache.bz/reference/linter-presets.html#badness), and
[external-tool
configuration](https://panache.bz/guide/configuration.html#external-code-linters).
