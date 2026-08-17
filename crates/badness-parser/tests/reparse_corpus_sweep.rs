//! The incremental reparse oracle: breadth.
//!
//! Seeded edits over the pinned gate corpora (`corpora/`, fetched by
//! `scripts/fetch_gate_corpora.sh`) — ~6.3k files and ~39 MB of real package
//! source and adversarial test input, against the ~30 hand-written snippets and 58
//! in-crate corpus files `incremental_reparse.rs` runs in `cargo test`. Same
//! generator, same checker, same seeds: both harnesses share
//! [`reparse_harness`], so anything this finds reduces in the fast suite.
//!
//! Run it with `task reparse-corpora:check`, which builds release, runs this, and
//! diffs the tallies against `tests/reparse_baselines/`. Directly:
//!
//! ```sh
//! cargo test --release -p badness-parser --test reparse_corpus_sweep -- --ignored --nocapture
//! ```
//!
//! # What it asserts, and what it records
//!
//! Two different things, and the distinction is the point.
//!
//! **Asserted, in this file:** the reparse invariant on every edit (a divergence is
//! a bug, never a baseline — there is nothing to record and nothing to accept), and
//! a **splice-rate floor per driver per corpus**. The floor is the tripwire: every
//! invariant assertion here is vacuously true on a `None`, so a guard that narrowed
//! a tier to nothing would leave the whole sweep green while testing nothing.
//! Panache's window cutoff cost its fuzzer two thirds of its coverage exactly that
//! way, with every assertion it carried still passing.
//!
//! **Recorded, in `tests/reparse_baselines/`:** the exact tallies, as a two-sided
//! ratchet in the shape of `tests/gate_baselines/` — a rate that falls is a
//! regression, a rate that rises means the baseline is stale and gets re-recorded
//! with the commit that moved it. The floors survive a careless re-record; the
//! recorded tallies notice the drift a floor is too coarse to see, including a
//! workload silently changing *tier*, which no floor can catch because declining is
//! always sound.
//!
//! # Why these five drivers
//!
//! Each is a workload with a tier it should reach, not a random shape:
//!
//! - `word-typing` / `word-deleting` — a caret inside a real word of a real
//!   document, the token tier's entire reason to exist. Over a corpus this measures
//!   the tier's *reach*: how much of real LaTeX prose is a place where typing
//!   splices. That number is the interesting one, and it is what a guard change
//!   moves first.
//! - `protected-typing` — a caret inside a `VERBATIM_BODY` or a `VERB`, the
//!   protected-body tier's workload, on bodies the corpus wrote rather than ones a
//!   test invented.
//! - `hazard-single` / `hazard-chain` — the correctness workhorses. The alphabet is
//!   deliberately most of what a tier must *refuse*, so the rate is low by
//!   construction and the value is in the refusals being right.
//!
//! Seeds are derived from each file's corpus-relative path, never from an absolute
//! path or a directory listing order, so the recorded tallies are a property of the
//! pinned corpora and not of the machine that swept them.

#[path = "support/reparse_harness.rs"]
mod reparse_harness;

use std::fs;
use std::path::{Path, PathBuf};

use badness_parser::parser::{Edit, LatexFlavor, LexConfig};
use badness_parser::syntax::{SyntaxKind, SyntaxToken};
use rayon::prelude::*;

use reparse_harness::{
    Base, Lcg, Tally, assert_splice_floor, char_boundary_at_or_below, next_char_boundary,
    random_edit,
};

/// The corpora `scripts/fetch_gate_corpora.sh` pins, in the order it lists them.
const CORPORA: &[&str] = &["latex3", "latex2e", "pgf", "latexindent"];

