//! A lightweight Wadler/Prettier-style intermediate representation (IR) for the
//! formatter.
//!
//! Construct formatters build an [`Ir`] tree describing possible layouts.
//! [`super::printer::Printer`] resolves it against the configured line width.

// Some language-independent IR primitives are not used by every lowering.
#![allow(dead_code)]

use std::rc::Rc;

/// A document node describing how a piece of code may be laid out.
#[derive(Debug, Clone)]
pub(crate) enum Ir {
    /// Literal text. Must never contain a newline.
    Text(Rc<str>),
    /// A sequence of nodes printed back-to-back.
    Concat(Rc<[Ir]>),
    /// Flat mode: a single space. Break mode: newline + current indent.
    Line,
    /// Flat mode: nothing. Break mode: newline + current indent.
    SoftLine,
    /// Always a newline + current indent, regardless of mode. Forces every
    /// enclosing [`Ir::Group`] to break.
    HardLine,
    /// A blank line followed by the next line's indent. Like [`Ir::HardLine`] it
    /// forces enclosing groups to break.
    EmptyLine,
    /// Increase the indent of everything inside by one `indent_width` step.
    Indent(Rc<Ir>),
    /// Increase the indent of everything inside by an explicit number of columns
    /// — unlike [`Ir::Indent`], not tied to `indent_width`. Used for a *hanging
    /// indent* that must align continuation lines under a marker of arbitrary
    /// width, e.g. a list item's wrapped lines aligning under the text after
    /// `\item `. Build via [`Ir::align`].
    Align(usize, Rc<Ir>),
    /// Set continuation indentation to the column where `inner` begins. Unlike
    /// [`Ir::Align`], this reads the actual rendered column, so a block nested
    /// after a variable-width inline prefix can hang its closer under its opener.
    /// Build via [`Ir::align_current`].
    AlignCurrent(Rc<Ir>),
    /// A current-column-aware choice between an aligned layout and its base-indent
    /// fallback. In break mode, the aligned branch is used only when every
    /// continuation line it would render fits the configured width from the
    /// actual current column. Flat mode always uses `aligned`, since no
    /// continuation indentation is emitted. Build via [`Ir::bounded_align`].
    BoundedAlign { aligned: Rc<Ir>, fallback: Rc<Ir> },
    /// A break-decision boundary. The printer measures the flat rendering of
    /// `inner`; if it fits and contains no forced break, it prints flat,
    /// otherwise broken. `expand` forces broken unconditionally. After
    /// lowering, [`Ir::propagate_breaks`] saturates the flag: every non-hug
    /// group whose inner contains a forced break is marked, making `expand`
    /// the one representation of "forced open" the printer trusts. Hug groups
    /// are never marked — their inner holds a forced break by construction
    /// (the trailing block), and their break decision is the hug fit, not the
    /// flag. `flat_width` deliberately ignores the flag: a group forced only
    /// by a single-line comment still has a flat width.
    ///
    /// `hug` enables trailing-block hugging: the fit measurement stops
    /// *successfully* at the first forced line break (the opening of a trailing
    /// block) rather than failing on it. This lets a group whose last element is
    /// a block (`f(a, {`…`})`) stay flat — the prefix hugs the block's open
    /// brace — when only the prefix needs to fit. A comment in the prefix
    /// (`Verbatim { force_break: true }`) still fails the fit, forcing expansion.
    Group {
        inner: Rc<Ir>,
        expand: bool,
        hug: bool,
        /// Whether `inner` contains an unconditional forced break. Computed
        /// when the immutable group is built, so lowering-time queries stop at
        /// group boundaries instead of repeatedly walking nested subtrees.
        forced_break: bool,
        /// Only meaningful together with `hug`. When set, the prefix fit
        /// measurement *excuses* a leading argument that is an unbreakable atom
        /// too wide to fit on any line (`width >= line_width`): such an atom
        /// would overflow whether or not the list breaks, so it must not, by
        /// itself, force the hug to expand. Set by the rule only when every
        /// leading argument is such a bare atom (no nested breakable group, so
        /// nothing is rescuable by breaking). See the `test_that("<long>", {…})`
        /// case: breaking buys no width, only lines.
        hug_excuse_overflow: bool,
    },
    /// Emit `flat` when the enclosing group is flat, `broken` when it is broken.
    IfBreak { flat: Rc<Ir>, broken: Rc<Ir> },
    /// Pre-rendered text spliced through untouched. When `force_break` is set the enclosing group cannot
    /// stay flat (used for comments and for multi-line bridged renderings);
    /// otherwise it behaves as opaque inline text of its own width.
    Verbatim { text: Rc<str>, force_break: bool },
    /// Pre-rendered trailing text that contributes **zero width** to every fit
    /// measurement (a rustfmt-style trailing comment): the printer splices
    /// `text` verbatim, but the flat/fill/rest measurements skip it entirely,
    /// so its length never influences how the code before it breaks — the
    /// rendered line may simply overflow. Must sit immediately before a line
    /// break (the text ends its physical line, e.g. a `%` comment); it is the
    /// caller's job to guarantee nothing follows on the same line. Build via
    /// [`Ir::zero_width`].
    ZeroWidth(Rc<str>),
    /// A verbatim chunk pinned to column 0: before splicing `text` the printer
    /// discards any pending indent so the chunk starts flush at the line's left
    /// margin. Used for a `.dtx` documentation margin (`%`) or docstrip guard
    /// (`%<…>`), which docstrip anchors at column 0 regardless of the surrounding
    /// LaTeX nesting. Always the first visible token of its physical line, so
    /// zeroing the indent is exactly the column-0 rule. Behaves as opaque inline
    /// text otherwise (no forced break). Build via [`Ir::column_zero`].
    ColumnZero(Rc<str>),
    /// An ordered list of candidate layouts. The printer picks the first
    /// candidate whose *first line* fits at the current column under a
    /// break-aware measurement (nested groups decide their own break, success
    /// is the first emitted newline); if none fit, the last candidate is
    /// rendered broken. With a single candidate this degenerates to a
    /// "break-aware group": flat if its first line fits, broken otherwise.
    /// Must contain at least one candidate.
    ConditionalGroup(Rc<[Ir]>),
    /// Same shape as [`Ir::ConditionalGroup`] but selected by an *all-lines*
    /// measurement: the printer renders each candidate at the current column
    /// and picks the first whose every rendered line fits within
    /// `line_width`. The last candidate is rendered broken when none fit.
    /// Use for choices like "keep this body bare if every rendered line fits,
    /// else wrap in braces" — the IR port of the legacy `fits_with_newlines`
    /// check.
    ConditionalGroupAllLines(Rc<[Ir]>),
    /// A Wadler/Prettier *fill*: an alternating list `[atom, sep, atom, sep, …,
    /// atom]` (even indices content, odd indices separators, each separator an
    /// [`Ir::Line`]). Unlike a [`Ir::Group`], the printer decides each separator
    /// *independently* — it stays flat (a space) when the surrounding pair fits
    /// and breaks otherwise — so a run of words greedily fills each line. This is
    /// the primitive paragraph reflow lowers to. Build via [`Ir::fill`].
    ///
    /// `Group`/`ConditionalGroup` cannot express this: a group is all-or-nothing
    /// (every `Line` flat or every `Line` broken), and a conditional group picks
    /// among whole-layout candidates — neither wraps word-by-word.
    Fill(Rc<[Ir]>),
    /// A *sticky-break* fill: laid out greedily like [`Ir::Fill`], but the break
    /// decision cascades — once any atom is placed on a broken (multi-line) line,
    /// every subsequent atom breaks too, instead of each gap deciding
    /// independently. Same alternating `[atom, sep, atom, …]` shape as
    /// [`Ir::Fill`]. Used for expl3 statement lines, whose hanging brace arguments
    /// must all move to their own line once the true-branch body detonates
    /// (`\@ifpackageloaded {pkg} {…block…} {}` → the empty false-branch drops to
    /// its own line rather than gluing onto the block's short closing `}` line).
    /// The greedy fill's independent gaps would glue it back — and, worse,
    /// unstably, since where the block's own body happens to break is not a
    /// pass-invariant (issue #94). Shares [`Ir::Fill`]'s builder shape; the
    /// expl3 statement lowering constructs it directly.
    StickyFill(Rc<[Ir]>),
    /// A *hugging* fill: laid out greedily like [`Ir::Fill`], but an atom that
    /// carries a forced break is measured by its **first line** (the prefix up
    /// to that break, [`crate::formatter::printer::FlatMeasure::HugPrefix`])
    /// instead of a whole-atom flat width it can never have. So a detonating
    /// block stays glued to the head it follows and lets its own body break
    /// below, exactly as [`Ir::group_hug`] does for a single trailing block —
    /// the fill-level form of the same rule. Same alternating `[atom, sep,
    /// atom, …]` shape as [`Ir::Fill`].
    ///
    /// Used for expl3 *fallback* statement lines, where whether an atom is
    /// forced is not pass-invariant: a width wrap inside a fallback statement's
    /// content mints newlines the next parse re-segments into hard-broken
    /// statements, so a plain fill's `flat_width` dispatch renders one layout on
    /// pass 1 and another on pass 2 (`\vbox to \Gin@req@height{%`,
    /// `\hbox_set_to_wd:Nnn \l_shipout_box \l_shipout_box_wd_dim {…}`). Measuring
    /// the first line is invariant under that flip: a soft atom's prefix *is* its
    /// flat width, so both passes place the atom identically.
    HugFill(Rc<[Ir]>),
    /// A paragraph fill whose gaps remember which ones were authored newlines.
    /// The printer selects a global minimum-cost layout: overflow first, then
    /// short lines relative to `target`, changed authored breaks, displacement,
    /// raggedness, and line count. `preferred.len() == atoms.len() - 1`.
    PreferredFill {
        atoms: Rc<[Ir]>,
        preferred: Rc<[bool]>,
        target: usize,
    },
    /// Re-emit `prefix` at column 0 on every line `inner` produces. While the
    /// printer lays out `inner`, each line break (and the first line) writes
    /// `prefix` flush at the left margin immediately after the newline, and
    /// subsequent width decisions on that line measure from after the prefix.
    /// `prefix` is opaque text the engine attaches no meaning to (the `.dtx`
    /// lowering uses `"% "` to re-emit a documentation margin on each *wrapped*
    /// line); it must never contain a newline. A blank line never carries the
    /// prefix on its empty line. Build via [`Ir::margin_prefix`].
    MarginPrefix {
        prefix: Rc<str>,
        blank_prefix: Option<Rc<str>>,
        inner: Rc<Ir>,
    },
    /// Nothing.
    Nil,
}

