//! Generate the linter-rules reference page from rule metadata.
//!
//! Run with `cargo run --example docgen`. It renders the same markdown the
//! snapshot test pins ([`badness::linter::docs::render_reference_page`]) and
//! writes it to the mdBook source tree. Living as an `examples/` target (not a
//! `[[bin]]`) keeps badness a single, publishable crate.

use std::fs;
use std::io;
use std::path::Path;

use badness::linter::docs::render_reference_page;

fn main() -> io::Result<()> {
    let path = Path::new("docs/src/reference/linter-rules.md");
    write_if_changed(path, &render_reference_page())
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
