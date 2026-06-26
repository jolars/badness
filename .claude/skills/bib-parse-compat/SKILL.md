---
name: bib-parse-compat
description: Use when the user wants to check or analyze badness's BibTeX parse concordance against texlab — "run bib-parse-compat", "bib parse concordance vs texlab", "analyze bib divergences", or after a `.bib` parser/CST change. Runs the soft gauge and triages any unexplained divergence.
---

# bib-parse-compat

A **soft differential gauge** of badness's BibTeX CST against texlab's BibTeX CST
over `tests/bib_corpus/*.bib`. It is **not a quality gate**: per AGENTS.md we
*measure against texlab, never match it*. The skeleton stops at entry types and
field names; cite keys and value internals are intentionally dropped because the
two legitimately diverge there. Note texlab's bib parser has no error channel, so
this gauge never reports texlab parse errors.

## Run it

```sh
task bib-parse-compat
```

This runs `cargo test --test bib_parse_compat -- --ignored --nocapture` and
rewrites the report at `.claude/skills/bib-parse-compat/BIB_PARSE_COMPAT.md` (the
generated artifact next to this skill — do not hand-edit it).

## Analyze

1. Read the regenerated `BIB_PARSE_COMPAT.md`. The gauge currently sits at **100%
   concordance with 0 unexplained divergences**, so the usual job is "confirm it
   is still clean."
2. If "Unexplained divergences" is `0`, report that and stop.
3. For each unexplained divergence, classify it against
   `tests/bib_parse_compat_allowlist.toml`:
   - **Deliberate deviation** (badness is the faithful reading): add a
     `[deviations]` entry keyed by the corpus filename with a one-line reason;
     re-run to confirm.
   - **Genuine parser modeling gap:** fix it in the bib parser per tenet 3, with
     corpus + snapshot tests and a losslessness assertion. Do not allowlist it.
