//! Math bodies, scripts, and paired `\left`/`\right` delimiters.
//!
//! All math bodies share atom and script parsing, including named environments.
//! Entry gates live with the grammar's shared shape-gate machinery.

use super::trivia::CommentMode;
use super::{Block, LEFT_CMD, PARSER_STEP_LIMIT, Parser, RIGHT_CMD, peek_end_name};
use crate::parser::events::Event;
use crate::semantic::signature::ArgumentDomain;
use crate::syntax::SyntaxKind;

impl Parser<'_> {
    /// Inline `$ … $` or display `$$ … $$` math. The body's atoms are wrapped in
    /// a `MATH` node (the delimiters stay direct children of the math node); the
    /// atoms themselves are parsed in math mode (see [`Self::math_element`]).
    /// Entry is gated by [`Self::dollar_closes`]: the caller has already
    /// verified a closer is reachable, so the unclosed-math recovery paths
    /// below fire only for shapes the gate scan cannot see (they remain as
    /// belt-and-braces recovery, never the expected path).
    pub(super) fn dollar_math(&mut self) {
        let display = self.nth_kind(1) == Some(SyntaxKind::DOLLAR);
        let (kind, label) = if display {
            (SyntaxKind::DISPLAY_MATH, "$$")
        } else {
            (SyntaxKind::INLINE_MATH, "$")
        };
        let opener = (
            self.starts[self.pos],
            self.starts[self.pos + if display { 2 } else { 1 }],
        );
        self.open(kind);
        self.bump(); // $
        if display {
            self.bump(); // second $
        }
        self.open(SyntaxKind::MATH);
        self.math_dollar.push(true);
        loop {
            match self.kind() {
                None => {
                    self.error_at(opener, format!("unclosed `{label}`"));
                    break;
                }
                // `}` and `\end` are recovery anchors: `$`-math cannot span a
                // group or environment boundary, so a `}` here closes the
                // enclosing group (a math subgroup would have entered via `{`)
                // and a `\end` belongs to an enclosing environment. Leave the
                // token for the caller and report the unclosed math.
                Some(SyntaxKind::R_BRACE) => {
                    self.error_at(opener, format!("unclosed `{label}`"));
                    break;
                }
                Some(SyntaxKind::CONTROL_WORD) if self.at_env_end() => {
                    self.error_at(opener, format!("unclosed `{label}`"));
                    break;
                }
                Some(SyntaxKind::DOLLAR) => {
                    if display && self.nth_kind(1) != Some(SyntaxKind::DOLLAR) {
                        // A lone `$` inside `$$`: malformed; emit and continue.
                        self.bump();
                        continue;
                    }
                    // The closing delimiter belongs to the math node, not its
                    // body: break and bump it after closing `MATH`.
                    break;
                }
                _ => {
                    if self.at_paragraph_break() {
                        // Faithful to TeX: a blank line is a `\par`, and `\par`
                        // in math mode is "Missing $ inserted" — even inside an
                        // alignment cell (#35). Name the cause so the opener
                        // span isn't read as a bogus report.
                        self.error_at(
                            opener,
                            format!("unclosed `{label}` (a blank line ends math)"),
                        );
                        break;
                    }
                    self.math_element();
                }
            }
        }
        self.math_dollar.pop();
        self.close(); // MATH
        if self.kind() == Some(SyntaxKind::DOLLAR) {
            self.bump(); // closing $
            if display {
                self.bump(); // second closing $
            }
        }
        self.close(); // INLINE_MATH / DISPLAY_MATH
    }

    /// Delimited math: `\[ … \]` (display) or `\( … \)` (inline). As with
    /// [`Self::dollar_math`], the body's atoms are wrapped in a `MATH` node and
    /// parsed in math mode.
    pub(super) fn delim_math(&mut self, kind: SyntaxKind, opener: &str, closer: &str) {
        let opener_span = self.token_span(self.pos);
        self.open(kind);
        self.bump(); // \[ or \(
        self.open(SyntaxKind::MATH);
        self.math_dollar.push(false);
        loop {
            match self.kind() {
                None => {
                    self.error_at(opener_span, format!("unclosed `{opener}`"));
                    break;
                }
                Some(SyntaxKind::CONTROL_SYMBOL) if self.text() == closer => {
                    // The closer belongs to the math node, not its body.
                    break;
                }
                // A `}` closes an enclosing group: it cannot belong to this
                // math (a subgroup would have entered via `{`). Leave it for
                // the caller and report the unclosed math.
                Some(SyntaxKind::R_BRACE) => {
                    self.error_at(opener_span, format!("unclosed `{opener}`"));
                    break;
                }
                Some(SyntaxKind::CONTROL_WORD) if self.at_env_end() => {
                    self.error_at(opener_span, format!("unclosed `{opener}`"));
                    break;
                }
                _ => {
                    if self.at_paragraph_break() {
                        // Same rationale as in `dollar_math`: `\par` ends math.
                        self.error_at(
                            opener_span,
                            format!("unclosed `{opener}` (a blank line ends math)"),
                        );
                        break;
                    }
                    self.math_element();
                }
            }
        }
        self.math_dollar.pop();
        self.close(); // MATH
        if self.kind() == Some(SyntaxKind::CONTROL_SYMBOL) && self.text() == closer {
            self.bump(); // \] or \)
        }
        self.close(); // INLINE_MATH / DISPLAY_MATH
    }

    /// One element inside a math body. Trivia is emitted inline (for
    /// losslessness); everything else is an atom, possibly carrying `^`/`_`
    /// scripts (see [`Self::math_scripted`]). Callers guard the math closers and
    /// recovery anchors before invoking this, so the cursor is at body content.
    pub(super) fn math_element(&mut self) {
        match self.kind() {
            Some(k) if Self::is_trivia(k) => self.bump(),
            _ => self.math_scripted(),
        }
    }

    /// A base atom with any tightly-bound `^`/`_` scripts — the one sanctioned
    /// Pratt site (`AGENTS.md`, decision #3). Sub/superscripts are postfix with a
    /// single-atom right operand, so this is a base atom followed by a postfix
    /// loop, not full precedence climbing.
    ///
    /// We only wrap the base in a `SCRIPTED` node when a script actually
    /// attaches, so an unscripted atom stays a bare token/node (matching the
    /// `LINE_BREAK`-only-when-modifiers idiom). Because the base atom's extent is
    /// not known until parsed (a command greedily attaches its args), we parse it
    /// first and, if a script follows, retroactively splice a `SCRIPTED` start
    /// event in front of it — the event-stream analog of rust-analyzer's
    /// `precede`, done locally without touching the event layer.
    fn math_scripted(&mut self) {
        // The lexer keeps ordinary characters in coarse `WORD` runs. Preserve an
        // unscripted run as one CST token. When a script follows, expose only the
        // final Unicode scalar as TeX's one-token base without changing the lexer.
        if self.kind() == Some(SyntaxKind::WORD) {
            let idx = self.pos;
            self.pos += 1;
            if self.at_script() {
                let end = self.tokens[idx].text.len();
                self.math_word_fragment(idx, 0, end);
            } else {
                self.events.push(Event::Tok(idx));
            }
            return;
        }
        let checkpoint = self.events.len();
        self.math_atom();
        self.math_scripts(checkpoint);
    }

    /// Emit one unconsumed byte range of a lexer `WORD`. If a script follows, the
    /// final Unicode scalar is isolated as its base; TeX tokenizes an ordinary
    /// input character separately even though the lossless lexer coalesces such
    /// characters.
    ///
    /// A fragment can be the remainder of a bare script argument (`x^23_i` leaves
    /// `3` after the `2`). In that case `self.pos` already points beyond the lexer
    /// token, so a following script correctly binds to the fragment's final atom.
    fn math_word_fragment(&mut self, idx: usize, start: usize, end: usize) {
        debug_assert!(start < end, "math WORD fragment must be non-empty");
        if !self.at_script() {
            self.events.push(Event::SubTok { idx, start, end });
            return;
        }

        let last = self.tokens[idx].text[start..end]
            .char_indices()
            .next_back()
            .map(|(offset, _)| start + offset)
            .expect("a WORD fragment is non-empty");
        if start < last {
            self.events.push(Event::SubTok {
                idx,
                start,
                end: last,
            });
        }
        let checkpoint = self.events.len();
        self.events.push(Event::SubTok {
            idx,
            start: last,
            end,
        });
        self.math_scripts(checkpoint);
    }

    /// Attach any `^`/`_` scripts that follow the base atom emitted since
    /// `checkpoint`, retro-splicing a `SCRIPTED` wrapper in front of it
    /// ([`Self::precede`]). No script → the base stays a bare atom.
    fn math_scripts(&mut self, checkpoint: usize) {
        if !self.at_script() {
            return; // bare atom, no wrapper
        }
        self.precede(checkpoint, SyntaxKind::SCRIPTED);
        let mut remainder = None;
        while self.at_script() {
            self.skip_trivia(); // trivia between base/scripts rides inside SCRIPTED
            let sub = self.kind() == Some(SyntaxKind::UNDERSCORE);
            self.open(if sub {
                SyntaxKind::SUBSCRIPT
            } else {
                SyntaxKind::SUPERSCRIPT
            });
            self.bump(); // `_` or `^`
            remainder = self.math_script_arg();
            self.close();
            // The rest of a coalesced WORD is outer math content. Any next script
            // belongs to its final atom, not to the base we just closed.
            if remainder.is_some() {
                break;
            }
        }
        self.close(); // SCRIPTED
        if let Some((idx, start, end)) = remainder {
            self.math_word_fragment(idx, start, end);
        }
    }

    /// True if a `^`/`_` script operator directly follows, skipping only
    /// `WHITESPACE`/`NEWLINE` (not a comment, which must end its line — so a
    /// script never binds across a comment) and not a blank line (a paragraph
    /// break ends the math).
    fn at_script(&self) -> bool {
        // `CommentMode::Stop`: a comment ends the line, so it stops the scan (and
        // is reported as the next meaningful token, which is not a script), rather
        // than being skipped as it is elsewhere. A blank line ends the math.
        let s = self.scan_trivia(self.pos, CommentMode::Stop);
        !s.saw_blank_line
            && matches!(
                s.next_kind,
                Some(SyntaxKind::CARET | SyntaxKind::UNDERSCORE)
            )
    }

    /// A single base atom: a `{…}` group (parsed in math mode), a command with
    /// its greedily-attached arguments, an environment, a `\\` line break, or one
    /// ordinary token. Always consumes at least one token.
    ///
    /// **Caller contract: the cursor must not be at EOF.** The `None` arm below
    /// consumes nothing and emits nothing, so a caller that reaches it from a
    /// loop spins until [`PARSER_STEP_LIMIT`] and panics far from the mistake.
    /// Every loop that reaches here guards EOF already — the four math bodies
    /// with an explicit `None` arm, `math_environment_body` through
    /// [`Self::at_block_end`], and [`Self::math_script_arg`] through its own
    /// missing-argument check — and this turns that unwritten contract into a
    /// tripwire that fires at the offending call instead.
    fn math_atom(&mut self) {
        debug_assert!(!self.at_end(), "math_atom at EOF: caller must guard first");
        match self.kind() {
            Some(SyntaxKind::L_BRACE) => self.math_group(),
            Some(SyntaxKind::CONTROL_WORD) => {
                // Same definition-body/expl3-region and brace-less gates as
                // [`Self::element`] (issues #45/#60).
                if !self.in_macro_code(self.pos) && self.at_env_begin() {
                    // The group-escape gate is mode-independent: braces remain
                    // TeX structure inside math, so an environment macro cannot
                    // consume the closing brace of the group that contains it.
                    if self.environment_escapes_group(self.pos) {
                        if let Some(name) = peek_end_name(self.tokens, self.pos) {
                            self.demoted_envs.insert(name.into_owned());
                        }
                        self.command();
                    } else {
                        self.environment();
                    }
                } else if !self.in_macro_code(self.pos) && self.at_env_end() {
                    // [`Self::at_block_end`] declines to end a math body at a
                    // `\end` that orphans a `\begin` the brace-group gate demoted
                    // (issue #71), so that one arrives *here* — and must land as
                    // the plain command that verdict already made it. Reporting
                    // it stray would have the two halves of one gate disagree.
                    if self.end_orphans_a_demoted_begin(self.pos) {
                        self.command();
                    } else {
                        self.stray_end();
                    }
                } else if let Some((target, closer)) = (!self.in_macro_code(self.pos))
                    .then(|| {
                        let target = self.alias_openers.get(&self.pos)?.clone();
                        Some((target, self.alias_closer(self.pos)?))
                    })
                    .flatten()
                {
                    // The [`Self::element`] arm, in math (issue #117). Not an
                    // optional extra: `split` — the environment the issue is
                    // about — is math-only, so an alias for it is *always* read
                    // here and nowhere else. `alias_environment` routes the body
                    // by the target exactly as `environment()` does one token
                    // earlier in this same match.
                    self.alias_environment(&target, closer);
                } else if self.at_command(LEFT_CMD) && self.left_right_closes(self.pos) {
                    self.left_right();
                } else if self.at_command(RIGHT_CMD) {
                    self.stray_right();
                } else {
                    self.command();
                }
            }
            // `\\` line break (with its tightly-bound `*`/`[len]`) vs. a bare
            // control symbol (`\,`, `\;`, `\!`, spacing) — emit the latter as a
            // single token.
            Some(SyntaxKind::CONTROL_SYMBOL) if self.text() == "\\\\" => self.line_break(),
            // Any other single token (WORD, digit, `&`, `~`, `#`, brackets, a
            // bare control symbol, or a `^`/`_` with no base): one token, so the
            // loop always makes progress.
            Some(_) => self.bump(),
            // Ruled out by the caller contract above; kept because release
            // builds compile the assert away and the match must be total.
            None => {}
        }
    }

    /// One script argument: a single atom (a `{…}` group, a command with its
    /// args, or one input character from a lexer `WORD`). The remainder of a
    /// `WORD` is returned to [`Self::math_scripts`] so it can become outer math
    /// content after the `SCRIPTED` node closes. A missing argument (the next
    /// meaningful token is a closer, `\end`, a paragraph break, or EOF) is
    /// reported, not consumed — the closer must stay for the enclosing math loop.
    fn math_script_arg(&mut self) -> Option<(usize, usize, usize)> {
        if self.at_paragraph_break() {
            self.error("missing argument after `^`/`_`");
            return None;
        }
        self.skip_trivia();
        let missing = match self.kind() {
            None | Some(SyntaxKind::R_BRACE | SyntaxKind::DOLLAR) => true,
            Some(SyntaxKind::CONTROL_SYMBOL) => matches!(self.text(), "\\]" | "\\)"),
            Some(SyntaxKind::CONTROL_WORD) => self.at_env_end(),
            _ => false,
        };
        if missing {
            self.error("missing argument after `^`/`_`");
            return None;
        }
        if self.kind() == Some(SyntaxKind::WORD) {
            let idx = self.pos;
            let text = &self.tokens[idx].text;
            let end = text.len();
            let first_end = text.char_indices().nth(1).map_or(end, |(offset, _)| offset);
            self.events.push(Event::SubTok {
                idx,
                start: 0,
                end: first_end,
            });
            self.pos += 1;
            return (first_end < end).then_some((idx, first_end, end));
        }
        self.math_atom();
        None
    }

    /// A brace group `{ … }` whose body is parsed in math mode (so `x^{a_b}`
    /// nests). Recovery mirrors [`Self::group`].
    fn math_group(&mut self) {
        self.argument_group(ArgumentDomain::Math);
    }

    /// A `\left<delim> … \right<delim>` matched delimiter pair (`AGENTS.md`,
    /// decision #3: the one precedence-climbing site — here just balanced
    /// matching by *count*, which is exactly how TeX pairs them, so a mismatched
    /// `\left( … \right]` still nests correctly). The `\left`/`\right` control
    /// words and their delimiter tokens are direct children (mirroring how `$` /
    /// `\[` delimiters stay direct children of the math node); the enclosed atoms
    /// are wrapped in a `MATH` body. Nested pairs recurse via [`Self::math_atom`].
    ///
    /// An unclosed `\left` recovers at the enclosing math/group/environment
    /// closer (the same anchors the surrounding math loop uses), leaving that
    /// token for the caller.
    fn left_right(&mut self) {
        debug_assert!(self.at_command(LEFT_CMD));
        let opener = self.token_span(self.pos);
        self.open(SyntaxKind::LEFT_RIGHT);
        self.bump(); // \left
        self.math_delim(LEFT_CMD);
        self.open(SyntaxKind::MATH);
        loop {
            match self.kind() {
                None => {
                    self.error_at(opener, "unclosed `\\left`");
                    break;
                }
                Some(SyntaxKind::CONTROL_WORD) if self.at_command(RIGHT_CMD) => break,
                // Enclosing-scope closers: `\left … \right` cannot span a group,
                // math, or environment boundary, so hand the token back.
                Some(SyntaxKind::R_BRACE | SyntaxKind::DOLLAR) => {
                    self.error_at(opener, "unclosed `\\left`");
                    break;
                }
                Some(SyntaxKind::CONTROL_SYMBOL) if matches!(self.text(), "\\]" | "\\)") => {
                    self.error_at(opener, "unclosed `\\left`");
                    break;
                }
                Some(SyntaxKind::CONTROL_WORD) if self.at_env_end() => {
                    self.error_at(opener, "unclosed `\\left`");
                    break;
                }
                _ => {
                    if self.at_paragraph_break() {
                        self.error_at(opener, "unclosed `\\left`");
                        break;
                    }
                    self.math_element();
                }
            }
        }
        self.close(); // MATH
        if self.at_command(RIGHT_CMD) {
            self.bump(); // \right
            self.math_delim(RIGHT_CMD);
        }
        self.close(); // LEFT_RIGHT
    }

    /// Consume the single delimiter token following `\left`/`\right`: skip inline
    /// trivia (it rides as a direct child of the pair for losslessness; the
    /// formatter drops it), then take one token. The lexer has already isolated a
    /// word-character delimiter (`(`, `|`, `.`, …) into its own token, so a single
    /// `bump` suffices. A missing delimiter — the next meaningful token is a
    /// closer, another `\left`/`\right`, `\end`, a paragraph break, or EOF — is
    /// reported, not consumed.
    fn math_delim(&mut self, after: &str) {
        self.skip_trivia();
        let missing = match self.kind() {
            None | Some(SyntaxKind::R_BRACE | SyntaxKind::DOLLAR) => true,
            Some(SyntaxKind::CONTROL_SYMBOL) => matches!(self.text(), "\\]" | "\\)"),
            Some(SyntaxKind::CONTROL_WORD) => {
                self.at_env_end() || self.at_command(LEFT_CMD) || self.at_command(RIGHT_CMD)
            }
            _ => false,
        };
        if missing {
            self.error(format!("missing delimiter after `{after}`"));
            return;
        }
        self.bump();
    }

    /// A `\right` with no open `\left` (the math loop only reaches one here when
    /// it is unmatched). Report it and consume it with its delimiter so the parse
    /// stays lossless and makes progress.
    fn stray_right(&mut self) {
        debug_assert!(self.at_command(RIGHT_CMD));
        self.error("`\\right` without matching `\\left`");
        self.bump(); // \right
        self.math_delim(RIGHT_CMD);
    }

    /// The body of a named math environment (`equation`, `align`, `gather`, …): its
    /// atoms wrapped in a `MATH` node and parsed in math mode, exactly as `\[…\]`
    /// (see [`Self::delim_math`]) — so `^`/`_` build `SCRIPTED` nodes, the operator
    /// split fires, and `\left…\right` pair. Routed here for environments the
    /// signature data flags `math` ([`ParseCtx::is_math_environment`]).
    ///
    /// The terminator is the matching `\end` (or EOF), read via [`Self::at_block_end`]
    /// just like [`Self::parse_block`]; [`Self::finish_environment`] then consumes and
    /// name-checks it. Unlike `$`-math (where a `\end` is an *unclosed*-recovery
    /// anchor), `\end` is the normal, expected terminator here. A blank line inside the
    /// body stays trivia within the `MATH` node — no paragraph split — so losslessness
    /// holds. Progress is guaranteed: [`Self::math_element`] bumps trivia or descends
    /// into [`Self::math_scripted`], whose atom parser always consumes a token.
    pub(super) fn math_environment_body(&mut self) {
        self.open(SyntaxKind::MATH);
        self.math_dollar.push(false);
        while !self.at_block_end(Block::Environment) {
            self.math_element();
        }
        self.math_dollar.pop();
        self.close(); // MATH
    }
}
