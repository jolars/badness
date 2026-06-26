//! Soft differential parse-concordance gauge (NOT a quality gate).
//!
//! This harness measures how *structurally close* badness's CST is to texlab's over a
//! corpus, after projecting both onto one coarse skeleton (see `parse_skeleton`). It
//! is the parse-side analog of arity's `air_compat.rs`, and is subordinate to the
//! same principle: a divergence from texlab is **never a bug by itself**. badness is
//! a deliberately *generic* TeX-surface parser; texlab is *semantically enriched*.
//! AGENTS.md is explicit that these references are "measured against, never matched."
//!
//! It is `#[ignore]`d so it does not run in `cargo test`; invoke it explicitly:
//!
//! ```sh
//! task parse-compat
//! # or
//! cargo test --test parse_compat -- --ignored --nocapture
//! ```
//!
//! Methodology — for each corpus file we project badness's and texlab's trees onto the
//! shared skeleton, render each to indented S-expression lines, and score the Dice
//! coefficient over those lines. Files badness cannot parse cleanly are skipped (we
//! never measure against our own parse errors). The result is written to
//! `.claude/skills/parse-compat/PARSE_COMPAT.md` and is a triage queue, not a
//! pass/fail signal.

#[path = "support/parse_skeleton.rs"]
mod parse_skeleton;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use badness::parser::parse;
use parse_skeleton::{
    Badness, dice, lcs_len, project, project_texlab, render_lines, texlab_has_error,
};

/// One corpus file's outcome.
enum Outcome {
    /// The two projected skeletons are identical.
    Concordant,
    /// The skeletons differ. Carries the Dice similarity over skeleton lines.
    Divergent { similarity: f64 },
    /// badness could not parse the input cleanly (syntax errors).
    SkippedBadness,
}

struct FileReport {
    key: String,
    outcome: Outcome,
    /// Whether texlab also flagged a parse error (recorded for context, never a skip).
    texlab_error: bool,
}

#[test]
#[ignore = "soft parse-concordance gauge; run via `task parse-compat`"]
fn parse_compat_report() {
    let corpus = collect_corpus();
    if corpus.is_empty() {
        eprintln!("parse-compat: empty corpus; nothing to measure.");
        return;
    }

    let allowlist = load_allowlist();

    // Optional triage aid: `PARSE_COMPAT_DUMP=1 task parse-compat` writes a per-file
    // skeleton diff for every divergent/skipped file to `target/parse_compat_diffs.txt`,
    // so a classifier can read the actual badness-vs-texlab shapes (the report only
    // carries similarity numbers). Off by default; touches only this test harness.
    let dump_enabled = std::env::var_os("PARSE_COMPAT_DUMP").is_some();
    let mut dump = String::new();

    let mut reports: Vec<FileReport> = Vec::new();
    let mut total_lcs2 = 0usize; // sum of 2 * LCS
    let mut total_lines = 0usize; // sum of (badness_lines + texlab_lines) over compared files

    for (key, path) in &corpus {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };

        let parsed = parse(&text);
        if !parsed.errors.is_empty() {
            if dump_enabled {
                let badness_lines = render_lines(&project::<Badness>(&parsed.syntax()));
                let texlab_lines = render_lines(&project_texlab(&text));
                let errs: String = parsed
                    .errors
                    .iter()
                    .map(|e| format!("    {e:?}\n"))
                    .collect();
                dump.push_str(&dump_section(key, &errs, &badness_lines, &texlab_lines));
            }
            reports.push(FileReport {
                key: key.clone(),
                outcome: Outcome::SkippedBadness,
                texlab_error: false,
            });
            continue;
        }

        let badness_lines = render_lines(&project::<Badness>(&parsed.syntax()));
        let texlab_lines = render_lines(&project_texlab(&text));
        let texlab_error = texlab_has_error(&text);

        total_lcs2 += 2 * lcs_len(&badness_lines, &texlab_lines);
        total_lines += badness_lines.len() + texlab_lines.len();

        let outcome = if badness_lines == texlab_lines {
            Outcome::Concordant
        } else {
            Outcome::Divergent {
                similarity: dice(&badness_lines, &texlab_lines),
            }
        };
        if dump_enabled && !matches!(outcome, Outcome::Concordant) {
            dump.push_str(&dump_section(key, "", &badness_lines, &texlab_lines));
        }
        reports.push(FileReport {
            key: key.clone(),
            outcome,
            texlab_error,
        });
    }

    if dump_enabled {
        let dump_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("parse_compat_diffs.txt");
        fs::write(&dump_path, &dump).expect("write parse_compat_diffs.txt");
        eprintln!(
            "parse-compat: wrote {} (PARSE_COMPAT_DUMP)",
            dump_path.display()
        );
    }

    let report = render_report(
        &reports,
        &allowlist,
        total_lcs2,
        total_lines,
        &corpus_label(),
    );
    print!("{report}");

    let out_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(".claude/skills/parse-compat/PARSE_COMPAT.md");
    fs::write(&out_path, &report).expect("write PARSE_COMPAT.md");
    eprintln!("parse-compat: wrote {}", out_path.display());
}

