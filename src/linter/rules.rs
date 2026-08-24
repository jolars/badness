//! The rule abstraction: the [`Rule`] trait every lint implements, the
//! [`RuleContext`] handed to it, and the registry of built-in rules.
//!
//! Every rule is on by default; the
//! `badness.toml` `[lint]` `select`/`ignore` keys (and the CLI's matching flags)
//! narrow the active set via [`RuleSelection`], applied as a post-filter so the
//! shared `lint_document` driver stays config-unaware.

use std::path::Path;
use std::sync::OnceLock;

use rowan::{TextRange, TextSize};

use crate::ast::{AstNode, AstToken, ControlWord, Environment, child_token};
use crate::project::{ResolvedCitations, ResolvedLabels, ResolvedPackageOptions};
use crate::semantic::define::scan_definitions;
use crate::semantic::signature::SignatureDb;
use crate::semantic::{Mode, ModeIndex, SemanticModel};
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode};

use super::diagnostic::{Diagnostic, Severity};

pub mod abbreviation_spacing;
pub mod blank_line_in_keyval;
pub mod dash_length;
pub mod deprecated_command;
pub mod dollar_display_math;
pub mod duplicate_label;
pub mod duplicate_package;
pub mod ellipsis;
pub mod extra_alignment_tab;
pub mod hard_coded_reference;
pub mod label_before_caption;
pub mod makeat_macro;
pub mod math_operator_name;
pub mod mismatched_delimiter;
pub mod missing_nonbreaking_space;
pub mod missing_provides;
pub mod missing_required_argument;
pub mod obsolete_environment;
pub mod primitive_command;
pub mod redundant_script_braces;
pub mod sectioning_level_jump;
pub mod space_before_command;
pub mod straight_quotes;
pub mod swallowed_space;
pub mod times_variable;
pub mod unclosed_math_delimiter;
pub mod undefined_citation;
pub mod undefined_ref;
pub mod unknown_option;
pub mod unreferenced_label;
pub mod verbatim_trailing_text;

pub use abbreviation_spacing::AbbreviationSpacing;
pub use blank_line_in_keyval::BlankLineInKeyval;
pub use dash_length::DashLength;
pub use deprecated_command::DeprecatedCommand;
pub use dollar_display_math::DollarDisplayMath;
pub use duplicate_label::DuplicateLabel;
pub use duplicate_package::DuplicatePackage;
pub use ellipsis::Ellipsis;
pub use extra_alignment_tab::ExtraAlignmentTab;
pub use hard_coded_reference::HardCodedReference;
pub use label_before_caption::LabelBeforeCaption;
pub use makeat_macro::MakeatMacro;
pub use math_operator_name::MathOperatorName;
pub use mismatched_delimiter::MismatchedDelimiter;
pub use missing_nonbreaking_space::MissingNonbreakingSpace;
pub use missing_provides::MissingProvides;
pub use missing_required_argument::MissingRequiredArgument;
pub use obsolete_environment::ObsoleteEnvironment;
pub use primitive_command::PrimitiveCommand;
pub use redundant_script_braces::RedundantScriptBraces;
pub use sectioning_level_jump::SectioningLevelJump;
pub use space_before_command::SpaceBeforeCommand;
pub use straight_quotes::StraightQuotes;
pub use swallowed_space::SwallowedSpace;
pub use times_variable::TimesVariable;
pub use unclosed_math_delimiter::UnclosedMathDelimiter;
pub use undefined_citation::UndefinedCitation;
pub use undefined_ref::UndefinedRef;
pub use unknown_option::UnknownOption;
pub use unreferenced_label::UnreferencedLabel;
pub use verbatim_trailing_text::VerbatimTrailingText;

/// Everything a [`Rule`] reads to produce diagnostics for one file.
///
/// `path` is informational (rules may name the file in a message); the driver
/// still stamps each diagnostic's `path` afterward, so rules construct
/// diagnostics with an empty path.
pub struct RuleContext<'a> {
    pub path: &'a Path,
    pub root: &'a SyntaxNode,
    pub model: &'a SemanticModel,
    /// Cross-file label resolution for the project `path` belongs to, or `None`
    /// when there is no project view (stdin, or a context — like the language
    /// server today — that hasn't assembled one). Cross-file rules are inert when
    /// this is `None`. `path` keys into it to find this file's label namespace.
    pub resolution: Option<&'a ResolvedLabels>,
    /// Cross-file citation resolution (cite keys reachable via the project's
    /// `.bib` resources), or `None` when there is no project view. Gates
    /// `undefined-citation`, the bibliographic analog of `resolution`.
    pub citations: Option<&'a ResolvedCitations>,
    /// The project's package-option model (each analyzed `.sty` member's
    /// statically-declared options), or `None` when there is no project view.
    /// Gates `unknown-option`, the load-graph analog of `resolution`.
    pub packages: Option<&'a ResolvedPackageOptions>,
    /// Effective text/math/unknown mode, computed once for all mode-sensitive
    /// rules.
    mode_index: ModeIndex,
    /// The `\if…\else…\fi` branch path per byte offset, precomputed once so the
    /// duplicate-detection rules share one conditional tracker instead of each
    /// interpreting `\else`/`\fi` tokens themselves (see
    /// [`crate::linter::conditional`]). Same posture as `math_regions`: a
    /// read-only side index derived purely from the tree.
    conditionals: super::conditional::ConditionalIndex,
    /// The document's expl3 code regions (byte ranges), shared with the formatter's
    /// [`crate::formatter::core::expl3_regions`] so the two never drift on what
    /// counts as expl3 code. Only `unclosed-math-delimiter` reads it (to stay
    /// silent on `\[`/`$` demoted to data inside expl3 code), and demoted
    /// delimiters are rare, so — unlike `math_regions` — it is computed *lazily*
    /// on first [`RuleContext::in_expl3`] call rather than for every lint.
    expl3_regions: OnceLock<Vec<TextRange>>,
    /// The document's user command/environment definitions ([`scan_definitions`]),
    /// shared by the rules that must not flag a name the file itself redefines
    /// (`missing-required-argument`, `deprecated-command`, `primitive-command`).
    /// Same lazy posture as `expl3_regions`: scanned once, on the first
    /// [`RuleContext::user_definitions`] call, so a lint that never asks (the common
    /// case, since these rules ask only after matching a curated name) pays nothing.
    user_definitions: OnceLock<SignatureDb>,
}

