//! Shared helpers for the post-build doc tools (`sitemap`, `canonical`).
//!
//! mdbook renders the book to a directory of HTML pages but exposes no notion of
//! the public site URL those pages are served from, so anything URL-shaped that
//! a search engine wants — a sitemap, a `<link rel="canonical">` — has to be
//! reconstructed after the build from the rendered tree plus a base URL passed
//! in. This module owns that reconstruction so the two tools agree byte-for-byte
//! on what each page's canonical URL is.

use std::path::{Path, PathBuf};

use lol_html::{element, rewrite_str, text, RewriteStrSettings};

pub const HELPER_PAGES: [&str; 3] = ["404.html", "print.html", "toc.html"];

#[derive(Default)]
pub struct Head {
    pub title: String,
    pub description: String,
    pub canonical: Option<String>,
    pub redirect: bool,
    pub metadata: Vec<String>,
}

/// Read rendered HTML so quoting and entity spelling do not affect metadata.
pub fn read_head(html: &str) -> Head {
    let mut title = String::new();
    let mut head = Head::default();
    rewrite_str(
        html,
        RewriteStrSettings {
            element_content_handlers: vec![
                text!("head title", |chunk| {
                    title.push_str(chunk.as_str());
                    Ok(())
                }),
                element!("head meta", |element| {
                    if let Some(name) = element
                        .get_attribute("property")
                        .or_else(|| element.get_attribute("name"))
                    {
                        if name == "description" {
                            head.description = html_escape::decode_html_entities(
                                &element.get_attribute("content").unwrap_or_default(),
                            )
                            .into_owned();
                        }
                        head.metadata.push(name);
                    }
                    if element
                        .get_attribute("http-equiv")
                        .is_some_and(|value| value.eq_ignore_ascii_case("refresh"))
                    {
                        head.redirect = true;
                    }
                    Ok(())
                }),
                element!("head link[rel='canonical']", |element| {
                    head.canonical = element
                        .get_attribute("href")
                        .map(|url| html_escape::decode_html_entities(&url).into_owned());
                    Ok(())
                }),
            ],
            ..RewriteStrSettings::default()
        },
    )
    .expect("rendered HTML must be readable");
    head.title = html_escape::decode_html_entities(&title).into_owned();
    head
}

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
/// Refresh redirects are excluded because their destinations are listed instead.
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
        if HELPER_PAGES.contains(&rel.as_str()) {
            continue;
        }
        let html = std::fs::read_to_string(&path).expect("rendered page must be readable");
        if read_head(&html).redirect {
            continue;
        }
        let loc = match rel.strip_suffix("index.html") {
            Some(prefix) => prefix.to_string(),
            None => rel,
        };
        pages.push(Page { path, loc });
    }
}
