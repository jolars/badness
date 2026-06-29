//! Build the per-file label/reference model from the CST.
//!
//! A single whole-tree walk (mirror of `project::collect_include_edges`)
//! collects `\label{…}` definitions and the reference-command family, then a
//! flat `resolve` pass matches refs to defs by name. Labels live in one
//! document-global namespace, so there is no scope walk — resolution is a flat
//! name match, not a scope-chain resolution.

use smol_str::SmolStr;

use rowan::{TextRange, TextSize};

use crate::ast::{command_name, first_group_range, nth_group_inner};
use crate::semantic::SemanticModel;
use crate::semantic::label::{CitationRef, LabelDef, LabelRef, RefCommand};
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
            // conservative like an unresolvable include target. A label key is the
            // whole inner content (not comma-split): `split = false`.
            if let Some((inner_range, inner)) = nth_group_inner(&command, 0) {
                for (key, key_range) in key_spans(&inner, inner_range, false) {
                    model.labels.push(LabelDef {
                        name: SmolStr::from(key),
                        range: first_group_range(&command),
                        key_range,
                        referenced: false,
                    });
                }
            }
        } else if let Some(kind) = ref_command(&name)
            && let Some((inner_range, inner)) = nth_group_inner(&command, 0)
        {
            for (key, key_range) in key_spans(&inner, inner_range, kind.is_key_list()) {
                model.refs.push(LabelRef {
                    name: SmolStr::from(key),
                    command: kind,
                    range: command.text_range(),
                    key_range,
                    resolved: false,
                });
            }
        } else if is_cite_command(&name)
            && let Some((inner_range, inner)) = nth_group_inner(&command, 0)
        {
            // `\nocite{*}` is a wildcard pulling in every entry — recorded as a flag,
            // not a key, so it suppresses `undefined-citation` rather than being one.
            if name == "nocite" && inner.trim() == "*" {
                model.nocite_all = true;
            } else {
                // Cite commands always take a comma-separated key list.
                for (key, key_range) in key_spans(&inner, inner_range, true) {
                    model.citations.push(CitationRef {
                        name: SmolStr::from(key),
                        command: SmolStr::from(name.as_str()),
                        range: command.text_range(),
                        key_range,
                    });
                }
            }
        }
    }

    resolve(&mut model);
    model
}

/// Whether `name` is a citation command (`\cite` and the natbib/biblatex family,
/// plus `\nocite`). Capitalized biblatex variants (`\Cite`, `\Textcite`, …) and
/// the `cite`-prefixed natbib set are covered by the prefix check; an explicit
/// short list catches the rest. Keys are comma-separated for all of them.
pub(crate) fn is_cite_command(name: &str) -> bool {
    const EXTRA: &[&str] = &[
        "parencite",
        "Parencite",
        "footcite",
        "footcitetext",
        "textcite",
        "Textcite",
        "smartcite",
        "Smartcite",
        "autocite",
        "Autocite",
        "supercite",
        "fullcite",
        "footfullcite",
        "nocite",
        "notecite",
        "Notecite",
        "pnotecite",
        "fnotecite",
    ];
    // The `\cite` family: `\cite`, `\citep`, `\citet`, `\citeauthor`,
    // `\citeyear`, `\Citep`, … all begin with `cite`/`Cite`.
    name.starts_with("cite") || name.starts_with("Cite") || EXTRA.contains(&name)
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

/// Split a group's inner text into keys paired with their precise byte ranges in
/// the source. When `split` (key-list commands, citations), keys are comma-
/// separated; otherwise the whole inner is one key (`\label`, single-key refs).
/// Surrounding whitespace is trimmed (TeX ignores it around these keys) and empty
/// keys are dropped. `inner_range` is the byte span of `inner`, so each key's range
/// is sliced off it by offset — exact because trimming removes only single-byte
/// ASCII whitespace.
fn key_spans(inner: &str, inner_range: TextRange, split: bool) -> Vec<(&str, TextRange)> {
    let base = inner_range.start();
    let mut out = Vec::new();
    if split {
        // Track each comma-segment's byte offset within `inner` (the segment text
        // plus one byte for the comma that followed it).
        let mut seg_off = 0usize;
        for segment in inner.split(',') {
            if let Some((key, lo, hi)) = trimmed_span(segment) {
                out.push((key, key_range(base, seg_off + lo, seg_off + hi)));
            }
            seg_off += segment.len() + 1;
        }
    } else if let Some((key, lo, hi)) = trimmed_span(inner) {
        out.push((key, key_range(base, lo, hi)));
    }
    out
}

/// The trimmed key of `segment` with its start/end byte offsets *within* `segment`,
/// or `None` when the segment is empty after trimming.
fn trimmed_span(segment: &str) -> Option<(&str, usize, usize)> {
    let key = segment.trim();
    if key.is_empty() {
        return None;
    }
    let lo = segment.len() - segment.trim_start().len();
    Some((key, lo, lo + key.len()))
}

/// Build a [`TextRange`] from `base` plus byte offsets `lo`/`hi`.
fn key_range(base: TextSize, lo: usize, hi: usize) -> TextRange {
    TextRange::new(
        base + TextSize::from(lo as u32),
        base + TextSize::from(hi as u32),
    )
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

#[cfg(test)]
mod tests {
    use crate::parser::parse;
    use crate::syntax::SyntaxNode;

    use super::build;

    fn model(src: &str) -> crate::semantic::SemanticModel {
        build(&SyntaxNode::new_root(parse(src).green))
    }

    #[test]
    fn label_key_range_excludes_command_and_braces() {
        let src = "\\label{ sec:intro }\n";
        let model = model(src);
        let def = &model.labels()[0];
        assert_eq!(def.name, "sec:intro");
        // The key range covers only the trimmed key, not the braces or padding.
        assert_eq!(&src[def.key_range], "sec:intro");
    }

    #[test]
    fn cref_list_keys_get_isolated_ranges() {
        let src = "\\cref{a,b,c}\n";
        let model = model(src);
        let keys: Vec<_> = model
            .refs()
            .iter()
            .map(|r| (r.name.as_str(), &src[r.key_range]))
            .collect();
        assert_eq!(
            keys,
            vec![("a", "a"), ("b", "b"), ("c", "c")],
            "each key in a list command isolates its own span"
        );
    }

    #[test]
    fn cite_list_keys_get_isolated_ranges() {
        let src = "\\cite{ foo , bar }\n";
        let model = model(src);
        let keys: Vec<_> = model
            .citations()
            .iter()
            .map(|c| (c.name.as_str(), &src[c.key_range]))
            .collect();
        assert_eq!(keys, vec![("foo", "foo"), ("bar", "bar")]);
    }
}