/// The drivers, in report order, each with the splice-rate floor it must hold **in
/// every corpus**.
///
/// A floor is a tripwire, not a target. Each sits at roughly half the *lowest* rate
/// any corpus recorded, so an ordinary guard change re-records the baseline without
/// tripping it and only a collapse fails here — the recorded tallies are what notice
/// the ordinary movement.
///
/// They are per corpus rather than over the union because the corpora are not the
/// same workload: a rate is dominated by which file kinds a corpus holds, and `.dtx`
/// splices nothing at all on either leaf tier (`implicit_expl` is derived from a
/// whole-file scan, so a fragment can be lexed under a regime the file never had).
/// latex3 is half `.dtx` by file and more by bytes; pgf has none. Aggregated, a
/// collapse confined to `.tex` could hide behind pgf's volume.
const DRIVERS: &[(&str, usize)] = &[
    ("word-typing", 20),
    ("word-deleting", 20),
    ("protected-typing", 5),
    ("hazard-single", 9),
    ("hazard-chain", 1),
];

/// Per-file edit counts. Fixed rather than scaled by an environment knob: the
/// recorded tallies are exact numbers, so a knob that changed them would make the
/// ratchet unreadable.
const HAZARD_EDITS: usize = 16;
const HAZARD_CHAINS: usize = 5;
/// Typing/deleting sites per file, and keystrokes at each.
const TYPING_SITES: usize = 3;
const KEYSTROKES: usize = 5;

/// One (corpus, driver) cell of the report.
#[derive(Default, Clone, Copy)]
struct Cell {
    tally: Tally,
    /// Files that offered this driver at least one edit. A driver whose sites are
    /// found in the tree (typing, protected) skips files that have none, and a
    /// skipped file is not a refusal — recording the two separately keeps the
    /// splice rate meaning what it says.
    files: usize,
}

impl Cell {
    fn merge(&mut self, other: &Cell) {
        self.tally.merge(&other.tally);
        self.files += other.files;
    }
}

/// One file's tallies, one per driver, in `DRIVERS` order.
type FileCells = Vec<Cell>;

#[test]
#[ignore = "sweeps the pinned gate corpora; run via `task reparse-corpora:check`"]
fn reparse_corpus_sweep() {
    let root = corpora_root();
    assert!(
        root.is_dir(),
        "gate corpora not found at {root:?} — fetch them first:\n  task gate-corpora:fetch",
    );

    let selected = selected_corpora();
    assert!(!selected.is_empty(), "no corpora selected");

    // Every cell is reported before any floor is asserted. A tripped floor must
    // not truncate the report: `check_reparse_baselines.sh` diffs the whole thing,
    // and a run that fails one floor is exactly the run whose other numbers say
    // whether a guard narrowed or a workload moved.
    let mut cells = Vec::new();
    for corpus in &selected {
        let dir = root.join(corpus);
        assert!(
            dir.is_dir(),
            "corpus {corpus:?} not found at {dir:?} — fetch it first:\n  task gate-corpora:fetch",
        );
        cells.push((corpus.clone(), sweep_corpus(corpus, &dir)));
    }

    for (corpus, cells) in &cells {
        for (cell, (driver, floor)) in cells.iter().zip(DRIVERS) {
            assert_splice_floor(&format!("{corpus}/{driver}"), &cell.tally, *floor);
        }
    }
}

fn sweep_corpus(corpus: &str, dir: &Path) -> FileCells {
    let files = collect_files(dir);
    assert!(!files.is_empty(), "no source files under {dir:?}");

    let bytes: usize = files.iter().map(|(_, _, text)| text.len()).sum();
    println!(
        "sweep\t{corpus}\tcorpus\tfiles={}\tbytes={bytes}",
        files.len()
    );

    let cells = files
        .par_iter()
        .map(|(rel, _, text)| sweep_file(rel, text))
        .reduce(
            || vec![Cell::default(); DRIVERS.len()],
            |mut acc, one| {
                for (slot, cell) in acc.iter_mut().zip(one.iter()) {
                    slot.merge(cell);
                }
                acc
            },
        );

    for (cell, (driver, _)) in cells.iter().zip(DRIVERS) {
        let t = &cell.tally;
        println!(
            "sweep\t{corpus}\t{driver}\tspliced={}/{}\ttoken={}\tverbatim={}\tregion={}\tfiles={}",
            t.spliced, t.attempted, t.token, t.verbatim, t.region, cell.files,
        );
    }
    cells
}

