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
        assert!(
            findings(good)
                .iter()
                .all(|d| d.rule != "expl3-variant-type"),
            "{good}"
        );
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
                .filter(|d| d.rule == "expl3-deprecated-command")
                .count(),
            usize::from(operation == "gset"),
            "{source}: {out:?}"
        );
        assert_eq!(
            out.iter()
                .filter(|d| d.rule == "expl3-invalid-message-parameter")
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

#[test]
fn expl3_core_variable_contracts() {
    let source = r"\ExplSyntaxOn
\tl_const:Nn \g_demo_tl { value }
\tl_set:cn { c_demo_tl } { value }
\seq_count:N \l_demo_int
\str_set_eq:Nc \l_demo_str { l_demo_tl }
\tl_set_eq:NN \l_demo_str \l_demo_tl
\seq_set_from_clist:NN \l_demo_seq \l_demo_tl
\str_set:Nn \l_demo_tl { string }
\str_count:N \l_demo_tl
\tl_clear:N \l_demo_str
\tl_set_eq:NN \l_demo_tl \c_demo_str
\int_add:Nn \l_demo_int { 1 }
\ExplSyntaxOff";
    let out = findings_at(source, "demo.tex");
    assert_eq!(
        out.iter()
            .map(|d| (d.rule, &source[d.start..d.end]))
            .collect::<Vec<_>>(),
        [
            ("expl3-variable-mutability", r"\g_demo_tl"),
            ("expl3-variable-mutability", "c_demo_tl"),
            ("expl3-variable-type", r"\l_demo_int"),
            ("expl3-variable-type", "l_demo_tl"),
            ("expl3-variable-type", r"\l_demo_str"),
            ("expl3-variable-type", r"\l_demo_tl"),
        ],
        "{out:?}"
    );
    assert!(
        out.iter()
            .all(|d| d.fix.is_none() && d.severity == Severity::Warning)
    );
}

#[test]
fn expl3_count_operations_check_their_own_operand_contracts() {
    let source = r"\ExplSyntaxOn
\tl_set:Nn \l_demo_tl { a,,b }
\clist_set:Nn \l_demo_clist { a,,b }
\clist_count:N \l_demo_tl
\clist_count:c { l_demo_str }
\clist_count:N \l_demo_clist
\str_count:N \l_demo_clist
\tl_count:N \l_demo_clist";
    let out = findings_at(source, "demo.tex");
    assert_eq!(
        out.iter()
            .map(|d| (d.rule, &source[d.start..d.end]))
            .collect::<Vec<_>>(),
        [
            ("expl3-variable-type", r"\l_demo_tl"),
            ("expl3-variable-type", "l_demo_str"),
        ],
        "{out:?}"
    );
}

#[test]
fn expl3_variable_scope_tracks_recognized_contexts() {
    let out = findings(
        r"
\tl_set:Nn \g_top_tl {}
\bool_if:NT \l_demo_bool { \tl_gset:Nn \l_branch_tl {} }
\cs_new_protected:Nn \demo: {
    \tl_set:Nn \g_body_tl {}
    \bool_if:NT \l_demo_bool { \tl_gset:Nn \l_nested_tl {} }
    \unknown:nT {} { \tl_gset:Nn \l_unknown_tl {} }
}
{ \int_gzero:N \l_group_int }
\group_begin:
  \bool_if:NT \l_demo_bool { \tl_set:Nn \g_paired_tl {} }
\group_end:
\tl_set:Nn \g_after_tl {}
\group_begin:
\tl_set:Nn \g_unmatched_tl {}
",
    );
    assert_eq!(out.len(), 4, "{out:?}");
    for (d, name) in out
        .iter()
        .zip(["g_body_tl", "l_nested_tl", "l_group_int", "g_paired_tl"])
    {
        assert_eq!(d.rule, "expl3-variable-scope");
        assert!(d.message.contains(name), "{d:?}");
    }
}

#[test]
fn expl3_variable_unknown_operands_and_bodies_stay_silent() {
    for source in [
        r"\tl_set:cn { c_\computed_tl } { value }",
        r"\tl_set:Nn \c_demo_tl",
        r"\tl_set:NN \c_demo_tl \l_demo_tl",
        r"\tl_set:Nn \l_demo_tl { \int_zero:N \c_demo_int }",
        r"\exp_args:Nc \int_zero:N { c_demo_int }",
        r"\unknown:nTF {} { \int_zero:N \c_demo_int } {}",
        "% badness-lint skip expl3-variable-mutability\n\\int_zero:N \\c_demo_int",
    ] {
        assert!(findings(source).is_empty(), "{source}");
    }
}

#[test]
fn expl3_variable_scope_declines_groups_crossing_docstrip_guards() {
    let source = r"%<@@=demo>
%    \begin{macrocode}
%<a>\group_begin:
%<-a>\tl_set:Nn \g_alternative_tl { value }
%<a>\group_end:
%<a>\group_begin:
\tl_set:Nn \g_maybe_grouped_tl { value }
\group_end:
\group_begin:
\tl_set:Nn \g_grouped_tl { value }
\group_end:
%    \end{macrocode}";
    let out = findings_at(source, "demo.dtx");
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0].rule, "expl3-variable-scope");
    assert_eq!(&source[out[0].start..out[0].end], r"\g_grouped_tl");
}