// --- corpus ---------------------------------------------------------------

fn corpus_label() -> String {
    "corpus (`tests/corpus/*.tex`)".to_string()
}

/// Returns `(identifier, path)` pairs; the identifier is the allowlist key (the
/// corpus file name).
fn collect_corpus() -> Vec<(String, PathBuf)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("corpus");
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("tex") {
                let key = path.file_name().unwrap().to_string_lossy().into_owned();
                files.push((key, path));
            }
        }
    }
    files.sort();
    files
}

// --- allowlist ------------------------------------------------------------

/// Loads the deviations allowlist: `key = "reason"` lines (a simple TOML subset,
/// hand-parsed to avoid a dev-dependency). Mirrors `air_compat.rs`.
fn load_allowlist() -> BTreeMap<String, String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("parse_compat_allowlist.toml");
    let mut map = BTreeMap::new();
    let Ok(text) = fs::read_to_string(&path) else {
        return map;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().trim_matches('"').to_string();
        let value = value.trim().trim_matches('"').to_string();
        map.insert(key, value);
    }
    map
}

// --- triage dump (PARSE_COMPAT_DUMP) --------------------------------------

/// One file's section in the triage dump: optional badness parse errors followed
/// by an LCS-aligned diff of the two skeletons.
fn dump_section(key: &str, errors: &str, badness: &[String], texlab: &[String]) -> String {
    let mut s = format!("===== {key} =====\n");
    if !errors.is_empty() {
        s.push_str("badness parse errors (SkippedBadness):\n");
        s.push_str(errors);
    }
    s.push_str("diff (`-` badness-only, `+` texlab-only, ` ` common):\n");
    s.push_str(&unified_diff(badness, texlab));
    s.push('\n');
    s
}

/// A minimal LCS-aligned unified diff of two line slices. `-` marks a line only in
/// `a` (badness), `+` a line only in `b` (texlab), and a leading space a shared line.
fn unified_diff(a: &[String], b: &[String]) -> String {
    let (n, m) = (a.len(), b.len());
    // dp[i][j] = LCS length of a[i..] and b[j..].
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let mut out = String::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push_str(&format!("  {}\n", a[i]));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            out.push_str(&format!("- {}\n", a[i]));
            i += 1;
        } else {
            out.push_str(&format!("+ {}\n", b[j]));
            j += 1;
        }
    }
    for line in &a[i..] {
        out.push_str(&format!("- {line}\n"));
    }
    for line in &b[j..] {
        out.push_str(&format!("+ {line}\n"));
    }
    out
}

// --- reporting ------------------------------------------------------------