impl<'a> RuleContext<'a> {
    /// Assemble the context for one file, precomputing the shared math-region
    /// index. `resolution`/`citations`/`packages` are `None` when there is no
    /// project view.
    pub fn new(
        path: &'a Path,
        root: &'a SyntaxNode,
        model: &'a SemanticModel,
        resolution: Option<&'a ResolvedLabels>,
        citations: Option<&'a ResolvedCitations>,
        packages: Option<&'a ResolvedPackageOptions>,
    ) -> Self {
        Self {
            path,
            root,
            model,
            resolution,
            citations,
            packages,
            mode_index: ModeIndex::build(root),
            conditionals: super::conditional::ConditionalIndex::compute(root),
            expl3_regions: OnceLock::new(),
            user_definitions: OnceLock::new(),
        }
    }

    /// The conditional branch path in effect at byte `offset` — empty when the
    /// offset is not inside any `\if…\fi`. Compare two sites with
    /// [`crate::linter::conditional::mutually_exclusive`]. `pub(crate)` (unlike
    /// [`RuleContext::in_math`]) so `Frame` stays out of the crate's public API.
    pub(crate) fn conditional_path_at(&self, offset: usize) -> &[super::conditional::Frame] {
        self.conditionals.path_at(offset)
    }

    pub fn mode_at(&self, offset: usize) -> Mode {
        self.mode_index.mode_at(offset)
    }

    /// Whether byte `offset` is proven math.
    pub fn in_math(&self, offset: usize) -> bool {
        self.mode_at(offset) == Mode::Math
    }

    /// Whether byte `offset` is proven text.
    pub fn in_text(&self, offset: usize) -> bool {
        self.mode_at(offset) == Mode::Text
    }

    /// Whether byte `offset` falls inside an expl3 code region (`\ExplSyntaxOn`…
    /// `\ExplSyntaxOff`, or a `\ProvidesExpl*` opener). Computed lazily from the
    /// shared [`crate::formatter::core::expl3_regions`] on first use and cached,
    /// so a lint that never asks (the common case) pays nothing. `O(log n)` over
    /// the disjoint, document-ordered regions.
    pub(crate) fn in_expl3(&self, offset: usize) -> bool {
        let regions = self
            .expl3_regions
            .get_or_init(|| crate::formatter::core::expl3_regions(self.root));
        let offset = TextSize::from(offset as u32);
        match regions.binary_search_by(|r| r.start().cmp(&offset)) {
            Ok(_) => true, // a region opens exactly here
            Err(0) => false,
            Err(i) => regions[i - 1].contains(offset),
        }
    }

    /// The document's user command/environment definitions, scanned lazily and
    /// cached. A command a rule would otherwise flag by name (a deprecated switch,
    /// a discouraged primitive, a low-arity built-in) must be left alone when the
    /// file redefines it: `\renewcommand\sp{…}` turns every later `\sp` into the
    /// user's macro, not the primitive. Query with
    /// [`SignatureDb::command`]/[`SignatureDb::environment`].
    pub(crate) fn user_definitions(&self) -> &SignatureDb {
        self.user_definitions
            .get_or_init(|| scan_definitions(self.root))
    }
}

/// TeX primitives that merely *reference* the following control word(s) — alias
/// them (`\let\x\rm`) or compare them (`\ifx\rm\y`) — rather than execute them,
/// and (unlike the `\def`/`\renewcommand` family) carry no replacement body, so
/// [`RuleContext::user_definitions`] does not record them. A deprecated switch or
/// discouraged primitive sitting in one of their operand slots must not get the
/// control-word swap: `\let\x\rmfamily` copies a *different* meaning, and where
/// this idiom guards the plain-TeX/ConTeXt branch, `\rmfamily`/`^` is undefined.
/// Redefinitions (a new meaning) are handled upstream by `user_definitions` — they
/// suppress the whole finding — so this narrow set is reference-only. Stored with
/// the leading backslash to compare against `CONTROL_WORD` text directly.
const REFERENCE_PRIMITIVES: &[&str] = &["\\let", "\\ifx"];

/// Whether `command`'s control word sits in a [`REFERENCE_PRIMITIVES`] operand slot.
/// The CST is flat, so a `\let`/`\ifx` primitive is one or two control words back
/// (`\let\x\rm`: `\x` then `\let`; `\ifx\a\rm`: `\a` then `\ifx`). Scan backward over
/// trivia and a possible `=` separator (`\let\x=\rm`), inspecting the two nearest
/// control words; a reference primitive among them means "referenced". Shared by
/// `deprecated-command` and `primitive-command` to withhold their control-word swap.
pub(crate) fn in_reference_position(command: &SyntaxNode) -> bool {
    let Some(control_word) = child_token::<ControlWord>(command) else {
        return false;
    };
    let mut token = control_word.syntax().prev_token();
    let mut control_words_seen = 0;
    while let Some(current) = token {
        match current.kind() {
            // Trivia never breaks the chain. A `WORD` is skipped too: an at-letter
            // definee splits under document catcodes (`\let\foo@bar\rm` lexes as
            // `\foo` + `@bar`), and `\let\x=\rm` writes an explicit `=`. The
            // two-control-word cutoff below bounds how far this look-back reaches, so
            // skipping intervening words cannot run away.
            SyntaxKind::WHITESPACE
            | SyntaxKind::NEWLINE
            | SyntaxKind::COMMENT
            | SyntaxKind::WORD => {}
            SyntaxKind::CONTROL_WORD => {
                if REFERENCE_PRIMITIVES.contains(&current.text()) {
                    return true;
                }
                control_words_seen += 1;
                if control_words_seen >= 2 {
                    return false;
                }
            }
            _ => return false,
        }
        token = current.prev_token();
    }
    false
}

