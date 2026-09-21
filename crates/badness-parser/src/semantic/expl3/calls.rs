//! Conservative expl3 call reads shared by semantic lint rules and completion.
//!
//! The CST proves argument attachment, not execution. This index enters only
//! recognized executable bodies, and an unresolved head stops recognition in
//! its surrounding stream. Stored token lists and expansion wrappers stay
//! opaque. Parameter escaping is a source-mapped view of known definition
//! bodies; it never substitutes arguments, expands macros, or changes the tree.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use rowan::{TextRange, TextSize};

use crate::ast::{AstNode, Command, Environment, Group, command_name};
use crate::parser::lexer::{ExplToggle, expl_toggle};
use crate::semantic::expl3::{Expl3Slot, expl3_slots};
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode, is_trivia};

#[derive(Default)]
pub struct Expl3Index {
    calls: HashMap<TextSize, Call>,
}

pub struct Call {
    pub name: String,
    pub arguments: Vec<Argument>,
    /// Actual message parameters after the enclosing definitions' hash escapes.
    pub message_parameters: Vec<(TextRange, u8)>,
}

pub struct Argument {
    pub specifier: u8,
    elements: Vec<SyntaxElement>,
}

pub struct Literal {
    pub text: String,
    pub range: TextRange,
}

struct Stream {
    elements: VecDeque<SyntaxElement>,
    definitions: Arc<[u8]>,
    active: bool,
}

impl Expl3Index {
    /// Recognized calls in source order.
    pub fn calls(&self) -> impl Iterator<Item = &Call> {
        let mut calls: Vec<_> = self.calls.iter().collect();
        calls.sort_unstable_by_key(|(offset, _)| **offset);
        calls.into_iter().map(|(_, call)| call)
    }

    pub fn get(&self, node: &SyntaxNode) -> Option<&Call> {
        self.calls.get(&node.text_range().start())
    }

    pub fn build(root: &SyntaxNode) -> Self {
        let mut index = Self::default();
        let documentation = root
            .descendants_with_tokens()
            .any(|el| el.kind() == SyntaxKind::DOC_MARGIN);
        // Docstrip extracts only macrocode bodies. Documentation can itself
        // contain live-looking expl3 examples, including syntax toggles.
        let mut pending = if documentation {
            root.descendants()
                .filter_map(Environment::cast)
                .filter(|env| {
                    env.name()
                        .is_some_and(|name| matches!(name.as_str(), "macrocode" | "macrocode*"))
                })
                .map(|env| environment_stream(env.syntax(), Arc::from([]), true))
                .collect()
        } else {
            vec![Stream {
                elements: root.children_with_tokens().collect(),
                definitions: Arc::from([]),
                active: false,
            }]
        };
        while let Some(mut stream) = pending.pop() {
            let mut known_boundary = true;
            while let Some(element) = stream.elements.pop_front() {
                let Some(node) = element.as_node() else {
                    // A parameter or expansion token at statement position may
                    // consume subsequent commands. Trivia cannot restore proof.
                    if stream.active && !is_stream_trivia(element.kind()) {
                        known_boundary = false;
                    }
                    continue;
                };
                match node.kind() {
                    SyntaxKind::PARAGRAPH | SyntaxKind::TEXT => {
                        for child in node
                            .children_with_tokens()
                            .collect::<Vec<_>>()
                            .into_iter()
                            .rev()
                        {
                            stream.elements.push_front(child);
                        }
                    }
                    SyntaxKind::DOC_COMMENT => {}
                    SyntaxKind::ENVIRONMENT => {
                        let Some(env) = Environment::cast(node.clone()) else {
                            continue;
                        };
                        let Some(name) = env.name() else { continue };
                        if known_boundary
                            && matches!(name.as_str(), "document" | "macrocode" | "macrocode*")
                        {
                            pending.push(environment_stream(
                                node,
                                stream.definitions.clone(),
                                stream.active || name.starts_with("macrocode"),
                            ));
                        }
                    }
                    SyntaxKind::GROUP if known_boundary => {
                        if let Some(group) = Group::cast(node.clone()).filter(closed_group) {
                            pending.push(body_stream(
                                &group,
                                stream.definitions.clone(),
                                stream.active,
                            ));
                        }
                    }
                    SyntaxKind::COMMAND => {
                        let Some(name) = command_name(node) else {
                            known_boundary &= !stream.active;
                            continue;
                        };
                        if let Some(toggle) = expl_toggle(&format!("\\{name}")) {
                            if (known_boundary || !stream.active) && !toggle_may_be_an_operand(node)
                            {
                                stream.active = toggle == ExplToggle::On;
                                known_boundary = true;
                            } else {
                                // A referenced toggle can still have changed the
                                // lexer mode; that does not prove execution.
                                stream.active = true;
                                known_boundary = false;
                            }
                            continue;
                        }
                        if !known_boundary {
                            continue;
                        }
                        let Some(mut call) = read_call(node) else {
                            known_boundary &= !stream.active;
                            continue;
                        };
                        stream.active = true;
                        if call.name.starts_with("exp_") || call.name.starts_with("use:") {
                            // Expansion drivers can expose a head that consumes
                            // further siblings beyond the driver's own argspec.
                            known_boundary = false;
                            continue;
                        }
                        if let Some((body, count)) = definition_body(&call, &stream.definitions) {
                            let mut definitions = stream.definitions.to_vec();
                            definitions.push(count);
                            pending.push(body_stream(&body, definitions.into(), true));
                        }
                        for argument in &call.arguments {
                            if matches!(argument.specifier, b'T' | b'F')
                                && let Some(group) = argument.group()
                            {
                                pending.push(body_stream(&group, stream.definitions.clone(), true));
                            }
                        }
                        if is_message_definition(&call.name) {
                            for argument in &call.arguments[2..] {
                                if let Some(atoms) = argument.atoms(&stream.definitions) {
                                    call.message_parameters.extend(message_parameters(&atoms));
                                }
                            }
                        }
                        index.calls.insert(node.text_range().start(), call);
                    }
                    _ => {
                        if stream.active {
                            known_boundary = false;
                        }
                    }
                }
            }
        }
        index
    }
}