impl Ir {
    pub(crate) fn text(s: impl Into<Rc<str>>) -> Ir {
        Ir::Text(s.into())
    }

    pub(crate) fn concat(items: impl IntoIterator<Item = Ir>) -> Ir {
        let items: Vec<Ir> = items
            .into_iter()
            .filter(|i| !matches!(i, Ir::Nil))
            .collect();
        match items.len() {
            0 => Ir::Nil,
            1 => items.into_iter().next().unwrap(),
            _ => Ir::Concat(items.into()),
        }
    }

    /// Interleave `items` with `sep`.
    pub(crate) fn join(sep: Ir, items: impl IntoIterator<Item = Ir>) -> Ir {
        let mut out = Vec::new();
        for (i, item) in items.into_iter().enumerate() {
            if i > 0 {
                out.push(sep.clone());
            }
            out.push(item);
        }
        Ir::concat(out)
    }

    pub(crate) fn group(inner: Ir) -> Ir {
        let forced_break = inner.contains_forced_break();
        Ir::Group {
            inner: Rc::new(inner),
            expand: false,
            hug: false,
            forced_break,
            hug_excuse_overflow: false,
        }
    }

    /// A group that hugs a trailing block: the printer keeps it flat as long as
    /// the prefix up to the block's opening brace fits, then lets the block
    /// break onto its own lines. See [`Ir::Group`]'s `hug` field.
    pub(crate) fn group_hug(inner: Ir) -> Ir {
        let forced_break = inner.contains_forced_break();
        Ir::Group {
            inner: Rc::new(inner),
            expand: false,
            hug: true,
            forced_break,
            hug_excuse_overflow: false,
        }
    }

