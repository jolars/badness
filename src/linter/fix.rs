//! Fix application: turn a set of [`Fix`] edits into rewritten source text.
//!
//! Shared by `lint --fix` (CLI) and the LSP code-action path. Two entry points,
//! both pure (they never read or write files):
//!
//! - [`apply_fixes`] rewrites a **single** source string. It is the fast path
//!   for the overwhelmingly common single-file fix; a fix that reaches into
//!   another file (any edit with [`Edit::path`] `Some`) is skipped here, since
//!   one source cannot honor it.
//! - [`apply_fixes_multi`] rewrites a **set** of files keyed by path, so a
//!   cross-file fix lands atomically across all of them (or not at all).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::diagnostic::{Applicability, Edit, Fix};

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
///
/// A **cross-file** fix — any edit with [`Edit::path`] `Some` — is skipped here
/// (counted in `skipped_conflicts`): a single source cannot honor it. Use
/// [`apply_fixes_multi`] when the caller holds the whole file set.
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
        // spans, or edits that overlap each other. A cross-file edit (one bound
        // to another file) can't apply against this lone source, so drop the
        // whole fix here too — `apply_fixes_multi` is the path that honors it.
        let malformed = edits.is_empty()
            || edits.iter().any(|e| e.path.is_some())
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

/// Result of applying a batch of fixes across a set of files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiFixOutcome {
    /// Rewritten text, keyed by path. Contains **only** files an accepted fix
    /// actually touched; untouched inputs are absent (the caller keeps their
    /// original text).
    pub outputs: HashMap<PathBuf, String>,
    /// Number of fixes applied.
    pub applied: usize,
    /// Number of fixes dropped because they were malformed, referenced a file
    /// absent from `sources`, or overlapped an already-applied fix.
    pub skipped_conflicts: usize,
}

