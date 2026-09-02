# Formatter fixture recap

This file is mutable project memory for the `formatter-fixture` workflow. It is
not part of the stable instructions and should be read only when choosing an
unnamed next construct or checking whether a surveyed family is already known.

Re-measure all claims before acting. Prefer links, fixture slugs, and short
status notes over implementation narratives. Durable rules belong in tests,
`AGENTS.md`, or the architecture documentation; project work belongs in
`TODO.md`.

## Current lead

- No current lead. Re-measure the remaining environment and argument families
  before choosing the next construct.

## Known exclusions and blockers

- `tokenChecks` exercises latexindent's internal placeholder-token mechanism,
  not a LaTeX formatter construct in Badness.
- `specials` is latexindent's configurable begin/end-pair mechanism; its inputs
  reduce to math constructs already covered elsewhere.
- `diacritics` tests non-ASCII paths rather than document layout.
- Consult the formatter failure inventory before mining a family with a known
  content-changing or format-error failure.
- `ifelsefi` is parser-owned through `CONDITIONAL`; do not derive a
  formatter-only extent rule for it.
- A paragraph-spanning optional attaches only in the tight `[…]{…}` shape.
  Standalone or trivia-separated xparse `+O` would require signature-directed
  generic attachment, which the parser contract forbids without a source-shape
  proof.

## Covered areas

Use these slugs as navigation aids, not as a substitute for searching the
fixture tables:

| Area | Representative fixtures |
| --- | --- |
| Environment frames and bodies | `environment_empty_body`, `environment_special_character_names`, `begin_tail_is_body`, `environment_leading_body_command`, `environment_inline_prose_boundaries`, `environment_adjacent_siblings` |
| Declared environment arguments | `environment_argument_blank_lines`, `environment_argument_comment_barrier`, `environment_argument_comment_slots`, `environment_argument_delimiter_comments`, `environment_argument_escaped_delimiters`, `environment_omitted_optional_slots` |
| Protected bodies | `filecontents_protected_body` |
| Sectioning | `sectioning_starts_own_line`, `sectioning_blank_line_and_comment` |
| Keyval groups | `keyval_group_splits_entries`, `keyval_group_declines_on_comment`, `environment_keyval_group_splits_entries` |
| List markers | `list_item_overlay_prefix` |
| Inline command arguments | `inline_command_argument_glue` |
| Raw macro definitions | `def_delimited_parameters` |
| Display math in prose | `display_math_prose_boundaries` |

## Maintenance

- Keep only the current lead, durable exclusions, blockers, and compact links to
  landed coverage.
- Add a fixture slug to an existing row when useful; do not add a prose account
  of its implementation.
- Do not store corpus counts here. Recount and inspect current inputs whenever a
  family is selected.
- Remove stale leads and resolved blockers instead of preserving chronology.
