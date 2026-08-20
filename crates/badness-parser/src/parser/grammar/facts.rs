//! Curated static facts about particular commands.
//!
//! Each set is closed and hand-maintained. Entries describe text-falsifiable
//! surface syntax rather than command behavior. Unknown names use the generic path.

/// How [`Parser::attach_arguments`] treats a trailing `[…]` (issue #43).
/// `[`/`]` are not real grouping in TeX, so bracket attachment is a heuristic;
/// the policy is the caller's shape knowledge about the construct being
/// attached to. The in-math gates apply on top of it — see
/// [`Parser::attach_arguments`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BracketPolicy {
    /// Attach across intervening trivia (decision #8's default).
    Greedy,
    /// Attach only a directly-abutting `[` (a curated math environment's
    /// `\begin`: its math body starts right after, so a detached `[` is
    /// content).
    Tight,
    /// Never attach one (the delimiter-size commands: their `[` is the
    /// delimiter being sized).
    Forbid,
}

/// The delimiter-size commands (`\big`…`\Bigg` and their `l`/`m`/`r` variants).
/// A closed, curated set of TeX/amsmath primitives whose sole "argument" is the
/// delimiter token that follows (`\Big[`, `\bigl(`, `\Bigg|`), so a `[…]` after
/// one is never an optional argument (issue #43). The static-fact posture
/// mirrors `\left`/`\right` (`AGENTS.md`, decision #1).
pub(super) fn is_big_delimiter_command(text: &str) -> bool {
    let Some(name) = text.strip_prefix('\\') else {
        return false;
    };
    ["bigg", "Bigg", "big", "Big"].iter().any(|s| {
        name.strip_prefix(s)
            .is_some_and(|rest| matches!(rest, "" | "l" | "m" | "r"))
    })
}

/// The *definition-body* commands: commands whose trailing brace groups are
/// macro-code bodies, where TeX does not require `\begin`/`\end` to balance
/// within an individual group. Three families:
///
/// - The environment-definition commands (the LaTeX2e `\newenvironment` family
///   and the xparse `\NewDocumentEnvironment` family): the `\begin` lives in
///   the begin-code and its matching `\end` in the end-code by design
///   (`\newenvironment{wrap}{\begin{center}}{\end{center}}`, issue #45).
/// - The command-definition commands (the LaTeX2e `\newcommand` family and the
///   xparse `\NewDocumentCommand` family): a body may open or close an
///   environment for a matching hook to balance
///   (`\newcommand{\@@newpage}{\end{page}\begin{page}}`, issue #55).
/// - The LaTeX2e document/package hooks (`\AtBeginDocument` family): the code
///   argument runs at a different point in the document, so it balances
///   against that context, not within its own group
///   (`\AtBeginDocument{\begin{page}}` … `\AtEndDocument{\end{page}}`).
///
/// Inside those bodies `\begin`/`\end` parse as plain commands (see
/// [`Parser::in_def_body`]). A closed, curated set read as a static fact — the
/// bodies are never executed, mirroring [`is_big_delimiter_command`].
pub(crate) fn is_definition_body_command(text: &str) -> bool {
    matches!(
        text,
        "\\newenvironment"
            | "\\renewenvironment"
            | "\\provideenvironment"
            | "\\NewDocumentEnvironment"
            | "\\RenewDocumentEnvironment"
            | "\\ProvideDocumentEnvironment"
            | "\\DeclareDocumentEnvironment"
            | "\\AtBeginDocument"
            | "\\AtEndDocument"
            | "\\AtEndOfClass"
            | "\\AtEndOfPackage"
            | "\\AddToHook"
    ) || is_command_definition_command(text)
}

/// The LaTeX2e and xparse command-definition families. Unlike environment
/// definitions and hooks, these consume a control sequence as their name, so a
/// control-symbol name must remain data rather than opening live syntax.
pub(super) fn is_command_definition_command(text: &str) -> bool {
    matches!(
        text,
        "\\newcommand"
            | "\\renewcommand"
            | "\\providecommand"
            | "\\DeclareRobustCommand"
            | "\\NewDocumentCommand"
            | "\\RenewDocumentCommand"
            | "\\ProvideDocumentCommand"
            | "\\DeclareDocumentCommand"
    )
}

/// The TeX `\def`-family primitives, whose next token is always the control
/// sequence being (re)defined. A control-*symbol* name would otherwise be
/// misparsed as live syntax — `\def\[{…}`/`\def\]{…}` (a document class
/// restyling display math, stacks-project issue #65) reads as a math opener,
/// `\def\\{…}` as a line break — so [`Parser::command`] consumes it as a plain
/// token inside the `\def`'s node. A control-*word* name already parses
/// benignly as a generic command and keeps its current shape. A closed,
/// curated set read as a static fact, mirroring [`is_definition_body_command`];
/// the definition is never executed.
///
/// Also read by the formatter's expl3 region gate (in the `badness-formatter` crate): a toggle
/// spelling immediately preceded by one of these is a *definee*, never an executed
/// catcode switch, so it must not open a formatter-owned region.
pub fn is_def_prefix_command(text: &str) -> bool {
    matches!(text, "\\def" | "\\gdef" | "\\edef" | "\\xdef")
}
