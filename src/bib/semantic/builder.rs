//! Builds the bib [`Model`](super::Model) in a single CST walk, then a resolve pass.
//!
//! Mirrors [`crate::semantic::builder`]: one `root.descendants()` pass collects
//! entries, `@string` definitions, and `@string` uses; then [`resolve`] flags
//! duplicate cite keys and marks each use resolved/undefined by name match. No
//! diagnostics are produced here — the model exposes facts; the linter (Phase 3)
//! turns them into diagnostics.

use std::collections::HashSet;

use smol_str::SmolStr;

use crate::bib::ast;
use crate::bib::semantic::Model;
use crate::bib::semantic::entry::{Entry, StringDef, StringUse};
use crate::bib::syntax::{SyntaxKind, SyntaxNode};

/// Month abbreviations BibTeX/biber predefine as `@string` macros. A bare use of one
/// is always resolved, so whitelisting them avoids false "undefined string" findings.
pub(crate) const MONTH_MACROS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

/// Build the model from a bib parse-tree root.
pub fn build(root: &SyntaxNode) -> Model {
    let mut model = Model::default();
    for node in root.descendants() {
        match node.kind() {
            SyntaxKind::ENTRY => collect_entry(&node, &mut model),
            SyntaxKind::STRING_ENTRY => collect_string(&node, &mut model),
            _ => {}
        }
    }
    resolve(&mut model);
    model
}

/// Record a regular entry (when it has a key) and any `@string` uses in its values.
fn collect_entry(entry: &SyntaxNode, model: &mut Model) {
    if let Some((key, key_range)) = ast::cite_key(entry) {
        let entry_type = ast::entry_type(entry).unwrap_or_default().to_lowercase();
        model.entries.push(Entry {
            entry_type: SmolStr::new(entry_type),
            key: SmolStr::new(key),
            key_range,
            range: entry.text_range(),
            duplicate: false,
        });
    }
    collect_uses(entry, model);
}

/// Record an `@string` definition and any `@string` uses in its (concatenated) value.
fn collect_string(string_entry: &SyntaxNode, model: &mut Model) {
    if let Some((name, range)) = ast::string_def_name(string_entry) {
        model.string_defs.push(StringDef {
            name: SmolStr::new(name.to_lowercase()),
            range,
        });
    }
    collect_uses(string_entry, model);
}

/// Collect the bare-macro uses across every field value of `node`.
fn collect_uses(node: &SyntaxNode, model: &mut Model) {
    for field in ast::fields(node) {
        let Some(value) = ast::field_value(&field) else {
            continue;
        };
        for (name, range) in ast::value_macro_uses(&value) {
            model.string_uses.push(StringUse {
                name: SmolStr::new(name.to_lowercase()),
                range,
                resolved: false,
            });
        }
    }
}

/// Flag duplicate cite keys and mark each `@string` use resolved or not.
fn resolve(model: &mut Model) {
    // Duplicate cite keys (case-insensitive; the first occurrence stays `false`).
    let mut seen: HashSet<SmolStr> = HashSet::new();
    for entry in &mut model.entries {
        let folded = SmolStr::new(entry.key.to_lowercase());
        if !seen.insert(folded) {
            entry.duplicate = true;
        }
    }

    // Undefined `@string` uses: defined names are the in-file defs plus the predefined
    // month macros. Whole-file set, order-independent (no forward-reference rule).
    let mut defined: HashSet<SmolStr> = model.string_defs.iter().map(|d| d.name.clone()).collect();
    defined.extend(MONTH_MACROS.iter().map(|m| SmolStr::new(*m)));
    for string_use in &mut model.string_uses {
        string_use.resolved = defined.contains(&string_use.name);
    }
}
