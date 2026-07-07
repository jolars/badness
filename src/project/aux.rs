//! Read the compiler's `.aux` files: resolved label numbers (`\newlabel`) and
//! toc entries (`\@writefile{toc}{\contentsline …}`), plus `\@input` links to
//! per-chapter aux files.
//!
//! **LSP-only, like [`super::texmf`].** The `.aux` is a project-local build
//! artifact read to enrich hover and document symbols with the numbers the last
//! compile assigned; it must never feed the formatter or linter (the formatter's
//! output is a pure function of the input — see AGENTS.md, "Non-goals").
//!
//! A dedicated line-oriented scanner, deliberately **not** the LaTeX parser: aux
//! files are machine-generated under `\makeatletter`, so their commands
//! (`\@input`, `\@writefile`, `\caption@xref`) contain `@`, which the
//! catcode-faithful lexer treats as a non-letter outside `\makeatletter` regions.
//! A small brace matcher over the raw text is simpler and immune to that.
//!
//! Number extraction mirrors texlab (`crates/base-db/src/semantics/auxiliary.rs`):
//! from `\newlabel{key}{{num}{page}…}`'s second argument, take the first
//! top-level `{…}` whose content starts with text (skipping `\caption@xref`-style
//! command groups and empty groups) and strip any remaining braces, so
//! ntheorem's `{1.{1}}` yields `1.1`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use smol_str::SmolStr;

/// Aux facts for one document: everything hover and document symbols consume.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuxData {
    /// `\newlabel{key}{{num}…}`: label key → resolved number (`"1.2"`).
    pub labels: HashMap<SmolStr, String>,
    /// `\@writefile{toc}{\contentsline …}` entries, in document order.
    pub toc: Vec<TocEntry>,
}

/// One `\contentsline` written to the toc stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocEntry {
    /// The sectioning unit name as written (`part`, `chapter`, `section`, …).
    pub level: SmolStr,
    /// The `\numberline{…}` number; `None` for unnumbered (starred) entries.
    pub number: Option<String>,
    /// The title source after `\numberline`, as written by TeX (may contain
    /// macros, with TeX's inserted spacing — normalize before matching).
    pub title: String,
}

/// One parsed `.aux` file: its facts plus the `\@input{…}` targets it pulls in
/// (per-chapter aux files under `\include`), not yet resolved to paths.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ParsedAux {
    pub(crate) data: AuxData,
    pub(crate) inputs: Vec<String>,
}

/// Scan one `.aux` text. Never fails: unrecognized or malformed lines are
/// skipped (an aborted compile truncates the file mid-entry).
pub(crate) fn parse_aux(text: &str) -> ParsedAux {
    let mut out = ParsedAux::default();
    let mut i = 0;
    while let Some(off) = text[i..].find('\\') {
        let start = i + off + 1;
        i = if let Some(rest) = command_at(text, start, "newlabel") {
            newlabel(text, rest, &mut out).unwrap_or(start)
        } else if let Some(rest) = command_at(text, start, "@writefile") {
            writefile(text, rest, &mut out).unwrap_or(start)
        } else if let Some(rest) = command_at(text, start, "@input") {
            input(text, rest, &mut out).unwrap_or(start)
        } else {
            start
        };
    }
    out
}

/// If the control word at `start` (just past the `\`) is exactly `name`, return
/// the offset past it; aux command names are letters and `@`.
fn command_at(text: &str, start: usize, name: &str) -> Option<usize> {
    let rest = &text[start..];
    if !rest.starts_with(name) {
        return None;
    }
    let end = start + name.len();
    match text.as_bytes().get(end) {
        Some(b) if b.is_ascii_alphabetic() || *b == b'@' => None,
        _ => Some(end),
    }
}

/// `\newlabel{key}{{num}{page}…}`: record `key → num`. Returns the offset past
/// the second group.
fn newlabel(text: &str, at: usize, out: &mut ParsedAux) -> Option<usize> {
    let (key, at) = group(text, at)?;
    let (value, end) = group(text, at)?;
    let key = key.trim();
    if !key.is_empty()
        && let Some(number) = extract_number(value)
    {
        out.data.labels.insert(SmolStr::new(key), number);
    }
    Some(end)
}

