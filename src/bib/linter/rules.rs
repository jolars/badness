//! The bib rule abstraction: the [`BibRule`] trait every bib lint implements, the
//! [`BibRuleContext`] handed to it, and the registry of built-in bib rules.
//!
//! Structurally a copy of [`crate::linter::rules`], retyped to the bib
//! [`SyntaxKind`] and the bib [`Model`]. The context additionally carries the
//! [`BibFieldDb`] — the required/optional-field signatures that drive
//! `missing-required-field` and `unknown-field` — since unlike the LaTeX side
//! there is no separate semantic-model handle for it.
//!
//! No `resolution` field yet: every rule in this slice is single-file-sound, so
//! there is no cross-file gate to thread (the way `undefined-ref` needs one). When
//! a cross-file rule lands (`undefined-string`, Phase 4) it gains the equivalent of
//! [`crate::linter::rules::RuleContext::resolution`].
//!
//! [`SyntaxKind`]: crate::bib::syntax::SyntaxKind
//! [`Model`]: crate::bib::semantic::Model
//! [`BibFieldDb`]: crate::bib::semantic::BibFieldDb

use std::path::Path;

use crate::bib::semantic::{BibFieldDb, Model};
use crate::bib::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};
use crate::linter::diagnostic::{Diagnostic, Severity};
// The example shape is language-agnostic (a caption plus a source snippet), so the
// bib rules reuse the LaTeX linter's `Example` rather than duplicating it.
pub use crate::linter::rules::Example;

pub mod duplicate_field;
pub mod duplicate_key;
pub mod edits;
pub mod empty_field;
pub mod encoding_hints;
pub mod missing_required_field;
pub mod title_capitalization;
pub mod undefined_string;
pub mod unknown_field;
pub mod unused_string;

pub use duplicate_field::DuplicateField;
pub use duplicate_key::DuplicateKey;
pub use empty_field::EmptyField;
pub use encoding_hints::EncodingHints;
pub use missing_required_field::MissingRequiredField;
pub use title_capitalization::TitleCapitalization;
pub use undefined_string::UndefinedString;
pub use unknown_field::UnknownField;
pub use unused_string::UnusedString;

/// Everything a [`BibRule`] reads to produce diagnostics for one `.bib` file.
///
/// `path` is informational (rules may name the file in a message); the driver
/// still stamps each diagnostic's `path` afterward, so rules construct diagnostics
/// with an empty path.
pub struct BibRuleContext<'a> {
    pub path: &'a Path,
    pub root: &'a SyntaxNode,
    pub model: &'a Model,
    /// The built-in field/entry signature database ([`crate::bib::semantic::builtin`]).
    pub db: &'a BibFieldDb,
}

/// A single bib lint. `Send + Sync` so the registry can be shared across a future
/// LSP read pool, matching [`crate::linter::rules::Rule`].
///
/// Rules come in two flavors, both driven by [`lint_document`](super::check::lint_document)'s
/// single shared traversal:
///
/// - **Node-shape rules** subscribe to [`BibRule::interests`] and implement
///   [`BibRule::check`]; the driver invokes `check` once per visited element whose
///   kind they named.
/// - **Whole-file rules** leave `interests` empty and implement
///   [`BibRule::check_file`]; the driver calls it once, after the walk. This is for
///   rules driven by the semantic [`Model`](crate::bib::semantic::Model).
pub trait BibRule: Send + Sync {
    /// The stable, kebab-case identifier reported as the diagnostic's `rule`.
    fn id(&self) -> &'static str;

    /// The severity a rule emits unless it overrides per-finding.
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    /// One-paragraph (markdown) description of what the rule flags and why, used
    /// to generate the rule reference. Empty means "not yet documented"; the
    /// `every_bib_rule_is_documented` test (`tests/bib_rule_docs.rs`) requires a
    /// non-empty value for every shipped rule.
    fn description(&self) -> &'static str {
        ""
    }

    /// Worked examples for the rule reference. Each `source` is linted live and
    /// rendered with its diagnostics (and autofix before/after) by
    /// [`crate::bib::linter::docs::render_rule_doc`]. The default is empty; the
    /// docs tests require at least one example per rule, and that each one
    /// actually triggers the rule.
    fn examples(&self) -> &'static [Example] {
        &[]
    }

    /// The bib `SyntaxKind`s this rule subscribes to. The default (`&[]`) opts out
    /// of node dispatch entirely — appropriate for whole-file rules.
    fn interests(&self) -> &'static [SyntaxKind] {
        &[]
    }

    /// Per-element callback, invoked for each CST element whose kind is in
    /// [`BibRule::interests`]. Node-shape rules unwrap `el.as_node()`. Findings are
    /// pushed onto `sink` with the path left empty.
    fn check(&self, el: &SyntaxElement, ctx: &BibRuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let _ = (el, ctx, sink);
    }

    /// Whole-file pass, run once after the shared traversal. Findings are pushed
    /// onto `sink` with the path left empty.
    fn check_file(&self, ctx: &BibRuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let _ = (ctx, sink);
    }
}

/// Every built-in bib rule, in registry order.
pub fn all_rules() -> Vec<Box<dyn BibRule>> {
    vec![
        Box::new(DuplicateKey),
        Box::new(MissingRequiredField),
        Box::new(UnknownField),
        Box::new(EmptyField),
        Box::new(DuplicateField),
        Box::new(UnusedString),
        Box::new(UndefinedString),
        Box::new(TitleCapitalization),
        Box::new(EncodingHints),
    ]
}

/// The ids of every built-in bib rule. Kept in lockstep with [`all_rules`].
pub const ALL_BIB_RULE_IDS: &[&str] = &[
    "duplicate-key",
    "missing-required-field",
    "unknown-field",
    "empty-field",
    "duplicate-field",
    "unused-string",
    "undefined-string",
    "title-capitalization",
    "encoding-hints",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_and_id_list_agree() {
        let ids: Vec<&str> = all_rules().iter().map(|r| r.id()).collect();
        assert_eq!(ids, ALL_BIB_RULE_IDS);
    }
}