/// Whether `tok` sits inside an argument of a key-argument command — `\label`,
/// the `\ref`/`\cite`/`\gls`/color families, `\tag`, `\hyperref` (see
/// [`crate::semantic::builder::key_argument_command`]) — whose content is an
/// opaque key or text rather than typeset math. The shared gate the math-shape
/// rules (`math-operator-name`, `times-variable`) and `dash-length` use to keep
/// identifier keys like `\label{eq:thing_max}` and `\cite{smith2020-1}` out of
/// scope. *All* argument groups of such a
/// command are skipped, not just the key slot: argument attachment is greedy
/// (arity is unknown at parse time, AGENTS.md decision #8), so index mapping is
/// unreliable, and a false negative beats flagging a key.
pub(crate) fn in_key_argument(tok: &crate::syntax::SyntaxToken) -> bool {
    tok.parent_ancestors().any(|node| {
        matches!(node.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL)
            && node.parent().is_some_and(|cmd| {
                cmd.kind() == SyntaxKind::COMMAND
                    && crate::ast::command_name(&cmd)
                        .is_some_and(|name| crate::semantic::builder::key_argument_command(&name))
            })
    })
}

/// The curated set of commands whose braced argument holds a *foreign programming
/// language* (Lua), not LaTeX prose: the LuaTeX primitives `\directlua`/`\latelua`
/// and luacode's `\luadirect`/`\luaexec`. Their body is source code executed by the
/// engine, so ASCII `"`, `...`, and friends there are Lua syntax, never typeset
/// text. Curated and deliberately small — a wrong entry only silences a prose lint
/// inside that command (a false negative), never the reverse.
fn is_code_argument_command(name: &str) -> bool {
    matches!(name, "directlua" | "latelua" | "luadirect" | "luaexec")
}

/// Whether `tok` sits inside the argument of a code-argument command
/// ([`is_code_argument_command`]) — Lua source passed to `\directlua` and its kin.
/// The prose rules (`straight-quotes`, …) use this to stay off Lua string literals
/// like `require("lfs")`, which are code, not quotation. Same greedy-attachment
/// caveat as [`in_key_argument`]: *all* argument groups are skipped, since arity is
/// unknown at parse time (AGENTS.md decision #8) and a false negative is preferred.
pub(crate) fn in_code_argument(tok: &crate::syntax::SyntaxToken) -> bool {
    tok.parent_ancestors().any(|node| {
        matches!(node.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL)
            && node.parent().is_some_and(|cmd| {
                cmd.kind() == SyntaxKind::COMMAND
                    && crate::ast::command_name(&cmd)
                        .is_some_and(|name| is_code_argument_command(&name))
            })
    })
}

/// The curated set of commands whose braced argument is a *font-map line*, not
/// LaTeX prose: pdfTeX's `\pdfmapline`/`\pdfmapfile`. Their body is a dvips-style
/// map entry (`+cmr10 <cmr10.pfb " .167 SlantFont"`), where a `"` delimits a
/// PostScript transform and a `-`/`--` is a flag, never quotation or a dash.
/// Curated and deliberately small like [`is_code_argument_command`]; a wrong entry
/// only silences a prose lint inside that command (a false negative), never the
/// reverse.
fn is_pdfmap_command(name: &str) -> bool {
    matches!(name, "pdfmapline" | "pdfmapfile")
}

/// Whether `tok` sits inside the argument of a font-map command
/// ([`is_pdfmap_command`]) — a `\pdfmapline` map entry, which is font-map data, not
/// prose. `straight-quotes` uses this to stay off the `"` PostScript-transform
/// delimiters there. Same greedy-attachment posture as [`in_key_argument`]: *all*
/// argument groups are skipped, since arity is unknown at parse time (AGENTS.md
/// decision #8) and a false negative is preferred.
pub(crate) fn in_pdfmap_argument(tok: &crate::syntax::SyntaxToken) -> bool {
    tok.parent_ancestors().any(|node| {
        matches!(node.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL)
            && node.parent().is_some_and(|cmd| {
                cmd.kind() == SyntaxKind::COMMAND
                    && crate::ast::command_name(&cmd).is_some_and(|name| is_pdfmap_command(&name))
            })
    })
}

/// Whether `tok` sits inside a doc/ltxdoc *description* command — `\DescribeMacro`
/// or `\DescribeEnv`, whose argument is the macro or environment *being
/// documented*, rendered as a syntax illustration in the margin rather than as
/// running prose. Syntax-placeholder idioms are the norm there (`\foo[...]`,
/// `\bar{...}`, quoted literals), so prose rules like `ellipsis` must stay off
/// them. Same greedy-attachment posture as [`in_key_argument`].
pub(crate) fn in_describe_argument(tok: &crate::syntax::SyntaxToken) -> bool {
    tok.parent_ancestors().any(|node| {
        matches!(node.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL)
            && node.parent().is_some_and(|cmd| {
                cmd.kind() == SyntaxKind::COMMAND
                    && crate::ast::command_name(&cmd)
                        .is_some_and(|name| matches!(&*name, "DescribeMacro" | "DescribeEnv"))
            })
    })
}

