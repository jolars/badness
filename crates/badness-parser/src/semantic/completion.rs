//! Static word lists and metadata used only by completion and hover.

use std::collections::HashMap;
use std::sync::LazyLock;

use serde::Deserialize;

// The baked `.sty`/`.cls` name lists are generated from TeX Live's tlpdb. File
// order is completion rank; comments and the primary/secondary separator are not
// candidates.
const PACKAGE_NAMES_TXT: &str = include_str!("../../data/package_names.txt");
const CLASS_NAMES_TXT: &str = include_str!("../../data/class_names.txt");

static PACKAGE_NAMES: LazyLock<Vec<&'static str>> =
    LazyLock::new(|| parse_name_list(PACKAGE_NAMES_TXT));
static CLASS_NAMES: LazyLock<Vec<&'static str>> =
    LazyLock::new(|| parse_name_list(CLASS_NAMES_TXT));

fn parse_name_list(text: &'static str) -> Vec<&'static str> {
    text.lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#') && *line != "---")
        .collect()
}

/// All known `.sty` package name stems, in completion-rank order.
pub fn package_names() -> &'static [&'static str] {
    &PACKAGE_NAMES
}

/// All known `.cls` class name stems, in completion-rank order.
pub fn class_names() -> &'static [&'static str] {
    &CLASS_NAMES
}

const COLORS_JSON: &str = include_str!("../../data/colors.json");
const TIKZ_LIBRARIES_JSON: &str = include_str!("../../data/tikz_libraries.json");
const ARG_ENUMS_JSON: &str = include_str!("../../data/arg_enums.json");

#[derive(Deserialize)]
struct ColorsData {
    names: Vec<String>,
    models: Vec<String>,
}

#[derive(Deserialize)]
struct TikzLibrariesData {
    tikz: Vec<String>,
    pgf: Vec<String>,
}

static COLORS: LazyLock<ColorsData> = LazyLock::new(|| {
    serde_json::from_str(COLORS_JSON).expect("bundled data/colors.json must be valid")
});
static TIKZ_LIBRARIES: LazyLock<TikzLibrariesData> = LazyLock::new(|| {
    serde_json::from_str(TIKZ_LIBRARIES_JSON)
        .expect("bundled data/tikz_libraries.json must be valid")
});
static ARG_ENUMS: LazyLock<HashMap<String, HashMap<usize, Vec<String>>>> = LazyLock::new(|| {
    serde_json::from_str(ARG_ENUMS_JSON).expect("bundled data/arg_enums.json must be valid")
});

fn as_static_slice(names: &'static [String]) -> Vec<&'static str> {
    names.iter().map(String::as_str).collect()
}

/// Built-in color names for color-argument completion.
pub fn color_names() -> &'static [&'static str] {
    static NAMES: LazyLock<Vec<&'static str>> = LazyLock::new(|| as_static_slice(&COLORS.names));
    &NAMES
}

/// Built-in color models for `\definecolor` model completion.
pub fn color_models() -> &'static [&'static str] {
    static MODELS: LazyLock<Vec<&'static str>> = LazyLock::new(|| as_static_slice(&COLORS.models));
    &MODELS
}

/// Built-in TikZ library names.
pub fn tikz_libraries() -> &'static [&'static str] {
    static LIBS: LazyLock<Vec<&'static str>> =
        LazyLock::new(|| as_static_slice(&TIKZ_LIBRARIES.tikz));
    &LIBS
}

/// Built-in PGF library names.
pub fn pgf_libraries() -> &'static [&'static str] {
    static LIBS: LazyLock<Vec<&'static str>> =
        LazyLock::new(|| as_static_slice(&TIKZ_LIBRARIES.pgf));
    &LIBS
}

/// Suggested values for a command's brace-group argument.
pub fn arg_enum_values(name: &str, index: usize) -> Option<&'static [String]> {
    ARG_ENUMS.get(name)?.get(&index).map(Vec::as_slice)
}

type PackageMetaMap = phf::Map<&'static str, PackageMeta>;

/// Shipped CTAN metadata for one package or class stem.
#[derive(Debug, Clone, Copy)]
pub struct PackageMeta {
    pub desc: Option<&'static str>,
    pub ctan: Option<&'static str>,
}

impl PackageMeta {
    /// The canonical CTAN package page, when a catalogue id is known.
    pub fn ctan_url(&self) -> Option<String> {
        self.ctan.map(|id| format!("https://ctan.org/pkg/{id}"))
    }
}

const fn meta(desc: Option<&'static str>, ctan: Option<&'static str>) -> PackageMeta {
    PackageMeta { desc, ctan }
}

include!(concat!(env!("OUT_DIR"), "/package_metadata.rs"));

/// The shipped CTAN metadata for a package or class stem.
pub fn package_metadata(name: &str) -> Option<&'static PackageMeta> {
    PACKAGE_METADATA.get(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arg_enums_load_and_resolve() {
        assert_eq!(
            arg_enum_values("pagenumbering", 0),
            Some(
                ["arabic", "roman", "Roman", "alph", "Alph"]
                    .map(String::from)
                    .as_slice()
            )
        );
        assert!(arg_enum_values("pagestyle", 0).is_some());
        assert!(arg_enum_values("pagestyle", 1).is_none());
        assert!(arg_enum_values("definitelynotacommand", 0).is_none());
    }

    #[test]
    fn package_metadata_resolves_stem_to_ctan_facts() {
        let meta = package_metadata("amsmath").expect("amsmath in metadata DB");
        assert_eq!(meta.desc, Some("AMS mathematical facilities for LaTeX"));
        assert_eq!(
            meta.ctan_url().as_deref(),
            Some("https://ctan.org/pkg/latex-amsmath")
        );
        assert!(package_metadata("definitely-not-a-real-package").is_none());
    }
}
