//! The rule abstraction: the [`Rule`] trait every lint implements, the
//! [`RuleContext`] handed to it, and the registry of built-in rules.
//!
//! Mirrors arity's `linter/rules.rs`, trimmed to what this first slice needs:
//! there is no config layer yet (badness has none), so every rule is always on
//! and there is no `select`/`ignore` resolution (arity's `ResolvedRules`).

use std::path::Path;

use crate::project::ResolvedLabels;
use crate::semantic::SemanticModel;
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};

use super::diagnostic::{Diagnostic, Severity};

pub mod deprecated_command;
pub mod dollar_display_math;
pub mod duplicate_label;
pub mod mismatched_delimiter;
pub mod obsolete_environment;
pub mod undefined_ref;

pub use deprecated_command::DeprecatedCommand;
pub use dollar_display_math::DollarDisplayMath;
pub use duplicate_label::DuplicateLabel;
pub use mismatched_delimiter::MismatchedDelimiter;
pub use obsolete_environment::ObsoleteEnvironment;
pub use undefined_ref::UndefinedRef;

/// Everything a [`Rule`] reads to produce diagnostics for one file.
///
/// `path` is informational (rules may name the file in a message); the driver
/// still stamps each diagnostic's `path` afterward, so rules construct
/// diagnostics with an empty path.
pub struct RuleContext<'a> {
    pub path: &'a Path,
    pub root: &'a SyntaxNode,
    pub model: &'a SemanticModel,
    /// Cross-file label resolution for the project `path` belongs to, or `None`
    /// when there is no project view (stdin, or a context — like the language
    /// server today — that hasn't assembled one). Cross-file rules are inert when
    /// this is `None`. `path` keys into it to find this file's label namespace.
    pub resolution: Option<&'a ResolvedLabels>,
}

/// A single lint. `Send + Sync` so the registry can be shared across the LSP's
/// read pool.
///
/// Rules come in two flavors, both driven by [`lint_document`](super::check::lint_document)'s
/// single shared traversal:
///
/// - **Node-shape rules** subscribe to [`Rule::interests`] and implement
///   [`Rule::check`]; the driver invokes `check` once per visited element whose
///   kind they named. They never walk the tree themselves.
/// - **Whole-file rules** leave `interests` empty and implement
///   [`Rule::check_file`]; the driver calls it once, after the walk. This is for
///   rules driven by the semantic model or cross-file resolution rather than by
///   node shape.
pub trait Rule: Send + Sync {
    /// The stable, kebab-case identifier reported as the diagnostic's `rule` and
    /// targeted by `% badness-ignore <id>`.
    fn id(&self) -> &'static str;

    /// The severity a rule emits unless it overrides per-finding.
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    /// The `SyntaxKind`s this rule subscribes to. During the driver's single
    /// shared traversal, [`Rule::check`] is invoked once for every element whose
    /// kind appears here. The default (`&[]`) opts out of node dispatch entirely —
    /// appropriate for rules that work off the whole file via [`Rule::check_file`].
    fn interests(&self) -> &'static [SyntaxKind] {
        &[]
    }

    /// Per-element callback, invoked for each CST element (node *or* token) whose
    /// kind is in [`Rule::interests`]. Node-shape rules unwrap `el.as_node()`.
    /// Findings are pushed onto `sink` with the path left empty.
    fn check(&self, el: &SyntaxElement, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let _ = (el, ctx, sink);
    }

    /// Whole-file pass, run once after the shared traversal. For rules driven by
    /// the semantic model or cross-file resolution rather than node shape. The
    /// default is a no-op. Findings are pushed onto `sink` with the path left empty.
    fn check_file(&self, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let _ = (ctx, sink);
    }
}

/// Every built-in rule, in registry order.
pub fn all_rules() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(DuplicateLabel),
        Box::new(DeprecatedCommand),
        Box::new(ObsoleteEnvironment),
        Box::new(DollarDisplayMath),
        Box::new(MismatchedDelimiter),
        Box::new(UndefinedRef),
    ]
}

/// The ids of every built-in rule. Kept in lockstep with [`all_rules`].
pub const ALL_RULE_IDS: &[&str] = &[
    "duplicate-label",
    "deprecated-command",
    "obsolete-environment",
    "dollar-display-math",
    "mismatched-delimiter",
    "undefined-ref",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_and_id_list_agree() {
        let ids: Vec<&str> = all_rules().iter().map(|r| r.id()).collect();
        assert_eq!(ids, ALL_RULE_IDS);
    }
}
