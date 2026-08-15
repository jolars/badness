//! The **migration oracle** for arity-directed expl3 attachment (`AGENTS.md`
//! decision #8's staged migration, stage 2; `TODO.md` carries the plan).
//!
//! Mis-attachment is byte-invisible — losslessness and idempotence hold over a
//! wrong tree — so this harness is the one oracle that can see the migration:
//! it parses every gate-corpus file twice (production greedy vs the
//! migration-only arity entry) and diffs, per statement-leading derivable
//! head, the **semantic call-unit extent** over the greedy tree
//! (`semantic::expl3::expl3_unit`, the independent implementation the grammar
//! scan ports) against the **head node's extent** under arity attachment (the
//! grammar's actual consumption). The two walks share one collector, so
//! statement-start alignment is symmetric by construction.
//!
//! Verdict per head, keyed by the head's start byte:
//! - the semantic scan resolved a unit → the arity node must span exactly it;
//! - the semantic scan degraded to the fallback → the grammar scan must have
//!   aborted too, leaving the arity node byte-identical to the greedy one;
//! - a head present on one side only is a statement-alignment drift (a
//!   consumed-vs-skipped disagreement upstream).
//!
//! `#[ignore]`d: the corpora are fetched by `scripts/fetch_gate_corpora.sh`
//! (gitignored). Run via `scripts/check_expl3_attach_oracle.sh`, or:
//! `cargo test --release -p badness-parser --test expl3_attach_oracle -- --ignored --nocapture`.
//! `EXPL3_ORACLE_DUMP=1` writes per-head detail for every disagreement to
//! `target/expl3_attach_oracle.txt`. The test fails on any disagreement in a
//! file not covered by `tests/expl3_attach_allowlist.toml`; the harness (and
//! the allowlist) are deleted at stage 3, with every triaged class surviving
//! as a corpus fixture instead.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use badness_parser::ast::command_name;
use badness_parser::parser::{LatexFlavor, LexConfig, parse_with_expl3_arity, parse_with_flavor};
use badness_parser::semantic::expl3::{expl3_slots, expl3_unit};
use badness_parser::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};

/// What the collector records for one statement-leading derivable head.
#[derive(Clone, Copy)]
struct HeadRecord {
    /// End byte of the semantic call unit (`expl3_unit`), when it resolved.
    sem_end: Option<u32>,
    /// End byte of the head `COMMAND` node itself.
    node_end: u32,
}

/// One disagreement between the greedy-side expectation and the arity tree.
struct Disagreement {
    start: u32,
    head: String,
    class: &'static str,
    expected: Option<u32>,
    actual: Option<u32>,
}

