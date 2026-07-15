# Pre-commit

Badness ships a [pre-commit](https://pre-commit.com) hook through the
[badness-pre-commit](https://github.com/jolars/badness-pre-commit) mirror
repository. The hook installs the prebuilt `badness` wheel from PyPI, so no Rust
toolchain is needed. Add this to your `.pre-commit-config.yaml`:

```yaml
repos:
  - repo: https://github.com/jolars/badness-pre-commit
    # badness version
    rev: v0.9.0
    hooks:
      # Lint .tex, .sty, .cls, .dtx, .ins, and .bib files
      - id: badness-lint
      # Format the same files in place
      - id: badness-format
```

Tags mirror badness releases: `rev: v0.9.0` runs badness 0.9.0.

To apply safe lint autofixes before formatting (the fix-then-format pipeline),
pass `--fix`:

```yaml
- id: badness-lint
  args: [--fix]
- id: badness-format
```

To check formatting without rewriting files:

```yaml
- id: badness-format
  args: [--check]
```
