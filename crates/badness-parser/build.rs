//! Build script: bake the bulk CWL signature tier into the binary as a `phf` map.
//!
//! `data/cwl_signatures.json` is the reviewable, generated source of truth (see
//! `scripts/gen_cwl_signatures.py`). At ~400 KB it is too large to parse at
//! runtime — decompressing+parsing it cost ~4.5 ms once per process, which
//! dominated small-document CLI latency (see `benches/README.md`). Instead we
//! generate, at build time, a perfect-hash `phf::Map` whose values are calls to
//! the `const fn` constructors in `src/semantic/signature.rs` (`command`,
//! `environment`, `arg`). The result is read-only static data with O(1) lookup
//! and *zero* runtime parse or decompress.
//!
//! The deserialize schema below mirrors the `Raw*` types in
//! `src/semantic/signature.rs`; the `reflow`/`block` *derivations* are NOT
//! duplicated here — they live in the `environment` const fn, applied at the
//! generated call site, so the JSON path and this codegen path can never differ.

use std::collections::BTreeMap;
use std::env;
use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use serde::Deserialize;

/// `"req"` (mandatory `{…}`) or `"opt"` (optional `[…]`), as written in the JSON.
#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum RawArgKind {
    Req,
    Opt,
}

/// An argument's content kind as written in the JSON: `"opaque"` (default),
/// `"prose"`, `"tokenList"`, or `"keyval"`. Mirrors `ContentKind`.
#[derive(Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "camelCase")]
enum RawContentKind {
    #[default]
    Opaque,
    Prose,
    TokenList,
    Keyval,
}

#[derive(Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "lowercase")]
enum RawArgumentDomain {
    #[default]
    Unknown,
}

/// A normalized math atom class as written in `data/math_symbols.json`.
#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum RawMathClass {
    Ord,
    Op,
    Bin,
    Rel,
    Open,
    Close,
    Punct,
    Fence,
}

impl RawMathClass {
    fn variant(self) -> &'static str {
        match self {
            Self::Ord => "MathClass::Ord",
            Self::Op => "MathClass::Op",
            Self::Bin => "MathClass::Bin",
            Self::Rel => "MathClass::Rel",
            Self::Open => "MathClass::Open",
            Self::Close => "MathClass::Close",
            Self::Punct => "MathClass::Punct",
            Self::Fence => "MathClass::Fence",
        }
    }
}

/// An upstream delimiter-shaped role before Badness's curated exceptions.
#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum RawDelimiterRole {
    Open,
    Close,
    Fence,
}

