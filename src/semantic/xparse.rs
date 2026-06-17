//! Parser for the **xparse argument specification** mini-language — the string in
//! the second group of `\NewDocumentCommand{\foo}{<spec>}{…}` (and the environment
//! variants). It is *parsed*, never executed (AGENTS.md decision #1): we read the
//! shape of each argument, not its processing.
//!
//! The full grammar is tokenized so the cursor never desyncs on a type's trailing
//! material (delimiter tokens, `{default}` groups, embellishment sets). But our
//! [`ArgSpec`] model only distinguishes a `{…}` [`ArgKind::Brace`] from a `[…]`
//! [`ArgKind::Bracket`] slot, because that is all the CST produces and all a
//! consumer (the formatter's arity glue) can act on. So an [`ArgSpec`] is emitted
//! **only** for argument types that yield an actual `{…}`/`[…]` node:
//!
//! - `m` → required brace; `o`, `O{default}` → optional bracket.
//! - `r⟨t1⟩⟨t2⟩` / `R⟨…⟩{default}` (required delimited) and `d⟨t1⟩⟨t2⟩` /
//!   `D⟨…⟩{default}` (optional delimited) → an [`ArgSpec`] **only** when the
//!   delimiters are literally `[`/`]` (bracket) or `{`/`}` (brace); other delimiters
//!   (`(`/`)`, `<`/`>`, …) produce no CST node, so no slot.
//! - `s` (star), `t⟨token⟩` (optional token), `v` (verbatim), and `e`/`E`
//!   (embellishments) produce no `{…}`/`[…]` node, so no slot — but their trailing
//!   material is still consumed.
//!
//! This keeps the emitted slot count equal to the `GROUP`/`OPTIONAL` nodes the
//! greedy parser actually attaches, which is what the formatter counts. Modifiers
//! (`+`, `!`) and argument processors (`>{…}`) are skipped. Unknown type letters
//! stop the scan (conservative: never panic, never invent slots).

use super::signature::{ArgKind, ArgSpec};

/// Parse an xparse argument-spec string into the `{…}`/`[…]` argument slots it
/// declares, in order. See the module docs for the type-by-type mapping.
pub fn parse_spec(spec: &str) -> Vec<ArgSpec> {
    let chars: Vec<char> = spec.chars().collect();
    let mut cursor = Cursor {
        chars: &chars,
        i: 0,
    };
    let mut args = Vec::new();

    loop {
        cursor.skip_modifiers();
        cursor.skip_ws();
        let Some(c) = cursor.bump() else { break };
        match c {
            'm' => args.push(brace(true)),
            'o' => args.push(bracket(false)),
            'O' => {
                cursor.skip_group();
                args.push(bracket(false));
            }
            // Required (`r`/`R`) and optional (`d`/`D`) delimited args: a slot only
            // when the delimiters are the bracket/brace pair the CST models.
            'r' | 'R' | 'd' | 'D' => {
                let required = matches!(c, 'r' | 'R');
                let open = cursor.read_token();
                let close = cursor.read_token();
                if matches!(c, 'R' | 'D') {
                    cursor.skip_group(); // the {default}
                }
                if let Some(kind) = delimiter_kind(open.as_deref(), close.as_deref()) {
                    args.push(ArgSpec {
                        required,
                        kind,
                        prose: false,
                        collapse: false,
                    });
                }
            }
            't' => {
                cursor.read_token(); // the test token; yields no node
            }
            'e' => {
                cursor.skip_group(); // {<tokens>}
            }
            'E' => {
                cursor.skip_group(); // {<tokens>}
                cursor.skip_group(); // {<defaults>}
            }
            // `s` (star), `v` (verbatim): consumed, no `{…}`/`[…]` node.
            's' | 'v' => {}
            // Unknown letter: stop rather than guess and miscount.
            _ => break,
        }
    }

    args
}

fn brace(required: bool) -> ArgSpec {
    ArgSpec {
        required,
        kind: ArgKind::Brace,
        prose: false,
        collapse: false,
    }
}

fn bracket(required: bool) -> ArgSpec {
    ArgSpec {
        required,
        kind: ArgKind::Bracket,
        prose: false,
        collapse: false,
    }
}

/// The `ArgKind` for a delimited arg whose delimiters are `open`/`close`, or `None`
/// when the pair is not one the CST produces a node for.
fn delimiter_kind(open: Option<&str>, close: Option<&str>) -> Option<ArgKind> {
    match (open, close) {
        (Some("["), Some("]")) => Some(ArgKind::Bracket),
        (Some("{"), Some("}")) => Some(ArgKind::Brace),
        _ => None,
    }
}