/// The curated set of commands that typeset their braced argument in a
/// *typewriter (monospace)* font, where the `--`/`---` dash ligatures are off and
/// a literal hyphen is the intended glyph — an MSC classification code
/// (`\texttt{03-02}`), a hyphenated identifier, a flag (`--verbose`). Curated and
/// deliberately small like [`is_code_argument_command`]; a wrong entry only
/// silences the dash lint inside that command (a false negative), never the
/// reverse. Verbatim monospace commands (`\verb`, `\lstinline`, `\url`) need no
/// entry: their content never lexes as `WORD`, so `dash-length` already skips it.
fn is_typewriter_argument_command(name: &str) -> bool {
    matches!(name, "texttt")
}

/// Whether `tok` sits inside the argument of a typewriter-font command
/// ([`is_typewriter_argument_command`]). `dash-length` uses this to stay off
/// monospace text, where an en dash is neither rendered nor wanted
/// (`\texttt{03-02}`). Same greedy-attachment posture as [`in_key_argument`].
pub(crate) fn in_typewriter_argument(tok: &crate::syntax::SyntaxToken) -> bool {
    tok.parent_ancestors().any(|node| {
        matches!(node.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL)
            && node.parent().is_some_and(|cmd| {
                cmd.kind() == SyntaxKind::COMMAND
                    && crate::ast::command_name(&cmd)
                        .is_some_and(|name| is_typewriter_argument_command(&name))
            })
    })
}

/// Whether `tok` sits inside the *range list* of a pgffor `\foreach` — the
/// `{0,20,...,350}` in `\foreach \x in {0,20,...,350}`. That `...` is pgffor's
/// range operator (it loops `0,20,40,…,350`), not prose, and `\dots` would break
/// the loop, so `ellipsis` must stay off it. Unlike the other argument gates the
/// range list is *not* a direct argument of `\foreach`: it is a sibling separated
/// by the loop variable and the `in` keyword, so we anchor on that structure with
/// a previous-sibling walk. The list group is the one immediately preceded
/// (skipping trivia) by the `in` keyword, with a `\foreach` command reachable
/// across the loop header before it. Anchoring on `in` keeps the gate off the
/// loop *body* group (`\foreach \x in {…} {body}`), whose immediate predecessor
/// is the list group rather than `in` and whose `...` would be real text. Reads
/// only the `\foreach` name (curated, like the rule-span gate); a wrong
/// suppression is just a false negative.
pub(crate) fn in_foreach_range(tok: &crate::syntax::SyntaxToken) -> bool {
    let is_foreach = |node: &SyntaxNode| {
        node.kind() == SyntaxKind::COMMAND
            && crate::ast::command_name(node).is_some_and(|name| name == "foreach")
    };
    let Some(group) = tok
        .parent_ancestors()
        .find(|node| node.kind() == SyntaxKind::GROUP)
    else {
        return false;
    };
    // Walk previous siblings: the list group must sit right after `in` (skipping
    // trivia), then a `\foreach` must be reachable across the loop header (the
    // variable command(s), a `/` separator, an `[options]` group).
    let mut saw_in = false;
    let mut prev = group.prev_sibling_or_token();
    while let Some(el) = prev {
        match &el {
            SyntaxElement::Node(node) => {
                if !saw_in {
                    // First non-trivia predecessor is a node, not `in`: this is the
                    // body group after the list, not the list itself.
                    return false;
                }
                if is_foreach(node) {
                    return true;
                }
                // Otherwise loop-header material (the `\x` variable, an `[options]`
                // group); keep walking back toward `\foreach`.
            }
            SyntaxElement::Token(t) => match t.kind() {
                SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT => {}
                SyntaxKind::WORD if !saw_in && t.text() == "in" => saw_in = true,
                // A `/` between paired variables (`\x/\y`) once past `in`.
                SyntaxKind::WORD if saw_in => {}
                _ => return false,
            },
        }
        prev = el.prev_sibling_or_token();
    }
    false
}

/// Whether `tok` sits inside an argument of a horizontal-rule command — `\cline`,
/// booktabs `\cmidrule`/`\specialrule`, … per the *curated* signature DB's `rule`
/// flag — whose content is a column span or dimension spec, never typeset text.
/// Keeps `dash-length` off `\cline{1-3}` (issue #34): the `1-3` there is a column
/// span, not a number range. Two shapes, both anchored on the nearest enclosing
/// argument node:
///
/// - *Attached:* the `GROUP`/`OPTIONAL`'s parent is a rule `COMMAND`
///   (`\cline{1-3}`, `\cmidrule[0.5pt]{4-5}` — greedy attachment, decision #8).
/// - *Detached:* a `\cmidrule(lr){2-3}` paren trim breaks greedy attachment, so
///   the span `GROUP` is a *sibling* of the command. Walk previous non-whitespace
///   siblings past at most one paren-trim `WORD`
///   ([`crate::formatter::core::is_paren_trim_word`], the same shape the
///   formatter's rule-line recognizer consumes) to the rule `COMMAND`.
///
/// Reads the curated built-in tier only (like `is_rule_command` on the formatter
/// side): a wrong suppression is just a false negative, which these rules prefer.
pub(crate) fn in_rule_span_argument(tok: &crate::syntax::SyntaxToken) -> bool {
    let is_rule_command = |node: &SyntaxNode| {
        node.kind() == SyntaxKind::COMMAND
            && crate::ast::command_name(node).is_some_and(|name| {
                crate::semantic::signature::builtin()
                    .command(&name)
                    .is_some_and(|sig| sig.rule)
            })
    };
    let Some(arg) = tok
        .parent_ancestors()
        .find(|node| matches!(node.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL))
    else {
        return false;
    };
    if arg.parent().is_some_and(|cmd| is_rule_command(&cmd)) {
        return true;
    }
    // Detached span: skip whitespace and one paren trim, then expect the command.
    let mut seen_trim = false;
    let mut prev = arg.prev_sibling_or_token();
    while let Some(el) = prev {
        match &el {
            crate::syntax::SyntaxElement::Node(node) => return is_rule_command(node),
            crate::syntax::SyntaxElement::Token(t) => match t.kind() {
                SyntaxKind::WHITESPACE => {}
                SyntaxKind::WORD if !seen_trim && crate::formatter::core::is_paren_trim_word(t) => {
                    seen_trim = true;
                }
                _ => return false,
            },
        }
        prev = el.prev_sibling_or_token();
    }
    false
}

