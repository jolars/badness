//! Living-documentation tests for the linter-rules reference.
//!
//! The reference page (`docs/src/reference/linter-rules.md`) is rendered from
//! each rule's `description()`/`examples()` by running the *real* linter, so it
//! cannot drift from behavior. These tests pin that rendering, require every
//! rule to be documented, and check that each example actually triggers its
//! rule. The generator (`examples/docgen.rs`) writes the same
//! `render_reference_page` output to the mdBook source tree.

use std::path::Path;

use badness::linter::docs::{
    demo_diagnostics_with, explain_rule, render_reference_page, render_rule_doc,
};
use badness::linter::rules::{ALL_RULE_IDS, all_rules};

/// Pin the rendered section for every rule. Any change to a rule's diagnostic or
/// fix that alters its page fails here before the docs go stale.
#[test]
fn rule_docs_render() {
    for rule in all_rules() {
        insta::assert_snapshot!(rule.id().replace('-', "_"), render_rule_doc(rule.as_ref()));
    }
}

/// Every shipped rule must carry a description and at least one example, so the
/// generated reference is complete.
#[test]
fn every_rule_is_documented() {
    for rule in all_rules() {
        assert!(
            !rule.description().trim().is_empty(),
            "rule `{}` has no description",
            rule.id(),
        );
        assert!(
            !rule.examples().is_empty(),
            "rule `{}` has no examples",
            rule.id(),
        );
    }
}

/// Every documented example must actually produce a finding of its own rule —
/// guards against a snippet that looks plausible but no longer triggers.
#[test]
fn documented_examples_actually_trigger() {
    for rule in all_rules() {
        for example in rule.examples() {
            let diagnostics = demo_diagnostics_with(
                Path::new(rule.example_path()),
                example.source,
                rule.example_companions(),
            );
            assert!(
                diagnostics.iter().any(|d| d.rule == rule.id()),
                "example for rule `{}` produced no finding of that rule:\n{}",
                rule.id(),
                example.source,
            );
        }
    }
}

/// `lint --explain <rule>` resolves every built-in rule id and rejects unknown
/// ones, so the CLI help surface stays in step with the registry.
#[test]
fn explain_resolves_every_rule() {
    for id in ALL_RULE_IDS {
        let doc = explain_rule(id).unwrap_or_else(|| panic!("no explanation for `{id}`"));
        assert!(doc.contains(id), "explanation for `{id}` omits its id");
    }
    assert!(explain_rule("no-such-rule").is_none());
}

/// The committed reference page must equal what the generator would write, so a
/// metadata change that isn't regenerated fails CI instead of shipping stale
/// docs. Run `cargo run --example docgen` to refresh it.
#[test]
fn reference_page_is_committed() {
    let committed = std::fs::read_to_string("docs/src/reference/linter-rules.md")
        .expect("linter-rules.md should exist");
    assert_eq!(
        committed,
        render_reference_page(),
        "docs/src/reference/linter-rules.md is stale; run `cargo run --example docgen`",
    );
}
