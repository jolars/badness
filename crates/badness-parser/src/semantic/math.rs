//! Static math-atom classification over the lossless CST.
//!
//! The generated baseline comes from unicode-math; a deliberately small curated
//! tier supplies LaTeX aliases, primitive class constructors, and the exceptions
//! where a TeX spacing class is not a pairable delimiter. Classification is pure
//! over shipped data and source shape—package scope and user definitions do not
//! participate.

use rowan::{TextRange, TextSize};

use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken};

/// The useful TeX math-atom class family.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MathClass {
    #[default]
    Ord,
    Op,
    Bin,
    Rel,
    Open,
    Close,
    Punct,
    Fence,
    Inner,
}

/// Whether an atom is a genuinely pairable delimiter, independently of its TeX
/// spacing class. For example, `\sqrt` is `Open` but has no delimiter role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DelimiterRole {
    Open,
    Close,
    Fence,
}

/// Class metadata without a source location.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MathAtomInfo {
    pub class: MathClass,
    pub delimiter: Option<DelimiterRole>,
}

/// One virtual math atom and the exact source bytes that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MathAtom {
    pub range: TextRange,
    pub class: MathClass,
    pub delimiter: Option<DelimiterRole>,
}

const fn info(class: MathClass, delimiter: Option<DelimiterRole>) -> MathAtomInfo {
    MathAtomInfo { class, delimiter }
}

type MathCommandMap = phf::Map<&'static str, MathAtomInfo>;

include!(concat!(env!("OUT_DIR"), "/math_symbols.rs"));

/// LaTeX and amsmath's named, upright function operators.
///
/// This shared vocabulary also drives the `math-operator-name` lint, which
/// diagnoses these spellings when their leading backslash is omitted.
pub const NAMED_MATH_OPERATORS: &[&str] = &[
    "arccos", "arcsin", "arctan", "arg", "cos", "cosh", "cot", "coth", "csc", "deg", "det", "dim",
    "exp", "gcd", "hom", "inf", "ker", "lg", "lim", "liminf", "limsup", "ln", "log", "max", "min",
    "Pr", "sec", "sin", "sinh", "sup", "tan", "tanh",
];

/// Classify a control-sequence name without its leading backslash.
///
/// Unknown commands conservatively behave as ordinary atoms.
pub fn math_command_info(name: &str) -> MathAtomInfo {
    curated_command_info(name)
        .or_else(|| UNICODE_MATH_COMMANDS.get(name).copied())
        .unwrap_or_default()
}

/// Classify a literal Unicode scalar. Unknown characters are ordinary atoms.
pub fn math_char_info(character: char) -> MathAtomInfo {
    curated_char_info(character)
        .or_else(|| {
            UNICODE_MATH_CHARS
                .binary_search_by_key(&character, |(candidate, _)| *candidate)
                .ok()
                .map(|index| UNICODE_MATH_CHARS[index].1)
        })
        .unwrap_or_default()
}

/// Virtual atoms for one CST element. A coalesced `WORD` yields one atom per
/// Unicode scalar; structural nodes remain one source-spanning atom.
pub fn math_atoms(element: &SyntaxElement) -> MathAtoms<'_> {
    match element {
        SyntaxElement::Token(token) if token.kind() == SyntaxKind::WORD => MathAtoms {
            inner: MathAtomsInner::Word {
                text: token.text(),
                start: token.text_range().start(),
                offset: 0,
            },
        },
        SyntaxElement::Token(token) => MathAtoms {
            inner: MathAtomsInner::One(Some(atom(token.text_range(), token_info(token)))),
        },
        SyntaxElement::Node(node) => MathAtoms {
            inner: MathAtomsInner::One(Some(atom(node.text_range(), node_info(node)))),
        },
    }
}

/// Iterator returned by [`math_atoms`].
pub struct MathAtoms<'a> {
    inner: MathAtomsInner<'a>,
}

enum MathAtomsInner<'a> {
    Word {
        text: &'a str,
        start: TextSize,
        offset: usize,
    },
    One(Option<MathAtom>),
}

impl Iterator for MathAtoms<'_> {
    type Item = MathAtom;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            MathAtomsInner::One(atom) => atom.take(),
            MathAtomsInner::Word {
                text,
                start,
                offset,
            } => {
                let character = text.get(*offset..)?.chars().next()?;
                let len = character.len_utf8();
                let atom_start = *start + TextSize::from(*offset as u32);
                *offset += len;
                let atom_end = *start + TextSize::from(*offset as u32);
                let value = math_char_info(character);
                Some(MathAtom {
                    range: TextRange::new(atom_start, atom_end),
                    class: value.class,
                    delimiter: value.delimiter,
                })
            }
        }
    }
}

fn atom(range: TextRange, value: MathAtomInfo) -> MathAtom {
    MathAtom {
        range,
        class: value.class,
        delimiter: value.delimiter,
    }
}

