//! `textDocument/documentLink` for `.bib` files: turn `doi`/`url` field values into
//! clickable external links.
//!
//! BibTeX has no include structure (the LaTeX document-link walk in
//! [`crate::lsp::document_link`] finds none), but two verbatim fields carry a web
//! resource an editor otherwise cannot follow: a `doi` holds a bare DOI (e.g.
//! `10.1000/xyz`, which resolves under `https://doi.org/`) and a `url` holds a full
//! address. We surface each as a link over the value span.
//!
//! Purely syntactic and hermetic (AGENTS.md non-goals): the target is a pure string
//! transform of the field value—no network, no environment. Conservative by
//! construction: only a single braced or quoted value piece is linked (a
//! macro-concatenated `#` value or a bare `@string` reference is skipped, since its
//! text is not the literal URL), and a `url` must already read as an `http(s)`
//! address, so non-URL prose never becomes a bogus link.

use rowan::{TextRange, TextSize};

use crate::bib::ast::{field_name, field_value};
use crate::bib::syntax::{SyntaxKind, SyntaxNode};

/// A clickable link in a `.bib` file: the source span to underline and the absolute
/// `http(s)` URL it targets. Kept free of LSP/URI types so the walk stays
/// unit-testable; the caller ([`crate::lsp`]) maps `target` to an `lsp_types::Uri`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BibLink {
    /// Byte range of the value text (delimiters excluded, whitespace trimmed) to
    /// underline.
    pub range: TextRange,
    /// The absolute URL to open.
    pub target: String,
}

/// Collect the `doi`/`url` links in `root`, in source order.
pub fn document_links(root: &SyntaxNode) -> Vec<BibLink> {
    let mut links = Vec::new();
    for field in root
        .descendants()
        .filter(|node| node.kind() == SyntaxKind::FIELD)
    {
        let Some(name) = field_name(&field) else {
            continue;
        };
        let Some(kind) = LinkField::classify(&name) else {
            continue;
        };
        let Some(value) = field_value(&field) else {
            continue;
        };
        let Some((inner, range)) = single_delimited_value(&value) else {
            continue;
        };
        if let Some(target) = kind.target(&inner) {
            links.push(BibLink { range, target });
        }
    }
    links
}

/// The two verbatim fields we turn into links.
#[derive(Clone, Copy)]
enum LinkField {
    /// `doi` — a bare DOI resolved under `https://doi.org/`.
    Doi,
    /// `url` — a full address, linked as-is when it is `http(s)`.
    Url,
}

impl LinkField {
    /// Classify a field name (case-insensitively); `None` for a field we don't link.
    fn classify(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "doi" => Some(Self::Doi),
            "url" => Some(Self::Url),
            _ => None,
        }
    }

    /// Build the absolute target URL from the trimmed value text, or `None` when it
    /// does not yield an `http(s)` URL.
    fn target(self, value: &str) -> Option<String> {
        match self {
            // Some authors store the resolver URL or a `doi:` prefix in the field;
            // strip either back to the bare DOI before rebuilding a canonical link.
            Self::Doi => {
                let doi = value
                    .trim_start_matches("https://doi.org/")
                    .trim_start_matches("http://doi.org/")
                    .trim_start_matches("https://dx.doi.org/")
                    .trim_start_matches("http://dx.doi.org/")
                    .trim_start_matches("doi:")
                    .trim();
                // A DOI always begins `10.`; reject anything else (a stray macro, a
                // half-authored field) rather than mint a dead `doi.org` link. A DOI
                // has no internal whitespace, so a space signals prose, not a DOI.
                (doi.starts_with("10.") && !doi.contains(char::is_whitespace))
                    .then(|| format!("https://doi.org/{doi}"))
            }
            Self::Url => (value.starts_with("http://") || value.starts_with("https://"))
                .then(|| value.to_owned()),
        }
    }
}

