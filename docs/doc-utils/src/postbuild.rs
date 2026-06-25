//! Shared helpers for the post-build doc tools (`sitemap`, `canonical`).
//!
//! mdbook renders the book to a directory of HTML pages but exposes no notion of
//! the public site URL those pages are served from, so anything URL-shaped that
//! a search engine wants — a sitemap, a `<link rel="canonical">` — has to be
//! reconstructed after the build from the rendered tree plus a base URL passed
//! in. This module owns that reconstruction so the two tools agree byte-for-byte
//! on what each page's canonical URL is.

use std::path::{Path, PathBuf};

/// A rendered content page of the book.
pub struct Page {
    /// Path to the HTML file on disk.
    pub path: PathBuf,
    /// URL path relative to the site base, with `index.html` collapsed to its
    /// directory: `index.html` -> ``, `guide/index.html` -> `guide/`,
    /// `guide/x.html` -> `guide/x.html`.
    pub loc: String,
}

/// Normalize a base URL to exactly one trailing slash so joins are unambiguous.
pub fn normalize_base(base_url: &str) -> String {
    format!("{}/", base_url.trim_end_matches('/'))
}

/// Recursively collect every public HTML content page under `book_dir`, sorted
/// by URL path for deterministic output. mdbook's helper pages (`404.html`,
/// `print.html`, the `toc.html` sidebar fragment) are skipped: they are not
/// standalone content and want neither a sitemap entry nor a canonical URL.
pub fn collect_pages(book_dir: &Path) -> Vec<Page> {
    let mut pages = Vec::new();
    collect_html(book_dir, book_dir, &mut pages);
    pages.sort_by(|a, b| a.loc.cmp(&b.loc));
    pages
}

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
        if matches!(rel.as_str(), "404.html" | "print.html" | "toc.html") {
            continue;
        }
        let loc = match rel.strip_suffix("index.html") {
            Some(prefix) => prefix.to_string(),
            None => rel,
        };
        pages.push(Page { path, loc });
    }
}
