//! The losslessness invariant: `reconstruct(text) == text`, byte-for-byte.
//! This is badness's foundational parser test (Tenet 4 / Core decision in
//! `AGENTS.md`).

use std::fs;
use std::path::Path;

use badness::parser::{LatexFlavor, LexConfig, parse_with_flavor, reconstruct};

fn assert_lossless(text: &str) {
    assert_eq!(reconstruct(text), text);
}

/// Reconstruct under the docstrip (`.dtx`) lexer config. Losslessness must hold
/// in this mode exactly as in the plain one: the two-layer parse only *re-parents*
/// tokens (margins become trivia, `macrocode` bodies become code), never drops a
/// byte.
fn reconstruct_dtx(text: &str) -> String {
    let config = LexConfig {
        flavor: LatexFlavor::Document,
        dtx: true,
    };
    parse_with_flavor(text, config).syntax().to_string()
}

fn assert_lossless_dtx(text: &str) {
    assert_eq!(reconstruct_dtx(text), text);
}

#[test]
fn roundtrip_units() {
    let cases = [
        "",
        "hello world",
        r"\section{Introduction}",
        r"$x^2 + y_i = \frac{1}{2}$",
        "a % comment\nb",
        r"\begin{itemize}\item one\end{itemize}",
        "line1\n\nline2\r\nline3\r",
        "unicode: café — naïve ∑∫ 𝕏",
        r"\\ \{ \} \% \, \;",
        "trailing backslash \\",
        "[opt] {req} & # ~ ^_",
        "no final newline",
        // Argument-taking verbatim environments: the args precede the raw body, and
        // the body holds characters the generic lexer would otherwise (mis)read.
        "\\begin{lstlisting}[language=C]\nint a[3] = {1};  % literal\n\\end{lstlisting}",
        "\\begin{minted}[frame=single]{python}\nprint(\"$x$\")\n\\end{minted}",
        // A user-defined verbatim environment (catcode-othering begin-code) routes its
        // body to the opaque branch via the two-pass parse; it must still round-trip.
        "\\newenvironment{shellenv}{\\@makeother\\$}{}\n\\begin{shellenv}\na_$b$ % literal\n\\end{shellenv}\n",
        // Leading comment-bind: comments attached *into* a command/environment
        // must still reconstruct byte-for-byte (the bind only re-parents tokens).
        "% a doc comment\n\\section{Intro}\n",
        "% caption note\n\\begin{figure}\nbody\n\\end{figure}\n",
        "%a\n\n%b\n\\foo",
    ];
    for case in cases {
        assert_lossless(case);
    }
}

#[test]
fn roundtrip_dtx_units() {
    // Realistic `.dtx` surface shapes: a meta-comment header, a guarded driver
    // block, documentation prose behind `%` margins, and a `macrocode` block whose
    // code lines carry no margin. Losslessness must hold under the docstrip config
    // through every milestone.
    let cases = [
        "% \\iffalse meta-comment\n%<*driver>\n\\documentclass{ltxdoc}\n\\begin{document}\n\\DocInput{foo.dtx}\n\\end{document}\n%</driver>\n% \\fi\n",
        "% \\section{Introduction}\n% Some prose about \\foo.\n%    \\begin{macrocode}\n\\def\\foo{\\bar@baz}\n%    \\end{macrocode}\n",
        // A doc line whose content itself ends in a real trailing comment.
        "% prose with a real trailing comment % todo\n% \\DescribeMacro{\\foo}\n",
        // A margin-only blank line between two doc paragraphs.
        "% first paragraph\n%\n% second paragraph\n",
        // CRLF line endings throughout.
        "% doc line\r\n%    \\begin{macrocode}\r\n\\foo\r\n%    \\end{macrocode}\r\n",
        // An unterminated macrocode block must still reconstruct.
        "%    \\begin{macrocode}\n\\foo\n\\bar\n",
        // Inline docstrip guard prefixing a code line.
        "%<*pkg>\n\\RequirePackage{xcolor}\n%</pkg>\n",
    ];
    for case in cases {
        assert_lossless_dtx(case);
        // The same bytes must also round-trip under the plain config: dtx-ness only
        // changes structure, never which bytes are kept.
        assert_lossless(case);
    }
}

#[test]
fn roundtrip_dtx_corpus() {
    // Optional: any `.dtx` files dropped into the corpus (e.g. from CTAN) must
    // round-trip under the docstrip config. Absence is not a failure — unlike the
    // `.tex` corpus, this set may be empty until sources are vendored.
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    for entry in fs::read_dir(&dir).expect("read corpus dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) == Some("dtx") {
            let text = fs::read_to_string(&path).expect("read corpus file");
            assert_eq!(
                reconstruct_dtx(&text),
                text,
                "dtx losslessness failed for {path:?}"
            );
        }
    }
}

#[test]
fn roundtrip_corpus() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut count = 0;
    for entry in fs::read_dir(&dir).expect("read corpus dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) == Some("tex") {
            let text = fs::read_to_string(&path).expect("read corpus file");
            assert_eq!(reconstruct(&text), text, "losslessness failed for {path:?}");
            count += 1;
        }
    }
    assert!(count > 0, "no .tex corpus files found in {dir:?}");
}
