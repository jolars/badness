//! The losslessness invariant: `reconstruct(text) == text`, byte-for-byte.
//! This is badness's foundational parser test (Tenet 4 / Core decision in
//! `AGENTS.md`).

use std::fs;
use std::path::Path;

use badness_parser::declarations::Declarations;
use badness_parser::parser::{
    LatexFlavor, LexConfig, parse_with_declarations, parse_with_flavor, reconstruct,
};

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
        // expl3 syntax mode: `_`/`:` become letters between the toggles, so names
        // lex as single control words. Losslessness holds regardless of token kind.
        r"\ExplSyntaxOn\seq_new:N \g_@@_x_tl\ExplSyntaxOff\seq_new:N",
        // A `.ins` docstrip driver: plain `Document`-config code (no docstrip mode),
        // so a `%<…>`-looking line and a commented-out `\generate` are ordinary
        // comments and must reconstruct byte-for-byte.
        "\\input docstrip.tex\n\\keepsilent\n%<*nonsense>\n\\generate{\\file{foo.sty}{\\from{foo.dtx}{package}}}\n% \\generate{\\file{x}{\\from{y}{z}}}\n\\endbatchfile\n",
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
        // A guard block with CRLF line endings (the `>` terminates before `\r`).
        "%<*driver>\r\n\\documentclass{ltxdoc}\r\n%</driver>\r\n",
        // A guard with a boolean tag expression.
        "%<*package|driver>\n\\foo\n%</package|driver>\n",
        // A `macrocode` body with nested groups (the formatter indents these from a
        // column-0 base; losslessness must hold regardless).
        "%    \\begin{macrocode}\n\\def\\foo{%\n\\begingroup\n\\bar\n\\endgroup\n}\n%    \\end{macrocode}\n",
        // A documentation-layer environment whose frames sit on margin lines.
        "% \\begin{itemize}\n% \\item first\n% \\item second\n% \\end{itemize}\n",
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

/// Losslessness holds under a **declaration block** too, on the same bytes the
/// blind corpus sweep above already covers.
///
/// The invariant is the one thing config may never buy an exception to: a
/// declaration widens what is *recognized*, so it changes the tree's shape, and
/// a shape change that dropped a byte would be invisible to every test that
/// parses declaration-blind. `declared_alias.tex` is the corpus file whose two
/// readings differ — commands blind, an `eqnarray` under the block below.
#[test]
fn roundtrip_declared_corpus_file() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/declared_alias.tex");
    let text = fs::read_to_string(&path).expect("read the declared-alias corpus file");
    let declared = serde_json::from_str::<Declarations>(
        r#"{"environments": {"eqnarray": {"begin": ["\\bea"], "end": ["\\eea"]}}}"#,
    )
    .expect("declarations deserialize")
    .resolve()
    .expect("declarations resolve");

    let parsed = parse_with_declarations(&text, LatexFlavor::Document, &declared);
    assert_eq!(parsed.syntax().to_string(), text, "declared losslessness");
    assert!(
        parsed.errors.is_empty(),
        "the declared reading must parse cleanly: {:?}",
        parsed.errors
    );
    // The two readings really are different trees, so the assertion above is not
    // the blind one restated: the declaration pairs the two `\\bea … \\eea`
    // blocks, while blind only the literal `\\begin{bea}` is an environment.
    let environments = |root: &badness_parser::syntax::SyntaxNode| {
        root.descendants()
            .filter(|n| n.kind() == badness_parser::syntax::SyntaxKind::ENVIRONMENT)
            .count()
    };
    let blind = parse_with_flavor(&text, LatexFlavor::Document);
    assert_eq!(environments(&blind.syntax()), 1);
    assert_eq!(environments(&parsed.syntax()), 3);
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
