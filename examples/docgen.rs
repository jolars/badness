//! Generate the linter-rules reference pages (LaTeX and BibTeX) from rule
//! metadata.
//!
//! Run with `cargo run --example docgen`. It renders the same markdown the
//! snapshot tests pin ([`badness::linter::docs::render_reference_page`] and
//! [`badness::bib::linter::docs::render_reference_page`]) and writes it to the
//! mdBook source tree. Living as an `examples/` target (not a `[[bin]]`) keeps
//! badness a single, publishable crate.

use std::fs;
use std::io;
use std::path::Path;

fn main() -> io::Result<()> {
    write_if_changed(
        Path::new("docs/src/reference/linter-rules.md"),
        &badness::linter::docs::render_reference_page(),
    )?;
    write_if_changed(
        Path::new("docs/src/reference/bib-linter-rules.md"),
        &badness::bib::linter::docs::render_reference_page(),
    )
}

/// Write `content` to `path` only when it differs from what's already there, so
/// re-running the generator leaves an unchanged file (and its mtime) alone.
fn write_if_changed(path: &Path, content: &str) -> io::Result<()> {
    if fs::read_to_string(path).is_ok_and(|existing| existing == content) {
        return Ok(());
    }
    fs::write(path, content)?;
    println!("wrote {}", path.display());
    Ok(())
}
