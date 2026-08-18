# Badness <picture><source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/jolars/badness/main/branding/logo-dark.svg"><img src="https://raw.githubusercontent.com/jolars/badness/main/branding/logo.svg" align="right" width="120" alt="" /></picture>

[![Build and
Test](https://github.com/jolars/badness/actions/workflows/build-and-test.yml/badge.svg?branch=main)](https://github.com/jolars/badness/actions/workflows/build-and-test.yml)
[![Crates.io](https://img.shields.io/crates/v/badness.svg?logo=rust)](https://crates.io/crates/badness)
[![Open
VSX](https://img.shields.io/open-vsx/v/jolars/badness?logo=vsix)](https://open-vsx.org/extension/jolars/badness)
[![VS
Code](https://vsmarketplacebadges.dev/version-short/jolars.badness.svg?logo=vsix)](https://marketplace.visualstudio.com/items?itemName=jolars.badness)
[![PyPI
version](https://badge.fury.io/py/badness.svg?icon=si%3Apython)](https://pypi.org/project/badness/)
[![npm
version](https://badge.fury.io/js/@badness%2Fbadness.svg?icon=si%3Anpm)](https://www.npmjs.com/package/badness)

Badness is a language server, formatter, and linter for LaTeX. It is designed to
be fast, robust, and memory efficient. It bundles three tools in one:

- **Formatter** (`badness format`): opinionated, deterministic, and rule-based
  layout.
- **Linter** (`badness lint`): syntax errors and best practices.
- **Language server** (`badness lsp`): both of the above, plus information on
  hovering, symbol outlines, go-to-definitions, code actions, and much more.

The architecture is modeled after
[rust-analyzer](https://rust-analyzer.github.io/), relying on a incremental
parser that forms a full concrete syntax tree of the document, and then using
that tree to provide formatting, linting, and language server features. It is
designed to used both inside your editor and on the command line, and is fast
enough to provide real-time analysis after every keystroke and formatting on
save, even for large documents and complex projects.

The audience for Badness is both authors who write LaTeX documents (`.tex` and
`.bib` files) and developers who write LaTeX packages (`.sty`, `.cls`, `.dtx`,
and `.ins` files), and provides support both for the newer LaTeX3 programming
layer and the older LaTeX2e layer.

## Installation

Badness is available from several sources:

- **crates.io**: `cargo install badness`
- **Homebrew**: `brew install jolars/tap/badness`
- **npm**: `npm install -g badness` (bundles a prebuilt binary)
- **PyPI**: `uv tool install badness`/`pipx install badness`
- **Aqua**: `aqua install jolars/badness`
- **Prebuilt binaries**: from the [releases
  page](https://github.com/jolars/badness/releases)
- **VS Code/Open VSX**: the [**Badness**
  extension](https://marketplace.visualstudio.com/items?itemName=jolars.badness)
  (also works in Positron and Cursor)
- **NixOS**: the `badness` package on
  [Nixpkgs](https://search.nixos.org/packages?channel=unstable&show=badness&from=0&size=50&sort=relevance&type=packages)
- **From source**: `cargo install --path .` in a checkout

### Installation scripts

If you prefer a one-liner installer that picks the right binary for your
platform, you can use the installer scripts below.

For macOS and Linux:

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
    https://github.com/jolars/badness/releases/latest/download/badness-installer.sh | sh
```

For Windows PowerShell:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -Command "irm https://github.com/jolars/badness/releases/latest/download/badness-installer.ps1 | iex"
```

The VS Code/Open VSX extension bundles the `badness` binary and starts the
language server automatically when you open a `.tex` file.

## Usage

```sh
# Format a file in place (or `badness format -` for stdin → stdout)
badness format paper.tex

# Verify formatting without writing, showing diffs
badness format --check bibliography.bib

# Lint, reporting parse diagnostics
badness lint paper.tex

# Fix lint issues in place
badness lint --fix paper.tex
```

Formatting is configurable via a TOML file named `badness.toml`. See the
documentation for the full reference.

## Editor Integration

The language server runs over stdio (`badness lsp`); see the [editor setup
guide](https://badness.dev/guide/editor-setup.html) for instructions on how to
integrate with your editor.

## Pre-Commit Hook

[badness-pre-commit](https://github.com/jolars/badness-pre-commit) provides
[pre-commit](https://pre-commit.com) hooks for linting and formatting. It
installs a prebuilt binary wheel from PyPI, so no Rust toolchain or LaTeX
distribution is required:

```yaml
repos:
  - repo: https://github.com/jolars/badness-pre-commit
    # badness version
    rev: v0.11.0
    hooks:
      # Lint .tex, .sty, .cls, .dtx, .ins, and .bib files
      - id: badness-lint
      # Format the same files in place
      - id: badness-format
```

## GitHub Actions

[badness-action](https://github.com/jolars/badness-action) installs badness and
runs format and lint checks in CI:

```yaml
jobs:
  badness:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - uses: jolars/badness-action@v1
```

## Documentation

See <https://badness.dev/> for the full documentation, including a user guide,
reference, and developer guide.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md).

## License

[MIT](LICENSE)
