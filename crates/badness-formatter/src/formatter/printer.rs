//! The layout engine: walks an [`Ir`] tree and renders it to a string, deciding
//! for each [`Ir::Group`] whether it fits flat on the current line or must break.
//!
//! This is a language-agnostic Wadler/Prettier-style layout engine. Alongside
//! ordinary greedy fills it supports source-break-aware preferred fills whose
//! gaps are selected by a global lexicographic cost.

// `print_at` is part of the engine but is not used by every lowering.
#![allow(dead_code)]

use super::ir::Ir;
use super::style::FormatStyle;
use std::rc::Rc;

/// Lexicographic cost for a source-break-aware paragraph layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct LayoutCost {
    overflow: usize,
    underflow: usize,
    changed_breaks: usize,
    displacement: usize,
    raggedness: usize,
    lines: usize,
}

impl LayoutCost {
    fn add(self, other: Self) -> Self {
        Self {
            overflow: self.overflow.saturating_add(other.overflow),
            underflow: self.underflow.saturating_add(other.underflow),
            changed_breaks: self.changed_breaks.saturating_add(other.changed_breaks),
            displacement: self.displacement.saturating_add(other.displacement),
            raggedness: self.raggedness.saturating_add(other.raggedness),
            lines: self.lines.saturating_add(other.lines),
        }
    }
}

/// The layout mode used to render a subtree. `Flat` means the producer has
/// verified that the entire flat rendering fits. `Break` lets child groups make
/// their own choices.
///
/// `FlatPrefix` is the weaker claim used by trailing-block hugs: measurement
/// stops at the first forced break, so nested groups beyond it must re-evaluate.
///
/// An `expand` group always renders in `Break`, even under an incoming `Flat`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Flat,
    FlatPrefix,
    Break,
}

/// Policy for the shared flat-measurement traversal ([`Printer::flat_end`]):
/// one walker, three deliberate readings of the same subtree, kept as explicit
/// variants so the differences stay visible parameters instead of divergent
/// copies that can drift apart.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FlatMeasure {
    /// The flat-rendered *footprint*, unbounded by the line width. A
    /// single-line forced-break `Verbatim` (a comment) counts its width — it
    /// shares the line, forcing a break only *after* — as does a group forced
    /// open only by one (`expand` is ignored; a genuinely unflattenable group
    /// surfaces its `HardLine` by recursion). Behind [`Printer::flat_width`],
    /// the fill layout's pair-fit width.
    Footprint,
    /// Can the subtree lie *fully flat* within the line width from the start
    /// column: any forced break (a comment included) fails, an `expand` group
    /// fails, and the measurement fails as soon as the width is exceeded. The
    /// non-hug `Group` decision.
    Fits,
    /// The trailing-block hug's prefix claim: a forced line break
    /// (`HardLine`/`EmptyLine`, or a *multi-line* `Verbatim`) stops the
    /// measurement *successfully* — only the prefix up to the block's opening
    /// needs to fit — while a single-line comment still fails, and content
    /// decides instead of the `expand` flag. With `excuse_overflow`, an atom
    /// that can never fit on any line (`width >= line_width`) is excused
    /// rather than failed (see `hug_excuse_overflow` on [`Ir::Group`]).
    HugPrefix { excuse_overflow: bool },
}

/// Outcome of accounting one unbreakable atom in [`Printer::flat_end`].
enum AtomStep {
    Counted,
    Excused,
    Overflow,
}

/// How the shared line measurement ([`Printer::line_fits`]) treats a
/// single-line forced-break `Verbatim` (a standalone comment) — the one
/// deliberate policy difference between its two contexts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CommentFit {
    /// A candidate carrying a comment can never render flat: fail, so the
    /// candidate picker falls through to a more broken layout.
    Fails,
    /// The rest of an already-committed line: the comment is there either
    /// way, so its width counts and the line simply ends at the break the
    /// comment forces after itself.
    SharesLine,
}