/// A char cursor over the spec string with the small consumption primitives the
/// xparse types need.
struct Cursor<'a> {
    chars: &'a [char],
    i: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.i).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.i += 1;
        Some(c)
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.i += 1;
        }
    }

    /// Skip the type-prefix modifiers that may precede any argument type: `+`
    /// (long), `!` (no-leading-space), and `>{processor}` argument processors.
    fn skip_modifiers(&mut self) {
        loop {
            self.skip_ws();
            match self.peek() {
                Some('+') | Some('!') => self.i += 1,
                Some('>') => {
                    self.i += 1;
                    self.skip_group();
                }
                _ => break,
            }
        }
    }

    /// Read a single spec token after optional whitespace: a control sequence
    /// (`\` + a letter run, or `\` + one symbol) or a single character. Used for the
    /// delimiter tokens of `r`/`R`/`d`/`D` and the test token of `t`.
    fn read_token(&mut self) -> Option<String> {
        self.skip_ws();
        let first = self.bump()?;
        if first != '\\' {
            return Some(first.to_string());
        }
        let mut token = String::from('\\');
        match self.peek() {
            Some(c) if c.is_ascii_alphabetic() => {
                while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
                    token.push(self.bump().expect("peeked"));
                }
            }
            Some(_) => token.push(self.bump().expect("peeked")),
            None => {}
        }
        Some(token)
    }

    /// If the next non-whitespace char opens a `{…}` group, skip the whole balanced
    /// group (nested braces included). A no-op otherwise — tolerant of a malformed
    /// spec missing the group a type would normally carry.
    fn skip_group(&mut self) {
        self.skip_ws();
        if self.peek() != Some('{') {
            return;
        }
        let mut depth = 0;
        while let Some(c) = self.bump() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(spec: &str) -> Vec<(bool, ArgKind)> {
        parse_spec(spec)
            .into_iter()
            .map(|a| (a.required, a.kind))
            .collect()
    }

    #[test]
    fn mandatory_and_optional_basics() {
        assert_eq!(
            kinds("m o m"),
            vec![
                (true, ArgKind::Brace),
                (false, ArgKind::Bracket),
                (true, ArgKind::Brace),
            ]
        );
    }

    #[test]
    fn optional_with_default_consumes_group() {
        // The `{0}` default must not be read as another argument.
        assert_eq!(
            kinds("O{0} m"),
            vec![(false, ArgKind::Bracket), (true, ArgKind::Brace)]
        );
    }

    #[test]
    fn star_and_token_yield_no_slot() {
        assert_eq!(kinds("s m"), vec![(true, ArgKind::Brace)]);
        // `t` consumes its test token (`*`), leaving just the `m`.
        assert_eq!(kinds("t* m"), vec![(true, ArgKind::Brace)]);
    }

    #[test]
    fn verbatim_yields_no_slot() {
        assert_eq!(kinds("v"), vec![]);
    }

    #[test]
    fn bracket_delimited_maps_to_bracket() {
        // `d[]` and `r[]` are `[…]`-delimited, so they yield a bracket slot.
        assert_eq!(kinds("d[]"), vec![(false, ArgKind::Bracket)]);
        assert_eq!(kinds("r[]"), vec![(true, ArgKind::Bracket)]);
    }

    #[test]
    fn paren_delimited_yields_no_slot() {
        // `()`-delimited args produce no CST node, so no slot — but the two
        // delimiter tokens are still consumed, so a trailing `m` is found.
        assert_eq!(kinds("d() m"), vec![(true, ArgKind::Brace)]);
        assert_eq!(kinds("r<> m"), vec![(true, ArgKind::Brace)]);
    }

    #[test]
    fn required_delimited_with_default_consumes_group() {
        // `R(){default}`: two delimiter tokens then a default group, then `m`.
        assert_eq!(kinds("R(){x} m"), vec![(true, ArgKind::Brace)]);
        // `D[]{default}`: bracket-delimited optional with a default → one bracket
        // slot, then `m`.
        assert_eq!(
            kinds("D[]{x} m"),
            vec![(false, ArgKind::Bracket), (true, ArgKind::Brace)]
        );
    }

    #[test]
    fn embellishments_consume_their_groups() {
        // `e{^_}` consumes one group; `E{^_}{\d\d}` consumes two. Neither yields a
        // slot, so only the `m` remains.
        assert_eq!(kinds("e{^_} m"), vec![(true, ArgKind::Brace)]);
        assert_eq!(kinds("E{^_}{00} m"), vec![(true, ArgKind::Brace)]);
    }

    #[test]
    fn modifiers_and_processors_skipped() {
        assert_eq!(kinds("+m"), vec![(true, ArgKind::Brace)]);
        assert_eq!(kinds("!o"), vec![(false, ArgKind::Bracket)]);
        assert_eq!(kinds(">{\\TrimSpaces} m"), vec![(true, ArgKind::Brace)]);
    }

    #[test]
    fn empty_and_whitespace_specs() {
        assert_eq!(kinds(""), vec![]);
        assert_eq!(kinds("   "), vec![]);
        assert_eq!(
            kinds("  m   o  "),
            vec![(true, ArgKind::Brace), (false, ArgKind::Bracket)]
        );
    }

    #[test]
    fn unknown_letter_stops_scan() {
        // A garbage letter halts parsing; the `m` before it is kept, the rest dropped.
        assert_eq!(kinds("m z m"), vec![(true, ArgKind::Brace)]);
    }

    #[test]
    fn control_sequence_delimiters_consumed() {
        // `d\langle\rangle`: control-word delimiter tokens, non-bracket → no slot,
        // but both are consumed so the `m` is reached.
        assert_eq!(kinds("d\\langle\\rangle m"), vec![(true, ArgKind::Brace)]);
    }
}