/// The curated set of TikZ/pgf *picture* environments whose body is coordinate
/// and pgfmath-expression space rather than typeset prose: `tikzpicture`,
/// `pgfpicture`, and the pgfplots axis family. A `-` between numbers there is
/// coordinate arithmetic or a pgfmath subtraction (`(2-1,3)`, `{(y^2-1)^2}`),
/// never a typeset number range, so `dash-length`'s en-dash rewrite would turn a
/// meaning-bearing minus into an en dash. Curated and deliberately small like the
/// sibling pgf gate [`in_foreach_range`]; a wrong entry only silences a prose lint
/// inside that environment (a false negative), never the reverse.
///
/// The same family is curated a second time, as the `statementBody` flag in
/// `data/signatures.json`, which is what routes a picture body to statement
/// layout in the formatter. The two sets are near-identical (that one also names
/// `scope` and `pgfonlayer`, which need no entry here because the ancestor walk
/// reaches the enclosing `tikzpicture` anyway) and should be kept in step —
/// merging them waits on a signature DB in [`RuleContext`], which has none today
/// (TODO.md, *Linter*).
fn is_pgf_picture_environment(name: &str) -> bool {
    matches!(
        name,
        "tikzpicture"
            | "pgfpicture"
            | "axis"
            | "loglogaxis"
            | "semilogxaxis"
            | "semilogyaxis"
            | "groupplot"
            | "polaraxis"
            | "ternaryaxis"
    )
}

/// Whether `tok` sits inside a TikZ/pgf picture environment
/// ([`is_pgf_picture_environment`]) — `tikzpicture`, a pgfplots `axis`, and kin,
/// whose content is coordinate and pgfmath-expression space. `dash-length` uses
/// this to stay off coordinate arithmetic (`(2-1,3)`, `{(y^2-1)^2}`), where its
/// number-range en-dash rewrite would corrupt a meaning-bearing minus.
pub(crate) fn in_pgf_picture(tok: &crate::syntax::SyntaxToken) -> bool {
    tok.parent_ancestors().any(|node| {
        node.kind() == SyntaxKind::ENVIRONMENT
            && Environment::cast(node)
                .and_then(|env| env.name())
                .is_some_and(|name| is_pgf_picture_environment(&name))
    })
}

/// The curated set of pgf/pgfplots commands whose braced argument is a *pgfmath
/// expression* — `\addplot{expr}`/`\addplot3{expr}` and the `\pgfmath…` setters —
/// evaluated by the pgfmath parser, where `-` is subtraction, not a typeset range.
/// Small and curated like [`is_typewriter_argument_command`]; a wrong entry only
/// silences a lint inside that argument (a false negative), never the reverse.
fn is_pgfmath_expression_command(name: &str) -> bool {
    matches!(
        name,
        "addplot"
            | "pgfmathparse"
            | "pgfmathsetmacro"
            | "pgfmathsetlengthmacro"
            | "pgfmathtruncatemacro"
    )
}

/// Whether `tok` sits inside a pgfmath-expression argument
/// ([`is_pgfmath_expression_command`]). The argument group is either the command's
/// direct child (`\pgfmathparse{y-1}`) or a *detached* sibling separated by a
/// numeric-variant `WORD` (`\addplot3 {(y^2-1)^2}` — the `3` breaks greedy
/// attachment, decision #8), mirroring the detached shape [`in_rule_span_argument`]
/// handles for `\cmidrule(lr){2-3}`. `dash-length` uses this to stay off pgfmath
/// subtraction, whose en-dash rewrite would corrupt a meaning-bearing minus.
pub(crate) fn in_pgfmath_argument(tok: &crate::syntax::SyntaxToken) -> bool {
    let is_pgfmath = |node: &SyntaxNode| {
        node.kind() == SyntaxKind::COMMAND
            && crate::ast::command_name(node)
                .is_some_and(|name| is_pgfmath_expression_command(&name))
    };
    let Some(arg) = tok
        .parent_ancestors()
        .find(|node| matches!(node.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL))
    else {
        return false;
    };
    if arg.parent().is_some_and(|cmd| is_pgfmath(&cmd)) {
        return true;
    }
    // Detached span: skip whitespace and at most one trailing-variant `WORD`
    // (`\addplot3`), then expect the command.
    let mut skipped_word = false;
    let mut prev = arg.prev_sibling_or_token();
    while let Some(el) = prev {
        match &el {
            SyntaxElement::Node(node) => return is_pgfmath(node),
            SyntaxElement::Token(t) => match t.kind() {
                SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE => {}
                SyntaxKind::WORD if !skipped_word => skipped_word = true,
                _ => return false,
            },
        }
        prev = el.prev_sibling_or_token();
    }
    false
}

/// Whether `tok` sits inside a math alphabet or `\operatorname` argument, where
/// an operator-like spelling is intentionally upright rather than missing a
/// control sequence.
pub(crate) fn in_math_alphabet_or_operator_argument(tok: &crate::syntax::SyntaxToken) -> bool {
    tok.parent_ancestors().any(|node| {
        matches!(node.kind(), SyntaxKind::GROUP | SyntaxKind::OPTIONAL)
            && node.parent().is_some_and(|cmd| {
                cmd.kind() == SyntaxKind::COMMAND
                    && crate::ast::command_name(&cmd).is_some_and(|name| {
                        matches!(
                            name.as_str(),
                            "mathrm"
                                | "mathsf"
                                | "mathbf"
                                | "mathit"
                                | "mathtt"
                                | "mathnormal"
                                | "mathcal"
                                | "mathbb"
                                | "mathfrak"
                                | "mathscr"
                                | "operatorname"
                        )
                    })
            })
    })
}

