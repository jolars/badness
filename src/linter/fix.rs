//! Fix application: turn a set of [`Fix`] edits into rewritten source text.
//!
//! Shared by `lint --fix` (CLI) and the LSP code-action path. The engine is a
//! pure function over `(source, fixes, include_unsafe)`; it never reads or
//! writes files.

use super::diagnostic::{Applicability, Fix};

/// Result of applying a batch of fixes to a source string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixOutcome {
    /// The rewritten source.
    pub output: String,
    /// Number of fixes applied.
    pub applied: usize,
    /// Number of fixes dropped because they were malformed or overlapped an
    /// already-applied fix.
    pub skipped_conflicts: usize,
}

/// Apply `fixes` to `source`, returning the rewritten text.
///
/// `Unsafe` fixes are skipped unless `include_unsafe` is set. Remaining fixes
/// are considered in source order (by their first edit) and applied
/// **atomically**: a fix lands with all of its edits or not at all. A fix is
/// dropped (counted in [`FixOutcome::skipped_conflicts`]) when it is malformed
/// (no edits, a backwards or out-of-bounds span, or internally overlapping
/// edits) or when any of its edits overlaps an edit of an already-accepted fix,
/// so the output stays well-formed. Adjacent edits do not conflict. Accepted
/// edits are spliced right-to-left so earlier byte offsets remain valid as
/// later ones are rewritten.
pub fn apply_fixes(source: &str, fixes: &[Fix], include_unsafe: bool) -> FixOutcome {
    // Eligible fixes, sorted by their leading edit (then overall extent) so the
    // first-wins conflict policy is deterministic and position-ordered.
    let mut eligible: Vec<(usize, usize, &Fix)> = fixes
        .iter()
        .filter(|f| include_unsafe || f.applicability == Applicability::Safe)
        .map(|f| {
            let start = f.edits.iter().map(|e| e.start).min().unwrap_or(0);
            let end = f.edits.iter().map(|e| e.end).max().unwrap_or(0);
            (start, end, f)
        })
        .collect();
    eligible.sort_by_key(|&(start, end, _)| (start, end));

    // Byte ranges claimed by accepted fixes: sorted by start, mutually disjoint
    // (adjacency allowed), so each candidate edit needs one partition_point probe.
    let mut occupied: Vec<(usize, usize)> = Vec::new();
    let mut accepted: Vec<&super::diagnostic::Edit> = Vec::new();
    let mut applied = 0usize;
    let mut skipped_conflicts = 0usize;
    for (_, _, fix) in eligible {
        let mut edits: Vec<_> = fix.edits.iter().collect();
        edits.sort_by_key(|e| (e.start, e.end));
        // Drop malformed fixes defensively: empty, backwards or out-of-bounds
        // spans, or edits that overlap each other.
        let malformed = edits.is_empty()
            || edits
                .iter()
                .any(|e| e.start > e.end || e.end > source.len())
            || edits.windows(2).any(|pair| pair[1].start < pair[0].end);
        if malformed || edits.iter().any(|e| overlaps(&occupied, e.start, e.end)) {
            skipped_conflicts += 1;
            continue;
        }
        for edit in edits {
            let at = occupied.partition_point(|&(_, end)| end <= edit.start);
            occupied.insert(at, (edit.start, edit.end));
            accepted.push(edit);
        }
        applied += 1;
    }

    // Apply right-to-left so each splice leaves earlier offsets untouched.
    accepted.sort_by_key(|e| (e.start, e.end));
    let mut output = source.to_string();
    for edit in accepted.iter().rev() {
        output.replace_range(edit.start..edit.end, &edit.content);
    }

    FixOutcome {
        output,
        applied,
        skipped_conflicts,
    }
}

