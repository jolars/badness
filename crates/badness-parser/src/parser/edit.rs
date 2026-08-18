//! Byte-range text edits.
//!
//! [`Edit`] is the parser's one edit currency: a byte range in some old text plus
//! the string that replaces it. Everything here is pure text manipulation with no
//! parser content; [`super::reparse`] re-exports it so `parser::Edit` is the single
//! path the parser layer uses. Converting LSP `didChange` content changes into
//! these lives host-side (`crate::lsp` in the root crate), which keeps this crate
//! free of protocol dependencies and wasm-clean.
//!
//! Edits reaching the reparse are **untrusted**. A language server can hand over a
//! chain staged against a buffer that has since moved, and slicing on a stale range
//! is a panic in an analysis query rather than a wrong answer. So every consumer
//! validates before it slices: [`try_apply_edits`] is the apply-and-verify guard,
//! and reconstructing the current buffer from an old snapshot plus a chain is what
//! proves the chain is the exact transform between them.

use std::ops::Range;

/// A single contiguous text edit: replace `range` (a byte range in the *old* text)
/// with `insert`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub insert: String,
}

impl Edit {
    /// The signed length change this edit applies to text after `range`.
    ///
    /// Diagnostic offsets at or after the edit shift by exactly this much, which is
    /// what lets a tier keep the errors it did not regenerate.
    pub fn delta(&self) -> isize {
        self.insert.len() as isize - (self.range.end - self.range.start) as isize
    }

    /// Whether this edit fits `text`: in bounds, non-inverted, and both offsets on
    /// char boundaries. Check before slicing — the edit is untrusted.
    pub fn fits(&self, text: &str) -> bool {
        self.range.start <= self.range.end
            && self.range.end <= text.len()
            && text.is_char_boundary(self.range.start)
            && text.is_char_boundary(self.range.end)
    }

    /// Apply the edit to `old`, producing the new text.
    ///
    /// # Panics
    ///
    /// If the edit does not [fit](Self::fits) `old`.
    pub fn apply(&self, old: &str) -> String {
        let mut out =
            String::with_capacity(old.len().saturating_sub(self.range.len()) + self.insert.len());
        out.push_str(&old[..self.range.start]);
        out.push_str(&self.insert);
        out.push_str(&old[self.range.end..]);
        out
    }
}

/// Apply `edits` to `old` left-to-right, each expressed against the text its
/// predecessors produced — the shape an LSP `didChange` batch arrives in.
///
/// # Panics
///
/// If any edit does not fit the text its predecessors produced. Use
/// [`try_apply_edits`] for a chain of unproven provenance.
pub fn apply_edits(old: &str, edits: &[Edit]) -> String {
    try_apply_edits(old, edits).expect("apply_edits: edit chain does not fit the text")
}

/// [`apply_edits`] for an edit chain of unproven provenance: [`None`] when any edit
/// does not fit the text its predecessors produced.
///
/// Folds in place, so peak memory is one text rather than one per edit.
pub fn try_apply_edits(old: &str, edits: &[Edit]) -> Option<String> {
    let mut text = old.to_string();
    for e in edits {
        if !e.fits(&text) {
            return None;
        }
        text.replace_range(e.range.clone(), &e.insert);
    }
    Some(text)
}

