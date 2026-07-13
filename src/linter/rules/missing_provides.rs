//! `missing-provides`: a `.sty`/`.cls` source that never identifies itself with
//! `\ProvidesPackage`/`\ProvidesClass`.
//!
//! Every well-formed package/class declares its own identity so LaTeX can report
//! it in the log and enforce `\@ifpackagelater` date checks: a `.sty` should call
//! `\ProvidesPackage{name}[date version desc]`, a `.cls` `\ProvidesClass{…}`.
//! A file that omits this still compiles, so this is a [`Severity::Warning`].
//!
//! The rule is **gated purely on the file extension** of the linted path — it
//! fires only for `.sty` and `.cls`. A `.tex` document has nothing to provide; a
//! `.dtx` literate source hides its `\ProvidesPackage` inside a guarded
//! `macrocode` block that this file-level check would not see, so linting it would
//! false-positive. One predicate covers both "absent" and "wrong kind": a `.sty`
//! carrying only `\ProvidesClass` still lacks its *package* identifier.
//!
//! Caret: the identifier conventionally follows `\NeedsTeXFormat`, so the finding
//! anchors on that declaration's range when present, else at the start of the file
//! (byte `0`). **No autofix:** synthesizing a correct `\ProvidesPackage` line
//! (placement, date, version) is judgment, not a mechanical correctness edit.

use std::path::PathBuf;

use crate::linter::diagnostic::{Diagnostic, Severity};
use crate::semantic::ProvidesKind;

use super::{Example, Rule, RuleContext};

const EXAMPLES: &[Example] = &[Example {
    caption: "A package source with no self-identification \
              (the docs are rendered against a `.sty` path):",
    source: "\\NeedsTeXFormat{LaTeX2e}\n\\RequirePackage{xcolor}\n",
}];

pub struct MissingProvides;

impl MissingProvides {
    /// The `\Provides…` kind a file of this extension is expected to declare:
    /// `Package` for `.sty`, `Class` for `.cls`, and `None` (rule inert) for any
    /// other extension.
    fn expected_kind(path: &std::path::Path) -> Option<ProvidesKind> {
        match path.extension().and_then(|e| e.to_str()) {
            Some("sty") => Some(ProvidesKind::Package),
            Some("cls") => Some(ProvidesKind::Class),
            _ => None,
        }
    }
}

impl Rule for MissingProvides {
    fn id(&self) -> &'static str {
        "missing-provides"
    }

    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    fn description(&self) -> &'static str {
        "Flag a package or class source (`.sty`/`.cls`) that never identifies \
         itself with the matching `\\ProvidesPackage`/`\\ProvidesClass`. Every \
         well-formed package declares its identity so LaTeX can log it and honor \
         date-based compatibility checks; a `.sty` carrying only `\\ProvidesClass` \
         (wrong kind) still counts as missing. The rule is inert for any other \
         extension -- a `.tex` has nothing to provide, and a `.dtx` hides its \
         declaration inside guarded `macrocode`. No autofix: writing a correct \
         `\\Provides…` line (placement, date, version) is the author's call."
    }

    fn examples(&self) -> &'static [Example] {
        EXAMPLES
    }

    fn example_path(&self) -> &'static str {
        // The rule is inert outside `.sty`/`.cls`, so the docs snippet must be
        // linted as a package source to trigger.
        "example.sty"
    }

    fn check_file(&self, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let Some(expected) = Self::expected_kind(ctx.path) else {
            return;
        };
        // Fire when there is no `\Provides…` of the expected kind: either none at
        // all, or one naming the wrong namespace.
        if ctx.model.provides().is_some_and(|p| p.kind == expected) {
            return;
        }
        // Anchor at the `\NeedsTeXFormat` line the identifier should follow, else
        // the file start.
        let (start, end) = ctx
            .model
            .needs_format()
            .map(|nf| (usize::from(nf.range.start()), usize::from(nf.range.end())))
            .unwrap_or((0, 0));
        let (noun, command) = match expected {
            ProvidesKind::Package => ("package", "ProvidesPackage"),
            ProvidesKind::Class => ("class", "ProvidesClass"),
            ProvidesKind::File => ("file", "ProvidesFile"),
        };
        sink.push(Diagnostic {
            rule: self.id(),
            severity: self.default_severity(),
            path: PathBuf::new(),
            start,
            end,
            message: format!("{noun} file lacks `\\{command}`"),
            fix: None,
            related: Vec::new(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse;
    use crate::semantic::SemanticModel;
    use crate::syntax::SyntaxNode;

    fn findings(src: &str, path: &str) -> Vec<Diagnostic> {
        let root = SyntaxNode::new_root(parse(src).green);
        let model = SemanticModel::build(&root);
        let ctx = RuleContext::new(std::path::Path::new(path), &root, &model, None, None, None);
        let mut out = Vec::new();
        MissingProvides.check_file(&ctx, &mut out);
        out
    }

    #[test]
    fn sty_without_provides_is_flagged() {
        let out = findings("\\RequirePackage{xcolor}\n", "mypkg.sty");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule, "missing-provides");
        assert!(
            out[0].message.contains("ProvidesPackage"),
            "got: {}",
            out[0].message
        );
    }

    #[test]
    fn sty_with_provides_is_clean() {
        assert!(findings("\\ProvidesPackage{mypkg}[2026/01/01 v1]\n", "mypkg.sty").is_empty());
    }

    #[test]
    fn cls_without_provides_is_flagged() {
        let out = findings("\\LoadClass{article}\n", "myclass.cls");
        assert_eq!(out.len(), 1);
        assert!(
            out[0].message.contains("ProvidesClass"),
            "got: {}",
            out[0].message
        );
    }

    #[test]
    fn cls_with_wrong_kind_provides_is_flagged() {
        // A class file that mistakenly uses `\ProvidesPackage` still lacks its
        // class identifier.
        let out = findings("\\ProvidesPackage{myclass}\n", "myclass.cls");
        assert_eq!(out.len(), 1);
        assert!(
            out[0].message.contains("ProvidesClass"),
            "got: {}",
            out[0].message
        );
    }

    #[test]
    fn tex_document_is_inert() {
        assert!(findings("\\documentclass{article}\n", "main.tex").is_empty());
    }

    #[test]
    fn dtx_and_extensionless_are_inert() {
        assert!(findings("\\RequirePackage{xcolor}\n", "mypkg.dtx").is_empty());
        assert!(findings("\\RequirePackage{xcolor}\n", "README").is_empty());
    }

    #[test]
    fn caret_lands_on_needs_tex_format_when_present() {
        // The `\NeedsTeXFormat` control word occupies bytes 0..15.
        let out = findings(
            "\\NeedsTeXFormat{LaTeX2e}\n\\RequirePackage{xcolor}\n",
            "p.sty",
        );
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].start, out[0].end), (0, 15));
    }

    #[test]
    fn caret_is_file_start_without_needs_tex_format() {
        let out = findings("\\RequirePackage{xcolor}\n", "p.sty");
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].start, out[0].end), (0, 0));
    }
}
