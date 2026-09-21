//! Completion names inferred from literal expl3 definition calls.

use std::collections::BTreeMap;

use super::calls::{Call, Expl3Index, definition_kind, is_variant_generation};
use super::variants::{Conversion, classify, is_specifier};
use crate::syntax::SyntaxNode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SymbolKind {
    Function,
    Variable,
    Constant,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
}

/// Collect names without claiming argument signatures or execution order.
pub fn collect(root: &SyntaxNode) -> Vec<Symbol> {
    let index = Expl3Index::build(root);
    let mut names = BTreeMap::new();
    for call in index.calls() {
        for symbol in defined_names(call) {
            names.insert(symbol.name, symbol.kind);
        }
    }
    names
        .into_iter()
        .map(|(name, kind)| Symbol { name, kind })
        .collect()
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|c| c.is_ascii_alphabetic() || b"_:@".contains(&c))
}

fn symbol(name: String, kind: SymbolKind) -> Vec<Symbol> {
    if valid_name(&name) {
        vec![Symbol { name, kind }]
    } else {
        Vec::new()
    }
}

fn defined_names(call: &Call) -> Vec<Symbol> {
    let Some((stem, spec)) = call.name.rsplit_once(':') else {
        return Vec::new();
    };
    let Some(name) = call
        .arguments
        .first()
        .and_then(|a| a.name())
        .filter(|n| valid_name(n))
    else {
        return Vec::new();
    };
    if is_variant_generation(&call.name) {
        let Some((base, args)) = name.rsplit_once(':') else {
            return Vec::new();
        };
        if !args.bytes().all(is_specifier) {
            return Vec::new();
        }
        let Some(variants) = call.arguments[1].list() else {
            return Vec::new();
        };
        return variants
            .into_iter()
            .filter_map(|variant| {
                let v = variant.text;
                if !v.bytes().all(is_specifier)
                    || classify(args.as_bytes(), v.as_bytes()) == Conversion::Incompatible
                {
                    return None;
                }
                let generated = format!("{base}:{v}{}", &args[v.len()..]);
                Some(if stem == "prg_generate_conditional_variant" {
                    conditional_names(&generated, &call.arguments[2], false)
                } else {
                    symbol(generated, SymbolKind::Function)
                })
            })
            .flatten()
            .collect();
    }
    if let Some(conditional) = definition_kind(stem) {
        if conditional && matches!(spec, "Nnn" | "Npnn" | "cnn" | "cpnn") {
            return conditional_names(
                &name,
                &call.arguments[call.arguments.len() - 2],
                stem.contains("_protected"),
            );
        }
        if !conditional
            && matches!(
                spec,
                "Nn" | "Ne"
                    | "Nx"
                    | "Npn"
                    | "Npe"
                    | "Npx"
                    | "cn"
                    | "ce"
                    | "cx"
                    | "cpn"
                    | "cpe"
                    | "cpx"
            )
        {
            return symbol(name, SymbolKind::Function);
        }
    }
    if matches!(stem, "cs_new_eq" | "cs_set_eq" | "cs_gset_eq")
        && matches!(spec, "NN" | "Nc" | "cN" | "cc")
    {
        return symbol(name, SymbolKind::Function);
    }
    if matches!(
        stem,
        "prg_new_eq_conditional" | "prg_set_eq_conditional" | "prg_gset_eq_conditional"
    ) && matches!(spec, "NNn" | "Ncn" | "cNn" | "ccn")
    {
        return conditional_names(&name, &call.arguments[2], false);
    }
    if let Some(kind) = variable_constructor_kind(stem, spec) {
        return symbol(name, kind);
    }
    Vec::new()
}

fn variable_constructor_kind(stem: &str, spec: &str) -> Option<SymbolKind> {
    // The suggestion catalog is incomplete, so it cannot establish which calls
    // define names. Explicit constructor shapes also exclude APIs that create
    // messages, hooks, or objects rather than control sequences.
    match (stem, spec) {
        (
            "bool_new"
            | "box_new"
            | "cctab_new"
            | "clist_new"
            | "coffin_new"
            | "dim_new"
            | "flag_new"
            | "fp_new"
            | "int_new"
            | "ior_new"
            | "iow_new"
            | "muskip_new"
            | "prop_new"
            | "prop_new_linked"
            | "seq_new"
            | "skip_new"
            | "str_new"
            | "tl_new"
            | "box_clear_new"
            | "box_gclear_new"
            | "clist_clear_new"
            | "clist_gclear_new"
            | "flag_clear_new"
            | "prop_clear_new"
            | "prop_gclear_new"
            | "prop_clear_new_linked"
            | "prop_gclear_new_linked"
            | "seq_clear_new"
            | "seq_gclear_new"
            | "str_clear_new"
            | "str_gclear_new"
            | "tl_clear_new"
            | "tl_gclear_new"
            | "dim_zero_new"
            | "dim_gzero_new"
            | "fp_zero_new"
            | "fp_gzero_new"
            | "int_zero_new"
            | "int_gzero_new"
            | "muskip_zero_new"
            | "muskip_gzero_new"
            | "skip_zero_new"
            | "skip_gzero_new",
            "N" | "c",
        )
        | ("bitset_new", "N" | "c" | "Nn" | "cn")
        | ("fparray_new" | "intarray_new", "Nn" | "cn")
        | ("quark_new" | "regex_new" | "scan_new", "N") => Some(SymbolKind::Variable),
        (
            "bool_const"
            | "cctab_const"
            | "dim_const"
            | "fp_const"
            | "int_const"
            | "muskip_const"
            | "skip_const"
            | "seq_const_from_clist"
            | "intarray_const_from_clist"
            | "prop_const_from_keyval"
            | "prop_const_linked_from_keyval",
            "Nn" | "cn",
        )
        | ("clist_const", "Nn" | "Ne" | "Nx" | "cn" | "ce" | "cx")
        | ("str_const" | "tl_const", "Nn" | "NV" | "Ne" | "Nx" | "cn" | "cV" | "ce" | "cx")
        | ("regex_const", "Nn") => Some(SymbolKind::Constant),
        _ => None,
    }
}

