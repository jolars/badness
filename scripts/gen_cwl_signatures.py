#!/usr/bin/env python3
"""Generate ``data/cwl_signatures.json`` from the TeXstudio CWL corpus.

CWL (Completion Word List) files are TeXstudio's per-package completion data. They
are a broad, low-precision source of command/environment **names** and **argument
arity** — exactly the bulk tier badness wants *underneath* its hand-curated
``data/signatures.json`` (see ``src/semantic/signature.rs`` module docs). This script
ingests them into the *same* JSON schema.

What it extracts (deliberately minimal — see AGENTS.md decision #2 and the plan):

* command/environment **names**, and
* **argument shapes** only: each top-level ``{…}`` → ``"req"``, ``[…]`` → ``"opt"``.

Every behavior flag (``prose``/``collapse``/``inline``/``verbatim``/``verbatimBody``/
``sectioning``/``math``/``align``/…) is left at its default. CWL's classification
suffixes (``#V``, ``#\\math``, ``#L0``-``#L5``, ``#U``, …) are *reported* (``--report``)
so a human can promote a trustworthy one into the curated file by hand, but are never
applied automatically: this bulk tier is conservative by construction, so a wrong CWL
fact can only ever contribute arity, never flip a formatter/lexer behavior.

The corpus is **fetched from a pinned texstudio commit** at sync time; no GPL ``.cwl``
files are committed. Only mechanical facts (names + arity) reach the output, never CWL
source text — the same posture as ``gen_bib_fields.py`` versus biblatex's data model.

Usage::

    scripts/gen_cwl_signatures.py                 # check sync, exit 1 on drift
    scripts/gen_cwl_signatures.py --write         # regenerate the JSON in place
    scripts/gen_cwl_signatures.py --report        # also print observed classifications
    scripts/gen_cwl_signatures.py --source DIR    # use a local dir of .cwl files
    scripts/gen_cwl_signatures.py --selftest      # run offline unit checks
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tempfile
from collections import OrderedDict, defaultdict
from pathlib import Path

DATA_FILE = Path(__file__).resolve().parent.parent / "data" / "cwl_signatures.json"

# Pinned source: texstudio's completion/ directory at this commit. Bump deliberately
# (then `--write`); the SHA is echoed into the generated file's header comment.
TEXSTUDIO_REPO = "https://github.com/texstudio-org/texstudio.git"
TEXSTUDIO_REF = "dd992ba761e8f8861f09eda5f254ce538b21b423"

# Curated allowlist of `.cwl` basenames to ingest. The full corpus is ~4400 files /
# 117k commands (~560 KB gzipped, mostly obscure single-package symbols); a focused
# subset of widely-used packages keeps the embedded data and completion noise small
# while covering the long tail that actually shows up in documents. Expand freely —
# add a basename here and `task cwl:sync`. (Document *classes* are a separate axis,
# intentionally omitted for now: they pull in large, document-specific command sets.)
ALLOWLIST = frozenset(
    # base / core LaTeX
    "tex latex-document latex-dev standardsectioning textcomp latexsym"
    # math
    " amsmath amssymb amsthm amsfonts amsopn mathtools bm esint mathrsfs cancel physics"
    # graphics & color
    " graphicx graphics xcolor color"
    # page layout & sectioning
    " geometry fancyhdr titlesec titletoc tocloft appendix setspace parskip"
    # tables
    " array tabularx longtable booktabs multirow multicol makecell colortbl"
    # lists
    " enumitem paralist"
    # floats & captions
    " caption subcaption subfig float wrapfig rotating"
    # cross-references & links
    " hyperref url csquotes cleveref varioref nameref"
    # bibliography
    " biblatex natbib cite"
    # verbatim / code / boxes
    " listings minted fancyvrb verbatim tcolorbox mdframed framed"
    # fonts & language
    " fontspec babel inputenc fontenc microtype"
    # units
    " siunitx"
    # tikz / pgf
    " tikz pgfplots pgf"
    # presentation: beamer's authoring API is split across beamerbase* files that
    # class-beamer pulls in via #include (which we don't flatten), so list them.
    " class-beamer beamerbaseframe beamerbaselocalstructure beamerbaseoverlay"
    " beamerbasetemplates beamerbasetheorems beamerbasecolor beamerbasefont"
    " beamerbasetitle beamerbasesection beamerbasetoc beamerbasenotes beamerbaseboxes"
    # notes & markup
    " todonotes soul ulem"
    # algorithms
    " algorithm algorithm2e algorithmicx algpseudocode algorithmic"
    # programming utilities
    " etoolbox xparse xspace ifthen calc environ"
    # glossaries & index
    " glossaries acronym nomencl makeidx imakeidx"
    # document inclusion
    " pdfpages standalone import subfiles"
    # common content helpers
    " lipsum blindtext mhchem chemfig datetime2 hyphenat enumerate".split()
)


# --- CWL line parsing ---------------------------------------------------------


def _split_classification(line: str) -> tuple[str, str]:
    """Split a CWL entry into ``(head, classification)`` at the first *unescaped*
    ``#`` (``\\#`` is the literal-hash command, not a separator). The classification
    is everything after it (without the ``#``); empty if there is none."""
    i = 0
    while i < len(line):
        c = line[i]
        if c == "\\":  # skip an escaped char (e.g. \# or \%)
            i += 2
            continue
        if c == "#":
            return line[:i], line[i + 1 :]
        i += 1
    return line, ""


def _classifier_flags(classification: str) -> str:
    """The leading run of single-letter classifiers, before any ``\\env`` alias,
    ``/env`` context, ``,`` list, or nested ``#``. E.g. ``"mS"`` from ``mS\\array``."""
    m = re.match(r"[A-Za-z0-9*]*", classification)
    return m.group(0) if m else ""


def _consume_group(s: str, i: int, open_ch: str, close_ch: str) -> int:
    """Given ``s[i] == open_ch``, return the index just past the matching close.
    Balances only on ``open_ch``/``close_ch`` (so ``[key={x}]`` closes correctly) and
    honors backslash escapes. Returns ``len(s)`` if unbalanced (tolerant)."""
    depth = 0
    while i < len(s):
        c = s[i]
        if c == "\\":
            i += 2
            continue
        if c == open_ch:
            depth += 1
        elif c == close_ch:
            depth -= 1
            if depth == 0:
                return i + 1
        i += 1
    return len(s)


def _parse_arg_shape(rest: str) -> list[str]:
    """Walk the contiguous leading argument groups of ``rest``: ``{…}`` → ``"req"``,
    ``[…]`` → ``"opt"``, in order. Stops at the first non-bracket, non-space character
    (e.g. ``(`` picture coords, ``*`` stars, ``|`` verb delimiters) — deliberately
    conservative: an uncounted trailing group stays ordinary content downstream."""
    args: list[str] = []
    i = 0
    while i < len(rest):
        c = rest[i]
        if c.isspace():
            i += 1
            continue
        if c == "{":
            i = _consume_group(rest, i, "{", "}")
            args.append("req")
        elif c == "[":
            i = _consume_group(rest, i, "[", "]")
            args.append("opt")
        else:
            break
    return args


# One parsed entry: kind in {"command", "environment"}, a name, and an arg shape.
class Entry:
    __slots__ = ("kind", "name", "args", "flags")

    def __init__(self, kind: str, name: str, args: list[str], flags: str):
        self.kind = kind
        self.name = name
        self.args = args
        self.flags = flags


def parse_line(line: str) -> Entry | None:
    """Parse a single CWL entry line into an :class:`Entry`, or ``None`` if the line
    is not a command/environment entry (blank, directive, comment, template)."""
    line = line.rstrip("\n").rstrip("\r")
    if not line.startswith("\\"):
        return None  # directives/comments start with '#', templates with '%'

    head, classification = _split_classification(line)
    head = head.rstrip()
    flags = _classifier_flags(classification)

    if head.startswith("\\begin{"):
        end = _consume_group(head, len("\\begin"), "{", "}")
        name = head[len("\\begin") + 1 : end - 1]
        if not name:
            return None
        return Entry("environment", name, _parse_arg_shape(head[end:]), flags)

    if head.startswith("\\end{"):
        return None  # \end entries carry no new signature

    # A command: control word (\section) or control symbol (\#, \,). Drop a trailing
    # '*' star — a starred variant maps to the same base name (the parser lexes the
    # star separately); its differing arity reconciles like any other variant.
    m = re.match(r"\\([a-zA-Z]+|.)", head)
    if not m:
        return None
    name = m.group(1)
    rest = head[m.end() :]
    if rest.startswith("*"):
        rest = rest[1:]
    return Entry("command", name, _parse_arg_shape(rest), flags)


# --- corpus -> database -------------------------------------------------------


def _reconcile(variants: list[list[str]]) -> list[str]:
    """Pick one canonical arg shape for a name seen with several arities. Bias to the
    fewest **mandatory** (``req``) slots, then fewest total, then lexicographically —
    deterministic, and conservative: under-attaching is lossless, over-attaching could
    glue following content into a phantom argument."""
    return min(variants, key=lambda a: (a.count("req"), len(a), a))


def build_db(files: dict[str, str], report: dict[str, set[str]] | None = None) -> OrderedDict:
    """Parse every CWL file's text into the signature-DB JSON shape. ``report``, if
    given, collects ``flags -> {names}`` for observed classifications."""
    commands: dict[str, list[list[str]]] = defaultdict(list)
    environments: dict[str, list[list[str]]] = defaultdict(list)
    in_keyvals = False

    for text in files.values():
        for raw in text.splitlines():
            stripped = raw.lstrip()
            if stripped.startswith("#keyvals:"):
                in_keyvals = True
                continue
            if stripped.startswith("#endkeyvals"):
                in_keyvals = False
                continue
            if in_keyvals or stripped.startswith("#") or not stripped:
                continue  # directives, comments, keyval bodies, blanks
            entry = parse_line(raw)
            if entry is None:
                continue
            if "S" in entry.flags:
                continue  # #S: hidden from completer — drop entirely
            if report is not None and entry.flags:
                report.setdefault(entry.flags, set()).add(entry.name)
            bucket = commands if entry.kind == "command" else environments
            bucket[entry.name].append(entry.args)

    def emit(bucket: dict[str, list[list[str]]]) -> OrderedDict:
        out: OrderedDict = OrderedDict()
        for name in sorted(bucket):
            args = _reconcile(bucket[name])
            out[name] = {"args": args} if args else {}
        return out

    return OrderedDict(
        [
            (
                "_comment",
                "GENERATED by scripts/gen_cwl_signatures.py — do not edit by hand. "
                "Mechanical facts (command/environment names and argument arity) "
                "extracted from a curated package subset of the TeXstudio CWL corpus "
                f"at {TEXSTUDIO_REF}; no CWL source text is reproduced. Lower-precision "
                "tier consulted under the hand-curated data/signatures.json. Run "
                "`task cwl:sync` to regenerate (edit ALLOWLIST in the script to widen).",
            ),
            ("commands", emit(commands)),
            ("environments", emit(environments)),
        ]
    )


def _dump(db: OrderedDict) -> str:
    return json.dumps(db, indent=1, ensure_ascii=False) + "\n"


# --- source fetching ----------------------------------------------------------


def _read_cwl_dir(directory: Path) -> dict[str, str]:
    """Read the allowlisted ``*.cwl`` files from ``directory``. Warns about any
    allowlisted basename that is absent (an upstream rename or a typo here)."""
    out: dict[str, str] = {}
    for stem in sorted(ALLOWLIST):
        f = directory / f"{stem}.cwl"
        if f.is_file():
            out[f.name] = f.read_text(encoding="utf-8", errors="replace")
        else:
            print(f"warning: allowlisted {stem}.cwl not found in corpus", file=sys.stderr)
    if not out:
        sys.exit(f"error: none of the {len(ALLOWLIST)} allowlisted .cwl files under {directory}")
    return out


def fetch_corpus(source: str | None, ref: str) -> dict[str, str]:
    """Return ``{filename: text}`` for every ``completion/*.cwl``. With ``--source`` use
    a local directory; otherwise sparse-fetch ``completion/`` at the pinned ``ref``."""
    if source is not None:
        return _read_cwl_dir(Path(source))
    with tempfile.TemporaryDirectory() as tmp:
        def git(*a: str) -> None:
            subprocess.run(["git", "-C", tmp, *a], check=True, capture_output=True, text=True)

        try:
            git("init", "-q")
            git("remote", "add", "origin", TEXSTUDIO_REPO)
            git("sparse-checkout", "init", "--cone")
            git("sparse-checkout", "set", "completion")
            git("fetch", "-q", "--depth", "1", "--filter=blob:none", "origin", ref)
            git("checkout", "-q", "FETCH_HEAD")
        except FileNotFoundError:
            sys.exit("error: git not found; install git or pass --source DIR")
        except subprocess.CalledProcessError as e:
            sys.exit(f"error: failed to fetch texstudio {ref}:\n{e.stderr}")
        return _read_cwl_dir(Path(tmp) / "completion")


# --- self-test ----------------------------------------------------------------


def _selftest() -> int:
    def eq(got, want, msg):
        assert got == want, f"{msg}: got {got!r}, want {want!r}"

    eq(_parse_arg_shape("{a}[b]{c}"), ["req", "opt", "req"], "mixed order")
    eq(_parse_arg_shape("{%<num%>}{%<den%>}"), ["req", "req"], "placeholders")
    eq(_parse_arg_shape("[opt={x}]{w}"), ["opt", "req"], "brace inside bracket")
    eq(_parse_arg_shape("(w,h){len}"), [], "stop at picture coords")

    def pl(line: str) -> Entry:
        e = parse_line(line)
        assert e is not None, f"expected an entry from {line!r}"
        return e

    eq(pl("\\frac{num}{den}").args, ["req", "req"], "frac")
    eq(pl("\\begin{minipage}[pos]{width}").name, "minipage", "env name")
    eq(pl("\\begin{minipage}[pos]{width}").args, ["opt", "req"], "env args")
    eq(pl("\\section[short]{title}#L2").name, "section", "section name")
    eq(pl("\\Alph*#*").name, "Alph", "star stripped")
    eq(pl("\\Alph*#*").args, [], "starred no args")
    eq(pl("\\hidden{x}#S").flags, "S", "S flag parsed")
    eq(parse_line("\\end{foo}"), None, "end dropped")
    eq(parse_line("# a comment"), None, "comment dropped")
    eq(_reconcile([["opt", "req"], ["req"], ["opt", "opt", "req"]]), ["req"], "fewest req")

    db = build_db({"t.cwl": "\\foo{a}\n\\foo[b]{a}\n\\bar#S\n\\baz\n\\begin{env}{x}\n"})
    eq(db["commands"]["foo"], {"args": ["req"]}, "reconciled foo")
    eq("bar" in db["commands"], False, "hidden bar skipped")
    eq(db["commands"]["baz"], {}, "no-arg baz")
    eq(db["environments"]["env"], {"args": ["req"]}, "env emitted")
    eq(_dump(build_db({"a": _dump(db)})) is not None, True, "dump runs")  # smoke
    print("selftest: ok")
    return 0


# --- main ---------------------------------------------------------------------


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--write", action="store_true", help="regenerate the JSON in place")
    ap.add_argument("--report", action="store_true", help="print observed classifications")
    ap.add_argument("--source", metavar="DIR", help="local dir of .cwl files (skip fetch)")
    ap.add_argument("--ref", default=TEXSTUDIO_REF, help="texstudio commit to fetch")
    ap.add_argument("--selftest", action="store_true", help="run offline unit checks")
    args = ap.parse_args()

    if args.selftest:
        return _selftest()

    report: dict[str, set[str]] | None = {} if args.report else None
    files = fetch_corpus(args.source, args.ref)
    db = build_db(files, report)
    n_cmd, n_env = len(db["commands"]), len(db["environments"])

    if report is not None:
        print(f"observed classifications across {len(files)} files:", file=sys.stderr)
        for flags in sorted(report):
            names = sorted(report[flags])
            sample = ", ".join(names[:6]) + (" …" if len(names) > 6 else "")
            print(f"  #{flags}: {len(names)} ({sample})", file=sys.stderr)

    rendered = _dump(db)
    if args.write:
        DATA_FILE.write_text(rendered)
        print(f"wrote {DATA_FILE.relative_to(Path.cwd())} ({n_cmd} commands, {n_env} environments)")
        return 0

    current = DATA_FILE.read_text() if DATA_FILE.is_file() else ""
    if current == rendered:
        print(f"{DATA_FILE.name} is in sync ({n_cmd} commands, {n_env} environments)")
        return 0
    print(f"{DATA_FILE.name} is OUT OF SYNC with the CWL corpus at {args.ref}.", file=sys.stderr)
    print("run with --write to regenerate.", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