/// The inner text of a value that is exactly one braced or quoted piece, with its
/// byte range (delimiters excluded, leading/trailing whitespace trimmed). `None` for
/// a bare macro/number piece, a multi-piece (`#`-concatenated) value, or an
/// all-whitespace body—none of which yield a stable link.
fn single_delimited_value(value: &SyntaxNode) -> Option<(String, TextRange)> {
    let mut pieces = value.children();
    let piece = pieces.next()?;
    if pieces.next().is_some() {
        return None;
    }
    let (open, close) = match piece.kind() {
        SyntaxKind::BRACE_GROUP => ('{', '}'),
        SyntaxKind::QUOTED => ('"', '"'),
        _ => return None,
    };
    let text = piece.to_string();
    let inner = text.strip_prefix(open)?.strip_suffix(close)?;
    let lead = inner.len() - inner.trim_start().len();
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        return None;
    }
    let start = usize::from(piece.text_range().start()) + open.len_utf8() + lead;
    let range = TextRange::at(
        TextSize::new(start as u32),
        TextSize::new(trimmed.len() as u32),
    );
    Some((trimmed.to_owned(), range))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bib::parse;

    fn links_of(src: &str) -> Vec<BibLink> {
        document_links(&parse(src).syntax())
    }

    /// The source substring each link underlines, paired with its target.
    fn spans(src: &str) -> Vec<(&str, String)> {
        links_of(src)
            .into_iter()
            .map(|link| (&src[link.range], link.target))
            .collect()
    }

    #[test]
    fn doi_becomes_resolver_link() {
        let src = "@article{k, doi = {10.1000/xyz}}\n";
        assert_eq!(
            spans(src),
            vec![("10.1000/xyz", "https://doi.org/10.1000/xyz".to_owned())]
        );
    }

    #[test]
    fn quoted_url_is_linked_verbatim() {
        let src = "@misc{k, url = \"https://example.com/a\"}\n";
        assert_eq!(
            spans(src),
            vec![("https://example.com/a", "https://example.com/a".to_owned())]
        );
    }

    #[test]
    fn doi_field_name_is_case_insensitive() {
        let src = "@article{k, DOI = {10.5/AB}}\n";
        assert_eq!(
            spans(src),
            vec![("10.5/AB", "https://doi.org/10.5/AB".to_owned())]
        );
    }

    #[test]
    fn doi_resolver_prefix_is_stripped() {
        let src = "@article{k, doi = {https://doi.org/10.1/x}}\n";
        assert_eq!(
            spans(src),
            vec![(
                "https://doi.org/10.1/x",
                "https://doi.org/10.1/x".to_owned()
            )]
        );
    }

    #[test]
    fn doi_colon_prefix_is_stripped() {
        let src = "@article{k, doi = {doi:10.1/x}}\n";
        assert_eq!(
            spans(src),
            vec![("doi:10.1/x", "https://doi.org/10.1/x".to_owned())]
        );
    }

    #[test]
    fn non_doi_value_is_not_linked() {
        // Prose in a `doi` field must not mint a dead resolver link.
        assert!(links_of("@article{k, doi = {see the website}}\n").is_empty());
    }

    #[test]
    fn non_http_url_is_not_linked() {
        assert!(links_of("@misc{k, url = {example.com}}\n").is_empty());
    }

    #[test]
    fn bare_macro_value_is_skipped() {
        // `url = homepage` is an `@string` reference, not the literal URL text.
        assert!(links_of("@misc{k, url = homepage}\n").is_empty());
    }

    #[test]
    fn concatenated_value_is_skipped() {
        assert!(links_of("@misc{k, url = base # {/path}}\n").is_empty());
    }

    #[test]
    fn empty_value_is_skipped() {
        assert!(links_of("@misc{k, url = {}}\n").is_empty());
    }

    #[test]
    fn surrounding_whitespace_is_trimmed_from_span() {
        let src = "@misc{k, url = { https://x.io }}\n";
        assert_eq!(
            spans(src),
            vec![("https://x.io", "https://x.io".to_owned())]
        );
    }

    #[test]
    fn other_fields_yield_no_links() {
        assert!(links_of("@article{k, title = {A}, year = 2020}\n").is_empty());
    }
}
