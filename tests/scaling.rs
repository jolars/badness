//! Growth-rate guards for the paths that were quadratic in TODO.md:1120-1136.
//!
//! Each case doubles an input dimension and asserts the time grows by less than
//! its case bound. A **ratio**, not a wall-clock threshold: the machine cancels
//! out, so these need no pinned hardware and no recorded baseline, and they say
//! the one thing a benchmark cannot — that the *shape* of the cost is still
//! linear. Linear doubles (~2x); the quadratics these replaced tripled to
//! quadrupled, so the bound sits between.
//!
//! Timing in a test is inherently noisy, so each measurement is the **minimum**
//! of several runs (a min is far more stable under load than a mean: noise only
//! ever adds time) and the sizes are large enough that the asymptotic term, not
//! the fixed cost, dominates the ratio. A fixed cost would *deflate* the ratio
//! and hide a regression, which is why they are not smaller.
//!
//! Everything here is in-process. Shelling out to the binary would fold process
//! startup into the constant term and blunt exactly the signal being measured.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use badness::formatter::{FormatStyle, format_node};
use badness::linter::{Diagnostic, OutputMode, Severity, render_findings};
use badness::parser::parse;
use badness::syntax::SyntaxNode;

/// Linear doubles; the quadratics these guard against tripled or worse, so the
/// bound sits between. Measured headroom at the time of writing: 1.91x and
/// 2.10x for the two clean cases.
const MAX_RATIO: f64 = 3.0;

/// The nesting case alone, which still carries a known superlinear residue
/// (`Ir::contains_forced_break`, filed in TODO.md) and measures 2.76x. Loose
/// enough not to flake, tight enough to catch a return to quadratic. **Tighten
/// this to `MAX_RATIO` when that item lands; never loosen it.**
const MAX_NESTING_RATIO: f64 = 3.4;

/// Best of five — see the module docs on why the minimum.
fn best_of<T>(mut run: impl FnMut() -> T) -> Duration {
    (0..5)
        .map(|_| {
            let start = Instant::now();
            let out = run();
            let elapsed = start.elapsed();
            drop(std::hint::black_box(out));
            elapsed
        })
        .min()
        .unwrap()
}

/// Run `body` on a thread with a stack deep recursion fits in, propagating a
/// panic (an assertion failure) so the test still fails.
fn deep_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(body)
        .unwrap()
        .join()
        // Re-raise rather than `unwrap`, so an assertion failure inside keeps
        // its own message instead of becoming `Any { .. }`.
        .unwrap_or_else(|payload| std::panic::resume_unwind(payload));
}

/// Assert that doubling the input size grows the time by less than `bound`.
fn assert_scales<T>(what: &str, n: usize, bound: f64, mut run: impl FnMut(usize) -> T) {
    let small = best_of(|| run(n));
    let large = best_of(|| run(2 * n));
    let ratio = large.as_secs_f64() / small.as_secs_f64().max(f64::EPSILON);
    assert!(
        ratio < bound,
        "{what}: doubling {n} -> {} took {ratio:.2}x ({small:?} -> {large:?}), \
         over the {bound}x growth bound",
        2 * n,
    );
}

#[test]
fn pretty_rendering_scales_with_findings_not_findings_times_file_length() {
    // Handing `annotate-snippets` the whole file per finding rebuilt an O(file)
    // source map on every render. Both dimensions grow together here, which is
    // what the product term needs to show up.
    let path = PathBuf::from("scaling.tex");
    assert_scales("pretty rendering", 2000, MAX_RATIO, |n| {
        let source = "\\[\n".repeat(n);
        let diagnostics: Vec<Diagnostic> = (0..n)
            .map(|i| Diagnostic {
                rule: "unclosed-math-delimiter",
                severity: Severity::Warning,
                path: path.clone(),
                start: i * 3,
                end: i * 3 + 2,
                message: "`\\[` has no matching `\\]`".to_owned(),
                fix: None,
                related: Vec::new(),
            })
            .collect();
        render_findings(&diagnostics, OutputMode::Pretty, false, &|_: &Path| {
            Some(source.clone())
        })
    });
}

#[test]
fn parsing_a_single_long_line_scales_with_its_length() {
    // `on_doc_margin_line` walked back to the previous newline for every
    // `\begin`/`\end`, so a document written as one line was O(N x line length).
    assert_scales("long-line parse", 4000, MAX_RATIO, |n| {
        let src = format!("{{{}}}", "\\begin{itemize}".repeat(n));
        parse(&src)
    });
}

#[test]
fn lowering_scales_with_brace_nesting_depth() {
    // The relayout arms' `contains_doc_margin` guard walked each node's whole
    // subtree, so nested groups re-walked at every level.
    //
    // NOTE: this one is the loosest of the three. `Ir::contains_forced_break`
    // is still a per-child subtree walk at lowering time (a documented
    // decision, filed in TODO.md), so nesting has a residual superlinear term
    // this bound tolerates. Tighten it when that lands; do not loosen it.
    // Lowering recurses with the tree, and a `cargo test` thread's default 2 MiB
    // does not hold 500 frames of it in a debug build. The depth is the point of
    // the test, so give it room rather than shrinking it into the range where
    // fixed costs dominate the ratio.
    deep_stack(|| {
        // `format_node` over a pre-built CST, so the parse is out of the timing
        // and this measures lowering alone.
        let tree = |n: usize| {
            let src = format!("{}x{}\n", "{".repeat(n), "}".repeat(n));
            SyntaxNode::new_root(parse(&src).green)
        };
        let (small, large) = (tree(250), tree(500));
        assert_scales("nested-group lowering", 250, MAX_NESTING_RATIO, |n| {
            let root = if n == 250 { &small } else { &large };
            format_node(root, FormatStyle::default())
        });
    });
}