/// Run every driver over one file. Pure, so the sweep parallelizes across files.
fn sweep_file(rel: &str, text: &str) -> FileCells {
    let mut cells = vec![Cell::default(); DRIVERS.len()];
    let seed = path_seed(rel);
    let config = lex_config(rel);
    let base = Base::with_config(text.to_string(), config);

    typing_driver(
        &mut cells[0],
        rel,
        text,
        config,
        &word_sites(&base, seed),
        false,
    );
    typing_driver(
        &mut cells[1],
        rel,
        text,
        config,
        &word_sites(&base, seed ^ 0xA5),
        true,
    );
    typing_driver(
        &mut cells[2],
        rel,
        text,
        config,
        &protected_sites(&base, seed),
        false,
    );
    hazard_driver(&mut cells[3], rel, text, &base, seed);
    chain_driver(&mut cells[4], rel, text, &base, seed);

    cells
}

/// How the CLI would parse this file, by extension.
///
/// A restatement of the root crate's `FileKind::lex_config` — which is the source
/// of truth and lives in a crate this one cannot depend on. It is three lines and
/// two facts (`.sty`/`.cls`/`*.code.tex` start under an implicit `\makeatletter`;
/// a `.dtx` runs the docstrip mode), and getting it wrong would be visible: the
/// leaf tiers refuse `.dtx` outright, so a sweep that read `.dtx` as a plain
/// document would record splice rates no caller can ever obtain.
fn lex_config(rel: &str) -> LexConfig {
    let lower = rel.to_ascii_lowercase();
    let package =
        lower.ends_with(".sty") || lower.ends_with(".cls") || lower.ends_with(".code.tex");
    LexConfig {
        flavor: if package {
            LatexFlavor::Package
        } else {
            LatexFlavor::Document
        },
        dtx: lower.ends_with(".dtx"),
    }
}

/// `KEYSTROKES` single-character edits at each site, each against the text its
/// predecessor produced — a caret held still while the document moves under it.
fn typing_driver(
    cell: &mut Cell,
    rel: &str,
    text: &str,
    config: LexConfig,
    sites: &[usize],
    deleting: bool,
) {
    if sites.is_empty() {
        return;
    }
    cell.files += 1;
    for (n, &at) in sites.iter().enumerate() {
        let mut current = text.to_string();
        for (k, ch) in "typed".chars().take(KEYSTROKES).enumerate() {
            let edit = if deleting {
                Edit {
                    range: at..next_char_boundary(&current, at),
                    insert: String::new(),
                }
            } else {
                Edit {
                    range: at..at,
                    insert: ch.to_string(),
                }
            };
            // A deletion that found nothing left to delete is not an edit; it is
            // still an attempt, so the rate cannot be inflated by dropping it.
            if !edit.fits(&current) || (deleting && edit.range.is_empty()) {
                cell.tally.record(None);
                continue;
            }
            let base = Base::with_config(current.clone(), config);
            base.check(&edit, (n * KEYSTROKES + k) as u64, rel, &mut cell.tally);
            current = edit.apply(&current);
        }
    }
}

fn hazard_driver(cell: &mut Cell, rel: &str, text: &str, base: &Base, seed: u64) {
    cell.files += 1;
    let mut rng = Lcg::new(seed);
    for n in 0..HAZARD_EDITS {
        let edit = random_edit(&mut rng, text);
        base.check(&edit, n as u64, rel, &mut cell.tally);
    }
}

fn chain_driver(cell: &mut Cell, rel: &str, text: &str, base: &Base, seed: u64) {
    cell.files += 1;
    let mut rng = Lcg::new(seed ^ 0x5EED);
    for n in 0..HAZARD_CHAINS {
        let mut current = text.to_string();
        let mut chain = Vec::new();
        for _ in 0..(2 + rng.below(3)) {
            let edit = random_edit(&mut rng, &current);
            if !edit.fits(&current) {
                break;
            }
            current = edit.apply(&current);
            chain.push(edit);
        }
        if chain.is_empty() {
            cell.tally.record(None);
            continue;
        }
        base.check_chain(&chain, &current, n as u64, rel, &mut cell.tally);
    }
}

// --- site selection --------------------------------------------------------

