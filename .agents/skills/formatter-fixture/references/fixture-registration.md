# Formatter fixture registration

Read this reference only while adding or changing a formatter fixture.

## Files

The usual fixture is:

```text
crates/badness-formatter/tests/fixtures/formatter/<slug>/input.tex
crates/badness-formatter/tests/fixtures/formatter/<slug>/expected.tex
```

Package, class, `.dtx`, and `.ins` fixtures use their corresponding extensions.
If a new fixture extension is introduced, add an `eol=lf` rule to
`.gitattributes`; fixture tests compare bytes on Windows.

## Tables

Fixtures are registered explicitly in
`crates/badness-formatter/tests/format.rs`:

- `FIXTURES`: ordinary `(slug, WrapMode, line_width)` fixtures.
- `MATH_WRAP_FIXTURES`: display-math policy fixtures.
- `DTX_FIXTURES`: ordinary `.dtx` fixtures.
- `DTX_REFLOW_FIXTURES`: width-sensitive `.dtx` reflow fixtures.
- `PACKAGE_FIXTURES`: `.sty` and `.cls` fixtures.
- `INS_FIXTURES`: `.ins` fixtures.

Choose the table whose test supplies the file kind and formatter configuration
the construct needs. Do not register a specialized fixture in the main table
merely for convenience.

## Proof of registration

Run `every_formatter_fixture_is_registered_once`; it rejects orphaned fixture
directories, nonexistent registrations, and duplicate registrations.

Each table is exercised by one looping test. A slug is not a Rust test name, so
filtering `cargo test` by the slug and seeing zero tests proves nothing. To
prove that the intended loop reaches a new fixture:

1. Make a reversible, one-fixture change to `expected.<ext>`.
2. Run the table's test and confirm that its failure names the slug.
3. Restore the exact accepted bytes before continuing.

Change only one expected file at a time because the loop stops at its first
mismatch. Preserve the accepted output before this check; a new fixture is
untracked, so Git cannot restore it for you.