fn environment_stream(node: &SyntaxNode, definitions: Arc<[u8]>, active: bool) -> Stream {
    Stream {
        elements: node
            .children_with_tokens()
            .filter(|el| !matches!(el.kind(), SyntaxKind::BEGIN | SyntaxKind::END))
            .collect(),
        definitions,
        active,
    }
}

fn toggle_may_be_an_operand(node: &SyntaxNode) -> bool {
    if crate::semantic::define::in_reference_position(node) {
        return true;
    }
    let mut previous = node.prev_sibling_or_token();
    while let Some(element) = previous {
        if !is_trivia(element.kind()) && element.kind() != SyntaxKind::DOC_COMMENT {
            return element.as_node().is_some_and(|previous| {
                previous.kind() == SyntaxKind::COMMAND
                    && read_call(previous).is_none()
                    && !command_name(previous)
                        .is_some_and(|name| expl_toggle(&format!("\\{name}")).is_some())
                    && !previous.children().any(|child| {
                        matches!(child.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL)
                    })
            });
        }
        previous = match element {
            SyntaxElement::Node(node) => node.prev_sibling_or_token(),
            SyntaxElement::Token(token) => token.prev_sibling_or_token(),
        };
    }
    false
}

fn is_stream_trivia(kind: SyntaxKind) -> bool {
    is_trivia(kind)
        || matches!(
            kind,
            SyntaxKind::GUARD | SyntaxKind::DOC_MARGIN | SyntaxKind::TILDE
        )
}

fn closed_group(group: &Group) -> bool {
    group
        .syntax()
        .first_child_or_token()
        .is_some_and(|el| el.kind() == SyntaxKind::L_BRACE)
        && group
            .syntax()
            .last_child_or_token()
            .is_some_and(|el| el.kind() == SyntaxKind::R_BRACE)
}

fn body_stream(group: &Group, definitions: Arc<[u8]>, active: bool) -> Stream {
    let mut elements: VecDeque<_> = group.syntax().children_with_tokens().collect();
    elements.pop_front();
    elements.pop_back();
    Stream {
        elements,
        definitions,
        active,
    }
}