/// A documented example for a rule: a snippet of LaTeX that triggers it.
///
/// The rule reference (`docs/src/reference/linter-rules.md`) is generated by
/// running the *real* linter on `source`, so the rendered diagnostics and the
/// autofix "after" state are *derived* rather than stored — the snippet stays the
/// single source of truth (see [`crate::linter::docs`]).
pub struct Example {
    /// One-line caption rendered above the snippet (markdown). May be empty.
    pub caption: &'static str,
    /// LaTeX source that triggers the rule. Should end with a trailing newline.
    pub source: &'static str,
}

/// A single lint. `Send + Sync` so the registry can be shared across the LSP's
/// read pool.
///
/// Rules come in two flavors, both driven by [`lint_document`](super::check::lint_document)'s
/// single shared traversal:
///
/// - **Node-shape rules** subscribe to [`Rule::interests`] and implement
///   [`Rule::check`]; the driver invokes `check` once per visited element whose
///   kind they named. They never walk the tree themselves.
/// - **Whole-file rules** leave `interests` empty and implement
///   [`Rule::check_file`]; the driver calls it once, after the walk. This is for
///   rules driven by the semantic model or cross-file resolution rather than by
///   node shape.
pub trait Rule: Send + Sync {
    /// The stable, kebab-case identifier reported as the diagnostic's `rule` and
    /// targeted by `% badness-lint skip <id>`.
    fn id(&self) -> &'static str;

    /// The severity a rule emits unless it overrides per-finding.
    fn default_severity(&self) -> Severity {
        Severity::Warning
    }

    /// One-paragraph (markdown) description of what the rule flags and why, used
    /// to generate the rule reference. Empty means "not yet documented"; the
    /// `every_rule_is_documented` test (`tests/rule_docs.rs`) requires a non-empty
    /// value for every shipped rule.
    fn description(&self) -> &'static str {
        ""
    }

    /// Worked examples for the rule reference. Each `source` is linted live and
    /// rendered with its diagnostics (and autofix before/after) by
    /// [`crate::linter::docs::render_rule_doc`]. The default is empty; the docs
    /// tests require at least one example per rule, and that each one actually
    /// triggers the rule.
    fn examples(&self) -> &'static [Example] {
        &[]
    }

    /// The synthetic filename an example snippet is linted as when rendering the
    /// rule reference. Defaults to `example.tex`; a rule gated on the file
    /// extension (like `missing-provides`, inert outside `.sty`/`.cls`) overrides
    /// this so its examples actually trigger under the docs renderer.
    fn example_path(&self) -> &'static str {
        "example.tex"
    }

    /// Synthetic `(path, source)` sibling files linted alongside every example
    /// of the rule — the two-file story a cross-file rule (like
    /// `unknown-option`, whose example loads a local `.sty`) needs to fire
    /// under the docs renderer. Paths are relative to the example's directory.
    /// Defaults to none.
    fn example_companions(&self) -> &'static [(&'static str, &'static str)] {
        &[]
    }

    /// The `SyntaxKind`s this rule subscribes to. During the driver's single
    /// shared traversal, [`Rule::check`] is invoked once for every element whose
    /// kind appears here. The default (`&[]`) opts out of node dispatch entirely —
    /// appropriate for rules that work off the whole file via [`Rule::check_file`].
    fn interests(&self) -> &'static [SyntaxKind] {
        &[]
    }

    /// Per-element callback, invoked for each CST element (node *or* token) whose
    /// kind is in [`Rule::interests`]. Node-shape rules unwrap `el.as_node()`.
    /// Findings are pushed onto `sink` with the path left empty.
    fn check(&self, el: &SyntaxElement, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let _ = (el, ctx, sink);
    }

    /// Whole-file pass, run once after the shared traversal. For rules driven by
    /// the semantic model or cross-file resolution rather than node shape. The
    /// default is a no-op. Findings are pushed onto `sink` with the path left empty.
    fn check_file(&self, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let _ = (ctx, sink);
    }

    /// An **ordered, stateful** visitor riding the driver's single shared
    /// traversal, for rules whose finding depends on the *sequence* of elements
    /// (a running toggle, the previous heading's level, …) that a stateless
    /// per-element [`check`](Rule::check) cannot carry. Returning `Some` opts the
    /// rule into the shared walk instead of a private `check_file` re-traversal;
    /// the default `None` opts out. The driver constructs one visitor per file and
    /// feeds it every element in document order, then calls
    /// [`StreamVisitor::finish`].
    fn stream(&self) -> Option<Box<dyn StreamVisitor>> {
        None
    }

    /// Whether this rule can emit an autofix. The `--fix` fixpoint loop runs only
    /// the fix-emitting rules each round (report-only rules contribute nothing to
    /// fix), so it must be `true` for every rule that ever sets `Diagnostic::fix`.
    /// Guarded by `emits_fix_matches_reality` in this module's tests.
    fn emits_fix(&self) -> bool {
        false
    }
}

/// An ordered, stateful pass driven by the linter's single shared traversal (see
/// [`Rule::stream`]). Constructed fresh per file, it receives every CST element in
/// document (preorder) order via [`visit`](StreamVisitor::visit), then a final
/// [`finish`](StreamVisitor::finish). Findings are pushed onto `sink` with the
/// path left empty, exactly like [`Rule::check`].
pub trait StreamVisitor {
    /// Called once for every element of the shared walk, in document order.
    fn visit(&mut self, el: &SyntaxElement, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>);

    /// Called once after the walk, for any deferred finding. Default no-op.
    fn finish(&mut self, ctx: &RuleContext<'_>, sink: &mut Vec<Diagnostic>) {
        let _ = (ctx, sink);
    }
}

