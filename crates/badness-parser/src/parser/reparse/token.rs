//! The token tier: relex one leaf in isolation and splice it in place.
//!
//! # What it does
//!
//! An edit that lands strictly inside a single `WORD` / `WHITESPACE` / `COMMENT`
//! leaf changes that leaf's *text* and nothing else. Rowan's
//! [`SyntaxToken::replace_with`] rebuilds only the leaf-to-root spine and shares
//! every green node off it, so the splice is `O(depth)` rather than `O(file)`.
//! Diagnostics keep their prefix, shift their suffix, and refuse anything that
//! touches the leaf.
//!
//! # Why it is sound
//!
//! This is the argument a reviewer has to check, so it is written out rather than
//! implied.
//!
//! Everything the parser does is a function of two things: the **token vector**
//! and the [`ParseCtx`](crate::parser::lexer::ParseCtx). Fix those two and the
//! grammar is deterministic — the shape gates, the prescan indices, the trivia
//! binding, and the attachment walk all read tokens, never source offsets. So a
//! splice reproduces a full parse exactly when it can show that
//!
//! 1. the token **kind** sequence is unchanged, and only the one leaf's text moved;
//! 2. the `ParseCtx` is unchanged;
//! 3. no decision that reads a token's **text** can flip.
//!
//! Each is a guard below.
//!
//! **(1) The kind sequence.** [`lex_with`] over the new leaf text alone must yield
//! exactly one token of the leaf's own kind, and the two join probes must show it
//! still separates from its neighbours. The isolated relex is faithful because the
//! lexer's modes cannot be entered or left by a token of these three kinds: every
//! mode is armed by a control word, a brace, or a `\begin{…}` name — and a leaf
//! that relexes to a single `WORD`/`WHITESPACE`/`COMMENT` spells none of them.
//! Conversely the leaf's *presence* in the tree as one of these kinds is what
//! proves the lexer was in the ordinary regime there: inside `\verb` or a verbatim
//! body the same bytes would be a `VERB` or `VERBATIM_BODY`. The join probes are
//! not decoration — `\foo` followed by `WORD("1ab")` is two tokens only because the
//! word starts with a non-letter, and editing it to `aab` would merge the pair into
//! one control word.
//!
//! **(2) The context.** [`scan_definitions`](crate::semantic::define::scan_definitions)
//! walks only `COMMAND` nodes whose head names a definition family, so a leaf that
//! sits under none of them cannot change what the scan found.
//! [`context_admits`] bans those, plus the environment-name positions and the
//! commands whose *lexing* reads the raw text after them.
//!
//! **(3) The text reads.** [`text_reads_are_inert`] enumerates every place the
//! grammar branches on a `WORD`'s text, and `tests::the_text_read_survey_is_complete`
//! reads the grammar sources to pin that the enumeration is still the whole set. A
//! new one appears as a failing test, not as a silent divergence.
//!
//! Trivia kinds carry no such reads: the grammar's text comparisons all run on
//! tokens it has already established are non-trivia (`peek_meaningful_text` skips
//! trivia before reading, and its one caller asks whether the text starts with a
//! backslash, which no leaf of these kinds ever does).
//!
//! # What it refuses, and why that is free
//!
//! Every guard returns [`None`] and the caller full-parses, so the cost of being
//! wrong about a guard's *necessity* is speed. The deliberate refusals worth
//! knowing about: a `.dtx` parse (the docstrip lexer mode is line-oriented, and an
//! isolated fragment has no column), any edit carrying a line terminator, a leaf
//! whose neighbour is too large to probe cheaply, and anything in math whose word
//! splits into operator atoms.

use rowan::{GreenToken, NodeOrToken, TextRange, TextSize};

use crate::parser::core::SyntaxError;
use crate::parser::grammar::is_def_prefix_command;
use crate::parser::grammar::{BEGIN_CMD, END_CMD, reads_definition_body, split_math_word};
use crate::parser::lexer::{lex_with, reads_following_text};
use crate::semantic::define::is_definition_command;
use crate::syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

use super::{Edit, ReparseBase, ReparseTier, Reparsed, finish};

/// How much neighbour text a join probe will relex.
///
/// A probe is `O(neighbour)`, so an unbounded one would make the tier `O(file)` the
/// moment a leaf sits beside a 100 KB `VERBATIM_BODY` — the exact shape this tier
/// exists to be cheaper than. Over the cap the tier refuses; a real neighbour is a
/// newline, a space, or a word.
const MAX_PROBE_BYTES: usize = 1024;

