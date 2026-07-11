//! `obsolete-environment`: math environments the LaTeX community has superseded,
//! reported with their modern replacement.
//!
//! The canonical case is `eqnarray`, which `amsmath` replaced with `align`
//! decades ago (it mis-spaces relations and is a perennial l2tabu/chktex
//! warning). As with [`deprecated_command`](super::deprecated_command), the
//! replacement is a near-mechanical swap carried as a `Safe` autofix: the
//! `\begin`/`\end` names are rewritten in place and the body copied verbatim, so
//! it is correct by construction (tenet 1) and never touches layout. The fix is
//! withheld on the rare shape where the pair is not cleanly matched (an
//! unterminated or recovery-paired environment).
//!
//! The table lives here, not in `data/signatures.json`: "this environment is
//! obsolete" is a lint judgment, not the structural arity/math fact the signature
//! DB carries (AGENTS.md core decision #2).

use std::path::PathBuf;

use crate::ast::{AstNode, Begin, Environment};
use crate::syntax::{SyntaxElement, SyntaxKind};

use crate::linter::diagnostic::{Diagnostic, Fix, Severity};

use super::{Example, Rule, RuleContext};

/// Obsolete environment name → its modern replacement.
const OBSOLETE: &[(&str, &str)] = &[("eqnarray", "align"), ("eqnarray*", "align*")];

const EXAMPLES: &[Example] = &[Example {
    caption: "The superseded `eqnarray` environment:",
    source: "\\begin{eqnarray}\n  a &=& b\n\\end{eqnarray}\n",
}];

pub struct ObsoleteEnvironment;

impl Rule for ObsoleteEnvironment {
    fn id(&self) -> &'static str {
        "obsolete-environment"
    }

    fn emits_fix(&self) -> bool {
        true
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag math environments the community has superseded, naming the modern \
         replacement in the message. The canonical case is `eqnarray`, which \
         `amsmath` replaced with `align` decades ago (it mis-spaces relations and \
         is a perennial l2tabu warning). The autofix swaps the `\\begin`/`\\end` \
         names, copying the body verbatim, so it is correct by construction."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn interests(&self) -> &'static [SyntaxKind] {
        &[SyntaxKind::ENVIRONMENT]
    }

    fn check(&self, el: &SyntaxElement, _ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(env) = el.as_node().cloned().and_then(Environment::cast) else {
            return;
        };
        let Some(begin) = env.begin() else {
            return;
        };
        let Some(name) = begin.name() else {
            return;
        };
        let Some((_, replacement)) = OBSOLETE.iter().find(|(obs, _)| *obs == name) else {
            return;
        };
        // Underline the name inside `\begin{…}`, not the whole environment.
        let range = begin
            .name_group()
            .map(|g| g.syntax().text_range())
            .unwrap_or_else(|| begin.syntax().text_range());
        sink.push(Diagnostic {
            rule: self.id(),
            severity: self.default_severity(),
            path: PathBuf::new(),
            start: usize::from(range.start()),
            end: usize::from(range.end()),
            message: format!("`{name}` is obsolete; use `{replacement}`"),
            fix: environment_swap_fix(&env, &begin, &name, replacement),
        });
    }
}

/// A `Safe` swap of an environment's name at both ends (`eqnarray` → `align`).
///
/// A [`Fix`] is a single contiguous replacement, but a rename must touch two
/// disjoint spans (the `\begin` and `\end` names). We splice the whole
/// environment: copy its text verbatim and rewrite only the two name spans, so
/// the body is preserved byte-for-byte and the edit owes only correctness, never
/// layout (tenet 1). Withheld unless the pair is cleanly matched — no `\end`
/// (unterminated), or an `\end` naming a different environment (parser recovery)
/// would make a symmetric swap corrupt the source, so those stay report-only.
fn environment_swap_fix(
    env: &Environment,
    begin: &Begin,
    name: &str,
    replacement: &str,
) -> Option<Fix> {
    let end = env.end()?;
    if end.name().as_deref() != Some(name) {
        return None;
    }
    let begin_name = begin.name_range()?;
    let end_name = end.name_range()?;

    // `\begin` precedes `\end`, so the name spans are ordered and disjoint.
    let env_start = env.syntax().text_range().start();
    let env_text = env.syntax().text().to_string();
    let bs = usize::from(begin_name.start() - env_start);
    let be = usize::from(begin_name.end() - env_start);
    let es = usize::from(end_name.start() - env_start);
    let ee = usize::from(end_name.end() - env_start);
    let content = format!(
        "{}{replacement}{}{replacement}{}",
        &env_text[..bs],
        &env_text[be..es],
        &env_text[ee..],
    );
    Some(Fix::safe(
        usize::from(env_start),
        usize::from(env.syntax().text_range().end()),
        content,
        format!("Replace `{name}` with `{replacement}`"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use crate::semantic::SemanticModel;
    use crate::syntax::SyntaxNode;

    fn findings(src: &str) -> Vec<Diagnostic> {
        let root = SyntaxNode::new_root(parse(src).green);
        let model = SemanticModel::build(&root);
        let ctx = RuleContext::new(
            std::path::Path::new("x.tex"),
            &root,
            &model,
            None,
            None,
            None,
        );
        let mut out = Vec::new();
        for el in root.descendants_with_tokens() {
            if ObsoleteEnvironment.interests().contains(&el.kind()) {
                ObsoleteEnvironment.check(&el, &ctx, &mut out);
            }
        }
        out
    }

    #[test]
    fn flags_eqnarray() {
        let out = findings("\\begin{eqnarray}\na &=& b\n\\end{eqnarray}\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "obsolete-environment");
        assert!(out[0].message.contains("align"), "got: {}", out[0].message);
        // Caret covers `{eqnarray}` of the `\begin`, not the whole environment.
        assert_eq!((out[0].start, out[0].end), (6, 16));
    }

    #[test]
    fn flags_starred_variant() {
        let out = findings("\\begin{eqnarray*}\na\n\\end{eqnarray*}\n");
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains("align*"));
    }

    #[test]
    fn align_is_fine() {
        assert!(findings("\\begin{align}\na &= b\n\\end{align}\n").is_empty());
    }

    #[test]
    fn carries_safe_swap_fix() {
        use crate::linter::diagnostic::Applicability;
        use crate::linter::fix::apply_fixes;

        let src = "\\begin{eqnarray}\n  a &=& b\n\\end{eqnarray}\n";
        let out = findings(src);
        let fix = out[0].fix.as_ref().expect("should carry a fix");
        assert_eq!(fix.applicability, Applicability::Safe);
        // Both ends are renamed and the body is copied verbatim.
        assert_eq!(
            apply_fixes(src, std::slice::from_ref(fix), false).output,
            "\\begin{align}\n  a &=& b\n\\end{align}\n"
        );
    }

    #[test]
    fn swaps_starred_variant() {
        use crate::linter::fix::apply_fixes;

        let src = "\\begin{eqnarray*}\na\n\\end{eqnarray*}\n";
        let fix = findings(src)[0].fix.clone().expect("a fix");
        assert_eq!(
            apply_fixes(src, &[fix], false).output,
            "\\begin{align*}\na\n\\end{align*}\n"
        );
    }

    #[test]
    fn reports_but_withholds_fix_when_unterminated() {
        let out = findings("\\begin{eqnarray}\na &=& b\n");
        assert_eq!(out.len(), 1);
        assert!(out[0].fix.is_none(), "no `\\end` -> no safe swap");
    }
}