#[test]
fn expl3_deprecation_is_lexical_and_keeps_exact_spans() {
    let source = r"\ExplSyntaxOn
\str_lower_case:n { ABC }
\tl_set:Nn \l_demo_tl { \l_keys_choice_tl }
% \str_lower_case:n {}
\verb|\str_lower_case:n|
\c_zero
\ExplSyntaxOff
\str_lower_case:n { ABC }";
    let out = findings_at(source, "demo.tex");
    assert_eq!(
        out.iter()
            .map(|d| (d.rule, &source[d.start..d.end]))
            .collect::<Vec<_>>(),
        [
            ("expl3-deprecated-command", r"\str_lower_case:n"),
            ("expl3-deprecated-command", r"\l_keys_choice_tl"),
        ],
        "{out:?}"
    );
    assert!(out[0].message.contains("2020-01-03"));

    let dtx = "%<@@=demo>\n% \\str_lower_case:n { documentation }\n\
               %    \\begin{macrocode}\n\\str_lower_case:n { code }\n\
               \\int_zero:N \\c_demo_int\n%    \\end{macrocode}\n";
    let out = findings_at(dtx, "demo.dtx");
    assert_eq!(out.len(), 2, "{out:?}");
    assert_eq!(out[0].start, dtx.rfind(r"\str_lower_case:n").unwrap());
    assert_eq!(&dtx[out[1].start..out[1].end], r"\c_demo_int");
}

#[test]
fn expl3_deprecation_checks_executable_docstrip_lines() {
    let source = r"%<pkg>\ExplSyntaxOn
% Documentation \str_lower_case:n { prose }
\str_lower_case:n { ordinary }
%<*pkg>\str_lower_case:n { ignored guard suffix }
%<pkg>\str_lower_case:n { ABC }
%<+pkg>\str_lower_case:n { DEF }
%    \begin{macrocode}
%<*extra>\str_lower_case:n { ignored macrocode suffix }
\str_lower_case:n { macrocode }
%</extra>\str_lower_case:n { ignored closing suffix }
%    \end{macrocode}
%<pkg>\ExplSyntaxOff
%<pkg>\str_lower_case:n { inactive }
%</pkg>";
    let out = findings_at(source, "demo.dtx");
    assert_eq!(
        out.iter()
            .map(|d| (d.rule, d.start, &source[d.start..d.end]))
            .collect::<Vec<_>>(),
        ["ordinary", "ABC", "DEF", "macrocode"].map(|argument| (
            "expl3-deprecated-command",
            source
                .find(&format!(r"\str_lower_case:n {{ {argument} }}"))
                .unwrap(),
            r"\str_lower_case:n",
        )),
        "{out:?}"
    );
}