/// Splice `edit` into the single leaf that contains it, or [`None`].
pub(super) fn reparse_token(
    base: &ReparseBase<'_>,
    edit: &Edit,
    new_text: &str,
) -> Option<Reparsed> {
    // Cheapest first, and a guard must bail on cheap evidence: a rejected attempt
    // is paid *on top of* the full parse it falls back to.

    // A line terminator restructures paragraphs, comment extents, and `.dtx` lines,
    // none of which a single-leaf splice can account for. Checked on both sides of
    // the edit: a `WORD` cannot contain one, but proving that here beats assuming it.
    if edit.insert.contains(['\n', '\r']) || base.text[edit.range.clone()].contains(['\n', '\r']) {
        return None;
    }

    // The docstrip mode lexes by line and by column 0, which an isolated relex of a
    // fragment cannot reproduce — the fragment is always at the start of its own
    // input. Refusing the whole flavor is the honest version of that.
    if base.config.dtx {
        return None;
    }

    let root = base.syntax();
    let range = TextRange::new(
        TextSize::try_from(edit.range.start).ok()?,
        TextSize::try_from(edit.range.end).ok()?,
    );

    candidates(&root, range)
        .into_iter()
        .find_map(|leaf| try_leaf(base, edit, new_text, &leaf, range))
}

/// The leaves an edit of `range` could be inside, in preference order.
///
/// An insertion at a token boundary belongs to *either* neighbour, and which one
/// works is not knowable up front: typing a letter after a space extends the word
/// to its right, while typing one after a word extends the word to its left. Both
/// are offered and the guards decide. A non-empty range has at most one covering
/// token, and a range that straddles two lands on their parent node instead.
fn candidates(root: &SyntaxNode, range: TextRange) -> Vec<SyntaxToken> {
    if range.is_empty() {
        root.token_at_offset(range.start()).collect()
    } else {
        match root.covering_element(range) {
            NodeOrToken::Token(t) => vec![t],
            NodeOrToken::Node(_) => Vec::new(),
        }
    }
}

fn try_leaf(
    base: &ReparseBase<'_>,
    edit: &Edit,
    new_text: &str,
    leaf: &SyntaxToken,
    range: TextRange,
) -> Option<Reparsed> {
    if !leaf.text_range().contains_range(range) {
        return None;
    }
    if !matches!(
        leaf.kind(),
        SyntaxKind::WORD | SyntaxKind::WHITESPACE | SyntaxKind::COMMENT
    ) {
        return None;
    }
    let ctx = context_admits(leaf)?;

    let leaf_start = usize::from(leaf.text_range().start());
    let old = leaf.text();
    let cut = edit.range.start.checked_sub(leaf_start)?..edit.range.end.checked_sub(leaf_start)?;
    let mut new_leaf = String::with_capacity(old.len() + edit.insert.len());
    new_leaf.push_str(old.get(..cut.start)?);
    new_leaf.push_str(&edit.insert);
    new_leaf.push_str(old.get(cut.end..)?);

    // An emptied leaf is a token *removed*, which is a change to the kind sequence
    // and so a different question than this tier answers.
    if new_leaf.is_empty() {
        return None;
    }

    if !text_reads_are_inert(leaf.kind(), old, &new_leaf, ctx) {
        return None;
    }

    // The isolated relex, under the base's own context and flavor — a `\newcommand`
    // the definition scan found must lex the fragment the way it lexed the tree.
    let relexed = lex_with(&new_leaf, base.ctx, base.config);
    if relexed.len() != 1 || relexed[0].kind != leaf.kind() {
        return None;
    }

    if !joins(base, leaf.prev_token().as_ref(), &new_leaf, Side::Before)
        || !joins(base, leaf.next_token().as_ref(), &new_leaf, Side::After)
    {
        return None;
    }

    let errors = shifted_errors(base.errors, leaf.text_range(), edit)?;
    let green = leaf.replace_with(GreenToken::new(leaf.kind().into(), &new_leaf));
    finish(green, errors, ReparseTier::Token, base, new_text)
}

/// Which side of the leaf a join probe is testing.
#[derive(Clone, Copy)]
enum Side {
    Before,
    After,
}

