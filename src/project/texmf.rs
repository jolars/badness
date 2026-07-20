//! A read-only index of the installed TEXMF tree, for **LSP-only** package
//! resolution (document links, hover, go-to-definition, and installed-set
//! completion).
//!
//! This is the one place badness looks past the document's own directory into the
//! wider TeX installation. It is deliberately fenced off from the formatter: the
//! index never feeds `scope_signatures`/`DiskPackageSource`, so `badness format`
//! output depends only on the input and the shipped data, never on what is installed
//! (the "deterministic, rule-based formatting" tenet — see `AGENTS.md`). Reading
//! `.sty`/`.cls`/`.dtx` filenames off disk is not typesetting and runs no TeX engine.
//!
//! Two halves, per the design split:
//! - **Root discovery is delegated.** Reproducing kpathsea's `texmf.cnf` resolution
//!   is a fragile rabbit hole (and MiKTeX doesn't use it), so we ask the installed
//!   tool: `kpsewhich -var-value=TEXMF{HOME,LOCAL,DIST,MAIN}`. When `kpsewhich` is
//!   absent we fall back to default-path heuristics; when nothing resolves the index
//!   is empty and resolution degrades to today's local-only behavior.
//! - **Enumeration is ours.** Given the roots, a by-name lookup needs no kpathsea: we
//!   read each root's `ls-R` filename database when present (TeX Live ships it) or
//!   walk once, building a `filename -> path` map. This yields the whole installed
//!   set (which `kpsewhich`, one file per call, cannot) for completion.
//!
//! The built index is cached to the OS cache dir keyed by a distro fingerprint (root
//! paths + `ls-R`/root mtimes) and rebuilt when that changes, so the walk runs at most
//! once per install. The process holds it in a [`OnceLock`]; the first config seen
//! wins (TEXMF is session-stable environment state).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

/// The `texmf` editor settings: how the language server discovers the installed TeX
/// tree for *LSP-only* package resolution (document links, hover, go-to-definition,
/// and installed-set completion). This never feeds the formatter — `badness format`
/// stays hermetic regardless of what is installed (see `AGENTS.md`).
///
/// Where an installation lives is a fact about the *machine*, not the project, so
/// these settings come from the editor (`initializationOptions` or
/// `workspace/didChangeConfiguration`, camelCase JSON), never from `badness.toml` —
/// a committed project config can't point at paths that only exist on one
/// contributor's system.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TexmfConfig {
    /// Whether to scan the TEXMF tree at all. When `false`, package resolution stays
    /// local to the document's directory.
    pub enabled: bool,
    /// Extra TEXMF root directories to index in addition to (and ahead of) the
    /// discovered ones. Useful for a non-standard install `kpsewhich` can't see.
    pub roots: Vec<PathBuf>,
    /// Whether to shell out to `kpsewhich -var-value=…` to discover the tree roots.
    /// When `false`, discovery falls back to default-path heuristics only.
    pub use_kpsewhich: bool,
}

impl Default for TexmfConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            roots: Vec::new(),
            use_kpsewhich: true,
        }
    }
}

/// File extensions the index tracks: package/class sources and their literate `.dtx`.
const INDEXED_EXTS: &[&str] = &["sty", "cls", "dtx"];

/// The `kpsewhich` variables naming the standard content trees, most-specific first
/// (so a user/local override shadows the distribution copy).
const TEXMF_VARS: &[&str] = &["TEXMFHOME", "TEXMFLOCAL", "TEXMFDIST", "TEXMFMAIN"];

/// An immutable `filename -> absolute path` index over one or more TEXMF roots, plus
/// the sorted `.sty`/`.cls` stem lists for completion. Cheap to clone-by-reference
/// (it lives behind a `OnceLock`); built once per install.
#[derive(Debug, Default, Clone)]
pub struct TexmfIndex {
    /// `"amsmath.sty" -> /usr/share/texmf-dist/tex/latex/amsmath/amsmath.sty`.
    by_name: HashMap<String, PathBuf>,
    /// Sorted, de-duplicated `.sty` stems (`amsmath`, `tikz`, …).
    sty_stems: Vec<String>,
    /// Sorted, de-duplicated `.cls` stems (`article`, `beamer`, …).
    cls_stems: Vec<String>,
}

