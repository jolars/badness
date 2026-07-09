# Installation

Badness is distributed as a single binary, `badness`. The current version is
`{{ badness-version }}`. It is available from several sources:

- **crates.io**: `cargo install badness`
- **npm**: `npm install -g badness` (bundles a prebuilt binary)
- **PyPI**: `uv tool install badness`/`pipx install badness`
- **Prebuilt binaries**: from the [releases
  page](https://github.com/jolars/badness/releases)
- **VS Code/Open VSX**: the
  [**Badness**](https://marketplace.visualstudio.com/items?itemName=jolars.badness)
  extension (also on [Open VSX](https://open-vsx.org/extension/jolars/badness);
  works in Positron and Cursor)

The editor extension bundles a platform-specific `badness` binary and starts the
language server automatically, so no separate CLI install is required. See
[Editor Setup](editor-setup.md) for configuration.

## From Source

Badness is written in Rust. With a Rust toolchain installed, build from a
checkout:

```sh
git clone https://github.com/jolars/badness
cd badness
cargo build --release
```

The binary lands at `target/release/badness`. Copy it onto your `PATH`, or run
it in place.

To install it into Cargo's bin directory instead:

```sh
cargo install --path .
```

## Verifying the Install

```sh
badness --version
```

This should print `badness {{ badness-version }}`.