/// Read only arguments attached to this head. The parser's greedy and partial
/// fallbacks do not prove a complete call, so sibling recovery is deliberately
/// absent here.
fn read_call(node: &SyntaxNode) -> Option<Call> {
    let command = Command::cast(node.clone())?;
    let name = command.name()?.to_string();
    let (stem, spec) = name.rsplit_once(':')?;
    if stem.is_empty() || stem.starts_with(':') {
        return None;
    }
    let slots = expl3_slots(&name)?;
    let mut elements: VecDeque<_> = node
        .children_with_tokens()
        .skip_while(|el| el.kind() != SyntaxKind::CONTROL_WORD)
        .skip(1)
        .filter(|el| !is_trivia(el.kind()) && el.kind() != SyntaxKind::TILDE)
        .collect();
    let mut arguments = Vec::new();
    for (specifier, slot) in spec.bytes().zip(slots) {
        let mut argument = Argument {
            specifier,
            elements: Vec::new(),
        };
        match slot {
            Expl3Slot::Group | Expl3Slot::Branch => {
                let el = elements.pop_front()?;
                let group = Group::cast(el.as_node()?.clone())?;
                if !closed_group(&group) {
                    return None;
                }
                argument.elements.push(el);
            }
            Expl3Slot::SingleToken => {
                let el = elements.pop_front()?;
                match &el {
                    SyntaxElement::Node(node) if node.kind() == SyntaxKind::GROUP => {
                        if !closed_group(&Group::cast(node.clone())?) {
                            return None;
                        }
                    }
                    SyntaxElement::Node(node) if node.kind() == SyntaxKind::COMMAND => {
                        if node
                            .children_with_tokens()
                            .filter(|el| !is_trivia(el.kind()))
                            .count()
                            != 1
                        {
                            return None;
                        }
                    }
                    SyntaxElement::Token(token)
                        if matches!(
                            token.kind(),
                            SyntaxKind::CONTROL_WORD | SyntaxKind::CONTROL_SYMBOL
                        ) => {}
                    SyntaxElement::Token(token)
                        if token.kind() == SyntaxKind::WORD
                            && token.text().chars().count() == 1 => {}
                    _ => return None,
                }
                argument.elements.push(el);
            }
            Expl3Slot::ParameterText => {
                while elements.front()?.kind() != SyntaxKind::GROUP {
                    argument.elements.push(elements.pop_front()?);
                }
            }
        }
        arguments.push(argument);
    }
    if !elements.is_empty() {
        return None;
    }
    Some(Call {
        name,
        arguments,
        message_parameters: Vec::new(),
    })
}

impl Argument {
    fn group(&self) -> Option<Group> {
        if self.elements.len() != 1 {
            return None;
        }
        Group::cast(self.elements[0].as_node()?.clone())
    }

    /// Literal `N` and `c` names have the same semantic value, but require
    /// different structural proofs. A command somewhere in a group is not
    /// enough to establish a single control-sequence argument.
    pub fn name(&self) -> Option<String> {
        match self.specifier {
            b'N' => {
                let mut elements = self.elements.clone();
                if let Some(group) = self.group() {
                    elements = group
                        .syntax()
                        .children_with_tokens()
                        .filter(|el| {
                            !is_trivia(el.kind())
                                && !matches!(el.kind(), SyntaxKind::L_BRACE | SyntaxKind::R_BRACE)
                        })
                        .collect();
                }
                if elements.len() != 1 {
                    return None;
                }
                let command = Command::cast(elements[0].as_node()?.clone())?;
                if command.syntax().children_with_tokens().count() != 1 {
                    return None;
                }
                Some(command.name()?.to_string())
            }
            b'c' => Some(self.literal()?.text),
            _ => None,
        }
    }

    pub fn literal(&self) -> Option<Literal> {
        let group = self.group()?;
        let mut text = String::new();
        let mut range = None;
        for el in group.syntax().children_with_tokens() {
            if is_trivia(el.kind())
                || matches!(el.kind(), SyntaxKind::L_BRACE | SyntaxKind::R_BRACE)
            {
                continue;
            }
            let token = el.as_token()?;
            if token.kind() != SyntaxKind::WORD {
                return None;
            }
            text.push_str(token.text());
            range = Some(range.map_or(token.text_range(), |r: TextRange| {
                r.cover(token.text_range())
            }));
        }
        Some(Literal {
            text,
            range: range.unwrap_or_else(|| {
                TextRange::empty(group.syntax().text_range().start() + TextSize::from(1))
            }),
        })
    }

