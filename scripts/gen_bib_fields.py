#!/usr/bin/env python3
"""Sync ``data/bib_fields.json`` with BibLaTeX's canonical data model.

The data model (``blx-dm.def``, shipped with the ``biblatex`` package and consumed
by ``biber``) is the source of truth for the *mechanical* facts in our DB:

* the set of entry types,
* every input field and its category (name / date / verbatim / literal), and
* each entry type's mandatory-field constraints (the ``required`` lists).

This script keeps those in lockstep with whatever ``biblatex`` is installed. It does
**not** own the *hand-curated* parts, which it preserves verbatim:

* the ``optional`` field lists — their order is a deliberate formatter display choice
  (fields are emitted required-then-optional), and the data model only lists fields
  alphabetically, which would read badly;
* the classic-BibTeX overlay — any entry type or field present in the JSON but absent
  from the data model (e.g. ``mastersthesis``/``school``, the ``journal`` alias) is
  treated as an intentional addition and left untouched;
* the ``aliases`` map (classic-BibTeX field aliases -> canonical BibLaTeX field), which
  biber resolves on input and which is not declared in the data model; and
* the comment headers.

Usage::

    scripts/gen_bib_fields.py                 # check sync, exit 1 on drift (CI hook)
    scripts/gen_bib_fields.py --write         # update the mechanical facts in place
    scripts/gen_bib_fields.py --def PATH ...  # use a specific blx-dm.def

With no ``--def``, the model is located via ``kpsewhich blx-dm.def``.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from collections import OrderedDict
from pathlib import Path

DATA_FILE = Path(__file__).resolve().parent.parent / "data" / "bib_fields.json"

# biblatex datatype -> our coarse category. Anything not named here (integer, key,
# entrykey, keyword, option, range, code) is a plain literal.
_DATATYPE_CATEGORY = {
    "name": "name",
    "date": "date",
    "verbatim": "verbatim",
    "uri": "verbatim",
}


# --- blx-dm.def parsing ----------------------------------------------------


def _balanced(s: str, i: int) -> tuple[str, int]:
    """Given ``s[i] == '{'``, return ``(inner_text, index_past_closing_brace)``."""
    assert s[i] == "{", "expected a brace"
    depth = 0
    for j in range(i, len(s)):
        if s[j] == "{":
            depth += 1
        elif s[j] == "}":
            depth -= 1
            if depth == 0:
                return s[i + 1 : j], j + 1
    raise ValueError("unbalanced braces")


def _names(body: str) -> list[str]:
    return [n for n in re.split(r"[,\s]+", body.strip()) if n]


class DataModel:
    """The mechanical facts extracted from ``blx-dm.def``."""

    def __init__(self, text: str) -> None:
        m_types = re.search(r"\\DeclareDatamodelEntrytypes\{([^}]*)\}", text, re.S)
        m_glob = re.search(r"\\DeclareDatamodelEntryfields\{([^}]*)\}", text, re.S)
        if m_types is None or m_glob is None:
            raise ValueError(
                "blx-dm.def: missing entry-type or global entry-field declaration"
            )
        self.entry_types = _names(m_types.group(1))

        # field -> category, from every \DeclareDatamodelFields[... datatype=X]{...}
        self.field_category: dict[str, str] = {}
        declared: set[str] = set()
        for m in re.finditer(
            r"\\DeclareDatamodelFields\[([^\]]*)\]\{([^}]*)\}", text, re.S
        ):
            dt = re.search(r"datatype=(\w+)", m.group(1))
            cat = _DATATYPE_CATEGORY.get(dt.group(1) if dt else "", "literal")
            for name in _names(m.group(2)):
                self.field_category[name] = cat
                declared.add(name)

        # The global entry-field set (allowed on every type) additionally lists the
        # date-component input fields (day, urlyear, origmonth, ...) that biber accepts
        # but that are not in the \DeclareDatamodelFields blocks. They are all dates.
        for name in _names(m_glob.group(1)):
            if name not in declared:
                self.field_category[name] = "date"

        # type -> ordered list of its type-specific entry fields.
        self.type_fields: dict[str, list[str]] = {}
        for m in re.finditer(
            r"\\DeclareDatamodelEntryfields\[([^\]]*)\]\{([^}]*)\}", text, re.S
        ):
            for t in _names(m.group(1).replace(",", " ")):
                seen = self.type_fields.setdefault(t, [])
                for fld in _names(m.group(2)):
                    if fld not in seen:
                        seen.append(fld)

        # type -> required spec: each item is a str (single) or a sorted list (one-of).
        self.required: dict[str, list] = {}
        for m in re.finditer(r"\\DeclareDatamodelConstraints(\[[^\]]*\])?\s*\{", text):
            if (
                m.group(1) is None
            ):  # global data constraints (isbn/issn/...), not per-type
                continue
            types = _names(m.group(1)[1:-1].replace(",", " "))
            block, _ = _balanced(text, m.end() - 1)
            mm = re.search(r"\\constraint\[type=mandatory\]\s*\{", block)
            if not mm:
                continue
            mbody, _ = _balanced(block, mm.end() - 1)
            spans = []
            for cm in re.finditer(r"\\constraintfields(?:x?or)\s*\{", mbody):
                _, end = _balanced(mbody, cm.end() - 1)
                spans.append((cm.start(), end))

            def in_group(p: int) -> bool:
                return any(a <= p < b for a, b in spans)

            items: list = []
            for cm in re.finditer(
                r"\\constraintfields(x?or)\s*\{|\\constraintfield\{(\w+)\}", mbody
            ):
                if cm.group(1) is not None:
                    inner, _ = _balanced(mbody, cm.end() - 1)
                    items.append(
                        sorted(re.findall(r"\\constraintfield\{(\w+)\}", inner))
                    )
                elif not in_group(cm.start()):
                    items.append(cm.group(2))
            for t in types:
                dst = self.required.setdefault(t, [])
                for it in items:
                    if it not in dst:
                        dst.append(it)


def _req_key(spec: list) -> set:
    """Order-insensitive identity of a required list, for comparison."""
    out = set()
    for it in spec:
        out.add(("oneof", frozenset(it)) if isinstance(it, list) else ("one", it))
    return out


def _req_str(spec: list) -> str:
    """A readable rendering of a required spec, e.g. ``author, title, (date|year)``."""
    parts = [
        "(" + "|".join(sorted(it)) + ")" if isinstance(it, list) else it for it in spec
    ]
    return ", ".join(parts) or "(none)"


# --- emit (matches the hand-authored layout so a synced file round-trips clean) ---


def _arr(items: list) -> str:
    return json.dumps(items, ensure_ascii=False, separators=(", ", ": "))


def _dump(doc: OrderedDict) -> str:
    out: list[str] = ["{"]
    out.append(f'  "_comment": {json.dumps(doc["_comment"], ensure_ascii=False)},')
    out.append("")

    out.append('  "entries": {')
    entries = list(doc["entries"].items())
    for i, (name, sig) in enumerate(entries):
        tail = "" if i == len(entries) - 1 else ","
        out.append(f'    "{name}": {{')
        out.append(f'      "required": {_arr(sig["required"])},')
        out.append(f'      "optional": {_arr(sig["optional"])}')
        out.append(f"    }}{tail}")
    out.append("  },")
    out.append("")

    out.append(
        f'  "_fields_comment": {json.dumps(doc["_fields_comment"], ensure_ascii=False)},'
    )
    out.append("")

    out.append('  "fields": {')
    fields = list(doc["fields"].items())
    prev_cat = None
    for i, (name, sig) in enumerate(fields):
        cat = sig["category"]
        if prev_cat is not None and cat != prev_cat:
            out.append("")  # blank line between category groups
        prev_cat = cat
        tail = "" if i == len(fields) - 1 else ","
        out.append(f'    "{name}": {{ "category": "{cat}" }}{tail}')

    # The classic-BibTeX alias overlay is hand-curated (not in the data model); emit it
    # verbatim after the fields so a synced file round-trips unchanged.
    if "aliases" in doc:
        out.append("  },")
        out.append("")
        out.append(
            f'  "_aliases_comment": {json.dumps(doc["_aliases_comment"], ensure_ascii=False)},'
        )
        out.append('  "aliases": {')
        aliases = list(doc["aliases"].items())
        for i, (alias, canon) in enumerate(aliases):
            tail = "" if i == len(aliases) - 1 else ","
            out.append(f'    "{alias}": "{canon}"{tail}')
        out.append("  }")
    else:
        out.append("  }")

    out.append("}")
    return "\n".join(out) + "\n"


# --- check / write ---------------------------------------------------------


def _expected(model: DataModel, doc: OrderedDict):
    """Yield ('kind', detail) drift records comparing the model to the JSON ``doc``."""
    entries, fields = doc["entries"], doc["fields"]

    for ty in model.entry_types:
        if ty not in entries:
            yield ("missing-entry-type", ty)

    for name, cat in model.field_category.items():
        if name not in fields:
            yield ("missing-field", f"{name} ({cat})")
        elif fields[name]["category"] != cat:
            yield (
                "wrong-category",
                f"{name}: have {fields[name]['category']!r}, model {cat!r}",
            )

    for ty in model.entry_types:
        if ty not in entries:
            continue
        model_req = model.required.get(ty, [])
        if _req_key(model_req) != _req_key(entries[ty]["required"]):
            yield (
                "required-mismatch",
                f"{ty}: model wants [{_req_str(model_req)}], have [{_req_str(entries[ty]['required'])}]",
            )


def _apply(model: DataModel, doc: OrderedDict) -> None:
    """Mutate ``doc`` in place to match the model, preserving curation and overlay."""
    entries, fields = doc["entries"], doc["fields"]

    for name, cat in model.field_category.items():
        if name in fields:
            fields[name]["category"] = cat
        else:
            fields[name] = {"category": cat}

    for ty in model.entry_types:
        req = [
            sorted(x) if isinstance(x, list) else x for x in model.required.get(ty, [])
        ]
        reqnames = {n for x in req for n in ([x] if isinstance(x, str) else x)}
        if ty in entries:
            # Only rewrite when the *set* differs — preserve the curated item order
            # (and one-of alternative order) of an already-correct list, so a synced
            # file round-trips unchanged. On genuine drift, emit the model order; a
            # human can reorder for display.
            if _req_key(entries[ty]["required"]) != _req_key(req):
                entries[ty]["required"] = req
        else:  # new type: seed optional from the model (a human may reorder later)
            opt = [f for f in model.type_fields.get(ty, []) if f not in reqnames]
            entries[ty] = {"required": req, "optional": opt}


def _load_model(def_path: str | None) -> DataModel:
    if def_path is None:
        try:
            def_path = subprocess.run(
                ["kpsewhich", "blx-dm.def"], capture_output=True, text=True, check=True
            ).stdout.strip()
        except (subprocess.CalledProcessError, FileNotFoundError):
            sys.exit(
                "error: could not locate blx-dm.def (is biblatex installed?); pass --def PATH"
            )
    if not def_path or not Path(def_path).is_file():
        sys.exit(f"error: blx-dm.def not found at {def_path!r}")
    return DataModel(Path(def_path).read_text())


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--def", dest="def_path", metavar="PATH", help="path to blx-dm.def")
    ap.add_argument(
        "--write", action="store_true", help="update data/bib_fields.json in place"
    )
    args = ap.parse_args()

    model = _load_model(args.def_path)
    doc = json.loads(DATA_FILE.read_text(), object_pairs_hook=OrderedDict)

    if args.write:
        _apply(model, doc)
        DATA_FILE.write_text(_dump(doc))
        drift = list(_expected(model, doc))
        if drift:  # should never happen right after _apply
            print(
                "warning: residual drift after write:",
                *drift,
                sep="\n  ",
                file=sys.stderr,
            )
        print(f"wrote {DATA_FILE.relative_to(Path.cwd())}")
        return 0

    drift = list(_expected(model, doc))
    if not drift:
        print(
            f"{DATA_FILE.name} is in sync with the data model "
            f"({len(model.entry_types)} entry types, {len(model.field_category)} fields)"
        )
        return 0
    print(f"{DATA_FILE.name} is OUT OF SYNC with the data model:", file=sys.stderr)
    for kind, detail in drift:
        print(f"  {kind}: {detail}", file=sys.stderr)
    print("\nrun with --write to update the mechanical facts.", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