fn conditional_names(name: &str, forms: &super::calls::Argument, protected: bool) -> Vec<Symbol> {
    let Some((base, args)) = name.rsplit_once(':') else {
        return Vec::new();
    };
    if !args.bytes().all(is_specifier) {
        return Vec::new();
    }
    let Some(forms) = forms.list() else {
        return Vec::new();
    };
    if forms
        .iter()
        .any(|f| !matches!(f.text.as_str(), "p" | "T" | "F" | "TF"))
    {
        return Vec::new();
    }
    forms
        .into_iter()
        .filter_map(|form| {
            let name = match form.text.as_str() {
                "p" if protected => return None,
                "p" => format!("{base}_p:{args}"),
                branch => format!("{name}{branch}"),
            };
            Some(Symbol {
                name,
                kind: SymbolKind::Function,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn symbols(body: &str) -> Vec<Symbol> {
        let text = format!("\\ExplSyntaxOn\n{body}\n\\ExplSyntaxOff\n");
        let parsed = parse(&text);
        assert_eq!(parsed.syntax().text().to_string(), text);
        collect(&parsed.syntax())
    }

    fn has(body: &str, name: &str, kind: SymbolKind) {
        let got = symbols(body);
        assert!(
            got.contains(&Symbol {
                name: name.into(),
                kind
            }),
            "{body}: {got:?}"
        );
    }

    #[test]
    fn literal_functions_and_aliases() {
        for body in [
            r"\cs_new:Npn \demo:n #1 {#1}",
            r"\cs_set_protected:Nn \demo:n {#1}",
            r"\cs_gset_nopar:cpx {demo:n} #1 {#1}",
            r"\cs_new_eq:NN \demo:n \other:n",
            r"\cs_set_eq:cN {demo:n} \other:n",
        ] {
            has(body, "demo:n", SymbolKind::Function);
        }
    }

    #[test]
    fn variables_constants_and_literal_c_names() {
        has(r"\tl_new:N \l_demo_tl", "l_demo_tl", SymbolKind::Variable);
        has(
            r"\int_zero_new:c {l_demo_int}",
            "l_demo_int",
            SymbolKind::Variable,
        );
        has(
            r"\tl_const:Nn \c_demo_tl {text}",
            "c_demo_tl",
            SymbolKind::Constant,
        );
        has(
            r"\seq_gclear_new:N \g_demo_seq",
            "g_demo_seq",
            SymbolKind::Variable,
        );
        has(
            r"\intarray_new:Nn \g_demo_intarray {4}",
            "g_demo_intarray",
            SymbolKind::Variable,
        );
    }

    #[test]
    fn multiword_variable_and_constant_constructors() {
        for (body, name, kind) in [
            (
                r"\seq_const_from_clist:Nn \c_demo_seq {a,b}",
                "c_demo_seq",
                SymbolKind::Constant,
            ),
            (
                r"\seq_const_from_clist:cn {c_demo_seq} {a,b}",
                "c_demo_seq",
                SymbolKind::Constant,
            ),
            (
                r"\prop_const_from_keyval:Nn \c_demo_prop {a=b}",
                "c_demo_prop",
                SymbolKind::Constant,
            ),
            (
                r"\prop_const_from_keyval:cn {c_demo_prop} {a=b}",
                "c_demo_prop",
                SymbolKind::Constant,
            ),
            (
                r"\prop_const_linked_from_keyval:Nn \c_demo_prop {a=b}",
                "c_demo_prop",
                SymbolKind::Constant,
            ),
            (
                r"\prop_const_linked_from_keyval:cn {c_demo_prop} {a=b}",
                "c_demo_prop",
                SymbolKind::Constant,
            ),
            (
                r"\intarray_const_from_clist:Nn \c_demo_intarray {1,2}",
                "c_demo_intarray",
                SymbolKind::Constant,
            ),
            (
                r"\intarray_const_from_clist:cn {c_demo_intarray} {1,2}",
                "c_demo_intarray",
                SymbolKind::Constant,
            ),
            (
                r"\prop_new_linked:N \l_demo_prop",
                "l_demo_prop",
                SymbolKind::Variable,
            ),
            (
                r"\prop_new_linked:c {l_demo_prop}",
                "l_demo_prop",
                SymbolKind::Variable,
            ),
            (
                r"\prop_clear_new_linked:N \l_demo_prop",
                "l_demo_prop",
                SymbolKind::Variable,
            ),
            (
                r"\prop_clear_new_linked:c {l_demo_prop}",
                "l_demo_prop",
                SymbolKind::Variable,
            ),
            (
                r"\prop_gclear_new_linked:N \g_demo_prop",
                "g_demo_prop",
                SymbolKind::Variable,
            ),
            (
                r"\prop_gclear_new_linked:c {g_demo_prop}",
                "g_demo_prop",
                SymbolKind::Variable,
            ),
        ] {
            has(body, name, kind);
        }
    }

    #[test]
    fn constructor_variants_do_not_require_catalog_entries() {
        for body in [
            r"\tl_const:Ne \c_demo_tl {text}",
            r"\tl_const:Nx \c_demo_tl {text}",
            r"\tl_const:ce {c_demo_tl} {text}",
            r"\tl_const:cx {c_demo_tl} {text}",
            r"\tl_const:NV \c_demo_tl \l_tmpa_tl",
            r"\tl_const:cV {c_demo_tl} \l_tmpa_tl",
        ] {
            has(body, "c_demo_tl", SymbolKind::Constant);
        }
    }

    #[test]
    fn unsupported_or_incomplete_constructors_are_not_definitions() {
        for body in [
            r"\tl_new:Nn \l_fake_tl {text}",
            r"\tl_const:N \c_fake_tl",
            r"\tl_const:Nx \c_fake_tl",
            r"\tl_const:Nx \c_fake_tl {text",
            r"\tl_const:cx {c_\module_tl} {text}",
            r"\seq_const_from_clist:Nn \c_fake_seq",
            r"\seq_const_from_clist:cn {c_\module_seq} {a,b}",
            r"\tl_const_from_clist:Nn \c_fake_tl {a,b}",
            r"\seq_const_from_keyval:Nn \c_fake_seq {a=b}",
            r"\tl_new_linked:N \l_fake_tl",
            r"\intarray_new:N \g_fake_intarray",
            r"\msg_const:nnn {module}{name}{text}",
        ] {
            assert!(symbols(body).is_empty(), "{body}: {:?}", symbols(body));
        }
    }

    #[test]
    fn conditional_and_variant_names() {
        let got = symbols(r"\prg_new_conditional:Npnn \demo:n #1 {p,T,TF} {\prg_return_true:}");
        for name in ["demo_p:n", "demo:nT", "demo:nTF"] {
            assert!(got.iter().any(|s| s.name == name), "{got:?}");
        }
        assert!(
            !got.iter()
                .any(|s| s.name == "demo:nF" || s.name == "demo:n")
        );
        has(
            r"\cs_generate_variant:Nn \demo:nn {V,ne}",
            "demo:Vn",
            SymbolKind::Function,
        );
        has(
            r"\cs_generate_variant:cn {demo:nn} {V,ne}",
            "demo:ne",
            SymbolKind::Function,
        );
        has(
            r"\prg_generate_conditional_variant:Nnn \demo:nn {V} {p,TF}",
            "demo_p:Vn",
            SymbolKind::Function,
        );
        has(
            r"\prg_generate_conditional_variant:Nnn \demo:nn {V} {p,TF}",
            "demo:VnTF",
            SymbolKind::Function,
        );
    }

    #[test]
    fn unexpanded_bodies_and_branches_are_scanned() {
        has(
            r"\cs_new:Nn \outer: {\tl_new:N \l_inner_tl}",
            "l_inner_tl",
            SymbolKind::Variable,
        );
        has(
            r"\bool_if:NT \l_flag_bool {\tl_new:N \l_inner_tl}",
            "l_inner_tl",
            SymbolKind::Variable,
        );
    }

    #[test]
    fn unresolved_or_stored_names_are_not_definitions() {
        for body in [
            r"\tl_new:c {l_\module_tl}",
            r"\cs_new:Npn \demo:n #1",
            r"\cs_generate_variant:Nn \demo:n {nn}",
            r"\cs_generate_variant:Nn \demo:n {\variants}",
            r"\tl_set:Nn \l_data_tl {\tl_new:N \l_fake_tl}",
            r"\use:n {\tl_new:N \l_fake_tl}",
            r"\msg_new:nnn {module}{name}{text}",
            r"\hook_new:n {name}",
            r"% \tl_new:N \l_fake_tl",
            r"\verb|\tl_new:N \l_fake_tl|",
        ] {
            assert!(symbols(body).is_empty(), "{body}: {:?}", symbols(body));
        }
    }
}
