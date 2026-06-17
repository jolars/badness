//! Linter: diagnostics over the lossless CST.
//!
//! Beyond surfacing **parse diagnostics** (the parser's byte-range error side
//! channel), the linter owns a small set of rules of its own ([`rules`]),
//! comment suppression (`% badness-ignore` style — [`suppression`]), a driver
//! ([`check::lint_document`]) that both the CLI and the language server call, and
//! an autofix engine ([`fix::apply_fixes`]) backing `lint --fix`. The module is
//! kept close to arity's `src/linter/` shape so the eventual shared-crate
//! extraction stays a mechanical lift (see `AGENTS.md`). **[copy shape]**

pub mod check;
pub mod diagnostic;
pub mod fix;
pub mod render;
pub mod rules;
pub mod suppression;

pub use check::{check_document, lint_document};
pub use diagnostic::{Applicability, Diagnostic, Fix, Severity};
pub use fix::{FixOutcome, apply_fixes};
pub use render::{OutputMode, render_findings};
