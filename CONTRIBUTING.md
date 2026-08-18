# Contributing to Badness

Thanks for your interest in Badness, a formatter, linter, and language server
for LaTeX. This guide covers everything you need to build the project, run the
tests, and get a change merged. Contributions of all sizes are welcome, from
typo fixes to new lint rules and parser features.

## Getting set up

Badness is a Rust workspace (edition 2024): the root package is the `badness`
CLI/LSP/linter crate, and the publishable `badness-parser` and
`badness-formatter` library crates live under `crates/`. The toolchain is pinned
by `rust-toolchain.toml`, so a stable `rustup` install picks up the right
version automatically.

```sh
git clone https://github.com/jolars/badness
cd badness
cargo build
```

If you use [Nix](https://nixos.org/) with [devenv](https://devenv.sh/), the dev
shell provides the full toolchain plus the profiling and benchmarking tools
(`perf`, `cargo-flamegraph`, `hyperfine`, `cargo-show-asm`, `cargo-llvm-cov`)
and the `go-task` runner. It loads automatically with `direnv`.

The task runner is [go-task](https://taskfile.dev/); `task --list` shows every
available task. The most common ones are below, but every task maps to a plain
`cargo` invocation if you'd rather not install it.

## Building and testing

  | Task         | Equivalent                                                 | What it does                                             |
  | ------------ | ---------------------------------------------------------- | -------------------------------------------------------- |
  | `task build` | `cargo build`                                              | Dev build.                                               |
  | `task test`  | `cargo test`                                               | Run the whole test suite.                                |
  | `task fmt`   | `cargo fmt`                                                | Format the code.                                         |
  | `task lint`  | `cargo clippy --all-targets --all-features -- -D warnings` | Clippy, warnings as errors.                              |
  | `task check` |                                                            | Everything CI runs: `fmt-check`, `lint`, `test`, `wasm`. |

Run `task check` before opening a pull request; it mirrors CI exactly.

Badness uses [insta](https://insta.rs/) for snapshot tests. When a change
deliberately alters formatter or parser output, refresh snapshots with
`task snapshots` and review the diff before committing.

Performance is first-class. Benchmark before optimizing, and never regress
losslessness for speed.

### Checks that don't run in CI

Two oracles need more than a Rust toolchain, so run them by hand when your
change touches what they cover.

`task typeset:check` compiles `tests/typeset/*.tex` before and after formatting
and diffs the typeset output. The CST oracles cannot see the one risk the
key-value argument flag takes, where a space token is trivia to the CST and
content to TeX, so run this when touching keyval signature data or the
optional-argument lowering. It needs a TeX install.

`task parse-compat` runs [texlab](https://github.com/latex-lsp/texlab)'s parser
as a differential oracle over a corpus, skeletonizing both trees and comparing.
It is a reference we measure against, not one we match, so a divergence is
something to explain rather than automatically fix.

## Project layout

Badness parses LaTeX into a lossless concrete syntax tree (CST) and builds three
tools on top of it: a formatter (`badness format`), a linter (`badness lint`),
and a language server (`badness lsp`). The architecture follows
[rust-analyzer](https://rust-analyzer.github.io/): a hand-written,
error-tolerant lexer and parser turn LaTeX into a flat token stream, then an
event stream that a tree builder feeds into
[rowan](https://github.com/rust-analyzer/rowan); a **semantic layer** assigns
meaning on top of the generic tree; and incremental recomputation is
[salsa](https://github.com/salsa-rs/salsa)-first.

The [Architecture](https://badness.dev/development/architecture.html) page in
the book is the full tour, and it is worth reading before a non-trivial change.

Where things live:

- `crates/badness-parser` — syntax layer, parser, semantic layer, the BibTeX
  pipeline, and the `data/` signature artifacts.
- `crates/badness-formatter` — the layout engine and the `.bib` formatter.
- `crates/badness-wasm` — the wasm shim powering the docs playground.
- `src/` — the CLI, LSP, linter, and project layers, plus shim modules
  re-exporting the member crates at their old paths.

Both library crates must keep building for `wasm32-unknown-unknown`, so nothing
in them may touch the filesystem, threads, or processes. A CI job guards this.
Anything that needs the outside world belongs in the root crate.

## Invariants

These properties are held by construction and enforced as test oracles. A change
that breaks one is a bug, not a trade-off.

- Losslessness: `reconstruct(text) == text`, byte for byte.
- Idempotence: `format(format(x)) == format(x)`.
- The formatter is whitespace-only: it changes trivia (whitespace, newlines,
  comments, `.dtx` margins and guards) and nothing else. Content rewrites such
  as `x^{2}` → `x^2` are linter autofixes, not layout.
- Protected regions: verbatim-like content (`verbatim`, `lstlisting`, `\verb`,
  comments) is never altered by the formatter, apart from a document-wide
  line-terminator normalization.

A couple of ground rules keep the design coherent:

- Semantic facts reach the parser only through a narrow, curated admission test;
  when in doubt, a fact belongs in the semantic layer. Parsing is the parser's
  job; layout is the formatter's job. Never paper over a parser mistake in the
  formatter.
- New parser features need corpus and snapshot tests **and** a losslessness
  assertion.

## Making a change

- Prefer trunk-based development and atomic commits. Branch first for
  substantial changes; small fixes can go straight to `main`.
- Follow [Conventional Commits](https://www.conventionalcommits.org/), for
  example `feat(linter): add missing-required-argument rule` or
  `fix(parser): recover at unbalanced brace`. The `CHANGELOG.md` is generated
  from the commit history by [versionary](https://github.com/jolars/versionary),
  so a clear, well-scoped commit message is what shows up in the release notes.
  Don't hand-edit `CHANGELOG.md`.
- Keep commit subjects short (imperative mood, ideally under 60 characters) and
  use the body for rationale. Close issues with `Fixes #123` in the body.
- A rustfmt git hook rewrites unformatted files and aborts the commit, so run
  `cargo fmt` first. Clippy warnings are treated as errors.

Each workspace crate is its own versionary package with its own changelog and
version. The root CLI tags bare `v*`; the members tag `badness-parser-v*` and
`badness-formatter-v*`. Only the bare `v*` stream carries release assets.

### Adding a lint rule

The `add-lint-rule` workflow automates this, but the shape is fixed:

1. Implement `Rule` in a new `src/linter/rules/<name>.rs`, choosing node-shape,
   whole-file, or streaming dispatch, with an `id`, a `default_severity`, a
   description, and at least one triggering example. Emit a losslessness-safe
   fix where one is warranted, and set `emits_fix` accordingly.
2. Register it in the three lockstep lists in `src/linter/rules.rs`: the module
   declaration, the re-export, and the entry in `all_rules()`.
3. Ship unit tests next to the rule and an integration test, plus a losslessness
   assertion on any fixture.
4. Regenerate the rules reference with `task docs:rules`. Do not edit the
   rendered page by hand.

### Generated data files

Several files in `crates/badness-parser/data/` are generated from pinned
upstream sources by `scripts/gen_*.py` and guarded by paired `task …:check` and
`:sync` targets: `cwl_signatures.json`, the package and class name lists with
`package_metadata.json`, and `bib_fields.json`. Re-sync them through their task
rather than hand-editing the mechanical facts. `signatures.json`, `colors.json`,
and `tikz_libraries.json` are curated by hand and may be edited directly.

### Windows CI bites twice

Line endings: the formatter emits LF and tests compare bytes against checked-in
fixtures. When you add a fixture in a new extension under
`crates/*/tests/fixtures/**` or `crates/badness-parser/tests/corpus/**`, add a
matching `… eol=lf` line to `.gitattributes`. Never normalize line endings in
code to pass a test; fix the attribute instead.

URIs: decode LSP URIs to filesystem paths only through `uri_to_fs_path` and
`path_to_uri` in `lsp.rs`. Tests and snapshots must not assume `/` versus `\`.

## Documentation

User-facing docs are an [mdBook](https://rust-lang.github.io/mdBook/) under
`docs/`. Preview them locally with `task docs:serve` (live reload) or build them
with `task docs`. The linter-rules reference and the benchmark page are
generated; regenerate them with `task docs:rules` and `task bench` respectively
rather than editing the rendered pages by hand.

## A note on `AGENTS.md`

The repo's `AGENTS.md` is the operational contract for AI coding agents. It
includes both repository-wide and subsystem-specific directives and is kept
under 32 KiB so agents can load it as a single checklist of things not to break.
For architectural rationale and tradeoffs, read the book's
[Architecture](https://badness.dev/development/architecture.html) page.

## License

By contributing, you agree that your contributions are licensed under the
project's [MIT License](https://github.com/jolars/badness/blob/main/LICENSE).
