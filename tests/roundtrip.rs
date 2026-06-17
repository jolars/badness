//! The losslessness invariant: `reconstruct(text) == text`, byte-for-byte.
//! This is badness's foundational parser test (Tenet 4 / Core decision in
//! `AGENTS.md`).

use std::fs;
use std::path::Path;

use badness::parser::reconstruct;

fn assert_lossless(text: &str) {
    assert_eq!(reconstruct(text), text);
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
