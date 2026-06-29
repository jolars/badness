# Installation

Badness is distributed as a single binary, `badness`. The current version is
`{{ badness-version }}`.

## From Source

Badness is written in Rust (edition 2024). With a Rust toolchain installed,
build from a checkout:

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
