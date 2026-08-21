//! Property coverage for the parsers' byte-for-byte losslessness contract.
//!
//! Curated corpora exercise realistic documents. These generators attack the
//! complementary surface: arbitrary Unicode and syntax-heavy malformed input
//! whose recovery path no author would preserve as a fixture deliberately.

use badness_parser::bib;
use badness_parser::declarations::{Declarations, ResolvedDeclarations};
use badness_parser::parser::{LatexFlavor, LexConfig, parse_with_declarations, parse_with_flavor};
use proptest::prelude::*;
use proptest::test_runner::{Config as ProptestConfig, TestCaseResult};

const DEFAULT_CASES: u32 = 256;

const LATEX_HAZARDS: &[&str] = &[
    "\\",
    "{",
    "}",
    "[",
    "]",
    "$",
    "$$",
    "%",
    "#",
    "&",
    "^",
    "_",
    "~",
    "`",
    " ",
    "\t",
    "\n",
    "\r",
    "\r\n",
    "\0",
    "\\foo",
    "\\begin{",
    "\\end{",
    "\\left",
    "\\right",
    "\\ifx",
    "\\else",
    "\\fi",
    "\\ExplSyntaxOn",
    "\\ExplSyntaxOff",
    "\\verb|",
    "\\MakeShortVerb{\\|}",
    "\\DeleteShortVerb{\\|}",
    "\\catcode`\\%=12",
    "\\begin{verbatim}\n",
    "\\end{verbatim}",
    "\\fuzzbegin",
    "\\fuzzend",
    "\\begin{fuzzverb}",
    "\\end{fuzzverb}",
    "%<*package>\n",
    "%</package>\n",
    "%    \\begin{macrocode}\n",
    "%    \\end{macrocode}\n",
];

const BIB_HAZARDS: &[&str] = &[
    "@",
    "{",
    "}",
    "(",
    ")",
    "=",
    ",",
    "#",
    "\"",
    "%",
    " ",
    "\t",
    "\n",
    "\r",
    "\r\n",
    "\0",
    "@article",
    "@string",
    "@preamble",
    "@comment",
    "title",
    "author",
    "key",
];

fn property_config() -> ProptestConfig {
    let cases = std::env::var("BADNESS_PROPERTY_CASES")
        .map(|value| {
            value
                .parse()
                .expect("BADNESS_PROPERTY_CASES must be a positive integer")
        })
        .unwrap_or(DEFAULT_CASES);
    assert!(cases > 0, "BADNESS_PROPERTY_CASES must be positive");
    ProptestConfig {
        cases,
        // A minimized source string is a more durable regression than an opaque
        // RNG seed: copy it into `roundtrip.rs` or `bib_roundtrip.rs` when found.
        failure_persistence: None,
        ..ProptestConfig::default()
    }
}

fn short_unicode() -> BoxedStrategy<String> {
    prop::collection::vec(any::<char>(), 0..9)
        .prop_map(|chars| chars.into_iter().collect())
        .boxed()
}

fn ascii_word() -> BoxedStrategy<String> {
    prop::collection::vec(prop::sample::select(('a'..='z').collect::<Vec<_>>()), 0..13)
        .prop_map(|chars| chars.into_iter().collect())
        .boxed()
}

fn arbitrary_unicode() -> BoxedStrategy<String> {
    prop::collection::vec(any::<char>(), 0..513)
        .prop_map(|chars| chars.into_iter().collect())
        .boxed()
}