/// `\@writefile{toc}{\contentsline {level}{\numberline {num}Title}{page}…}`:
/// record a [`TocEntry`] per `\contentsline` in a toc-stream group. Other
/// streams (`lof`, `lot`, …) are ignored. Returns the offset past the payload.
fn writefile(text: &str, at: usize, out: &mut ParsedAux) -> Option<usize> {
    let (stream, at) = group(text, at)?;
    let (payload, end) = group(text, at)?;
    if stream.trim() != "toc" {
        return Some(end);
    }
    let mut i = 0;
    while let Some(off) = payload[i..].find('\\') {
        let start = i + off + 1;
        i = match command_at(payload, start, "contentsline").and_then(|rest| {
            let (level, rest) = group(payload, rest)?;
            let (heading, entry_end) = group(payload, rest)?;
            let level = level.trim();
            if !level.is_empty() {
                let (number, title) = split_numberline(heading);
                out.data.toc.push(TocEntry {
                    level: SmolStr::new(level),
                    number,
                    title: title.trim().to_owned(),
                });
            }
            Some(entry_end)
        }) {
            Some(next) => next,
            None => start,
        };
    }
    Some(end)
}

/// `\@input{chapter.aux}`: record the target. Returns the offset past the group.
fn input(text: &str, at: usize, out: &mut ParsedAux) -> Option<usize> {
    let (target, end) = group(text, at)?;
    let target = target.trim();
    if !target.is_empty() {
        out.inputs.push(target.to_owned());
    }
    Some(end)
}

/// The next `{…}` group at or after `at` (skipping ASCII whitespace): its inner
/// content and the offset past the closing `}`. `None` when the next
/// non-whitespace byte is not `{` or the group never closes (truncated file).
fn group(text: &str, at: usize) -> Option<(&str, usize)> {
    let bytes = text.as_bytes();
    let mut i = at;
    while bytes.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
        i += 1;
    }
    if bytes.get(i) != Some(&b'{') {
        return None;
    }
    let mut depth = 0usize;
    let open = i;
    while i < bytes.len() {
        match bytes[i] {
            // Skip the escaped byte so `\{`/`\}` never count. (Skipping one byte
            // inside a multi-byte UTF-8 sequence is safe: continuation bytes are
            // ≥ 0x80 and match no arm.)
            b'\\' => i += 1,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&text[open + 1..i], i + 1));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The resolved number inside `\newlabel`'s second argument: the first
/// top-level group whose content starts with text (not a command or another
/// group), with any nested braces stripped (`1.{1}` → `1.1`).
fn extract_number(value: &str) -> Option<String> {
    let mut at = 0;
    while let Some((content, end)) = group(value, at) {
        let t = content.trim();
        if !t.is_empty() && !t.starts_with('\\') && !t.starts_with('{') {
            let number: String = t.chars().filter(|&c| c != '{' && c != '}').collect();
            let number = number.trim().to_owned();
            if !number.is_empty() {
                return Some(number);
            }
        }
        at = end;
    }
    None
}

/// Split a `\contentsline` heading group into its `\numberline{…}` number (if
/// any) and the remaining title source.
fn split_numberline(heading: &str) -> (Option<String>, &str) {
    let trimmed = heading.trim_start();
    let Some(rest) = trimmed.strip_prefix("\\numberline") else {
        return (None, heading);
    };
    // `\numberline` must end the control word (guard against `\numberlinefoo`).
    if rest.starts_with(|c: char| c.is_ascii_alphabetic() || c == '@') {
        return (None, heading);
    }
    let offset = heading.len() - rest.len();
    match group(heading, offset) {
        Some((number, end)) => {
            let number: String = number.chars().filter(|&c| c != '{' && c != '}').collect();
            let number = number.trim().to_owned();
            ((!number.is_empty()).then_some(number), &heading[end..])
        }
        None => (None, heading),
    }
}

/// Cap on the `\@input` chain from any one root aux file — real projects nest a
/// couple of levels at most; the cap only bounds a corrupt or adversarial file.
const MAX_INPUT_DEPTH: usize = 16;

/// One parsed aux file in the process-global cache, valid while the on-disk
/// `(mtime, len)` still match — the same freshness idea as the TEXMF index's
/// fingerprint, but per file, so a recompile is picked up on the next request
/// without any watcher.
struct CachedFile {
    mtime: SystemTime,
    len: u64,
    parsed: Arc<ParsedAux>,
}

