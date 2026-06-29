//! The layout engine: walks an [`Ir`] tree and renders it to a string, deciding
//! for each [`Ir::Group`] whether it fits flat on the current line or must break.
//!
//! This is a language-agnostic Wadler/Prettier-style layout engine.

// `print_at` is part of the engine but unused by the identity lowering;
// keep it ready for real rules.
#![allow(dead_code)]

use super::ir::Ir;
use super::style::FormatStyle;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Flat,
    Break,
}

/// A unit of pending work on the printer's layout stack. Most IR nodes are a
/// plain [`Cmd::Node`]; [`Ir::Fill`] is processed incrementally as a
/// [`Cmd::Fill`] carrying the not-yet-laid-out remainder of its alternating
/// `[atom, sep, …]` list, so each gap is decided one at a time (see
/// [`Printer::step_fill`]).
enum Cmd<'a> {
    Node {
        indent: usize,
        mode: Mode,
        node: &'a Ir,
    },
    Fill {
        indent: usize,
        mode: Mode,
        parts: &'a [Ir],
    },
}

pub(crate) struct Printer {
    line_width: usize,
    indent_unit: usize,
}

/// Accumulates output while deferring indentation until visible content is
/// written, so blank lines never carry trailing whitespace.
struct Writer {
    out: String,
    col: usize,
    pending_indent: usize,
    needs_indent: bool,
}

impl Writer {
    fn new() -> Self {
        Self {
            out: String::new(),
            col: 0,
            pending_indent: 0,
            needs_indent: false,
        }
    }

    /// The column the next visible character would land at, accounting for an
    /// indent that has been queued (`needs_indent`) but not yet flushed — so a
    /// fill decision made right after a newline measures from the indent, not 0.
    fn current_col(&self) -> usize {
        self.col
            + if self.needs_indent {
                self.pending_indent
            } else {
                0
            }
    }

    fn flush_indent(&mut self) {
        if self.needs_indent {
            for _ in 0..self.pending_indent {
                self.out.push(' ');
            }
            self.col += self.pending_indent;
            self.needs_indent = false;
        }
    }

    /// Write text that contains no newline.
    fn write_text(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        self.flush_indent();
        self.out.push_str(s);
        self.col += s.chars().count();
    }

    /// Move to a fresh line indented to `indent`.
    fn newline(&mut self, indent: usize) {
        self.out.push('\n');
        self.col = 0;
        self.pending_indent = indent;
        self.needs_indent = true;
    }

    /// Emit a blank line, then position on a fresh line indented to `indent`.
    fn empty_line(&mut self, indent: usize) {
        self.out.push('\n');
        self.out.push('\n');
        self.col = 0;
        self.pending_indent = indent;
        self.needs_indent = true;
    }

    /// Splice a possibly multi-line string verbatim. The string is assumed to
    /// already carry its own indentation, so only a pending indent on the very
    /// first line is honored.
    fn write_verbatim(&mut self, s: &str) {
        let mut first = true;
        for segment in s.split('\n') {
            if first {
                self.flush_indent();
                first = false;
            } else {
                self.out.push('\n');
                self.col = 0;
                self.needs_indent = false;
            }
            self.out.push_str(segment);
            self.col += segment.chars().count();
        }
    }

    /// Write a single-line chunk pinned to column 0: discard any pending indent so
    /// the chunk starts flush at the line's left margin (a `.dtx` margin/guard,
    /// see [`Ir::ColumnZero`]). The caller guarantees this is the first visible
    /// token of its physical line, so dropping the indent is exactly the column-0
    /// rule and never clobbers already-emitted content.
    fn write_column_zero(&mut self, s: &str) {
        self.needs_indent = false;
        self.pending_indent = 0;
        self.out.push_str(s);
        self.col += s.chars().count();
    }
}

impl Printer {
    pub(crate) fn new(style: FormatStyle) -> Self {
        Self {
            line_width: style.line_width,
            indent_unit: style.indent_width,
        }
    }

    /// Print a complete document starting at column 0.
    pub(crate) fn print(&self, ir: &Ir) -> String {
        self.run(ir, 0, 0)
    }

    /// Print an expression that will be placed at indent level `indent_level`,
    /// without emitting the leading indent on the first line (the caller does
    /// that). The starting column accounts for the indent so width decisions
    /// match where the expression actually sits.
    pub(crate) fn print_at(&self, ir: &Ir, indent_level: usize) -> String {
        let base = indent_level * self.indent_unit;
        self.run(ir, base, base)
    }

