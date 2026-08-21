# Incremental-reparse sweep baselines

What `parser::reparse` actually splices, over the same pinned corpora
`tests/gate_baselines/` uses. **Compare rows, not vibes** — every number here is
exact and reproducible, and any movement in either direction fails the check.
Run `task reparse-corpora:check`; re-record with `task reparse-corpora:record`.

## What is asserted, and what is recorded

Two different things, and the split is the point.

**Asserted, in `crates/badness-parser/tests/reparse_corpus_sweep.rs`:**

- The reparse invariant on every generated edit — a successful reparse yields a
  green tree and a `SyntaxError` vector byte-identical to a full parse of the
  edited text, and the tree round-trips losslessly. A divergence is a bug, so
  there is nothing here to record and nothing to accept. Nothing in this
  directory can baseline one away.
- A **splice-rate floor per driver per corpus**. Every invariant assertion is
  vacuously true on a refusal, so a guard that narrowed a tier to nothing would
  leave the sweep green while testing nothing — panache's window cutoff cost its
  fuzzer two thirds of its coverage with every assertion it carried still
  passing. The floors sit at roughly half the lowest recorded rate, so they
  survive an ordinary re-record and only a collapse trips them.

**Recorded, here:** the exact tallies. The floors are too coarse to see ordinary
movement, and one class of movement no floor can see at all — a workload
silently changing *tier*, since declining is always sound and a cheaper tier
taking work from a dearer one keeps every rate identical. The `token=` /
`verbatim=` / `math=` / `region=` columns are in the row for that reason.

## The rows

One `corpus` header row plus one row per driver, per corpus:

```
<corpus>  corpus    files=<n>  bytes=<n>
<corpus>  <driver>  spliced=<s>/<a>  token=<n>  verbatim=<n>  math=<n>  region=<n>  files=<n>
```

The header row pins the corpus itself: a pin bumped in
`scripts/fetch_gate_corpora.sh` without a re-record fails here, which is the
warning that script's header gives in prose.

`files=` on a driver row counts the files that offered it at least one edit. The
two site-seeking drivers skip a file with no candidate site, and a skipped file
is not a refusal — keeping the two apart is what lets the splice rate mean what
it says.

The seven drivers, each a workload with a tier it should reach:

  | driver              | what it does                                                     |
  | ------------------- | ---------------------------------------------------------------- |
  | `word-typing`       | five keystrokes inside a real `WORD`, at three sites per file    |
  | `word-deleting`     | five single-character deletions, at three sites of its own       |
  | `protected-typing`  | five keystrokes inside a `VERBATIM_BODY` or `VERB`               |
  | `math-word-typing`  | five partition-preserving keystrokes in an unscripted math word  |
  | `math-shape-typing` | five advancing keystrokes after a scripted math-word base        |
  | `hazard-single`     | 16 random single edits from the hazard alphabet                  |
  | `hazard-chain`      | five chains of 2–4 such edits, the shape a `didChange` batch has |

Seeds come from each file's **corpus-relative** path, so the tallies are a
property of the pinned corpora and not of where the repository lives.

## The corpora, and what the numbers say about them

Same four pins as `tests/gate_baselines/README.md`, which carries the table and
the note on why `latexindent` is read differently from the other three. Files
swept are `.tex`, `.sty`, `.cls`, `.dtx`, `.ins` — `.bib` is absent because the
reparse tiers splice the LaTeX tree and the bib parser has none of them. Each
file is parsed the way the CLI would parse it (`.sty`/`.cls`/`*.code.tex` under
an implicit `\makeatletter`, `.dtx` under the docstrip mode). The token and
protected tiers can splice `.dtx` only when their fragment proof reproduces the
relevant line, margin, macrocode, and implicit-expl state. The math tier remains
more conservative and declines `.dtx` because a delimiter-bearing node does not
carry the docstrip line/column context.

  | corpus      | word typing | math word | math shape | hazard single |
  | ----------- | ----------- | --------- | ---------- | ------------- |
  | latex3      | 73%         | 80%       | 3%         | 25%           |
  | latex2e     | 77%         | 66%       | 54%        | 28%           |
  | pgf         | 84%         | 90%       | 83%        | 39%           |
  | latexindent | 63%         | 42%       | 87%        | 22%           |

The math-shape rate ranges from 3% in dtx-heavy latex3 to 87% in latexindent.
The exact `math=` column distinguishes a real math-fragment splice from a token
splice at the same generated site. `region=` remains zero because none of these
drivers targets the region tier's multi-token prose and paragraph-seam workload.
