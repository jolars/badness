//! CLI-level tests for `badness debug format`, the invariant-check command the
//! smoke-test workflow (`.github/workflows/smoke-test.yml`) drives per file.
//!
//! These pin the output contracts the workflow greps — the parenthesized
//! failure labels, the report header, and the sanitized dump-file names — by
//! running the real binary (`CARGO_BIN_EXE_badness`). Every invocation passes
//! `--no-config` so a developer's own `badness.toml` cannot leak in.

use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

fn badness(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_badness"))
        .arg("--no-config")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run badness")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn passing_file_exits_zero() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("ok.tex"), "Hello \\emph{world}.\n").unwrap();

    let output = badness(
        dir.path(),
        &["debug", "format", "--checks", "all", "ok.tex"],
    );

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("All checks passed (checks: all, files: 1)"));
}

#[test]
fn report_on_passing_file_has_no_failure_headings() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("ok.tex"), "Hello world.\n").unwrap();

    let output = badness(dir.path(), &["debug", "format", "--report", "ok.tex"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let report = stdout(&output);
    assert!(report.contains("# Debug-format regression report"));
    assert!(report.contains("All checks passed."));
    assert!(!report.contains("(idempotency)"));
    assert!(!report.contains("(losslessness)"));
}

#[test]
fn dump_passes_writes_sanitized_artifact_names() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("sub dir")).unwrap();
    let source = "Hello \\emph{world}.\n";
    std::fs::write(dir.path().join("sub dir/a b.tex"), source).unwrap();

    let output = badness(
        dir.path(),
        &[
            "debug",
            "format",
            "--dump-dir",
            "dumps",
            "--dump-passes",
            "sub dir/a b.tex",
        ],
    );

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    // The stem is the path as passed, sanitized exactly like the workflow's
    // `sed 's/[^[:alnum:]._-]/_/g'` — this is the artifact-lookup contract.
    let dumps = dir.path().join("dumps");
    let input = std::fs::read_to_string(dumps.join("sub_dir_a_b.tex.idempotency.input.txt"))
        .expect("input dump exists");
    let once = std::fs::read_to_string(dumps.join("sub_dir_a_b.tex.idempotency.once.txt"))
        .expect("once dump exists");
    let twice = std::fs::read_to_string(dumps.join("sub_dir_a_b.tex.idempotency.twice.txt"))
        .expect("twice dump exists");
    assert_eq!(input, source);
    assert_eq!(once, twice);
    assert!(
        dumps
            .join("sub_dir_a_b.tex.losslessness.input.txt")
            .exists()
    );
    assert!(
        dumps
            .join("sub_dir_a_b.tex.losslessness.parsed.txt")
            .exists()
    );
}

#[test]
fn dump_passes_requires_dump_dir() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("ok.tex"), "x\n").unwrap();

    let output = badness(dir.path(), &["debug", "format", "--dump-passes", "ok.tex"]);

    assert_eq!(output.status.code(), Some(2), "clap usage error expected");
}

#[test]
fn trivia_check_passes_on_stable_prose() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("ok.tex"), "alpha\nbeta gamma delta.\n").unwrap();

    let output = badness(
        dir.path(),
        &["debug", "format", "--checks", "trivia", "ok.tex"],
    );

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("All checks passed (checks: trivia, files: 1)"));
}

#[test]
fn trivia_check_counts_skipped_bib_files_separately() {
    // The trivia oracle is LaTeX-CST-based and runs nothing on a `.bib` file:
    // the summary must report it as skipped, never fold it into the checked
    // count (which would overstate oracle coverage on a `.bib`-heavy sweep).
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("ok.tex"), "alpha beta.\n").unwrap();
    std::fs::write(
        dir.path().join("refs.bib"),
        "@article{key, title = {T}, year = {2020}}\n",
    )
    .unwrap();

    let output = badness(dir.path(), &["debug", "format", "--checks", "trivia", "."]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("All checks passed (checks: trivia, files: 1, skipped: 1)"),
        "stdout: {}",
        stdout(&output)
    );

    let output = badness(
        dir.path(),
        &["debug", "format", "--checks", "trivia", "--report", "."],
    );
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let report = stdout(&output);
    assert!(report.contains("- Files checked: 1"), "report: {report}");
    assert!(report.contains("- Files skipped: 1"), "report: {report}");
}

