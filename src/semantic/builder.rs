//! Build the per-file label/reference model from the CST.
//!
//! A single whole-tree walk (mirror of `project::collect_include_edges`)
//! collects `\label{…}` definitions and the reference-command family, then a
//! flat `resolve` pass matches refs to defs by name. Labels live in one
//! document-global namespace, so there is no scope walk — resolution is a flat
//! name match (contrast arity's scope-chain `resolve_reads`).

use smol_str::SmolStr;

use crate::ast::{command_name, first_group_range, nth_group_text};
use crate::semantic::SemanticModel;
use crate::semantic::label::{LabelDef, LabelRef, RefCommand};
use crate::syntax::{SyntaxKind, SyntaxNode};

pub fn build(root: &SyntaxNode) -> SemanticModel {
    let mut model = SemanticModel::default();

    for command in root
        .descendants()
        .filter(|node| node.kind() == SyntaxKind::COMMAND)
    {
        let Some(name) = command_name(&command) else {
            continue;
        };

        if name == "label" {
            // A nested-macro key (`\label{\foo}`) yields `None`; skip it,
            // conservative like an unresolvable include target.
            if let Some(key) = nth_group_text(&command, 0) {
                let key = key.trim();
                if !key.is_empty() {
                    model.labels.push(LabelDef {
                        name: SmolStr::from(key),
                        range: first_group_range(&command),
                        referenced: false,
                    });
                }
            }
        } else if let Some(kind) = ref_command(&name)
            && let Some(arg) = nth_group_text(&command, 0)
        {
            for key in split_keys(&arg, kind) {
                model.refs.push(LabelRef {
                    name: SmolStr::from(key),
                    command: kind,
                    range: command.text_range(),
                    resolved: false,
                });
            }
        }
    }

    resolve(&mut model);
    model
}

/// The recognized reference command for a control-word name, or `None`. A small
/// explicit table — the analog of `project::include::include_kind`. Shared with
/// the completion classifier (`crate::completion`) so the ref-family name set has
/// a single source of truth.
pub(crate) fn ref_command(name: &str) -> Option<RefCommand> {
    Some(match name {
        "ref" => RefCommand::Ref,
        "pageref" => RefCommand::PageRef,
        "eqref" => RefCommand::EqRef,
        "autoref" => RefCommand::AutoRef,
        "nameref" => RefCommand::NameRef,
        "cref" => RefCommand::Cref,
        "Cref" => RefCommand::CrefUpper,
        "vref" => RefCommand::Vref,
        "Vref" => RefCommand::VrefUpper,
        "cpageref" => RefCommand::CpageRef,
        _ => return None,
    })
}

/// Split a reference command's argument into individual keys. Key-list commands
/// (cleveref / varioref) split on commas; the rest are a single key. Surrounding
/// whitespace is trimmed (TeX ignores it around these keys) and empty keys are
/// dropped.
fn split_keys(arg: &str, kind: RefCommand) -> Vec<&str> {
    if kind.is_key_list() {
        arg.split(',')
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .collect()
    } else {
        let key = arg.trim();
        if key.is_empty() {
            Vec::new()
        } else {
            vec![key]
        }
    }
}

/// Flat name-match resolution: mark each ref `resolved` when a same-named label
/// exists, and each such label `referenced`.
fn resolve(model: &mut SemanticModel) {
    for ref_idx in 0..model.refs.len() {
        let name = model.refs[ref_idx].name.clone();
        let mut hit = false;
        for label in &mut model.labels {
            if label.name == name {
                label.referenced = true;
                hit = true;
            }
        }
        model.refs[ref_idx].resolved = hit;
    }
}