fn token_info(token: &SyntaxToken) -> MathAtomInfo {
    match token.kind() {
        SyntaxKind::CONTROL_WORD | SyntaxKind::CONTROL_SYMBOL => token
            .text()
            .strip_prefix('\\')
            .map_or_else(MathAtomInfo::default, math_command_info),
        _ => {
            let mut characters = token.text().chars();
            match (characters.next(), characters.next()) {
                (Some(character), None) => math_char_info(character),
                _ => MathAtomInfo::default(),
            }
        }
    }
}

fn node_info(node: &SyntaxNode) -> MathAtomInfo {
    match node.kind() {
        SyntaxKind::COMMAND => node
            .children_with_tokens()
            .filter_map(SyntaxElement::into_token)
            .find(|token| {
                matches!(
                    token.kind(),
                    SyntaxKind::CONTROL_WORD | SyntaxKind::CONTROL_SYMBOL
                )
            })
            .as_ref()
            .map_or_else(MathAtomInfo::default, token_info),
        SyntaxKind::SCRIPTED => node
            .children_with_tokens()
            .find(|element| {
                !matches!(
                    element.kind(),
                    SyntaxKind::WHITESPACE
                        | SyntaxKind::NEWLINE
                        | SyntaxKind::SUBSCRIPT
                        | SyntaxKind::SUPERSCRIPT
                )
            })
            .and_then(|base| math_atoms(&base).next())
            .map_or_else(MathAtomInfo::default, |base| {
                info(base.class, base.delimiter)
            }),
        SyntaxKind::GROUP
        | SyntaxKind::OPTIONAL
        | SyntaxKind::LEFT_RIGHT
        | SyntaxKind::ENVIRONMENT => info(MathClass::Inner, None),
        _ => MathAtomInfo::default(),
    }
}

fn curated_char_info(character: char) -> Option<MathAtomInfo> {
    match character {
        // unicode-math applies these ASCII remaps outside its symbol table.
        '*' | '-' => Some(info(MathClass::Bin, None)),
        // TeX spacing classes do not imply a paired delimiter.
        '!' => Some(info(MathClass::Close, None)),
        '√' => Some(info(MathClass::Ord, None)),
        '∛' | '∜' | '⟌' => Some(info(MathClass::Open, None)),
        _ => None,
    }
}

