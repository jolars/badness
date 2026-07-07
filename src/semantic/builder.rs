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
use crate::semantic::label::{
    CitationRef, ColorDef, ColorDefKind, GlossaryDef, GlossaryDefKind, LabelDef, LabelRef,
    RefCommand,
};
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
        } else if let Some(kind) = glossary_definer(&name) {
            // Like `\label`: the key is the whole first group (never comma-split),
            // and a nested-macro key (`\newacronym{\foo}…`) is skipped. The
            // optional `[opts]` of `\newacronym` is an OPTIONAL node, not a GROUP,
            // so it never shifts the key's group index.
            if let Some((inner_range, inner)) = nth_group_inner(&command, 0) {
                for (key, key_range) in key_spans(&inner, inner_range, false) {
                    model.glossary_defs.push(GlossaryDef {
                        key: SmolStr::from(key),
                        kind,
                        range: first_group_range(&command),
                        key_range,
                    });
                }
            }
        } else if let Some(kind) = color_definer(&name) {
            // The defined color name is the first `{…}` group for all three
            // definers (`\definecolor{name}…`, `\colorlet{name}{base}`), never
            // comma-split, and a nested-macro name is skipped like `\label{\foo}`.
            if let Some((inner_range, inner)) = nth_group_inner(&command, 0) {
                for (key, key_range) in key_spans(&inner, inner_range, false) {
                    model.color_defs.push(ColorDef {
                        name: SmolStr::from(key),
                        kind,
                        range: first_group_range(&command),
                        key_range,
                    });
                }
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

/// The recognized glossary/acronym *definer* command for a control-word name, or
/// `None`. The definition-side analog of [`ref_command`]; the key is always the
/// first `{…}` group.
pub(crate) fn glossary_definer(name: &str) -> Option<GlossaryDefKind> {
    Some(match name {
        "newglossaryentry"
        | "longnewglossaryentry"
        | "provideglossaryentry"
        | "longprovideglossaryentry" => GlossaryDefKind::Entry,
        "newacronym" => GlossaryDefKind::Acronym,
        "newabbreviation" => GlossaryDefKind::Abbreviation,
        _ => return None,
    })
}

/// The recognized color *definer* command for a control-word name, or `None`.
/// The definition-side analog of [`glossary_definer`]: the newly defined color
/// name is always the first `{…}` group (`\definecolor{name}{model}{spec}`,
/// `\colorlet{name}{base}`).
pub(crate) fn color_definer(name: &str) -> Option<ColorDefKind> {
    Some(match name {
        "definecolor" => ColorDefKind::DefineColor,
        "providecolor" => ColorDefKind::ProvideColor,
        "colorlet" => ColorDefKind::Colorlet,
        _ => return None,
    })
}

/// Whether `name` is a glossary/acronym *reference* command whose first `{…}`
/// group is an entry key (`\gls`, `\acrshort`, `\glsxtrfull`, …). Shared with the
/// completion classifier (`crate::completion`), like [`ref_command`] and
/// [`is_cite_command`], so the name set has a single source of truth. Unlike
/// citations, every command here takes exactly **one** key per group (no comma
/// list).
pub(crate) fn is_glossary_ref_command(name: &str) -> bool {
    // The `\gls` core set: base name + first-letter-uppercase + all-caps
    // sentence-start variants, each with an optional plural `pl`.
    const GLS: &[&str] = &[
        "gls",
        "Gls",
        "GLS",
        "glspl",
        "Glspl",
        "GLSpl",
        // Text-form accessors (`\glstext{key}` prints the entry text without
        // triggering first-use).
        "glstext",
        "Glstext",
        "glsfirst",
        "Glsfirst",
        "glsplural",
        "Glsplural",
        "glsfirstplural",
        "Glsfirstplural",
        "glsdesc",
        "Glsdesc",
        "glsname",
        "Glsname",
        "glssymbol",
        "Glssymbol",
        // Key-first commands with further groups (`\glslink{key}{text}`).
        "glslink",
        "glsdisp",
        "glsadd",
        // glossaries-extra short/long/full accessors.
        "glsxtrshort",
        "Glsxtrshort",
        "glsxtrlong",
        "Glsxtrlong",
        "glsxtrfull",
        "Glsxtrfull",
    ];
    if GLS.contains(&name) {
        return true;
    }
    // The acronym set: `\acrshort`/`\acrlong`/`\acrfull`, plural `pl`, in
    // `acr`/`Acr`/`ACR` casing — a stem check like `is_cite_command`'s
    // `cite`/`Cite` prefix trick.
    for stem in ["acr", "Acr", "ACR"] {
        if let Some(rest) = name.strip_prefix(stem) {
            return matches!(
                rest,
                "short" | "shortpl" | "long" | "longpl" | "full" | "fullpl"
            );
        }
    }
    false
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
    fn newglossaryentry_key_scanned_with_range() {
        let src = "\\newglossaryentry{ex}{name={example},description={an example}}\n";
        let model = model(src);
        let def = &model.glossary_defs()[0];
        assert_eq!(def.key, "ex");
        assert_eq!(def.kind, crate::semantic::label::GlossaryDefKind::Entry);
        assert_eq!(&src[def.key_range], "ex");
    }

    #[test]
    fn newacronym_optional_arg_does_not_shift_key() {
        let src = "\\newacronym[longplural={frames}]{fps}{FPS}{frame rate}\n";
        let model = model(src);
        let def = &model.glossary_defs()[0];
        assert_eq!(def.key, "fps");
        assert_eq!(def.kind, crate::semantic::label::GlossaryDefKind::Acronym);
        assert_eq!(&src[def.key_range], "fps");
    }

    #[test]
    fn glossary_definer_family_scanned() {
        let src = "\\longnewglossaryentry{a}{name={a}}{desc}\n\\newabbreviation{b}{B}{bee}\n\\provideglossaryentry{c}{name={c}}\n";
        let model = model(src);
        let keys: Vec<_> = model
            .glossary_defs()
            .iter()
            .map(|d| d.key.as_str())
            .collect();
        assert_eq!(keys, vec!["a", "b", "c"]);
    }

    #[test]
    fn glossary_nested_macro_key_skipped() {
        // Like `\label{\foo}`: an unresolvable key is skipped, never guessed.
        let model = model("\\newacronym{\\foo}{F}{foo}\n");
        assert!(model.glossary_defs().is_empty());
    }

    #[test]
    fn gls_use_is_not_a_definition() {
        let model = model("\\gls{ex}\\acrshort{fps}\n");
        assert!(model.glossary_defs().is_empty());
    }

    #[test]
    fn color_definers_scanned_with_ranges() {
        let src = "\\definecolor{brandblue}{HTML}{0055AA}\n\\colorlet{accent}{brandblue}\n\\providecolor{muted}{gray}{0.5}\n";
        let model = model(src);
        let defs: Vec<_> = model
            .color_defs()
            .iter()
            .map(|d| (d.name.as_str(), d.kind, &src[d.key_range]))
            .collect();
        use crate::semantic::label::ColorDefKind::*;
        assert_eq!(
            defs,
            vec![
                ("brandblue", DefineColor, "brandblue"),
                ("accent", Colorlet, "accent"),
                ("muted", ProvideColor, "muted"),
            ]
        );
    }

    #[test]
    fn textcolor_use_is_not_a_color_definition() {
        let model = model("\\textcolor{red}{x}\\color{blue}\n");
        assert!(model.color_defs().is_empty());
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
