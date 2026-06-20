//! BibTeX linter: diagnostics over the bib CST.
//!
//! The bib analog of [`crate::linter`], mirroring its shape exactly (a [`BibRule`]
//! trait, a kind-indexed driver in [`check::lint_document`], and the per-file
//! entry point [`check::check_document`]) but typed to the bib [`SyntaxKind`] and
//! [`Model`]. It **reuses the language-agnostic diagnostic surface** wholesale —
//! [`Diagnostic`]/[`Fix`]/[`Severity`] and the [`apply_fixes`] engine live in
//! [`crate::linter`] and are byte-offset based, so nothing about them is
//! LaTeX-specific. Only the rule trait, the context (which carries the bib `Model`
//! plus the field DB), and the rules themselves are bib-specific.
//!
//! Like the bib formatter mirrors the LaTeX formatter, this is a parallel module
//! rather than a generalization of [`crate::linter::rules::Rule`] — "copy now,
//! extract later" (`AGENTS.md`). A future shared linter-core crate parameterized
//! over kind + context would lift both sides mechanically.
//!
//! **Suppression** is carried in `@comment{badness-ignore …}` entries (bib has no
//! `%` line-comment token), parsed by [`suppression::BibSuppressionMap`] — the bib
//! analog of the LaTeX `% badness-ignore` directive.
//!
//! [`SyntaxKind`]: crate::bib::syntax::SyntaxKind
//! [`Model`]: crate::bib::semantic::Model
//! [`Diagnostic`]: crate::linter::Diagnostic
//! [`Fix`]: crate::linter::Fix
//! [`Severity`]: crate::linter::Severity
//! [`apply_fixes`]: crate::linter::apply_fixes

pub mod check;
pub mod rules;
pub mod suppression;

pub use check::{check_document, lint_document};
pub use rules::{ALL_BIB_RULE_IDS, BibRule, BibRuleContext, all_rules};
pub use suppression::BibSuppressionMap;

// The diagnostic surface is shared with the LaTeX linter; re-export it here so bib
// callers don't reach across into `crate::linter`.
pub use crate::linter::{Applicability, Diagnostic, Fix, FixOutcome, Severity, apply_fixes};