impl TexmfIndex {
    /// Build an index over `roots` by reading each root's `ls-R` (or walking it when
    /// absent). Earlier roots win a filename collision (local shadows distribution).
    pub fn build_from_roots(roots: &[PathBuf]) -> Self {
        let mut by_name: HashMap<String, PathBuf> = HashMap::new();
        for root in roots {
            match read_ls_r(root) {
                Some(entries) => {
                    for (name, path) in entries {
                        by_name.entry(name).or_insert(path);
                    }
                }
                None => walk_root(root, &mut by_name),
            }
        }
        Self::from_files(by_name)
    }

    /// Assemble the index (and its derived stem lists) from a `filename -> path` map.
    fn from_files(by_name: HashMap<String, PathBuf>) -> Self {
        let sty_stems = sorted_stems(&by_name, "sty");
        let cls_stems = sorted_stems(&by_name, "cls");
        Self {
            by_name,
            sty_stems,
            cls_stems,
        }
    }

    /// The installed path for `stem` under the first of `exts` that exists in the
    /// tree, or `None`. `exts` are tried in order (e.g. `["sty", "dtx"]` for a
    /// package with a `.dtx`-only literate source).
    pub fn resolve(&self, stem: &str, exts: &[&str]) -> Option<&Path> {
        for ext in exts {
            if let Some(path) = self.by_name.get(&format!("{stem}.{ext}")) {
                return Some(path);
            }
        }
        None
    }

    /// The sorted `.sty` stems, for `\usepackage` installed-set completion.
    pub fn sty_stems(&self) -> &[String] {
        &self.sty_stems
    }

    /// The sorted `.cls` stems, for `\documentclass` installed-set completion.
    pub fn cls_stems(&self) -> &[String] {
        &self.cls_stems
    }

    /// Whether the index found nothing (no TeX install, or scanning disabled). Callers
    /// treat an empty index the same as no index — resolution stays local-only.
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

/// The process-wide TEXMF index, built lazily on first use. `config.enabled == false`
/// yields an empty index. The first call's config wins (TEXMF is session-stable), so
/// later `[texmf]` root/`use-kpsewhich` changes need a server restart. The `enabled`
/// toggle, however, is honored per call: a disabled config always yields the empty
/// index without touching (or being shadowed by) the built one, so a document that
/// opts out is never served installed-tree links even if another already opted in.
pub fn global_index(config: &TexmfConfig) -> &'static TexmfIndex {
    static EMPTY: OnceLock<TexmfIndex> = OnceLock::new();
    static GLOBAL: OnceLock<TexmfIndex> = OnceLock::new();
    if !config.enabled {
        return EMPTY.get_or_init(TexmfIndex::default);
    }
    GLOBAL.get_or_init(|| load_or_build(config))
}

/// Discover roots, then return the cached index when the distro fingerprint matches,
/// else build fresh and cache it. An empty root set yields an empty (uncached) index.
fn load_or_build(config: &TexmfConfig) -> TexmfIndex {
    let roots = discover_roots(config);
    if roots.is_empty() {
        return TexmfIndex::default();
    }
    let fingerprint = fingerprint(&roots);
    if let Some(files) = load_cache(&fingerprint) {
        return TexmfIndex::from_files(files);
    }
    let index = TexmfIndex::build_from_roots(&roots);
    save_cache(&fingerprint, &index.by_name);
    index
}

/// The TEXMF root directories to index: the configured extras first, then the trees
/// `kpsewhich` reports, then default-path heuristics when `kpsewhich` yields nothing.
/// Only existing directories are kept, de-duplicated in first-seen order.
fn discover_roots(config: &TexmfConfig) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let push = |roots: &mut Vec<PathBuf>, dir: PathBuf| {
        if dir.is_dir() && !roots.contains(&dir) {
            roots.push(dir);
        }
    };

    for extra in &config.roots {
        push(&mut roots, extra.clone());
    }
    if config.use_kpsewhich {
        for var in TEXMF_VARS {
            if let Some(dir) = kpsewhich_var(var) {
                push(&mut roots, dir);
            }
        }
    }
    if roots.is_empty() {
        for dir in heuristic_roots() {
            push(&mut roots, dir);
        }
    }
    roots
}