fn render_report(
    reports: &[FileReport],
    allowlist: &BTreeMap<String, String>,
    total_lcs2: usize,
    total_lines: usize,
    corpus_label: &str,
) -> String {
    let mut concordant = 0usize;
    let mut skipped_badness = 0usize;
    let mut texlab_errors = 0usize;
    let mut intentional: Vec<(&str, &str, f64)> = Vec::new();
    let mut unexplained: Vec<(&str, f64)> = Vec::new();

    for r in reports {
        if r.texlab_error {
            texlab_errors += 1;
        }
        match r.outcome {
            Outcome::Concordant => concordant += 1,
            Outcome::SkippedBadness => skipped_badness += 1,
            Outcome::Divergent { similarity } => {
                if let Some(reason) = allowlist.get(&r.key) {
                    intentional.push((&r.key, reason, similarity));
                } else {
                    unexplained.push((&r.key, similarity));
                }
            }
        }
    }

    let measured = concordant + intentional.len() + unexplained.len();
    let file_concord = if measured == 0 {
        100.0
    } else {
        concordant as f64 / measured as f64 * 100.0
    };
    let line_sim = if total_lines == 0 {
        100.0
    } else {
        total_lcs2 as f64 / total_lines as f64 * 100.0
    };

    let mut s = String::new();
    s.push_str("# Parse concordance vs texlab (soft signal)\n\n");
    s.push_str(
        "_Generated by `task parse-compat` (`tests/parse_compat.rs`). Do not edit by hand._\n\n",
    );
    s.push_str(
        "This is a **soft gauge, not a quality gate.** It projects badness's generic CST \
         and texlab's semantic CST onto one coarse skeleton (commands, environments, \
         groups, math, verbatim — names but not roles) and measures how structurally \
         close they are. A divergence is never a bug by itself: it is either a deliberate, \
         recorded deviation or an open question (badness models TeX surface syntax; texlab \
         resolves semantics). AGENTS.md: measure against, never match.\n\n",
    );
    s.push_str(&format!("- **Corpus:** {corpus_label}\n"));
    s.push_str(&format!(
        "- **Skeleton similarity:** {line_sim:.1}%  _(Dice coefficient over skeleton lines)_\n"
    ));
    s.push_str(&format!(
        "- **File concordance:** {file_concord:.1}%  ({concordant}/{measured} files identical after projection)\n"
    ));
    s.push_str(&format!(
        "- **Intentional deviations:** {}  ·  **Unexplained divergences:** {}\n",
        intentional.len(),
        unexplained.len()
    ));
    if skipped_badness > 0 {
        s.push_str(&format!(
            "- **Skipped:** {skipped_badness} (badness could not parse cleanly)\n"
        ));
    }
    if texlab_errors > 0 {
        s.push_str(&format!(
            "- **texlab parse errors:** {texlab_errors} (badness-clean inputs texlab flagged — see the gate, `parse_oracle.rs`)\n"
        ));
    }
    s.push('\n');

    if !unexplained.is_empty() {
        unexplained.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        s.push_str("## Unexplained divergences (triage queue)\n\n");
        s.push_str(
            "Each is **either a parser modeling gap to fix** or a **deliberate deviation to \
             record** (add it to `tests/parse_compat_allowlist.toml` with a reason). Leaving \
             it here is the tension: diverging from texlab should be a conscious choice.\n\n",
        );
        s.push_str("| File | Skeleton similarity |\n|---|---|\n");
        for (key, sim) in &unexplained {
            s.push_str(&format!("| `{key}` | {:.1}% |\n", sim * 100.0));
        }
        s.push('\n');
    }

    if !intentional.is_empty() {
        intentional.sort_by(|a, b| a.0.cmp(b.0));
        s.push_str("## Recorded intentional deviations\n\n");
        s.push_str("Listed in `tests/parse_compat_allowlist.toml`. These diverge from texlab on purpose.\n\n");
        s.push_str("| File | Skeleton similarity | Reason |\n|---|---|---|\n");
        for (key, reason, sim) in &intentional {
            s.push_str(&format!("| `{key}` | {:.1}% | {reason} |\n", sim * 100.0));
        }
        s.push('\n');
    }

    s
}
