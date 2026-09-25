//! Publish canonical links and social previews for the book and playground.
//!
//! Usage: canonical <book-dir> <base-url>

use std::path::Path;

use doc_utils::postbuild::{collect_pages, normalize_base, read_head, HELPER_PAGES};
use lol_html::{element, html_content::ContentType, rewrite_str, RewriteStrSettings};

const IMAGE: &[u8] = include_bytes!("../../../../branding/og.png");

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(book_dir), Some(base_url)) = (args.next(), args.next()) else {
        eprintln!("usage: canonical <book-dir> <base-url>");
        std::process::exit(1);
    };
    let book_dir = Path::new(&book_dir);
    let base = normalize_base(&base_url);
    let image_url = format!("{base}images/og.png");
    std::fs::create_dir_all(book_dir.join("images"))?;
    std::fs::write(book_dir.join("images/og.png"), IMAGE)?;
    // PNG's IHDR dimensions keep the tags correct when the branding is rebuilt.
    let width = u32::from_be_bytes(IMAGE[16..20].try_into()?);
    let height = u32::from_be_bytes(IMAGE[20..24].try_into()?);
    let pages = collect_pages(book_dir);
    for page in &pages {
        let html = std::fs::read_to_string(&page.path)?;
        let head = read_head(&html);
        let url = head
            .canonical
            .clone()
            .unwrap_or_else(|| format!("{base}{}", page.loc));
        let mut tags = String::new();
        if head.canonical.is_none() {
            tags.push_str(&format!(
                "<link rel=\"canonical\" href=\"{}\">\n",
                escape(&url)
            ));
        }
        for (key, value) in [
            ("og:type", "website"),
            ("og:site_name", "Badness"),
            ("og:title", head.title.as_str()),
            ("og:description", head.description.as_str()),
            ("og:url", url.as_str()),
            ("og:image", image_url.as_str()),
            ("og:image:type", "image/png"),
            ("og:image:width", &width.to_string()),
            ("og:image:height", &height.to_string()),
            (
                "og:image:alt",
                "Badness: a formatter, linter, and language server for LaTeX.",
            ),
            ("twitter:card", "summary_large_image"),
            ("twitter:title", head.title.as_str()),
            ("twitter:description", head.description.as_str()),
            ("twitter:image", image_url.as_str()),
            (
                "twitter:image:alt",
                "Badness: a formatter, linter, and language server for LaTeX.",
            ),
        ] {
            if !head.metadata.iter().any(|existing| existing == key) {
                let attribute = if key.starts_with("og:") {
                    "property"
                } else {
                    "name"
                };
                tags.push_str(&format!(
                    "<meta {attribute}=\"{key}\" content=\"{}\">\n",
                    escape(value)
                ));
            }
        }
        let output = rewrite_page(&html, &tags)?;
        std::fs::write(&page.path, output)?;
    }
    // Helper pages still expose navigation, but must not gain indexing metadata.
    for name in HELPER_PAGES {
        let path = book_dir.join(name);
        if path.exists() {
            let html = std::fs::read_to_string(&path)?;
            std::fs::write(path, rewrite_page(&html, "")?)?;
        }
    }
    eprintln!(
        "published canonical links and social metadata for {} pages",
        pages.len()
    );
    Ok(())
}

fn rewrite_page(html: &str, tags: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(rewrite_str(
        html,
        RewriteStrSettings {
            element_content_handlers: vec![
                element!("head", |head| {
                    head.append(tags, ContentType::Html);
                    Ok(())
                }),
                element!("label#mdbook-sidebar-toggle", |toggle| {
                    toggle.set_tag_name("button")?;
                    toggle.remove_attribute("for");
                    toggle.set_attribute("type", "button")?;
                    Ok(())
                }),
            ],
            ..RewriteStrSettings::default()
        },
    )?)
}

fn escape(value: &str) -> String {
    html_escape::encode_double_quoted_attribute(value).into_owned()
}
