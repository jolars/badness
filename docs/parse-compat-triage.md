# Parse-compat triage — vetting badness's CST shape against texlab

Companion to the generated `PARSE_COMPAT.md` (do not hand-edit that file). This
is a human-authored triage of the divergences surfaced by expanding the
parse-compat corpus from 4 to 22 files, recording how each was classified and
why. It is the "decide" input for the genuine modeling gap found — no parser
changes were made in this pass.

Reproduce the per-file skeleton diffs with:

```sh
PARSE_COMPAT_DUMP=1 task parse-compat   # writes target/parse_compat_diffs.txt
```

## Summary

  | Metric                        | Before (4 files) | After (22 files)                            |
  | ----------------------------- | ---------------- | ------------------------------------------- |
  | Skeleton similarity           | 81.7%            | 79.5%                                       |
  | File concordance              | 25.0% (1/4)      | 27.3% (6/22)                                |
  | Intentional deviations        | 3                | 16                                          |
  | Unexplained divergences       | 0                | **0** (the one gap found was fixed — below) |
  | Skipped (badness parse error) | 0                | 1 (`malformed.tex`, expected)               |

18 new probes were added (`tests/corpus/*.tex`), hand-authored MIT-clean and
informed by texlab's own \~150-case parser test suite used as a *category
checklist* (texlab is GPLv3; no snippets were copied). The drop in headline
similarity is expected and healthy: the new files exercise constructs that
trigger texlab's semantic scoping, which badness deliberately does not model.

**Headline:** badness's CST shape held up well. Of 13 divergences, 12 are
deliberate and correct (and in several the *more* faithful surface reading); 1
was a genuine shape inconsistency, **now fixed** (below). The single skipped
file is correct error handling.

## The one genuine modeling gap — fixed in this pass

### Verbatim-argument commands emitted the `VERB` body as a *sibling*, not a child

Originally, for a verbatim-argument command like `\url{…}` or `\lstinline{…}`,
badness lexed the control word and the verbatim body as two flat tokens, and the
parser's `attach_arguments` (which only attached `{…}`/`[…]`) left the `VERB`
token unattached, so it landed as a *sibling* of the `COMMAND` — an empty
`COMMAND` node with the body floating beside it. That was inconsistent with
badness's own greedy-attachment model (AGENTS.md decision #8: trailing arguments
attach as *children*), and meant a downstream consumer could not ask "what is
`\url`'s argument?".

**Fix applied** (`src/parser/grammar.rs`): `attach_arguments` now also attaches
a directly-following `VERB` token into the open node, so the verbatim body nests
under the command after any leading `{…}`/`[…]` args (matching the lexer's
`lex_verbatim_command` order: control word → leading args → `VERB`):

```
$ printf '\\url{http://a.b?q=1&r=2}' | badness parse
PARAGRAPH
  COMMAND@0..24
    CONTROL_WORD "\url"
    VERB@4..24 "{http://a.b?q=1&r=2}"   <- now a CHILD of COMMAND
```

For `\mintinline{lang}{code}` both the `{lang}` group and the `{code}` `VERB`
now nest under the command, which spans the whole construct. A *standalone*
`\verb…`/`\verb*…` token is excluded from capture by guarding on its text (it
starts with `\`, whereas a verbatim *argument* `VERB` starts with `{` or a
delimiter); only `lex_verbatim_command` emits a non-`\` `VERB`, always
immediately after its own command, so the open node provably owns it. New parser
tests cover the brace-argument child, the leading-arg case, and the adversarial
`\foo \verb|x|` (verb stays a sibling); the existing `tree()` helper enforces
losslessness on every input.

After the fix, `verbatim_cmd.tex` is allowlisted (no longer unexplained):
badness and texlab now agree the verbatim body is the command's *child*; the
residual divergence is deliberate — badness keeps the body opaque (`(verbatim)`)
where texlab parses it as a `(group)`, plus the `\verb`/`\verb*` single-token vs
command+verbatim tokenization split.

## The skipped file is correct behavior

`malformed.tex` is `SkippedBadness` — by design the gauge never measures against
its own parse errors. badness correctly diagnosed all three malformations and
recovered losslessly without aborting (tenet 5, error recovery):

```
unclosed `[`                      (\cmd[option …)
`\end` without matching `\begin`  (\end{orphan})
unclosed `$`                      ($x + y <eol>)
```

The unmatched `{` recovered into a `GROUP`, the unclosed `[` into an `OPTIONAL`,
and the dangling `$` into a `MATH` region. This is a positive result, not a gap
— keep the file as a standing error-recovery probe.

## Deliberate deviations (recorded in the allowlist)

All 12 new allowlisted files reduce to a few recurring, already-sanctioned
patterns.

- **Semantic scope nesting (the dominant pattern).** texlab opens a scope for
  sectioning commands and `\item`, nesting following siblings inside; badness
  keeps them flat generic `COMMAND`s (tenet 2: meaning never leaks into the
  parser). This *alone* accounts for the entire divergence in `sectioning.tex`,
  `citations.tex`, `comments_trivia.tex`, `nested_envs.tex`, and
  `optional_args.tex` — in each, the constructs the probe was actually targeting
  (citations, optional/bracketed args, trivia attachment) are **concordant**;
  only the scope nesting differs.
- **Control-symbol argument attachment** (`accents.tex`): `\"{o}` keeps `{o}` a
  sibling because greedy attachment fires on control *words*, not symbols. Same
  as the pre-existing `edge.tex` deviation.
- **`\def`not signature-special-cased** (`newcommand.tex`): only `\def\foo{bar}`
  diverges;
  `\newcommand`/`\renewcommand`/`\DeclareMathOperator`/`\newenvironment` are
  concordant.
- **`\\`line-break modeling** (`display_math.tex`): texlab manufactures empty
  `(opt)` arg nodes for `\\`; badness's `LINE_BREAK` is cleaner.

In several cases **badness is the *more* faithful reading**, and texlab is the
one diverging:

- `\left…\right` (`math_operators.tex`): badness's `LEFT_RIGHT` isolates the
  delimiter token, so `\left[ a,b \right)` correctly reads `[` as the delimiter;
  texlab parses the content into a bracket/optional group.
- `\(…\)` (`display_math.tex`): badness wraps it as `INLINE_MATH`; texlab leaves
  the delimiters as bare commands.
- Verbatim *environments* (`verbatim_env.tex`): badness protects `verbatim`/
  `lstlisting` bodies as one opaque `VERBATIM_BODY`; texlab under default config
  parses *into* the body.
- `\iffalse…\fi` (`conditionals.tex`): badness reads the body as generic
  commands (no TeX evaluation, a non-goal); texlab's conditional handling drops
  `\section`'s group.

## One gauge limitation worth noting (not a parser issue)

`tables.tex` (92.3%) diverges only because the **skeleton projector** drops
`BEGIN` wholesale (`Cat::Drop`), hiding the `\begin{tabular}{lcr}` column-spec
group that badness's CST *does* preserve as a `GROUP` child of `BEGIN`. texlab
surfaces the spec, so the projection — not the parse — diverges. It is
allowlisted with that reason.

**Optional gauge improvement (not applied):** in
`tests/support/parse_skeleton.rs`, project an environment's `BEGIN` argument
groups (the `GROUP`/`OPTIONAL` children, skipping the control word and
`NAME_GROUP`) as children of the `Env` atom, so the column spec is compared
instead of dropped. This is test-infra only and would make `tables.tex`
concordant, sharpening the signal. Left for a separate decision since it shifts
the similarity metric.