    pub fn list(&self) -> Option<Vec<Literal>> {
        if self.specifier != b'n' {
            return None;
        }
        let atoms = self.atoms(&[])?;
        let mut entries = Vec::new();
        let mut text = String::new();
        let mut range: Option<TextRange> = None;
        for atom in atoms {
            let AtomKind::Character(c) = atom.kind else {
                return None;
            };
            if c == ',' {
                if let Some(range) = range.take() {
                    entries.push(Literal {
                        text: std::mem::take(&mut text),
                        range,
                    });
                }
            } else if c.is_ascii_alphabetic() {
                text.push(c);
                range = Some(range.map_or(atom.range, |r| r.cover(atom.range)));
            } else {
                return None;
            }
        }
        if let Some(range) = range {
            entries.push(Literal { text, range });
        }
        Some(entries)
    }

    fn atoms(&self, definitions: &[u8]) -> Option<Vec<Atom>> {
        let elements = if let Some(group) = self.group() {
            body_stream(&group, Arc::from([]), true)
                .elements
                .into_iter()
                .collect()
        } else {
            self.elements.clone()
        };
        let mut atoms = Vec::new();
        for element in elements {
            match element {
                SyntaxElement::Token(token) => append_atoms(&token, &mut atoms),
                SyntaxElement::Node(node) => {
                    for token in node
                        .descendants_with_tokens()
                        .filter_map(|el| el.into_token())
                    {
                        append_atoms(&token, &mut atoms);
                    }
                }
            }
        }
        for &count in definitions {
            atoms = unescape_parameters(atoms, count)?;
        }
        Some(atoms)
    }
}

/// The direct creator families are a small semantic allowlist. In particular,
/// `*_eq` and expansion variants do not establish executable replacement text.
pub(super) fn definition_kind(stem: &str) -> Option<bool> {
    let (family, tail) = stem.split_once('_')?;
    let tail = ["new", "set", "gset"]
        .into_iter()
        .find_map(|op| tail.strip_prefix(op))?;
    match family {
        "cs" if matches!(tail, "" | "_protected" | "_nopar" | "_protected_nopar") => Some(false),
        "prg" if matches!(tail, "_conditional" | "_protected_conditional") => Some(true),
        _ => None,
    }
}

fn definition_body(call: &Call, definitions: &[u8]) -> Option<(Group, u8)> {
    let (stem, spec) = call.name.rsplit_once(':')?;
    let conditional = definition_kind(stem)?;
    if !(if conditional {
        matches!(spec, "Nnn" | "Npnn" | "cnn" | "cpnn")
    } else {
        matches!(spec, "Nn" | "Npn" | "cn" | "cpn")
    }) {
        return None;
    }
    let name = call.arguments.first()?.name()?;
    let count = if let Some(argument) = call.arguments.iter().find(|a| a.specifier == b'p') {
        let atoms = argument.atoms(definitions)?;
        let mut count = 0;
        let mut pairs = atoms.chunks_exact(2);
        for pair in &mut pairs {
            count += 1;
            if count > 9
                || pair[0].kind != AtomKind::Hash
                || pair[1].kind != AtomKind::Character(char::from(b'0' + count))
            {
                return None;
            }
        }
        if !pairs.remainder().is_empty() {
            return None;
        }
        count
    } else {
        let slots = expl3_slots(&name)?;
        if slots.len() > 9 || slots.contains(&Expl3Slot::ParameterText) {
            return None;
        }
        slots.len() as u8
    };
    Some((call.arguments.last()?.group()?, count))
}

pub fn is_message_definition(name: &str) -> bool {
    name.rsplit_once(':').is_some_and(|(stem, spec)| {
        matches!(stem, "msg_new" | "msg_set" | "msg_gset") && matches!(spec, "nnn" | "nnnn")
    })
}

pub fn is_protected_conditional(name: &str) -> bool {
    name.rsplit_once(':').is_some_and(|(stem, spec)| {
        matches!(
            stem,
            "prg_new_protected_conditional"
                | "prg_set_protected_conditional"
                | "prg_gset_protected_conditional"
        ) && matches!(spec, "Nnn" | "Npnn" | "cnn" | "cpnn")
    })
}