/// Whether the new leaf text still lexes apart from its neighbour.
///
/// The probe relexes just the pair and demands the same two tokens back. A missing
/// neighbour is the file edge, where there is nothing to merge with. A neighbour
/// that does not reproduce itself in isolation — a `VERB`, a `VERBATIM_BODY`, a
/// `WORD` that is really a sub-slice of one the math split cut up — fails the probe
/// and the tier refuses, which is the conservative answer in every one of those
/// cases.
fn joins(
    base: &ReparseBase<'_>,
    neighbour: Option<&SyntaxToken>,
    leaf_text: &str,
    side: Side,
) -> bool {
    let Some(neighbour) = neighbour else {
        return true;
    };
    let n = neighbour.text();
    if n.len() > MAX_PROBE_BYTES {
        return false;
    }
    let (first, second) = match side {
        Side::Before => (n, leaf_text),
        Side::After => (leaf_text, n),
    };
    let mut probe = String::with_capacity(first.len() + second.len());
    probe.push_str(first);
    probe.push_str(second);

    let toks = lex_with(&probe, base.ctx, base.config);
    if toks.len() != 2 || toks[0].text != first || toks[1].text != second {
        return false;
    }
    // The neighbour must also come back as *itself*. A token that lexes to a
    // different kind in isolation than it holds in the tree means the probe was run
    // in a regime the tree was not parsed in, so its verdict says nothing.
    match side {
        Side::Before => toks[0].kind == neighbour.kind(),
        Side::After => toks[1].kind == neighbour.kind(),
    }
}

/// What the leaf's surroundings say about which text reads can reach it.
#[derive(Clone, Copy)]
struct Context {
    /// The leaf sits in a math body, where `math_atom` splits a `WORD` into
    /// operator atoms. Exact rather than approximate: every math body — `$…$`,
    /// `\[…\]`, `\left…\right`, and a math environment's — opens a `MATH` node.
    in_math: bool,
}

/// Whether the leaf sits somewhere a text change may be spliced at all, and if so
/// what its surroundings imply.
///
/// Three complementary bans, because a construct that reads a token's text can
/// reach it through the tree, through the raw source, or through simple adjacency:
///
/// - **By ancestry**, which is how greedy attachment records an argument that *was*
///   claimed: a `NAME_GROUP`, a `BEGIN`/`END`, any `COMMAND` whose head reads the
///   text after it (`\begin`, `\documentclass`, every definition family), and any
///   `COMMAND` whose head is an expl3 name, whose argspec slots read a `WORD`'s
///   length. Attachment is greedy, so a slot the expl3 scan *declined* is attached
///   to the same command anyway and lands under the same ban.
/// - **By the preceding tokens on the line**, which covers the residue where
///   attachment declined outright — a shape gate demoting a `[`, a `\documentclass`
///   stranded at the end of a group. The lexer's forward scans read raw text and do
///   not care what the tree made of it.
/// - **By immediate adjacency**, for the reads that only ever fire on the token
///   right after another: a parameter digit after a `#`.
fn context_admits(leaf: &SyntaxToken) -> Option<Context> {
    let mut in_math = false;
    for node in leaf.parent_ancestors() {
        match node.kind() {
            // An environment name decides routing, verbatim-ness, and pairing; a
            // relex of the same kind proves nothing about any of it.
            SyntaxKind::NAME_GROUP | SyntaxKind::BEGIN | SyntaxKind::END => return None,
            SyntaxKind::MATH => in_math = true,
            SyntaxKind::COMMAND => {
                let head = command_head(&node)?;
                if head_reads_following_text(&head) || is_expl3_name(&head) {
                    return None;
                }
            }
            _ => {}
        }
    }

    let mut prev = leaf.prev_token();
    while let Some(token) = prev {
        if token.kind() == SyntaxKind::NEWLINE {
            break;
        }
        if token.kind() == SyntaxKind::CONTROL_WORD && head_reads_following_text(token.text()) {
            return None;
        }
        prev = token.prev_token();
    }

    // `#1`: the expl3 plan reads whether the token after a `#` is a parameter
    // digit. That read fires nowhere else, so adjacency is the whole guard.
    if leaf
        .prev_token()
        .is_some_and(|t| t.kind() == SyntaxKind::HASH)
    {
        return None;
    }

    Some(Context { in_math })
}

/// Whether a control word is an expl3 name — the heads whose argspec suffix directs
/// attachment, and so the only place a `WORD`'s *length* is read.
///
/// A colon-carrying `CONTROL_WORD` only lexes inside an expl3 region, so the colon
/// is both the name test and the region test.
fn is_expl3_name(text: &str) -> bool {
    text.contains(':')
}

/// The control word a `COMMAND` node leads with, if any.
fn command_head(command: &SyntaxNode) -> Option<String> {
    command
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| {
            matches!(
                t.kind(),
                SyntaxKind::CONTROL_WORD | SyntaxKind::CONTROL_SYMBOL
            )
        })
        .map(|t| t.text().to_owned())
}

