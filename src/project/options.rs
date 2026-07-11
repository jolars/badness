//! Cross-file package **option** model: which options a locally-analyzed `.sty`
//! statically declares, and whether its declared set can be trusted at all.
//!
//! Layered like [`crate::project::labels`]: [`package_option_facts`] is the pure
//! per-file extractor (no salsa, no disk) shared by the CLI's one-shot lint
//! driver and the incremental layer's per-file firewall query;
//! [`ResolvedPackageOptions::build`] folds the per-file facts into the map the
//! `unknown-option` lint consumes through `RuleContext`.
//!
//! Everything here reads static facts only (AGENTS.md decision #1): literal
//! `\DeclareOption{name}` declarations from the semantic model, command *names*
//! (never meanings) for the keyval-family processors, and load/include edges.
//! The bias is conservative by construction — any signal that the declared set
//! might be incomplete flips [`PackageOptionFacts::handles_unknown`] and the
//! lint stays silent for that package, because a false "unknown option" finding
//! is worse than a miss.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use smol_str::SmolStr;

use crate::ast::command_name;
use crate::incremental::{IncrementalDb, QueryKind, QueryLogEntry, file_package_option_facts};
use crate::project::graph::Project;
use crate::project::include::collect_include_edge_keys;
use crate::project::package::{PackageTarget, collect_package_edges};
use crate::semantic::SemanticModel;
use crate::syntax::{SyntaxKind, SyntaxNode};

/// Commands that declare or process options *dynamically* — their presence means
/// the literal `\DeclareOption` set is not the whole story, so the lint must not
/// judge options against it. Curated: classic option forwarding, xkeyval,
/// kvoptions, the LaTeX 2022 keys interface, l3keys2e, and pgfopts.
const DYNAMIC_OPTION_COMMANDS: &[&str] = &[
    "RequirePackageWithOptions",
    "LoadClassWithOptions",
    // xkeyval
    "DeclareOptionX",
    "ProcessOptionsX",
    // kvoptions
    "SetupKeyvalOptions",
    "DeclareStringOption",
    "DeclareBoolOption",
    "DeclareComplementaryOption",
    "DeclareVoidOption",
    "DeclareDefaultOption",
    "ProcessKeyvalOptions",
    // LaTeX 2022-06 key-value option interface
    "DeclareKeys",
    "DeclareUnknownKeyHandler",
    "ProcessKeyOptions",
    // l3keys2e
    "ProcessKeysOptions",
    // pgfopts
    "ProcessPgfOptions",
    "ProcessPgfPackageOptions",
];

/// Packages whose *load* signals a dynamic option processor even when none of
/// the [`DYNAMIC_OPTION_COMMANDS`] appears literally (the package may wrap them).
const OPTION_PROCESSOR_PACKAGES: &[&str] = &[
    "xkeyval",
    "kvoptions",
    "kvoptions-patch",
    "l3keys2e",
    "pgfopts",
    "options",
];

/// One analyzed `.sty` member's statically-declared option surface.
///
/// `Eq` is load-bearing for the incremental layer: the per-file
/// `file_package_option_facts` query backdates when a body edit leaves the
/// option surface unchanged, so the cross-file model is not rebuilt (the same
/// firewall reasoning as `file_labels`).
#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct PackageOptionFacts {
    /// The `.sty` file these facts describe (the resolved load target key).
    pub path: PathBuf,
    /// Literal `\DeclareOption{name}` names, sorted and deduped.
    pub declared: Vec<SmolStr>,
    /// The file swallows or dynamically processes unknown options — a
    /// `\DeclareOption*` default handler (or a dynamic-named `\DeclareOption`,
    /// which the extractor deliberately records the same way), a keyval-family
    /// processor, an option-forwarding load, or an `\input` that could pull in
    /// declarations we never see. The lint must stay silent for this target.
    pub handles_unknown: bool,
}

impl PackageOptionFacts {
    /// Whether `option` is in the literal declared set.
    pub fn declares(&self, option: &str) -> bool {
        self.declared
            .binary_search_by(|d| d.as_str().cmp(option))
            .is_ok()
    }
}

/// Extract the option facts for one file. `Some` only for a `.sty` path:
/// classes are excluded because an unknown *class* option is not a LaTeX error
/// (it becomes an unused global option silently offered to every package), and
/// the `.dtx` fallback is excluded because its `\DeclareOption`s sit inside
/// guarded docstrip macrocode the plain parse does not surface.
pub fn package_option_facts(
    path: &Path,
    root: &SyntaxNode,
    model: &SemanticModel,
) -> Option<PackageOptionFacts> {
    if path.extension().and_then(|e| e.to_str()) != Some("sty") {
        return None;
    }

    let mut declared: Vec<SmolStr> = Vec::new();
    let mut handles_unknown = false;
    for decl in model.options() {
        match &decl.name {
            Some(name) => declared.push(name.clone()),
            // `\DeclareOption*` or a dynamic-named `\DeclareOption{\x}` — either
            // way the declared set is not trustworthy.
            None => handles_unknown = true,
        }
    }
    declared.sort_unstable();
    declared.dedup();

    handles_unknown = handles_unknown
        || root
            .descendants()
            .filter(|node| node.kind() == SyntaxKind::COMMAND)
            .filter_map(|node| command_name(&node))
            .any(|name| DYNAMIC_OPTION_COMMANDS.contains(&name.as_str()))
        || collect_package_edges(root, None).iter().any(|edge| {
            let PackageTarget::Path(target) = &edge.target else {
                return false;
            };
            target
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|stem| OPTION_PROCESSOR_PACKAGES.contains(&stem))
        })
        || !collect_include_edge_keys(root, None).is_empty();

    Some(PackageOptionFacts {
        path: path.to_path_buf(),
        declared,
        handles_unknown,
    })
}