fn curated_command_info(name: &str) -> Option<MathAtomInfo> {
    if NAMED_MATH_OPERATORS.contains(&name) {
        return Some(info(MathClass::Op, None));
    }
    let value = match name {
        // `\operatorname` constructs an operator from its argument.
        "operatorname" => info(MathClass::Op, None),

        // Primitive class constructors classify the whole command result, but do
        // not promise that arbitrary content passed to `\mathopen` is pairable.
        "mathord" => info(MathClass::Ord, None),
        "mathop" => info(MathClass::Op, None),
        "mathbin" => info(MathClass::Bin, None),
        "mathrel" => info(MathClass::Rel, None),
        "mathopen" => info(MathClass::Open, None),
        "mathclose" => info(MathClass::Close, None),
        "mathpunct" => info(MathClass::Punct, None),
        "mathinner" => info(MathClass::Inner, None),

        // The formatter's established relation vocabulary, including trusted
        // kernel/mathtools aliases not present as primary unicode-math commands.
        "le" | "leq" | "ge" | "geq" | "ne" | "neq" | "equiv" | "approx" | "approxeq" | "sim"
        | "simeq" | "cong" | "propto" | "asymp" | "doteq" | "models" | "vdash" | "dashv"
        | "perp" | "parallel" | "mid" | "in" | "ni" | "notin" | "subset" | "subseteq"
        | "subsetneq" | "supset" | "supseteq" | "supsetneq" | "sqsubseteq" | "sqsupseteq"
        | "prec" | "preceq" | "succ" | "succeq" | "ll" | "gg" | "lll" | "ggg" | "to"
        | "rightarrow" | "longrightarrow" | "Rightarrow" | "Longrightarrow" | "implies"
        | "impliedby" | "iff" | "mapsto" | "longmapsto" | "leftarrow" | "Leftarrow" | "gets"
        | "leftrightarrow" | "Leftrightarrow" | "Longleftrightarrow" | "hookrightarrow"
        | "hookleftarrow" | "triangleq" | "coloneq" | "Coloneq" | "coloneqq" | "Coloneqq"
        | "eqcolon" | "Eqcolon" | "eqqcolon" | "Eqqcolon" | "colonapprox" | "Colonapprox"
        | "colonsim" | "Colonsim" | "lesssim" | "gtrsim" => info(MathClass::Rel, None),

        // The established binary vocabulary. `bigtriangledown` deliberately
        // overrides unicode-math's `Ord` classification for compatibility.
        "pm" | "mp" | "times" | "div" | "cdot" | "ast" | "star" | "circ" | "bullet" | "cup"
        | "cap" | "uplus" | "sqcup" | "sqcap" | "vee" | "wedge" | "lor" | "land" | "oplus"
        | "ominus" | "otimes" | "oslash" | "odot" | "setminus" | "amalg" | "diamond" | "wr"
        | "dagger" | "ddagger" | "bigtriangleup" | "bigtriangledown" | "triangleleft"
        | "triangleright" => info(MathClass::Bin, None),

        // Control-symbol delimiters and LaTeX's directional vertical-bar aliases.
        "{" | "lvert" | "lVert" => info(MathClass::Open, Some(DelimiterRole::Open)),
        "}" | "rvert" | "rVert" => info(MathClass::Close, Some(DelimiterRole::Close)),
        "|" => info(MathClass::Fence, Some(DelimiterRole::Fence)),

        // These upstream `Open`/`Close` classes affect TeX spacing but do not form
        // pairs for Badness's structural bracket accounting.
        "sqrt" | "cuberoot" | "fourthroot" | "longdivision" => info(MathClass::Open, None),
        "mathexclam" => info(MathClass::Close, None),
        _ => return None,
    };
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn root(source: &str) -> SyntaxNode {
        SyntaxNode::new_root(parse(source).green)
    }

    #[test]
    fn generated_and_curated_lookups_share_one_default() {
        assert_eq!(UNICODE_MATH_COMMANDS.len(), 2448);
        assert_eq!(math_command_info("nleq").class, MathClass::Rel);
        assert_eq!(math_command_info("sin").class, MathClass::Op);
        assert_eq!(math_command_info("bigtriangledown").class, MathClass::Bin);
        assert_eq!(math_char_info('≤').class, MathClass::Rel);
        assert_eq!(math_char_info(',').class, MathClass::Punct);
        assert_eq!(math_command_info("vert").class, MathClass::Fence);
        assert_eq!(math_char_info('-').class, MathClass::Bin);
        assert_eq!(math_char_info('/').class, MathClass::Ord);
        assert_eq!(
            math_command_info("not-a-real-command"),
            MathAtomInfo::default()
        );
        assert_eq!(math_char_info('🦀'), MathAtomInfo::default());
    }

    #[test]
    fn delimiter_role_is_independent_of_spacing_class() {
        assert_eq!(
            math_command_info("langle").delimiter,
            Some(DelimiterRole::Open)
        );
        assert_eq!(math_command_info("sqrt"), info(MathClass::Open, None));
        assert_eq!(math_char_info('!'), info(MathClass::Close, None));
        assert_eq!(math_char_info('√'), info(MathClass::Ord, None));
    }

    #[test]
    fn word_atoms_have_exact_multibyte_source_spans() {
        let tree = root("$a≤b$");
        let word = tree
            .descendants_with_tokens()
            .filter_map(SyntaxElement::into_token)
            .find(|token| token.text() == "a≤b")
            .expect("coalesced math word");
        let atoms: Vec<_> = math_atoms(&word.into()).collect();
        assert_eq!(
            atoms.iter().map(|atom| atom.class).collect::<Vec<_>>(),
            [MathClass::Ord, MathClass::Rel, MathClass::Ord]
        );
        assert_eq!(
            atoms.iter().map(|atom| atom.range).collect::<Vec<_>>(),
            [
                TextRange::new(1.into(), 2.into()),
                TextRange::new(2.into(), 5.into()),
                TextRange::new(5.into(), 6.into()),
            ]
        );
    }

    #[test]
    fn commands_and_scripted_bases_are_single_atoms() {
        let tree = root("$\\leq \\}^{1/2}$");
        let command = tree
            .descendants()
            .find(|node| node.kind() == SyntaxKind::COMMAND)
            .expect("relation command");
        let command_atom = math_atoms(&command.clone().into()).next().unwrap();
        assert_eq!(command_atom.class, MathClass::Rel);
        assert_eq!(command_atom.range, command.text_range());

        let scripted = tree
            .descendants()
            .find(|node| node.kind() == SyntaxKind::SCRIPTED)
            .expect("scripted delimiter");
        let scripted_atom = math_atoms(&scripted.clone().into()).next().unwrap();
        assert_eq!(scripted_atom.delimiter, Some(DelimiterRole::Close));
        assert_eq!(scripted_atom.range, scripted.text_range());
    }

    #[test]
    fn structural_subformulas_are_inner_atoms() {
        let tree = root("${x}$");
        let group = tree
            .descendants()
            .find(|node| node.kind() == SyntaxKind::GROUP)
            .expect("math group");
        assert_eq!(
            math_atoms(&group.into()).next().unwrap().class,
            MathClass::Inner
        );
    }
}
