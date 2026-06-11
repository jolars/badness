# Wrap Modes

The `--wrap` flag controls how the formatter lays out line breaks *inside a
paragraph*. It does not affect structure—only where soft line breaks fall.

```sh
badness format --wrap preserve paper.tex
```

  | Mode       | Status                        | Behavior                                                                                                     |
  | ---------- | ----------------------------- | ------------------------------------------------------------------------------------------------------------ |
  | `reflow`   | implemented                   | **Default.** Greedy fill: pack words up to `--line-width`, breaking only where the next word would overflow. |
  | `preserve` | implemented                   | Leave the authored line breaks untouched.                                                                    |
  | `sentence` | accepted, not yet implemented | Intended to put one sentence per line. Currently falls back to `preserve`.                                   |
  | `semantic` | accepted, not yet implemented | Intended for [semantic line breaks](https://sembr.org). Currently falls back to `preserve`.                  |

`sentence` and `semantic` are accepted on the command line so scripts can adopt
them now, but they currently behave like `preserve`. They will gain their own
behavior in a future release without a flag change.

## How it works

Reflow is implemented through the formatter's `Doc` layout engine as a *fill*
node (in the Wadler/Prettier sense): per-gap greedy break decisions, where each
inter-word gap independently becomes a space or a line break depending on
whether the next word fits within `--line-width`. The printer remains the single
layout authority—wrap modes are expressed *through* it rather than as a separate
line-filling pass.