fn cache() -> &'static Mutex<HashMap<PathBuf, CachedFile>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedFile>>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

/// Read and parse `path`, through the cache. `None` when the file is missing,
/// unreadable, or not a regular file. Non-UTF-8 bytes (legacy inputenc) are
/// replaced lossily — keys and numbers are ASCII in practice.
fn read_aux(path: &Path) -> Option<Arc<ParsedAux>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mtime = meta.modified().ok()?;
    let len = meta.len();
    if let Ok(map) = cache().lock()
        && let Some(cached) = map.get(path)
        && cached.mtime == mtime
        && cached.len == len
    {
        return Some(Arc::clone(&cached.parsed));
    }
    let bytes = std::fs::read(path).ok()?;
    let parsed = Arc::new(parse_aux(&String::from_utf8_lossy(&bytes)));
    if let Ok(mut map) = cache().lock() {
        map.insert(
            path.to_owned(),
            CachedFile {
                mtime,
                len,
                parsed: Arc::clone(&parsed),
            },
        );
    }
    Some(parsed)
}

/// The merged aux facts for a document's label namespace, or `None` when no
/// `.aux` exists (an uncompiled project — features degrade to numberless).
///
/// For each namespace member `foo.tex` the candidates are, first hit wins:
/// its sibling `foo.aux`; with `aux_dir` configured (resolved against
/// `root_dir`, the root document's directory, when relative), the member's
/// root-relative path under it (latexmk `-auxdir` layout), then flat
/// `{aux_dir}/foo.aux`. Each found file's `\@input` chain is followed
/// (relative to that aux file's directory), so `\include`'s per-chapter aux
/// files surface even when the namespace is incomplete. Label conflicts keep
/// the first number seen; toc entries concatenate in traversal order.
pub fn aux_data_for(
    namespace: &[&Path],
    root_dir: &Path,
    aux_dir: Option<&Path>,
) -> Option<AuxData> {
    let base = aux_dir.map(|dir| {
        if dir.is_absolute() {
            dir.to_owned()
        } else {
            root_dir.join(dir)
        }
    });

    let mut merged = AuxData::default();
    let mut visited: HashSet<PathBuf> = HashSet::new();
    for member in namespace {
        if member.extension().and_then(|e| e.to_str()) != Some("tex") {
            continue;
        }
        let mut candidates = vec![member.with_extension("aux")];
        if let Some(base) = &base {
            if let Ok(rel) = member.strip_prefix(root_dir) {
                candidates.push(base.join(rel).with_extension("aux"));
            }
            if let Some(name) = member.file_name() {
                candidates.push(base.join(name).with_extension("aux"));
            }
        }
        if let Some(found) = candidates.iter().find(|c| c.is_file()) {
            merge_chain(found, &mut merged, &mut visited, 0);
        }
    }
    (!merged.labels.is_empty() || !merged.toc.is_empty()).then_some(merged)
}

