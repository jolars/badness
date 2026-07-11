//! The rule abstraction: the [`Rule`] trait every lint implements, the
//! [`RuleContext`] handed to it, and the registry of built-in rules.
//!
//! Every rule is on by default; the
//! `badness.toml` `[lint]` `select`/`ignore` keys (and the CLI's matching flags)
//! narrow the active set via [`RuleSelection`], applied as a post-filter so the
//! shared `lint_document` driver stays config-unaware.

use std::path::Path;
use std::sync::OnceLock;

use rowan::{TextRange, TextSize};

use crate::project::{ResolvedCitations, ResolvedLabels, ResolvedPackageOptions};
use crate::semantic::SemanticModel;
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};

use super::diagnostic::{Diagnostic, Severity};

pub mod abbreviation_spacing;
pub mod dash_length;
pub mod deprecated_command;
pub mod dollar_display_math;
pub mod duplicate_label;
pub mod duplicate_package;
pub mod ellipsis;
pub mod hard_coded_reference;
pub mod makeat_macro;
pub mod math_operator_name;
pub mod mismatched_delimiter;
pub mod missing_nonbreaking_space;
pub mod missing_provides;
pub mod missing_required_argument;
pub mod obsolete_environment;
pub mod primitive_command;
pub mod sectioning_level_jump;
pub mod space_before_command;
pub mod straight_quotes;
pub mod swallowed_space;
pub mod times_variable;
pub mod undefined_citation;
pub mod undefined_ref;
pub mod unknown_option;
pub mod unreferenced_label;
pub mod verbatim_trailing_text;

pub use abbreviation_spacing::AbbreviationSpacing;
pub use dash_length::DashLength;
pub use deprecated_command::DeprecatedCommand;
pub use dollar_display_math::DollarDisplayMath;
pub use duplicate_label::DuplicateLabel;
pub use duplicate_package::DuplicatePackage;
pub use ellipsis::Ellipsis;
pub use hard_coded_reference::HardCodedReference;
pub use makeat_macro::MakeatMacro;
pub use math_operator_name::MathOperatorName;
pub use mismatched_delimiter::MismatchedDelimiter;
pub use missing_nonbreaking_space::MissingNonbreakingSpace;
pub use missing_provides::MissingProvides;
pub use missing_required_argument::MissingRequiredArgument;
pub use obsolete_environment::ObsoleteEnvironment;
pub use primitive_command::PrimitiveCommand;
pub use sectioning_level_jump::SectioningLevelJump;
pub use space_before_command::SpaceBeforeCommand;
pub use straight_quotes::StraightQuotes;
pub use swallowed_space::SwallowedSpace;
pub use times_variable::TimesVariable;
pub use undefined_citation::UndefinedCitation;
pub use undefined_ref::UndefinedRef;
pub use unknown_option::UnknownOption;
pub use unreferenced_label::UnreferencedLabel;
pub use verbatim_trailing_text::VerbatimTrailingText;

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
    /// Cross-file citation resolution (cite keys reachable via the project's
    /// `.bib` resources), or `None` when there is no project view. Gates
    /// `undefined-citation`, the bibliographic analog of `resolution`.
    pub citations: Option<&'a ResolvedCitations>,
    /// The project's package-option model (each analyzed `.sty` member's
    /// statically-declared options), or `None` when there is no project view.
    /// Gates `unknown-option`, the load-graph analog of `resolution`.
    pub packages: Option<&'a ResolvedPackageOptions>,
    /// Disjoint byte ranges covered by `MATH` nodes, sorted by start. Computed
    /// once per file so the many rules that must ignore math (`e.g.` inside `$…$`
    /// is not sentence punctuation, a `-` there is not a dash, …) share one
    /// membership test ([`RuleContext::in_math`]) instead of each climbing the
    /// ancestor chain per token. Mirrors the formatter's `expl3_regions` side
    /// channel: a read-only, precomputed range set derived purely from the tree.
    math_regions: Vec<TextRange>,
}