/// Every built-in rule, in registry order.
pub fn all_rules() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(AbbreviationSpacing),
        Box::new(BlankLineInKeyval),
        Box::new(DuplicateLabel),
        Box::new(DeprecatedCommand),
        Box::new(MissingNonbreakingSpace),
        Box::new(ObsoleteEnvironment),
        Box::new(PrimitiveCommand),
        Box::new(DollarDisplayMath),
        Box::new(Ellipsis),
        Box::new(ExtraAlignmentTab),
        Box::new(HardCodedReference),
        Box::new(StraightQuotes),
        Box::new(SwallowedSpace),
        Box::new(SpaceBeforeCommand),
        Box::new(MismatchedDelimiter),
        Box::new(DashLength),
        Box::new(TimesVariable),
        Box::new(MathOperatorName),
        Box::new(MakeatMacro),
        Box::new(SectioningLevelJump),
        Box::new(MissingRequiredArgument),
        Box::new(UndefinedRef),
        Box::new(UndefinedCitation),
        Box::new(UnreferencedLabel),
        Box::new(VerbatimTrailingText),
        Box::new(DuplicatePackage),
        Box::new(MissingProvides),
        Box::new(UnknownOption),
        Box::new(RedundantScriptBraces),
        Box::new(UnclosedMathDelimiter),
        Box::new(LabelBeforeCaption),
    ]
}

/// A prebuilt, shareable view of a rule set: the boxed rules plus the
/// kind → subscriber dispatch table, computed once. The table is identical for
/// every file, so `lint_document` borrows a cached registry instead of rebuilding
/// it per file (the old per-invocation cost, quadratic over a project). Being
/// `Sync` (rules are `Send + Sync`), one registry is also shared by reference
/// across the CLI's rayon lint phase.
pub struct RuleRegistry {
    /// Every rule in the set, in registry order.
    pub rules: Vec<Box<dyn Rule>>,
    /// `by_kind[kind as usize]` lists the indices into `rules` of the node-shape
    /// rules subscribed to that `SyntaxKind`. Indexed by the `#[repr(u16)]`
    /// discriminant, so dispatch is an `O(1)` slice index.
    pub by_kind: Vec<Vec<usize>>,
    /// Whether any rule subscribed to a node kind (lets the driver skip the walk
    /// entirely when only whole-file/streaming rules are present).
    pub any_node_rules: bool,
}

impl RuleRegistry {
    fn build(rules: Vec<Box<dyn Rule>>) -> Self {
        let mut by_kind: Vec<Vec<usize>> = vec![Vec::new(); SyntaxKind::COUNT];
        let mut any_node_rules = false;
        for (i, rule) in rules.iter().enumerate() {
            for kind in rule.interests() {
                by_kind[*kind as usize].push(i);
                any_node_rules = true;
            }
        }
        Self {
            rules,
            by_kind,
            any_node_rules,
        }
    }
}

/// The shared registry of every built-in rule, built once on first use.
pub fn registry() -> &'static RuleRegistry {
    static REGISTRY: OnceLock<RuleRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| RuleRegistry::build(all_rules()))
}

/// The registry restricted to fix-emitting rules ([`Rule::emits_fix`]), used by
/// the `--fix` fixpoint loop so report-only rules aren't recomputed each round.
pub fn fixable_registry() -> &'static RuleRegistry {
    static REGISTRY: OnceLock<RuleRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        RuleRegistry::build(all_rules().into_iter().filter(|r| r.emits_fix()).collect())
    })
}

/// The ids of every built-in **LaTeX** rule. Kept in lockstep with [`all_rules`].
/// The bib rules live in [`crate::bib::linter::ALL_BIB_RULE_IDS`]; the selectable
/// universe is the union of the two (see [`all_known_rule_ids`]).
pub const ALL_RULE_IDS: &[&str] = &[
    "abbreviation-spacing",
    "blank-line-in-keyval",
    "duplicate-label",
    "deprecated-command",
    "missing-nonbreaking-space",
    "obsolete-environment",
    "primitive-command",
    "dollar-display-math",
    "ellipsis",
    "extra-alignment-tab",
    "hard-coded-reference",
    "straight-quotes",
    "swallowed-space",
    "space-before-command",
    "mismatched-delimiter",
    "dash-length",
    "times-variable",
    "math-operator-name",
    "makeat-macro",
    "sectioning-level-jump",
    "missing-required-argument",
    "undefined-ref",
    "undefined-citation",
    "unreferenced-label",
    "verbatim-trailing-text",
    "duplicate-package",
    "missing-provides",
    "unknown-option",
    "redundant-script-braces",
    "unclosed-math-delimiter",
    "label-before-caption",
];

/// Every known built-in rule id across **both** linters (LaTeX ∪ BibTeX).
///
/// The CLI lints `.tex` and `.bib` files in one pass and folds their findings into
/// a single diagnostic stream filtered by one [`RuleSelection`], so the selectable
/// universe — and the set `select`/`ignore` are validated against — must span both
/// registries. Without the bib half, every bib finding's id reads as "not active"
/// and the CLI silently drops it (the LSP, which doesn't post-filter, still shows
/// them — the source of the CLI/LSP divergence).
pub fn all_known_rule_ids() -> impl Iterator<Item = &'static str> {
    ALL_RULE_IDS
        .iter()
        .copied()
        .chain(crate::bib::linter::ALL_BIB_RULE_IDS.iter().copied())
}

/// The pseudo-rule id parse diagnostics carry. It is never a lint rule, so
/// `select`/`ignore` never touch it: a parse error always surfaces.
pub const PARSE_RULE_ID: &str = "parse";