    /// Render `ir` on a single line (every break primitive laid out flat). Used by
    /// the alignment lowering to measure and emit a table cell's content; callers
    /// must ensure `ir` carries no unconditional forced break (a `HardLine` would
    /// still emit a newline in flat mode), which the alignment grid guarantees by
    /// falling back when any cell `contains_forced_break`. Width is taken as
    /// effectively infinite so a width-driven `Group`/`ConditionalGroup` inside a
    /// cell stays flat rather than breaking on the configured line width.
    pub(crate) fn print_flat(&self, ir: &Ir) -> String {
        let flat = Printer {
            line_width: usize::MAX / 2,
            indent_unit: self.indent_unit,
        };
        flat.run_with_mode(ir, 0, 0, Mode::Flat)
    }

    fn run(&self, ir: &Ir, base_indent: usize, init_col: usize) -> String {
        self.run_with_mode(ir, base_indent, init_col, Mode::Break)
    }

    fn run_with_mode(&self, ir: &Ir, base_indent: usize, init_col: usize, mode: Mode) -> String {
        let mut w = Writer::new();
        w.col = init_col;
        let mut stack: Vec<Cmd<'_>> = vec![Cmd::Node {
            indent: base_indent,
            mode,
            node: ir,
        }];
        while let Some(cmd) = stack.pop() {
            let (indent, mode, node) = match cmd {
                Cmd::Node { indent, mode, node } => (indent, mode, node),
                // A fill continuation: lay out the next word/separator pair (see
                // `step_fill`), pushing the remainder back for the next iteration.
                Cmd::Fill {
                    indent,
                    mode,
                    parts,
                } => {
                    self.step_fill(&w, indent, mode, parts, &mut stack);
                    continue;
                }
            };
            match node {
                Ir::Nil => {}
                Ir::Text(s) => w.write_text(s),
                Ir::Verbatim { text, .. } => w.write_verbatim(text),
                Ir::ColumnZero(text) => w.write_column_zero(text),
                Ir::Concat(items) => {
                    for item in items.iter().rev() {
                        stack.push(Cmd::Node {
                            indent,
                            mode,
                            node: item,
                        });
                    }
                }
                Ir::Fill(parts) => stack.push(Cmd::Fill {
                    indent,
                    mode,
                    parts: &parts[..],
                }),
                Ir::Indent(inner) => {
                    stack.push(Cmd::Node {
                        indent: indent + self.indent_unit,
                        mode,
                        node: inner,
                    });
                }
                Ir::Align(width, inner) => {
                    stack.push(Cmd::Node {
                        indent: indent + width,
                        mode,
                        node: inner,
                    });
                }
                Ir::Line => match mode {
                    Mode::Flat => w.write_text(" "),
                    Mode::Break => w.newline(indent),
                },
                Ir::SoftLine => {
                    if mode == Mode::Break {
                        w.newline(indent);
                    }
                }
                Ir::HardLine => w.newline(indent),
                Ir::EmptyLine => w.empty_line(indent),
                Ir::IfBreak { flat, broken } => {
                    let chosen = if mode == Mode::Break { broken } else { flat };
                    stack.push(Cmd::Node {
                        indent,
                        mode,
                        node: chosen,
                    });
                }
                Ir::Group {
                    inner,
                    expand,
                    hug,
                    hug_excuse_overflow,
                } => {
                    let m = if *expand {
                        Mode::Break
                    } else if *hug {
                        // A trailing-block hug measures only its own prefix up to
                        // the block's opening brace; what follows sits on the
                        // block's closing line, not this one.
                        if self.fits(w.col, inner, true, *hug_excuse_overflow) {
                            Mode::Flat
                        } else {
                            Mode::Break
                        }
                    } else if self.group_fits(w.col, inner, &stack) {
                        Mode::Flat
                    } else {
                        Mode::Break
                    };
                    stack.push(Cmd::Node {
                        indent,
                        mode: m,
                        node: inner,
                    });
                }
                Ir::ConditionalGroup(cands) => {
                    let (m, chosen) = self.pick_candidate(w.col, cands);
                    stack.push(Cmd::Node {
                        indent,
                        mode: m,
                        node: chosen,
                    });
                }
                Ir::ConditionalGroupAllLines(cands) => {
                    let (m, chosen) = self.pick_candidate_all_lines(w.col, indent, cands);
                    stack.push(Cmd::Node {
                        indent,
                        mode: m,
                        node: chosen,
                    });
                }
            }
        }
        w.out
    }

    /// One step of laying out an [`Ir::Fill`] — the Wadler/Prettier greedy fill.
    /// `parts` is the alternating `[atom, sep, atom, …]` remainder. In `Flat`
    /// mode every separator is a space (the whole fill on one line); in `Break`
    /// mode each gap is decided independently: the first atom is printed, then
    /// the separator stays flat (a space) iff the *pair* `atom + sep + next-atom`
    /// fits flat from the current column, else it breaks. A lone atom that does
    /// not fit is printed anyway (no break can rescue an unbreakable word). The
    /// remaining fill is pushed back so the next iteration decides the next gap.
    fn step_fill<'a>(
        &self,
        w: &Writer,
        indent: usize,
        mode: Mode,
        parts: &'a [Ir],
        stack: &mut Vec<Cmd<'a>>,
    ) {
        if parts.is_empty() {
            return;
        }
        if mode == Mode::Flat {
            for part in parts.iter().rev() {
                stack.push(Cmd::Node {
                    indent,
                    mode: Mode::Flat,
                    node: part,
                });
            }
            return;
        }

        let col = w.current_col();
        let content = &parts[0];
        let w0 = self.flat_width(content);
        let content_fits = matches!(w0, Some(width) if col + width <= self.line_width);

        if parts.len() == 1 {
            stack.push(Cmd::Node {
                indent,
                mode: if content_fits {
                    Mode::Flat
                } else {
                    Mode::Break
                },
                node: content,
            });
            return;
        }

        let sep = &parts[1];
        // Pair fit: the current atom, its separator, and the next atom, all flat.
        // Alternating fills always end on an atom, so `parts[2]` exists here.
        let pair_fits = match (w0, self.flat_width(sep), self.flat_width(&parts[2])) {
            (Some(a), Some(s), Some(b)) => col + a + s + b <= self.line_width,
            _ => false,
        };
        // Push the remainder first (popped last), then the separator, then the
        // content (popped first), so they print in order.
        stack.push(Cmd::Fill {
            indent,
            mode: Mode::Break,
            parts: &parts[2..],
        });
        stack.push(Cmd::Node {
            indent,
            mode: if pair_fits { Mode::Flat } else { Mode::Break },
            node: sep,
        });
        stack.push(Cmd::Node {
            indent,
            mode: if content_fits {
                Mode::Flat
            } else {
                Mode::Break
            },
            node: content,
        });
    }

    /// The flat-rendered width of `node`, or `None` if it cannot be laid flat
    /// (it carries a forced line break: a `HardLine`/`EmptyLine` or a multi-line
    /// `Verbatim`). A single-line force-break `Verbatim` (a comment) *can* share
    /// a line with what precedes it — it only forces a break *after* — so it
    /// counts as its text width here. Used by the fill layout's pair-fit test.
    fn flat_width(&self, node: &Ir) -> Option<usize> {
        let mut total = 0usize;
        let mut stack: Vec<&Ir> = vec![node];
        while let Some(node) = stack.pop() {
            match node {
                Ir::Nil | Ir::SoftLine => {}
                Ir::Text(s) | Ir::ColumnZero(s) => total += s.chars().count(),
                Ir::Verbatim { text, .. } => {
                    if text.contains('\n') {
                        return None;
                    }
                    total += text.chars().count();
                }
                Ir::HardLine | Ir::EmptyLine => return None,
                Ir::Line => total += 1,
                Ir::Concat(items) => stack.extend(items.iter()),
                Ir::Fill(parts) => stack.extend(parts.iter()),
                Ir::Indent(inner) | Ir::Align(_, inner) => stack.push(inner),
                Ir::IfBreak { flat, .. } => stack.push(flat),
                Ir::Group { inner, expand, .. } => {
                    if *expand {
                        return None;
                    }
                    stack.push(inner);
                }
                Ir::ConditionalGroup(cands) | Ir::ConditionalGroupAllLines(cands) => {
                    if let Some(first) = cands.first() {
                        stack.push(first);
                    }
                }
            }
        }
        Some(total)
    }

    /// Pick the layout for an [`Ir::ConditionalGroup`] at the current column:
    /// the first candidate whose first line fits is rendered flat; if none, the
    /// last candidate is rendered broken. With a single candidate this is a
    /// "break-aware group" — flat if its first line fits, broken otherwise.
    fn pick_candidate<'a>(&self, col: usize, cands: &'a [Ir]) -> (Mode, &'a Ir) {
        let n = cands.len();
        for (i, c) in cands.iter().enumerate() {
            if self.first_line_fits(col, c) {
                return (Mode::Flat, c);
            }
            if i + 1 == n {
                return (Mode::Break, c);
            }
        }
        unreachable!("Ir::ConditionalGroup builder rejects empty candidate lists")
    }

    /// Pick the layout for an [`Ir::ConditionalGroupAllLines`]: the first
    /// candidate every one of whose rendered lines fits within `line_width`
    /// is rendered flat; if none qualifies the last candidate is rendered
    /// broken. The IR-native equivalent of the legacy `fits_with_newlines`
    /// check.
    fn pick_candidate_all_lines<'a>(
        &self,
        col: usize,
        indent: usize,
        cands: &'a [Ir],
    ) -> (Mode, &'a Ir) {
        let n = cands.len();
        for (i, c) in cands.iter().enumerate() {
            if self.all_lines_fit(col, indent, c) {
                return (Mode::Flat, c);
            }
            if i + 1 == n {
                return (Mode::Break, c);
            }
        }
        unreachable!("Ir::ConditionalGroupAllLines builder rejects empty candidate lists")
    }

    /// Whether every line `node` would render to fits within `line_width`,
    /// when placed at column `start_col` under the active `indent` in Flat
    /// mode (the mode the chosen candidate would be rendered in). Used by
    /// [`Self::pick_candidate_all_lines`]. Renders the candidate via the
    /// same printer machinery (so nested group decisions match the real
    /// render), then walks the output lines.
    fn all_lines_fit(&self, start_col: usize, indent: usize, node: &Ir) -> bool {
        let rendered = self.run_with_mode(node, indent, start_col, Mode::Flat);
        let mut lines = rendered.split('\n');
        if let Some(first) = lines.next()
            && start_col + first.chars().count() > self.line_width
        {
            return false;
        }
        for line in lines {
            if line.chars().count() > self.line_width {
                return false;
            }
        }
        true
    }

    /// Simulate `node` flat, starting at column `start_col`. Returns false on the
    /// first forced break or as soon as the running width exceeds the line.
    ///
    /// When `hug` is set, a forced line break (`HardLine`/`EmptyLine`) instead
    /// stops the measurement *successfully*: only the prefix up to a trailing
    /// block's opening brace needs to fit. A forced-break `Verbatim` (a comment)
    /// still fails, so a comment in the prefix prevents hugging.
    /// Whether an overflowing atom of width `w` should be *excused* during a
    /// hug-prefix fit: it can never fit on any line (`w >= line_width`), so
    /// breaking the argument list would not rescue it — only cost lines. Gated
    /// on `excuse_overflow`, which the rule sets solely when every leading
    /// argument is a bare atom (nothing breaking could rescue). See the
    /// `hug_excuse_overflow` field on [`Ir::Group`].
    fn atom_is_unfittable(&self, hug: bool, excuse_overflow: bool, w: usize) -> bool {
        hug && excuse_overflow && w >= self.line_width
    }

    fn fits(&self, start_col: usize, node: &Ir, hug: bool, excuse_overflow: bool) -> bool {
        let mut remaining = self.line_width.saturating_sub(start_col);
        let mut stack: Vec<&Ir> = vec![node];
        while let Some(node) = stack.pop() {
            match node {
                Ir::Nil | Ir::SoftLine => {}
                Ir::Text(s) | Ir::ColumnZero(s) => {
                    let w = s.chars().count();
                    if w > remaining {
                        if self.atom_is_unfittable(hug, excuse_overflow, w) {
                            return true;
                        }
                        return false;
                    }
                    remaining -= w;
                }
                Ir::HardLine | Ir::EmptyLine => return hug,
                Ir::Verbatim { text, force_break } => {
                    if *force_break {
                        // A multi-line force-break verbatim (e.g. a brace-token
                        // param default) carries its own embedded line breaks
                        // and behaves like a HardLine for hugging: the prefix
                        // up to its own first newline is what needs to fit.
                        // A single-line force-break (a standalone comment) still
                        // fails — a comment in the prefix forbids the hug.
                        if hug && text.contains('\n') {
                            return true;
                        }
                        return false;
                    }
                    let w = text.chars().count();
                    if w > remaining {
                        if self.atom_is_unfittable(hug, excuse_overflow, w) {
                            return true;
                        }
                        return false;
                    }
                    remaining -= w;
                }
                Ir::Concat(items) => {
                    for item in items.iter().rev() {
                        stack.push(item);
                    }
                }
                Ir::Indent(inner) | Ir::Align(_, inner) => stack.push(inner),
                Ir::Line => {
                    if remaining == 0 {
                        return false;
                    }
                    remaining -= 1;
                }
                Ir::IfBreak { flat, .. } => stack.push(flat),
                Ir::Group { inner, expand, .. } => {
                    if *expand {
                        return false;
                    }
                    stack.push(inner);
                }
                // Conservative: measure as the flat-most candidate. A nested
                // conditional group inside a flat measurement is rare today
                // (the only producer is the trailing-function call hug); if
                // and when one nests, this matches the most permissive layout.
                Ir::ConditionalGroup(cands) | Ir::ConditionalGroupAllLines(cands) => {
                    if let Some(first) = cands.first() {
                        stack.push(first);
                    }
                }
                // A fill measured flat is its atoms separated by single-space
                // `Line`s; push the parts and let the arms above account them.
                Ir::Fill(parts) => {
                    for item in parts.iter().rev() {
                        stack.push(item);
                    }
                }
            }
        }
        true
    }

    /// Rest-aware fit check for a non-hugging [`Ir::Group`]: whether `inner`
    /// laid flat, *followed by* the already-queued `rest` commands up to the
    /// next line break, fits within the line width from `start_col`. Trailing
    /// same-line content (e.g. the closing `)` of a call hugging this group as
    /// its sole argument) counts toward the decision, so a group breaks when the
    /// inner plus what follows would overflow — not just the inner in isolation.
    /// This is the Wadler/Prettier "fits the rest of the line" rule and the cure
    /// for break decisions that were previously purely local.
    fn group_fits(&self, start_col: usize, inner: &Ir, rest: &[Cmd]) -> bool {
        // Phase 1: `inner`, laid flat. A forced break (or an already-expanded
        // nested group) means it cannot be flat, so the group must break.
        let mut col = start_col;
        let mut stack: Vec<&Ir> = vec![inner];
        while let Some(node) = stack.pop() {
            match node {
                Ir::Nil | Ir::SoftLine => {}
                Ir::Text(s) | Ir::ColumnZero(s) => {
                    col += s.chars().count();
                    if col > self.line_width {
                        return false;
                    }
                }
                Ir::HardLine | Ir::EmptyLine => return false,
                Ir::Verbatim { text, force_break } => {
                    if *force_break {
                        return false;
                    }
                    col += text.chars().count();
                    if col > self.line_width {
                        return false;
                    }
                }
                Ir::Concat(items) => {
                    for item in items.iter().rev() {
                        stack.push(item);
                    }
                }
                Ir::Indent(i) | Ir::Align(_, i) => stack.push(i),
                Ir::Line => {
                    col += 1;
                    if col > self.line_width {
                        return false;
                    }
                }
                Ir::IfBreak { flat, .. } => stack.push(flat),
                Ir::Group {
                    inner: gi, expand, ..
                } => {
                    if *expand {
                        return false;
                    }
                    stack.push(gi);
                }
                Ir::ConditionalGroup(cands) | Ir::ConditionalGroupAllLines(cands) => {
                    if let Some(first) = cands.first() {
                        stack.push(first);
                    }
                }
                Ir::Fill(parts) => {
                    for item in parts.iter().rev() {
                        stack.push(item);
                    }
                }
            }
        }
        // Phase 2: the rest of the line, each command in its decided mode, until
        // a line break (the line fits) or the width is exceeded (it does not).
        self.rest_fits(col, rest)
    }

    /// Measure the queued commands `rest` (the printer stack after the group
    /// being decided) from `start_col`, stopping at the first line break. Each
    /// command keeps its already-decided mode; an undecided nested group is
    /// measured flat (optimistic), an expanded one in break mode so its first
    /// soft break ends the line. Returns whether everything up to that break
    /// fits within the line width.
    fn rest_fits(&self, start_col: usize, rest: &[Cmd]) -> bool {
        let mut col = start_col;
        // Seed the work stack from the printer stack (`rest` is bottom→top; `pop`
        // takes the top, i.e. the next thing to print). A `Cmd::Fill`'s parts are
        // pushed reversed so they `pop` back in fill order.
        let mut work: Vec<(Mode, &Ir)> = Vec::new();
        for cmd in rest {
            match cmd {
                Cmd::Node { mode, node, .. } => work.push((*mode, node)),
                Cmd::Fill { mode, parts, .. } => {
                    for part in parts.iter().rev() {
                        work.push((*mode, part));
                    }
                }
            }
        }
        while let Some((mode, node)) = work.pop() {
            match node {
                Ir::Nil | Ir::SoftLine if mode == Mode::Flat => {}
                Ir::Nil => {}
                Ir::SoftLine => return true,
                Ir::Text(s) | Ir::ColumnZero(s) => {
                    col += s.chars().count();
                    if col > self.line_width {
                        return false;
                    }
                }
                Ir::Verbatim { text, .. } => {
                    if let Some((first, _)) = text.split_once('\n') {
                        col += first.chars().count();
                        return col <= self.line_width;
                    }
                    col += text.chars().count();
                    if col > self.line_width {
                        return false;
                    }
                }
                Ir::HardLine | Ir::EmptyLine => return true,
                Ir::Line => match mode {
                    Mode::Flat => {
                        col += 1;
                        if col > self.line_width {
                            return false;
                        }
                    }
                    Mode::Break => return true,
                },
                Ir::Concat(items) => {
                    for item in items.iter().rev() {
                        work.push((mode, item));
                    }
                }
                Ir::Indent(i) | Ir::Align(_, i) => work.push((mode, i)),
                Ir::IfBreak { flat, broken } => {
                    work.push((mode, if mode == Mode::Break { broken } else { flat }));
                }
                Ir::Group { inner, expand, .. } => {
                    work.push((if *expand { Mode::Break } else { Mode::Flat }, inner));
                }
                Ir::ConditionalGroup(cands) | Ir::ConditionalGroupAllLines(cands) => {
                    if let Some(first) = cands.first() {
                        work.push((Mode::Flat, first));
                    }
                }
                Ir::Fill(parts) => {
                    for item in parts.iter().rev() {
                        work.push((mode, item));
                    }
                }
            }
        }
        col <= self.line_width
    }

    /// Does the *first line* of `node` fit starting at `start_col`? Unlike
    /// [`Self::fits`] (a flat simulation), this lets nested [`Ir::Group`]s
    /// decide their own break naturally — they re-use the existing flat
    /// `fits` exactly as the real printer does — and treats the first
    /// newline that would actually be emitted (a `HardLine`/`EmptyLine`, a
    /// `Line`/`SoftLine` in `Break` mode, or anything in a nested group
    /// decided `Break`) as success. A *single-line* forced-break `Verbatim`
    /// (e.g. a standalone comment) fails, since it can't be rendered flat;
    /// a *multi-line* `Verbatim` (e.g. a function arg fallback-rendered as
    /// a multi-line legacy chunk) measures only its first line — its own
    /// embedded newline counts as the success signal.
    fn first_line_fits(&self, start_col: usize, node: &Ir) -> bool {
        let mut col = start_col;
        let mut stack: Vec<(Mode, &Ir)> = vec![(Mode::Flat, node)];
        while let Some((mode, node)) = stack.pop() {
            match node {
                Ir::Nil => {}
                Ir::Text(s) | Ir::ColumnZero(s) => {
                    col += s.chars().count();
                    if col > self.line_width {
                        return false;
                    }
                }
                Ir::Verbatim { text, force_break } => {
                    if let Some(first_line) = text.split_once('\n').map(|(l, _)| l) {
                        col += first_line.chars().count();
                        if col > self.line_width {
                            return false;
                        }
                        return true;
                    }
                    if *force_break {
                        return false;
                    }
                    col += text.chars().count();
                    if col > self.line_width {
                        return false;
                    }
                }
                Ir::Concat(items) => {
                    for item in items.iter().rev() {
                        stack.push((mode, item));
                    }
                }
                Ir::Indent(inner) | Ir::Align(_, inner) => stack.push((mode, inner)),
                Ir::Line => match mode {
                    Mode::Flat => {
                        col += 1;
                        if col > self.line_width {
                            return false;
                        }
                    }
                    Mode::Break => return true,
                },
                Ir::SoftLine => {
                    if mode == Mode::Break {
                        return true;
                    }
                }
                Ir::HardLine | Ir::EmptyLine => return true,
                Ir::IfBreak { flat, broken } => {
                    let chosen = if mode == Mode::Break { broken } else { flat };
                    stack.push((mode, chosen));
                }
                Ir::Group {
                    inner,
                    expand,
                    hug,
                    hug_excuse_overflow,
                } => {
                    let m = if *expand || !self.fits(col, inner, *hug, *hug_excuse_overflow) {
                        Mode::Break
                    } else {
                        Mode::Flat
                    };
                    stack.push((m, inner));
                }
                Ir::Fill(parts) => {
                    for item in parts.iter().rev() {
                        stack.push((mode, item));
                    }
                }
                Ir::ConditionalGroup(cands) | Ir::ConditionalGroupAllLines(cands) => {
                    let (m, chosen) = self.pick_candidate(col, cands);
                    stack.push((m, chosen));
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A block that always breaks: `{`, an indented body, then `}`.
    fn block() -> Ir {
        Ir::concat([
            Ir::text("{"),
            Ir::indent(Ir::concat([Ir::hard_line(), Ir::text("body")])),
            Ir::hard_line(),
            Ir::text("}"),
        ])
    }

    /// `f(a, {block})` as a hug group: prefix `f(a, ` then a trailing block.
    fn hug_call() -> Ir {
        Ir::group_hug(Ir::concat([
            Ir::text("f("),
            Ir::indent(Ir::concat([
                Ir::soft_line(),
                Ir::text("a"),
                Ir::if_break(Ir::text(", "), Ir::text(",")),
            ])),
            Ir::if_break(block(), Ir::indent(Ir::concat([Ir::soft_line(), block()]))),
            Ir::soft_line(),
            Ir::text(")"),
        ]))
    }

    #[test]
    fn hug_group_keeps_prefix_flat_when_it_fits() {
        let printer = Printer::new(FormatStyle::default());
        assert_eq!(printer.print(&hug_call()), "f(a, {\n  body\n})");
    }

    #[test]
    fn hug_group_expands_when_prefix_does_not_fit() {
        // A narrow line forces even the short prefix `f(a, {` to break.
        let style = FormatStyle {
            line_width: 5,
            indent_width: 2,
            ..FormatStyle::default()
        };
        let printer = Printer::new(style);
        assert_eq!(
            printer.print(&hug_call()),
            "f(\n  a,\n  {\n    body\n  }\n)"
        );
    }

    #[test]
    fn hug_group_expands_when_prefix_has_a_comment() {
        // A forced-break verbatim (a comment) in the prefix prevents hugging
        // even though the prefix is short.
        let printer = Printer::new(FormatStyle::default());
        let ir = Ir::group_hug(Ir::concat([
            Ir::text("f("),
            Ir::indent(Ir::concat([
                Ir::soft_line(),
                Ir::verbatim_forced("# c"),
                Ir::hard_line(),
                Ir::text("a"),
                Ir::if_break(Ir::text(", "), Ir::text(",")),
            ])),
            Ir::if_break(block(), Ir::indent(Ir::concat([Ir::soft_line(), block()]))),
            Ir::soft_line(),
            Ir::text(")"),
        ]));
        // Expanded: the comment lands on its own line and the block is indented.
        assert_eq!(printer.print(&ir), "f(\n  # c\n  a,\n  {\n    body\n  }\n)");
    }

    /// A nested group whose flat form overflows the line but whose own break
    /// emits a newline before the overflow point. The conditional group's
    /// first-line measurement lets the nested group break, so the outer line
    /// fits even though the inner cannot stay flat.
    fn nested_breakable_group(width: usize) -> Ir {
        let long = "x".repeat(width);
        // Inner group: flat = `(<long>)` (overflows at width ≥ ~outer.width);
        // broken = `(\n  <long>\n)`.
        let inner = Ir::group(Ir::concat([
            Ir::text("("),
            Ir::indent(Ir::concat([Ir::soft_line(), Ir::text(long)])),
            Ir::soft_line(),
            Ir::text(")"),
        ]));
        // Outer candidate: `f` then the inner group. Its first line is `f(`.
        Ir::concat([Ir::text("f"), inner])
    }

    #[test]
    fn conditional_group_single_candidate_flat_when_first_line_fits() {
        // The inner group cannot fit flat (long >> width), but the conditional
        // group's first-line measurement lets it break naturally: `f(` fits
        // and the inner emits its own newline.
        let style = FormatStyle {
            line_width: 10,
            indent_width: 2,
            ..FormatStyle::default()
        };
        let printer = Printer::new(style);
        let ir = Ir::conditional_group([nested_breakable_group(20)]);
        assert_eq!(printer.print(&ir), "f(\n  xxxxxxxxxxxxxxxxxxxx\n)");
    }

    #[test]
    fn conditional_group_single_candidate_breaks_when_first_line_does_not_fit() {
        // A long literal in the candidate's first line itself blows the budget
        // before any nested group can break: fall to Break mode for the same
        // (single) candidate.
        let style = FormatStyle {
            line_width: 5,
            indent_width: 2,
            ..FormatStyle::default()
        };
        let printer = Printer::new(style);
        // Candidate: `verylong` then a Line. In Flat: `verylong ` overflows;
        // in Break: the Line becomes a newline.
        let ir = Ir::conditional_group([Ir::concat([
            Ir::text("verylong"),
            Ir::line(),
            Ir::text("x"),
        ])]);
        assert_eq!(printer.print(&ir), "verylong\nx");
    }

    #[test]
    fn conditional_group_picks_first_fitting_candidate() {
        let style = FormatStyle {
            line_width: 6,
            indent_width: 2,
            ..FormatStyle::default()
        };
        let printer = Printer::new(style);
        // c0 doesn't fit; c1 fits; c2 (fallback) never reached.
        let c0 = Ir::text("toolongtofit");
        let c1 = Ir::text("ok");
        let c2 = Ir::concat([Ir::text("fallback"), Ir::hard_line(), Ir::text("more")]);
        let ir = Ir::conditional_group([c0, c1, c2]);
        assert_eq!(printer.print(&ir), "ok");
    }

    #[test]
    fn conditional_group_falls_back_to_last_in_break_mode() {
        let style = FormatStyle {
            line_width: 4,
            indent_width: 2,
            ..FormatStyle::default()
        };
        let printer = Printer::new(style);
        // Neither earlier candidate fits; the last is rendered broken (its
        // `Line` becomes a newline).
        let c0 = Ir::text("toolongtofit");
        let c1 = Ir::text("alsotoolong");
        let c2 = Ir::concat([Ir::text("ab"), Ir::line(), Ir::text("cd")]);
        let ir = Ir::conditional_group([c0, c1, c2]);
        assert_eq!(printer.print(&ir), "ab\ncd");
    }

    #[test]
    fn fill_keeps_everything_on_one_line_when_it_fits() {
        let printer = Printer::new(FormatStyle::default());
        let ir = Ir::fill([Ir::text("a"), Ir::text("b"), Ir::text("c")]);
        assert_eq!(printer.print(&ir), "a b c");
    }

    #[test]
    fn fill_wraps_words_greedily_at_the_width() {
        let style = FormatStyle {
            line_width: 10,
            indent_width: 2,
            ..FormatStyle::default()
        };
        let printer = Printer::new(style);
        // "aaa bbb" (7) fits; adding " ccc" would reach 11 > 10, so break; then
        // "ccc ddd" (7) fits. The break is decided per gap, not all-or-nothing.
        let ir = Ir::fill([
            Ir::text("aaa"),
            Ir::text("bbb"),
            Ir::text("ccc"),
            Ir::text("ddd"),
        ]);
        assert_eq!(printer.print(&ir), "aaa bbb\nccc ddd");
    }

    #[test]
    fn fill_continuation_lines_take_the_current_indent() {
        let style = FormatStyle {
            line_width: 6,
            indent_width: 2,
            ..FormatStyle::default()
        };
        let printer = Printer::new(style);
        // Inside an indent: "aa bb" (5) fits on the first line (which carries no
        // leading indent here), then "cc" wraps to a fresh line at indent 2.
        let ir = Ir::indent(Ir::fill([Ir::text("aa"), Ir::text("bb"), Ir::text("cc")]));
        assert_eq!(printer.print(&ir), "aa bb\n  cc");
    }

    #[test]
    fn align_hangs_continuation_to_marker_width() {
        let style = FormatStyle {
            line_width: 12,
            indent_width: 2,
            ..FormatStyle::default()
        };
        let printer = Printer::new(style);
        // A `\item `-style marker (width 6) followed by a hanging-indented fill:
        // the first word sits after the marker, wrapped words align under it.
        let ir = Ir::concat([
            Ir::text("* "),
            Ir::align(
                2,
                Ir::fill([Ir::text("aa"), Ir::text("bbbb"), Ir::text("cc")]),
            ),
        ]);
        // "* aa" = 4, +" bbbb" = 9, +" cc" = 12 <= 12, so it all fits on one line.
        assert_eq!(printer.print(&ir), "* aa bbbb cc");
        // Narrower: force a wrap and check the continuation aligns to column 2.
        let narrow = Printer::new(FormatStyle {
            line_width: 9,
            indent_width: 2,
            ..FormatStyle::default()
        });
        assert_eq!(narrow.print(&ir), "* aa bbbb\n  cc");
    }
}