    /// Like [`Self::group_hug`], but the prefix fit measurement excuses a
    /// leading argument that is an unbreakable atom too wide to fit on any line.
    /// See [`Ir::Group`]'s `hug_excuse_overflow` field. Callers must only use
    /// this when every leading argument is a bare atom (nothing breaking could
    /// rescue), so the excuse cannot hide a genuinely fittable argument.
    pub(crate) fn group_hug_excused(inner: Ir) -> Ir {
        let forced_break = inner.contains_forced_break();
        Ir::Group {
            inner: Rc::new(inner),
            expand: false,
            hug: true,
            forced_break,
            hug_excuse_overflow: true,
        }
    }

    /// An ordered list of candidate layouts; see [`Ir::ConditionalGroup`].
    /// Panics if `candidates` is empty.
    pub(crate) fn conditional_group(candidates: impl IntoIterator<Item = Ir>) -> Ir {
        let cands: Vec<Ir> = candidates.into_iter().collect();
        assert!(
            !cands.is_empty(),
            "Ir::conditional_group requires at least one candidate"
        );
        Ir::ConditionalGroup(cands.into())
    }

    /// An ordered list of candidate layouts selected by all-lines-fit; see
    /// [`Ir::ConditionalGroupAllLines`]. Panics if `candidates` is empty.
    pub(crate) fn conditional_group_all_lines(candidates: impl IntoIterator<Item = Ir>) -> Ir {
        let cands: Vec<Ir> = candidates.into_iter().collect();
        assert!(
            !cands.is_empty(),
            "Ir::conditional_group_all_lines requires at least one candidate"
        );
        Ir::ConditionalGroupAllLines(cands.into())
    }