/// Merge `path`'s facts into `out`, then recurse into its `\@input` targets
/// (resolved against `path`'s directory), depth-capped and cycle-guarded.
fn merge_chain(path: &Path, out: &mut AuxData, visited: &mut HashSet<PathBuf>, depth: usize) {
    if depth > MAX_INPUT_DEPTH || !visited.insert(path.to_owned()) {
        return;
    }
    let Some(parsed) = read_aux(path) else {
        return;
    };
    for (key, number) in &parsed.data.labels {
        out.labels
            .entry(key.clone())
            .or_insert_with(|| number.clone());
    }
    out.toc.extend(parsed.data.toc.iter().cloned());
    let dir = path.parent().unwrap_or(Path::new(""));
    for target in &parsed.inputs {
        merge_chain(&dir.join(target), out, visited, depth + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels_of(text: &str) -> HashMap<SmolStr, String> {
        parse_aux(text).data.labels
    }

    #[test]
    fn simple_newlabel() {
        let labels = labels_of("\\newlabel{sec:foo}{{1}{1}}\n");
        assert_eq!(labels[&SmolStr::new("sec:foo")], "1");
    }

    #[test]
    fn newlabel_with_caption_and_anchor_groups() {
        let labels = labels_of("\\newlabel{thm:foo}{{1}{1}{Foo}{lemma.1}{}}\n");
        assert_eq!(labels[&SmolStr::new("thm:foo")], "1");
    }

    #[test]
    fn ntheorem_nested_braces_flatten() {
        let labels = labels_of("\\newlabel{thm:test}{{1.{1}}{1}}\n");
        assert_eq!(labels[&SmolStr::new("thm:test")], "1.1");
    }

    #[test]
    fn caption_xref_group_is_skipped_for_the_page_number() {
        // The first group holds a command, not text; the next textual group wins
        // (texlab behavior).
        let labels =
            labels_of("\\newlabel{fig:qux}{{\\caption@xref {fig:qux}{ on input line 15}}{1}}\n");
        assert_eq!(labels[&SmolStr::new("fig:qux")], "1");
    }

    #[test]
    fn empty_groups_are_skipped() {
        let labels = labels_of("\\newlabel{x}{{}{2}}\n");
        assert_eq!(labels[&SmolStr::new("x")], "2");
    }

    #[test]
    fn newlabel_without_number_is_absent() {
        let labels = labels_of("\\newlabel{x}{{}{}}\n\\newlabel{y}{{\\relax}{}}\n");
        assert!(labels.is_empty());
    }

    #[test]
    fn truncated_entry_is_skipped_earlier_entries_survive() {
        let labels = labels_of("\\newlabel{a}{{1}{1}}\n\\newlabel{b}{{2}{1}\n");
        assert_eq!(labels.len(), 1);
        assert_eq!(labels[&SmolStr::new("a")], "1");
    }

    #[test]
    fn similarly_named_commands_do_not_match() {
        let parsed = parse_aux("\\newlabelx{a}{{1}{1}}\n\\@inputonce{f.aux}\n");
        assert!(parsed.data.labels.is_empty());
        assert!(parsed.inputs.is_empty());
    }

    #[test]
    fn at_input_targets_collected_in_order() {
        let parsed = parse_aux("\\@input{ch1.aux}\n\\relax\n\\@input{ch2.aux}\n");
        assert_eq!(parsed.inputs, vec!["ch1.aux", "ch2.aux"]);
    }

    #[test]
    fn toc_contentsline_with_numberline() {
        let toc = parse_aux(
            "\\@writefile{toc}{\\contentsline {section}{\\numberline {1.2}Basics}{3}{section.1.2}}\n",
        )
        .data
        .toc;
        assert_eq!(toc.len(), 1);
        assert_eq!(toc[0].level, "section");
        assert_eq!(toc[0].number.as_deref(), Some("1.2"));
        assert_eq!(toc[0].title, "Basics");
    }

    #[test]
    fn starred_contentsline_has_no_number() {
        let toc =
            parse_aux("\\@writefile{toc}{\\contentsline {section}{Preface}{1}{section*.1}}\n")
                .data
                .toc;
        assert_eq!(toc.len(), 1);
        assert_eq!(toc[0].number, None);
        assert_eq!(toc[0].title, "Preface");
    }

    #[test]
    fn title_keeps_macro_source() {
        let toc = parse_aux(
            "\\@writefile{toc}{\\contentsline {section}{\\numberline {1}\\textsc  {Intro}}{1}{section.1}}\n",
        )
        .data
        .toc;
        assert_eq!(toc[0].title, "\\textsc  {Intro}");
    }

    #[test]
    fn non_toc_writefile_streams_are_ignored() {
        let parsed = parse_aux(
            "\\@writefile{lof}{\\contentsline {figure}{\\numberline {1}{\\ignorespaces A chart}}{2}{figure.1}}\n",
        );
        assert!(parsed.data.toc.is_empty());
    }

    #[test]
    fn escaped_braces_do_not_unbalance_groups() {
        let labels = labels_of("\\newlabel{k}{{1}{1}{a \\{b\\} c}{s.1}{}}\n");
        assert_eq!(labels[&SmolStr::new("k")], "1");
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn sibling_aux_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main.tex");
        write(&main, "\\documentclass{article}\n");
        write(&dir.path().join("main.aux"), "\\newlabel{sec:a}{{1}{1}}\n");
        let data = aux_data_for(&[&main], dir.path(), None).expect("aux found");
        assert_eq!(data.labels[&SmolStr::new("sec:a")], "1");
    }

    #[test]
    fn missing_aux_yields_none() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main.tex");
        write(&main, "x\n");
        assert_eq!(aux_data_for(&[&main], dir.path(), None), None);
    }

    #[test]
    fn aux_dir_candidates_flat_and_relative() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main.tex");
        let chap = dir.path().join("chapters/one.tex");
        write(&main, "x\n");
        write(&chap, "x\n");
        // latexmk -auxdir layout: root flat, includes under their relative path.
        write(
            &dir.path().join("build/main.aux"),
            "\\newlabel{sec:root}{{1}{1}}\n",
        );
        write(
            &dir.path().join("build/chapters/one.aux"),
            "\\newlabel{sec:one}{{2}{3}}\n",
        );
        let data =
            aux_data_for(&[&main, &chap], dir.path(), Some(Path::new("build"))).expect("aux found");
        assert_eq!(data.labels[&SmolStr::new("sec:root")], "1");
        assert_eq!(data.labels[&SmolStr::new("sec:one")], "2");
    }

    #[test]
    fn at_input_chain_is_followed_with_cycles_guarded() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main.tex");
        write(&main, "x\n");
        // main.aux → ch1.aux → main.aux (cycle); ch1 is *not* in the namespace.
        write(
            &dir.path().join("main.aux"),
            "\\newlabel{a}{{1}{1}}\n\\@input{ch1.aux}\n",
        );
        write(
            &dir.path().join("ch1.aux"),
            "\\newlabel{b}{{2}{2}}\n\\@input{main.aux}\n",
        );
        let data = aux_data_for(&[&main], dir.path(), None).expect("aux found");
        assert_eq!(data.labels.len(), 2);
        assert_eq!(data.labels[&SmolStr::new("b")], "2");
    }

    #[test]
    fn stale_cache_reparses_on_mtime_change() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main.tex");
        write(&main, "x\n");
        let aux = dir.path().join("main.aux");
        write(&aux, "\\newlabel{a}{{1}{1}}\n");
        let first = aux_data_for(&[&main], dir.path(), None).expect("aux found");
        assert_eq!(first.labels[&SmolStr::new("a")], "1");

        // A recompile rewrites the file; ensure the timestamp moves even on
        // coarse-mtime filesystems.
        let text = "\\newlabel{a}{{7}{1}}\n";
        write(&aux, text);
        let old = std::fs::metadata(&aux).unwrap().modified().unwrap();
        std::fs::File::options()
            .append(true)
            .open(&aux)
            .unwrap()
            .set_modified(old + std::time::Duration::from_secs(2))
            .unwrap();

        let second = aux_data_for(&[&main], dir.path(), None).expect("aux found");
        assert_eq!(second.labels[&SmolStr::new("a")], "7");
    }

    #[test]
    fn non_tex_members_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let bib = dir.path().join("refs.bib");
        write(&bib, "@article{a}\n");
        write(&dir.path().join("refs.aux"), "\\newlabel{x}{{1}{1}}\n");
        assert_eq!(aux_data_for(&[&bib], dir.path(), None), None);
    }

    #[test]
    fn realistic_aux_mix() {
        let text = "\\relax \n\
                    \\providecommand\\hyper@newdestlabel[2]{}\n\
                    \\@writefile{toc}{\\contentsline {section}{\\numberline {1}Intro}{1}{section.1}}\n\
                    \\newlabel{sec:intro}{{1}{1}{Intro}{section.1}{}}\n\
                    \\@writefile{lof}{\\contentsline {figure}{\\numberline {1}{\\ignorespaces X}}{2}{figure.1}}\n\
                    \\newlabel{fig:x}{{1}{2}{X}{figure.1}{}}\n\
                    \\@input{ch1.aux}\n\
                    \\citation{knuth1984}\n\
                    \\gdef \\@abspage@last{3}\n";
        let parsed = parse_aux(text);
        assert_eq!(parsed.data.labels.len(), 2);
        assert_eq!(parsed.data.labels[&SmolStr::new("sec:intro")], "1");
        assert_eq!(parsed.data.labels[&SmolStr::new("fig:x")], "1");
        assert_eq!(parsed.data.toc.len(), 1);
        assert_eq!(parsed.inputs, vec!["ch1.aux"]);
    }
}