pub fn is_variant_generation(name: &str) -> bool {
    matches!(
        name,
        "cs_generate_variant:Nn"
            | "cs_generate_variant:cn"
            | "prg_generate_conditional_variant:Nnn"
            | "prg_generate_conditional_variant:cnn"
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AtomKind {
    Hash,
    Character(char),
    Other,
    Unknown,
}

struct Atom {
    kind: AtomKind,
    range: TextRange,
}

fn append_atoms(token: &crate::syntax::SyntaxToken, atoms: &mut Vec<Atom>) {
    if is_trivia(token.kind()) {
        return;
    }
    if token.kind() == SyntaxKind::WORD {
        for (offset, c) in token.text().char_indices() {
            let start = token.text_range().start() + TextSize::from(offset as u32);
            atoms.push(Atom {
                kind: AtomKind::Character(c),
                range: TextRange::at(start, TextSize::of(c)),
            });
        }
    } else {
        atoms.push(Atom {
            kind: if token.kind() == SyntaxKind::HASH {
                AtomKind::Hash
            } else {
                AtomKind::Other
            },
            range: token.text_range(),
        });
    }
}

fn unescape_parameters(atoms: Vec<Atom>, count: u8) -> Option<Vec<Atom>> {
    let mut input = atoms.into_iter();
    let mut output = Vec::new();
    while let Some(atom) = input.next() {
        if atom.kind != AtomKind::Hash {
            output.push(atom);
            continue;
        }
        let next = input.next()?;
        let kind = match next.kind {
            AtomKind::Hash => AtomKind::Hash,
            AtomKind::Character(c) if c >= '1' && c <= char::from(b'0' + count) => {
                AtomKind::Unknown
            }
            AtomKind::Unknown => AtomKind::Unknown,
            _ => return None,
        };
        output.push(Atom {
            kind,
            range: atom.range.cover(next.range),
        });
    }
    Some(output)
}

fn message_parameters(atoms: &[Atom]) -> Vec<(TextRange, u8)> {
    let mut found = Vec::new();
    let mut atoms = atoms.iter();
    while let Some(atom) = atoms.next() {
        if atom.kind != AtomKind::Hash {
            continue;
        }
        if let Some(next) = atoms.next()
            && let AtomKind::Character(c @ '5'..='9') = next.kind
        {
            found.push((atom.range.cover(next.range), c as u8 - b'0'));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;

    fn root(body: &str) -> SyntaxNode {
        let source = format!("\\ExplSyntaxOn\n{body}\n\\ExplSyntaxOff\n");
        let parsed = parse(&source);
        let root = SyntaxNode::new_root(parsed.green);
        assert_eq!(root.text().to_string(), source);
        root
    }

    #[test]
    fn expl3_call_reader_preserves_expansion_letters() {
        let root = root(r"\demo:NVncvoxef \a \b {n}{c}{v}{o}{x}{e}{f}");
        let index = Expl3Index::build(&root);
        let call = index
            .calls
            .values()
            .find(|c| c.name == "demo:NVncvoxef")
            .unwrap();
        assert_eq!(
            call.arguments
                .iter()
                .map(|a| a.specifier)
                .collect::<Vec<_>>(),
            b"NVncvoxef"
        );
    }

    #[test]
    fn expl3_call_reader_declines_partial_and_greedy_shapes() {
        for source in [
            r"\demo:Nn \a",
            "{\\demo:Nn \\a\n\n{n}}",
            r"\demo:w {n}",
            r"\demo:D {n}",
            r"\demo:Tn {a}{b}",
            r"\demo:Nn \a {unclosed",
        ] {
            let root = root(source);
            let index = Expl3Index::build(&root);
            assert!(index.calls.is_empty(), "{source}");
        }
    }

    #[test]
    fn expl3_definees_and_token_lists_are_not_calls() {
        let root = root(
            r"\cs_new_eq:NN \msg_new:nnn \other:nnn
            \tl_set:Nn \l_data_tl {\msg_new:nnn{a}{b}{#5}}
            \msg_new:nnn{a}{b}{#6}",
        );
        let index = Expl3Index::build(&root);
        let calls: Vec<_> = index
            .calls
            .values()
            .filter(|c| c.name == "msg_new:nnn")
            .collect();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].message_parameters[0].1, 6);
    }
}
