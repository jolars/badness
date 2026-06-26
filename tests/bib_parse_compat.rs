//! Soft differential parse-concordance gauge for BibTeX (NOT a quality gate).
//!
//! The bib analog of `parse_compat.rs`. It measures how *structurally close* badness's
//! bib CST is to texlab's over a corpus, after projecting both onto one coarse skeleton
//! (see `bib_skeleton`). A divergence from texlab is **never a bug by itself**: badness
//! is a deliberately generic surface parser and the skeleton deliberately stops at the
//! field level, where the two genuinely diverge (texlab resolves accents/commands inside
//! values; badness keeps them generic). AGENTS.md: measure against, never match.
//!
//! It is `#[ignore]`d so it does not run in `cargo test`; invoke it explicitly:
//!
//! ```sh
//! task bib-parse-compat
//! # or
//! cargo test --test bib_parse_compat -- --ignored --nocapture
//! ```
//!
//! The result is written to `.claude/skills/bib-parse-compat/BIB_PARSE_COMPAT.md`
//! and is a triage queue, not a pass/fail signal.

#[path = "support/bib_skeleton.rs"]
mod bib_skeleton;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use bib_skeleton::{dice, lcs_len, project_badness, project_texlab, render_lines};

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
}

#[test]
#[ignore = "soft parse-concordance gauge; run via `task bib-parse-compat`"]
fn bib_parse_compat_report() {
    let corpus = collect_corpus();
    if corpus.is_empty() {
        eprintln!("bib-parse-compat: empty corpus; nothing to measure.");
        return;
    }

    let allowlist = load_allowlist();

    let mut reports: Vec<FileReport> = Vec::new();
    let mut total_lcs2 = 0usize; // sum of 2 * LCS
    let mut total_lines = 0usize; // sum of (badness_lines + texlab_lines) over compared files

    for (key, path) in &corpus {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };

        if !badness::bib::parse(&text).errors.is_empty() {
            reports.push(FileReport {
                key: key.clone(),
                outcome: Outcome::SkippedBadness,
            });
            continue;
        }

        let badness_lines = render_lines(&project_badness(&text));
        let texlab_lines = render_lines(&project_texlab(&text));

        total_lcs2 += 2 * lcs_len(&badness_lines, &texlab_lines);
        total_lines += badness_lines.len() + texlab_lines.len();

        let outcome = if badness_lines == texlab_lines {
            Outcome::Concordant
        } else {
            Outcome::Divergent {
                similarity: dice(&badness_lines, &texlab_lines),
            }
        };
        reports.push(FileReport {
            key: key.clone(),
            outcome,
        });
    }

    let report = render_report(&reports, &allowlist, total_lcs2, total_lines);
    print!("{report}");

    let out_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".claude/skills/bib-parse-compat/BIB_PARSE_COMPAT.md");
    fs::write(&out_path, &report).expect("write BIB_PARSE_COMPAT.md");
    eprintln!("bib-parse-compat: wrote {}", out_path.display());
}

// --- corpus ---------------------------------------------------------------

/// Returns `(identifier, path)` pairs; the identifier is the allowlist key (the corpus
/// file name).
fn collect_corpus() -> Vec<(String, PathBuf)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("bib_corpus");
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("bib") {
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
/// hand-parsed to avoid a dev-dependency). Mirrors `parse_compat.rs`.
fn load_allowlist() -> BTreeMap<String, String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("bib_parse_compat_allowlist.toml");
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

// --- reporting ------------------------------------------------------------

fn render_report(
    reports: &[FileReport],
    allowlist: &BTreeMap<String, String>,
    total_lcs2: usize,
    total_lines: usize,
) -> String {
    let mut concordant = 0usize;
    let mut skipped_badness = 0usize;
    let mut intentional: Vec<(&str, &str, f64)> = Vec::new();
    let mut unexplained: Vec<(&str, f64)> = Vec::new();

    for r in reports {
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
    s.push_str("# BibTeX parse concordance vs texlab (soft signal)\n\n");
    s.push_str(
        "_Generated by `task bib-parse-compat` (`tests/bib_parse_compat.rs`). Do not edit by hand._\n\n",
    );
    s.push_str(
        "This is a **soft gauge, not a quality gate.** It projects badness's bib CST and \
         texlab's bib CST onto one coarse skeleton (entry types and field names — but not \
         cite keys or value internals, where the two legitimately diverge) and measures how \
         structurally close they are. A divergence is never a bug by itself: it is either a \
         deliberate, recorded deviation or an open question. Note texlab's bib parser has no \
         error channel, so this gauge never reports texlab parse errors. AGENTS.md: measure \
         against, never match.\n\n",
    );
    s.push_str("- **Corpus:** bib corpus (`tests/bib_corpus/*.bib`)\n");
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
    s.push('\n');

    if !unexplained.is_empty() {
        unexplained.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        s.push_str("## Unexplained divergences (triage queue)\n\n");
        s.push_str(
            "Each is **either a parser modeling gap to fix** or a **deliberate deviation to \
             record** (add it to `tests/bib_parse_compat_allowlist.toml` with a reason). Leaving \
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
        s.push_str(
            "Listed in `tests/bib_parse_compat_allowlist.toml`. These diverge from texlab on purpose.\n\n",
        );
        s.push_str("| File | Skeleton similarity | Reason |\n|---|---|---|\n");
        for (key, reason, sim) in &intentional {
            s.push_str(&format!("| `{key}` | {:.1}% | {reason} |\n", sim * 100.0));
        }
        s.push('\n');
    }

    s
}
