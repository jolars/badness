---
name: parse-compat
description: Use when the user wants to check or analyze badness's LaTeX parse concordance against texlab — "run parse-compat", "parse concordance vs texlab", "analyze parse-compat divergences", or after a parser/CST change that might shift the differential gauge. Runs the soft gauge and triages any unexplained divergence.
---

# parse-compat

A **soft differential gauge** of badness's generic CST against texlab's semantic
CST over `crates/badness-parser/tests/corpus/*.tex`. It is **not a quality gate**: per AGENTS.md we
*measure against texlab, never match it*. badness models TeX surface syntax;
texlab resolves semantics, so divergences are expected and either deliberate or a
real modeling gap.

## Run it

```sh
task parse-compat
```

This runs `cargo test -p badness-parser --test parse_compat -- --ignored --nocapture` and rewrites
the report at `.agents/skills/parse-compat/PARSE_COMPAT.md` (the generated
artifact next to this skill — do not hand-edit it).

For per-file skeleton diffs when a divergence is unclear:

```sh
PARSE_COMPAT_DUMP=1 task parse-compat   # writes target/parse_compat_diffs.txt
```

## Analyze

1. Read the regenerated `PARSE_COMPAT.md`. The headline numbers are skeleton
   similarity, file concordance, **intentional deviations**, and **unexplained
   divergences**.
2. **The number that matters is "Unexplained divergences."** If it is `0`, the
   gauge is clean — report that and stop.
3. For each unexplained divergence, classify it (the recorded reasons live in
   `crates/badness-parser/tests/parse_compat_allowlist.toml`; the human triage narrative is
   `docs/parse-compat-triage.md`):
   - **Deliberate deviation** (badness is the faithful surface reading, e.g.
     section/item scoping, subscript gluing, `\left…\right` isolation, verbatim
     opacity): add a `[deviations]` entry to
     `crates/badness-parser/tests/parse_compat_allowlist.toml` keyed by the corpus filename, with a
     one-line reason. Re-run `task parse-compat` to confirm it moves into
     "Recorded intentional deviations".
   - **Genuine parser modeling gap:** fix it in the parser per tenet 3 (parsing
     is the parser's job — never paper over it elsewhere), with corpus + snapshot
     tests and a losslessness assertion. Do not add it to the allowlist.

Default to skepticism: an allowlist entry is a claim that badness is *right* and
texlab diverges. If that is not clearly true, treat it as a parser gap.

## The gauge is declaration-blind

The harness parses each corpus file on its own, with no `badness.toml` in sight,
so a project's [declarations](../../../docs/src/reference/configuration.md)
(`[environments.…]`, AGENTS.md decision #12) never reach it. That is deliberate —
the gauge measures the *text-only* reading both parsers can be held to — and it
is why `declared_alias.tex` sits in the corpus as fully concordant plain
commands: the pairing it exists to exercise is asserted in
`tests/roundtrip.rs::roundtrip_declared_corpus_file` and
`tests/parser.rs::declared_alias_tree`, not here. A declaration-shaped change
should therefore move these numbers by nothing at all; if it does, something
reached the parse that should not have.

The *inferred* alias path is a different matter and does show up here
(`env_command_alias.tex`, and `one_sided_env_alias.tex` for the issue-#117
half-defined shapes), since it reads only the file's own definitions.
