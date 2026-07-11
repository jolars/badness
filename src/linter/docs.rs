//! Rendering the linter-rules reference from rule metadata.
//!
//! [`render_rule_doc`] and [`render_reference_page`] are the single source of
//! truth shared by the snapshot test (`tests/rule_docs.rs`) and the docs
//! generator (`examples/docgen.rs`), so the committed
//! `docs/src/reference/linter-rules.md` and the pinned snapshots can never
//! diverge from behavior. Every example is linted by the *real* driver
//! ([`demo_diagnostics`]), so the rendered diagnostics and the autofix
//! before/after always reflect the current rules.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use smol_str::SmolStr;

use crate::file_discovery::file_kind_or_tex;
use crate::linter::check::lint_document;
use crate::linter::diagnostic::{Diagnostic, Fix};
use crate::linter::fix::apply_fixes;
use crate::linter::render::{OutputMode, render_findings};
use crate::linter::rules::{Rule, all_rules};
use crate::parser::{parse, parse_with_flavor};
use crate::project::include::BibTarget;
use crate::project::labels::{document_label_names, document_ref_names};
use crate::project::{
    CiteFileFacts, FileFacts, IncludeGraph, PackageOptionFacts, ResolvedCitations, ResolvedLabels,
    ResolvedPackageOptions, collect_bib_resource_targets, collect_include_edge_keys,
    package_option_facts,
};
use crate::semantic::SemanticModel;
use crate::syntax::SyntaxNode;

/// The synthetic path used when linting an example snippet. The same value keys
/// both [`lint_document`] and the `render_findings` source lookup: the driver
/// stamps every diagnostic's `path` to it, and the pretty renderer degrades to a
/// location-only line if the source can't be found for that exact path.
fn example_path() -> PathBuf {
    PathBuf::from("example.tex")
}

/// Lint `source` as a self-contained document under a synthetic **closed,
/// rooted** single-file project view, so even the cross-file rules
/// (`undefined-ref`, `undefined-citation`) are demonstrable from one snippet.
///
/// Labels defined in the snippet are honored (a `\ref` to a *defined* label does
/// not flag), and every `\bibliography`/`\addbibresource` resource the snippet
/// names is registered as an analyzed, empty bibliography so the namespace is
/// closed and any cited key reads as undefined.
pub fn demo_diagnostics(source: &str) -> Vec<Diagnostic> {
    demo_diagnostics_at(&example_path(), source)
}

/// Like [`demo_diagnostics`] but lints the snippet under a given synthetic path.
/// Path-sensitive rules (gated on the file extension, like `missing-provides`)
/// pass their [`Rule::example_path`](crate::linter::rules::Rule::example_path) so
/// their examples fire.
pub fn demo_diagnostics_at(path: &Path, source: &str) -> Vec<Diagnostic> {
    demo_diagnostics_with(path, source, &[])
}

/// Like [`demo_diagnostics_at`] with synthetic sibling files linted alongside
/// the snippet — the two-file story a cross-file rule like `unknown-option`
/// needs (its example loads a `.sty` whose declared options live in a
/// companion). Each companion is parsed under its own file kind and its
/// package-option facts folded into the project view; companion paths are
/// relative, so a bare `mypkg.sty` is exactly what the snippet's
/// `\usepackage{mypkg}` resolves to next to `example.tex`.
pub fn demo_diagnostics_with(
    path: &Path,
    source: &str,
    companions: &[(&str, &str)],
) -> Vec<Diagnostic> {
    let path = path.to_path_buf();
    let root = SyntaxNode::new_root(parse(source).green);
    let model = SemanticModel::build(&root);

    let mut option_facts: Vec<PackageOptionFacts> = package_option_facts(&path, &root, &model)
        .into_iter()
        .collect();
    for (companion_path, companion_source) in companions {
        let companion_path = Path::new(companion_path);
        let parsed = parse_with_flavor(
            companion_source,
            file_kind_or_tex(companion_path).lex_config(),
        );
        let companion_root = SyntaxNode::new_root(parsed.green);
        let companion_model = SemanticModel::build(&companion_root);
        option_facts.extend(package_option_facts(
            companion_path,
            &companion_root,
            &companion_model,
        ));
    }
    let resolved_packages = ResolvedPackageOptions::build(option_facts);

    let facts = [FileFacts {
        path: path.clone(),
        include_edges: collect_include_edge_keys(&root, None),
    }];
    let graph = IncludeGraph::build(&facts, None);

    // `true` treats the snippet as a document root, so the namespace is rooted
    // (and, with no unresolved includes, closed).
    let labels = [(
        path.clone(),
        document_label_names(&model),
        document_ref_names(&model),
        true,
    )];
    let resolved_labels = ResolvedLabels::build(&labels, &graph);

    let bib_targets = collect_bib_resource_targets(&root, None);
    let mut bib_keys: HashMap<PathBuf, Vec<SmolStr>> = HashMap::new();
    for target in &bib_targets {
        if let BibTarget::Path(p) = target {
            bib_keys.entry(p.clone()).or_default();
        }
    }
    let cite_facts = [CiteFileFacts {
        path: path.clone(),
        bib_targets,
        nocite_all: model.has_wildcard_nocite(),
        is_document_root: true,
    }];
    let resolved_citations = ResolvedCitations::build(&cite_facts, &graph, &bib_keys);

    lint_document(
        &path,
        &root,
        &model,
        Some(&resolved_labels),
        Some(&resolved_citations),
        Some(&resolved_packages),
    )
}

