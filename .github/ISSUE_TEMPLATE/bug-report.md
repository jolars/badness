---
name: Bug report
about: A crash, a losslessness or idempotence violation, wrong output, or LSP
  misbehavior
title: ""
labels: bug
assignees: ""
---

<!-- Fill in what you can. Losslessness (reconstruct == input) and idempotence
     (formatting twice == formatting once) are core invariants; violations of
     either are always bugs. -->

**What happened**

<!-- e.g. panic, `reconstruct != input`, non-idempotent format, wrong diagnostic, LSP misbehavior. -->

**Input `.tex`**

```tex

```

**`badness.toml` settings** (if any)

```toml

```

**Actual output**

```tex

```

**Expected output**

```tex

```

**Command and version**

<!-- e.g. `badness format file.tex`; `badness --version`; OS. -->

**Anything else**