/// A unit of pending work on the printer's layout stack. Most IR nodes are a
/// plain [`Cmd::Node`]; [`Ir::Fill`] is processed incrementally as a
/// [`Cmd::Fill`], while [`Ir::PreferredFill`] carries the globally selected break
/// plan through [`Cmd::PreferredFill`].
enum Cmd<'a> {
    Node {
        indent: usize,
        mode: Mode,
        node: &'a Ir,
        /// The active margin prefix (see [`Ir::MarginPrefix`]), re-emitted at
        /// column 0 on every line break this command's subtree produces.
        /// Inherited by child commands; `None` outside any `MarginPrefix`.
        prefix: Option<&'a str>,
    },
    Fill {
        indent: usize,
        mode: Mode,
        parts: &'a [Ir],
        prefix: Option<&'a str>,
        /// A [`Ir::StickyFill`]: once an atom has broken, the rest break too.
        sticky: bool,
        /// A [`Ir::HugFill`]: an atom carrying a forced break is measured by
        /// its first line instead of a flat width it cannot have.
        hug: bool,
        /// Sticky bookkeeping: set once any earlier atom in this fill broke, so
        /// every remaining atom is forced to break. Always `false` for a plain
        /// (non-sticky) fill.
        broken: bool,
    },
    PreferredFill {
        indent: usize,
        mode: Mode,
        atoms: &'a [Ir],
        breaks: Rc<[bool]>,
        index: usize,
        prefix: Option<&'a str>,
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

    /// Move to a fresh line. With no `prefix`, indentation is deferred to `indent`
    /// columns (the default path). With a `prefix` (a [`Ir::MarginPrefix`] margin),
    /// the prefix is written eagerly flush at column 0 right after the newline and
    /// no indent is queued, so the prefix re-appears on every wrapped line.
    fn newline(&mut self, indent: usize, prefix: Option<&str>) {
        self.out.push('\n');
        self.col = 0;
        self.start_line(indent, prefix);
    }

    /// Emit a blank line, then position on a fresh line. The empty middle line
    /// never carries the `prefix` (no trailing whitespace); the fresh line does.
    fn empty_line(&mut self, indent: usize, prefix: Option<&str>) {
        self.out.push('\n');
        self.out.push('\n');
        self.col = 0;
        self.start_line(indent, prefix);
    }

    /// Position at the start of a fresh line: write an eager margin `prefix` at
    /// column 0 if active, else queue `indent` spaces (deferred until content).
    fn start_line(&mut self, indent: usize, prefix: Option<&str>) {
        match prefix {
            Some(p) => {
                self.out.push_str(p);
                self.col = p.chars().count();
                self.pending_indent = 0;
                self.needs_indent = false;
            }
            None => {
                self.pending_indent = indent;
                self.needs_indent = true;
            }
        }
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
        self.wide().run_with_mode(ir, 0, 0, Mode::Flat)
    }

    /// A copy of this printer with an effectively infinite line width, so a
    /// width-driven `Group`/`ConditionalGroup` never breaks and only structural
    /// `HardLine`s split the output. The probe behind [`Self::print_flat`] and
    /// [`Self::all_lines_fit`].
    fn wide(&self) -> Printer {
        Printer {
            line_width: usize::MAX / 2,
            indent_unit: self.indent_unit,
        }
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
            prefix: None,
        }];
        while let Some(cmd) = stack.pop() {
            let (indent, mode, node, prefix) = match cmd {
                Cmd::Node {
                    indent,
                    mode,
                    node,
                    prefix,
                } => (indent, mode, node, prefix),
                // A fill continuation: lay out the next word/separator pair (see
                // `step_fill`), pushing the remainder back for the next iteration.
                Cmd::Fill {
                    indent,
                    mode,
                    parts,
                    prefix,
                    sticky,
                    hug,
                    broken,
                } => {
                    self.step_fill(
                        &w, indent, mode, parts, prefix, sticky, hug, broken, &mut stack,
                    );
                    continue;
                }
                Cmd::PreferredFill {
                    indent,
                    mode,
                    atoms,
                    breaks,
                    index,
                    prefix,
                } => {
                    if index > 0 {
                        if mode == Mode::Break && breaks[index - 1] {
                            w.newline(indent, prefix);
                        } else {
                            w.write_text(" ");
                        }
                    }
                    if index + 1 < atoms.len() {
                        stack.push(Cmd::PreferredFill {
                            indent,
                            mode,
                            atoms,
                            breaks: Rc::clone(&breaks),
                            index: index + 1,
                            prefix,
                        });
                    }
                    // The break plan places atoms, it does not verify them:
                    // in `Break` mode each atom's own groups decide for
                    // themselves (an inherited `Flat` was verified by the
                    // parent and stays).
                    stack.push(Cmd::Node {
                        indent,
                        mode,
                        node: &atoms[index],
                        prefix,
                    });
                    continue;
                }
            };
            match node {
                Ir::Nil => {}
                Ir::Text(s) => w.write_text(s),
                Ir::Verbatim { text, .. } => w.write_verbatim(text),
                Ir::ZeroWidth(text) => w.write_verbatim(text),
                Ir::ColumnZero(text) => w.write_column_zero(text),
                Ir::Concat(items) => {
                    for item in items.iter().rev() {
                        stack.push(Cmd::Node {
                            indent,
                            mode,
                            node: item,
                            prefix,
                        });
                    }
                }
                Ir::Fill(parts) => stack.push(Cmd::Fill {
                    indent,
                    mode,
                    parts: &parts[..],
                    prefix,
                    sticky: false,
                    hug: false,
                    broken: false,
                }),
                Ir::StickyFill(parts) => stack.push(Cmd::Fill {
                    indent,
                    mode,
                    parts: &parts[..],
                    prefix,
                    sticky: true,
                    hug: false,
                    broken: false,
                }),
                Ir::HugFill(parts) => stack.push(Cmd::Fill {
                    indent,
                    mode,
                    parts: &parts[..],
                    prefix,
                    sticky: false,
                    hug: true,
                    broken: false,
                }),
                Ir::PreferredFill {
                    atoms,
                    preferred,
                    target,
                } => {
                    let breaks = if mode != Mode::Break {
                        vec![false; atoms.len().saturating_sub(1)].into()
                    } else {
                        self.stable_breaks(
                            w.current_col(),
                            indent,
                            prefix,
                            atoms,
                            preferred,
                            *target,
                        )
                    };
                    stack.push(Cmd::PreferredFill {
                        indent,
                        mode,
                        atoms,
                        breaks,
                        index: 0,
                        prefix,
                    });
                }
                Ir::Indent(inner) => {
                    stack.push(Cmd::Node {
                        indent: indent + self.indent_unit,
                        mode,
                        node: inner,
                        prefix,
                    });
                }
                Ir::Align(width, inner) => {
                    stack.push(Cmd::Node {
                        indent: indent + width,
                        mode,
                        node: inner,
                        prefix,
                    });
                }
                // Activate the margin prefix for `inner`: emit it now for the
                // first line (the leading break that put us here came from the
                // parent, outside this scope) via the column-0 pin, then print
                // `inner` with the prefix active so every later break re-emits it.
                Ir::MarginPrefix {
                    prefix: margin,
                    inner,
                } => {
                    w.write_column_zero(margin);
                    stack.push(Cmd::Node {
                        indent,
                        mode,
                        node: inner,
                        prefix: Some(margin),
                    });
                }
                Ir::Line => match mode {
                    Mode::Flat | Mode::FlatPrefix => w.write_text(" "),
                    Mode::Break => w.newline(indent, prefix),
                },
                Ir::SoftLine => {
                    if mode == Mode::Break {
                        w.newline(indent, prefix);
                    }
                }
                Ir::HardLine => w.newline(indent, prefix),
                Ir::EmptyLine => w.empty_line(indent, prefix),
                Ir::IfBreak { flat, broken } => {
                    let chosen = if mode == Mode::Break { broken } else { flat };
                    stack.push(Cmd::Node {
                        indent,
                        mode,
                        node: chosen,
                        prefix,
                    });
                }
                // Requires a `propagate_breaks`-saturated tree: honoring an
                // incoming `Flat` trusts that a group whose subtree carries a
                // hard break is `expand`-marked (the first arm), so a stale
                // flag would wrongly pin its `Line`s flat.
                Ir::Group {
                    inner,
                    expand,
                    hug,
                    hug_excuse_overflow,
                } => {
                    // Both measurements start from `current_col()` (not the
                    // raw `col`): a group dispatched right after a newline
                    // must count the pending indent, or the dropped width
                    // lets an overflowing flat layout be wrongly verified —
                    // an error the honest contract would then pin instead of
                    // letting nested re-decisions paper over it.
                    let m = if *expand {
                        Mode::Break
                    } else if mode == Mode::Flat {
                        // Honest contract: the producer verified the whole
                        // flat rendering, so don't re-decide.
                        Mode::Flat
                    } else if *hug {
                        // A trailing-block hug measures only its own prefix up to
                        // the block's opening brace; what follows sits on the
                        // block's closing line, not this one — so the claim it
                        // makes is `FlatPrefix`, never full `Flat`.
                        let measure = FlatMeasure::HugPrefix {
                            excuse_overflow: *hug_excuse_overflow,
                        };
                        if self.flat_end(w.current_col(), inner, measure).is_some() {
                            Mode::FlatPrefix
                        } else {
                            Mode::Break
                        }
                    } else if self.group_fits(w.current_col(), inner, &stack) {
                        Mode::Flat
                    } else {
                        Mode::Break
                    };
                    stack.push(Cmd::Node {
                        indent,
                        mode: m,
                        node: inner,
                        prefix,
                    });
                }
                Ir::ConditionalGroup(cands) => {
                    // Under a verified `Flat`, the flat-most candidate without
                    // re-picking — exactly how every measurement predicate
                    // models a nested conditional group, so the parent's
                    // verification and the print agree by construction.
                    let (m, chosen) = if mode == Mode::Flat {
                        (Mode::Flat, &cands[0])
                    } else {
                        self.pick_candidate(w.current_col(), cands)
                    };
                    stack.push(Cmd::Node {
                        indent,
                        mode: m,
                        node: chosen,
                        prefix,
                    });
                }
                Ir::ConditionalGroupAllLines(cands) => {
                    // `current_col()` (not the raw `col`) so a group dispatched right
                    // after a newline measures its first line from the pending
                    // indent, not column 0 — otherwise the indent width is dropped
                    // and an overflowing flat candidate is wrongly accepted.
                    let (m, chosen) = if mode == Mode::Flat {
                        (Mode::Flat, &cands[0])
                    } else {
                        self.pick_candidate_all_lines(w.current_col(), indent, cands)
                    };
                    stack.push(Cmd::Node {
                        indent,
                        mode: m,
                        node: chosen,
                        prefix,
                    });
                }
            }
        }
        w.out
    }

    /// Choose all breaks for a [`Ir::PreferredFill`] together. Authored breaks are
    /// stable once a line reaches `target` (or the next atom would reach it), but
    /// overflow always wins; ties minimize changed breaks and their displacement.
    fn stable_breaks(
        &self,
        first_col: usize,
        indent: usize,
        prefix: Option<&str>,
        atoms: &[Ir],
        preferred: &[bool],
        target: usize,
    ) -> Rc<[bool]> {
        let n = atoms.len();
        if n < 2 {
            return Vec::new().into();
        }
        debug_assert_eq!(preferred.len(), n - 1);

        let widths: Vec<usize> = atoms
            .iter()
            .map(|atom| {
                self.flat_width(atom)
                    .unwrap_or(self.line_width.saturating_add(1))
            })
            .collect();
        let mut sums = Vec::with_capacity(n + 1);
        sums.push(0usize);
        for &width in &widths {
            sums.push(sums.last().copied().unwrap_or(0).saturating_add(width));
        }
        // Source positions of the gaps in the normalized, space-joined run. They
        // are used only to rank displacement from the nearest authored anchor.
        let gap_positions: Vec<usize> = (0..n - 1)
            .map(|gap| sums[gap + 1].saturating_add(gap + 1))
            .collect();
        let authored_positions: Vec<usize> = preferred
            .iter()
            .enumerate()
            .filter_map(|(i, &is_authored)| is_authored.then_some(gap_positions[i]))
            .collect();
        let continuation_col = prefix.map_or(indent, |p| p.chars().count());
        let target = target.clamp(1, self.line_width.max(1));

        let mut costs: Vec<Option<LayoutCost>> = vec![None; n + 1];
        let mut previous: Vec<Option<usize>> = vec![None; n + 1];
        costs[0] = Some(LayoutCost::default());

        for start in 0..n {
            let Some(base_cost) = costs[start] else {
                continue;
            };
            let base_col = if start == 0 {
                first_col
            } else {
                continuation_col
            };
            let mut saw_non_overflow = false;
            for end in start + 1..=n {
                let content_width = sums[end]
                    .saturating_sub(sums[start])
                    .saturating_add(end.saturating_sub(start + 1));
                let length = base_col.saturating_add(content_width);
                let next_length =
                    (end < n).then(|| length.saturating_add(1).saturating_add(widths[end]));
                let final_line = end == n;
                let overflow = length.saturating_sub(self.line_width);
                if overflow == 0 {
                    saw_non_overflow = true;
                } else if saw_non_overflow {
                    break;
                }

                let removed_anchors = if end > start + 1 {
                    preferred[start..end - 1]
                        .iter()
                        .filter(|&&is_authored| is_authored)
                        .count()
                } else {
                    0
                };
                let new_break = usize::from(!final_line && !preferred[end - 1]);
                let displacement = if new_break == 0 || authored_positions.is_empty() {
                    0
                } else {
                    authored_positions
                        .iter()
                        .map(|&anchor| anchor.abs_diff(gap_positions[end - 1]))
                        .min()
                        .unwrap_or(0)
                };
                let underflow = if final_line
                    || length >= target
                    || next_length.is_some_and(|next| next >= target)
                {
                    0
                } else {
                    target - length
                };
                let segment = LayoutCost {
                    overflow,
                    underflow,
                    changed_breaks: removed_anchors + new_break,
                    displacement,
                    raggedness: if final_line {
                        0
                    } else {
                        target.abs_diff(length)
                    },
                    lines: 1,
                };
                let candidate = base_cost.add(segment);
                if costs[end].is_none_or(|cost| candidate < cost) {
                    costs[end] = Some(candidate);
                    previous[end] = Some(start);
                }
            }
        }

        let mut breaks = vec![false; n - 1];
        let mut cursor = n;
        while let Some(prior) = previous[cursor] {
            if prior > 0 {
                breaks[prior - 1] = true;
            }
            cursor = prior;
        }
        breaks.into()
    }

    /// One step of laying out an [`Ir::Fill`] — the Wadler/Prettier greedy fill.
    /// `parts` is the alternating `[atom, sep, atom, …]` remainder. In `Flat`
    /// mode every separator is a space (the whole fill on one line); in `Break`
    /// mode each gap is decided independently: the first atom is printed, then
    /// the separator stays flat (a space) iff the *pair* `atom + sep + next-atom`
    /// fits flat from the current column, else it breaks. A lone atom that does
    /// not fit is printed anyway (no break can rescue an unbreakable word). The
    /// remaining fill is pushed back so the next iteration decides the next gap.
    ///
    /// A **sticky** fill (`sticky`, [`Ir::StickyFill`]) instead cascades: `broken`
    /// records that an earlier atom in this fill already broke, so every remaining
    /// atom is forced to break regardless of its own fit — the greedy fill's
    /// independent gaps are exactly what would re-glue an expl3 false-branch onto a
    /// detonated block's short closing line (issue #94).
    ///
    /// A **hugging** fill (`hug`, [`Ir::HugFill`]) changes the fit question rather
    /// than the cascade: an atom that carries a forced break is placed when its
    /// *first line* fits (see [`Self::fill_atom_mode`]), instead of being pushed to
    /// a line of its own for want of a flat width.
    #[allow(clippy::too_many_arguments)]
    fn step_fill<'a>(
        &self,
        w: &Writer,
        indent: usize,
        mode: Mode,
        parts: &'a [Ir],
        prefix: Option<&'a str>,
        sticky: bool,
        hug: bool,
        broken: bool,
        stack: &mut Vec<Cmd<'a>>,
    ) {
        if parts.is_empty() {
            return;
        }
        if mode != Mode::Break {
            for part in parts.iter().rev() {
                stack.push(Cmd::Node {
                    indent,
                    mode,
                    node: part,
                    prefix,
                });
            }
            return;
        }

        let col = w.current_col();
        let content = &parts[0];
        let w0 = self.flat_width(content);
        // Under a sticky cascade every remaining atom breaks; otherwise the atom
        // is placed here when it fits — fully flat, or (in a hugging fill) with
        // only its first line on this line.
        let content_mode = (!broken)
            .then(|| self.fill_atom_mode(col, content, hug))
            .flatten();

        if parts.len() == 1 {
            // The fill's last atom shares its line with whatever the lowering
            // glued after the fill (a trailing command riding a statement), so
            // its flat claim must survive the rest of the line too — the same
            // rest-awareness `group_fits` has. Without this the atom's folded
            // hang break is never taken: the atom alone fits, goes `Flat`, and
            // the glued tail overflows the line the measurement never saw.
            //
            // A hug claim is exempt: like [`Ir::group_hug`]'s, it never covered
            // the rest of the line to begin with (what follows sits on the
            // atom's *closing* line, not this one). Without the exemption the
            // last atom of a hugging fill would break where the same atom
            // mid-fill hugs, and those are the same atom on consecutive passes
            // — a statement whose trailing `,` moved to its own line ends the
            // fill one atom earlier (`xfm.dtx`).
            let mode = match content_mode {
                Some(Mode::Flat) if matches!(w0, Some(width) if self.rest_fits(col + width, stack)) => {
                    Mode::Flat
                }
                Some(Mode::FlatPrefix) => Mode::FlatPrefix,
                _ => Mode::Break,
            };
            stack.push(Cmd::Node {
                indent,
                mode,
                node: content,
                prefix,
            });
            return;
        }

        let sep = &parts[1];
        // Pair fit: the current atom, its separator, and the next atom, all
        // flat — the next atom by the same rule as `content_mode`, so a hugging
        // fill keeps a detonating atom glued to the head before it. The current
        // atom must still claim a full flat width: after a multi-line atom the
        // next atom's column is unknown, so the gap breaks (a `Nil` separator
        // renders nothing either way, which is what re-glues a sibling onto a
        // hanging group's closing line). Alternating fills always end on an
        // atom, so `parts[2]` exists here.
        let pair_fits = !broken
            && match (w0, self.flat_width(sep)) {
                (Some(a), Some(s)) => self.fill_atom_mode(col + a + s, &parts[2], hug).is_some(),
                _ => false,
            };
        // Once any atom breaks, a sticky fill stays broken for its remainder, so
        // the later gaps break unconditionally instead of each deciding afresh.
        // A *hugged* atom starts the cascade too: it was placed on this line,
        // but only its first line fits — the rest of the line is gone, so the
        // atoms after it are exactly the ones issue #94 must not re-glue onto a
        // detonated block's short closing line.
        let remainder_broken = sticky && (broken || content_mode != Some(Mode::Flat));
        // Push the remainder first (popped last), then the separator, then the
        // content (popped first), so they print in order.
        stack.push(Cmd::Fill {
            indent,
            mode: Mode::Break,
            parts: &parts[2..],
            prefix,
            sticky,
            hug,
            broken: remainder_broken,
        });
        stack.push(Cmd::Node {
            indent,
            mode: if pair_fits { Mode::Flat } else { Mode::Break },
            node: sep,
            prefix,
        });
        stack.push(Cmd::Node {
            indent,
            mode: content_mode.unwrap_or(Mode::Break),
            node: content,
            prefix,
        });
    }

    /// The mode a fill atom is printed in when it can be placed at `col`, or
    /// `None` when it must move to the next line.
    ///
    /// A plain fill asks one question: does the atom render *fully flat* within
    /// the line width. A **hugging** fill ([`Ir::HugFill`]) accepts a second
    /// answer: an atom carrying a forced break has no flat width at all, but its
    /// *first line* may still fit here, in which case it is placed and prints as
    /// [`Mode::FlatPrefix`] — the same claim [`Ir::group_hug`] makes about a
    /// trailing block, so the atom's own body breaks below instead of the atom
    /// being pushed to a line of its own. Flat is tried first, so a single-line
    /// comment (which `HugPrefix` rejects but a flat footprint counts) still
    /// shares its line.
    fn fill_atom_mode(&self, col: usize, atom: &Ir, hug: bool) -> Option<Mode> {
        if matches!(self.flat_width(atom), Some(width) if col + width <= self.line_width) {
            return Some(Mode::Flat);
        }
        let prefix_fits = hug
            && self
                .flat_end(
                    col,
                    atom,
                    FlatMeasure::HugPrefix {
                        excuse_overflow: false,
                    },
                )
                .is_some();
        prefix_fits.then_some(Mode::FlatPrefix)
    }

    /// The flat-rendered width of `node`, or `None` if it cannot be laid flat
    /// (it carries a forced line break: a `HardLine`/`EmptyLine` or a multi-line
    /// `Verbatim`). A single-line force-break `Verbatim` (a comment) *can* share
    /// a line with what precedes it — it only forces a break *after* — so it
    /// counts as its text width here. Used by the fill layout's pair-fit test.
    fn flat_width(&self, node: &Ir) -> Option<usize> {
        self.flat_end(0, node, FlatMeasure::Footprint)
    }

    /// The shared traversal behind every flat measurement: simulate `node`
    /// laid out flat from `start_col` and return the column it ends at, or
    /// `None` when the measurement fails under `measure`'s policy (a forced
    /// break, an overflow, an `expand` group — see [`FlatMeasure`]). For a
    /// [`FlatMeasure::HugPrefix`] stop and for an excused unfittable atom the
    /// returned column is where the measurement stopped, not a full line end —
    /// those callers only ask `is_some()`.
    fn flat_end(&self, start_col: usize, node: &Ir, measure: FlatMeasure) -> Option<usize> {
        let hug = matches!(measure, FlatMeasure::HugPrefix { .. });
        let mut col = start_col;
        let mut stack: Vec<&Ir> = vec![node];
        while let Some(node) = stack.pop() {
            match node {
                Ir::Nil | Ir::SoftLine | Ir::ZeroWidth(_) => {}
                Ir::Text(s) | Ir::ColumnZero(s) => {
                    match self.flat_atom(&mut col, s.chars().count(), measure) {
                        AtomStep::Counted => {}
                        AtomStep::Excused => return Some(col),
                        AtomStep::Overflow => return None,
                    }
                }
                Ir::Verbatim { text, force_break } => {
                    // A multi-line verbatim behaves like a `HardLine`: only a
                    // hug survives it (the prefix up to its own first newline
                    // is what needed to fit).
                    if text.contains('\n') {
                        return hug.then_some(col);
                    }
                    // A standalone comment cannot lie flat (it forces a break
                    // after itself), and a comment in a hug prefix forbids
                    // the hug. Only the footprint counts it: the comment
                    // shares the line, breaking only *after*.
                    if *force_break && measure != FlatMeasure::Footprint {
                        return None;
                    }
                    match self.flat_atom(&mut col, text.chars().count(), measure) {
                        AtomStep::Counted => {}
                        AtomStep::Excused => return Some(col),
                        AtomStep::Overflow => return None,
                    }
                }
                Ir::HardLine | Ir::EmptyLine => return hug.then_some(col),
                Ir::Line => {
                    col = col.saturating_add(1);
                    if measure != FlatMeasure::Footprint && col > self.line_width {
                        return None;
                    }
                }
                Ir::Concat(items) => stack.extend(items.iter().rev()),
                Ir::Indent(inner) | Ir::Align(_, inner) => stack.push(inner),
                Ir::MarginPrefix { inner, .. } => stack.push(inner),
                Ir::IfBreak { flat, .. } => stack.push(flat),
                // `Fits` trusts the saturated `expand` flag; `Footprint` and
                // `HugPrefix` deliberately let the content decide instead. A
                // group forced open only by a single-line comment still has a
                // flat footprint (the comment shares the line, forcing a
                // break only after), and a hug must stop *successfully* at a
                // nested block's first hard break while a prefix comment
                // fails it — distinctions the flag cannot carry.
                Ir::Group { inner, expand, .. } => {
                    if *expand && measure == FlatMeasure::Fits {
                        return None;
                    }
                    stack.push(inner);
                }
                // Conservative: measure as the flat-most candidate, matching
                // how the printer resolves a conditional group under a
                // verified `Flat`.
                Ir::ConditionalGroup(cands) | Ir::ConditionalGroupAllLines(cands) => {
                    if let Some(first) = cands.first() {
                        stack.push(first);
                    }
                }
                // A fill measured flat is its atoms separated by single-space
                // `Line`s; push the parts and let the arms above account them.
                Ir::Fill(parts) | Ir::StickyFill(parts) | Ir::HugFill(parts) => {
                    stack.extend(parts.iter().rev());
                }
                Ir::PreferredFill { atoms, .. } => {
                    let gaps = atoms.len().saturating_sub(1);
                    col = col.saturating_add(gaps);
                    if measure != FlatMeasure::Footprint && col > self.line_width {
                        return None;
                    }
                    stack.extend(atoms.iter().rev());
                }
            }
        }
        Some(col)
    }

    /// Account one unbreakable atom of `width` columns during [`Self::flat_end`].
    fn flat_atom(&self, col: &mut usize, width: usize, measure: FlatMeasure) -> AtomStep {
        *col = col.saturating_add(width);
        match measure {
            FlatMeasure::Footprint => AtomStep::Counted,
            _ if *col <= self.line_width => AtomStep::Counted,
            // The atom can never fit on any line, so breaking would not
            // rescue it — the hug is excused and the measurement ends
            // successfully. See `hug_excuse_overflow` on [`Ir::Group`].
            FlatMeasure::HugPrefix {
                excuse_overflow: true,
            } if width >= self.line_width => AtomStep::Excused,
            _ => AtomStep::Overflow,
        }
    }

    /// Flat layout of an [`Ir::PreferredFill`] from `col`: its atoms joined by
    /// single spaces (`atoms.len() - 1` gaps). Returns the resulting column, or
    /// `None` if it overflows the line width or an atom cannot be laid flat
    /// (each atom is measured as its [`Self::flat_width`] footprint). Used by
    /// [`Self::line_fits`]'s flat-mode arm; [`Self::flat_end`] has its own
    /// arm, which measures the atoms under its policy instead of the footprint.
    fn preferred_fill_flat_end(&self, col: usize, atoms: &[Ir]) -> Option<usize> {
        let mut end = col.saturating_add(atoms.len().saturating_sub(1));
        for atom in atoms {
            end = end.checked_add(self.flat_width(atom)?)?;
            if end > self.line_width {
                return None;
            }
        }
        Some(end)
    }

    /// Pick the layout for an [`Ir::ConditionalGroup`] at the current column:
    /// the first candidate whose first line fits, or the last as the fallback.
    /// Always announced as `Break` — a first-line fit verifies nothing about
    /// the rest of the subtree, so under the honest [`Mode`] contract the
    /// *choice of candidate* is the whole decision and the candidate's own
    /// groups decide for themselves.
    fn pick_candidate<'a>(&self, col: usize, cands: &'a [Ir]) -> (Mode, &'a Ir) {
        let (last, rest) = cands
            .split_last()
            .expect("Ir::ConditionalGroup builder rejects empty candidate lists");
        let chosen = rest
            .iter()
            .find(|c| self.first_line_fits(col, c))
            .unwrap_or(last);
        (Mode::Break, chosen)
    }

    /// Pick the layout for an [`Ir::ConditionalGroupAllLines`]: the first
    /// candidate every one of whose rendered lines fits within `line_width`
    /// is rendered flat; if none qualifies the last candidate is rendered
    /// broken. The IR-native equivalent of the legacy `fits_with_newlines`
    /// check. Unlike [`Self::pick_candidate`], the `Flat` announced here is
    /// honest — [`Self::all_lines_fit`] verified the candidate's whole
    /// flat-mode rendering — so the chosen candidate's nested groups are
    /// pinned to exactly the layout that was measured.
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

    /// Whether every line `node` would render to fits within `line_width`, when
    /// placed at column `start_col` under the active `indent`. Used by
    /// [`Self::pick_candidate_all_lines`]. The candidate is rendered with a
    /// *very wide* line so every width-driven `Group`/`ConditionalGroup` inside it
    /// stays flat — only the candidate's own structural `HardLine`s split it into
    /// lines — and then each of those lines is measured against the real
    /// `line_width`. A candidate therefore "fits" only when its content genuinely
    /// lays out within the width without any nested group having to break: a flat
    /// candidate whose single line overflows is rejected (the broken fallback is
    /// taken) rather than silently accepted as a hybrid where a nested brace group
    /// broke to keep each printed line short.
    fn all_lines_fit(&self, start_col: usize, indent: usize, node: &Ir) -> bool {
        let rendered = self
            .wide()
            .run_with_mode(node, indent, start_col, Mode::Flat);
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

    /// Rest-aware fit check for a non-hugging [`Ir::Group`]: whether `inner`
    /// laid flat, *followed by* the already-queued `rest` commands up to the
    /// next line break, fits within the line width from `start_col`. Trailing
    /// same-line content (e.g. the closing `)` of a call hugging this group as
    /// its sole argument) counts toward the decision, so a group breaks when the
    /// inner plus what follows would overflow — not just the inner in isolation.
    /// This is the Wadler/Prettier "fits the rest of the line" rule and the cure
    /// for break decisions that were previously purely local.
    fn group_fits(&self, start_col: usize, inner: &Ir, rest: &[Cmd]) -> bool {
        self.flat_end(start_col, inner, FlatMeasure::Fits)
            .is_some_and(|end| self.rest_fits(end, rest))
    }

    /// Measure the queued commands `rest` (the printer stack after the group
    /// being decided) from `start_col`, stopping at the first line break. Each
    /// command keeps its already-decided mode. A seeding adapter over
    /// [`Self::line_fits`].
    fn rest_fits(&self, start_col: usize, rest: &[Cmd]) -> bool {
        // Seed the work stack from the printer stack (`rest` is bottom→top; `pop`
        // takes the top, i.e. the next thing to print). A `Cmd::Fill`'s parts are
        // pushed reversed so they `pop` back in fill order.
        // A stack command's `Flat` is a *verified* flat (the honest contract:
        // the printer only dispatches `Flat` after measuring the whole flat
        // rendering), so it is seeded verified and pins nested groups exactly
        // as the run loop will.
        let mut work: Vec<(Mode, bool, &Ir)> = Vec::new();
        for cmd in rest {
            match cmd {
                Cmd::Node { mode, node, .. } => work.push((*mode, *mode == Mode::Flat, node)),
                Cmd::Fill { mode, parts, .. } => {
                    for part in parts.iter().rev() {
                        work.push((*mode, *mode == Mode::Flat, part));
                    }
                }
                Cmd::PreferredFill {
                    mode, atoms, index, ..
                } => {
                    work.push((*mode, *mode == Mode::Flat, &atoms[*index]));
                }
            }
        }
        self.line_fits(start_col, work, CommentFit::SharesLine)
    }

    /// Does the *first line* of `node` fit starting at `start_col`? Unlike a
    /// flat simulation ([`Self::flat_end`]), this lets nested [`Ir::Group`]s
    /// decide their own break naturally and treats the first newline that
    /// would actually be emitted as success. A single-line forced-break
    /// `Verbatim` (a standalone comment) fails ([`CommentFit::Fails`]), since
    /// a candidate carrying one can't be rendered flat.
    ///
    /// The seed's `Flat` is a *measurement* flat, not a verified one (nothing
    /// has measured the candidate yet — that is what this call is doing), so
    /// it is seeded unverified and nested groups re-decide.
    fn first_line_fits(&self, start_col: usize, node: &Ir) -> bool {
        self.line_fits(
            start_col,
            vec![(Mode::Flat, false, node)],
            CommentFit::Fails,
        )
    }

    /// The shared line measurement behind [`Self::rest_fits`] and
    /// [`Self::first_line_fits`]: walk the pending `work` items from
    /// `start_col`, each in its mode, until the first newline that would
    /// actually be emitted (a `HardLine`/`EmptyLine`, a `Line`/`SoftLine` in
    /// `Break` mode, a multi-line `Verbatim`'s own embedded newline, or a
    /// break inside a nested node decided `Break`) — success — or the line
    /// width is exceeded — failure. Modes govern trivia; a nested `Group` or
    /// conditional group is re-decided here exactly as the printer will
    /// decide it — including the honest contract: a work item carrying a
    /// *verified* `Mode::Flat` (a stack command seeded by [`Self::rest_fits`],
    /// or a subtree this measurement itself verified via [`Self::flat_end`])
    /// pins its nested groups flat instead of re-deciding, mirroring the run
    /// loop's `Group`/conditional arms. A *measurement* `Flat` (a
    /// [`Self::first_line_fits`] candidate seed) is unverified and still
    /// re-decides.
    ///
    /// A nested group is decided *in the mode it will actually print in*:
    /// flat when its own flat rendering still fits from here, broken
    /// otherwise, with its own hug flags honored. Measuring a doomed group
    /// flat would charge the current line for width that will never land on
    /// it — and the charge would depend on where the doomed group's body
    /// happens to break, which the *previous formatting pass* decided; that
    /// was exactly the expl3 `\EditInstance{a}{b}{ …long keyvals… }`
    /// instability (issue #71). The same rule re-decides a conditional group
    /// through [`Self::pick_candidate`] rather than assuming its flat-most
    /// candidate — the two measurements share this one traversal so they
    /// cannot drift apart again.
    fn line_fits(
        &self,
        start_col: usize,
        mut work: Vec<(Mode, bool, &Ir)>,
        comments: CommentFit,
    ) -> bool {
        let mut col = start_col;
        while let Some((mode, verified, node)) = work.pop() {
            match node {
                Ir::Nil | Ir::ZeroWidth(_) => {}
                Ir::SoftLine => {
                    if mode == Mode::Break {
                        return true;
                    }
                }
                Ir::Text(s) | Ir::ColumnZero(s) => {
                    col += s.chars().count();
                    if col > self.line_width {
                        return false;
                    }
                }
                Ir::Verbatim { text, force_break } => {
                    // A multi-line verbatim's own embedded newline ends the
                    // line: only its first segment is measured.
                    if let Some((first, _)) = text.split_once('\n') {
                        col += first.chars().count();
                        return col <= self.line_width;
                    }
                    if *force_break && comments == CommentFit::Fails {
                        return false;
                    }
                    col += text.chars().count();
                    if col > self.line_width {
                        return false;
                    }
                }
                Ir::HardLine | Ir::EmptyLine => return true,
                Ir::Line => match mode {
                    Mode::Flat | Mode::FlatPrefix => {
                        col += 1;
                        if col > self.line_width {
                            return false;
                        }
                    }
                    Mode::Break => return true,
                },
                Ir::Concat(items) => {
                    for item in items.iter().rev() {
                        work.push((mode, verified, item));
                    }
                }
                Ir::Indent(inner) | Ir::Align(_, inner) => work.push((mode, verified, inner)),
                Ir::MarginPrefix { inner, .. } => work.push((mode, verified, inner)),
                Ir::IfBreak { flat, broken } => {
                    work.push((
                        mode,
                        verified,
                        if mode == Mode::Break { broken } else { flat },
                    ));
                }
                Ir::Group {
                    inner,
                    expand,
                    hug,
                    hug_excuse_overflow,
                } => {
                    // Mirrors the run loop's `Group` arm, honest contract
                    // included: under a verified `Flat` the printer pins the
                    // nested group flat (`expand` carve-out aside), so the
                    // measurement must too, or the two diverge. A non-hug
                    // group this measurement decides flat is itself verified
                    // ([`Self::flat_end`] measured its whole flat rendering).
                    let (m, v) = if *expand {
                        (Mode::Break, false)
                    } else if verified && mode == Mode::Flat {
                        (Mode::Flat, true)
                    } else {
                        let measure = if *hug {
                            FlatMeasure::HugPrefix {
                                excuse_overflow: *hug_excuse_overflow,
                            }
                        } else {
                            FlatMeasure::Fits
                        };
                        if self.flat_end(col, inner, measure).is_none() {
                            (Mode::Break, false)
                        } else if *hug {
                            (Mode::FlatPrefix, false)
                        } else {
                            (Mode::Flat, true)
                        }
                    };
                    work.push((m, v, inner));
                }
                Ir::ConditionalGroup(cands) | Ir::ConditionalGroupAllLines(cands) => {
                    // Under a verified `Flat`, the flat-most candidate without
                    // re-picking — exactly the run loop's conditional arms.
                    let (m, chosen) = if verified && mode == Mode::Flat {
                        (Mode::Flat, &cands[0])
                    } else {
                        self.pick_candidate(col, cands)
                    };
                    work.push((m, m == Mode::Flat, chosen));
                }
                Ir::Fill(parts) | Ir::StickyFill(parts) | Ir::HugFill(parts) => {
                    for item in parts.iter().rev() {
                        work.push((mode, verified, item));
                    }
                }
                Ir::PreferredFill { atoms, .. } => match mode {
                    // A preferred fill chooses its own breaks: in `Break` mode it
                    // breaks at a gap, so its first line ends after (at most) the
                    // first atom — the line fits iff that atom fits here.
                    // Measuring the whole fill flat would spuriously fail and
                    // force an enclosing group to break. In `Flat`/`FlatPrefix`
                    // mode the whole fill stays on the line.
                    Mode::Break => {
                        return atoms.first().is_none_or(|atom| {
                            self.line_fits(col, vec![(Mode::Flat, false, atom)], comments)
                        });
                    }
                    Mode::Flat | Mode::FlatPrefix => {
                        match self.preferred_fill_flat_end(col, atoms) {
                            Some(end) => col = end,
                            None => return false,
                        }
                    }
                },
            }
        }
        col <= self.line_width
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

    #[test]
    fn margin_prefix_re_emits_on_every_wrapped_line() {
        // An opaque `> ` prefix (the engine attaches no LaTeX meaning): a fill of
        // four words at width 8 wraps to two-per-line, each line re-opening with
        // the prefix flush at column 0, content measured from after it.
        let style = FormatStyle {
            line_width: 8,
            indent_width: 2,
            ..FormatStyle::default()
        };
        let printer = Printer::new(style);
        let fill = Ir::fill([
            Ir::text("aa"),
            Ir::text("bb"),
            Ir::text("cc"),
            Ir::text("dd"),
        ]);
        let ir = Ir::margin_prefix("> ", fill);
        assert_eq!(printer.print(&ir), "> aa bb\n> cc dd");
    }

    #[test]
    fn margin_prefix_emits_first_line_after_a_leading_break() {
        // The leading break is emitted by the parent (outside the MarginPrefix
        // scope); entering the scope still flushes the prefix for the first line.
        let printer = Printer::new(FormatStyle::default());
        let ir = Ir::concat([
            Ir::text("head"),
            Ir::hard_line(),
            Ir::margin_prefix("% ", Ir::text("body")),
        ]);
        assert_eq!(printer.print(&ir), "head\n% body");
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
    fn conditional_group_single_candidate_lets_children_decide_when_first_line_fits() {
        // The inner group cannot fit flat (long >> width), but the conditional
        // group's first-line measurement lets it break naturally: `f(` fits
        // and the inner emits its own newline. The chosen candidate is
        // dispatched in `Break` (the choice is the decision), so the nested
        // group re-decides for itself exactly as the measurement assumed.
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
    fn conditional_group_dispatches_the_chosen_candidate_in_break_mode() {
        // A first-line fit verifies nothing about the rest of the subtree, so
        // the chosen candidate is not announced `Flat`: its own `Line`s break
        // and its groups decide for themselves. A candidate that wants
        // flat-if-fits content must say so with a nested group.
        let printer = Printer::new(FormatStyle::default());
        let ir = Ir::conditional_group([Ir::concat([Ir::text("ab"), Ir::line(), Ir::text("cd")])]);
        assert_eq!(printer.print(&ir), "ab\ncd");
    }

    #[test]
    fn conditional_group_all_lines_pins_the_verified_candidate() {
        // The all-lines pick *did* verify the candidate's whole flat
        // rendering, so its `Flat` is honored downstream: the nested group
        // keeps exactly the layout that was measured instead of re-deciding
        // against the trailing text and detonating into a hybrid the
        // measurement never saw (`abc\ndefXXXX`).
        let style = FormatStyle {
            line_width: 10,
            indent_width: 2,
            ..FormatStyle::default()
        };
        let printer = Printer::new(style);
        let candidate = Ir::group(Ir::concat([Ir::text("abc"), Ir::line(), Ir::text("def")]));
        let ir = Ir::concat([
            Ir::conditional_group_all_lines([candidate]),
            Ir::text("XXXX"),
        ])
        .propagate_breaks();
        assert_eq!(printer.print(&ir), "abc defXXXX");
    }

    #[test]
    fn preferred_fill_atoms_inherit_the_fill_mode() {
        // The break plan places atoms, it does not verify them: in `Break`
        // mode an atom is dispatched `Break`, so a mode-sensitive child (here
        // an `IfBreak`) sees the honest mode instead of an unverified `Flat`.
        let printer = Printer::new(FormatStyle::default());
        let ir = Ir::preferred_fill(
            [Ir::text("aa"), Ir::if_break(Ir::text("F"), Ir::text("B"))],
            vec![false],
            10,
        );
        assert_eq!(printer.print(&ir), "aa B");
    }

    #[test]
    fn hug_prefix_claim_does_not_pin_a_group_past_the_forced_break() {
        // The hug's `fits` stops successfully at the first forced break, so a
        // soft group *after* the detonating block (a second trailing brace
        // argument, `\@@_if_key_value:VTF {T}{F}`) was never measured. It must
        // re-decide for itself under the hug's `FlatPrefix` — here it does not
        // fit and breaks — instead of inheriting an unverified `Flat`.
        let style = FormatStyle {
            line_width: 20,
            indent_width: 2,
            ..FormatStyle::default()
        };
        let printer = Printer::new(style);
        let t_branch = Ir::group(Ir::concat([
            Ir::text("{"),
            Ir::indent(Ir::concat([Ir::hard_line(), Ir::text("tt")])),
            Ir::hard_line(),
            Ir::text("}"),
        ]));
        let f_branch = Ir::group(Ir::concat([
            Ir::text("{"),
            Ir::indent(Ir::concat([Ir::line(), Ir::text("ffffffffffffffffffff")])),
            Ir::line(),
            Ir::text("}"),
        ]));
        let ir = Ir::group_hug(Ir::concat([
            Ir::text("\\head:TF"),
            Ir::line(),
            Ir::concat([
                Ir::text("\\v"),
                Ir::indent(Ir::concat([Ir::hard_line(), t_branch])),
                Ir::indent(Ir::concat([Ir::hard_line(), f_branch])),
            ]),
        ]))
        .propagate_breaks();
        assert_eq!(
            printer.print(&ir),
            "\\head:TF \\v\n  {\n    tt\n  }\n  {\n    ffffffffffffffffffff\n  }"
        );
    }

    #[test]
    fn hug_prefix_flat_does_not_pin_the_expanded_block() {
        // A hug verifies only its prefix, so its `Flat` must not reach into
        // the detonating block: the block group is `expand`-marked by
        // `propagate_breaks` and still prints `Break` (its `Line` breaks)
        // even though it is dispatched under the hug's flat prefix.
        let printer = Printer::new(FormatStyle::default());
        let block = Ir::group(Ir::concat([
            Ir::text("{"),
            Ir::indent(Ir::concat([
                Ir::hard_line(),
                Ir::text("x"),
                Ir::line(),
                Ir::text("y"),
            ])),
            Ir::hard_line(),
            Ir::text("}"),
        ]));
        let ir = Ir::group_hug(Ir::concat([Ir::text("f(a, "), block])).propagate_breaks();
        assert_eq!(printer.print(&ir), "f(a, {\n  x\n  y\n}");
    }

    #[test]
    fn fill_keeps_everything_on_one_line_when_it_fits() {
        let printer = Printer::new(FormatStyle::default());
        let ir = Ir::fill([Ir::text("a"), Ir::text("b"), Ir::text("c")]);
        assert_eq!(printer.print(&ir), "a b c");
    }

    #[test]
    fn fill_drops_nil_atoms_without_phantom_spaces() {
        let printer = Printer::new(FormatStyle::default());
        let ir = Ir::fill([Ir::text("a"), Ir::Nil, Ir::text("b")]);
        assert_eq!(printer.print(&ir), "a b");
    }

    #[test]
    fn preferred_fill_drops_nil_atoms_and_keeps_the_break_mask_aligned() {
        // A `Nil` atom is filtered out of `atoms`; its gap must be dropped from
        // `preferred` in lockstep, or the mask misindexes the surviving gaps (and
        // trips the `stable_breaks` debug assertions / panics on release).
        let printer = Printer::new(FormatStyle::default());
        // `preferred` has one bool per gap of the *unfiltered* three-atom run.
        let ir = Ir::preferred_fill(
            [Ir::text("a"), Ir::Nil, Ir::text("b")],
            vec![true, false],
            10,
        );
        // Fits on one line, so no break is taken regardless of the mask.
        assert_eq!(printer.print(&ir), "a b");
    }

    #[test]
    fn first_line_fits_lets_a_broken_group_break_its_preferred_fill() {
        // Five 3-char words: flat width is 5*3 + 4 gaps = 19 > 10, so the fill
        // cannot lie flat. But it chooses its own breaks, so a broken group
        // around it has a first line of just the first word (3 cols), which fits.
        // A flat-only measurement would spuriously report the first line as too
        // wide and force needless breaking upstream.
        let style = FormatStyle {
            line_width: 10,
            ..FormatStyle::default()
        };
        let printer = Printer::new(style);
        let fill = Ir::preferred_fill(
            [
                Ir::text("aaa"),
                Ir::text("bbb"),
                Ir::text("ccc"),
                Ir::text("ddd"),
                Ir::text("eee"),
            ],
            vec![false; 4],
            10,
        );
        // A break-aware group so `first_line_fits` reaches the fill in `Break`.
        let group = Ir::conditional_group([fill]);
        assert!(
            printer.first_line_fits(0, &group),
            "a broken group should let its preferred fill break for a fitting first line"
        );
    }

    #[test]
    fn preferred_fill_keeps_an_authored_break_after_a_dropped_nil() {
        // Width 1 forces every gap to break; the authored-break bit that survives
        // the `Nil` drop still drives the newline, proving the mask stayed aligned.
        let style = FormatStyle {
            line_width: 1,
            ..FormatStyle::default()
        };
        let printer = Printer::new(style);
        let ir = Ir::preferred_fill(
            [Ir::text("a"), Ir::Nil, Ir::text("b")],
            vec![false, true],
            1,
        );
        assert_eq!(printer.print(&ir), "a\nb");
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

    #[test]
    fn hug_fill_keeps_a_detonating_atom_on_the_head_line() {
        let printer = Printer::new(FormatStyle::default());
        let parts = [
            Ir::text("head"),
            Ir::line(),
            Ir::concat([Ir::text("cmd"), block()]),
        ];
        // A plain fill has no flat width for the block-bearing atom, so its gap
        // breaks and the atom starts a line of its own.
        let plain = Ir::Fill(parts.to_vec().into()).propagate_breaks();
        assert_eq!(printer.print(&plain), "head\ncmd{\n  body\n}");
        // The hugging fill measures the atom's first line instead: `cmd{` fits
        // after `head `, so the pair stays joined and only the body breaks.
        let hug = Ir::HugFill(parts.to_vec().into()).propagate_breaks();
        assert_eq!(printer.print(&hug), "head cmd{\n  body\n}");
    }

    #[test]
    fn hug_fill_hugs_its_last_atom_too() {
        // The rest-awareness that keeps a *flat* last atom honest must not
        // demote a hug claim: the same atom mid-fill and last-in-fill has to
        // land in the same place, since a statement can end one atom earlier on
        // the next pass.
        let printer = Printer::new(FormatStyle::default());
        let last = Ir::HugFill(
            vec![
                Ir::text("head"),
                Ir::line(),
                Ir::concat([Ir::text("cmd"), block()]),
            ]
            .into(),
        )
        .propagate_breaks();
        let mid = Ir::HugFill(
            vec![
                Ir::text("head"),
                Ir::line(),
                Ir::concat([Ir::text("cmd"), block()]),
                Ir::line(),
                Ir::text("tail"),
            ]
            .into(),
        )
        .propagate_breaks();
        assert_eq!(printer.print(&last), "head cmd{\n  body\n}");
        assert_eq!(printer.print(&mid), "head cmd{\n  body\n}\ntail");
    }

    #[test]
    fn hug_fill_breaks_when_even_the_first_line_does_not_fit() {
        let style = FormatStyle {
            line_width: 8,
            indent_width: 2,
            ..FormatStyle::default()
        };
        let printer = Printer::new(style);
        let ir = Ir::HugFill(
            vec![
                Ir::text("head"),
                Ir::line(),
                Ir::concat([Ir::text("command"), block()]),
            ]
            .into(),
        )
        .propagate_breaks();
        assert_eq!(printer.print(&ir), "head\ncommand{\n  body\n}");
    }
}
