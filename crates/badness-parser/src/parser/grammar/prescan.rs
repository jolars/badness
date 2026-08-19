//! The single pass over the token stream that runs before the walk starts.
//!
//! Four scans share one loop because three of them carry *running state* —
//! the definition-name countdown, the expl3 catcode mode, and the shared
//! conditional-opener state machine — that only a single ordered pass can
//! maintain. Fusing them is therefore not an optimization to be undone; the
//! interleaving is the point, and this module exists so it can be tested
//! directly rather than through a full parse.

use std::collections::{HashMap, HashSet};

use smol_str::SmolStr;

use super::RIGHT_CMD;
use crate::parser::conditional;
use crate::parser::lexer::{ExplToggle, ParseCtx, Token, definition_name_slots, expl_toggle};
use crate::syntax::SyntaxKind;

/// Everything [`super::Parser::new`] needs to know about the token stream up
/// front. Each field is documented on the `Parser` field it becomes.
pub(super) struct PreScan {
    pub(super) starts: Vec<usize>,
    pub(super) expl_toggles: Vec<(usize, bool)>,
    pub(super) doc_margin_lines: Vec<(usize, usize)>,
    pub(super) conditional_openers: HashSet<usize>,
    pub(super) def_parameter_dollars: HashSet<usize>,
    pub(super) alias_openers: HashMap<usize, SmolStr>,
    pub(super) alias_closers: HashMap<usize, SmolStr>,
    pub(super) literal_alias_closers: HashMap<usize, SmolStr>,
    pub(super) last_r_bracket: Option<usize>,
    pub(super) last_display_math_closer: Option<usize>,
    pub(super) last_inline_math_closer: Option<usize>,
    pub(super) last_right: Option<usize>,
    pub(super) last_r_brace: Option<usize>,
    pub(super) last_fi: Option<usize>,
    pub(super) last_dollar: Option<usize>,
}