/// Whether a control word's *following* text is read — by the grammar, by the
/// definition scan, or by the lexer.
///
/// Each disjunct names the reader rather than restating its set, so a family that
/// grows grows here too.
fn head_reads_following_text(text: &str) -> bool {
    if text == BEGIN_CMD || text == END_CMD {
        // The environment name drives math routing, verbatim capture, pairing, and
        // the statement-body flag.
        return true;
    }
    if reads_definition_body(text) || is_def_prefix_command(text) {
        return true;
    }
    // `semantic::define`'s own view of what a definition is — the scan that builds
    // the `ParseCtx` this splice reuses.
    if text.strip_prefix('\\').is_some_and(is_definition_command) {
        return true;
    }
    // The lexer's raw forward scans: the short-verb and document-class toggles, and
    // the one-shot lookahead.
    reads_following_text(text)
}

/// Whether every text-reading decision in the grammar answers the same for `old`
/// and `new`.
///
/// The enumeration is the whole point, and `tests::the_text_read_survey_is_complete`
/// pins that it is still exhaustive. Each arm names the site it stands for.
fn text_reads_are_inert(kind: SyntaxKind, old: &str, new: &str, ctx: Context) -> bool {
    // `COMMENT` and `WHITESPACE` are trivia. Every text comparison in the grammar
    // runs on a token it has already established is non-trivia — `peek_meaningful`
    // and friends skip trivia before reading — so a trivia leaf's text reaches no
    // decision at all.
    if kind != SyntaxKind::WORD {
        return true;
    }

    // `parse_block`'s statement run loop: a top-level `;`-carrying `WORD` ends a
    // picture-body statement. Only *whether* the token carries one is read, so a
    // word that gains or loses one is what matters, not where.
    if old.contains(';') != new.contains(';') {
        return false;
    }

    // `at_star_variant_marker`: a lone `*` folds into the invocation before it.
    if old == "*" || new == "*" {
        return false;
    }

    // The math operator split (`math_atom` → `split_math_word`), which cuts a word
    // into `SubTok` atoms. Equal splits are not enough: the pieces are offsets into
    // *this* text, so two words split the same way still carry different atoms.
    // Both sides must therefore split into nothing.
    //
    // Gated on being in math because that is the only place the call happens, and
    // an ungated version would refuse every hyphenated word in prose — `-` is a
    // sign, so `well-known` splits — which is most of the workload this tier is
    // for.
    if ctx.in_math && (split_math_word(old).is_some() || split_math_word(new).is_some()) {
        return false;
    }

    // `peek_meaningful_text`'s one caller asks whether the next non-trivia token
    // starts with a backslash. A `WORD` in a tree never does — a leading backslash
    // lexes as a control sequence — and a *proposed* one that does is refused by
    // the isolated relex a few lines later anyway. Checked here rather than left to
    // that: a guard whose necessity rests on the order of two other guards is one
    // refactor away from being no guard at all.
    if old.starts_with('\\') || new.starts_with('\\') {
        return false;
    }

    true
}

