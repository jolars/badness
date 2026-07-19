//! Linter: diagnostics over the lossless CST.
//!
//! Beyond surfacing **parse diagnostics** (the parser's byte-range error side
//! channel), the linter owns a small set of rules of its own ([`rules`]),
//! comment suppression (`% badness-ignore` style — [`suppression`]), a driver
//! ([`check::lint_document`]) that both the CLI and the language server call, and
//! an autofix engine ([`fix::apply_fixes`]) backing `lint --fix`.

pub mod check;
pub(crate) mod conditional;
pub mod diagnostic;
pub mod docs;
pub mod fix;
pub mod render;
pub mod rules;
pub mod suppression;

pub use check::{check_document, check_document_fixable, lint_document};
pub use diagnostic::{Applicability, Diagnostic, Edit, Fix, RelatedInfo, Severity};
pub use fix::{FixOutcome, apply_fixes};
pub use render::{OutputMode, render_findings};
pub use rules::RuleSelection;