    /// Build an [`Ir::Fill`] from a sequence of content `atoms`, interleaving an
    /// [`Ir::Line`] separator between consecutive atoms (so the printer may break
    /// at any gap). `Nil` atoms are dropped. Zero atoms → [`Ir::Nil`]; one atom →
    /// that atom (no fill needed).
    pub(crate) fn fill(atoms: impl IntoIterator<Item = Ir>) -> Ir {
        let atoms: Vec<Ir> = atoms
            .into_iter()
            .filter(|i| !matches!(i, Ir::Nil))
            .collect();
        match atoms.len() {
            0 => Ir::Nil,
            1 => atoms.into_iter().next().unwrap(),
            _ => {
                let mut parts = Vec::with_capacity(atoms.len() * 2 - 1);
                for (i, atom) in atoms.into_iter().enumerate() {
                    if i > 0 {
                        parts.push(Ir::Line);
                    }
                    parts.push(atom);
                }
                Ir::Fill(parts.into())
            }
        }
    }

    /// Build a source-break-aware paragraph fill. `preferred[i]` describes the
    /// gap between atoms `i` and `i + 1`.
    pub(crate) fn preferred_fill(
        atoms: impl IntoIterator<Item = Ir>,
        preferred: Vec<bool>,
        target: usize,
    ) -> Ir {
        // Pair each atom with the "authored break before me" flag (the first
        // atom has none), then drop `Ir::Nil` atoms together with their flag so
        // the gap mask stays aligned with the surviving atoms. Filtering `atoms`
        // alone would leave `preferred` too long, misindexing every downstream
        // gap (and tripping the debug assertions in `stable_breaks`).
        let mut flags = preferred.into_iter();
        let surviving: Vec<(Ir, bool)> = atoms
            .into_iter()
            .enumerate()
            .map(|(i, ir)| {
                let before = if i == 0 {
                    false
                } else {
                    flags.next().unwrap_or(false)
                };
                (ir, before)
            })
            .filter(|(ir, _)| !matches!(ir, Ir::Nil))
            .collect();
        let mut atoms: Vec<Ir> = Vec::with_capacity(surviving.len());
        let mut preferred: Vec<bool> = Vec::with_capacity(surviving.len().saturating_sub(1));
        for (i, (ir, before)) in surviving.into_iter().enumerate() {
            if i > 0 {
                preferred.push(before);
            }
            atoms.push(ir);
        }
        debug_assert_eq!(preferred.len(), atoms.len().saturating_sub(1));
        match atoms.len() {
            0 => Ir::Nil,
            1 => atoms.into_iter().next().unwrap(),
            _ => Ir::PreferredFill {
                atoms: atoms.into(),
                preferred: preferred.into(),
                target,
            },
        }
    }

    pub(crate) fn indent(inner: Ir) -> Ir {
        Ir::Indent(Rc::new(inner))
    }

    /// A hanging indent of `width` columns (see [`Ir::Align`]). A zero width or a
    /// [`Ir::Nil`] body degenerates to the body itself.
    pub(crate) fn align(width: usize, inner: Ir) -> Ir {
        if width == 0 || matches!(inner, Ir::Nil) {
            return inner;
        }
        Ir::Align(width, Rc::new(inner))
    }

    /// A hanging indent anchored at the current rendered column (see
    /// [`Ir::AlignCurrent`]).
    pub(crate) fn align_current(inner: Ir) -> Ir {
        if matches!(inner, Ir::Nil) {
            return inner;
        }
        Ir::AlignCurrent(Rc::new(inner))
    }

    /// Choose `aligned` only when its broken continuation lines fit at the
    /// current column; see [`Ir::BoundedAlign`].
    pub(crate) fn bounded_align(aligned: Ir, fallback: Ir) -> Ir {
        Ir::BoundedAlign {
            aligned: Rc::new(aligned),
            fallback: Rc::new(fallback),
        }
    }

    pub(crate) fn if_break(flat: Ir, broken: Ir) -> Ir {
        Ir::IfBreak {
            flat: Rc::new(flat),
            broken: Rc::new(broken),
        }
    }

    /// A bridged/inline verbatim chunk. It forces a break only if it spans
    /// multiple lines (i.e. its own layout cannot be collapsed).
    pub(crate) fn verbatim(s: impl Into<Rc<str>>) -> Ir {
        let text: Rc<str> = s.into();
        let force_break = text.contains('\n');
        Ir::Verbatim { text, force_break }
    }

    /// A verbatim chunk that always forces the enclosing group to break,
    /// regardless of whether it spans multiple lines (e.g. a comment).
    pub(crate) fn verbatim_forced(s: impl Into<Rc<str>>) -> Ir {
        Ir::Verbatim {
            text: s.into(),
            force_break: true,
        }
    }

    /// A verbatim chunk pinned to column 0; see [`Ir::ColumnZero`]. `text` must
    /// never contain a newline (a margin/guard is a single line-leading token).
    pub(crate) fn column_zero(s: impl Into<Rc<str>>) -> Ir {
        Ir::ColumnZero(s.into())
    }

    /// A zero-width trailing chunk (a trailing comment); see [`Ir::ZeroWidth`].
    /// `text` must never contain a newline.
    pub(crate) fn zero_width(s: impl Into<Rc<str>>) -> Ir {
        Ir::ZeroWidth(s.into())
    }

