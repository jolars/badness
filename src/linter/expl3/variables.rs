//! Variable operand contracts for a subset of the public LaTeX3 kernel API.

use std::collections::HashMap;

use rowan::{TextRange, TextSize};

use super::{Call, Expl3Index};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Effect {
    Declare,
    Constant,
    Local,
    Global,
    Read,
}

struct Operation<'a> {
    kind: &'a str,
    effect: Effect,
    source: Option<&'a str>,
    clears: bool,
}

fn core_type(kind: &str) -> bool {
    matches!(kind, "tl" | "str" | "seq" | "clist" | "prop" | "bool") || numeric(kind)
}

fn numeric(kind: &str) -> bool {
    matches!(kind, "int" | "dim" | "skip" | "muskip" | "fp")
}

fn operation(name: &str) -> Option<Operation<'_>> {
    use Effect::*;
    let (stem, spec) = name.split_once(':')?;
    let (kind, verb) = stem.split_once('_')?;
    if !core_type(kind) {
        return None;
    }
    let (base, global) = verb.strip_prefix('g').map_or((verb, false), |v| (v, true));
    let assignment = if global { Global } else { Local };
    let one = matches!(spec, "N" | "c");
    let value = matches!(spec, "Nn" | "cn");
    let pair = matches!(spec, "NN" | "Nc" | "cN" | "cc");
    let text = matches!(kind, "tl" | "str" | "clist");
    let collection = text || matches!(kind, "seq" | "prop");
    let expanded_text = matches!(spec, "Ne" | "ce" | "Nx" | "cx" | "NV" | "cV");
    let mut source = None;
    let effect = match base {
        "new" if !global && one => Declare,
        "const"
            if !global
                && (text || kind == "bool" || numeric(kind))
                && (value
                    || (text
                        && expanded_text
                        && (kind != "clist" || !matches!(spec, "NV" | "cV")))) =>
        {
            Constant
        }
        "set"
            if (text || kind == "bool" || numeric(kind))
                && (value
                    || (text && expanded_text)
                    || (numeric(kind) && matches!(spec, "NV" | "cV"))) =>
        {
            assignment
        }
        "set_eq" if pair => {
            source = Some(kind);
            assignment
        }
        "set_from_clist" if kind == "seq" && (pair || value) => {
            source = pair.then_some("clist");
            assignment
        }
        "clear" if collection && one => assignment,
        "zero" if numeric(kind) && one => assignment,
        "add" | "sub" if numeric(kind) && value => assignment,
        "set_true" | "set_false" if kind == "bool" && one => assignment,
        "put_left" | "put_right" if (text || kind == "seq") && (value || expanded_text) => {
            assignment
        }
        "put" if kind == "prop" && matches!(spec, "Nnn" | "cnn") => assignment,
        "count" if !global && collection && one => Read,
        "if_p" if !global && kind == "bool" && one => Read,
        "if" if !global
            && kind == "bool"
            && matches!(spec, "NT" | "NF" | "NTF" | "cT" | "cF" | "cTF") =>
        {
            Read
        }
        _ => return None,
    };
    Some(Operation {
        kind,
        effect,
        source,
        clears: base == "clear",
    })
}

fn fits(from: &str, into: &str) -> bool {
    from == into || (into == "tl" && matches!(from, "str" | "clist"))
}

fn variable(name: &str) -> Option<(&str, &str)> {
    let (scope, rest) = name.split_once('_')?;
    let (description, kind) = rest.rsplit_once('_')?;
    (matches!(scope, "l" | "g" | "c")
        && core_type(kind)
        && description.bytes().any(|b| b.is_ascii_alphabetic())
        && description
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'@')))
    .then_some((scope, kind))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum VariableFinding {
    Type,
    Mutability,
    Scope,
}

pub(crate) struct Finding {
    pub kind: VariableFinding,
    pub range: TextRange,
    pub message: String,
}

#[derive(Default)]
pub(crate) struct VariableIndex(HashMap<TextSize, Vec<Finding>>);

impl VariableIndex {
    pub fn recognizes(name: &str) -> bool {
        operation(name).is_some()
    }

    pub fn build(calls: &Expl3Index) -> Self {
        Self(
            calls
                .calls()
                .filter_map(|call| {
                    let findings = findings(call);
                    (!findings.is_empty()).then_some((call.offset, findings))
                })
                .collect(),
        )
    }

    pub fn get(&self, offset: TextSize) -> &[Finding] {
        self.0.get(&offset).map_or(&[], Vec::as_slice)
    }
}

fn findings(call: &Call) -> Vec<Finding> {
    use Effect::*;
    use VariableFinding::*;
    let (Some(nested), Some(op)) = (call.variable_context, operation(&call.name)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (index, expected) in [Some(op.kind), op.source].into_iter().enumerate() {
        let Some(expected) = expected else { continue };
        let Some(operand) = call.arguments.get(index).and_then(|arg| arg.literal_name()) else {
            continue;
        };
        let Some((scope, actual)) = variable(&operand.text) else {
            continue;
        };
        let mut push = |kind, message| {
            out.push(Finding {
                kind,
                range: operand.range,
                message,
            })
        };
        let compatible = match (index, op.effect) {
            (1, _) => fits(actual, expected),
            // String counting stringifies the contents of any text variable.
            (_, Read) if expected == "str" => fits(actual, "tl"),
            (_, Read) if expected == "tl" => fits(actual, expected),
            // Collection counts depend on the representation of their own type.
            (_, Read) => actual == expected,
            _ => fits(expected, actual) || (op.clears && fits(actual, expected)),
        };
        if !compatible {
            push(
                Type,
                format!(
                    "expl3 operation `\\{}` expects type `{expected}` for this operand, which has type `{actual}`",
                    call.name
                ),
            );
        }
        if index == 0 {
            match (op.effect, scope) {
                (Constant, "l" | "g") => push(
                    Mutability,
                    format!(
                        "creating a constant with variable name `\\{}`",
                        operand.text
                    ),
                ),
                (Local | Global, "c") => push(
                    Mutability,
                    format!("assigning to constant `\\{}`", operand.text),
                ),
                (Local, "g") if nested => push(
                    Scope,
                    format!("assigning locally to global variable `\\{}`", operand.text),
                ),
                (Global, "l") if nested => push(
                    Scope,
                    format!("assigning globally to local variable `\\{}`", operand.text),
                ),
                _ => {}
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_supported_kernel_signatures() {
        for name in [
            "tl_const:ce",
            "str_gset_eq:Nc",
            "prop_gput:cnn",
            "int_gadd:cn",
            "seq_set_from_clist:cN",
            "bool_if_p:c",
            "fp_gzero:N",
        ] {
            assert!(operation(name).is_some(), "{name}");
        }
        for name in [
            "tl_set:NN",
            "tl_gnew:N",
            "seq_set:Nn",
            "seq_set_from_clist:Nx",
            "prop_gset:Nn",
            "str_gcount:N",
            "bool_set:NV",
            "io_set:Nn",
        ] {
            assert!(operation(name).is_none(), "{name}");
        }
    }

    #[test]
    fn variable_names_carry_type_and_scope() {
        assert_eq!(variable("l_tmpa_int"), Some(("l", "int")));
        assert_eq!(variable("g__demo_name_str"), Some(("g", "str")));
        assert_eq!(variable("c_@@_size_dim"), Some(("c", "dim")));
        assert_eq!(variable("c_foo_dim:"), None);
        assert_eq!(variable("l__tl"), None);
        assert_eq!(variable("g_foo"), None);
        assert_eq!(variable("x_demo_name_str"), None);
    }
}