/// Apply cross-file `fixes` to the file set `sources`, returning the rewritten
/// text of every touched file.
///
/// This is the multi-file generalization of [`apply_fixes`]. Each fix is paired
/// with its **origin** path (the diagnostic's own file); an [`Edit`] with
/// `path` `None` resolves to that origin, `Some(p)` to `p`. Fixes are applied
/// **atomically across files**: a fix lands with all of its edits in all of
/// their files, or not at all. A fix is dropped (counted in
/// [`MultiFixOutcome::skipped_conflicts`]) when it is malformed (no edits, a
/// backwards or out-of-bounds span, an edit whose resolved file is missing from
/// `sources`, or two same-file edits that overlap each other) or when any of its
/// edits overlaps an edit of an already-accepted fix in the same file. The
/// first-wins order is deterministic: fixes sort by their lexicographically
/// smallest resolved `(path, start, end)`. Accepted edits are spliced
/// right-to-left within each file so earlier offsets stay valid.
pub fn apply_fixes_multi(
    sources: &HashMap<PathBuf, String>,
    fixes: &[(PathBuf, &Fix)],
    include_unsafe: bool,
) -> MultiFixOutcome {
    // Resolve an edit's target file against the fix's origin (`None` = origin).
    let target = |origin: &Path, e: &Edit| -> PathBuf {
        e.path.clone().unwrap_or_else(|| origin.to_path_buf())
    };

    // Eligible fixes, each tagged with a deterministic sort key: the smallest
    // resolved `(path, start, end)` among its edits, so the first-wins conflict
    // policy is stable and position-ordered across files.
    let mut eligible: Vec<(PathBuf, usize, usize, &Path, &Fix)> = fixes
        .iter()
        .filter(|(_, f)| include_unsafe || f.applicability == Applicability::Safe)
        .map(|(origin, f)| {
            let key = f
                .edits
                .iter()
                .map(|e| (target(origin, e), e.start, e.end))
                .min()
                .unwrap_or_else(|| (origin.clone(), 0, 0));
            (key.0, key.1, key.2, origin.as_path(), *f)
        })
        .collect();
    eligible.sort_by(|a, b| (&a.0, a.1, a.2).cmp(&(&b.0, b.1, b.2)));

    // Byte ranges claimed by accepted fixes, per file (each vec sorted by start,
    // mutually disjoint). Accepted edits await splicing, tagged by resolved path.
    let mut occupied: HashMap<PathBuf, Vec<(usize, usize)>> = HashMap::new();
    let mut accepted: Vec<(PathBuf, &Edit)> = Vec::new();
    let mut applied = 0usize;
    let mut skipped_conflicts = 0usize;

    for (_, _, _, origin, fix) in eligible {
        // Resolve each edit to its target file, then sort by (path, span) so the
        // same-file overlap check below only compares adjacent siblings.
        let mut resolved: Vec<(PathBuf, &Edit)> =
            fix.edits.iter().map(|e| (target(origin, e), e)).collect();
        resolved.sort_by(|a, b| (&a.0, a.1.start, a.1.end).cmp(&(&b.0, b.1.start, b.1.end)));

        let malformed = resolved.is_empty()
            || resolved.iter().any(|(p, e)| {
                sources
                    .get(p)
                    .is_none_or(|src| e.start > e.end || e.end > src.len())
            })
            || resolved
                .windows(2)
                .any(|pair| pair[0].0 == pair[1].0 && pair[1].1.start < pair[0].1.end);
        let conflicts = resolved.iter().any(|(p, e)| {
            occupied
                .get(p)
                .is_some_and(|claimed| overlaps(claimed, e.start, e.end))
        });
        if malformed || conflicts {
            skipped_conflicts += 1;
            continue;
        }
        for (p, e) in resolved {
            let claimed = occupied.entry(p.clone()).or_default();
            let at = claimed.partition_point(|&(_, end)| end <= e.start);
            claimed.insert(at, (e.start, e.end));
            accepted.push((p, e));
        }
        applied += 1;
    }

    // Splice each touched file right-to-left so earlier offsets stay valid.
    let mut by_file: HashMap<PathBuf, Vec<&Edit>> = HashMap::new();
    for (p, e) in accepted {
        by_file.entry(p).or_default().push(e);
    }
    let mut outputs = HashMap::new();
    for (path, mut edits) in by_file {
        edits.sort_by_key(|e| (e.start, e.end));
        let mut text = sources[&path].clone();
        for edit in edits.iter().rev() {
            text.replace_range(edit.start..edit.end, &edit.content);
        }
        outputs.insert(path, text);
    }

    MultiFixOutcome {
        outputs,
        applied,
        skipped_conflicts,
    }
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

    #[test]
    fn single_file_skips_cross_file_fix() {
        // A fix reaching into another file cannot apply against one source.
        let other = PathBuf::from("other.tex");
        let fix = Fix::safe_edits(
            vec![Edit::new(0, 1, "X"), Edit::in_file(other, 0, 1, "Y")],
            "cross",
        );
        let out = apply_fixes("abc", &[fix], false);
        assert_eq!(out.output, "abc");
        assert_eq!(out.applied, 0);
        assert_eq!(out.skipped_conflicts, 1);
    }

    fn sources(entries: &[(&str, &str)]) -> HashMap<PathBuf, String> {
        entries
            .iter()
            .map(|(p, t)| (PathBuf::from(p), (*t).to_string()))
            .collect()
    }

    #[test]
    fn multi_applies_across_two_files() {
        // Rename `x` -> `y` in the origin's `\label` and a `\ref` in another file.
        let a = PathBuf::from("a.tex");
        let b = PathBuf::from("b.tex");
        let src = sources(&[("a.tex", "\\label{x}"), ("b.tex", "\\ref{x}")]);
        let fix = Fix::safe_edits(
            vec![Edit::new(7, 8, "y"), Edit::in_file(b.clone(), 5, 6, "y")],
            "rename x to y",
        );
        let out = apply_fixes_multi(&src, &[(a.clone(), &fix)], false);
        assert_eq!(out.applied, 1);
        assert_eq!(out.skipped_conflicts, 0);
        assert_eq!(out.outputs[&a], "\\label{y}");
        assert_eq!(out.outputs[&b], "\\ref{y}");
    }

    #[test]
    fn multi_leaves_untouched_files_absent() {
        let a = PathBuf::from("a.tex");
        let src = sources(&[("a.tex", "abc"), ("b.tex", "def")]);
        let fix = Fix::safe_edits(vec![Edit::new(0, 1, "X")], "local");
        let out = apply_fixes_multi(&src, &[(a.clone(), &fix)], false);
        assert_eq!(out.outputs.len(), 1);
        assert_eq!(out.outputs[&a], "Xbc");
        assert!(!out.outputs.contains_key(&PathBuf::from("b.tex")));
    }

    #[test]
    fn multi_cross_file_conflict_drops_whole_fix() {
        // First fix claims a.tex 0..1; a second fix whose *other-file* edit is
        // fine but whose a.tex edit overlaps must drop entirely — atomicity spans
        // files, so b.tex is left untouched too.
        let a = PathBuf::from("a.tex");
        let b = PathBuf::from("b.tex");
        let src = sources(&[("a.tex", "abcd"), ("b.tex", "wxyz")]);
        let first = (
            a.clone(),
            Fix::safe_edits(vec![Edit::new(0, 2, "Z")], "first"),
        );
        let second = (
            a.clone(),
            Fix::safe_edits(
                vec![Edit::new(1, 3, "Q"), Edit::in_file(b.clone(), 0, 1, "!")],
                "second",
            ),
        );
        let refs = [(first.0.clone(), &first.1), (second.0.clone(), &second.1)];
        let out = apply_fixes_multi(&src, &refs, false);
        assert_eq!(out.applied, 1);
        assert_eq!(out.skipped_conflicts, 1);
        assert_eq!(out.outputs[&a], "Zcd");
        assert!(!out.outputs.contains_key(&b), "b.tex must stay untouched");
    }

    #[test]
    fn multi_drops_fix_targeting_missing_file() {
        let a = PathBuf::from("a.tex");
        let missing = PathBuf::from("gone.tex");
        let src = sources(&[("a.tex", "abc")]);
        let fix = Fix::safe_edits(
            vec![Edit::new(0, 1, "X"), Edit::in_file(missing, 0, 1, "Y")],
            "cross",
        );
        let out = apply_fixes_multi(&src, &[(a.clone(), &fix)], false);
        assert_eq!(out.applied, 0);
        assert_eq!(out.skipped_conflicts, 1);
        assert!(out.outputs.is_empty());
    }

    #[test]
    fn multi_none_edit_resolves_to_origin() {
        // A `None` edit lands in the fix's origin file, not some other file.
        let a = PathBuf::from("a.tex");
        let src = sources(&[("a.tex", "abc"), ("b.tex", "abc")]);
        let fix = Fix::safe_edits(vec![Edit::new(0, 1, "X")], "local");
        let out = apply_fixes_multi(&src, &[(a.clone(), &fix)], false);
        assert_eq!(out.outputs[&a], "Xbc");
        assert!(!out.outputs.contains_key(&PathBuf::from("b.tex")));
    }

    #[test]
    fn multi_skips_unsafe_unless_opted_in() {
        let a = PathBuf::from("a.tex");
        let src = sources(&[("a.tex", "abcdef")]);
        let fix = Fix::unsafe_edits(vec![Edit::new(0, 6, "")], "delete");
        let kept = apply_fixes_multi(&src, &[(a.clone(), &fix)], false);
        assert_eq!(kept.applied, 0);
        assert!(kept.outputs.is_empty());

        let applied = apply_fixes_multi(&src, &[(a.clone(), &fix)], true);
        assert_eq!(applied.applied, 1);
        assert_eq!(applied.outputs[&a], "");
    }
}
