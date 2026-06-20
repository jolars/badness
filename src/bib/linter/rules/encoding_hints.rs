//! `encoding-hints`: non-ASCII text in a field value.
//!
//! Raw non-ASCII bytes in a `.bib` file render correctly only when the file is
//! UTF-8 *and* the consuming document loads the right input encoding
//! (`\usepackage[utf8]{inputenc}` with pdfLaTeX, or `fontspec` with Xe/LuaLaTeX).
//! Legacy BibTeX toolchains often mangle them. This rule surfaces each non-ASCII
//! run as a [`Severity::Hint`] (not a warning — accented text is perfectly valid in
//! a UTF-8 setup) suggesting either confirming the encoding or using a LaTeX escape
//! (`\'e` for `é`). Report-only: the right fix depends on the project's toolchain.
//!
//! Only field *values* are scanned; one hint is emitted per maximal non-ASCII run.

use std::path::PathBuf;

use crate::bib::ast::field_value;
use crate::bib::syntax::{SyntaxElement, SyntaxKind};
use crate::linter::diagnostic::{Diagnostic, Severity};

use super::{BibRule, BibRuleContext};

pub struct EncodingHints;

impl BibRule for EncodingHints {
    fn id(&self) -> &'static str {
        "encoding-hints"
    }

    fn default_severity(&self) -> Severity {
        Severity::Hint
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::FIELD]
    }

    fn check(&self, el: &SyntaxElement, _ctx: &BibRuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(field) = el.as_node() else {
            return;
        };
        let Some(value) = field_value(field) else {
            return;
        };
        let text = value.to_string();
        let base = usize::from(value.text_range().start());
        for (start, end, run) in non_ascii_runs(&text) {
            sink.push(Diagnostic {
                rule: self.id(),
                severity: self.default_severity(),
                path: PathBuf::new(),
                start: base + start,
                end: base + end,
                message: format!(
                    "non-ASCII text `{run}`; ensure the file is UTF-8 and the document loads an \
                     input encoding (inputenc/fontspec), or use a LaTeX escape"
                ),
                fix: None,
            });
        }
    }
}

/// Byte ranges (relative to `text`) of maximal runs of non-ASCII characters, with
/// the matched substring.
fn non_ascii_runs(text: &str) -> Vec<(usize, usize, String)> {
    let mut runs = Vec::new();
    let mut run_start: Option<usize> = None;
    for (off, ch) in text.char_indices() {
        if ch.is_ascii() {
            if let Some(start) = run_start.take() {
                runs.push((start, off, text[start..off].to_string()));
            }
        } else if run_start.is_none() {
            run_start = Some(off);
        }
    }
    if let Some(start) = run_start {
        runs.push((start, text.len(), text[start..].to_string()));
    }
    runs
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
            if EncodingHints.interests().contains(&el.kind()) {
                EncodingHints.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_accented_text() {
        let out = findings("@article{k, author = {Erdős}}\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "encoding-hints");
        assert_eq!(out[0].severity, Severity::Hint);
    }

    #[test]
    fn ascii_only_is_fine() {
        assert!(findings("@article{k, author = {Erdos}}\n").is_empty());
    }

    #[test]
    fn coalesces_adjacent_non_ascii() {
        // Two adjacent non-ASCII chars are one run, not two findings.
        let out = findings("@article{k, title = {Café — bar}}\n");
        // "é" then later "—": two separate runs.
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn underlines_the_run() {
        let src = "@article{k, author = {Erdős}}\n";
        let out = findings(src);
        assert_eq!(out.len(), 1);
        assert_eq!(&src[out[0].start..out[0].end], "ő");
    }
}
