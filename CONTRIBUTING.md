# Contributing to Badness

Thanks for your interest in Badness, a formatter, linter, and language server for
LaTeX. This guide covers everything you need to build the project, run the tests,
and get a change merged. Contributions of all sizes are welcome, from typo fixes
to new lint rules and parser features.

## Getting set up

Badness is a single-crate Rust project (edition 2024). The toolchain is pinned by
`rust-toolchain.toml`, so a stable `rustup` install picks up the right version
automatically.

```sh
git clone https://github.com/jolars/badness
cd badness
cargo build
```

If you use [Nix](https://nixos.org/) with [devenv](https://devenv.sh/), the dev
shell provides the full toolchain plus the profiling and benchmarking tools
(`perf`, `cargo-flamegraph`, `hyperfine`, `cargo-show-asm`, `cargo-llvm-cov`) and
the `go-task` runner. It loads automatically with `direnv`.

The task runner is [go-task](https://taskfile.dev/); `task --list` shows every
available task. The most common ones are below, but every task maps to a plain
`cargo` invocation if you'd rather not install it.

## Building and testing

| Task | Equivalent | What it does |
| --- | --- | --- |
| `task build` | `cargo build` | Dev build. |
| `task test` | `cargo test` | Run the whole test suite. |
| `task fmt` | `cargo fmt` | Format the code. |
| `task lint` | `cargo clippy --all-targets --all-features -- -D warnings` | Clippy, warnings as errors. |
| `task check` | | Everything CI runs: `fmt-check`, `lint`, `test`. |

Run `task check` before opening a pull request; it mirrors CI exactly.

Badness uses [insta](https://insta.rs/) for snapshot tests. When a change
deliberately alters formatter or parser output, review and accept the new
snapshots with `task snapshots` (`cargo insta review`).

## Project layout

Badness parses LaTeX into a **lossless concrete syntax tree** and builds three
tools on top of it:

- a **formatter** (`badness format`) that lays out source deterministically,
- a **linter** (`badness lint`) that reports diagnostics, and
- a **language server** (`badness lsp`).

The architecture follows [rust-analyzer](https://rust-analyzer.github.io/):

- A hand-written, error-tolerant **lexer and parser** turn LaTeX into a flat token
  stream, then an **event stream** (`Start`/`Tok`/`Finish`), which a tree builder
  re-attaches trivia to and feeds into [rowan](https://github.com/rust-analyzer/rowan)
  to produce the lossless tree.
- A **semantic layer** (a signature database) assigns meaning (arity,
  verbatim-ness, sectioning) on top of the generic tree. Meaning never leaks into
  the parser.
- The **formatter** lowers the tree into a Wadler-style `Doc` IR, laid out under a
  flat/break fit model.
- Incremental recomputation is [salsa](https://github.com/salsa-rs/salsa)-first.

The source lives in one crate, organized into module folders: `parser/`,
`formatter/`, `linter/`, `semantic/`, `project/`, `text/`, plus `syntax.rs` and
`incremental.rs`.

## Invariants

Three properties are held by construction and enforced as test oracles. A change
that breaks any of them is a bug, not a trade-off:

- **Losslessness**: `reconstruct(text) == text`, byte-for-byte. The parser never
  loses or corrupts input.
- **Idempotence**: `format(format(x)) == format(x)`.
- **Protected regions**: verbatim-like content (`verbatim`, `lstlisting`,
  `\verb`, comments) is never altered by the formatter.

The formatter *may* normalize structure on purpose (for example, `x^{2}` becomes
`x^2`); it preserves meaning, not the exact parse tree.

A couple of ground rules keep the design coherent:

- Keep the syntactic layer free of semantic knowledge. Parsing is the parser's
  job; layout is the formatter's job.
- New parser features need corpus and snapshot tests **and** a losslessness
  assertion.

## Making a change

- Prefer trunk-based development and atomic commits. Branch first for substantial
  changes; small fixes can go straight to `main`.
- Follow [Conventional Commits](https://www.conventionalcommits.org/), for example
  `feat(linter): add missing-required-argument rule` or
  `fix(parser): recover at unbalanced brace`. The `CHANGELOG.md` is generated from
  the commit history by [versionary](https://github.com/jolars/versionary), so a
  clear, well-scoped commit message is what shows up in the release notes. Don't
  hand-edit `CHANGELOG.md`.
- Keep commit subjects short (imperative mood, ideally under 60 characters) and use
  the body for rationale. Close issues with `Fixes #123` in the body.
- A rustfmt git hook rewrites unformatted files and aborts the commit, so run
  `cargo fmt` first. Clippy warnings are treated as errors.

### Adding a lint rule

New lint rules implement the `Rule` trait, register in the rule list, ship unit
and integration tests with a losslessness-safe fix, and regenerate the rules
reference with `task docs:rules`. Look at the existing rules in
`src/linter/rules/` for the pattern.

## Documentation

User-facing docs are an [mdBook](https://rust-lang.github.io/mdBook/) under
`docs/`. Preview them locally with `task docs:serve` (live reload) or build them
with `task docs`. The linter-rules reference and the benchmark page are generated;
regenerate them with `task docs:rules` and `task bench` respectively rather than
editing the rendered pages by hand.

## A note on `AGENTS.md`

The repo also contains an `AGENTS.md` file. It is a detailed, decision-by-decision
record of the architecture aimed at AI coding agents. Humans are welcome to read
it for the deep rationale behind a design choice, but this file is the contributor
guide you should start from.

## License

By contributing, you agree that your contributions are licensed under the
project's [MIT License](LICENSE).
