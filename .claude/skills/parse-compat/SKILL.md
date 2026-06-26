---
name: parse-compat
description: Use when the user wants to check or analyze badness's LaTeX parse concordance against texlab — "run parse-compat", "parse concordance vs texlab", "analyze parse-compat divergences", or after a parser/CST change that might shift the differential gauge. Runs the soft gauge and triages any unexplained divergence.
---

# parse-compat

A **soft differential gauge** of badness's generic CST against texlab's semantic
CST over `tests/corpus/*.tex`. It is **not a quality gate**: per AGENTS.md we
*measure against texlab, never match it*. badness models TeX surface syntax;
texlab resolves semantics, so divergences are expected and either deliberate or a
real modeling gap.

## Run it

```sh
task parse-compat
```

This runs `cargo test --test parse_compat -- --ignored --nocapture` and rewrites
the report at `.claude/skills/parse-compat/PARSE_COMPAT.md` (the generated
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
   `tests/parse_compat_allowlist.toml`; the human triage narrative is
   `docs/parse-compat-triage.md`):
   - **Deliberate deviation** (badness is the faithful surface reading, e.g.
     section/item scoping, subscript gluing, `\left…\right` isolation, verbatim
     opacity): add a `[deviations]` entry to
     `tests/parse_compat_allowlist.toml` keyed by the corpus filename, with a
     one-line reason. Re-run `task parse-compat` to confirm it moves into
     "Recorded intentional deviations".
   - **Genuine parser modeling gap:** fix it in the parser per tenet 3 (parsing
     is the parser's job — never paper over it elsewhere), with corpus + snapshot
     tests and a losslessness assertion. Do not add it to the allowlist.

Default to skepticism: an allowlist entry is a claim that badness is *right* and
texlab diverges. If that is not clearly true, treat it as a parser gap.