impl<'a> RuleContext<'a> {
    /// Assemble the context for one file, precomputing the shared math-region
    /// index. `resolution`/`citations`/`packages` are `None` when there is no
    /// project view.
    pub fn new(
        path: &'a Path,
        root: &'a SyntaxNode,
        model: &'a SemanticModel,
        resolution: Option<&'a ResolvedLabels>,
        citations: Option<&'a ResolvedCitations>,
        packages: Option<&'a ResolvedPackageOptions>,
    ) -> Self {
        Self {
            path,
            root,
            model,
            resolution,
            citations,
            packages,
            math_regions: math_regions(root),
        }
    }

    /// Whether byte `offset` falls inside a `MATH` node. `O(log n)` over the
    /// precomputed disjoint regions — the shared replacement for the ad-hoc
    /// `parent_ancestors().any(MATH)` climbs the math-sensitive rules used to do.
    pub fn in_math(&self, offset: usize) -> bool {
        let offset = TextSize::from(offset as u32);
        // Find the last region starting at or before `offset`; it is the only one
        // that can contain it (regions are disjoint and sorted).
        match self
            .math_regions
            .binary_search_by(|r| r.start().cmp(&offset))
        {
            Ok(_) => true, // a region starts exactly here
            Err(0) => false,
            Err(i) => self.math_regions[i - 1].contains(offset),
        }
    }
}

/// Collect the disjoint byte ranges covered by `MATH` nodes, sorted by start.
/// Nested/adjacent `MATH` spans are coalesced so [`RuleContext::in_math`] can
/// binary-search a clean interval set.
fn math_regions(root: &SyntaxNode) -> Vec<TextRange> {
    let mut ranges: Vec<TextRange> = root
        .descendants()
        .filter(|node| node.kind() == SyntaxKind::MATH)
        .map(|node| node.text_range())
        .collect();
    ranges.sort_by_key(|r| r.start());
    let mut merged: Vec<TextRange> = Vec::with_capacity(ranges.len());
    for r in ranges {
        match merged.last_mut() {
            Some(last) if r.start() <= last.end() => {
                *last = TextRange::new(last.start(), last.end().max(r.end()));
            }
            _ => merged.push(r),
        }
    }
    merged
}

/// A documented example for a rule: a snippet of LaTeX that triggers it.
///
/// The rule reference (`docs/src/reference/linter-rules.md`) is generated by
/// running the *real* linter on `source`, so the rendered diagnostics and the
/// autofix "after" state are *derived* rather than stored — the snippet stays the
/// single source of truth (see [`crate::linter::docs`]).
pub struct Example {
    /// One-line caption rendered above the snippet (markdown). May be empty.
    pub caption: &'static str,
    /// LaTeX source that triggers the rule. Should end with a trailing newline.
    pub source: &'static str,
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

    /// One-paragraph (markdown) description of what the rule flags and why, used
    /// to generate the rule reference. Empty means "not yet documented"; the
    /// `every_rule_is_documented` test (`tests/rule_docs.rs`) requires a non-empty
    /// value for every shipped rule.
    fn description(&self) -> &'static str {
        ""
    }

    /// Worked examples for the rule reference. Each `source` is linted live and
    /// rendered with its diagnostics (and autofix before/after) by
    /// [`crate::linter::docs::render_rule_doc`]. The default is empty; the docs
    /// tests require at least one example per rule, and that each one actually
    /// triggers the rule.
    fn examples(&self) -> &'static [Example] {
        &[]
    }

    /// The synthetic filename an example snippet is linted as when rendering the
    /// rule reference. Defaults to `example.tex`; a rule gated on the file
    /// extension (like `missing-provides`, inert outside `.sty`/`.cls`) overrides
    /// this so its examples actually trigger under the docs renderer.
    fn example_path(&self) -> &'static str {
        "example.tex"
    }

    /// Synthetic `(path, source)` sibling files linted alongside every example
    /// of the rule — the two-file story a cross-file rule (like
    /// `unknown-option`, whose example loads a local `.sty`) needs to fire
    /// under the docs renderer. Paths are relative to the example's directory.
    /// Defaults to none.
    fn example_companions(&self) -> &'static [(&'static str, &'static str)] {
        &[]
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

    /// An **ordered, stateful** visitor riding the driver's single shared
    /// traversal, for rules whose finding depends on the *sequence* of elements
    /// (a running toggle, the previous heading's level, …) that a stateless
    /// per-element [`check`](Rule::check) cannot carry. Returning `Some` opts the
    /// rule into the shared walk instead of a private `check_file` re-traversal;
    /// the default `None` opts out. The driver constructs one visitor per file and
    /// feeds it every element in document order, then calls
    /// [`StreamVisitor::finish`].
    fn stream(&self) -> Option<Box<dyn StreamVisitor>> {
        None
    }

    /// Whether this rule can emit an autofix. The `--fix` fixpoint loop runs only
    /// the fix-emitting rules each round (report-only rules contribute nothing to
    /// fix), so it must be `true` for every rule that ever sets `Diagnostic::fix`.
    /// Guarded by `emits_fix_matches_reality` in this module's tests.
    fn emits_fix(&self) -> bool {
        false
    }
}

