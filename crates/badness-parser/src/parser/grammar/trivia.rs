//! Trivia scanning and the leading-comment bind.
//!
//! Every blank-line and comment-attachment question the grammar asks is decided
//! from one walk of a trivia run ([`Parser::scan_trivia`]) rather than five
//! near-identical ones. The attachment rule itself is `AGENTS.md` #9, after
//! rust-analyzer: comments bind *forward* into the construct they annotate,
//! whitespace floats, and a blank line breaks the bind.

use super::{BEGIN_CMD, END_CMD, Parser};
use crate::syntax::SyntaxKind;

/// How the shared trivia scanner ([`Parser::scan_trivia`]) treats a `%` comment.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CommentMode {
    /// A comment is content that occupies its own line: it resets the newline
    /// run (without undoing a blank line already seen) and the scan continues
    /// past it. Used everywhere paragraph structure and leading-comment binds are
    /// decided.
    Skip,
    /// A comment stops the scan and is reported as the next meaningful token.
    /// Used by [`Parser::at_script`], where a comment ends the line so a `^`/`_`
    /// script never binds across it.
    Stop,
}

/// Newlines in a trivia run that make it a **blank line** — TeX's `\par`
/// boundary, and one of the few trivia predicates layout may read (`AGENTS.md`,
/// trivia-invariant layout). Two, because one newline only ends a line.
pub(super) const BLANK_LINE_NEWLINES: usize = 2;

/// The result of scanning the contiguous trivia run at a position: everything the
/// blank-line and comment-bind rules (AGENTS.md #9) need to decide, computed once.
pub(super) struct TriviaScan {
    /// Index of the next meaningful (non-skipped) token, or `tokens.len()` at EOF.
    pub(super) next: usize,
    /// Kind of the token at [`Self::next`], or `None` at EOF.
    pub(super) next_kind: Option<SyntaxKind>,
    /// The run before `next` contains a blank line (≥2 `NEWLINE`s: the `\par`
    /// boundary).
    pub(super) saw_blank_line: bool,
    /// As [`Self::saw_blank_line`], but a `.dtx` docstrip guard line counts as
    /// content rather than blank space. Docstrip *deletes* a guard-only line
    /// when it strips the file, so `%<*dtx>` between two lines does not part
    /// them — read by the shape gates that only ask whether their construct's
    /// source ran out mid-shape (issue #71).
    pub(super) saw_blank_line_outside_guards: bool,
    /// Start index of the leading own-line `%` comment run immediately preceding
    /// `next` — the maximal blank-line-free suffix, the start of a
    /// leading-comment bind. `None` if that suffix has no own-line comment.
    /// Only populated in [`CommentMode::Skip`].
    pub(super) comment_start: Option<usize>,
}

