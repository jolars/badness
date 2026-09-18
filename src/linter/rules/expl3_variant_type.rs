//! Incompatible and deprecated expl3 variant conversions. The intended calling
//! convention is unknown, so changing either argspec cannot be an automatic fix.

use crate::ast::command_name;
use crate::linter::diagnostic::Diagnostic;
use crate::linter::expl3::is_variant_generation;
use crate::syntax::{SyntaxElement, SyntaxKind};

use super::{Example, Rule, RuleContext};

pub struct Expl3VariantType;

const EXAMPLES: &[Example] = &[
    Example {
        caption: "A variant cannot add arguments:",
        source: "\\ExplSyntaxOn\n\\cs_generate_variant:Nn \\demo_use:n { nn }\n\\ExplSyntaxOff\n",
    },
    Example {
        caption: "Converting a single-token argument to a token-list argument is deprecated:",
        source: "\\ExplSyntaxOn\n\\cs_generate_variant:Nn \\demo_use:Nn { nn }\n\\ExplSyntaxOff\n",
    },
];

impl Rule for Expl3VariantType {
    fn id(&self) -> &'static str {
        "expl3-variant-type"
    }

    fn description(&self) -> &'static str {
        "Flag incompatible or deprecated argument-type conversions in literal \
         expl3 variant-generation calls. A shorter variant inherits the original \
         suffix. Unchanged letters are valid, `N` may become `c`, and `n` may \
         become `o`, `V`, `v`, `f`, `e`, or `x`. Conversions between these two \
         families are deprecated; other changes are incompatible. Checks cover \
         recognized executable calls and unexpanded function bodies, not stored \
         token lists or unresolved expansion. Report-only: the intended signature \
         is the author's decision."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }
    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::COMMAND]
    }

    fn check(&self, el: &SyntaxElement, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(node) = el.as_node() else { return };
        if !command_name(node).is_some_and(|name| is_variant_generation(&name)) {
            return;
        }
        let Some(call) = ctx.expl3_call(node) else {
            return;
        };
        let Some(name) = call.arguments[0].name() else {
            return;
        };
        let Some((_, base)) = name.rsplit_once(':') else {
            return;
        };
        if !base.bytes().all(is_specifier) {
            return;
        }
        let Some(variants) = call.arguments[1].list() else {
            return;
        };
        // An unrecognized list is unknown data, not evidence of a bad variant.
        if !variants.iter().all(|v| v.text.bytes().all(is_specifier)) {
            return;
        }
        for variant in variants {
            let conversion = classify(base.as_bytes(), variant.text.as_bytes());
            let adjective = match conversion {
                Conversion::Compatible => continue,
                Conversion::Deprecated => "deprecated",
                Conversion::Incompatible => "incompatible",
            };
            sink.push(Diagnostic {
                rule: self.id(),
                severity: self.default_severity(),
                path: Default::default(),
                start: variant.range.start().into(),
                end: variant.range.end().into(),
                message: format!(
                    "{adjective} expl3 variant conversion from `{base}` to `{}`",
                    variant.text
                ),
                fix: None,
                related: Vec::new(),
            });
        }
    }
}

fn is_specifier(c: u8) -> bool {
    b"NVncvoxefTFpwD".contains(&c)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Conversion {
    Compatible,
    Deprecated,
    Incompatible,
}

fn classify(base: &[u8], variant: &[u8]) -> Conversion {
    if variant.len() > base.len() {
        return Conversion::Incompatible;
    }
    base.iter()
        .zip(variant)
        .map(|(&from, &to)| {
            if from == to || from == b'N' && to == b'c' || from == b'n' && b"oVvfex".contains(&to) {
                Conversion::Compatible
            } else if from == b'n' && b"Nc".contains(&to)
                || from == b'N' && b"noVvfex".contains(&to)
            {
                Conversion::Deprecated
            } else {
                Conversion::Incompatible
            }
        })
        .max()
        .unwrap_or(Conversion::Compatible)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expl3_variant_conversion_matrix_matches_the_reference() {
        // The table is pinned to explcheck 48bc583, including higher-order
        // variants, whose already-expanded slots must remain unchanged.
        for (from, compatible, deprecated) in [
            (b'N', "Nc", "noVvfex"),
            (b'n', "noVvfex", "Nc"),
            (b'c', "c", ""),
            (b'V', "V", ""),
            (b'v', "v", ""),
            (b'o', "o", ""),
            (b'x', "x", ""),
            (b'e', "e", ""),
            (b'f', "f", ""),
            (b'T', "T", ""),
            (b'F', "F", ""),
            (b'p', "p", ""),
            (b'w', "w", ""),
            (b'D', "D", ""),
        ] {
            for to in b"NVncvoxefTFpwD" {
                let expected = if compatible.as_bytes().contains(to) {
                    Conversion::Compatible
                } else if deprecated.as_bytes().contains(to) {
                    Conversion::Deprecated
                } else {
                    Conversion::Incompatible
                };
                assert_eq!(
                    classify(&[from], &[*to]),
                    expected,
                    "{} -> {}",
                    from as char,
                    *to as char
                );
            }
        }
        assert_eq!(classify(b"Nn", b"c"), Conversion::Compatible);
        assert_eq!(classify(b"cx", b"Ne"), Conversion::Incompatible);
        assert_eq!(classify(b"Nn", b"nnn"), Conversion::Incompatible);
    }
}
