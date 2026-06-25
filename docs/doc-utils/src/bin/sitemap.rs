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
//! e.g. `sitemap docs/book https://jolars.github.io/badness/`. The base URL is
//! the public root the book is served from (the GitHub Pages project URL).

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(book_dir), Some(base_url)) = (args.next(), args.next()) else {
        eprintln!("usage: sitemap <book-dir> <base-url>");
        std::process::exit(1);
    };

    let book_dir = PathBuf::from(book_dir);
    // Normalize to exactly one trailing slash so joins are unambiguous.
    let base = format!("{}/", base_url.trim_end_matches('/'));

    let mut pages = Vec::new();
    collect_html(&book_dir, &book_dir, &mut pages);
    pages.sort();

    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n");
    for page in &pages {
        out.push_str("  <url>\n");
        out.push_str(&format!("    <loc>{base}{}</loc>\n", page.loc));
        if let Some(lastmod) = &page.lastmod {
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

struct Page {
    /// URL path relative to the base URL.
    loc: String,
    /// `YYYY-MM-DD` last-commit date of the source page, when resolvable.
    lastmod: Option<String>,
}

// Sort by the URL path so the output is deterministic.
impl PartialEq for Page {
    fn eq(&self, other: &Self) -> bool {
        self.loc == other.loc
    }
}
impl Eq for Page {}
impl PartialOrd for Page {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Page {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.loc.cmp(&other.loc)
    }
}

/// Recursively collect every public HTML page under `dir`, recording its URL
/// path relative to `root`.
fn collect_html(root: &Path, dir: &Path, pages: &mut Vec<Page>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_html(root, &path, pages);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("html") {
            continue;
        }
        let rel = path.strip_prefix(root).unwrap();
        let rel = rel.to_string_lossy().replace('\\', "/");
        // mdbook ships these helpers; they are not content pages. `toc.html` is
        // the sidebar iframe fragment, not a standalone page.
        if matches!(rel.as_str(), "404.html" | "print.html" | "toc.html") {
            continue;
        }
        // Map `index.html` to its directory so `/` and `/index.html` don't both
        // appear: `index.html` -> ``, `guide/index.html` -> `guide/`.
        let loc = match rel.strip_suffix("index.html") {
            Some(prefix) => prefix.to_string(),
            None => rel,
        };
        let lastmod = source_lastmod(root, &path);
        pages.push(Page { loc, lastmod });
    }
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
