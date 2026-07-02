# Wrap Modes

The `--wrap` flag controls how the formatter lays out line breaks *inside a
paragraph*. It does not affect structure—only where soft line breaks fall.

```sh
badness format --wrap preserve paper.tex
```

  | Mode       | Behavior                                                                                                     |
  | ---------- | ------------------------------------------------------------------------------------------------------------ |
  | `reflow`   | **Default.** Greedy fill: pack words up to `--line-width`, breaking only where the next word would overflow. |
  | `preserve` | Leave the authored line breaks untouched.                                                                    |
  | `sentence` | One sentence per line. Line width is ignored—a long sentence stays on one line.                              |
  | `semantic` | [Semantic line breaks](https://sembr.org): keep the author's soft breaks *and* add a break after each sentence. |

## How reflow works

Reflow is implemented through the formatter's `Doc` layout engine as a *fill*
node (in the Wadler/Prettier sense): per-gap greedy break decisions, where each
inter-word gap independently becomes a space or a line break depending on
whether the next word fits within `--line-width`. The printer remains the single
layout authority—wrap modes are expressed *through* it rather than as a separate
line-filling pass.

## Sentence and semantic

Both `sentence` and `semantic` split a paragraph at sentence boundaries, one
sentence per line, ignoring `--line-width`. Boundary detection is a small
per-language rule engine over the words: a `.`, `!`, or `?` ends a sentence
*unless* the word is a known abbreviation (`e.g.`, `Fig.`, `Dr.`, …), an
ellipsis (`...`, `…`), or a contextual abbreviation whose following word signals
that the sentence continues (`U.S. Government` stays together, `U.S. However`
splits).

`semantic` additionally **preserves the author's own line breaks** on top of the
sentence breaks (the [sembr](https://sembr.org) convention). It does not detect
clause boundaries itself—a break after a comma or `and` survives only where the
author placed a newline. A run-on sentence on a single source line is still
sentence-split.

### Language and abbreviations

The abbreviation profile is chosen by document language. Built-in profiles cover
English (default), Czech, German, Spanish, and French; an unknown or unset
language falls back to English. Set the language and extend the no-break list in
`badness.toml`:

```toml
[format]
wrap = "sentence"
lang = "de"                 # BCP-47-style code; the region subtag is folded away

[format.no-break-abbreviations]
default = ["ibid."]         # applied to every document
de = ["bzw.", "Abb."]       # applied only when lang resolves to German
```

An abbreviation listed here never ends a sentence, so no line break is inserted
after it. (Automatic language detection from `babel`/`polyglossia` is not yet
implemented; set `lang` explicitly.)