impl Parser<'_> {
    pub(super) fn is_trivia(k: SyntaxKind) -> bool {
        matches!(
            k,
            SyntaxKind::WHITESPACE
                | SyntaxKind::NEWLINE
                | SyntaxKind::COMMENT
                | SyntaxKind::DOC_MARGIN
                | SyntaxKind::GUARD
        )
    }

    pub(super) fn skip_trivia(&mut self) {
        while self.kind().is_some_and(Self::is_trivia) {
            self.bump();
        }
    }

    /// Scan the contiguous trivia run starting at `from`, classifying each token
    /// so the blank-line and leading-comment-bind rules (AGENTS.md #9) can be
    /// decided from one walk instead of five near-identical ones. `WHITESPACE`,
    /// `DOC_MARGIN`, and `GUARD` float (a `.dtx` margin neither counts as a
    /// newline nor resets the run, so a margin-only line `%\n%\n` still reads as a
    /// blank line via its two `NEWLINE`s); `NEWLINE`s accumulate into the blank-line
    /// (`≥2`) test; a `COMMENT` is handled per [`CommentMode`]. A `GUARD` floats
    /// for `saw_blank_line` but breaks the run for
    /// [`TriviaScan::saw_blank_line_outside_guards`]. Does not consume.
    pub(super) fn scan_trivia(&self, from: usize, comment_mode: CommentMode) -> TriviaScan {
        let mut i = from;
        let mut newlines = 0;
        let mut guard_newlines = 0;
        let mut saw_blank_line = false;
        let mut saw_blank_line_outside_guards = false;
        let mut comment_start = None;
        while let Some(t) = self.tokens.get(i) {
            match t.kind {
                SyntaxKind::NEWLINE => {
                    newlines += 1;
                    guard_newlines += 1;
                    if newlines >= BLANK_LINE_NEWLINES {
                        saw_blank_line = true;
                        // A blank line breaks a leading-comment bind: only a
                        // comment *after* it can still bind, so drop any comment
                        // seen before it.
                        comment_start = None;
                    }
                    if guard_newlines >= BLANK_LINE_NEWLINES {
                        saw_blank_line_outside_guards = true;
                    }
                }
                SyntaxKind::WHITESPACE | SyntaxKind::DOC_MARGIN => {}
                // A docstrip guard floats like a margin for the layout rules
                // (`saw_blank_line`), but it is *content* on its line — and a
                // line docstrip deletes outright when it strips the file, so a
                // guard-only line is not a blank line separating what surrounds
                // it. Constructs that only need to know whether their source
                // ran out mid-shape read `saw_blank_line_outside_guards`
                // instead (issue #71).
                SyntaxKind::GUARD => guard_newlines = 0,
                SyntaxKind::COMMENT if comment_mode == CommentMode::Stop => break,
                // A comment occupies its own line: it is content, not blank space,
                // so it resets the newline run (without undoing a blank line
                // already seen) and, if it starts its line, opens a
                // leading-comment bind.
                SyntaxKind::COMMENT => {
                    newlines = 0;
                    guard_newlines = 0;
                    if comment_start.is_none() && self.comment_starts_line(i) {
                        comment_start = Some(i);
                    }
                }
                _ => break,
            }
            i += 1;
        }
        TriviaScan {
            next: i,
            next_kind: self.tokens.get(i).map(|t| t.kind),
            saw_blank_line,
            saw_blank_line_outside_guards,
            comment_start,
        }
    }

    /// Peek the kind of the next non-trivia token and whether the intervening
    /// trivia contains a paragraph break (a blank line, i.e. ≥2 newlines).
    /// Does not consume.
    pub(super) fn peek_meaningful(&self) -> (Option<SyntaxKind>, bool) {
        let s = self.scan_trivia(self.pos, CommentMode::Skip);
        (s.next_kind, s.saw_blank_line)
    }

    /// Text of the next non-trivia token at/after `self.pos`, if any. Does not
    /// consume. Used to distinguish a verbatim-argument `VERB` from a standalone
    /// `\verb…` token (see `attach_arguments`).
    pub(super) fn peek_meaningful_text(&self) -> Option<&str> {
        let mut i = self.pos;
        while let Some(t) = self.tokens.get(i) {
            if !Self::is_trivia(t.kind) {
                return Some(t.text.as_str());
            }
            i += 1;
        }
        None
    }

    /// True if a paragraph break (blank line) begins at the current position.
    pub(super) fn at_paragraph_break(&self) -> bool {
        self.scan_trivia(self.pos, CommentMode::Skip).saw_blank_line
    }

    /// [`Self::at_paragraph_break`], but blind to `.dtx` docstrip guard lines:
    /// a `%<*dtx>`/`%</dtx>` pair on its own lines is not the blank line it
    /// looks like, because docstrip deletes those lines outright. Used by the
    /// bail-out anchors of constructs that legitimately span a guarded block —
    /// `\ProvidesPackage{…}` and its `[…date…]` optional, split across
    /// `%<package>`/`%<*dtx>` variants (rotating.dtx, issue #71).
    pub(super) fn at_paragraph_break_outside_guards(&self) -> bool {
        self.scan_trivia(self.pos, CommentMode::Skip)
            .saw_blank_line_outside_guards
    }

    /// True if the comment at `pos` starts its own line: scanning back over
    /// inline whitespace only, the preceding token is a `NEWLINE` or the start of
    /// input. A same-line trailing comment (`\foo % x`) returns `false` and never
    /// binds forward (see [`Self::binding_run`]).
    pub(super) fn comment_starts_line(&self, pos: usize) -> bool {
        let mut i = pos;
        while i > 0 {
            i -= 1;
            match self.tokens[i].kind {
                // A `.dtx` margin or guard is skipped like whitespace when deciding
                // whether a comment owns its line (neither is itself the comment).
                SyntaxKind::WHITESPACE | SyntaxKind::DOC_MARGIN | SyntaxKind::GUARD => {
                    continue;
                }
                SyntaxKind::NEWLINE => return true,
                _ => return false,
            }
        }
        true
    }

    /// If the trivia run at `from` ends in a `%` comment run that binds *leading*
    /// into a following documentable construct, return
    /// `(comment_start, construct_pos, construct_kind)`:
    /// - `comment_start` — index of the first own-line comment of the binding run
    ///   (the maximal blank-line-free suffix; trivia before it floats),
    /// - `construct_pos` — index of the construct's control word,
    /// - `construct_kind` — `ENVIRONMENT` for `\begin`, otherwise `COMMAND`.
    ///
    /// Returns `None` when the run has no own-line comment, a blank line separates
    /// the comment from the construct, or the next non-trivia token is not a
    /// documentable construct. Mirrors rust-analyzer's `n_attached_trivias`
    /// (AGENTS.md #9): comments bind forward to the item they annotate, a blank
    /// line breaks the bind, and a same-line trailing comment never binds.
    ///
    /// One deliberate divergence: RA peeks *past* a blank line and keeps attaching
    /// when the next comment is an outer doc comment (`///`/`//!`). LaTeX's single
    /// `%` carries no such intent marker, so we always stop at the blank line (only
    /// the maximal blank-line-free suffix binds). See AGENTS.md #9;
    /// `comment_after_blank_line_still_binds` (`tests/parser.rs`) pins the divergence.
    pub(super) fn binding_run(&self, from: usize) -> Option<(usize, usize, SyntaxKind)> {
        let s = self.scan_trivia(from, CommentMode::Skip);
        let start = s.comment_start?;
        if s.next_kind != Some(SyntaxKind::CONTROL_WORD) {
            return None;
        }
        // An alias closer terminates the body just as `\end` does, so it is not a
        // construct a preceding comment run may bind to. Without this the run would
        // classify as `COMMAND`, the body loop would open a `DOC_COMMENT` and then
        // call `element` *at the closer* — consuming it inside the body, so the
        // environment never closes.
        if self.alias_end.is_some_and(|end| s.next >= end) {
            return None;
        }
        let kind = match self.tokens[s.next].text.as_str() {
            BEGIN_CMD => SyntaxKind::ENVIRONMENT,
            END_CMD => return None,
            _ => SyntaxKind::COMMAND,
        };
        Some((start, s.next, kind))
    }
}