impl PreScan {
    pub(super) fn run(tokens: &[Token], ctx: &ParseCtx) -> Self {
        let mut starts = Vec::with_capacity(tokens.len() + 1);
        let mut off = 0;
        let mut expl_toggles = Vec::new();
        let mut conditional_openers = HashSet::new();
        let mut def_parameter_dollars = HashSet::new();
        let mut alias_openers = HashMap::new();
        let mut alias_closers = HashMap::new();
        let mut literal_alias_closers = HashMap::new();
        let want_aliases = ctx.has_env_aliases();
        // The environments some alias opens: a literal `\end{X}` closes one of
        // those just as a closer alias does (issue #117), so it is indexed here
        // beside the alias spellings. Empty — and never consulted — for a file
        // whose only aliases are closers.
        let begin_alias_targets: HashSet<&str> = if want_aliases {
            ctx.begin_alias_targets().collect()
        } else {
            HashSet::new()
        };
        let mut opener_scan = conditional::OpenerScan::new();
        // `expl_on` mirrors `in_expl_region` exactly: the state is the one in
        // force *before* this token, so a toggle sits outside its own region.
        let mut expl_on = false;
        // How many upcoming control words are *names being bound* by a definition
        // keyword rather than calls — a countdown, since `\let\a\b` binds two. See
        // [`super::Parser::alias_openers`] for why this filter is mandatory.
        let mut def_name_slots = 0u8;
        // `\def` parameter text runs from the definee through the token before
        // the replacement body's first `{`. A `$` there is a literal delimiter,
        // never a math opener; record it before the recursive walk can mistake a
        // later delimiter for its mate (issue #129).
        let mut def_parameter_state = 0u8;
        let mut last_r_bracket = None;
        let mut last_display_math_closer = None;
        let mut last_inline_math_closer = None;
        let mut last_right = None;
        let mut last_r_brace = None;
        let mut last_fi = None;
        let mut last_dollar = None;
        // `.dtx` doc-margin lines, as `(first DOC_MARGIN on the line, the line's
        // terminating NEWLINE)`. Only a line that carries a margin is recorded,
        // so this stays empty — and allocation-free — for every non-`.dtx` file.
        // See [`super::Parser::on_doc_margin_line`].
        let mut doc_margin_lines: Vec<(usize, usize)> = Vec::new();
        let mut line_margin: Option<usize> = None;
        for (i, t) in tokens.iter().enumerate() {
            starts.push(off);
            off += t.text.len();
            // Last-closer indices for the gate bounds
            // ([`super::Parser::last_r_bracket`]), recorded before the
            // control-word early-out below so the non-control-word shapes are
            // seen. `\fi` recognition deliberately skips the expl3-region filter
            // [`super::Parser::conditional_flow_at`] applies: an upper bound only
            // needs to be at or past the last viable index, and keeping the
            // recording filter-free means it can never drift *below* the gate's
            // recognition.
            match t.kind {
                SyntaxKind::DOC_MARGIN => line_margin = line_margin.or(Some(i)),
                SyntaxKind::NEWLINE => {
                    if let Some(margin) = line_margin.take() {
                        doc_margin_lines.push((margin, i));
                    }
                }
                SyntaxKind::R_BRACKET => last_r_bracket = Some(i),
                SyntaxKind::R_BRACE => last_r_brace = Some(i),
                SyntaxKind::DOLLAR => last_dollar = Some(i),
                SyntaxKind::CONTROL_SYMBOL => match t.text.as_str() {
                    "\\]" => last_display_math_closer = Some(i),
                    "\\)" => last_inline_math_closer = Some(i),
                    _ => {}
                },
                SyntaxKind::CONTROL_WORD => {
                    if t.text.as_str() == RIGHT_CMD {
                        last_right = Some(i);
                    } else if t.text.strip_prefix('\\').and_then(conditional::flow_word)
                        == Some(conditional::FlowWord::Fi)
                    {
                        last_fi = Some(i);
                    }
                }
                _ => {}
            }
            match def_parameter_state {
                1 if super::Parser::is_trivia(t.kind) => {}
                1 if matches!(
                    t.kind,
                    SyntaxKind::CONTROL_WORD | SyntaxKind::CONTROL_SYMBOL
                ) =>
                {
                    def_parameter_state = 2;
                }
                1 => def_parameter_state = 0,
                2 if t.kind == SyntaxKind::L_BRACE => def_parameter_state = 0,
                2 if t.kind == SyntaxKind::DOLLAR => {
                    def_parameter_dollars.insert(i);
                }
                _ => {}
            }
            if def_parameter_state == 0
                && t.kind == SyntaxKind::CONTROL_WORD
                && super::is_def_prefix_command(&t.text)
            {
                def_parameter_state = 1;
            }
            if t.kind != SyntaxKind::CONTROL_WORD {
                // Trivia carries the definition-keyword state across (`\def  \bea`);
                // anything else clears it.
                if !super::Parser::is_trivia(t.kind) {
                    def_name_slots = 0;
                }
                continue;
            }
            if want_aliases {
                let name = (def_name_slots == 0 && !expl_on)
                    .then(|| t.text.strip_prefix('\\'))
                    .flatten();
                if let Some(name) = name {
                    if let Some(target) = ctx.begin_alias(name) {
                        alias_openers.insert(i, SmolStr::new(target));
                    } else if let Some(target) = ctx.end_alias(name) {
                        alias_closers.insert(i, SmolStr::new(target));
                    } else if name == "end"
                        && let Some(env) = super::peek_end_name(tokens, i)
                        && begin_alias_targets.contains(env.as_ref())
                    {
                        // `\def\bsplit{\begin{split}}` expands to `\begin{split}`,
                        // so a plain `\end{split}` closes it (issue #117). Indexed
                        // here rather than tested in the gate so the closer bound
                        // ([`super::AliasGate::last_closer`]) can be derived from
                        // it. `peek_end_name` is looser than the walk's
                        // [`super::Parser::env_end_at`] — it skips a blank line and
                        // takes a computed name — so the gate re-tests that; an
                        // index recorded here may over-approximate, never under-.
                        literal_alias_closers.insert(i, SmolStr::new(env.as_ref()));
                    }
                }
                // Consuming a slot short-circuits, so a keyword sitting *in* one
                // (`\let\a\def`) is the operand it looks like and does not arm a
                // fresh countdown — `conditional::OpenerScan::visit` resolves the
                // same collision the same way.
                def_name_slots = match def_name_slots {
                    0 => definition_name_slots(&t.text),
                    n => n - 1,
                };
            }
            // `visit` is a *state machine* over the whole stream (the operand-slot
            // countdown, the `\ifcsname` body), so it must run for every control
            // word, whatever we then do with the verdict. Kept on its own statement
            // rather than folded into the condition below: buried mid-`&&` behind a
            // cheaper test, a later reordering would skip the call and silently
            // desync the scan from the linter's.
            let word = t
                .text
                .strip_prefix('\\')
                .map(|name| opener_scan.visit(name));
            if word == Some(conditional::Word::Opens) && !expl_on {
                conditional_openers.insert(i);
            }
            if let Some(toggle) = expl_toggle(&t.text) {
                expl_toggles.push((i, toggle == ExplToggle::On));
                expl_on = toggle == ExplToggle::On;
            }
        }
        starts.push(off);
        // A final line the file never terminated still runs to the end of the
        // stream, so the backward scan this replaces would have reached its
        // margin from any token on it.
        if let Some(margin) = line_margin.take() {
            doc_margin_lines.push((margin, tokens.len()));
        }
        Self {
            starts,
            expl_toggles,
            doc_margin_lines,
            conditional_openers,
            def_parameter_dollars,
            alias_openers,
            alias_closers,
            literal_alias_closers,
            last_r_bracket,
            last_display_math_closer,
            last_inline_math_closer,
            last_right,
            last_r_brace,
            last_fi,
            last_dollar,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::lexer::{LexConfig, lex_with};

    /// Pre-scan `src` with no alias context — the shape every file gets before
    /// the second pass discovers any.
    fn scan(src: &str) -> (Vec<Token>, PreScan) {
        let ctx = ParseCtx::default();
        let tokens = lex_with(src, &ctx, LexConfig::default());
        let pre = PreScan::run(&tokens, &ctx);
        (tokens, pre)
    }

    /// Pre-scan `src` with `bea`/`eea` already registered as an `eqnarray`
    /// alias pair — the state the second pass hands in. Registering the pair
    /// directly rather than parsing for it keeps these tests about the *filter*
    /// (whose job this module owns) and not about the definition scan.
    fn scan_aliased(src: &str) -> (Vec<Token>, PreScan) {
        let mut ctx = ParseCtx::default();
        ctx.insert_begin_alias(SmolStr::new("bea"), SmolStr::new("eqnarray"));
        ctx.insert_end_alias(SmolStr::new("eea"), SmolStr::new("eqnarray"));
        let tokens = lex_with(src, &ctx, LexConfig::default());
        let pre = PreScan::run(&tokens, &ctx);
        (tokens, pre)
    }

    fn names_at(tokens: &[Token], idx: &HashMap<usize, SmolStr>) -> Vec<(String, String)> {
        let mut v: Vec<_> = idx
            .iter()
            .map(|(&i, target)| (tokens[i].text.to_string(), target.to_string()))
            .collect();
        v.sort();
        v
    }

    /// Pre-scan `src` in the `.dtx` lexer mode — the only one that emits
    /// `DOC_MARGIN`.
    fn scan_dtx(src: &str) -> (Vec<Token>, PreScan) {
        let ctx = ParseCtx::default();
        let cfg = LexConfig {
            flavor: crate::parser::lexer::LatexFlavor::Package,
            dtx: true,
        };
        let tokens = lex_with(src, &ctx, cfg);
        let pre = PreScan::run(&tokens, &ctx);
        (tokens, pre)
    }

    /// The predicate `doc_margin_lines` replaced: scan back from `idx` to the
    /// previous `NEWLINE` looking for a `DOC_MARGIN`. Kept here as the reference
    /// the memo is checked against.
    fn walked_back(tokens: &[Token], idx: usize) -> bool {
        tokens[..idx]
            .iter()
            .rev()
            .take_while(|t| t.kind != SyntaxKind::NEWLINE)
            .any(|t| t.kind == SyntaxKind::DOC_MARGIN)
    }

    fn memo_says(pre: &PreScan, idx: usize) -> bool {
        let n = pre.doc_margin_lines.partition_point(|&(m, _)| m < idx);
        n > 0 && pre.doc_margin_lines[n - 1].1 >= idx
    }

    #[test]
    fn doc_margin_lines_answer_exactly_what_the_backward_scan_did() {
        // Doc lines, a macrocode chunk whose code lines carry no margin, a
        // margin-only line, and an unterminated final doc line — every shape the
        // memo has to reproduce, checked at every token index.
        let src = "% \\begin{macro}{\\foo}\n\
                   %    \\begin{macrocode}\n\
                   \\def\\foo{\\begin{list}}\n\
                   %    \\end{macrocode}\n\
                   %\n\
                   plain line with no margin\n\
                   % \\end{macro}";
        let (tokens, pre) = scan_dtx(src);
        assert!(
            !pre.doc_margin_lines.is_empty(),
            "the fixture must actually produce margins"
        );
        for idx in 0..=tokens.len() {
            assert_eq!(
                memo_says(&pre, idx),
                walked_back(&tokens, idx),
                "index {idx} (token {:?})",
                tokens.get(idx).map(|t| (t.kind, t.text.as_str())),
            );
        }
    }

    #[test]
    fn doc_margin_lines_is_empty_outside_dtx() {
        // `DOC_MARGIN` is a `.dtx`-only token kind, so every other file pays
        // nothing for the memo — no allocation, and the predicate short-circuits
        // on the empty slice.
        let (_, pre) = scan("% a comment\n\\begin{itemize}\n\\end{itemize}\n");
        assert!(pre.doc_margin_lines.is_empty());
    }

    #[test]
    fn starts_are_cumulative_byte_offsets_with_a_total_at_the_end() {
        let (tokens, pre) = scan("\\foo bar");
        assert_eq!(pre.starts.len(), tokens.len() + 1);
        assert_eq!(pre.starts[0], 0);
        assert_eq!(*pre.starts.last().unwrap(), "\\foo bar".len());
        for (i, t) in tokens.iter().enumerate() {
            assert_eq!(pre.starts[i + 1] - pre.starts[i], t.text.len());
        }
    }

    #[test]
    fn empty_input_still_records_the_total_length() {
        let (_, pre) = scan("");
        assert_eq!(pre.starts, vec![0]);
        assert_eq!(pre.last_r_brace, None);
    }

    #[test]
    fn last_closer_bounds_record_the_final_occurrence_of_each_shape() {
        let src = "] } $ \\] \\) \\right \\fi ] x";
        let (tokens, pre) = scan(src);
        let text = |i: Option<usize>| i.map(|i| tokens[i].text.to_string());
        // The *last* `]`, not the first.
        assert_eq!(text(pre.last_r_bracket), Some("]".into()));
        assert!(pre.last_r_bracket.unwrap() > 0);
        assert_eq!(text(pre.last_r_brace), Some("}".into()));
        assert_eq!(text(pre.last_dollar), Some("$".into()));
        assert_eq!(text(pre.last_display_math_closer), Some("\\]".into()));
        assert_eq!(text(pre.last_inline_math_closer), Some("\\)".into()));
        assert_eq!(text(pre.last_right), Some("\\right".into()));
        assert_eq!(text(pre.last_fi), Some("\\fi".into()));
    }

    #[test]
    fn a_missing_shape_leaves_its_bound_unset() {
        let (_, pre) = scan("plain prose");
        assert_eq!(pre.last_r_bracket, None);
        assert_eq!(pre.last_r_brace, None);
        assert_eq!(pre.last_dollar, None);
        assert_eq!(pre.last_display_math_closer, None);
        assert_eq!(pre.last_inline_math_closer, None);
        assert_eq!(pre.last_right, None);
        assert_eq!(pre.last_fi, None);
    }

    /// The `\fi` bound deliberately skips the expl3 filter the gate applies: an
    /// upper bound may over-approximate but must never fall below the last index
    /// the gate could recognize.
    #[test]
    fn the_fi_bound_ignores_expl3_regions() {
        let (_, pre) = scan("\\ExplSyntaxOn \\fi \\ExplSyntaxOff");
        assert!(pre.last_fi.is_some());
        // ... while the opener set does not.
        let (_, pre) = scan("\\ExplSyntaxOn \\iftrue \\ExplSyntaxOff");
        assert!(pre.conditional_openers.is_empty());
    }

    #[test]
    fn a_conditional_opener_outside_a_region_is_recorded() {
        let (tokens, pre) = scan("\\iftrue a \\fi");
        assert_eq!(pre.conditional_openers.len(), 1);
        let i = *pre.conditional_openers.iter().next().unwrap();
        assert_eq!(tokens[i].text, "\\iftrue");
    }

    /// `\newif`'s operand is a name being declared, not a live opener — the
    /// running state `OpenerScan` carries and a per-token test could not.
    #[test]
    fn a_newif_operand_is_not_an_opener() {
        let (_, pre) = scan("\\newif\\ifdraft");
        assert!(pre.conditional_openers.is_empty());
    }

    #[test]
    fn expl_toggles_record_the_state_after_each_switch() {
        let (tokens, pre) = scan("a \\ExplSyntaxOn b \\ExplSyntaxOff c");
        let states: Vec<_> = pre
            .expl_toggles
            .iter()
            .map(|&(i, on)| (tokens[i].text.to_string(), on))
            .collect();
        assert_eq!(
            states,
            vec![
                ("\\ExplSyntaxOn".to_string(), true),
                ("\\ExplSyntaxOff".to_string(), false),
            ]
        );
    }

    #[test]
    fn an_alias_call_is_indexed_by_target() {
        let (tokens, pre) = scan_aliased("\\bea x \\eea\n");
        assert_eq!(
            names_at(&tokens, &pre.alias_openers),
            vec![("\\bea".to_string(), "eqnarray".to_string())]
        );
        assert_eq!(
            names_at(&tokens, &pre.alias_closers),
            vec![("\\eea".to_string(), "eqnarray".to_string())]
        );
    }

    /// The definee filter is load-bearing, not defensive: `\def\bea{…}` leaves
    /// `\bea` at brace depth 0 with `in_def_body` unset, so unfiltered the two
    /// *definition lines* would pair with each other and swallow the prose
    /// between them.
    #[test]
    fn a_definition_line_definee_is_not_an_opener() {
        let (_, pre) = scan_aliased("\\def\\bea{\\begin{eqnarray}}\n");
        assert!(pre.alias_openers.is_empty());
    }

    /// `\let\oldbea\bea` binds *two* names, so the countdown must span both. Left
    /// live, the source operand is a mention that pairs with the next stray
    /// closer and swallows the prose in between.
    #[test]
    fn a_let_consumes_two_name_slots() {
        let (_, pre) = scan_aliased("\\let\\oldbea\\bea\n");
        assert!(pre.alias_openers.is_empty());
        // One slot short, the source operand would have been indexed.
        let (_, pre) = scan_aliased("\\let\\oldbea\\bea \\eea\n");
        assert!(pre.alias_openers.is_empty());
        assert_eq!(pre.alias_closers.len(), 1);
    }

    /// A keyword sitting *in* a slot is the operand it looks like and does not
    /// arm a fresh countdown.
    #[test]
    fn a_definition_keyword_in_an_operand_slot_does_not_rearm() {
        let (tokens, pre) = scan_aliased("\\let\\a\\def\\bea x \\eea\n");
        // `\let\a\def` consumes both its slots. Had `\def` re-armed from the
        // operand slot, the following `\bea` would have read as a definee.
        assert_eq!(
            names_at(&tokens, &pre.alias_openers),
            vec![("\\bea".to_string(), "eqnarray".to_string())]
        );
    }

    /// Trivia between the keyword and the name must carry the countdown across.
    #[test]
    fn trivia_carries_the_name_countdown() {
        for src in [
            "\\def  \\bea",
            "\\def\n\\bea",
            "\\def % note\n\\bea",
            "\\def\t\\bea",
        ] {
            let (_, pre) = scan_aliased(src);
            assert!(pre.alias_openers.is_empty(), "{src:?} leaked a definee");
        }
    }

    /// ... and anything that is not trivia clears it.
    #[test]
    fn a_non_trivia_token_clears_the_name_countdown() {
        let (_, pre) = scan_aliased("\\def x \\bea\n");
        assert_eq!(pre.alias_openers.len(), 1);
    }

    #[test]
    fn an_alias_inside_an_expl3_region_is_excluded() {
        let (_, pre) = scan_aliased("\\ExplSyntaxOn\n\\bea\n\\ExplSyntaxOff\n\\eea\n");
        assert!(pre.alias_openers.is_empty());
        // The closer after the region is not.
        assert_eq!(pre.alias_closers.len(), 1);
    }

    #[test]
    fn no_alias_context_means_no_alias_scan() {
        let (_, pre) = scan("\\bea x \\eea");
        assert!(pre.alias_openers.is_empty());
        assert!(pre.alias_closers.is_empty());
    }
}