#[test]
fn trivia_check_fires_on_a_known_hybrid() {
    let dir = TempDir::new().unwrap();
    // A known convergence failure, delta-debugged down from latex3's
    // `l3packages/xparse/xparse-generic.tex` (one of the `non-fixed-point`
    // entries in `tests/gate_baselines/*.trivia.txt`): under the
    // `all-newlines-to-spaces` variant the expl3 layout picks a different set of
    // statement boundaries, so the formatted perturbed output is not a fixed
    // point. The trivia check must fail on it — and only under `--checks trivia`,
    // never `all`, which sees no losslessness or idempotency problem.
    //
    // Every layout hybrid is a column-arithmetic accident, so the exact shape is
    // load-bearing: tidying the braces or renaming the control sequences makes it
    // converge again. Do not "clean it up" — reduce a fresh corpus failure instead.
    std::fs::write(dir.path().join("hybrid.tex"), HYBRID_TEX).unwrap();

    let output = badness(
        dir.path(),
        &["debug", "format", "--checks", "trivia", "hybrid.tex"],
    );
    assert_eq!(output.status.code(), Some(1));
    let log = stderr(&output);
    assert!(log.contains("Debug check failed (trivia:"), "log: {log}");

    let output = badness(
        dir.path(),
        &[
            "debug",
            "format",
            "--checks",
            "trivia",
            "--report",
            "hybrid.tex",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let report = stdout(&output);
    assert!(report.contains("- Checks: `trivia`"), "report: {report}");
    assert!(
        report.contains("### 1. `hybrid.tex` (trivia)"),
        "report: {report}"
    );
    assert!(report.contains("- Variant: `"), "report: {report}");

    // `all` keeps its meaning: losslessness + idempotency only. The same file
    // passes it, and no `(trivia)` label can appear in its output.
    let output = badness(
        dir.path(),
        &[
            "debug",
            "format",
            "--checks",
            "all",
            "--report",
            "hybrid.tex",
        ],
    );
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(!stdout(&output).contains("(trivia)"));
}

#[test]
fn trivia_strict_check_fires_where_an_authored_break_is_preserved() {
    let dir = TempDir::new().unwrap();
    // Two *un-signatured* top-level commands separated by an authored newline.
    // The residual command-only-line rule keeps that break, but glues the pair
    // when the same gap is a space — the same bytes to the next parse, so a
    // read of the lone-newline predicate. The read is sanctioned Tier 2
    // (preservation-only; the fixed-point argument is on
    // `line_is_command_only`), but sanctioned or not only the strict survey can
    // see it. Curated block commands (`\usepackage`, …) no longer reach that
    // rule — they are intercepted as block-level statements via
    // `CommandSig::block` — so the probe uses names no signature tier knows,
    // whose block-ness only the authored break can carry.
    //
    // Neither `all` nor `trivia` can see it: both spellings are self-consistent
    // fixed points that round-trip losslessly, which is the whole reason the
    // strict oracle earns a CLI surface.
    std::fs::write(
        dir.path().join("preserved.tex"),
        "\\zzalpha{a}\n\\zzbeta{b}\nalpha\nbeta gamma\n",
    )
    .unwrap();

    let output = badness(
        dir.path(),
        &[
            "debug",
            "format",
            "--checks",
            "trivia-strict",
            "--report",
            "preserved.tex",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let report = stdout(&output);
    assert!(
        report.contains("- Checks: `trivia-strict`"),
        "report: {report}"
    );
    assert!(
        report.contains("### 1. `preserved.tex` (trivia-strict)"),
        "report: {report}"
    );
    // The reported reproducer must be a localized `flip@…` gap, not one of the
    // two whole-file bulk variants that are generated first — a mega-line diff
    // names no construct.
    assert!(
        report.contains("variants diverged, reported: flip@"),
        "report: {report}"
    );

    // Neither other check sees it, and no `trivia-strict` label may leak into
    // `all` — the smoke-test workflow classifies failures by grepping for its
    // own three labels.
    for checks in ["all", "trivia"] {
        let output = badness(
            dir.path(),
            &[
                "debug",
                "format",
                "--checks",
                checks,
                "--report",
                "preserved.tex",
            ],
        );
        assert!(
            output.status.success(),
            "checks={checks} stderr: {}",
            stderr(&output)
        );
        assert!(!stdout(&output).contains("trivia-strict"));
    }
}

#[test]
fn trivia_strict_check_counts_skipped_bib_files_separately() {
    // Same LaTeX-CST-based skip as the convergence check: a `.bib` runs nothing
    // and must be reported as skipped, never folded into the checked count.
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("ok.tex"), "alpha beta.\n").unwrap();
    std::fs::write(
        dir.path().join("refs.bib"),
        "@article{key, title = {T}, year = {2020}}\n",
    )
    .unwrap();

    let output = badness(
        dir.path(),
        &["debug", "format", "--checks", "trivia-strict", "."],
    );
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("All checks passed (checks: trivia-strict, files: 1, skipped: 1)"),
        "stdout: {}",
        stdout(&output)
    );
}

#[test]
fn dtx_doc_margin_frame_survives_reflow() {
    let dir = TempDir::new().unwrap();
    // A `.dtx` whose documentation prose sits either side of a margin-framed
    // `macrocode` chunk holding expl3 code. Reflow — now the default for every
    // file kind — must not join the `%    \begin{macrocode}` frame line onto the
    // prose above it: the frame's `%` would leave column 0, stop being a comment
    // at package-load time, and the next pass would not parse. Both checks pass.
    std::fs::write(
        dir.path().join("expl.dtx"),
        "% \\section{Implementation}\n\
         %    \\begin{macrocode}\n\
         \\ExplSyntaxOn\n\
         \\tl_new:N \\l_tmpa_tl\n\
         %    \\end{macrocode}\n\
         %\n\
         % Some prose.\n\
         %    \\begin{macrocode}\n\
         \\ExplSyntaxOff\n\
         %    \\end{macrocode}\n",
    )
    .unwrap();

    for checks in ["all", "trivia"] {
        let output = badness(
            dir.path(),
            &["debug", "format", "--checks", checks, "expl.dtx"],
        );
        assert!(
            output.status.success(),
            "--checks {checks} stderr: {}",
            stderr(&output)
        );
    }
}

/// The reduced hybrid behind [`trivia_check_fires_on_a_known_hybrid`]. Kept as a
/// literal so the exact whitespace survives editing.
///
/// Fuzz-reduced from the `\exp_after:wN`-chain family that survives the arity
/// migration (`l3fp-trig.dtx` and kin — the reduction is of the *family*, not
/// of a baseline entry: those files re-fill to a fixed point whole, which is
/// why none of them is recorded in `tests/gate_baselines`. The previous
/// xparse-generic reduction, and each subsequent corpus carve, converged as
/// structural argument ownership stabilized more shapes). Every head is
/// `w`-underivable, so both lines are fallback statements; the greedy refill
/// of the `all-newlines-to-spaces` variant lands the mid-line groups at
/// widths the next pass segments differently.
const HYBRID_TEX: &str = r#"\ExplSyntaxOn
{
\module_aux:w { \scan_stop: }
\int_value:w \module_pack:wNNNNNNNN \scan_stop: { \module_int_eval:w \scan_stop: }
}
\ExplSyntaxOff
"#;

#[test]
fn line_width_flag_reaches_the_formatter() {
    let dir = TempDir::new().unwrap();
    let long = "alpha beta gamma delta epsilon zeta eta theta iota kappa\n";
    std::fs::write(dir.path().join("wide.tex"), long).unwrap();

    let output = badness(
        dir.path(),
        &[
            "debug",
            "format",
            "--checks",
            "idempotency",
            "--line-width",
            "30",
            "--dump-dir",
            "dumps",
            "--dump-passes",
            "wide.tex",
        ],
    );

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let once = std::fs::read_to_string(
        dir.path()
            .join("dumps")
            .join("wide.tex.idempotency.once.txt"),
    )
    .expect("once dump exists");
    assert!(
        once.lines().count() > 1 && once.lines().all(|l| l.len() <= 30),
        "expected a wrap at width 30, got: {once:?}"
    );
}

#[test]
fn format_error_never_reads_as_an_invariant_failure() {
    let dir = TempDir::new().unwrap();
    // An unclosed group parses with diagnostics, so the formatter refuses it:
    // a `format-error`, not an idempotency or losslessness regression.
    std::fs::write(dir.path().join("bad.tex"), "a{\n").unwrap();

    let output = badness(dir.path(), &["debug", "format", "bad.tex"]);

    assert_eq!(output.status.code(), Some(1));
    let log = stderr(&output).to_lowercase();
    assert!(log.contains("(format-error)"), "log: {log}");
    assert!(!log.contains("idempot"), "log: {log}");
    assert!(!log.contains("lossless"), "log: {log}");

    let output = badness(dir.path(), &["debug", "format", "--report", "bad.tex"]);
    assert_eq!(output.status.code(), Some(1));
    let report = stdout(&output);
    assert!(report.contains("### 1. `bad.tex` (format-error)"));
    assert!(report.contains("- Files checked: 1"));
}