fn latex_document() -> BoxedStrategy<String> {
    let leaf = prop_oneof![
        6 => prop::sample::select(LATEX_HAZARDS.to_vec()).prop_map(str::to_owned),
        3 => ascii_word(),
        1 => short_unicode(),
    ];
    let fragment = leaf.prop_recursive(4, 64, 8, |inner| {
        prop_oneof![
            inner.clone().prop_map(|body| format!("{{{body}}}")),
            inner.clone().prop_map(|body| format!("{{{body}")),
            inner.clone().prop_map(|body| format!("[{body}]")),
            inner.clone().prop_map(|body| format!("${body}$")),
            inner.clone().prop_map(|body| format!("${body}")),
            inner.clone().prop_map(|body| format!("\\foo{{{body}}}")),
            inner
                .clone()
                .prop_map(|body| format!("\\begin{{itemize}}{body}\\end{{itemize}}")),
            inner
                .clone()
                .prop_map(|body| format!("\\begin{{itemize}}{body}\\end{{align}}")),
            (inner.clone(), inner.clone())
                .prop_map(|(yes, no)| { format!("\\ifx\\foo\\bar {yes}\\else {no}\\fi") }),
            inner.prop_map(|body| format!(
                "\\ExplSyntaxOn\\tl_set:Nn \\l_tmpa_tl {{{body}}}\\ExplSyntaxOff"
            )),
        ]
    });
    prop::collection::vec(fragment, 0..33)
        .prop_map(|parts| parts.concat())
        .boxed()
}

fn bib_document() -> BoxedStrategy<String> {
    let leaf = prop_oneof![
        6 => prop::sample::select(BIB_HAZARDS.to_vec()).prop_map(str::to_owned),
        3 => ascii_word(),
        1 => short_unicode(),
    ];
    let fragment = leaf.prop_recursive(4, 64, 8, |inner| {
        prop_oneof![
            inner.clone().prop_map(|value| format!("{{{value}}}")),
            inner.clone().prop_map(|value| format!("{{{value}")),
            inner.clone().prop_map(|value| format!("\"{value}\"")),
            inner.clone().prop_map(|value| format!("\"{value}")),
            inner
                .clone()
                .prop_map(|value| format!("@misc{{key, title = {{{value}}}}}")),
            inner
                .clone()
                .prop_map(|value| format!("@string{{name = \"{value}\"}}")),
            (inner.clone(), inner).prop_map(|(left, right)| {
                format!("@article(key, title = {{{left}}} # \"{right}\")")
            }),
        ]
    });
    prop::collection::vec(fragment, 0..33)
        .prop_map(|parts| parts.concat())
        .boxed()
}

fn declared() -> ResolvedDeclarations {
    serde_json::from_str::<Declarations>(
        r#"{
            "environments": {
                "fuzzalign": {
                    "like": "align",
                    "begin": ["fuzzbegin"],
                    "end": ["fuzzend"]
                },
                "fuzzverb": {"like": "verbatim"}
            }
        }"#,
    )
    .expect("property declarations deserialize")
    .resolve()
    .expect("property declarations resolve")
}

fn assert_latex_lossless(input: &str) -> TestCaseResult {
    let configs = [
        ("document", LexConfig::from(LatexFlavor::Document)),
        ("package", LexConfig::from(LatexFlavor::Package)),
        (
            "dtx",
            LexConfig {
                flavor: LatexFlavor::Document,
                dtx: true,
            },
        ),
    ];
    for (label, config) in configs {
        let reconstructed = parse_with_flavor(input, config).syntax().to_string();
        prop_assert_eq!(reconstructed.as_str(), input, "{} parse lost bytes", label);
    }

    let declarations = declared();
    let reconstructed = parse_with_declarations(input, LatexFlavor::Document, &declarations)
        .syntax()
        .to_string();
    prop_assert_eq!(
        reconstructed.as_str(),
        input,
        "declared document parse lost bytes"
    );
    Ok(())
}

fn assert_bib_lossless(input: &str) -> TestCaseResult {
    let reconstructed = bib::parse(input).syntax().to_string();
    prop_assert_eq!(reconstructed.as_str(), input, "BibTeX parse lost bytes");
    Ok(())
}

proptest! {
    #![proptest_config(property_config())]

    #[test]
    fn latex_roundtrips_arbitrary_unicode(input in arbitrary_unicode()) {
        assert_latex_lossless(&input)?;
    }

    #[test]
    fn latex_roundtrips_syntax_heavy_input(input in latex_document()) {
        assert_latex_lossless(&input)?;
    }

    #[test]
    fn bib_roundtrips_arbitrary_unicode(input in arbitrary_unicode()) {
        assert_bib_lossless(&input)?;
    }

    #[test]
    fn bib_roundtrips_syntax_heavy_input(input in bib_document()) {
        assert_bib_lossless(&input)?;
    }
}
