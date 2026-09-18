//! Expl3 semantic checks through the same entry point used by the CLI and LSP.

use std::path::Path;

use badness::declarations::ResolvedDeclarations;
use badness::file_discovery::file_kind_or_tex;
use badness::linter::{Diagnostic, Severity, check_document};

fn findings_at(source: &str, path: &str) -> Vec<Diagnostic> {
    let path = Path::new(path);
    check_document(
        path,
        source,
        file_kind_or_tex(path).lex_config(),
        &ResolvedDeclarations::default(),
    )
    .into_iter()
    .filter(|d| d.rule.starts_with("expl3-"))
    .collect()
}

fn findings(body: &str) -> Vec<Diagnostic> {
    findings_at(
        &format!("\\ExplSyntaxOn\n{body}\n\\ExplSyntaxOff\n"),
        "test.tex",
    )
}

#[test]
fn expl3_todo_examples_are_report_only_warnings() {
    let source = "\\ExplSyntaxOn\n\
        \\cs_generate_variant:Nn \\demo_use:n { nn }\n\
        \\prg_new_protected_conditional:Nnn \\demo: { p, TF } { \\prg_return_true: }\n\
        \\msg_new:nnn { demo } { bad } { #5 }\n\
        \\ExplSyntaxOff\n";
    let out = findings_at(source, "test.tex");
    assert_eq!(out.len(), 3, "{out:?}");
    assert_eq!(
        out.iter()
            .map(|d| (d.rule, &source[d.start..d.end]))
            .collect::<Vec<_>>(),
        [
            ("expl3-variant-type", "nn"),
            ("expl3-protected-predicate", "p"),
            ("expl3-invalid-message-parameter", "#5"),
        ]
    );
    for d in out {
        assert_eq!(d.severity, Severity::Warning);
        assert!(d.fix.is_none());
        assert_eq!(d.path, Path::new("test.tex"));
    }
}

#[test]
fn expl3_variant_lists_and_inherited_suffixes() {
    for good in [
        r"\cs_generate_variant:Nn \demo:Nn { c, cV, Nx, }",
        r"\cs_generate_variant:Nn { \demo:Nn } { c, ce }",
        r"\cs_generate_variant:cn { demo:Nn } { c, ce }",
        r"\cs_generate_variant:Nn \demo:cn { cx }",
        r"\cs_generate_variant:Nn \demo: { }",
        r"\cs_generate_variant:Nn \demo:nTF { V }",
    ] {
        assert!(findings(good).is_empty(), "{good}");
    }
    for bad in [
        r"\cs_generate_variant:Nn \demo:n { nn }",
        r"\cs_generate_variant:Nn \demo:cx { Ne }",
        r"\cs_generate_variant:cn { demo:n } { nn }",
        r"\prg_generate_conditional_variant:Nnn \demo:n { nn } { p, TF }",
        r"\prg_generate_conditional_variant:cnn { demo:n } { nn } { p, TF }",
    ] {
        let out = findings(bad);
        assert_eq!(out.len(), 1, "{bad}: {out:?}");
        assert!(out[0].message.contains("incompatible"));
    }
    let out = findings(r"\cs_generate_variant:Nn \demo:Nn { nn, NN, vc, nnn }");
    assert_eq!(out.len(), 4, "{out:?}");
    assert!(out[..3].iter().all(|d| d.message.contains("deprecated")));
    assert!(out[3].message.contains("incompatible"));
}

#[test]
fn expl3_protected_conditional_forms() {
    for operation in ["new", "set", "gset"] {
        for (spec, args) in [
            ("Nnn", r"\demo:n {p, T, F, TF} {#1}"),
            ("Npnn", r"\demo:n #1 {p} {#1}"),
            ("cnn", r"{demo:n} {p} {#1}"),
            ("cpnn", r"{demo:n} #1 {p} {#1}"),
        ] {
            let source = format!("\\prg_{operation}_protected_conditional:{spec} {args}");
            let out = findings(&source);
            assert_eq!(out.len(), 1, "{source}: {out:?}");
            assert_eq!(out[0].rule, "expl3-protected-predicate");
        }
    }
    for good in [
        r"\prg_new_conditional:Nnn \demo:n {p, TF} {#1}",
        r"\prg_new_protected_conditional:Nnn \demo:n {T,F,TF} {#1}",
        r"\cs_new_protected:Nn \demo_p:n {#1}",
        r"\prg_new_protected_conditional:Nnn \demo:n {\forms} {#1}",
    ] {
        assert!(findings(good).is_empty(), "{good}");
    }
}

#[test]
fn expl3_message_parameters_keep_token_boundaries() {
    for operation in ["new", "set", "gset"] {
        let source = format!(
            "\\ExplSyntaxOn\n\\msg_{operation}:nnnn {{demo}}{{bad}} \
             {{#1 #2 #3 #4 \\#5 ##5 #51 {{#6}}}} {{# 7 #% comment\n8 #9}}\n"
        );
        let out = findings_at(&source, "test.tex");
        assert_eq!(
            out.iter()
                .map(|d| &source[d.start..d.end])
                .collect::<Vec<_>>(),
            ["#5", "#6", "# 7", "#% comment\n8", "#9"],
            "{source}: {out:?}"
        );
    }
    assert!(findings(r"\msg_new:nnn{demo}{ok}{#1 #2 #3 #4 ##5 \#5}").is_empty());
}