/// An ordered, stateful pass driven by the linter's single shared traversal (see
/// [`Rule::stream`]). Constructed fresh per file, it receives every CST element in
/// document (preorder) order via [`visit`](StreamVisitor::visit), then a final
/// [`finish`](StreamVisitor::finish). Findings are pushed onto `sink` with the
/// path left empty, exactly like [`Rule::check`].
pub trait StreamVisitor {
    /// Called once for every element of the shared walk, in document order.
    fn visit(&mut self, el: &SyntaxElement, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>);

    /// Called once after the walk, for any deferred finding. Default no-op.
    fn finish(&mut self, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let _ = (ctx, sink);
    }
}

/// Every built-in rule, in registry order.
pub fn all_rules() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(AbbreviationSpacing),
        Box::new(DuplicateLabel),
        Box::new(DeprecatedCommand),
        Box::new(MissingNonbreakingSpace),
        Box::new(ObsoleteEnvironment),
        Box::new(PrimitiveCommand),
        Box::new(DollarDisplayMath),
        Box::new(Ellipsis),
        Box::new(HardCodedReference),
        Box::new(StraightQuotes),
        Box::new(SwallowedSpace),
        Box::new(SpaceBeforeCommand),
        Box::new(MismatchedDelimiter),
        Box::new(DashLength),
        Box::new(TimesVariable),
        Box::new(MathOperatorName),
        Box::new(MakeatMacro),
        Box::new(SectioningLevelJump),
        Box::new(MissingRequiredArgument),
        Box::new(UndefinedRef),
        Box::new(UndefinedCitation),
        Box::new(UnreferencedLabel),
        Box::new(VerbatimTrailingText),
        Box::new(DuplicatePackage),
        Box::new(MissingProvides),
        Box::new(UnknownOption),
    ]
}

/// A prebuilt, shareable view of a rule set: the boxed rules plus the
/// kind → subscriber dispatch table, computed once. The table is identical for
/// every file, so `lint_document` borrows a cached registry instead of rebuilding
/// it per file (the old per-invocation cost, quadratic over a project). Being
/// `Sync` (rules are `Send + Sync`), one registry is also shared by reference
/// across the CLI's rayon lint phase.
pub struct RuleRegistry {
    /// Every rule in the set, in registry order.
    pub rules: Vec<Box<dyn Rule>>,
    /// `by_kind[kind as usize]` lists the indices into `rules` of the node-shape
    /// rules subscribed to that `SyntaxKind`. Indexed by the `#[repr(u16)]`
    /// discriminant, so dispatch is an `O(1)` slice index.
    pub by_kind: Vec<Vec<usize>>,
    /// Whether any rule subscribed to a node kind (lets the driver skip the walk
    /// entirely when only whole-file/streaming rules are present).
    pub any_node_rules: bool,
}

impl RuleRegistry {
    fn build(rules: Vec<Box<dyn Rule>>) -> Self {
        let mut by_kind: Vec<Vec<usize>> = vec![Vec::new(); SyntaxKind::COUNT];
        let mut any_node_rules = false;
        for (i, rule) in rules.iter().enumerate() {
            for kind in rule.interests() {
                by_kind[*kind as usize].push(i);
                any_node_rules = true;
            }
        }
        Self {
            rules,
            by_kind,
            any_node_rules,
        }
    }
}

/// The shared registry of every built-in rule, built once on first use.
pub fn registry() -> &'static RuleRegistry {
    static REGISTRY: OnceLock<RuleRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| RuleRegistry::build(all_rules()))
}