    /// Re-emit `prefix` at column 0 on every line `inner` produces; see
    /// [`Ir::MarginPrefix`]. A [`Ir::Nil`] body degenerates to `Nil`.
    pub(crate) fn margin_prefix(prefix: impl Into<Rc<str>>, inner: Ir) -> Ir {
        if matches!(inner, Ir::Nil) {
            return Ir::Nil;
        }
        Ir::MarginPrefix {
            prefix: prefix.into(),
            blank_prefix: None,
            inner: Rc::new(inner),
        }
    }

    /// Re-emit the canonical `.dtx` documentation margin on every generated
    /// line, using a bare `%` for a virtual blank line.
    pub(crate) fn doc_margin(inner: Ir) -> Ir {
        if matches!(inner, Ir::Nil) {
            return Ir::Nil;
        }
        Ir::MarginPrefix {
            prefix: "% ".into(),
            blank_prefix: Some("%".into()),
            inner: Rc::new(inner),
        }
    }

    pub(crate) fn line() -> Ir {
        Ir::Line
    }

    pub(crate) fn soft_line() -> Ir {
        Ir::SoftLine
    }

    pub(crate) fn hard_line() -> Ir {
        Ir::HardLine
    }

    pub(crate) fn empty_line() -> Ir {
        Ir::EmptyLine
    }

    pub(crate) fn nil() -> Ir {
        Ir::Nil
    }

    /// Whether this tree contains an *unconditional* forced line break: a
    /// `HardLine`/`EmptyLine`, a force-break `Verbatim` (e.g. a comment), or an
    /// `expand` group. Conditional breaks (`IfBreak` branches, `SoftLine`,
    /// `Line`) do not count, since they only break when an enclosing group does.
    /// Used to detect, e.g., a non-empty block argument that should force its
    /// arg list open.
    pub(crate) fn contains_forced_break(&self) -> bool {
        match self {
            Ir::HardLine | Ir::EmptyLine => true,
            Ir::Verbatim { force_break, .. } => *force_break,
            Ir::Concat(items) => items.iter().any(Ir::contains_forced_break),
            // A fill's separators are soft `Line`s; only its atoms could carry a
            // forced break (none do under reflow lowering, but stay correct).
            Ir::Fill(parts) | Ir::StickyFill(parts) | Ir::HugFill(parts) => {
                parts.iter().any(Ir::contains_forced_break)
            }
            Ir::PreferredFill { atoms, .. } => atoms.iter().any(Ir::contains_forced_break),
            Ir::Indent(inner) | Ir::Align(_, inner) | Ir::AlignCurrent(inner) => {
                inner.contains_forced_break()
            }
            // Flat mode always chooses the aligned branch, so only that branch
            // can force an enclosing group open. The fallback is selected only
            // after the enclosing layout is already in break mode.
            Ir::BoundedAlign { aligned, .. } => aligned.contains_forced_break(),
            Ir::MarginPrefix { inner, .. } => inner.contains_forced_break(),
            Ir::Group { forced_break, .. } => *forced_break,
            // The flat-most candidate decides: if even it forces a break, the
            // conditional group always breaks; otherwise some layout is flat-able.
            Ir::ConditionalGroup(cands) | Ir::ConditionalGroupAllLines(cands) => {
                cands.first().is_some_and(Ir::contains_forced_break)
            }
            Ir::Text(_)
            | Ir::ColumnZero(_)
            | Ir::ZeroWidth(_)
            | Ir::Line
            | Ir::SoftLine
            | Ir::IfBreak { .. }
            | Ir::Nil => false,
        }
    }

    /// Post-lowering prepass: saturate `Group::expand` so it becomes the single
    /// representation of "this subtree is forced open" that the printer trusts.
    /// Marks every non-hug group whose inner contains an unconditional forced
    /// break, with the same semantics as [`Ir::contains_forced_break`] (an
    /// `IfBreak` shields its branches; a conditional group's flat-most
    /// candidate decides). Hug groups are never marked: their inner holds a
    /// forced break by construction and their break decision is the hug fit.
    /// Copy-on-write — unchanged subtrees are shared with `self`.
    pub(crate) fn propagate_breaks(&self) -> Ir {
        match saturate(self).1 {
            Some(rewritten) => {
                debug_assert!(
                    saturate(&rewritten).1.is_none(),
                    "propagate_breaks must reach a fixed point in one pass"
                );
                rewritten
            }
            None => self.clone(),
        }
    }
}

