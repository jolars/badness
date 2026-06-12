# badness

[Badness](https://jolars.github.io/badness/) is an LSP, formatter, and linter
for LaTeX documents.

## Install

```sh
npm install -g badness
```

This installs the `badness` command globally. The package detects your platform
at install time and pulls in a prebuilt binary via npm's optional dependencies
--- no Rust toolchain or postinstall download required.

You can also use it without a global install:

```sh
npx badness format document.tex
```

## Usage

```sh
badness format document.tex     # format in place
badness format <document.tex    # read stdin, write stdout
badness lint document.tex       # lint
badness lint --fix document.tex # lint and apply auto-fixes
badness lsp                     # start the language server
```

See `badness --help` and the [documentation](https://jolars.github.io/badness/)
for the full feature list and configuration reference.

## Supported platforms

Prebuilt binaries are shipped for:

- Linux x64 (glibc and musl)
- Linux arm64 (glibc and musl)
- macOS x64 (Intel) and arm64 (Apple Silicon)
- Windows x64 and arm64

If your platform isn't covered, install via
[Cargo](https://crates.io/crates/badness),
[PyPI](https://pypi.org/project/badness/), or one of the other methods listed at
<https://jolars.github.io/badness/>.

## License

MIT --- see [LICENSE](./LICENSE).