/// Render the reference *section* for a single rule: an `## \`id\`` heading, the
/// rule's `description()`, and each example rendered with its live diagnostics
/// and (for a safe autofix) the after state.
pub fn render_rule_doc(rule: &dyn Rule) -> String {
    let mut out = String::new();
    let id = rule.id();
    let example_path = PathBuf::from(rule.example_path());
    let _ = writeln!(out, "## `{id}`");

    let description = rule.description().trim();
    if !description.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "{description}");
    }

    // Synthetic sibling files linted alongside every example (the two-file
    // story of a cross-file rule); rendered once, before the examples.
    let companions = rule.example_companions();
    for (companion_path, companion_source) in companions {
        let _ = writeln!(out);
        let _ = writeln!(out, "With a sibling `{companion_path}`:");
        let _ = writeln!(out);
        fenced(&mut out, "tex", companion_source);
    }

    for example in rule.examples() {
        let _ = writeln!(out);
        if !example.caption.is_empty() {
            let _ = writeln!(out, "{}", example.caption);
            let _ = writeln!(out);
        }
        fenced(&mut out, "tex", example.source);

        // Restrict to this rule so an example can't advertise another's finding.
        let diagnostics: Vec<Diagnostic> =
            demo_diagnostics_with(&example_path, example.source, companions)
                .into_iter()
                .filter(|d| d.rule == id)
                .collect();
        let source = example.source.to_string();
        let rendered = render_findings(&diagnostics, OutputMode::Pretty, &|path| {
            (path == example_path.as_path()).then(|| source.clone())
        });
        let _ = writeln!(out);
        fenced(&mut out, "text", &rendered);

        // Safe fixes only, matching what `badness lint --fix` applies.
        let fixes: Vec<Fix> = diagnostics.iter().filter_map(|d| d.fix.clone()).collect();
        let after = apply_fixes(&source, &fixes, false);
        if after.applied > 0 {
            let _ = writeln!(out);
            let _ = writeln!(out, "After applying the fix:");
            let _ = writeln!(out);
            fenced(&mut out, "tex", &after.output);
        }
    }

    out
}

/// Render the reference section for the rule with `id`, or `None` if no built-in
/// LaTeX rule has that id. Backs `badness lint --explain <rule>`, reusing the
/// same live-linted rendering as the docs page.
pub fn explain_rule(id: &str) -> Option<String> {
    all_rules()
        .iter()
        .find(|rule| rule.id() == id)
        .map(|rule| render_rule_doc(rule.as_ref()))
}

/// The full `linter-rules.md` reference page: a static preamble, one generated
/// section per rule (registry order), and a static configuration footer.
pub fn render_reference_page() -> String {
    let mut out = String::from(PREAMBLE);
    for rule in all_rules() {
        out.push('\n');
        out.push_str(&render_rule_doc(rule.as_ref()));
    }
    out.push('\n');
    out.push_str(FOOTER);
    out
}

/// Write a fenced code block, normalizing the body to end with exactly one
/// newline so the closing fence always sits on its own line (idempotence).
fn fenced(out: &mut String, lang: &str, body: &str) {
    let _ = writeln!(out, "```{lang}");
    let _ = out.write_str(body);
    if !body.ends_with('\n') {
        let _ = out.write_str("\n");
    }
    let _ = writeln!(out, "```");
}

const PREAMBLE: &str = "\
<!-- Generated by `cargo run --example docgen`. Do not edit by hand: edit each \
rule's `description()`/`examples()` in `src/linter/rules/` and regenerate. -->

# Linter Rules

`badness lint` runs a set of built-in rules over each file's parse tree and
reports a diagnostic for every finding. This page is the catalogue: one section
per rule, keyed by its stable **rule id**. That id is what appears in a
diagnostic, what `[lint]` `select`/`ignore` (and `--select`/`--ignore`) target,
and what a `% badness-ignore <id>` comment suppresses.

Every rule is **on by default**; narrowing happens only through `select`/`ignore`
(see [Configuration](#configuration)). Where a rewrite is unambiguous a rule
carries an **auto-fix**: a *safe* fix (shown below as \"After applying the fix\")
is applied by `badness lint --fix`; an *unsafe* fix, one that may change output
such as inserting a line-breaking tie, is applied only with `--unsafe-fixes` or
as an editor code action, so it has no \"after\" block here.

Each example below is linted live to produce its diagnostic and fixed output, so
this page never drifts from the rules' actual behavior.

This page covers the **LaTeX** linter. BibTeX files have a parallel set of rules
(a separate `BibRule` registry under `src/bib/linter/`), selectable through the
same `[lint]` config but not yet catalogued here.
";

const FOOTER: &str = "\
## Configuration

Rules are selected through the `[lint]` table in `badness.toml`, or the matching
CLI flags:

```toml
[lint]
# When present, an allowlist: only these rules run.
select = [\"deprecated-command\", \"dollar-display-math\"]
# Applied on top of select (or the default set): these are turned off.
ignore = [\"missing-nonbreaking-space\"]
```

An unknown rule id is reported at lint time, not rejected at config-parse time.
To suppress a rule at a single site, use a comment directive:

```tex
% badness-ignore deprecated-command: legacy code, leave as-is
{\\bf here}
```

`% badness-ignore-file <id>: ...` suppresses one rule file-wide, and
`% badness-ignore-file: ...` suppresses all rules file-wide. Parse diagnostics
(rule id `parse`) are never suppressed by `select`/`ignore`.
";