/// One bottom-up walk of [`Ir::propagate_breaks`]. Returns `(forced,
/// rewritten)`: `forced` is [`Ir::contains_forced_break`] of the *saturated*
/// node (computed on the way up, never by re-traversal, so the pass stays
/// O(n)); `rewritten` is `None` when the subtree is unchanged, letting the
/// caller share the existing `Rc`.
fn saturate(ir: &Ir) -> (bool, Option<Ir>) {
    match ir {
        Ir::HardLine | Ir::EmptyLine => (true, None),
        Ir::Verbatim { force_break, .. } => (*force_break, None),
        Ir::Text(_) | Ir::ColumnZero(_) | Ir::ZeroWidth(_) | Ir::Line | Ir::SoftLine | Ir::Nil => {
            (false, None)
        }
        Ir::Concat(items) => {
            let (forced, rewritten) = saturate_slice(items);
            (forced.any, rewritten.map(Ir::Concat))
        }
        Ir::Fill(parts) => {
            let (forced, rewritten) = saturate_slice(parts);
            (forced.any, rewritten.map(Ir::Fill))
        }
        Ir::StickyFill(parts) => {
            let (forced, rewritten) = saturate_slice(parts);
            (forced.any, rewritten.map(Ir::StickyFill))
        }
        Ir::HugFill(parts) => {
            let (forced, rewritten) = saturate_slice(parts);
            (forced.any, rewritten.map(Ir::HugFill))
        }
        Ir::PreferredFill {
            atoms,
            preferred,
            target,
        } => {
            let (forced, rewritten) = saturate_slice(atoms);
            (
                forced.any,
                rewritten.map(|atoms| Ir::PreferredFill {
                    atoms,
                    preferred: preferred.clone(),
                    target: *target,
                }),
            )
        }
        Ir::Indent(inner) => {
            let (forced, rewritten) = saturate(inner);
            (forced, rewritten.map(|ir| Ir::Indent(Rc::new(ir))))
        }
        Ir::Align(width, inner) => {
            let (forced, rewritten) = saturate(inner);
            (forced, rewritten.map(|ir| Ir::Align(*width, Rc::new(ir))))
        }
        Ir::AlignCurrent(inner) => {
            let (forced, rewritten) = saturate(inner);
            (forced, rewritten.map(|ir| Ir::AlignCurrent(Rc::new(ir))))
        }
        Ir::BoundedAlign { aligned, fallback } => {
            let (forced, aligned_rw) = saturate(aligned);
            let (_, fallback_rw) = saturate(fallback);
            if aligned_rw.is_none() && fallback_rw.is_none() {
                (forced, None)
            } else {
                (
                    forced,
                    Some(Ir::BoundedAlign {
                        aligned: aligned_rw.map(Rc::new).unwrap_or_else(|| aligned.clone()),
                        fallback: fallback_rw.map(Rc::new).unwrap_or_else(|| fallback.clone()),
                    }),
                )
            }
        }
        Ir::MarginPrefix {
            prefix,
            blank_prefix,
            inner,
        } => {
            let (forced, rewritten) = saturate(inner);
            (
                forced,
                rewritten.map(|ir| Ir::MarginPrefix {
                    prefix: prefix.clone(),
                    blank_prefix: blank_prefix.clone(),
                    inner: Rc::new(ir),
                }),
            )
        }
        // Both branches are saturated (groups inside them must be marked), but
        // a conditional break never forces the parent.
        Ir::IfBreak { flat, broken } => {
            let (_, flat_rw) = saturate(flat);
            let (_, broken_rw) = saturate(broken);
            if flat_rw.is_none() && broken_rw.is_none() {
                (false, None)
            } else {
                (
                    false,
                    Some(Ir::IfBreak {
                        flat: flat_rw.map(Rc::new).unwrap_or_else(|| flat.clone()),
                        broken: broken_rw.map(Rc::new).unwrap_or_else(|| broken.clone()),
                    }),
                )
            }
        }
        Ir::Group {
            inner,
            expand,
            hug,
            forced_break,
            hug_excuse_overflow,
        } => {
            let (inner_forced, rewritten) = saturate(inner);
            debug_assert_eq!(*forced_break, *expand || inner_forced);
            let new_expand = if *hug {
                *expand
            } else {
                *expand || inner_forced
            };
            let forced = *expand || inner_forced;
            if rewritten.is_none() && new_expand == *expand {
                (forced, None)
            } else {
                (
                    forced,
                    Some(Ir::Group {
                        inner: rewritten.map(Rc::new).unwrap_or_else(|| inner.clone()),
                        expand: new_expand,
                        hug: *hug,
                        forced_break: *forced_break,
                        hug_excuse_overflow: *hug_excuse_overflow,
                    }),
                )
            }
        }
        // Every candidate is saturated (the printer may pick any), but only
        // the flat-most candidate decides the parent's forcedness.
        Ir::ConditionalGroup(cands) => {
            let (forced, rewritten) = saturate_slice(cands);
            (forced.first, rewritten.map(Ir::ConditionalGroup))
        }
        Ir::ConditionalGroupAllLines(cands) => {
            let (forced, rewritten) = saturate_slice(cands);
            (forced.first, rewritten.map(Ir::ConditionalGroupAllLines))
        }
    }
}

