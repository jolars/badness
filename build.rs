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

/// Compact (`"req"`/`"opt"`) or object (`{ "kind": …, "prose": …, "collapse": … }`).
#[derive(Deserialize)]
#[serde(untagged)]
enum RawArg {
    Short(RawArgKind),
    Full {
        kind: RawArgKind,
        #[serde(default)]
        prose: bool,
        #[serde(default)]
        collapse: bool,
    },
}

impl RawArg {
    /// Render as an `arg(required, ArgKind::…, prose, collapse)` const-fn call.
    fn render(&self) -> String {
        let (kind, prose, collapse) = match self {
            RawArg::Short(kind) => (*kind, false, false),
            RawArg::Full {
                kind,
                prose,
                collapse,
            } => (*kind, *prose, *collapse),
        };
        let (required, kind) = match kind {
            RawArgKind::Req => (true, "ArgKind::Brace"),
            RawArgKind::Opt => (false, "ArgKind::Bracket"),
        };
        format!("arg({required}, {kind}, {prose}, {collapse})")
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
    /// block_explicit, outline)` — `reflow`/`block` are derived inside the const fn.
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

fn main() {
    println!("cargo:rerun-if-changed=data/cwl_signatures.json");
    println!("cargo:rerun-if-changed=build.rs");

    let json = std::fs::read_to_string("data/cwl_signatures.json")
        .expect("data/cwl_signatures.json must exist (run `task cwl:sync`)");
    let db: RawDb = serde_json::from_str(&json).expect("data/cwl_signatures.json must be valid");

    let mut commands = phf_codegen::Map::new();
    for (name, sig) in &db.commands {
        commands.entry(name.as_str(), &sig.render());
    }
    let mut environments = phf_codegen::Map::new();
    for (name, sig) in &db.environments {
        environments.entry(name.as_str(), &sig.render());
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
