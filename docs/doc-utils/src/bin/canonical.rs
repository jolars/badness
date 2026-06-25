//! Post-build canonical-URL injector for the badness docs.
//!
//! mdbook has no `canonical-site-url` setting (see
//! <https://github.com/rust-lang/mdBook/pull/2706>), so rendered pages ship
//! without a `<link rel="canonical">`. This tool walks the built book after
//! `mdbook build` and inserts a canonical link into each content page's
//! `<head>`, pointing at the page's public URL under the given base. Run it as:
//!
//! ```text
//! canonical <book-dir> <base-url>
//! ```
//!
//! e.g. `canonical docs/book https://badness.dev/`. The canonical
//! URL of each page is derived exactly as the sitemap derives its `<loc>`, so a
//! page's canonical link and its sitemap entry always agree.
//!
//! The injection is idempotent: a page that already carries a `rel="canonical"`
//! link is left untouched, so re-running over an already-processed tree is a
//! no-op.

use std::path::Path;

use doc_utils::postbuild::{collect_pages, normalize_base};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(book_dir), Some(base_url)) = (args.next(), args.next()) else {
        eprintln!("usage: canonical <book-dir> <base-url>");
        std::process::exit(1);
    };

    let book_dir = Path::new(&book_dir);
    let base = normalize_base(&base_url);
    let pages = collect_pages(book_dir);

    let mut injected = 0usize;
    for page in &pages {
        let Ok(html) = std::fs::read_to_string(&page.path) else {
            eprintln!("warning: could not read {}", page.path.display());
            continue;
        };
        // Idempotent: never add a second canonical link.
        if html.contains("rel=\"canonical\"") {
            continue;
        }
        let Some(pos) = html.find("</head>") else {
            eprintln!("warning: no </head> in {}, skipping", page.path.display());
            continue;
        };
        let href = escape_attr(&format!("{base}{}", page.loc));
        let tag = format!("    <link rel=\"canonical\" href=\"{href}\">\n");
        let mut out = String::with_capacity(html.len() + tag.len());
        out.push_str(&html[..pos]);
        out.push_str(&tag);
        out.push_str(&html[pos..]);
        if let Err(e) = std::fs::write(&page.path, out) {
            eprintln!("failed to write {}: {e}", page.path.display());
            std::process::exit(1);
        }
        injected += 1;
    }
    eprintln!(
        "injected canonical links into {injected}/{} pages",
        pages.len()
    );
}

/// Escape the characters that are unsafe inside a double-quoted HTML attribute.
fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;")
}