/// The registry restricted to fix-emitting rules ([`Rule::emits_fix`]), used by
/// the `--fix` fixpoint loop so report-only rules aren't recomputed each round.
pub fn fixable_registry() -> &'static RuleRegistry {
    static REGISTRY: OnceLock<RuleRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        RuleRegistry::build(all_rules().into_iter().filter(|r| r.emits_fix()).collect())
    })
}

/// The ids of every built-in **LaTeX** rule. Kept in lockstep with [`all_rules`].
/// The bib rules live in [`crate::bib::linter::ALL_BIB_RULE_IDS`]; the selectable
/// universe is the union of the two (see [`all_known_rule_ids`]).
pub const ALL_RULE_IDS: &[&str] = &[
    "abbreviation-spacing",
    "duplicate-label",
    "deprecated-command",
    "missing-nonbreaking-space",
    "obsolete-environment",
    "primitive-command",
    "dollar-display-math",
    "ellipsis",
    "hard-coded-reference",
    "straight-quotes",
    "swallowed-space",
    "space-before-command",
    "mismatched-delimiter",
    "dash-length",
    "times-variable",
    "math-operator-name",
    "makeat-macro",
    "sectioning-level-jump",
    "missing-required-argument",
    "undefined-ref",
    "undefined-citation",
    "unreferenced-label",
    "verbatim-trailing-text",
    "duplicate-package",
    "missing-provides",
    "unknown-option",
];

/// Every known built-in rule id across **both** linters (LaTeX ∪ BibTeX).
///
/// The CLI lints `.tex` and `.bib` files in one pass and folds their findings into
/// a single diagnostic stream filtered by one [`RuleSelection`], so the selectable
/// universe — and the set `select`/`ignore` are validated against — must span both
/// registries. Without the bib half, every bib finding's id reads as "not active"
/// and the CLI silently drops it (the LSP, which doesn't post-filter, still shows
/// them — the source of the CLI/LSP divergence).
fn all_known_rule_ids() -> impl Iterator<Item = &'static str> {
    ALL_RULE_IDS
        .iter()
        .copied()
        .chain(crate::bib::linter::ALL_BIB_RULE_IDS.iter().copied())
}

/// The pseudo-rule id parse diagnostics carry. It is never a lint rule, so
/// `select`/`ignore` never touch it: a parse error always surfaces.
pub const PARSE_RULE_ID: &str = "parse";

/// The active lint-rule set for one run, after applying `select`/`ignore`.
///
/// Resolution by rule id (not by constructing the rule objects) so it can filter
/// the diagnostics `lint_document` already produced without changing that shared
/// entry point's signature. The semantics are:
///
/// 1. Base set = the ids in `select` when it is `Some`, else every built-in rule.
/// 2. Subtract anything in `ignore`.
/// 3. Unknown ids in `select`/`ignore` (not in [`ALL_RULE_IDS`]) are returned via
///    the second tuple element so the caller can surface them; they do not error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleSelection {
    active: Vec<&'static str>,
}

