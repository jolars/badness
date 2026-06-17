//! Fix application: turn a set of [`Fix`] edits into rewritten source text.
//!
//! Shared by `lint --fix` (CLI) and (later) the LSP code-action path. The engine
//! is a pure function over `(source, fixes, include_unsafe)`; it never reads or
//! writes files. Copied near-verbatim from arity's `linter/fix.rs` — keep it
//! close so the eventual shared-crate extraction stays a mechanical lift.
//! **[copy shape]**

use super::diagnostic::{Applicability, Fix};

/// Result of applying a batch of fixes to a source string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixOutcome {
    /// The rewritten source.
    pub output: String,
    /// Number of fixes applied.
    pub applied: usize,
    /// Number of fixes dropped because they overlapped an already-applied fix.
    pub skipped_conflicts: usize,
}

/// Apply `fixes` to `source`, returning the rewritten text.
///
/// `Unsafe` fixes are skipped unless `include_unsafe` is set. Remaining fixes
/// are applied in source order; any fix overlapping a previously-applied one is
/// dropped (counted in [`FixOutcome::skipped_conflicts`]) so the output stays
/// well-formed. Edits are spliced right-to-left so earlier byte offsets remain
/// valid as later ones are rewritten.
pub fn apply_fixes(source: &str, fixes: &[Fix], include_unsafe: bool) -> FixOutcome {
    // Eligible fixes, sorted by start (then end) so overlap detection is a
    // single left-to-right sweep.
    let mut eligible: Vec<&Fix> = fixes
        .iter()
        .filter(|f| include_unsafe || f.applicability == Applicability::Safe)
        .collect();
    eligible.sort_by_key(|f| (f.start, f.end));

    let mut accepted: Vec<&Fix> = Vec::with_capacity(eligible.len());
    let mut skipped_conflicts = 0usize;
    let mut last_end = 0usize;
    for fix in eligible {
        // Skip malformed or backwards spans defensively.
        if fix.start > fix.end || fix.end > source.len() {
            skipped_conflicts += 1;
            continue;
        }
        if !accepted.is_empty() && fix.start < last_end {
            skipped_conflicts += 1;
            continue;
        }
        last_end = fix.end;
        accepted.push(fix);
    }

    let applied = accepted.len();
    let mut output = source.to_string();
    // Apply right-to-left so each splice leaves earlier offsets untouched.
    for fix in accepted.iter().rev() {
        output.replace_range(fix.start..fix.end, &fix.content);
    }

    FixOutcome {
        output,
        applied,
        skipped_conflicts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