/// Whether `start..end` overlaps any claimed range. Ranges merely touching at a
/// boundary (including zero-width inserts at an endpoint) do not overlap.
fn overlaps(occupied: &[(usize, usize)], start: usize, end: usize) -> bool {
    // Claimed ranges are disjoint and sorted, so their `end`s are monotone: the
    // first range not entirely before `start` is the only overlap candidate.
    let at = occupied.partition_point(|&(_, e)| e <= start);
    occupied.get(at).is_some_and(|&(s, _)| s < end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linter::diagnostic::Edit;

    fn safe(start: usize, end: usize, content: &str) -> Fix {
        Fix::safe(start, end, content, "test")
    }

    #[test]
    fn applies_single_fix() {
        let out = apply_fixes("$$x$$", &[safe(0, 2, "\\[")], false);
        assert_eq!(out.output, "\\[x$$");
        assert_eq!(out.applied, 1);
        assert_eq!(out.skipped_conflicts, 0);
    }

    #[test]
    fn applies_multiple_fixes_right_to_left() {
        // Two non-overlapping replacements; the earlier one must not be shifted
        // by the later one.
        let out = apply_fixes("$$x$$", &[safe(0, 2, "\\["), safe(3, 5, "\\]")], false);
        assert_eq!(out.output, "\\[x\\]");
        assert_eq!(out.applied, 2);
    }

    #[test]
    fn skips_unsafe_unless_opted_in() {
        let fixes = [Fix::unsafe_(0, 6, "", "delete")];
        let kept = apply_fixes("abcdef", &fixes, false);
        assert_eq!(kept.output, "abcdef");
        assert_eq!(kept.applied, 0);

        let applied = apply_fixes("abcdef", &fixes, true);
        assert_eq!(applied.output, "");
        assert_eq!(applied.applied, 1);
    }

    #[test]
    fn drops_overlapping_fixes() {
        let out = apply_fixes("abcdef", &[safe(0, 3, "X"), safe(2, 5, "Y")], false);
        // First wins; second overlaps and is dropped.
        assert_eq!(out.output, "Xdef");
        assert_eq!(out.applied, 1);
        assert_eq!(out.skipped_conflicts, 1);
    }

    #[test]
    fn adjacent_fixes_do_not_conflict() {
        let out = apply_fixes("abcd", &[safe(0, 2, "X"), safe(2, 4, "Y")], false);
        assert_eq!(out.output, "XY");
        assert_eq!(out.applied, 2);
        assert_eq!(out.skipped_conflicts, 0);
    }

    #[test]
    fn applies_multi_edit_fix() {
        // A delimiter swap at both ends, body untouched.
        let fix = Fix::safe_edits(vec![Edit::new(0, 2, "\\["), Edit::new(3, 5, "\\]")], "swap");
        let out = apply_fixes("$$x$$", &[fix], false);
        assert_eq!(out.output, "\\[x\\]");
        assert_eq!(out.applied, 1);
        assert_eq!(out.skipped_conflicts, 0);
    }

    #[test]
    fn multi_edit_fix_is_atomic_on_conflict() {
        // The single-edit fix claims 0..2 first (it sorts earlier than the
        // multi-edit fix's leading edit at 1); the multi-edit fix's first edit
        // then conflicts, so *neither* of its edits applies — not even the
        // non-conflicting one at 6..8.
        let single = safe(0, 2, "Z");
        let multi = Fix::safe_edits(vec![Edit::new(1, 3, "X"), Edit::new(6, 8, "Y")], "pair");
        let out = apply_fixes("abcdefgh", &[multi, single], false);
        assert_eq!(out.output, "Zcdefgh");
        assert_eq!(out.applied, 1);
        assert_eq!(out.skipped_conflicts, 1);
    }

    #[test]
    fn multi_edit_fix_conflicts_detected_out_of_order() {
        // An accepted fix claims two disjoint ranges; a later fix landing
        // *between* them applies, one overlapping the trailing claim does not.
        let pair = Fix::safe_edits(vec![Edit::new(0, 2, "A"), Edit::new(6, 8, "B")], "pair");
        let between = safe(3, 4, "-");
        let clash = safe(7, 9, "!");
        let out = apply_fixes("abcdefghij", &[pair, between, clash], false);
        assert_eq!(out.output, "Ac-efBij");
        assert_eq!(out.applied, 2);
        assert_eq!(out.skipped_conflicts, 1);
    }

    #[test]
    fn drops_internally_overlapping_fix() {
        let bad = Fix::safe_edits(
            vec![Edit::new(0, 3, "X"), Edit::new(2, 5, "Y")],
            "malformed",
        );
        let out = apply_fixes("abcdef", &[bad], false);
        assert_eq!(out.output, "abcdef");
        assert_eq!(out.applied, 0);
        assert_eq!(out.skipped_conflicts, 1);
    }

    #[test]
    fn drops_out_of_bounds_fix() {
        let out = apply_fixes("abc", &[safe(1, 9, "X")], false);
        assert_eq!(out.output, "abc");
        assert_eq!(out.applied, 0);
        assert_eq!(out.skipped_conflicts, 1);
    }
}