/// Offsets inside real words: the interior of a `WORD` token long enough that
/// `KEYSTROKES` deletions still leave one behind.
///
/// Restricted to alphabetic ASCII on purpose. A `WORD` is catcode-12 text, so it
/// also carries `(1,1);` and `12.5pt`; typing into those is a different question
/// (and one `hazard-single` already asks at random offsets). What this driver is
/// for is the prose caret.
fn word_sites(base: &Base, seed: u64) -> Vec<usize> {
    let candidates = leaves(base)
        .filter(|token| {
            token.kind() == SyntaxKind::WORD
                && token.text().len() >= KEYSTROKES + 3
                && token.text().chars().all(|c| c.is_ascii_alphabetic())
        })
        .map(|token| usize::from(token.text_range().start()) + 1)
        .collect();
    pick(candidates, seed)
}

/// Offsets inside a raw capture: a `VERBATIM_BODY` or an attached/self-delimited
/// `VERB`.
///
/// The site is just inside the leaf's end rather than at its middle, because a
/// `VERB` token carries its own opening delimiter (`\verb|`) and its middle can
/// land inside that; its end never can.
fn protected_sites(base: &Base, seed: u64) -> Vec<usize> {
    let candidates = leaves(base)
        .filter(|token| {
            matches!(token.kind(), SyntaxKind::VERBATIM_BODY | SyntaxKind::VERB)
                && token.text().len() >= 3
        })
        .filter_map(|token| {
            let text = token.text();
            let at = char_boundary_at_or_below(text, text.len() - 1);
            (at > 0).then(|| usize::from(token.text_range().start()) + at)
        })
        .collect();
    pick(candidates, seed)
}

/// Take up to [`TYPING_SITES`] candidates, spread across the file rather than
/// clustered: a corpus file's first three words are its `\documentclass` line in
/// every file, which would measure one construct 6205 times.
fn pick(candidates: Vec<usize>, seed: u64) -> Vec<usize> {
    if candidates.len() <= TYPING_SITES {
        return candidates;
    }
    let mut rng = Lcg::new(seed);
    let stride = candidates.len() / TYPING_SITES;
    (0..TYPING_SITES)
        .map(|i| candidates[i * stride + rng.below(stride)])
        .collect()
}

fn leaves(base: &Base) -> impl Iterator<Item = SyntaxToken> {
    base.syntax()
        .descendants_with_tokens()
        .filter_map(|element| element.into_token())
}

// --- corpus discovery ------------------------------------------------------

fn corpora_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("corpora")
}

/// Which corpora to sweep. `BADNESS_REPARSE_SWEEP_CORPORA` restricts the set so
/// the script can take the same argument list `check_gate_baselines.sh` does, and
/// so a local iteration can run one corpus.
fn selected_corpora() -> Vec<String> {
    match std::env::var("BADNESS_REPARSE_SWEEP_CORPORA") {
        Ok(list) if !list.trim().is_empty() => list
            .split([',', ' '])
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
        _ => CORPORA.iter().map(|s| s.to_string()).collect(),
    }
}

/// `(corpus-relative path, absolute path, contents)`, sorted by the relative path
/// so the sweep is independent of directory listing order.
fn collect_files(dir: &Path) -> Vec<(String, PathBuf, String)> {
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf, String)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        if path.is_dir() {
            walk(root, &path, out);
            continue;
        }
        // `.bib` is deliberately absent: the reparse tiers splice the LaTeX tree,
        // and the bib parser has none of them.
        let ext = path.extension().and_then(|e| e.to_str());
        if !matches!(ext, Some("tex" | "sty" | "cls" | "dtx" | "ins")) {
            continue;
        }
        // A file that is not UTF-8 is skipped rather than lossily decoded: the
        // oracle compares byte offsets, so a substituted character would make the
        // edit under test a different edit from the one reported.
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        out.push((rel, path, text));
    }
}

/// FNV-1a over the corpus-relative path.
///
/// Stable across machines and checkout locations, which the recorded tallies
/// depend on — the in-crate harness seeds from `path.as_os_str().len()`, which is
/// fine for an assertion but would make a baseline a property of where the repo
/// lives.
fn path_seed(rel: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in rel.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}