impl RuleSelection {
    /// Build the active set from `select`/`ignore`, returning it plus any unknown
    /// ids encountered (preserving their original spelling and order).
    pub fn resolve(select: Option<&[String]>, ignore: &[String]) -> (Self, Vec<String>) {
        let mut unknown = Vec::new();
        for id in select.iter().flat_map(|v| v.iter()).chain(ignore.iter()) {
            if !all_known_rule_ids().any(|known| known == id) {
                unknown.push(id.clone());
            }
        }
        let base: Vec<&'static str> = match select {
            Some(picks) => all_known_rule_ids()
                .filter(|id| picks.iter().any(|p| p == id))
                .collect(),
            None => all_known_rule_ids().collect(),
        };
        let active = base
            .into_iter()
            .filter(|id| !ignore.iter().any(|i| i == id))
            .collect();
        (Self { active }, unknown)
    }

    /// The unfiltered set: every built-in rule active. The default for callers
    /// with no config (the LSP, the library API).
    pub fn all() -> Self {
        Self {
            active: all_known_rule_ids().collect(),
        }
    }

    /// Whether a diagnostic with this `rule` should be kept. Parse diagnostics
    /// ([`PARSE_RULE_ID`]) are always kept; lint rules are kept iff active.
    pub fn is_active(&self, rule: &str) -> bool {
        rule == PARSE_RULE_ID || self.active.contains(&rule)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_and_id_list_agree() {
        let ids: Vec<&str> = all_rules().iter().map(|r| r.id()).collect();
        assert_eq!(ids, ALL_RULE_IDS);
    }

    // A rule that ever produces an autofix must report `emits_fix()`, or the
    // `--fix` loop (which runs only `fixable_registry`) would silently skip it.
    // Lint each rule's own examples (which the docs tests require to trigger) and
    // assert any produced fix is backed by the flag.
    #[test]
    fn emits_fix_matches_reality() {
        for rule in all_rules() {
            if rule.emits_fix() {
                continue;
            }
            let path = std::path::Path::new(rule.example_path());
            for example in rule.examples() {
                let produced_fix = crate::linter::docs::demo_diagnostics_at(path, example.source)
                    .iter()
                    .any(|d| d.rule == rule.id() && d.fix.is_some());
                assert!(
                    !produced_fix,
                    "rule `{}` emits a fix but `emits_fix()` returns false",
                    rule.id()
                );
            }
        }
    }

    #[test]
    fn all_selection_keeps_every_rule_and_parse() {
        let sel = RuleSelection::all();
        for id in ALL_RULE_IDS {
            assert!(sel.is_active(id), "{id} should be active");
        }
        assert!(sel.is_active(PARSE_RULE_ID));
    }

    #[test]
    fn select_restricts_to_listed_rules_but_keeps_parse() {
        let (sel, unknown) = RuleSelection::resolve(Some(&["duplicate-label".to_string()]), &[]);
        assert!(unknown.is_empty());
        assert!(sel.is_active("duplicate-label"));
        assert!(!sel.is_active("deprecated-command"));
        // Parse errors are never filtered out by a `select`.
        assert!(sel.is_active(PARSE_RULE_ID));
    }

    #[test]
    fn ignore_subtracts_from_default_set() {
        let (sel, unknown) = RuleSelection::resolve(None, &["deprecated-command".to_string()]);
        assert!(unknown.is_empty());
        assert!(!sel.is_active("deprecated-command"));
        assert!(sel.is_active("duplicate-label"));
    }

    #[test]
    fn ignore_overrides_select() {
        let (sel, _) = RuleSelection::resolve(
            Some(&["duplicate-label".to_string(), "undefined-ref".to_string()]),
            &["undefined-ref".to_string()],
        );
        assert!(sel.is_active("duplicate-label"));
        assert!(!sel.is_active("undefined-ref"));
    }

    #[test]
    fn bib_rules_are_active_by_default() {
        // The CLI filters bib findings through the same `RuleSelection`; bib rule
        // ids must count as known/active or the CLI silently drops every bib finding
        // (while the LSP, which doesn't post-filter, still shows them).
        let sel = RuleSelection::all();
        for id in crate::bib::linter::ALL_BIB_RULE_IDS {
            assert!(sel.is_active(id), "{id} should be active");
        }
        let (sel, unknown) = RuleSelection::resolve(None, &[]);
        assert!(unknown.is_empty());
        assert!(sel.is_active("missing-required-field"));
    }

    #[test]
    fn bib_rules_are_selectable_and_ignorable() {
        let (sel, unknown) =
            RuleSelection::resolve(Some(&["missing-required-field".to_string()]), &[]);
        assert!(unknown.is_empty(), "bib id must be recognized, not unknown");
        assert!(sel.is_active("missing-required-field"));
        assert!(!sel.is_active("duplicate-label"));

        let (sel, unknown) = RuleSelection::resolve(None, &["missing-required-field".to_string()]);
        assert!(unknown.is_empty());
        assert!(!sel.is_active("missing-required-field"));
        assert!(sel.is_active("duplicate-label"));
    }

    #[test]
    fn unknown_ids_are_reported() {
        let (_, unknown) = RuleSelection::resolve(
            Some(&["duplicate-label".to_string(), "no-such-rule".to_string()]),
            &["also-bogus".to_string()],
        );
        assert_eq!(
            unknown,
            vec!["no-such-rule".to_string(), "also-bogus".to_string()]
        );
    }
}
