//! `title-capitalization`: an unprotected acronym (or mid-word capital) in a
//! title-like field.
//!
//! Many bibliography styles lowercase title text that is not brace-protected, so an
//! acronym like `DNA` renders as `dna` unless written `{DNA}`. This rule flags
//! capitals that such a style would clobber. A [`Severity::Warning`]; report-only
//! (wrapping in braces is a meaning-preserving edit, but choosing *what* to protect
//! is the author's call).
//!
//! **Acronym heuristic (not strict).** Only capitals likely to be acronyms are
//! flagged, to stay quiet on ordinary Title-Cased databases:
//! - a run of **two or more** consecutive capitals (`DNA`, any `[A-Z]{2,}` run), or
//! - a **single** capital in the *middle* of a **lowercase-initial** word — the
//!   camelCase brand pattern (`iPhone`'s `P`, `eBay`'s `B`, `pH`'s `H`).
//!
//! A lone capital *starting* a word (`The`, `Quick`) is left alone. So is a single
//! interior capital in a word that **already contains a capital**: that is the
//! surname-particle pattern (`McDonald`, `DeForest`, `MacArthur`) or a mixed-case
//! token (`LaTeX`), where the interior capital is a proper name or style choice, not
//! a clobbered acronym. Distinguishing these from a real acronym without a word list
//! is impossible in general, so the rule stays quiet on the capital-initial shape and
//! accepts the occasional miss (e.g. a cell line like `HeLa`).
//!
//! Only text at brace depth 0 inside the value is considered — content already inside
//! a nested `{…}` group is protected and skipped. Bare `LITERAL` pieces
//! (macros/numbers) are not scanned.

use std::path::PathBuf;

use crate::bib::ast::{field_name, field_value};
use crate::bib::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};
use crate::linter::diagnostic::{Diagnostic, Severity};

use super::{BibRule, BibRuleContext, Example};

/// Field names whose value is title-like prose subject to style lowercasing.
const TITLE_FIELDS: &[&str] = &[
    "title",
    "subtitle",
    "booktitle",
    "booksubtitle",
    "maintitle",
    "mainsubtitle",
    "journaltitle",
    "issuetitle",
    "issuesubtitle",
    "eventtitle",
    "shorttitle",
];

const EXAMPLES: &[Example] = &[Example {
    caption: "An unprotected acronym a title-lowercasing style would render as `dna`:",
    source: "@article{watson53, title = {Molecular structure of DNA}}\n",
}];

pub struct TitleCapitalization;

impl BibRule for TitleCapitalization {
    fn id(&self) -> &'static str {
        "title-capitalization"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag an unprotected acronym or mid-word capital in a title-like field \
         (`title`, `booktitle`, `journaltitle`, ...). Many bibliography styles \
         lowercase unprotected title text, so `DNA` renders as `dna` unless \
         written `{DNA}`. Flagged are runs of two or more capitals and the \
         camelCase brand pattern (a first capital mid-way through a \
         lowercase-initial word, like `iPhone`); ordinary Title Case, name \
         particles (`McDonald`), and mixed-case tokens (`LaTeX`) stay quiet, \
         as does anything already inside a `{...}` group. Report-only -- \
         choosing what to protect is the author's call."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::FIELD]
    }

    fn check(&self, el: &SyntaxElement, _ctx: &BibRuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(field) = el.as_node() else {
            return;
        };
        let Some(name) = field_name(field) else {
            return;
        };
        let name_lc = name.to_lowercase();
        if !TITLE_FIELDS.contains(&name_lc.as_str()) {
            return;
        }
        let Some(value) = field_value(field) else {
            return;
        };
        for piece in value.children() {
            let (inner, base) = match piece.kind() {
                SyntaxKind::BRACE_GROUP => match inner_of(&piece, '{', '}') {
                    Some(parts) => parts,
                    None => continue,
                },
                SyntaxKind::QUOTED => match inner_of(&piece, '"', '"') {
                    Some(parts) => parts,
                    None => continue,
                },
                // Bare LITERAL pieces are macros/numbers, not title prose.
                _ => continue,
            };
            for (start, end, run) in unprotected_acronyms(&inner) {
                sink.push(Diagnostic {
                    rule: self.id(),
                    severity: self.default_severity(),
                    path: PathBuf::new(),
                    start: base + start,
                    end: base + end,
                    message: format!(
                        "unprotected capitals `{run}` in `{name_lc}`; wrap in braces (`{{{run}}}`) \
                         to keep case under title-lowercasing styles"
                    ),
                    fix: None,
                    related: Vec::new(),
                });
            }
        }
    }
}

/// The inner text of a delimited piece (between `open` and `close`) and the byte
/// offset of that inner text in the document. Returns `None` for a piece missing
/// its closing delimiter (a recovery artifact) so a partial group is not scanned.
fn inner_of(node: &SyntaxNode, open: char, close: char) -> Option<(String, usize)> {
    let text = node.to_string();
    let stripped = text.strip_prefix(open)?.strip_suffix(close)?;
    let base = usize::from(node.text_range().start()) + open.len_utf8();
    Some((stripped.to_string(), base))
}