/// The forcedness of a saturated slice: `any` child forced (a concat/fill
/// forces when any part does) and `first` child forced (a conditional group's
/// flat-most candidate decides).
struct SliceForced {
    any: bool,
    first: bool,
}

/// [`saturate`] over a slice; `rewritten` is built only when some child
/// changed (unchanged children are cloned cheaply — composites hold `Rc`s).
fn saturate_slice(items: &[Ir]) -> (SliceForced, Option<Rc<[Ir]>>) {
    let mut forced = SliceForced {
        any: false,
        first: false,
    };
    let mut rewritten: Option<Vec<Ir>> = None;
    for (i, item) in items.iter().enumerate() {
        let (f, rw) = saturate(item);
        forced.any |= f;
        if i == 0 {
            forced.first = f;
        }
        match rw {
            Some(new) => {
                rewritten
                    .get_or_insert_with(|| items[..i].to_vec())
                    .push(new);
            }
            None => {
                if let Some(vec) = rewritten.as_mut() {
                    vec.push(item.clone());
                }
            }
        }
    }
    (forced, rewritten.map(Into::into))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The saturation invariant: after the pass, every non-hug group's
    /// `expand` agrees with [`Ir::contains_forced_break`] of its inner, and no
    /// hug group is marked — everywhere, including `IfBreak` branches and
    /// every conditional-group candidate.
    fn assert_saturated(ir: &Ir) {
        if let Ir::Group {
            inner,
            expand,
            hug,
            forced_break,
            ..
        } = ir
        {
            assert_eq!(*forced_break, *expand || inner.contains_forced_break());
            if *hug {
                assert!(!expand, "hug group must never be marked: {ir:?}");
            } else {
                assert_eq!(
                    *expand,
                    inner.contains_forced_break(),
                    "unsaturated group: {ir:?}"
                );
            }
        }
        match ir {
            Ir::Concat(items)
            | Ir::Fill(items)
            | Ir::StickyFill(items)
            | Ir::HugFill(items)
            | Ir::ConditionalGroup(items)
            | Ir::ConditionalGroupAllLines(items) => items.iter().for_each(assert_saturated),
            Ir::PreferredFill { atoms, .. } => atoms.iter().for_each(assert_saturated),
            Ir::Indent(inner)
            | Ir::Align(_, inner)
            | Ir::AlignCurrent(inner)
            | Ir::Group { inner, .. }
            | Ir::MarginPrefix { inner, .. } => assert_saturated(inner),
            Ir::BoundedAlign {
                aligned, fallback, ..
            } => {
                assert_saturated(aligned);
                assert_saturated(fallback);
            }
            Ir::IfBreak { flat, broken } => {
                assert_saturated(flat);
                assert_saturated(broken);
            }
            Ir::Text(_)
            | Ir::Verbatim { .. }
            | Ir::ColumnZero(_)
            | Ir::ZeroWidth(_)
            | Ir::HardLine
            | Ir::EmptyLine
            | Ir::Line
            | Ir::SoftLine
            | Ir::Nil => {}
        }
    }

    fn expand_of(ir: &Ir) -> bool {
        match ir {
            Ir::Group { expand, .. } => *expand,
            other => panic!("expected a group, got {other:?}"),
        }
    }

    #[test]
    fn marks_a_group_containing_a_hard_line() {
        let out = Ir::group(Ir::concat([Ir::text("{"), Ir::hard_line(), Ir::text("}")]))
            .propagate_breaks();
        assert!(expand_of(&out));
        assert_saturated(&out);
    }

    #[test]
    fn marks_a_group_containing_a_single_line_comment() {
        let out =
            Ir::group(Ir::concat([Ir::text("a"), Ir::verbatim_forced("% c")])).propagate_breaks();
        assert!(expand_of(&out));
        assert_saturated(&out);
    }

    #[test]
    fn leaves_a_soft_group_unmarked_and_shares_the_subtree() {
        let g = Ir::group(Ir::concat([Ir::text("a"), Ir::Line, Ir::text("b")]));
        let out = g.propagate_breaks();
        assert!(!expand_of(&out));
        let (Ir::Group { inner: before, .. }, Ir::Group { inner: after, .. }) = (&g, &out) else {
            unreachable!()
        };
        assert!(
            Rc::ptr_eq(before, after),
            "unchanged subtree must be shared, not rebuilt"
        );
    }

    #[test]
    fn marks_nested_groups_at_every_level() {
        let out = Ir::group(Ir::concat([Ir::text("head"), Ir::group(Ir::hard_line())]))
            .propagate_breaks();
        assert!(expand_of(&out));
        let Ir::Group { inner, .. } = &out else {
            unreachable!()
        };
        let Ir::Concat(items) = inner.as_ref() else {
            unreachable!()
        };
        assert!(expand_of(&items[1]));
        assert_saturated(&out);
    }

    #[test]
    fn if_break_shields_the_parent_but_not_groups_inside() {
        // A hard break in the broken branch never forces the parent…
        let out = Ir::group(Ir::if_break(Ir::text("a"), Ir::hard_line())).propagate_breaks();
        assert!(!expand_of(&out));
        assert_saturated(&out);
        // …but a group *inside* a branch is still saturated.
        let out =
            Ir::group(Ir::if_break(Ir::text("a"), Ir::group(Ir::hard_line()))).propagate_breaks();
        assert!(!expand_of(&out));
        let Ir::Group { inner, .. } = &out else {
            unreachable!()
        };
        let Ir::IfBreak { broken, .. } = inner.as_ref() else {
            unreachable!()
        };
        assert!(expand_of(broken));
        assert_saturated(&out);
    }

    #[test]
    fn conditional_group_forces_by_its_flat_most_candidate_only() {
        // Soft first candidate: the enclosing group stays soft even though the
        // second candidate carries a hard break…
        let out = Ir::group(Ir::conditional_group([
            Ir::text("flat"),
            Ir::concat([Ir::text("{"), Ir::hard_line(), Ir::text("}")]),
        ]))
        .propagate_breaks();
        assert!(!expand_of(&out));
        assert_saturated(&out);
        // …but a group inside a non-first candidate is still marked.
        let out = Ir::group(Ir::conditional_group([
            Ir::text("flat"),
            Ir::group(Ir::hard_line()),
        ]))
        .propagate_breaks();
        assert!(!expand_of(&out));
        let Ir::Group { inner, .. } = &out else {
            unreachable!()
        };
        let Ir::ConditionalGroup(cands) = inner.as_ref() else {
            unreachable!()
        };
        assert!(expand_of(&cands[1]));
        assert_saturated(&out);
        // A forced first candidate forces the enclosing group.
        let out = Ir::group(Ir::conditional_group([
            Ir::concat([Ir::text("{"), Ir::hard_line(), Ir::text("}")]),
            Ir::text("never"),
        ]))
        .propagate_breaks();
        assert!(expand_of(&out));
        assert_saturated(&out);
    }

    #[test]
    fn hug_groups_are_never_marked_but_their_contents_are() {
        let out = Ir::group_hug(Ir::concat([
            Ir::text("head"),
            Ir::Line,
            Ir::group(Ir::concat([Ir::text("{"), Ir::hard_line(), Ir::text("}")])),
        ]))
        .propagate_breaks();
        assert!(!expand_of(&out));
        let Ir::Group { inner, hug, .. } = &out else {
            unreachable!()
        };
        assert!(hug);
        let Ir::Concat(items) = inner.as_ref() else {
            unreachable!()
        };
        assert!(expand_of(&items[2]));
        assert_saturated(&out);
    }

    #[test]
    fn a_marked_group_forces_its_ancestors() {
        // The forced bit computed on the way up matches the query: an outer
        // group is marked because its inner *group* is, not by re-finding the
        // hard line.
        let out = Ir::group(Ir::group(Ir::group(Ir::hard_line()))).propagate_breaks();
        assert!(expand_of(&out));
        assert_saturated(&out);
    }

    #[test]
    fn the_pass_is_idempotent() {
        let ir = Ir::group(Ir::concat([
            Ir::text("head"),
            Ir::group_hug(Ir::concat([Ir::text("x"), Ir::group(Ir::hard_line())])),
            Ir::if_break(Ir::text("a"), Ir::group(Ir::empty_line())),
            Ir::conditional_group([Ir::text("flat"), Ir::group(Ir::hard_line())]),
        ]));
        let once = ir.propagate_breaks();
        let twice = once.propagate_breaks();
        assert_eq!(format!("{once:?}"), format!("{twice:?}"));
        assert_saturated(&once);
    }

    #[test]
    fn saturation_holds_over_a_zoo_of_shapes() {
        let zoo = [
            Ir::fill([Ir::text("a"), Ir::group(Ir::hard_line()), Ir::text("b")]),
            Ir::StickyFill(vec![Ir::group(Ir::verbatim_forced("% c")), Ir::Line].into()),
            Ir::preferred_fill(
                [Ir::text("w"), Ir::group(Ir::empty_line())],
                vec![false],
                72,
            ),
            Ir::indent(Ir::align(4, Ir::group(Ir::hard_line()))),
            Ir::margin_prefix("% ", Ir::group(Ir::hard_line())),
            Ir::group(Ir::verbatim("a\nb")),
            Ir::group(Ir::concat([Ir::zero_width("% t"), Ir::column_zero("%<a>")])),
            Ir::conditional_group_all_lines([Ir::group(Ir::hard_line()), Ir::text("b")]),
        ];
        for ir in zoo {
            assert_saturated(&ir.propagate_breaks());
        }
    }
}
