//! End-to-end test that the CLI formatter pulls a local package's signatures into
//! scope: a package-defined environment's arity changes how its `\begin` arguments
//! are laid out. Exercises `format_file_with_packages` → `DiskPackageSource` →
//! `collect_package_signatures` → the formatter's environment-arity lowering.

use std::fs;

use badness::formatter::{FormatStyle, format_file_with_packages, format_with_style_flavored};
use badness::parser::LatexFlavor;

const DOC: &str = "\\usepackage{mypkg}\n\\begin{myenv}\n{x}\nbody text here\n\\end{myenv}\n";

#[test]
fn local_package_environment_arity_glues_begin_argument() {
    let dir = tempfile::tempdir().unwrap();
    // A package defining a one-argument environment, alongside the document.
    fs::write(
        dir.path().join("mypkg.sty"),
        "\\newenvironment{myenv}[1]{start #1}{end}\n",
    )
    .unwrap();
    let main = dir.path().join("main.tex");

    let with_pkg =
        format_file_with_packages(DOC, &main, FormatStyle::default(), LatexFlavor::Document)
            .expect("formats cleanly");

    // Knowing `myenv` takes one argument, the formatter glues `{x}` onto the
    // `\begin{myenv}` line (the header break is dropped).
    assert!(
        with_pkg.contains("\\begin{myenv}{x}"),
        "expected the package arity to glue the argument, got:\n{with_pkg}"
    );

    // Without the package on disk, `myenv` is unknown (arity 0), so the argument is
    // not glued — the two outputs differ, proving the package drove the change.
    let without_pkg =
        format_with_style_flavored(DOC, FormatStyle::default(), LatexFlavor::Document)
            .expect("formats cleanly");
    assert!(!without_pkg.contains("\\begin{myenv}{x}"));
    assert_ne!(with_pkg, without_pkg);
}

#[test]
fn formatting_is_idempotent_with_packages() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("mypkg.sty"),
        "\\newenvironment{myenv}[1]{start #1}{end}\n",
    )
    .unwrap();
    let main = dir.path().join("main.tex");

    let once = format_file_with_packages(DOC, &main, FormatStyle::default(), LatexFlavor::Document)
        .expect("formats cleanly");
    let twice =
        format_file_with_packages(&once, &main, FormatStyle::default(), LatexFlavor::Document)
            .expect("formats cleanly");
    assert_eq!(once, twice, "format must be idempotent");
}