/// Find the byte ranges (relative to the start of `text`) of acronym-like capital
/// runs at brace depth 0, with the matched substring. See the module docs for the
/// heuristic.
fn unprotected_acronyms(text: &str) -> Vec<(usize, usize, String)> {
    let mut hits = Vec::new();
    let mut depth: i32 = 0;
    // State tracked across the current word (a maximal run of alphabetic chars at
    // depth 0): whether the previous char was a letter (to detect a mid-word capital)
    // and whether this word has already contained a capital (to tell the camelCase
    // brand pattern `iPhone` apart from a surname particle like `McDonald`).
    let mut prev_alpha = false;
    let mut word_has_upper = false;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let (off, ch) = chars[i];
        match ch {
            '{' => {
                depth += 1;
                prev_alpha = false;
                word_has_upper = false;
                i += 1;
                continue;
            }
            '}' => {
                depth -= 1;
                prev_alpha = false;
                word_has_upper = false;
                i += 1;
                continue;
            }
            _ => {}
        }
        if depth == 0 && ch.is_ascii_uppercase() {
            // Extend a maximal run of consecutive ASCII capitals at this depth.
            let run_start = off;
            // A single interior capital counts only when it is the *first* capital of
            // a lowercase-initial word; a later capital is a name/style token.
            let midword_single = prev_alpha && !word_has_upper;
            let mut j = i;
            while j < chars.len() && chars[j].1.is_ascii_uppercase() {
                j += 1;
            }
            let run_len = j - i;
            let run_end = chars.get(j).map(|&(o, _)| o).unwrap_or(text.len());
            if run_len >= 2 || midword_single {
                hits.push((run_start, run_end, text[run_start..run_end].to_string()));
            }
            prev_alpha = true;
            word_has_upper = true;
            i = j;
            continue;
        }
        let alpha = depth == 0 && ch.is_alphabetic();
        if !alpha {
            // Word boundary: reset the per-word capital flag.
            word_has_upper = false;
        }
        prev_alpha = alpha;
        i += 1;
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bib::parse;
    use crate::bib::semantic::Model;

    fn findings(src: &str) -> Vec<Diagnostic> {
        let root = parse(src).syntax();
        let model = Model::build(&root);
        let ctx = BibRuleContext {
            path: std::path::Path::new("x.bib"),
            root: &root,
            model: &model,
            db: crate::bib::semantic::builtin(),
        };
        let mut out = Vec::new();
        for el in root.descendants_with_tokens() {
            if TitleCapitalization.interests().contains(&el.kind()) {
                TitleCapitalization.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_bare_acronym() {
        let out = findings("@article{k, title = {The DNA helix}}\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "title-capitalization");
        assert!(out[0].message.contains("DNA"));
    }

    #[test]
    fn braced_acronym_is_protected() {
        assert!(findings("@article{k, title = {The {DNA} helix}}\n").is_empty());
    }

    #[test]
    fn ordinary_title_case_is_fine() {
        assert!(findings("@article{k, title = {The Quick Brown Fox}}\n").is_empty());
    }

    #[test]
    fn flags_midword_capital() {
        let out = findings("@article{k, title = {The iPhone era}}\n");
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains('P'));
    }

    #[test]
    fn flags_lowercase_initial_camelcase() {
        // The brand pattern: lowercase-initial word, first capital mid-word.
        assert_eq!(
            findings("@article{k, title = {The eBay auction}}\n").len(),
            1
        );
        assert_eq!(
            findings("@article{k, title = {Measuring pH levels}}\n").len(),
            1
        );
    }

    #[test]
    fn ignores_camelcase_surname() {
        // A single interior capital in a capital-initial word is a name particle,
        // not a clobbered acronym: the word already starts with a capital.
        assert!(findings("@article{k, title = {The McDonald study}}\n").is_empty());
        assert!(findings("@article{k, title = {On DeForest triodes}}\n").is_empty());
        assert!(findings("@article{k, title = {The MacArthur grant}}\n").is_empty());
    }

    #[test]
    fn ignores_mixed_case_style_token() {
        // `LaTeX`: capital-initial, so the interior `T`/`X` are style, not acronyms.
        assert!(findings("@article{k, title = {Typesetting with LaTeX}}\n").is_empty());
    }

    #[test]
    fn still_flags_uppercase_run_in_capital_initial_word() {
        // A 2+ capital run stays flagged even when the word is capital-initial.
        let out = findings("@article{k, title = {The QChem package}}\n");
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains("QC"));
    }

    #[test]
    fn ignores_non_title_field() {
        assert!(findings("@article{k, author = {DNA Smith}}\n").is_empty());
    }

    #[test]
    fn flags_in_quoted_value() {
        let out = findings("@article{k, title = \"A DNA study\"}\n");
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn quoted_with_braced_acronym_is_protected() {
        assert!(findings("@article{k, title = \"A {DNA} study\"}\n").is_empty());
    }

    #[test]
    fn underlines_the_run() {
        let src = "@article{k, title = {The DNA helix}}\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(&src[out[0].start..out[0].end], "DNA");
    }

    #[test]
    fn flags_each_acronym() {
        let out = findings("@article{k, title = {RNA and DNA}}\n");
        assert_eq!(out.len(), 2);
    }
}