/// Resolved-target-path → per-file option facts, threaded into `RuleContext`
/// parallel to [`super::ResolvedLabels`]/[`super::ResolvedCitations`]. A target
/// absent from the map is a system package or non-member — the lint skips it.
///
/// Holds `HashMap`s/`PathBuf`s, so (like [`super::ResolvedLabels`]) it is
/// neither `Eq` nor `salsa::Update`; the incremental wrapper query is `no_eq`.
#[derive(Debug, Default)]
pub struct ResolvedPackageOptions {
    files: HashMap<PathBuf, PackageOptionFacts>,
}

impl ResolvedPackageOptions {
    /// Fold per-file facts into the lookup map. Keyed by each fact's own path;
    /// pure and deterministic (last write wins, but paths are unique per file).
    pub fn build(facts: impl IntoIterator<Item = PackageOptionFacts>) -> Self {
        Self {
            files: facts.into_iter().map(|f| (f.path.clone(), f)).collect(),
        }
    }

    /// The facts for a resolved load `target`, when it is an analyzed `.sty`.
    pub fn get(&self, target: &Path) -> Option<&PackageOptionFacts> {
        self.files.get(target)
    }
}

/// The cross-file package-option model for `project`, built from the per-file
/// [`file_package_option_facts`] firewall.
///
/// `no_eq` + `unsafe(non_update_types)` for the same reason as
/// [`super::resolved_labels`]: [`ResolvedPackageOptions`] holds a `HashMap`
/// (not `Eq`/`salsa::Update`) and is a pure function of the interned
/// [`Project`] plus the backdated per-file facts. A body edit that leaves a
/// `.sty`'s option surface unchanged backdates the firewall and this query is
/// not re-run.
#[salsa::tracked(returns(ref), no_eq, unsafe(non_update_types))]
pub fn resolved_package_options<'db>(
    db: &'db dyn IncrementalDb,
    project: Project<'db>,
) -> ResolvedPackageOptions {
    db.record_query(QueryLogEntry {
        kind: QueryKind::ResolvedPackageOptions,
        file: None,
    });
    ResolvedPackageOptions::build(
        project
            .members(db)
            .iter()
            .filter(|member| member.kind.is_latex())
            .filter_map(|member| file_package_option_facts(db, member.file).clone()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_with_flavor;
    use crate::syntax::SyntaxNode;

    fn facts(path: &str, src: &str) -> Option<PackageOptionFacts> {
        // `.sty` sources lex with `@` as a letter, like the real load path.
        let kind = crate::file_discovery::file_kind_or_tex(Path::new(path));
        let root = SyntaxNode::new_root(parse_with_flavor(src, kind.lex_config()).green);
        let model = SemanticModel::build(&root);
        package_option_facts(Path::new(path), &root, &model)
    }

    #[test]
    fn declared_options_are_extracted_sorted() {
        let f = facts(
            "/p/mypkg.sty",
            "\\ProvidesPackage{mypkg}\n\\DeclareOption{final}{}\n\\DeclareOption{draft}{}\n\\ProcessOptions\\relax\n",
        )
        .expect("sty facts");
        assert_eq!(
            f.declared,
            vec![SmolStr::from("draft"), SmolStr::from("final")]
        );
        assert!(!f.handles_unknown);
        assert!(f.declares("draft"));
        assert!(!f.declares("typo"));
    }

    #[test]
    fn star_handler_marks_handles_unknown() {
        let f = facts(
            "/p/mypkg.sty",
            "\\DeclareOption{a}{}\n\\DeclareOption*{\\OptionNotUsed}\n",
        )
        .expect("sty facts");
        assert!(f.handles_unknown);
    }

    #[test]
    fn dynamic_named_declare_option_marks_handles_unknown() {
        let f = facts("/p/mypkg.sty", "\\DeclareOption{\\myopt}{}\n").expect("sty facts");
        assert!(f.handles_unknown);
    }

    #[test]
    fn keyval_family_commands_mark_handles_unknown() {
        for src in [
            "\\SetupKeyvalOptions{family=my,prefix=my@}\n",
            "\\ProcessKeyOptions\n",
            "\\DeclareOptionX{width}{}\n",
        ] {
            let f = facts("/p/mypkg.sty", src).expect("sty facts");
            assert!(f.handles_unknown, "for {src:?}");
        }
    }

    #[test]
    fn option_processor_package_load_marks_handles_unknown() {
        let f = facts("/p/mypkg.sty", "\\RequirePackage{kvoptions}\n").expect("sty facts");
        assert!(f.handles_unknown);
    }

    #[test]
    fn input_edge_marks_handles_unknown() {
        let f = facts("/p/mypkg.sty", "\\input{mypkg-options}\n").expect("sty facts");
        assert!(f.handles_unknown);
    }

    #[test]
    fn plain_package_load_does_not_mark() {
        let f = facts("/p/mypkg.sty", "\\RequirePackage{graphicx}\n").expect("sty facts");
        assert!(!f.handles_unknown);
    }

    #[test]
    fn non_sty_paths_have_no_facts() {
        assert!(facts("/p/myclass.cls", "\\DeclareOption{a}{}\n").is_none());
        assert!(facts("/p/mypkg.dtx", "\\DeclareOption{a}{}\n").is_none());
        assert!(facts("/p/main.tex", "").is_none());
    }

    #[test]
    fn resolved_map_is_keyed_by_path() {
        let f = facts("/p/mypkg.sty", "\\DeclareOption{a}{}\n").expect("sty facts");
        let resolved = ResolvedPackageOptions::build([f]);
        assert!(resolved.get(Path::new("/p/mypkg.sty")).is_some());
        assert!(resolved.get(Path::new("/p/other.sty")).is_none());
    }
}