/// Diagnostics for the spliced tree: keep the prefix, shift the suffix, refuse any
/// error that touches the leaf.
///
/// Refusing an overlap rather than regenerating it is the refusal-first contract:
/// an error *about* the leaf may change its message, its extent, or disappear, and
/// none of that is derivable from a relex of one token. Since a full parse emits
/// errors in offset order (pinned by the harness), shifting a suffix keeps them
/// sorted.
fn shifted_errors(
    errors: &[SyntaxError],
    leaf: TextRange,
    edit: &Edit,
) -> Option<Vec<SyntaxError>> {
    let start = usize::from(leaf.start());
    let end = usize::from(leaf.end());
    let delta = edit.delta();

    let mut out = Vec::with_capacity(errors.len());
    for error in errors {
        if error.end <= start {
            out.push(error.clone());
        } else if error.start >= end {
            out.push(SyntaxError {
                message: error.message.clone(),
                start: error.start.checked_add_signed(delta)?,
                end: error.end.checked_add_signed(delta)?,
            });
        } else {
            return None;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::declarations::ResolvedDeclarations;
    use crate::parser::core::parse_with_declarations_resolved;
    use crate::parser::lexer::{LatexFlavor, LexConfig};
    use crate::parser::reparse::{ReparseBase, reparse};

    /// Why a text-reading site in the grammar cannot see a leaf this tier splices.
    ///
    /// The verdicts are the point of the survey below: a bare list of matched lines
    /// would prove the scan still runs, but not that anyone read what it found.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Verdict {
        /// Kind-gated to `CONTROL_WORD`/`CONTROL_SYMBOL`, or reached only through a
        /// `strip_prefix('\\')` that a `WORD`/`WHITESPACE`/`COMMENT` never survives.
        /// This tier splices none of those kinds.
        ControlSequence,
        /// The definition of a text accessor, not a decision that branches on one.
        Accessor,
        /// Reads a length to advance an offset; the *content* reaches no decision.
        Offsets,
        /// A real read of a spliceable leaf's text, neutralized by a named guard in
        /// [`text_reads_are_inert`].
        Guarded(&'static str),
        /// A real read, neutralized by a named ban in [`context_admits`] — the leaf
        /// can never be in a position where the read happens.
        Context(&'static str),
    }
    use Verdict::*;

    /// Every place the grammar branches on a token's text, and why each is safe.
    ///
    /// `the_text_read_survey_is_complete` scans the grammar sources and asserts the
    /// set of matched lines is exactly the set of keys here. A new text read is
    /// therefore a *failing test* naming the line nobody classified, rather than a
    /// silent hole in the tier's soundness argument.
    const TEXT_READS: &[(&str, Verdict)] = &[
        (
            "&& P::MATH_ANCHOR.anchors(t.text.as_str()) =>",
            ControlSequence,
        ),
        (
            "&& let Some(pieces) = split_math_word(self.text())",
            Guarded("the math operator split, gated on `Context::in_math`"),
        ),
        ("&& self.tokens[i].text == END_CMD", ControlSequence),
        (".map(|t| t.text.as_str())", Accessor),
        (".text", ControlSequence),
        (r".then(|| t.text.strip_prefix('\\'))", ControlSequence),
        ("0 => definition_name_slots(&t.text),", ControlSequence),
        (
            r#"Some(SyntaxKind::CONTROL_SYMBOL) => matches!(self.text(), "\\]" | "\\)"),"#,
            ControlSequence,
        ),
        (
            r#"Some(SyntaxKind::CONTROL_SYMBOL) if matches!(self.text(), "\\]" | "\\)") => {"#,
            ControlSequence,
        ),
        (
            r#"Some(SyntaxKind::CONTROL_SYMBOL) if self.text() == "\\\\" => self.line_break(),"#,
            ControlSequence,
        ),
        (
            "Some(SyntaxKind::CONTROL_SYMBOL) if self.text() == closer => {",
            ControlSequence,
        ),
        (
            "SyntaxKind::CONTROL_SYMBOL => match t.text.as_str() {",
            ControlSequence,
        ),
        (
            "SyntaxKind::WORD if is_param_digit_text(&t.text) => {",
            Context("a `#` immediately before the leaf"),
        ),
        (
            "SyntaxKind::WORD if t.text.chars().count() == 1 => {",
            Context("an expl3 (colon-carrying) head on an ancestor `COMMAND`"),
        ),
        (
            "[t] => Cow::Borrowed(t.text.trim()),",
            Context("a `NAME_GROUP`/`BEGIN`/`END` ancestor, or a `\\begin` head"),
        ),
        (
            "if let Some(toggle) = expl_toggle(&t.text) {",
            ControlSequence,
        ),
        (
            "if self.kind() == Some(SyntaxKind::CONTROL_SYMBOL) && self.text() == closer {",
            ControlSequence,
        ),
        (
            r#"if self.kind() == Some(SyntaxKind::WORD) && self.text() == "*" {"#,
            Guarded("the lone-`*` ban"),
        ),
        (
            "if self.tokens.get(idx).is_some_and(|t| t.text == BEGIN_CMD) {",
            ControlSequence,
        ),
        (
            "if self.tokens.get(self.pos).map(|t| (t.kind, t.text.as_str()))",
            Guarded("the lone-`*` ban (`at_star_variant_marker`)"),
        ),
        ("if t.text.as_str() == RIGHT_CMD {", ControlSequence),
        (
            "let bracket = if is_big_delimiter_command(self.text()) {",
            ControlSequence,
        ),
        (
            "let def_prefix = is_def_prefix_command(self.text());",
            ControlSequence,
        ),
        (
            "let kind = match self.tokens[s.next].text.as_str() {",
            ControlSequence,
        ),
        (r"let name = t.text.strip_prefix('\\')?;", ControlSequence),
        ("let sym = self.text().to_owned();", ControlSequence),
        (
            "let text = self.p.tokens[idx].text.as_str();",
            ControlSequence,
        ),
        ("match self.p.tokens[idx].text.as_str() {", ControlSequence),
        (
            "name.push_str(&t.text);",
            Context("a `NAME_GROUP`/`BEGIN`/`END` ancestor, or a `\\begin` head"),
        ),
        (
            "name.push_str(self.text());",
            Context("a `NAME_GROUP`/`BEGIN`/`END` ancestor, or a `\\begin` head"),
        ),
        ("off += t.text.len();", Offsets),
        (
            "return Some(t.text.as_str());",
            Guarded("the leading-backslash ban (`peek_meaningful_text`)"),
        ),
        (
            "self.in_def_body = saved || is_definition_body_command(self.text());",
            ControlSequence,
        ),
        (
            "self.kind() == Some(SyntaxKind::CONTROL_WORD) && self.text() == name",
            ControlSequence,
        ),
        (
            r#"self.kind() == Some(SyntaxKind::WORD) && self.text().contains(';');"#,
            Guarded("the `;` presence ban"),
        ),
        (
            "self.tokens[idx].text == BEGIN_CMD && self.env_name_follows(idx)",
            ControlSequence,
        ),
        (
            "self.tokens[idx].text == END_CMD && self.env_name_follows(idx)",
            ControlSequence,
        ),
        (
            "t.kind == SyntaxKind::CONTROL_SYMBOL && t.text.as_str() == self.closer",
            ControlSequence,
        ),
        (
            "t.kind == SyntaxKind::CONTROL_WORD && t.text.as_str() == LEFT_CMD",
            ControlSequence,
        ),
        (
            "t.kind == SyntaxKind::CONTROL_WORD && t.text.as_str() == RIGHT_CMD",
            ControlSequence,
        ),
        (
            r"t.text.strip_prefix('\\').and_then(conditional::flow_word)",
            ControlSequence,
        ),
        (
            r"} else if t.text.strip_prefix('\\').and_then(conditional::flow_word)",
            ControlSequence,
        ),
    ];

    /// The grammar sources the survey reads. Compiled in, so the scan cannot go
    /// looking at a stale checkout or silently find no files.
    const GRAMMAR_SOURCES: &[(&str, &str)] = &[
        ("grammar.rs", include_str!("../grammar.rs")),
        ("grammar/prescan.rs", include_str!("../grammar/prescan.rs")),
        ("grammar/expl3.rs", include_str!("../grammar/expl3.rs")),
        ("grammar/trivia.rs", include_str!("../grammar/trivia.rs")),
        ("grammar/facts.rs", include_str!("../grammar/facts.rs")),
        ("conditional.rs", include_str!("../conditional.rs")),
    ];

    /// Every line of `src` that reads a token's text.
    ///
    /// Deliberately crude: `.text` not followed by an identifier character, which
    /// catches `.text()`, `.text.as_str()`, and `.text.len()` while skipping
    /// `.text_range()` (an offset API) and identifiers that merely contain the word.
    /// Over-matching costs a line in the table; under-matching costs the guarantee,
    /// so the bias is deliberate.
    fn text_reads(src: &str) -> Vec<&str> {
        // The `#[cfg(test)]` tail is test code, which decides nothing at parse time.
        let body = src.split("\n#[cfg(test)]").next().unwrap_or(src);
        body.lines()
            .map(str::trim)
            .filter(|line| !line.starts_with("//"))
            .filter(|line| {
                line.match_indices(".text").any(|(i, _)| {
                    !line[i + 5..]
                        .chars()
                        .next()
                        .is_some_and(|c| c == '_' || c.is_alphanumeric())
                })
            })
            .collect()
    }

    /// The tier's soundness rests on [`TEXT_READS`] being the *whole* set of places
    /// the grammar branches on a token's text. Nothing but this test keeps that
    /// true: the guards have no compile-time link to the sites they neutralize, so
    /// a new comparison would otherwise land silently and the oracle would only
    /// catch it if a fuzz seed happened to spell it.
    #[test]
    fn the_text_read_survey_is_complete() {
        let mut found: Vec<&str> = GRAMMAR_SOURCES
            .iter()
            .flat_map(|(_, src)| text_reads(src))
            .collect();
        found.sort_unstable();
        found.dedup();

        let mut classified: Vec<&str> = TEXT_READS.iter().map(|(line, _)| *line).collect();
        classified.sort_unstable();
        classified.dedup();
        assert_eq!(
            classified.len(),
            TEXT_READS.len(),
            "the survey table has duplicate entries",
        );

        let unclassified: Vec<&&str> = found.iter().filter(|l| !classified.contains(l)).collect();
        assert!(
            unclassified.is_empty(),
            "the grammar grew text reads nobody classified. Each one is a decision \
             that could differ between a splice and a full parse, so it must be \
             added to `TEXT_READS` with a verdict — and, if it can see a `WORD`, a \
             guard in `text_reads_are_inert` or `context_admits`:\n{unclassified:#?}",
        );

        let stale: Vec<&&str> = classified.iter().filter(|l| !found.contains(l)).collect();
        assert!(
            stale.is_empty(),
            "the survey table names lines the grammar no longer has. Drop them — a \
             table that outlives its sites stops describing anything:\n{stale:#?}",
        );
    }

    /// The scanner must be able to find something, or the test above passes by
    /// looking at nothing — panache lost two thirds of its fuzz coverage exactly
    /// that way, with every assertion still green.
    #[test]
    fn the_text_read_scanner_finds_the_sites_it_claims_to() {
        for (name, src) in GRAMMAR_SOURCES {
            assert!(
                !src.is_empty(),
                "{name} is empty: include_str! resolved wrong"
            );
        }
        let found = GRAMMAR_SOURCES
            .iter()
            .flat_map(|(_, src)| text_reads(src))
            .count();
        assert!(
            found >= 40,
            "the scanner found only {found} text reads; it has stopped matching",
        );
        // And it must not match things that are not reads.
        assert!(text_reads("let x = node.text_range().start();").is_empty());
        assert!(text_reads("self.text_bracket_batch.clear();").is_empty());
        assert!(text_reads("// t.text == BEGIN_CMD").is_empty());
        assert_eq!(text_reads("if t.text == BEGIN_CMD {").len(), 1);
    }

    fn with_base<R>(text: &str, f: impl FnOnce(&ReparseBase<'_>) -> R) -> R {
        let declared = ResolvedDeclarations::default();
        let (parse, ctx) = parse_with_declarations_resolved(text, LatexFlavor::Document, &declared);
        f(&ReparseBase {
            text,
            green: &parse.green,
            errors: &parse.errors,
            ctx: &ctx,
            config: LatexFlavor::Document.into(),
            declared: &declared,
        })
    }

    fn edit(range: std::ops::Range<usize>, insert: &str) -> Edit {
        Edit {
            range,
            insert: insert.to_string(),
        }
    }

    /// Splices, with the tier they must reach. The oracle inside `finish` is what
    /// checks the *result*; these pin that the guards let the case through at all.
    #[track_caller]
    fn assert_splices(text: &str, e: Edit) {
        with_base(text, |base| {
            let out = reparse(base, &e, &e.apply(text));
            let out = out.unwrap_or_else(|| panic!("expected a token-tier splice for {e:?}"));
            assert_eq!(out.tier, ReparseTier::Token);
        });
    }

    #[track_caller]
    fn assert_refuses(text: &str, e: Edit) {
        with_base(text, |base| {
            assert!(
                reparse(base, &e, &e.apply(text)).is_none(),
                "expected a refusal for {e:?}",
            );
        });
    }

    #[test]
    fn splices_a_letter_typed_into_a_prose_word() {
        assert_splices("Some ordinary prose.\n", edit(5..5, "x"));
        assert_splices("Some ordinary prose.\n", edit(5..8, "sensible"));
    }

    #[test]
    fn splices_inside_a_comment_and_inside_whitespace() {
        assert_splices("text % a trailing note\nmore\n", edit(10..10, "z"));
        assert_splices("a   b\n", edit(2..2, " "));
    }

    /// A hyphen makes `split_math_word` fire, but only math ever calls it — and
    /// hyphenated words are most of English prose.
    #[test]
    fn splices_a_hyphenated_word_outside_math() {
        assert_splices("a well-known result\n", edit(6..6, "l"));
    }

    #[test]
    fn refuses_an_edit_that_carries_a_newline() {
        assert_refuses("Some ordinary prose.\n", edit(5..5, "\n"));
        assert_refuses("Some ordinary prose.\n", edit(5..5, "\r\n"));
    }

    /// The environment name decides routing, verbatim capture, and pairing. A relex
    /// to the same kind proves nothing about any of it, so the position is banned
    /// outright — including for the `\begin` a shape gate demoted to a plain
    /// command, where the name sits in a `GROUP` rather than a `NAME_GROUP`.
    #[test]
    fn refuses_an_environment_name() {
        assert_refuses(
            "\\begin{itemize}\n\\item x\n\\end{itemize}\n",
            edit(8..8, "z"),
        );
        assert_refuses("{\\begin{itemize}\\item x}\n", edit(9..9, "z"));
    }

    #[test]
    fn refuses_a_definition_body_and_a_document_class() {
        // The definition scan builds the `ParseCtx` the splice reuses.
        assert_refuses("\\newcommand{\\bea}{\\begin{align}}\n", edit(26..26, "z"));
        // The lexer reads this name to decide whether `|` is a short verb.
        assert_refuses("\\documentclass{ltxdoc}\n", edit(16..16, "z"));
    }

    /// In math a word is cut into operator atoms, so its text is structure.
    #[test]
    fn refuses_a_word_that_splits_into_math_atoms() {
        assert_refuses("$a b$\n", edit(3..3, "+"));
        assert_refuses("\\begin{align}\n  a b\n\\end{align}\n", edit(17..17, "+"));
    }

    /// A `;` ends a picture-body statement, so gaining or losing one restructures
    /// the tree even though the token's kind is unchanged.
    #[test]
    fn refuses_a_word_that_gains_a_statement_terminator() {
        let text = "\\begin{tikzpicture}\n  \\draw (0,0) -- (1,1);\n\\end{tikzpicture}\n";
        // The end of `(0,0)`, which carries no `;` yet.
        let at = text.find("(0,0)").expect("fixture") + 5;
        assert_refuses(text, edit(at..at, ";"));
    }

    /// The backward join probe. `\foo` and `1ab` are two tokens only because the
    /// word starts with a non-letter; editing it to `aab` would merge the pair into
    /// a single control word, which is a change to the token *kind* sequence.
    #[test]
    fn refuses_an_edit_that_would_merge_with_the_previous_token() {
        assert_refuses("\\foo1ab\n", edit(4..5, "a"));
    }

    /// The forward probe's mirror: a `.dtx` parse is refused wholesale, since the
    /// docstrip mode reads column 0 and an isolated fragment has no column.
    #[test]
    fn refuses_every_edit_in_a_dtx_parse() {
        let text = "% \\begin{macro}{\\foo}\n%    \\begin{macrocode}\n\\def\\foo{bar}\n";
        let declared = ResolvedDeclarations::default();
        let config = LexConfig {
            flavor: LatexFlavor::Document,
            dtx: true,
        };
        let (parse, ctx) = parse_with_declarations_resolved(text, config, &declared);
        let base = ReparseBase {
            text,
            green: &parse.green,
            errors: &parse.errors,
            ctx: &ctx,
            config,
            declared: &declared,
        };
        let e = edit(5..5, "z");
        assert!(reparse(&base, &e, &e.apply(text)).is_none());
    }

    /// Deleting a leaf outright removes a token, which this tier does not model.
    #[test]
    fn refuses_an_edit_that_empties_the_leaf() {
        assert_refuses("a bb c\n", edit(2..4, ""));
    }

    /// A diagnostic that *touches* the leaf may change its message or extent, and
    /// neither is derivable from a relex of one token. One that sits after it just
    /// shifts.
    #[test]
    fn shifts_errors_after_the_leaf_and_refuses_ones_that_touch_it() {
        let text = "word\n\n\\begin{itemize}\n";
        with_base(text, |base| {
            assert!(
                !base.errors.is_empty(),
                "this fixture exists to carry an error"
            );
            let e = edit(2..2, "z");
            let out = reparse(base, &e, &e.apply(text)).expect("a splice before the error");
            assert_eq!(out.errors.len(), base.errors.len());
            assert_eq!(out.errors[0].start, base.errors[0].start + 1);
        });
    }

    /// The neighbour cap keeps the join probe from making the tier `O(file)`.
    ///
    /// Paired with the same edit beside a short neighbour, because a refusal on its
    /// own proves nothing about *which* guard refused — the first draft of this
    /// test was tripping the `\\end` scan instead and looked just as green.
    #[test]
    fn refuses_a_leaf_beside_an_oversized_neighbour() {
        let long = "a".repeat(MAX_PROBE_BYTES + 10);
        // The leaf is the space; its previous token is the word beside it.
        assert_refuses(&format!("{long} b\n"), edit(long.len()..long.len(), " "));

        let short = "a".repeat(8);
        assert_splices(&format!("{short} b\n"), edit(short.len()..short.len(), " "));
    }
}
