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
use std::path::{Path, PathBuf};

use clap::CommandFactory;
use clap_complete::{Shell, generate_to};
use clap_mangen::Man;
use serde::Deserialize;

// Pull in the clap command definition directly (it references only `std` and
// `clap`, so it compiles standalone in the build-script crate).
#[path = "src/cli.rs"]
mod cli;

use cli::Cli;

/// `"req"` (mandatory `{…}`) or `"opt"` (optional `[…]`), as written in the JSON.
#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum RawArgKind {
    Req,
    Opt,
}

/// An argument's content kind as written in the JSON: `"opaque"` (default),
/// `"prose"`, or `"tokenList"`. Mirrors `ContentKind`.
#[derive(Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "camelCase")]
enum RawContentKind {
    #[default]
    Opaque,
    Prose,
    TokenList,
}

impl RawContentKind {
    fn variant(self) -> &'static str {
        match self {
            RawContentKind::Opaque => "ContentKind::Opaque",
            RawContentKind::Prose => "ContentKind::Prose",
            RawContentKind::TokenList => "ContentKind::TokenList",
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
    },
}

impl RawArg {
    /// Render as an `arg(required, ArgKind::…, ContentKind::…)` const-fn call.
    fn render(&self) -> String {
        let (kind, content) = match self {
            RawArg::Short(kind) => (*kind, RawContentKind::Opaque),
            RawArg::Full { kind, content } => (*kind, *content),
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

/// Generate shell completions into `OUT_DIR` (for the cargo build) and copy the
/// bash/fish/zsh scripts to `target/completions/` for packaging.
fn generate_completions(outdir: &std::ffi::OsString) -> std::io::Result<()> {
    let mut cmd = Cli::command();

    for shell in [
        Shell::Bash,
        Shell::Fish,
        Shell::Zsh,
        Shell::PowerShell,
        Shell::Elvish,
    ] {
        generate_to(shell, &mut cmd, "badness", outdir)?;
    }

    let completions_dir = PathBuf::from("target/completions");
    std::fs::create_dir_all(&completions_dir)?;

    let outdir_path = PathBuf::from(outdir);
    for (src, dst) in [
        ("badness.bash", "badness.bash"),
        ("badness.fish", "badness.fish"),
        ("_badness", "_badness"),
    ] {
        let from = outdir_path.join(src);
        if from.exists() {
            std::fs::copy(&from, completions_dir.join(dst))?;
        }
    }

    Ok(())
}

/// Format a man-page `SEE ALSO` section from a list of page names.
fn format_see_also(refs: &[String]) -> String {
    let formatted: Vec<String> = refs.iter().map(|r| format!("\\fB{}\\fR(1)", r)).collect();
    format!(".SH \"SEE ALSO\"\n{}\n", formatted.join(", "))
}

/// Generate `target/man/badness.1` plus a `badness-<sub>.1` page per subcommand,
/// like git/cargo.
fn generate_man_pages() -> std::io::Result<()> {
    let out_dir = PathBuf::from("target/man");
    std::fs::create_dir_all(&out_dir)?;

    let cmd = Cli::command();

    // Collect top-level subcommand names (skip "help") for SEE ALSO sections.
    let subcommand_names: Vec<String> = cmd
        .get_subcommands()
        .filter(|s| s.get_name() != "help")
        .map(|s| format!("badness-{}", s.get_name()))
        .collect();

    // Main page.
    let man = Man::new(cmd.clone());
    let mut buffer = Vec::new();
    man.render(&mut buffer)?;
    let main_content =
        String::from_utf8_lossy(&buffer).into_owned() + &format_see_also(&subcommand_names);
    std::fs::write(out_dir.join("badness.1"), main_content.as_bytes())?;

    // One page per top-level subcommand.
    for subcommand in cmd.get_subcommands() {
        let subcommand_name = subcommand.get_name();
        if subcommand_name == "help" {
            continue;
        }

        let name = format!("badness-{}", subcommand_name);
        let man = Man::new(subcommand.clone().version(env!("CARGO_PKG_VERSION"))).title(&name);
        let mut buffer = Vec::new();
        man.render(&mut buffer)?;

        // Post-process: fix NAME and SYNOPSIS subcommand references.
        let content = String::from_utf8_lossy(&buffer);
        let fixed_content = content
            .replace(
                &format!("{} \\-", subcommand_name),
                &format!("{} \\-", name),
            )
            .replace(
                &format!("\\fB{}\\fR", subcommand_name),
                &format!("\\fBbadness {}\\fR", subcommand_name),
            )
            .replace(
                &format!("{}\\-", subcommand_name),
                &format!("badness\\-{}\\-", subcommand_name),
            );

        // SEE ALSO: badness(1) plus sibling subcommand pages.
        let mut see_also_refs: Vec<String> = vec!["badness".to_string()];
        see_also_refs.extend(subcommand_names.iter().filter(|n| *n != &name).cloned());
        let with_see_also = fixed_content + &format_see_also(&see_also_refs);

        std::fs::write(
            out_dir.join(format!("{}.1", name)),
            with_see_also.as_bytes(),
        )?;
    }

    Ok(())
}

/// Render the markdown CLI reference into `docs/src/reference/cli.md` for the
/// mdBook. Skipped during `cargo package` (the committed file is shipped, and
/// packaging runs the build from a temporary directory).
fn generate_cli_markdown() -> std::io::Result<()> {
    let is_packaging = env::current_dir()
        .ok()
        .and_then(|p| p.to_str().map(|s| s.contains("/target/package/")))
        .unwrap_or(false);
    if is_packaging {
        return Ok(());
    }

    let docs_dir = PathBuf::from("docs/src/reference");

    // Only write when the mdBook source exists (it isn't shipped in the crate).
    if !docs_dir.exists() {
        return Ok(());
    }

    let cmd = Cli::command();
    let opts = clapdown::Options::new()
        .title("Command-line reference")
        .footer(false)
        .table_of_contents(false);
    let markdown = opts.render(&cmd);

    std::fs::write(docs_dir.join("cli.md"), &markdown)?;

    Ok(())
}

fn main() -> std::io::Result<()> {
    println!("cargo:rerun-if-changed=data/cwl_signatures.json");
    println!("cargo:rerun-if-changed=src/cli.rs");
    println!("cargo:rerun-if-changed=build.rs");

    generate_cwl_signatures();

    // Generate shell completions (needs OUT_DIR), man pages, and the CLI markdown.
    if let Some(outdir) = env::var_os("OUT_DIR") {
        generate_completions(&outdir)?;
    }
    generate_man_pages()?;
    generate_cli_markdown()?;

    Ok(())
}