/// Query one `kpsewhich -var-value=<var>`; `None` when `kpsewhich` is missing, errors,
/// or prints nothing (an unset variable).
fn kpsewhich_var(var: &str) -> Option<PathBuf> {
    let output = Command::new("kpsewhich")
        .arg(format!("-var-value={var}"))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// Best-effort default TEXMF locations for when no `kpsewhich` is on `PATH`. Not
/// exhaustive — the "pure mimic" tier; a real install almost always ships `kpsewhich`.
fn heuristic_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = dirs::home_dir() {
        out.push(home.join("texmf"));
    }
    out.push(PathBuf::from("/usr/share/texmf-dist"));
    out.push(PathBuf::from("/usr/local/share/texmf-dist"));
    out.push(PathBuf::from("/usr/share/texmf"));
    out
}

/// A change-detection string for `roots`: each root path plus the mtime of its `ls-R`
/// (or the root dir itself when there is none). A differing fingerprint forces a
/// rebuild, so an install/removal invalidates the cache.
fn fingerprint(roots: &[PathBuf]) -> String {
    let mut parts = Vec::new();
    for root in roots {
        let ls_r = root.join("ls-R");
        let anchor = if ls_r.is_file() { ls_r } else { root.clone() };
        let secs = anchor
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        parts.push(format!("{}@{}", root.display(), secs));
    }
    parts.join(";")
}

/// The on-disk cache file (`<cache>/badness/texmf-index.json`), or `None` when no OS
/// cache dir is available.
fn cache_path() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("badness").join("texmf-index.json"))
}

/// The cached `filename -> path` map when the stored fingerprint matches `fingerprint`.
fn load_cache(fingerprint: &str) -> Option<HashMap<String, PathBuf>> {
    load_cache_from(&cache_path()?, fingerprint)
}

/// Persist `files` under `fingerprint`. Best-effort: any I/O error is ignored (the
/// index still works this session, just uncached).
fn save_cache(fingerprint: &str, files: &HashMap<String, PathBuf>) {
    let Some(path) = cache_path() else { return };
    save_cache_to(&path, fingerprint, files);
}

/// [`load_cache`] against an explicit path (the testable core).
fn load_cache_from(path: &Path, fingerprint: &str) -> Option<HashMap<String, PathBuf>> {
    let text = std::fs::read_to_string(path).ok()?;
    let cached: CachedIndex = serde_json::from_str(&text).ok()?;
    (cached.fingerprint == fingerprint).then_some(cached.files)
}

/// [`save_cache`] against an explicit path (the testable core).
fn save_cache_to(path: &Path, fingerprint: &str, files: &HashMap<String, PathBuf>) {
    let cached = CachedIndex {
        fingerprint: fingerprint.to_string(),
        files: files.clone(),
    };
    let Ok(text) = serde_json::to_string(&cached) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, text);
}

/// The serialized cache: the distro fingerprint plus the `filename -> path` map (the
/// stem lists are recomputed on load, so they aren't stored).
#[derive(Serialize, Deserialize)]
struct CachedIndex {
    fingerprint: String,
    files: HashMap<String, PathBuf>,
}

/// Parse a root's `ls-R` filename database into `(filename, absolute path)` pairs for
/// the indexed extensions, or `None` when the root has no `ls-R`.
///
/// The format is a flat listing: a line `./sub/dir:` opens a directory (relative to
/// the root), and the following non-empty, non-`%`-comment lines are its filenames
/// until the next directory header or blank line.
fn read_ls_r(root: &Path) -> Option<Vec<(String, PathBuf)>> {
    let text = std::fs::read_to_string(root.join("ls-R")).ok()?;
    let mut out = Vec::new();
    let mut dir = PathBuf::from(root);
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('%') {
            continue;
        }
        if let Some(rel) = line.strip_suffix(':') {
            dir = root.join(rel.trim_start_matches("./"));
        } else if has_indexed_ext(line) {
            out.push((line.to_string(), dir.join(line)));
        }
    }
    Some(out)
}

/// Walk `root` for indexed files, inserting each `filename -> path` (first-seen wins),
/// used when a root has no `ls-R`. Standard ignore filters (`.gitignore`, hidden) are
/// disabled — a TEXMF tree is not a source repo.
fn walk_root(root: &Path, by_name: &mut HashMap<String, PathBuf>) {
    if !root.is_dir() {
        return;
    }
    for entry in ignore::WalkBuilder::new(root)
        .standard_filters(false)
        .build()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|n| n.to_str())
            && has_indexed_ext(name)
        {
            by_name
                .entry(name.to_string())
                .or_insert_with(|| path.to_path_buf());
        }
    }
}

