use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Site(PathBuf);

impl Site {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "badness-site-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn write(&self, name: &str, html: &str) {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, html).unwrap();
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.0.join(name)).unwrap()
    }

    fn publish(&self) {
        let output = Command::new(env!("CARGO_BIN_EXE_canonical"))
            .arg(&self.0)
            .arg("https://example.org/book/")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

impl Drop for Site {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn social_metadata_covers_chapters_and_standalone_playground() {
    let site = Site::new();
    let html = "<!doctype html><html><head><title>LaTeX &amp; \"BibTeX\"</title>\
        <meta name=\"description\" content=\"Format &amp; lint.\"></head>\
        <body><p>Content</p></body></html>";
    for name in ["index.html", "guide/setup.html", "playground/index.html"] {
        site.write(name, html);
    }
    site.publish();
    for (name, url) in [
        ("index.html", "https://example.org/book/"),
        (
            "guide/setup.html",
            "https://example.org/book/guide/setup.html",
        ),
        (
            "playground/index.html",
            "https://example.org/book/playground/",
        ),
    ] {
        let output = site.read(name);
        assert!(output.contains(&format!("property=\"og:url\" content=\"{url}\"")));
        assert!(output.contains("property=\"og:title\" content=\"LaTeX &amp; &quot;BibTeX&quot;\""));
        assert!(output.contains("property=\"og:description\" content=\"Format &amp; lint.\""));
        assert!(output
            .contains("property=\"og:image\" content=\"https://example.org/book/images/og.png\""));
        assert!(output.contains("name=\"twitter:card\" content=\"summary_large_image\""));
        assert!(output.contains("property=\"og:image:width\" content=\"2391\""));
        assert!(output.contains("<p>Content</p>"));
    }
    assert_eq!(
        std::fs::read(site.0.join("images/og.png")).unwrap(),
        std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../branding/og.png")).unwrap()
    );
    let first = site.read("index.html");
    site.publish();
    assert_eq!(first, site.read("index.html"));
}

#[test]
fn redirects_and_helper_pages_are_not_content_destinations() {
    let site = Site::new();
    site.write("index.html", "<head><title>Home</title></head>");
    site.write(
        "old.html",
        "<head><meta content='0; URL=index.html' HTTP-EQUIV='Refresh'>\
        <link rel='canonical' href='index.html'></head>",
    );
    for name in ["404.html", "print.html", "toc.html"] {
        site.write(name, "<head><title>Helper</title></head><body><label id=\"mdbook-sidebar-toggle\" for=\"mdbook-sidebar-toggle-anchor\">Menu</label></body>");
    }
    let pages = doc_utils::postbuild::collect_pages(&site.0);
    assert_eq!(
        pages.iter().map(|p| p.loc.as_str()).collect::<Vec<_>>(),
        [""]
    );
    let redirect = site.read("old.html");
    site.publish();
    assert_eq!(redirect, site.read("old.html"));
    assert!(!site.read("print.html").contains("og:image"));
    assert!(site.read("print.html").contains("<button"));
}

#[test]
fn existing_canonical_does_not_prevent_social_metadata() {
    let site = Site::new();
    site.write(
        "index.html",
        "<head><title>Home</title>\
        <link rel=\"canonical\" href=\"https://example.org/book/\"></head>",
    );
    site.publish();
    let html = site.read("index.html");
    assert_eq!(html.matches("rel=\"canonical\"").count(), 1);
    assert_eq!(html.matches("property=\"og:image\"").count(), 1);
}

#[test]
fn navigation_toggle_is_a_native_button_without_changing_other_labels() {
    let site = Site::new();
    site.write("index.html", "<head><title>Home</title></head><body>\
        <label id=\"mdbook-sidebar-toggle\" for=\"mdbook-sidebar-toggle-anchor\" aria-label=\"Contents\"><span>Menu</span></label>\
        <label for=\"search\">Search</label></body>");
    site.publish();
    let html = site.read("index.html");
    assert!(html.contains("<button id=\"mdbook-sidebar-toggle\""));
    assert!(html.contains("<span>Menu</span></button>"));
    assert!(!html.contains("for=\"mdbook-sidebar-toggle-anchor\""));
    assert!(html.contains("<label for=\"search\">Search</label>"));
}