#[test]
#[ignore = "migration oracle over the gate corpora; run via scripts/check_expl3_attach_oracle.sh"]
fn expl3_attach_oracle() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpora");
    let corpora = ["latex3", "latex2e", "pgf", "latexindent"];
    let available: Vec<_> = corpora.iter().filter(|c| root.join(c).is_dir()).collect();
    assert!(
        !available.is_empty(),
        "no gate corpora under {}; run scripts/fetch_gate_corpora.sh first",
        root.display()
    );

    let allowlist = load_allowlist();
    let dump_enabled = std::env::var_os("EXPL3_ORACLE_DUMP").is_some();
    let mut dump = String::new();

    let mut files_with_heads = 0usize;
    let mut total_heads = 0usize;
    let mut agreeing_heads = 0usize;
    let mut unexplained: Vec<String> = Vec::new();
    let mut per_class: BTreeMap<&'static str, usize> = BTreeMap::new();

    for corpus in &available {
        let mut files = Vec::new();
        collect_files(&root.join(corpus), &mut files);
        files.sort();
        for path in files {
            let Ok(text) = fs::read_to_string(&path) else {
                continue; // non-UTF-8 corpus files are out of scope
            };
            let Some(config) = lex_config_for(&path) else {
                continue;
            };
            let key = format!(
                "{corpus}/{}",
                path.strip_prefix(root.join(corpus)).unwrap().display()
            );

            let greedy = parse_with_flavor(&text, config);
            let mut greedy_heads = BTreeMap::new();
            collect_heads(&greedy.syntax(), &mut greedy_heads);
            if greedy_heads.is_empty() {
                continue; // the lexer is mode-shared, so the arity parse can hold no heads either
            }
            files_with_heads += 1;

            let arity = parse_with_expl3_arity(&text, config);
            assert_eq!(
                arity.syntax().to_string(),
                text,
                "losslessness violated under arity attachment: {key}"
            );
            let mut arity_heads = BTreeMap::new();
            collect_heads(&arity.syntax(), &mut arity_heads);

            let disagreements = diff_heads(&greedy_heads, &arity_heads);
            total_heads += greedy_heads.len();
            agreeing_heads += greedy_heads.len().saturating_sub(disagreements.len());

            if !disagreements.is_empty() {
                for d in &disagreements {
                    *per_class.entry(d.class).or_default() += 1;
                }
                if dump_enabled {
                    dump.push_str(&dump_section(&key, &text, &disagreements));
                }
                // `leftover` is the accepted class (see `diff_heads`); anything
                // else must be fixed in the scan or recorded with a reason.
                let failing = disagreements
                    .iter()
                    .filter(|d| d.class != "leftover")
                    .count();
                if failing > 0 && !allowlist.contains_key(&key) {
                    unexplained.push(format!("{key}: {failing} disagreement(s)"));
                }
            }
        }
    }

    if dump_enabled {
        let dump_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join("expl3_attach_oracle.txt");
        fs::write(&dump_path, &dump).expect("write expl3_attach_oracle.txt");
        eprintln!("expl3-attach-oracle: wrote {}", dump_path.display());
    }

    println!("expl3 attachment oracle over {:?}:", available);
    println!("  files with derivable heads: {files_with_heads}");
    println!("  statement-leading heads:    {total_heads}");
    println!("  agreeing:                   {agreeing_heads}");
    for (class, n) in &per_class {
        println!("  class {class}: {n}");
    }
    if !unexplained.is_empty() {
        println!("  unexplained files:");
        for f in &unexplained {
            println!("    {f}");
        }
    }
    assert!(
        unexplained.is_empty(),
        "{} file(s) with unexplained attachment disagreements (rerun with \
         EXPL3_ORACLE_DUMP=1 for per-head detail, then fix the scan or record \
         the file in tests/expl3_attach_allowlist.toml with a reason)",
        unexplained.len()
    );
}

/// Recursively gather corpus files with a LaTeX-family extension.
fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out);
        } else if lex_config_for(&path).is_some() {
            out.push(path);
        }
    }
}

/// The lex config a corpus file parses under, by extension — mirroring the
/// CLI's routing: `.sty`/`.cls` under the package catcode regime, `.dtx`
/// under docstrip mode, `.tex` as a document.
fn lex_config_for(path: &Path) -> Option<LexConfig> {
    match path.extension()?.to_str()? {
        "tex" => Some(LexConfig {
            flavor: LatexFlavor::Document,
            dtx: false,
        }),
        "sty" | "cls" => Some(LexConfig {
            flavor: LatexFlavor::Package,
            dtx: false,
        }),
        "dtx" => Some(LexConfig {
            flavor: LatexFlavor::Document,
            dtx: true,
        }),
        _ => None,
    }
}

/// Whether this element is a *derivable head*: a `COMMAND` whose name carries
/// a derivable argspec — the exact trigger the grammar's arity attachment
/// keys on (colon-carrying, not a `\::n` driver, `expl3_slots` resolves).
fn derivable_head(el: &SyntaxElement) -> Option<String> {
    let node = el.as_node()?;
    if node.kind() != SyntaxKind::COMMAND {
        return None;
    }
    let name = command_name(node)?;
    if name.starts_with(':') || expl3_slots(&name).is_none() {
        return None;
    }
    Some(name)
}

/// Collect every statement-leading derivable head reachable from `node`.
///
/// A `COMMAND` node's own child stream is never a statement context (its
/// children are its name and arguments), so heads are collected only from
/// non-`COMMAND` streams; recursion still descends through every node, so an
/// argument group's *body* is a context on both tree shapes. Within one
/// stream the walk advances by the semantic unit's extent (`expl3_unit` is
/// shape-agnostic — its peel queue reads a head's attached children first, so
/// it resolves the same unit over either tree), which keeps a consumed
/// sibling head from being double-counted and keeps the two sides' statement
/// starts aligned wherever they agree.
fn collect_heads(node: &SyntaxNode, out: &mut BTreeMap<u32, HeadRecord>) {
    if node.kind() != SyntaxKind::COMMAND {
        let elements: Vec<SyntaxElement> = node.children_with_tokens().collect();
        let mut i = 0;
        while i < elements.len() {
            if derivable_head(&elements[i]).is_some() {
                let start = u32::from(elements[i].text_range().start());
                let unit = expl3_unit(&elements, i);
                let record = HeadRecord {
                    sem_end: unit
                        .as_ref()
                        .map(|u| u32::from(elements[u.last].text_range().end())),
                    node_end: u32::from(elements[i].text_range().end()),
                };
                out.insert(start, record);
                i = unit.map_or(i + 1, |u| u.last + 1);
            } else {
                i += 1;
            }
        }
    }
    for child in node.children() {
        collect_heads(&child, out);
    }
}

