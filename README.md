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

**Badness** is a language server, formatter, and linter for LaTeX, built on a
lossless concrete syntax tree.

It parses LaTeX once and serves three tools from that tree:

- **Formatter** (`badness format`): deterministic, rule-based layout.
- **Linter** (`badness lint`): diagnostics with source snippets.
- **Language server** (`badness lsp`): both, live in your editor.

The architecture follows [rust-analyzer](https://rust-analyzer.github.io/): a
generic, error-tolerant, hand-written parser produces a lossless tree, semantics
are layered on top as a separate concern, and recomputation is incremental.
Badness never *requires* resolving macros or catcodes to succeed. Anything it
cannot statically recognize degrades to generic nodes rather than a crash. Two
properties hold by construction and are enforced as tests: losslessness (the
tree reconstructs the input byte-for-byte) and idempotence (formatting an
already formatted file changes nothing).

## Installation

Badness is available from several sources:

- **crates.io**: `cargo install badness`
- **npm**: `npm install -g badness` (bundles a prebuilt binary)
- **PyPI**: `uv tool install badness`/`pipx install badness`
- **Prebuilt binaries**: from the [releases
  page](https://github.com/jolars/badness/releases)
- **VS Code/Open VSX**: the [**Badness**
  extension](https://marketplace.visualstudio.com/items?itemName=jolars.badness)
  (also works in Positron and Cursor)
- **From source**: `cargo install --path .` in a checkout

If you prefer a one-liner installer that picks the right release artifact for
your platform, you can use the installer scripts below. These scripts are
fetched directly from this repository and then download the latest matching
Badness release asset for your platform, installing to a user-local directory by
default. If you prefer, download and inspect the script before running it.

For macOS and Linux:

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
    https://raw.githubusercontent.com/jolars/badness/refs/heads/main/scripts/badness-installer.sh | sh
```

For Windows PowerShell:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -Command "irm https://raw.githubusercontent.com/jolars/badness/refs/heads/main/scripts/badness-installer.ps1 | iex"
```

The VS Code/Open VSX extension bundles the `badness` binary and starts the
language server automatically when you open a `.tex` file.

## Usage

```sh
# Format a file in place (or stdin → stdout with no path)
badness format paper.tex

# Verify formatting without writing—exits non-zero if anything would change
badness format --check paper.tex

# Lint, reporting parse diagnostics
badness lint paper.tex

# Run the language server over stdio
badness lsp
```

Formatting is configurable via a TOML file named `badness.toml`. See the
documentation for the full reference.

The language server runs over stdio (`badness lsp`); see the [editor setup
guide](https://badness.dev/guide/editor-setup.html) for Neovim and VS Code
wiring.

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
