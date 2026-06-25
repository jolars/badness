//! Post-build sitemap generator for the badness docs.
//!
//! mdbook has no built-in sitemap, so this small tool walks the rendered book
//! directory after `mdbook build` and writes a `sitemap.xml` listing every
//! HTML page. Run it as:
//!
//! ```text
//! sitemap <book-dir> <base-url>
//! ```
//!
//! e.g. `sitemap docs/book https://badness.dev/`. The base URL is the public
//! root the book is served from (the custom domain apex).

use std::path::Path;
use std::process::Command;

use doc_utils::postbuild::{collect_pages, normalize_base};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(book_dir), Some(base_url)) = (args.next(), args.next()) else {
        eprintln!("usage: sitemap <book-dir> <base-url>");
        std::process::exit(1);
    };

    let book_dir = Path::new(&book_dir);
    let base = normalize_base(&base_url);
    let pages = collect_pages(book_dir);

    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n");
    for page in &pages {
        out.push_str("  <url>\n");
        out.push_str(&format!("    <loc>{base}{}</loc>\n", page.loc));
        if let Some(lastmod) = source_lastmod(book_dir, &page.path) {
            out.push_str(&format!("    <lastmod>{lastmod}</lastmod>\n"));
        }
        out.push_str("  </url>\n");
    }
    out.push_str("</urlset>\n");

    let dest = book_dir.join("sitemap.xml");
    if let Err(e) = std::fs::write(&dest, out) {
        eprintln!("failed to write {}: {e}", dest.display());
        std::process::exit(1);
    }
    eprintln!("wrote {} ({} urls)", dest.display(), pages.len());
}

/// Best-effort last-modified date from git, derived from the page's source
/// markdown (`book/guide/x.html` -> `src/guide/x.md`). Returns `None` when the
/// source can't be mapped or git isn't available, in which case the entry is
/// emitted without a `<lastmod>`.
fn source_lastmod(root: &Path, html: &Path) -> Option<String> {
    let rel = html.strip_prefix(root).ok()?;
    let src_root = root.parent()?.join("src");
    let md = src_root.join(rel).with_extension("md");
    if !md.exists() {
        return None;
    }
    let output = Command::new("git")
        .args(["log", "-1", "--format=%cs", "--"])
        .arg(&md)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let date = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!date.is_empty()).then_some(date)
}