/// The active lint-rule set for one run, after applying `select`/`ignore`.
///
/// Resolution by rule id (not by constructing the rule objects) so it can filter
/// the diagnostics `lint_document` already produced without changing that shared
/// entry point's signature. The semantics are:
///
/// 1. Base set = the ids in `select` when it is `Some`, else every built-in rule.
/// 2. Subtract anything in `ignore`.
/// 3. Unknown ids in `select`/`ignore` (not in [`ALL_RULE_IDS`]) are returned via
///    the second tuple element so the caller can surface them; they do not error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleSelection {
    active: Vec<&'static str>,
}

impl RuleSelection {
    /// Build the active set from `select`/`ignore`, returning it plus any unknown
    /// ids encountered (preserving their original spelling and order).
    pub fn resolve(select: Option<&[String]>, ignore: &[String]) -> (Self, Vec<String>) {
        let mut unknown = Vec::new();
        for id in select.iter().flat_map(|v| v.iter()).chain(ignore.iter()) {
            if !all_known_rule_ids().any(|known| known == id) {
                unknown.push(id.clone());
            }
        }
        let base: Vec<&'static str> = match select {
            Some(picks) => all_known_rule_ids()
                .filter(|id| picks.iter().any(|p| p == id))
                .collect(),
            None => all_known_rule_ids().collect(),
        };
        let active = base
            .into_iter()
            .filter(|id| !ignore.iter().any(|i| i == id))
            .collect();
        (Self { active }, unknown)
    }

    /// The unfiltered set: every built-in rule active. The default for callers
    /// with no config (the LSP, the library API).
    pub fn all() -> Self {
        Self {
            active: all_known_rule_ids().collect(),
        }
    }

    /// Whether a diagnostic with this `rule` should be kept. Parse diagnostics
    /// ([`PARSE_RULE_ID`]) are always kept; lint rules are kept iff active.
    pub fn is_active(&self, rule: &str) -> bool {
        rule == PARSE_RULE_ID || self.active.contains(&rule)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_and_id_list_agree() {
        let ids: Vec<&str> = all_rules().iter().map(|r| r.id()).collect();
        assert_eq!(ids, ALL_RULE_IDS);
    }

    // A rule that ever produces an autofix must report `emits_fix()`, or the
    // `--fix` loop (which runs only `fixable_registry`) would silently skip it.
    // Lint each rule's own examples (which the docs tests require to trigger) and
    // assert any produced fix is backed by the flag.
    #[test]
    fn emits_fix_matches_reality() {
        for rule in all_rules() {
            if rule.emits_fix() {
                continue;
            }
            let path = std::path::Path::new(rule.example_path());
            for example in rule.examples() {
                let produced_fix = crate::linter::docs::demo_diagnostics_at(path, example.source)
                    .iter()
                    .any(|d| d.rule == rule.id() && d.fix.is_some());
                assert!(
                    !produced_fix,
                    "rule `{}` emits a fix but `emits_fix()` returns false",
                    rule.id()
                );
            }
        }
    }

    #[test]
    fn all_selection_keeps_every_rule_and_parse() {
        let sel = RuleSelection::all();
        for id in ALL_RULE_IDS {
            assert!(sel.is_active(id), "{id} should be active");
        }
        assert!(sel.is_active(PARSE_RULE_ID));
    }

    #[test]
    fn select_restricts_to_listed_rules_but_keeps_parse() {
        let (sel, unknown) = RuleSelection::resolve(Some(&["duplicate-label".to_string()]), &[]);
        assert!(unknown.is_empty());
        assert!(sel.is_active("duplicate-label"));
        assert!(!sel.is_active("deprecated-command"));
        // Parse errors are never filtered out by a `select`.
        assert!(sel.is_active(PARSE_RULE_ID));
    }

    #[test]
    fn ignore_subtracts_from_default_set() {
        let (sel, unknown) = RuleSelection::resolve(None, &["deprecated-command".to_string()]);
        assert!(unknown.is_empty());
        assert!(!sel.is_active("deprecated-command"));
        assert!(sel.is_active("duplicate-label"));
    }

    #[test]
    fn ignore_overrides_select() {
        let (sel, _) = RuleSelection::resolve(
            Some(&["duplicate-label".to_string(), "undefined-ref".to_string()]),
            &["undefined-ref".to_string()],
        );
        assert!(sel.is_active("duplicate-label"));
        assert!(!sel.is_active("undefined-ref"));
    }

    #[test]
    fn bib_rules_are_active_by_default() {
        // The CLI filters bib findings through the same `RuleSelection`; bib rule
        // ids must count as known/active or the CLI silently drops every bib finding
        // (while the LSP, which doesn't post-filter, still shows them).
        let sel = RuleSelection::all();
        for id in crate::bib::linter::ALL_BIB_RULE_IDS {
            assert!(sel.is_active(id), "{id} should be active");
        }
        let (sel, unknown) = RuleSelection::resolve(None, &[]);
        assert!(unknown.is_empty());
        assert!(sel.is_active("missing-required-field"));
    }

    #[test]
    fn bib_rules_are_selectable_and_ignorable() {
        let (sel, unknown) =
            RuleSelection::resolve(Some(&["missing-required-field".to_string()]), &[]);
        assert!(unknown.is_empty(), "bib id must be recognized, not unknown");
        assert!(sel.is_active("missing-required-field"));
        assert!(!sel.is_active("duplicate-label"));

        let (sel, unknown) = RuleSelection::resolve(None, &["missing-required-field".to_string()]);
        assert!(unknown.is_empty());
        assert!(!sel.is_active("missing-required-field"));
        assert!(sel.is_active("duplicate-label"));
    }

    #[test]
    fn unknown_ids_are_reported() {
        let (_, unknown) = RuleSelection::resolve(
            Some(&["duplicate-label".to_string(), "no-such-rule".to_string()]),
            &["also-bogus".to_string()],
        );
        assert_eq!(
            unknown,
            vec!["no-such-rule".to_string(), "also-bogus".to_string()]
        );
    }
}