#[test]
fn expl3_recognized_bodies_track_definition_parameters() {
    let source = "\\ExplSyntaxOn\n\
        \\cs_new:Npn \\outer:nnnnn #1#2#3#4#5 {\n\
          \\msg_new:nnn{demo}{one}{#5 ##5 ####5}\n\
          \\cs_new:Npn \\inner:n ##1 {\n\
            \\msg_new:nnn{demo}{two}{####6 ########6}\n\
          }\n\
        }\n";
    let out = findings_at(source, "test.tex");
    assert_eq!(
        out.iter()
            .map(|d| &source[d.start..d.end])
            .collect::<Vec<_>>(),
        ["##5", "####6"],
        "{out:?}"
    );
    for definition in [
        r"\cs_new:Nn \demo:n",
        r"\cs_set_protected:Npn \demo:n #1",
        r"\cs_gset_protected_nopar:cn {demo:n}",
        r"\cs_new:Nn \demo:nTF",
        r"\prg_new_conditional:Nnn \demo:n {TF}",
    ] {
        let source = format!(
            "{definition} {{ \\bool_if:NTF \\l_tmpa_bool \
             {{ \\msg_new:nnn{{demo}}{{bad}}{{##5}} }} \
             {{ \\cs_generate_variant:Nn \\demo:n {{nn}} }} }}"
        );
        assert_eq!(findings(&source).len(), 2, "{source}");
    }
}

#[test]
fn expl3_data_and_unknown_shapes_are_inert() {
    for source in [
        r"\tl_set:Nn \l_tmpa_tl {\msg_new:nnn{a}{b}{#5}}",
        r"\unknown:n {\msg_new:nnn{a}{b}{#5}}",
        r"\unknown \msg_new:nnn{a}{b}{#5}",
        r"\unknown:w x \msg_new:nnn{a}{b}{#5}",
        r"\exp_args:NNV \cs_generate_variant:Nn \demo:n \l_tmpa_tl",
        r"\cs_new:Npx \demo: {\msg_new:nnn{a}{b}{#5}}",
        r"\msg_new:nnx{a}{b}{#5}",
        r"\msg_new:nnn{a}{b}{\msg_new:nnn{a}{b}{##5}}",
        r"\cs_generate_variant:Nn \demo:n {\variants}",
        r"\cs_generate_variant:cn {demo_\name:n} {nn}",
        r"\cs_generate_variant:Nn \demo:n",
        "\\cs_generate_variant:Nn \\demo:n\n\n{nn}",
        r"\prg_new_protected_conditional:Nnn \demo:n {p}",
        r"\msg_new:nnnn{a}{b}{#5}",
        r"\msg_new:nnn{a}{b}{#5",
        r"\verb|\msg_new:nnn{a}{b}{#5}|",
        "% \\msg_new:nnn{a}{b}{#5}\n",
    ] {
        assert!(findings(source).is_empty(), "{source}");
    }
}

#[test]
fn expl3_expansion_wrappers_do_not_prove_following_calls() {
    for source in [
        r"\use:n {\use_none:n} {\msg_new:nnn{a}{b}{#5}}",
        r"\exp_args:Nn \use_none:n {} {\msg_new:nnn{a}{b}{#5}}",
        r"\exp_after:wN \use_none:n {\msg_new:nnn{a}{b}{#5}}",
        r"\let\saved\ExplSyntaxOn \msg_new:nnn{a}{b}{#5}",
        r"\string\ExplSyntaxOn \msg_new:nnn{a}{b}{#5}",
        r"\noexpand\ExplSyntaxOn \msg_new:nnn{a}{b}{#5}",
        r"\def\ExplSyntaxOn{} \msg_new:nnn{a}{b}{#5}",
    ] {
        assert!(findings_at(source, "test.tex").is_empty(), "{source}");
        assert!(findings(source).is_empty(), "{source}");
    }
}

#[test]
fn expl3_dtx_documentation_is_not_executable_code() {
    let source = "%<@@=demo>\n\
        % \\ExplSyntaxOn\n\
        % \\msg_new:nnn{demo}{documented}{#5}\n\
        %    \\begin{macrocode}\n\
        \\msg_new:nnn{demo}{real}{#6}\n\
        % \\msg_new:nnn{demo}{commented}{#7}\n\
        %    \\end{macrocode}\n";
    let out = findings_at(source, "demo.dtx");
    assert_eq!(
        out.iter()
            .map(|d| &source[d.start..d.end])
            .collect::<Vec<_>>(),
        ["#6"],
        "{out:?}"
    );
}

#[test]
fn expl3_lexical_regions_and_file_flavors() {
    assert!(findings_at(r"\msg_new:nnn{a}{b}{#5}", "test.tex").is_empty());
    let package = "\\ProvidesExplPackage{demo}{2026-09-18}{0.1}{Demo}\n\
                   \\msg_new:nnn{a}{b}{#5}\n";
    assert_eq!(findings_at(package, "demo.sty").len(), 1);
    let dtx =
        "%<@@=demo>\n%    \\begin{macrocode}\n\\msg_new:nnn{a}{b}{#5}\n%    \\end{macrocode}\n";
    assert_eq!(findings_at(dtx, "demo.dtx").len(), 1);
}

#[test]
fn expl3_suppression_uses_the_normal_driver() {
    let out = findings(
        "% badness-lint skip expl3-invalid-message-parameter\n\
                        \\msg_new:nnn{a}{b}{#5}\n\
                        \\cs_generate_variant:Nn \\demo:n {nn}",
    );
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0].rule, "expl3-variant-type");
}