impl RawDelimiterRole {
    fn variant(self) -> &'static str {
        match self {
            Self::Open => "DelimiterRole::Open",
            Self::Close => "DelimiterRole::Close",
            Self::Fence => "DelimiterRole::Fence",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMathSymbols {
    #[serde(rename = "_comment")]
    _comment: serde::de::IgnoredAny,
    source: RawMathSource,
    symbols: Vec<(String, String, RawMathClass, Option<RawDelimiterRole>)>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMathSource {
    repository: String,
    revision: String,
    path: String,
    license: String,
}

impl RawContentKind {
    fn variant(self) -> &'static str {
        match self {
            RawContentKind::Opaque => "ContentKind::Opaque",
            RawContentKind::Prose => "ContentKind::Prose",
            RawContentKind::TokenList => "ContentKind::TokenList",
            RawContentKind::Keyval => "ContentKind::Keyval",
        }
    }
}

/// Compact (`"req"`/`"opt"`) or object (`{ "kind": …, "content": … }`).
#[derive(Deserialize)]
#[serde(untagged)]
enum RawArg {
    Short(RawArgKind),
    Full {
        kind: RawArgKind,
        #[serde(default)]
        content: RawContentKind,
        #[serde(default)]
        #[serde(rename = "domain")]
        _domain: RawArgumentDomain,
        /// Accepted for schema parity, but behavior facts never enter the
        /// arity-only CWL tier.
        #[serde(default, rename = "verbatim")]
        _verbatim: bool,
    },
}

impl RawArg {
    /// Render as an `arg(required, ArgKind::…, ContentKind::…)` const-fn call.
    fn render(&self) -> String {
        let (kind, content) = match self {
            RawArg::Short(kind) => (*kind, RawContentKind::Opaque),
            RawArg::Full {
                kind,
                content,
                _domain: _,
                _verbatim: _,
            } => (*kind, *content),
        };
        let (required, kind) = match kind {
            RawArgKind::Req => (true, "ArgKind::Brace"),
            RawArgKind::Opt => (false, "ArgKind::Bracket"),
        };
        format!("arg({required}, {kind}, {})", content.variant())
    }
}

fn render_args(args: &[RawArg]) -> String {
    let mut out = String::from("&[");
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&a.render());
    }
    out.push(']');
    out
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawCommand {
    #[serde(default)]
    args: Vec<RawArg>,
    #[serde(default)]
    sectioning: Option<u8>,
    #[serde(default)]
    verbatim: bool,
    #[serde(default)]
    rule: bool,
    #[serde(default)]
    inline: bool,
}

impl RawCommand {
    /// `command(&[…], sectioning, verbatim, rule, inline)`.
    fn render(&self) -> String {
        let sectioning = match self.sectioning {
            Some(n) => format!("Some({n}u8)"),
            None => "None".to_string(),
        };
        format!(
            "command({}, {}, {}, {}, {})",
            render_args(&self.args),
            sectioning,
            self.verbatim,
            self.rule,
            self.inline,
        )
    }
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum RawOutlineKind {
    Float,
    Theorem,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawEnvironment {
    #[serde(default)]
    args: Vec<RawArg>,
    #[serde(default, rename = "verbatimBody")]
    verbatim_body: bool,
    #[serde(default)]
    math: bool,
    #[serde(default)]
    code: bool,
    #[serde(default)]
    align: bool,
    #[serde(default, rename = "noIndent")]
    no_indent: bool,
    #[serde(default)]
    list: bool,
    #[serde(default)]
    block: bool,
    #[serde(default)]
    outline: Option<RawOutlineKind>,
}

impl RawEnvironment {
    /// `environment(&[…], verbatim_body, math, code, align, no_indent, list,
    /// block_explicit, outline)` — the explicit source facts stored by the const fn.
    fn render(&self) -> String {
        let outline = match self.outline {
            Some(RawOutlineKind::Float) => "Some(OutlineKind::Float)",
            Some(RawOutlineKind::Theorem) => "Some(OutlineKind::Theorem)",
            None => "None",
        };
        format!(
            "environment({}, {}, {}, {}, {}, {}, {}, {}, {})",
            render_args(&self.args),
            self.verbatim_body,
            self.math,
            self.code,
            self.align,
            self.no_indent,
            self.list,
            self.block,
            outline,
        )
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawDb {
    #[serde(default, rename = "_comment")]
    _comment: Option<serde::de::IgnoredAny>,
    // BTreeMaps so the generated source is deterministic (sorted) across builds.
    #[serde(default)]
    commands: BTreeMap<String, RawCommand>,
    #[serde(default)]
    environments: BTreeMap<String, RawEnvironment>,
}

/// Bake the bulk CWL signature tier into `$OUT_DIR/cwl_signatures.rs` as a `phf`
/// map (see the module docs).
fn generate_cwl_signatures() {
    let json = std::fs::read_to_string("data/cwl_signatures.json")
        .expect("data/cwl_signatures.json must exist (run `task cwl:sync`)");
    let db: RawDb = serde_json::from_str(&json).expect("data/cwl_signatures.json must be valid");

    let mut commands = phf_codegen::Map::new();
    for (name, sig) in &db.commands {
        commands.entry(name.as_str(), sig.render());
    }
    let mut environments = phf_codegen::Map::new();
    for (name, sig) in &db.environments {
        environments.entry(name.as_str(), sig.render());
    }

    let mut out = String::new();
    writeln!(
        out,
        "// @generated by build.rs from data/cwl_signatures.json — do not edit."
    )
    .unwrap();
    writeln!(
        out,
        "static CWL_COMMANDS: CwlSigMap<CommandSig> = {};",
        commands.build()
    )
    .unwrap();
    writeln!(
        out,
        "static CWL_ENVIRONMENTS: CwlSigMap<EnvironmentSig> = {};",
        environments.build()
    )
    .unwrap();

    let path = Path::new(&env::var("OUT_DIR").unwrap()).join("cwl_signatures.rs");
    let mut file = BufWriter::new(File::create(&path).unwrap());
    file.write_all(out.as_bytes()).unwrap();
}

/// The on-disk shape of `data/package_metadata.json`: a `note` header (ignored) plus
/// the `stem -> {desc?, ctan?}` entries.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawMetaFile {
    #[serde(default, rename = "note")]
    _note: Option<serde::de::IgnoredAny>,
    // BTreeMap so the generated source is deterministic (sorted) across builds.
    #[serde(default)]
    entries: BTreeMap<String, RawMeta>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawMeta {
    #[serde(default)]
    desc: Option<String>,
    #[serde(default)]
    ctan: Option<String>,
}

impl RawMeta {
    /// `meta(Some("desc"), Some("ctan"))` — a call to the `const fn` constructor,
    /// with each field an escaped Rust string literal (via `{:?}`) or `None`.
    fn render(&self) -> String {
        format!(
            "meta({}, {})",
            render_opt(&self.desc),
            render_opt(&self.ctan)
        )
    }
}

/// Render an optional string as `Some("…")` (escaped via `{:?}`) or `None`.
fn render_opt(value: &Option<String>) -> String {
    match value {
        Some(s) => format!("Some({s:?})"),
        None => "None".to_string(),
    }
}

/// Bake the CTAN metadata tier into `$OUT_DIR/package_metadata.rs` as a `phf` map,
/// mirroring [`generate_cwl_signatures`]: a `stem -> PackageMeta` perfect-hash map of
/// `const fn` constructor calls, so there is zero runtime parse (the JSON is ~730 KB —
/// larger than the CWL tier, whose runtime parse we already eliminated for startup).
fn generate_package_metadata() {
    let json = std::fs::read_to_string("data/package_metadata.json")
        .expect("data/package_metadata.json must exist (run `task pkg-names:sync`)");
    let file: RawMetaFile =
        serde_json::from_str(&json).expect("data/package_metadata.json must be valid");

    let mut map = phf_codegen::Map::new();
    for (stem, meta) in &file.entries {
        map.entry(stem.as_str(), meta.render());
    }

    let mut out = String::new();
    writeln!(
        out,
        "// @generated by build.rs from data/package_metadata.json — do not edit."
    )
    .unwrap();
    writeln!(
        out,
        "static PACKAGE_METADATA: PackageMetaMap = {};",
        map.build()
    )
    .unwrap();

    let path = Path::new(&env::var("OUT_DIR").unwrap()).join("package_metadata.rs");
    let mut file = BufWriter::new(File::create(&path).unwrap());
    file.write_all(out.as_bytes()).unwrap();
}

/// Bake the generated unicode-math baseline into a command PHF and a sorted
/// character table. Duplicate code points are intentional aliases; later rows
/// win for direct-character lookup, matching the table's processing order.
fn generate_math_symbols() {
    let json = std::fs::read_to_string("data/math_symbols.json")
        .expect("data/math_symbols.json must exist (run `task math-symbols:sync`)");
    let file: RawMathSymbols =
        serde_json::from_str(&json).expect("data/math_symbols.json must be valid");
    assert!(!file.source.repository.is_empty());
    assert!(!file.source.revision.is_empty());
    assert!(!file.source.path.is_empty());
    assert!(!file.source.license.is_empty());

    let render_info = |class: RawMathClass, delimiter: Option<RawDelimiterRole>| {
        let delimiter = delimiter.map_or("None".to_owned(), |role| {
            format!("Some({})", role.variant())
        });
        format!("info({}, {delimiter})", class.variant())
    };

    let mut commands = phf_codegen::Map::new();
    let mut characters = BTreeMap::new();
    for (codepoint, command, class, delimiter) in &file.symbols {
        commands.entry(command.as_str(), render_info(*class, *delimiter));
        let value = u32::from_str_radix(codepoint, 16)
            .unwrap_or_else(|_| panic!("invalid math-symbol code point `{codepoint}`"));
        let character =
            char::from_u32(value).unwrap_or_else(|| panic!("invalid Unicode scalar U+{codepoint}"));
        characters.insert(character, (*class, *delimiter));
    }

    let mut out = String::new();
    writeln!(
        out,
        "// @generated by build.rs from data/math_symbols.json — do not edit."
    )
    .unwrap();
    writeln!(
        out,
        "static UNICODE_MATH_COMMANDS: MathCommandMap = {};",
        commands.build()
    )
    .unwrap();
    writeln!(
        out,
        "static UNICODE_MATH_CHARS: &[(char, MathAtomInfo)] = &["
    )
    .unwrap();
    for (character, (class, delimiter)) in characters {
        writeln!(
            out,
            "    ({character:?}, {}),",
            render_info(class, delimiter)
        )
        .unwrap();
    }
    writeln!(out, "];").unwrap();

    let path = Path::new(&env::var("OUT_DIR").unwrap()).join("math_symbols.rs");
    let mut file = BufWriter::new(File::create(&path).unwrap());
    file.write_all(out.as_bytes()).unwrap();
}

fn main() {
    println!("cargo:rerun-if-changed=data/cwl_signatures.json");
    println!("cargo:rerun-if-changed=data/package_metadata.json");
    println!("cargo:rerun-if-changed=data/math_symbols.json");
    println!("cargo:rerun-if-changed=build.rs");

    generate_cwl_signatures();
    generate_package_metadata();
    generate_math_symbols();
}
