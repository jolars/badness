//! Build script: generate shell completions, man pages, and the markdown CLI
//! reference from the clap command definition.
//!
//! The signature-database codegen (the `phf` maps baked from
//! `data/cwl_signatures.json` and `data/package_metadata.json`) lives in
//! `crates/badness-parser/build.rs`, next to the data it bakes.

use std::env;
use std::path::PathBuf;

use clap::CommandFactory;
use clap_complete::{Shell, generate_to};
use clap_mangen::Man;

// Pull in the clap command definition directly (it references only `std` and
// `clap`, so it compiles standalone in the build-script crate).
#[path = "src/cli.rs"]
mod cli;

use cli::Cli;

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
    let markdown = clapdown::render(&cmd, &opts);

    std::fs::write(docs_dir.join("cli.md"), &markdown)?;

    Ok(())
}

fn main() -> std::io::Result<()> {
    println!("cargo:rerun-if-changed=src/cli.rs");
    println!("cargo:rerun-if-changed=build.rs");

    // Generate shell completions (needs OUT_DIR), man pages, and the CLI markdown.
    if let Some(outdir) = env::var_os("OUT_DIR") {
        generate_completions(&outdir)?;
    }
    generate_man_pages()?;
    generate_cli_markdown()?;

    Ok(())
}