#[test]
fn expl3_document_body_is_top_level_for_scope_checks() {
    let out = findings_at(
        r"\documentclass{article}
\begin{document}
\ExplSyntaxOn
\tl_set:Nn \g_demo_tl {}
\tl_gset:Nn \l_demo_tl {}
\bool_if:cT { l_demo_bool } { \tl_gset:Nn \l_demo_tl {} }
\tl_set:Nn \c_demo_tl {}
\ExplSyntaxOff
\end{document}",
        "demo.tex",
    );
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0].rule, "expl3-variable-mutability");
}

#[test]
fn expl3_variable_checks_follow_docstrip_extraction() {
    let source = r"% Package documentation
%<pkg>\ExplSyntaxOn
% \ExplSyntaxOff \int_zero:N \c_prose_int
%<pkg>\int_zero:N \c_inline_int
\int_zero:N \c_ordinary_int
%    \begin{macrocode}
%<*pkg>\int_zero:N \c_open_int \ExplSyntaxOff
%<@@=demo>\int_zero:N \c_module_int
%</pkg>\int_zero:N \c_close_int
\int_zero:N \c_macrocode_int
%    \end{macrocode}";
    let out = findings_at(source, "demo.dtx");
    assert_eq!(
        out.iter()
            .map(|d| (d.rule, &source[d.start..d.end]))
            .collect::<Vec<_>>(),
        [r"\c_inline_int", r"\c_ordinary_int", r"\c_macrocode_int"]
            .map(|name| ("expl3-variable-mutability", name)),
        "{out:?}",
    );
}

#[test]
fn expl3_deprecation_carries_explicit_docstrip_mode_across_macrocode() {
    for toggle in [r"\ExplSyntaxOn", r"\ProvidesExplPackage{demo}{}{}{}"] {
        let source = format!(
            "%<@@=demo>\n%    \\begin{{macrocode}}\n%    \\end{{macrocode}}\n\\str_lower_case:n {{ implicit }}\n\
             %    \\begin{{macrocode}}\n{toggle}\n%    \\end{{macrocode}}\n\\str_lower_case:n {{ explicit }}\n\
             %    \\begin{{macrocode}}\n\\ExplSyntaxOff\n%    \\end{{macrocode}}\n\\str_lower_case:n {{ inactive }}\n"
        );
        let out = findings_at(&source, "demo.dtx");
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].rule, "expl3-deprecated-command");
        assert_eq!(
            out[0].start,
            source.find(r"\str_lower_case:n { explicit }").unwrap()
        );
    }
}

#[test]
fn expl3_braced_n_reports_exact_operand_spans() {
    let source = r"\ExplSyntaxOn
\cs_set_eq:NN { \demo_a: } { \demo_b: }
\cs_new:Nn \demo_outer: { \tl_clear:N { \l_tmpa_tl } }
\ExplSyntaxOff";
    let out = findings_at(source, "test.tex");
    assert_eq!(
        out.iter()
            .map(|d| &source[d.start..d.end])
            .collect::<Vec<_>>(),
        [r"{ \demo_a: }", r"{ \demo_b: }", r"{ \l_tmpa_tl }"]
    );
    for d in out {
        assert_eq!(d.rule, "expl3-braced-n-argument");
        assert_eq!(d.severity, Severity::Warning);
        assert!(d.fix.is_none());
    }
}

#[test]
fn expl3_braced_n_requires_a_recognized_n_operand() {
    for source in [
        r"\demo:N \l_tmpa_tl",
        r"\demo:n { \l_tmpa_tl }",
        r"\demo:V { \l_tmpa_tl }",
        r"\demo:Nn { \l_tmpa_tl }",
        r"\tl_set:Nn \l_tmpa_tl { \tl_clear:N { \l_tmpb_tl } }",
        r"\exp_args:NN \demo:N { \l_tmpa_tl }",
    ] {
        assert!(findings(source).is_empty(), "{source}");
    }
}

#[test]
fn expl3_braced_n_checks_dtx_code() {
    let source = r"%<@@=demo>
% \tl_clear:N { \l_documented_tl }
%    \begin{macrocode}
\tl_clear:N { \l_executable_tl }
% \tl_clear:N { \l_commented_tl }
%    \end{macrocode}";
    let out = findings_at(source, "demo.dtx");
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(&source[out[0].start..out[0].end], r"{ \l_executable_tl }");
}