/// Recover a single contiguous [`Edit`] from a pair of whole texts by stripping the
/// common prefix and suffix.
///
/// This is the **fallback**, not the hot path. The language server knows the exact
/// range it spliced and must hand it over; re-deriving it here costs more than the
/// reparse it feeds (fatou measured ~200 us of a ~500 us keystroke at 1 MB). It
/// stays for texts that changed by a route carrying no edits — a disk reload, a
/// whole-buffer replacement, a chain that failed to verify.
///
/// Multiple disjoint edits collapse into one spanning edit. Still a correct
/// transform, just coarser — and a coarse one spans everything between the changes,
/// which is exactly the shape a cost guard declines.
pub fn diff_edit(old: &str, new: &str) -> Edit {
    let ob = old.as_bytes();
    let nb = new.as_bytes();

    let mut prefix = 0;
    let max_prefix = ob.len().min(nb.len());
    while prefix < max_prefix && ob[prefix] == nb[prefix] {
        prefix += 1;
    }
    // Back off to a char boundary of *both* texts. They share these bytes, so one
    // test would do; testing `old` alone is the convention and `new` agrees.
    while prefix > 0 && !old.is_char_boundary(prefix) {
        prefix -= 1;
    }

    let mut suffix = 0;
    let max_suffix = (ob.len() - prefix).min(nb.len() - prefix);
    while suffix < max_suffix && ob[ob.len() - 1 - suffix] == nb[nb.len() - 1 - suffix] {
        suffix += 1;
    }
    // Here the two texts are at different offsets, so both need the test.
    while suffix > 0
        && (!old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix))
    {
        suffix -= 1;
    }

    Edit {
        range: prefix..(old.len() - suffix),
        insert: new[prefix..(new.len() - suffix)].to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(range: Range<usize>, insert: &str) -> Edit {
        Edit {
            range,
            insert: insert.to_string(),
        }
    }

    /// The property every `diff_edit` case below leans on, stated once: whatever it
    /// returns must transform `old` into `new`.
    fn assert_recovers(old: &str, new: &str) -> Edit {
        let e = diff_edit(old, new);
        assert!(e.fits(old), "{e:?} does not fit {old:?}");
        assert_eq!(e.apply(old), new, "diff_edit({old:?}, {new:?}) = {e:?}");
        e
    }

    #[test]
    fn diff_edit_recovers_a_noop() {
        assert_eq!(
            assert_recovers("\\section{Hi}\n", "\\section{Hi}\n").insert,
            ""
        );
    }

    #[test]
    fn diff_edit_recovers_an_insertion() {
        assert_eq!(assert_recovers("ab\n", "axb\n"), edit(1..1, "x"));
    }

    #[test]
    fn diff_edit_recovers_a_deletion() {
        assert_eq!(assert_recovers("axb\n", "ab\n"), edit(1..2, ""));
    }

    /// The span reaches only as far as the shared suffix allows: `\alpha` and
    /// `\gamma` share a trailing `a`, so the edit stops one char short of the end.
    #[test]
    fn diff_edit_recovers_a_replacement() {
        assert_eq!(
            assert_recovers("\\alpha\n", "\\gamma\n"),
            edit(1..5, "gamm")
        );
    }

    #[test]
    fn diff_edit_collapses_disjoint_edits_into_one_span() {
        // Two changes, one span: correct, and deliberately coarse.
        let e = assert_recovers("a x b y c\n", "a X b Y c\n");
        assert_eq!(e, edit(2..7, "X b Y"));
    }

    #[test]
    fn diff_edit_handles_whole_replacement_and_empty_texts() {
        assert_recovers("\\begin{a}\n", "\\end{b}\n");
        assert_recovers("", "\\section{x}");
        assert_recovers("\\section{x}", "");
        assert_recovers("", "");
    }

    /// A prefix that lands mid-`α` would slice a multi-byte char in half. This is
    /// not hypothetical: `\alpha` beside a literal `α` is ordinary LaTeX.
    #[test]
    fn diff_edit_clamps_to_char_boundaries() {
        let e = assert_recovers("αβ\n", "αγ\n");
        assert!("αβ\n".is_char_boundary(e.range.start));
        assert!("αβ\n".is_char_boundary(e.range.end));
    }

    #[test]
    fn diff_edit_clamps_a_shared_suffix_that_splits_a_char() {
        assert_recovers("xα\n", "yα\n");
        assert_recovers("α\n", "αα\n");
    }

    #[test]
    fn apply_edits_chains_left_to_right() {
        // The second range is expressed against the text the first produced.
        let edits = [edit(0..0, "\\a"), edit(2..2, "{b}")];
        assert_eq!(apply_edits("\n", &edits), "\\a{b}\n");
    }

    #[test]
    fn try_apply_edits_rejects_an_out_of_bounds_range() {
        assert_eq!(try_apply_edits("ab", &[edit(9..9, "x")]), None);
    }

    /// An inverted range is the shape a mis-ordered LSP batch produces, and it is
    /// the one `Range` case that would slice-panic rather than bounds-panic.
    #[test]
    #[allow(
        clippy::reversed_empty_ranges,
        reason = "the inverted range is the input under test"
    )]
    fn try_apply_edits_rejects_an_inverted_range() {
        assert_eq!(try_apply_edits("ab", &[edit(2..1, "x")]), None);
    }

    #[test]
    fn try_apply_edits_rejects_an_offset_inside_a_char() {
        // `α` is two bytes; offset 1 is inside it.
        assert_eq!(try_apply_edits("α", &[edit(1..1, "x")]), None);
    }

    /// A chain can go stale *mid-fold*, so the check has to run against the text
    /// each predecessor produced rather than the original — in both directions. The
    /// first chain's second edit fits the original and not the text it lands on; the
    /// second chain's fits the text it lands on and not the original.
    #[test]
    fn try_apply_edits_validates_each_step_against_its_predecessor() {
        assert_eq!(
            try_apply_edits("abc", &[edit(0..3, ""), edit(1..1, "x")]),
            None
        );
        assert_eq!(
            try_apply_edits("abc", &[edit(3..3, "de"), edit(4..5, "X")]).as_deref(),
            Some("abcdX"),
        );
    }

    #[test]
    fn delta_is_the_shift_applied_to_later_offsets() {
        assert_eq!(edit(0..0, "xy").delta(), 2);
        assert_eq!(edit(0..2, "").delta(), -2);
        assert_eq!(edit(0..2, "ab").delta(), 0);
    }
}