/// Diff the greedy-side expectation against the arity tree, per head.
fn diff_heads(
    greedy: &BTreeMap<u32, HeadRecord>,
    arity: &BTreeMap<u32, HeadRecord>,
) -> Vec<Disagreement> {
    let mut out = Vec::new();
    for (start, g) in greedy {
        // Recognized on the greedy side → the arity node must span the
        // semantic unit; fallback → the grammar must have aborted too,
        // leaving the node byte-identical to the greedy one.
        let expected = g.sem_end.unwrap_or(g.node_end);
        match arity.get(start) {
            Some(a) if a.node_end == expected => {}
            // The arity node ends *inside* a recognized semantic extent: slot
            // consumption is textual-order on both sides, so a shorter node is
            // a strict prefix — the difference is material greedy attachment
            // over-attached to a consumed argument (the "leftover attached
            // group rides the statement" tolerance the segmentation carries),
            // which arity deliberately leaves as siblings. Benign for
            // attachment; the statement-extent consequence is stage 3's
            // segmentation adaptation, pinned by the formatter fixtures.
            Some(a) if g.sem_end.is_some() && a.node_end < expected => out.push(Disagreement {
                start: *start,
                head: String::new(),
                class: "leftover",
                expected: Some(expected),
                actual: Some(a.node_end),
            }),
            Some(a) => out.push(Disagreement {
                start: *start,
                head: String::new(),
                class: if g.sem_end.is_some() {
                    "extent"
                } else {
                    "recognition"
                },
                expected: Some(expected),
                actual: Some(a.node_end),
            }),
            None => out.push(Disagreement {
                start: *start,
                head: String::new(),
                class: "alignment",
                expected: Some(expected),
                actual: None,
            }),
        }
    }
    for start in arity.keys() {
        if !greedy.contains_key(start) {
            out.push(Disagreement {
                start: *start,
                head: String::new(),
                class: "alignment",
                expected: None,
                actual: Some(arity[start].node_end),
            });
        }
    }
    out
}

/// One file's section in the triage dump: per disagreement, the head name,
/// the byte extents, and the source line it sits on.
fn dump_section(key: &str, text: &str, disagreements: &[Disagreement]) -> String {
    let mut s = format!("===== {key} =====\n");
    for d in disagreements {
        let start = d.start as usize;
        let line_no = text[..start.min(text.len())].matches('\n').count() + 1;
        let line_start = text[..start.min(text.len())]
            .rfind('\n')
            .map_or(0, |i| i + 1);
        let line_end = text[line_start..]
            .find('\n')
            .map_or(text.len(), |i| line_start + i);
        let head = if d.head.is_empty() {
            let end = d
                .actual
                .or(d.expected)
                .map_or(text.len(), |e| e as usize)
                .min(text.len());
            text[start..end]
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string()
        } else {
            d.head.clone()
        };
        s.push_str(&format!(
            "  L{line_no} @{start} {head} [{}]: expected end {:?}, got {:?}\n    | {}\n",
            d.class,
            d.expected,
            d.actual,
            &text[line_start..line_end]
        ));
    }
    s
}

/// `key = "reason"` lines, the `parse_compat_allowlist.toml` idiom (a simple
/// TOML subset, hand-parsed to avoid a dev-dependency). Keys are
/// corpus-relative paths (`latex3/l3kernel/l3tl.dtx`).
fn load_allowlist() -> BTreeMap<String, String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("expl3_attach_allowlist.toml");
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
        map.insert(
            key.trim().trim_matches('"').to_string(),
            value.trim().trim_matches('"').to_string(),
        );
    }
    map
}
