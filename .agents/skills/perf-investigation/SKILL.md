---
name: perf-investigation
description: >-
  Investigate Badness performance on real LaTeX or BibTeX workloads: find
  parser, formatter, linter, or language-server hotspots, explain latency or
  memory growth, and verify performance improvements. Use for profiling,
  slowness reports, and performance regressions. Use bench for refreshing
  published benchmark comparisons and smoke-test-triage for corpus failures.
---

# Investigate Badness performance

Measure the path the user cares about, identify the work responsible, and verify
any improvement without changing behavior. An investigation may end with an
explanation and measurements; implement optimizations when the request includes
them.

## Choose the measurement

Read the relevant subsystem instructions in `AGENTS.md`, then search
`TODO.md`'s performance notes for the reported symptom. Use
`benches/README.md` for harness context, but verify its historical findings
against the current harness and input.

| Reported cost | Starting point | Limit of the measurement |
| --- | --- | --- |
| LaTeX parsing or formatting | `task bench:micro` | Separates parse, format from a built CST, and full formatting; excludes CLI startup. |
| CLI latency, linting, or BibTeX | Release CLI on the actual file or project | Includes discovery, configuration, parsing, and output rendering. |
| Typing in the editor | `task bench:keystroke` | Splice, salsa upsert, edit staging, and parse; excludes diagnostics and request transport. |
| Incremental parser tiers | `task bench:reparse` | Direct reparse and full-parse comparison with explicit tier checks. |
| Warm LSP requests or process memory | External LSP harness | Request latency and process-tree RSS/PSS after a defined session. |
| Heap retained across edits or discovery | `task bench:lsp-memory` | Paired histories ending in the same state; live allocations, not RSS. |

Read [references/harness.md](references/harness.md) before timing or recording.
The LaTeX micro-bench has no lint or BibTeX mode. Formatting from a built CST
does not measure the entire LSP formatting request. For other editor symptoms,
reproduce the actual request or diagnostics path in a persistent server.

## Investigate and improve

1. Reproduce on representative input with the reported configuration. Record
   revision, build settings, workload, machine, and baseline. Separate cold
   startup from repeated work before choosing a target.
2. Profile the relevant phase. Attribute allocator and rowan leaves to a caller
   or allocation site before choosing a change. Read
   [references/hotspots.md](references/hotspots.md) for Badness-specific leads.
3. If optimizing, make one measured change at a time. Add a failing regression
   test first for a bug; cover the admitting and rejecting cases of a new fast
   path. Preserve formatter bytes, CST reconstruction, parser errors, lint
   findings and fixes, and incremental equivalence.
4. Re-profile, then compare baseline and candidate on the same production path.
   A symbol disappearing from a profile is insufficient evidence of a speedup.
   For memory work, compare equivalent end states and check latency too.
5. Remove only the investigation's own changes that do not improve the target
   metric beyond measured variability. Keep any useful diagnosis even if no
   optimization pays off.
6. Follow [references/verification.md](references/verification.md) before
   handing off a code change.

Keep parser, formatter, linter, and project responsibilities intact. In
particular, do not move work out of a measured phase, weaken reparse locality
proofs, or change cache lifetime just to improve a benchmark. Parser and
formatter runtime code must remain wasm-compatible.

## Report

State the workload and production path, the measured hotspot and attribution,
what changed, and the before/after metric with run count and variability.
Include correctness checks, experiments removed, and any measurement limits.
Suggest the next hotspot only when the profile supports it.

Keep investigation artifacts under `target/perf-investigation/` or a temporary
directory. Refresh published benchmark JSON only when that is part of the
request, using the [bench skill](../bench/SKILL.md). Keep durable debugging
findings in `TODO.md` and architectural rationale in
`docs/src/development/architecture.md` when an update is warranted.