/// Whether `filename` ends in one of the [`INDEXED_EXTS`] (ASCII-lowercased match).
fn has_indexed_ext(filename: &str) -> bool {
    Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| INDEXED_EXTS.contains(&e.as_str()))
}

/// The sorted, de-duplicated stems of every `filename.<ext>` key in `by_name`.
fn sorted_stems(by_name: &HashMap<String, PathBuf>, ext: &str) -> Vec<String> {
    let suffix = format!(".{ext}");
    let mut stems: Vec<String> = by_name
        .keys()
        .filter_map(|name| name.strip_suffix(&suffix))
        .map(str::to_string)
        .collect();
    stems.sort_unstable();
    stems.dedup();
    stems
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temp TEXMF root with a couple of package/class files laid out in subdirs.
    fn fixture_tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let latex = dir.path().join("tex/latex");
        std::fs::create_dir_all(latex.join("amsmath")).unwrap();
        std::fs::create_dir_all(latex.join("koma-script")).unwrap();
        std::fs::write(latex.join("amsmath/amsmath.sty"), "").unwrap();
        std::fs::write(latex.join("amsmath/amstext.sty"), "").unwrap();
        std::fs::write(latex.join("koma-script/scrartcl.cls"), "").unwrap();
        // A non-indexed file must be ignored.
        std::fs::write(latex.join("amsmath/amsmath.pdf"), "").unwrap();
        dir
    }

    #[test]
    fn walk_indexes_sty_and_cls_by_name() {
        let dir = fixture_tree();
        let index = TexmfIndex::build_from_roots(&[dir.path().to_path_buf()]);
        assert!(!index.is_empty());
        assert_eq!(
            index.resolve("amsmath", &["sty"]),
            Some(dir.path().join("tex/latex/amsmath/amsmath.sty").as_path())
        );
        assert_eq!(
            index.resolve("scrartcl", &["cls"]),
            Some(
                dir.path()
                    .join("tex/latex/koma-script/scrartcl.cls")
                    .as_path()
            )
        );
        // A `.pdf` sibling never enters the index.
        assert!(index.resolve("amsmath", &["pdf"]).is_none());
        assert_eq!(index.sty_stems(), &["amsmath", "amstext"]);
        assert_eq!(index.cls_stems(), &["scrartcl"]);
    }

    #[test]
    fn ls_r_is_preferred_over_walking() {
        let dir = tempfile::tempdir().unwrap();
        // No real files on disk: only an `ls-R` describing them. If `read_ls_r` is
        // used, the index is populated; a disk walk would find nothing.
        std::fs::write(
            dir.path().join("ls-R"),
            "% ls-R -- filename database\n./tex/latex/booktabs:\nbooktabs.sty\n",
        )
        .unwrap();
        let index = TexmfIndex::build_from_roots(&[dir.path().to_path_buf()]);
        assert_eq!(
            index.resolve("booktabs", &["sty"]),
            Some(dir.path().join("tex/latex/booktabs/booktabs.sty").as_path())
        );
    }

    #[test]
    fn resolve_tries_extensions_in_order() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("d")).unwrap();
        std::fs::write(dir.path().join("d/foo.dtx"), "").unwrap();
        let index = TexmfIndex::build_from_roots(&[dir.path().to_path_buf()]);
        // `.sty` is absent, so the `.dtx` literate source is the fallback hit.
        assert_eq!(
            index.resolve("foo", &["sty", "dtx"]),
            Some(dir.path().join("d/foo.dtx").as_path())
        );
    }

    #[test]
    fn cache_round_trips_and_invalidates_on_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("badness/texmf-index.json");
        let files: HashMap<String, PathBuf> =
            HashMap::from([("amsmath.sty".to_string(), PathBuf::from("/t/amsmath.sty"))]);
        save_cache_to(&path, "fingerprint-abc", &files);
        // Same fingerprint round-trips.
        assert_eq!(
            load_cache_from(&path, "fingerprint-abc"),
            Some(files.clone())
        );
        // A different fingerprint misses (stale cache is not returned).
        assert_eq!(load_cache_from(&path, "fingerprint-xyz"), None);
    }
}
