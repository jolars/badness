//! Scan a document for **user definitions** — `\newcommand`/`\newenvironment` and
//! the xparse `\NewDocument…` family — and extract their argument *signatures* into
//! a per-document [`SignatureDb`]. Signatures only: we read the declared argument
//! shape, never the replacement text, and never execute anything (AGENTS.md
//! non-goals and decision #1).
//!
//! A single whole-tree walk (mirror of [`super::builder::build`]) collects every
//! definition; the result overlays the built-in DB via [`Signatures`] (scanned
//! first). The greedy parser attaches definitions like any other command, so they
//! surface as plain `COMMAND` descendants — those inside a comment or a verbatim
//! body never parse to a `COMMAND`, so they are skipped for free.
//!
//! [`Signatures`]: super::signature::Signatures
//!
//! ## Both name forms
//!
//! For command definitions we extract **both** name forms: the braced
//! `\newcommand{\foo}…` and the unbraced `\newcommand\foo…`. The unbraced form
//! parses awkwardly under greedy attachment — `\foo` becomes a *sibling* `COMMAND`
//! and the `[n]`/replacement group attaches to it, not to `\newcommand` — so
//! `\newcommand` itself has no name group. We recover it with a scanner-side sibling
//! heuristic ([`resolve_command_def`]): when a definition command has no attached
//! group, the name and argument shape are read off the immediately-following sibling
//! `COMMAND`. This stays in the scanner — no parser change — so the parser remains
//! meaning-free (decision #2). Environment names are brace-delimited *text*, never a
//! bare control word, so they have no unbraced form to recover.

use std::collections::{HashMap, HashSet};

use crate::ast::{
    command_name, control_word_range, group_command_name, group_inner_source, nth_group,
    nth_group_inner, nth_group_text,
};
use crate::semantic::signature::{
    ArgKind, ArgSpec, CommandSig, ContentKind, EnvironmentSig, SignatureDb,
};
use crate::semantic::xparse;
use crate::syntax::{SyntaxKind, SyntaxNode};
use rowan::{NodeOrToken, TextRange, TextSize};
use smol_str::SmolStr;

/// Scan `root` for user command/environment definitions and return their extracted
/// signatures. Names already defined earlier in the document are overwritten, so a
/// later `\renewcommand` wins — TeX's last-definition-wins, modulo execution order
/// we do not track.
pub fn scan_definitions(root: &SyntaxNode) -> SignatureDb {
    let mut db = SignatureDb::default();
    // Replacement-body facts collected alongside each command signature, keyed by
    // name (last definition wins, mirroring `db`). Consumed after the walk to flag
    // catcode-othering verbatim-argument commands (`apply_verbatim_flags`).
    let mut bodies: HashMap<SmolStr, DefBody> = HashMap::new();
    // The same for environment *begin-code*, kept in a separate map because
    // environment names live in a different namespace from commands (and so a name
    // collision must not let one shadow the other during chain resolution). The
    // begin-code's *called* helpers are resolved against the command `bodies` map.
    let mut env_bodies: HashMap<SmolStr, DefBody> = HashMap::new();

    for command in root
        .descendants()
        .filter(|node| node.kind() == SyntaxKind::COMMAND)
    {
        let Some(name) = command_name(&command) else {
            continue;
        };
        match DefKind::of(&name) {
            Some(DefKind::Command) => scan_newcommand(&command, &mut db, &mut bodies),
            Some(DefKind::Def) => scan_def(&command, &mut db, &mut bodies),
            Some(DefKind::Environment) => scan_newenvironment(&command, &mut db, &mut env_bodies),
            Some(DefKind::XparseCommand) => scan_xparse_command(&command, &mut db, &mut bodies),
            Some(DefKind::XparseEnvironment) => {
                scan_xparse_environment(&command, &mut db, &mut env_bodies)
            }
            None => {}
        }
    }

    apply_verbatim_flags(&mut db, &bodies);
    apply_verbatim_env_flags(&mut db, &env_bodies, &bodies);
    db
}

/// Which namespace a scanned definition site names. Commands and environments live
/// in disjoint TeX namespaces, so a name match is only meaningful within a kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefSiteKind {
    Command,
    Environment,
}

/// One user definition's *location* — the range-bearing sibling of the signature
/// facts [`scan_definitions`] extracts. Signatures stay range-free so the
/// `document_signatures` salsa query backdates on pure-offset edits; definition
/// sites feed LSP navigation (goto-definition, references, rename), which needs
/// byte ranges and recomputes them per request off the memoized tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefSite {
    /// The defined name (no leading `\` for commands).
    pub name: SmolStr,
    pub kind: DefSiteKind,
    /// The defined name's own span. For a command this is the `\name` control-word
    /// token, backslash included, so it compares equal to the same token found by an
    /// occurrence walk; for an environment it is the name text between the braces of
    /// `\newenvironment{name}` (the [`environment_name_range`] convention).
    ///
    /// [`environment_name_range`]: crate::ast::environment_name_range
    pub name_range: TextRange,
    /// The whole definition's span, through the sibling name `COMMAND` in the
    /// unbraced `\newcommand\foo…`/`\def\foo…` forms.
    pub range: TextRange,
}

/// Scan `root` for user command/environment definitions and return their *sites*, in
/// document order. Same recognizer set and name resolution as [`scan_definitions`]
/// (the [`DefKind`] dispatch and [`resolve_command_def`] sibling heuristic), but
/// keeping every definition — no last-wins collapsing, since a `\renewcommand` of an
/// earlier definition is still a definition site the user may navigate to or rename.
pub fn scan_definition_sites(root: &SyntaxNode) -> Vec<DefSite> {
    let mut sites = Vec::new();
    for command in root
        .descendants()
        .filter(|node| node.kind() == SyntaxKind::COMMAND)
    {
        let Some(name) = command_name(&command) else {
            continue;
        };
        let site = match DefKind::of(&name) {
            Some(DefKind::Command | DefKind::XparseCommand) => command_def_site(&command),
            Some(DefKind::Def) => def_def_site(&command),
            Some(DefKind::Environment | DefKind::XparseEnvironment) => {
                environment_def_site(&command)
            }
            None => None,
        };
        sites.extend(site);
    }
    sites
}

/// The [`DefSite`] of a `\newcommand`/xparse command definition, resolving the same
/// two name forms as [`resolve_command_def`]: braced `{\name}` (the control word
/// inside the name group) and unbraced `\newcommand\name` (the sibling `COMMAND`
/// hosting the signature groups).
fn command_def_site(command: &SyntaxNode) -> Option<DefSite> {
    let def = resolve_command_def(command)?;
    let name_range = if def.first_arg_group == 1 {
        let group = nth_group(command, 0)?;
        let name_command = group
            .children()
            .find(|child| child.kind() == SyntaxKind::COMMAND)?;
        control_word_range(&name_command)?
    } else {
        control_word_range(&def.host)?
    };
    Some(DefSite {
        name: SmolStr::new(&def.name),
        kind: DefSiteKind::Command,
        name_range,
        range: TextRange::new(
            command.text_range().start(),
            command.text_range().end().max(def.host.text_range().end()),
        ),
    })
}

/// The [`DefSite`] of a `\def`-family definition — the name is always the
/// immediately-following sibling `COMMAND` (TeX has no braced `\def{\name}` form).
fn def_def_site(command: &SyntaxNode) -> Option<DefSite> {
    let name_node = adjacent_sibling_command(command)?;
    let name = command_name(&name_node)?;
    let name_range = control_word_range(&name_node)?;
    Some(DefSite {
        name: SmolStr::new(&name),
        kind: DefSiteKind::Command,
        name_range,
        range: TextRange::new(command.text_range().start(), name_node.text_range().end()),
    })
}

/// The [`DefSite`] of a `\newenvironment`/xparse environment definition. The name is
/// brace-delimited *text* in group 0; the recorded span is the trimmed name within
/// the group's inner range, mirroring the `.trim()` in [`scan_newenvironment`].
fn environment_def_site(command: &SyntaxNode) -> Option<DefSite> {
    let (inner_range, text) = nth_group_inner(command, 0)?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let leading = text.len() - text.trim_start().len();
    let name_range = TextRange::at(
        inner_range.start() + TextSize::new(leading as u32),
        TextSize::new(trimmed.len() as u32),
    );
    Some(DefSite {
        name: SmolStr::new(trimmed),
        kind: DefSiteKind::Environment,
        name_range,
        range: command.text_range(),
    })
}

/// Replacement-body facts for one scanned command definition, used to detect
/// verbatim-argument commands without executing anything. We read only *static*
/// surface text of the body — no macro expansion (AGENTS.md decision #1).
struct DefBody {
    /// A catcode-othering signal appears directly in this command's own body
    /// (`\@makeother`, `\catcode…12`, `\dospecials`, …) — see [`catcode_signal`].
    signal: bool,
    /// Control words the body invokes, so chained helpers can be followed to find a
    /// catcode signal one or more hops away (jss's `\code`→helper idiom).
    called: Vec<SmolStr>,
}

/// Flag user commands whose argument is verbatim. A command is verbatim when it
/// **takes at least one argument** (so it grabs the user's `{…}` itself) **and** a
/// catcode-othering signal is reachable from its body — present directly, or in the
/// body of a scanned macro it transitively calls. Conservative by construction
/// (AGENTS.md): a wrong flag *suppresses* real diagnostics inside the body, so we
/// flag only on a clear catcode signal and otherwise leave the body ordinary.
///
/// On a match we adopt the built-in convention: only the *leading* (non-verbatim)
/// arguments stay in `args`; the final argument becomes the implicit verbatim one, so
/// we drop the last `ArgSpec` and set `verbatim = true`. This keeps the lexer's
/// `lex_verbatim_command` path uniform between built-in and user commands.
fn apply_verbatim_flags(db: &mut SignatureDb, bodies: &HashMap<SmolStr, DefBody>) {
    let verbatim: Vec<SmolStr> = bodies
        .keys()
        .filter(|name| {
            // Needs an argument of its own to capture, and a reachable signal.
            db.command(name).is_some_and(|sig| !sig.args.is_empty())
                && reaches_signal(name, bodies, &mut HashSet::new())
        })
        .cloned()
        .collect();

    for name in verbatim {
        if let Some(mut sig) = db.command(&name).cloned() {
            sig.args.to_mut().pop(); // the final argument is the implicit verbatim one
            sig.verbatim = true;
            db.insert_command(name, sig);
        }
    }
}

/// Flag user environments whose body is verbatim — the environment analog of
/// [`apply_verbatim_flags`]. An environment is verbatim when a catcode-othering signal
/// is reachable from its **begin-code** (the first definition body), directly or via a
/// chained helper command. Unlike commands, no argument is dropped: an environment's
/// declared args are all leading and its body follows the `\begin{…}…` arguments, so
/// we only flip `verbatim_body` (and the derived `reflow`). The begin-code's called
/// helpers are resolved against the *command* `bodies` map (`\newcommand`/`\def`
/// helpers live there). Conservative by construction, like the command case.
fn apply_verbatim_env_flags(
    db: &mut SignatureDb,
    env_bodies: &HashMap<SmolStr, DefBody>,
    bodies: &HashMap<SmolStr, DefBody>,
) {
    let verbatim: Vec<SmolStr> = env_bodies
        .iter()
        .filter(|(name, body)| {
            db.environment(name).is_some() && reaches_signal_body(body, bodies, &mut HashSet::new())
        })
        .map(|(name, _)| name.clone())
        .collect();

    for name in verbatim {
        if let Some(mut sig) = db.environment(&name).cloned() {
            sig.verbatim_body = true;
            sig.reflow = false; // a verbatim body is never reflowed
            db.insert_environment(name, sig);
        }
    }
}

/// Whether a catcode-othering signal is reachable from `name`'s body, following
/// chained helper macros within the scanned definition set. A `visited` set breaks
/// definition cycles (mutually recursive helpers terminate). A helper defined via
/// `\def` (not scanned) is absent from `bodies`, so the chain breaks there and we do
/// not flag — the conservative false-negative.
fn reaches_signal(
    name: &str,
    bodies: &HashMap<SmolStr, DefBody>,
    visited: &mut HashSet<SmolStr>,
) -> bool {
    if !visited.insert(SmolStr::new(name)) {
        return false;
    }
    let Some(body) = bodies.get(name) else {
        return false;
    };
    reaches_signal_body(body, bodies, visited)
}

/// Whether a catcode-othering signal is reachable from a definition `body` —
/// present directly, or in the body of a scanned command it transitively calls. The
/// body-level entry point used both by [`reaches_signal`] (after a name lookup) and by
/// [`apply_verbatim_env_flags`] (for an environment's begin-code, which has no command
/// name to look up).
fn reaches_signal_body(
    body: &DefBody,
    bodies: &HashMap<SmolStr, DefBody>,
    visited: &mut HashSet<SmolStr>,
) -> bool {
    body.signal
        || body
            .called
            .iter()
            .any(|callee| reaches_signal(callee, bodies, visited))
}

/// Whether `body` text reassigns a special char's catcode to "other" — the static
/// fingerprint of a verbatim-argument command's setup. Strict, to avoid false
/// positives (which would silence real diagnostics): each pattern is verbatim-setup
/// specific. We match surface text only; no catcode arithmetic is evaluated.
fn catcode_signal(body: &str) -> bool {
    body.contains("\\@makeother")
        || body.contains("\\@sanitize")
        || body.contains("\\dospecials")
        // `\catcode`<char>`=12` others a char; the literal `12` is the "other"
        // category. Require both tokens so an unrelated `\catcode…=11` does not match.
        || (body.contains("\\catcode") && body.contains("12"))
}

/// The control-word names (leading `\` stripped) the body invokes, for chained-helper
/// resolution. `@` is treated as a name char so `\@makeother`/`\@codex`-style helpers
/// are captured; control symbols (`\$`, `\\`) yield no name and are skipped. Reads
/// surface text only.
fn called_macros(body: &str) -> Vec<SmolStr> {
    body.match_indices('\\')
        .filter_map(|(pos, _)| {
            let after = &body[pos + 1..];
            let len: usize = after
                .chars()
                .take_while(|c| c.is_ascii_alphabetic() || *c == '@')
                .map(char::len_utf8)
                .sum();
            (len > 0).then(|| SmolStr::new(&after[..len]))
        })
        .collect()
}

/// Kernel sectioning primitives that themselves scan a `(*/[toc]/{title})` argument
/// the static scanner cannot see from a redefinition body. A `\renewcommand{\cs}{…}`
/// whose body is `\secdef …`/`\@startsection …` carries no `#` parameter and no `[n]`,
/// so [`newcommand_arity`] reads it as arity 0 — but `\cs` really does consume a prose
/// title at expansion time (jss's `\renewcommand{\section}{\secdef …}` is the canonical
/// case). Curated and deliberately narrow: a *missed* name falls back to the safe status
/// quo (the redefinition wins and the argument is left un-reflowed), while a *false*
/// match is the only way to over-trust a built-in, so we keep the set tight and match
/// only these kernel primitives.
const DELEGATING_PRIMITIVES: &[&str] = &["secdef", "@startsection", "@dblarg", "@sect", "@ssect"];

/// The **trust gate** for a `\newcommand`/`\def` the static scanner reads as taking no
/// arguments. When the body *delegates* to a token-consuming kernel primitive
/// ([`DELEGATING_PRIMITIVES`]), the arity-0 reading is provably unreliable, so it must
/// not overwrite a curated built-in with a strictly less informative 0-arg signature
/// (which would drop, e.g., a sectioning command's `prose` title and its reflow — the
/// jss-class bug). The caller keeps the built-in showing through the overlay instead.
///
/// Narrow by construction (AGENTS.md conservatism): fires only when arity is 0, the body
/// delegates, *and* a built-in exists to preserve. A genuine 0-arg redefinition has a
/// self-contained body (no delegation) and is left to win, so it correctly loses prose.
fn keeps_builtin_over_arity0(name: &str, arity: usize, body: &DefBody) -> bool {
    arity == 0
        && body
            .called
            .iter()
            .any(|callee| DELEGATING_PRIMITIVES.contains(&callee.as_str()))
        && crate::semantic::signature::builtin()
            .command(name)
            .is_some()
}

/// Which definition family a control word names, if any.
enum DefKind {
    Command,
    Def,
    Environment,
    XparseCommand,
    XparseEnvironment,
}

impl DefKind {
    fn of(name: &str) -> Option<Self> {
        Some(match name {
            "newcommand" | "renewcommand" | "providecommand" | "DeclareRobustCommand" => {
                DefKind::Command
            }
            // Plain TeX `\def` and its global/expanded variants. `\let` is excluded: it
            // aliases an existing meaning rather than carrying a replacement body to scan.
            "def" | "edef" | "gdef" | "xdef" => DefKind::Def,
            "newenvironment" | "renewenvironment" => DefKind::Environment,
            "NewDocumentCommand"
            | "RenewDocumentCommand"
            | "ProvideDocumentCommand"
            | "DeclareDocumentCommand" => DefKind::XparseCommand,
            "NewDocumentEnvironment"
            | "RenewDocumentEnvironment"
            | "ProvideDocumentEnvironment"
            | "DeclareDocumentEnvironment" => DefKind::XparseEnvironment,
            _ => return None,
        })
    }
}

/// `\newcommand{\name}[n][default]{body}` → a [`CommandSig`]. The name is the
/// control word in the first group; `[n]` (if present) is the arg count, and a
/// second optional `[default]` makes the first argument optional `[…]` while the
/// rest are mandatory `{…}` — LaTeX2e's `\newcommand` shape. The unbraced
/// `\newcommand\name[n]…` form is recovered the same way via [`resolve_command_def`].
fn scan_newcommand(
    command: &SyntaxNode,
    db: &mut SignatureDb,
    bodies: &mut HashMap<SmolStr, DefBody>,
) {
    let Some(def) = resolve_command_def(command) else {
        return;
    };
    let (arity, first_optional) = newcommand_arity(&def.host);
    // The replacement body is the group right after the name: index `first_arg_group`
    // on the host (group 1 for the braced form, group 0 for the unbraced sibling).
    record_body(
        bodies,
        &def.name,
        nth_group(&def.host, def.first_arg_group).as_ref(),
    );
    // Trust gate: a `\secdef`/`\@startsection`-style body reads as arity 0 but really
    // consumes a title, so don't let it downgrade a curated built-in (keep the overlay
    // falling through to the built-in). See [`keeps_builtin_over_arity0`].
    if bodies
        .get(def.name.as_str())
        .is_some_and(|body| keeps_builtin_over_arity0(&def.name, arity, body))
    {
        return;
    }
    db.insert_command(
        def.name,
        CommandSig {
            args: latex2e_args(arity, first_optional).into(),
            sectioning: None,
            verbatim: false,
            rule: false,
            inline: false,
        },
    );
}

/// `\def\name<param text>{body}` (and the `\edef`/`\gdef`/`\xdef` variants) → a
/// [`CommandSig`]. `\def` has only the unbraced name form (TeX has no `\def{\name}`), so
/// the name is the immediately-following sibling `COMMAND`. The arity comes from the
/// **parameter text** (`#1#2…`) between the name and the body — counted by
/// [`def_params_and_body`] — not from a `[n]` optional. We record the body for the same
/// catcode-signal/helper-chain analysis as `\newcommand`, which is what lets a `\def`
/// helper participate in chain resolution ([`reaches_signal`]).
fn scan_def(command: &SyntaxNode, db: &mut SignatureDb, bodies: &mut HashMap<SmolStr, DefBody>) {
    let Some(name_node) = adjacent_sibling_command(command) else {
        return;
    };
    let Some(name) = command_name(&name_node) else {
        return;
    };
    let (arity, body) = def_params_and_body(&name_node);
    record_body(bodies, &name, body.as_ref());
    // Trust gate: same as `scan_newcommand` — a delegating `\def\section{\secdef …}`
    // must not downgrade a curated built-in. See [`keeps_builtin_over_arity0`].
    if bodies
        .get(name.as_str())
        .is_some_and(|body| keeps_builtin_over_arity0(&name, arity, body))
    {
        return;
    }
    db.insert_command(
        name,
        CommandSig {
            // `\def` parameters carry no brace/bracket distinction; model them as the same
            // all-mandatory-brace shape scanned `\newcommand`s use. `apply_verbatim_flags`
            // pops the final slot and sets `verbatim` if a catcode signal is reachable.
            args: latex2e_args(arity, false).into(),
            sectioning: None,
            verbatim: false,
            rule: false,
            inline: false,
        },
    );
}

/// The `(arity, body)` of a `\def`-style definition, reading its parameter text off the
/// name `COMMAND` node. Two CST shapes arise under greedy attachment:
/// - **No parameters** (`\def\foo{body}`): the body brace group attaches as `\foo`'s first
///   child `GROUP`, so arity is `0` and the body is `nth_group(name_node, 0)`.
/// - **With parameters** (`\def\foo#1#2{body}`): the leading `#` (`HASH`) breaks greedy
///   attachment, so `\foo` has no child group and the `#1`, `#2`, and `{body}` are all
///   siblings. Arity is the number of `HASH` tokens (each `#1` lexes as `HASH` + `WORD`)
///   before the first sibling `GROUP`, which is the body.
///
/// Anything other than trivia/`HASH`/`WORD` before a group means delimited or malformed
/// parameter text we do not model; we stop and report no body (so no catcode signal is
/// recorded for it — the conservative choice). Arity is capped at 9 like `\newcommand`.
fn def_params_and_body(name_node: &SyntaxNode) -> (usize, Option<SyntaxNode>) {
    // No parameter text: the body attached greedily as the name command's first group.
    if let Some(body) = nth_group(name_node, 0) {
        return (0, Some(body));
    }
    // Parameter text intervened: count `#` markers up to the first sibling group (the body).
    let mut arity = 0usize;
    let mut next = name_node.next_sibling_or_token();
    while let Some(element) = next {
        match element {
            NodeOrToken::Token(token) if is_trivia(token.kind()) => {
                next = token.next_sibling_or_token();
            }
            NodeOrToken::Token(token) if token.kind() == SyntaxKind::HASH => {
                arity += 1;
                next = token.next_sibling_or_token();
            }
            // The digit following `#`, or a literal delimiter token in a delimited macro.
            NodeOrToken::Token(token) if token.kind() == SyntaxKind::WORD => {
                next = token.next_sibling_or_token();
            }
            NodeOrToken::Node(node) if node.kind() == SyntaxKind::GROUP => {
                return (arity.min(9), Some(node));
            }
            _ => return (arity.min(9), None),
        }
    }
    (arity.min(9), None)
}

/// Record the catcode/called-macro facts of a command definition's replacement
/// `body` group (absent or unresolvable body → no signal, no calls).
fn record_body(bodies: &mut HashMap<SmolStr, DefBody>, name: &str, body: Option<&SyntaxNode>) {
    let text = body.map(group_inner_source).unwrap_or_default();
    bodies.insert(
        SmolStr::new(name),
        DefBody {
            signal: catcode_signal(&text),
            called: called_macros(&text),
        },
    );
}

/// `\newenvironment{name}[n][default]{begin}{end}` → an [`EnvironmentSig`]. Same
/// arg-count shape as [`scan_newcommand`]. The begin-code (group 1 — the optionals
/// `[n][default]` are `OPTIONAL` nodes, so they don't shift `nth_group` indexing) is
/// recorded so [`apply_verbatim_env_flags`] can flag a catcode-othering body verbatim.
fn scan_newenvironment(
    command: &SyntaxNode,
    db: &mut SignatureDb,
    env_bodies: &mut HashMap<SmolStr, DefBody>,
) {
    let Some(name) = nth_group_text(command, 0) else {
        return;
    };
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    record_body(env_bodies, name, nth_group(command, 1).as_ref());
    let (arity, first_optional) = newcommand_arity(command);
    db.insert_environment(name, environment_sig(latex2e_args(arity, first_optional)));
}

/// `\NewDocumentCommand{\name}{spec}{body}` → a [`CommandSig`] with args from the
/// xparse spec. The unbraced `\NewDocumentCommand\name{spec}…` form is recovered the
/// same way via [`resolve_command_def`]; `first_arg_group` indexes the spec group on
/// whichever node hosts the arguments.
fn scan_xparse_command(
    command: &SyntaxNode,
    db: &mut SignatureDb,
    bodies: &mut HashMap<SmolStr, DefBody>,
) {
    let Some(def) = resolve_command_def(command) else {
        return;
    };
    let Some(spec) = nth_group(&def.host, def.first_arg_group) else {
        return;
    };
    // The body follows the spec group, so it sits one index further along.
    record_body(
        bodies,
        &def.name,
        nth_group(&def.host, def.first_arg_group + 1).as_ref(),
    );
    db.insert_command(
        def.name,
        CommandSig {
            args: xparse::parse_spec(&group_inner_source(&spec)).into(),
            sectioning: None,
            verbatim: false,
            rule: false,
            inline: false,
        },
    );
}

/// A resolved command definition: the defined `name`, the node whose attached
/// `OPTIONAL`/`GROUP` children carry the argument shape (`host`), and the index of
/// the first *signature* group on that host.
///
/// Two name forms collapse to this shape:
/// - **Braced** `\newcommand{\foo}…`: the host is the definition command itself; its
///   group 0 is the `{\foo}` name, so signature groups start at index `1`.
/// - **Unbraced** `\newcommand\foo…`: greedy attachment makes `\foo` the next sibling
///   `COMMAND` and hangs the `[n]`/`{body}` (or xparse spec) off *it*, so the host is
///   that sibling and signature groups start at index `0`.
struct CommandDef {
    name: String,
    host: SyntaxNode,
    first_arg_group: usize,
}

/// Resolve `command` (a `\newcommand`/xparse definition) to its [`CommandDef`],
/// handling both the braced and unbraced name forms. Returns `None` when no command
/// name can be read (a malformed or empty definition) — the scan then skips it.
fn resolve_command_def(command: &SyntaxNode) -> Option<CommandDef> {
    // Braced `{\name}`: the name control word lives in the first group, and every
    // attached group/optional hangs off the definition command itself.
    if command.children().any(|c| c.kind() == SyntaxKind::GROUP) {
        let name = nth_group(command, 0)
            .as_ref()
            .and_then(group_command_name)?;
        return Some(CommandDef {
            name,
            host: command.clone(),
            first_arg_group: 1,
        });
    }
    // Unbraced `\newcommand\foo…`: read the name and signature groups off the
    // following sibling `COMMAND` (decision #2 — a scanner heuristic, no parser
    // change).
    let sibling = adjacent_sibling_command(command)?;
    let name = command_name(&sibling)?;
    Some(CommandDef {
        name,
        host: sibling,
        first_arg_group: 0,
    })
}

/// The immediately-following sibling `COMMAND`, separated from `command` by trivia
/// only. Returns `None` if any non-trivia element intervenes, so `\newcommand\foo`
/// (and the spaced `\newcommand \foo`) bind, but `\newcommand stray text \bar` does
/// not. A blank line cannot reach here: the `\par` break splits the two commands into
/// separate `PARAGRAPH` parents, so there is no sibling to find.
fn adjacent_sibling_command(command: &SyntaxNode) -> Option<SyntaxNode> {
    let mut next = command.next_sibling_or_token();
    while let Some(element) = next {
        match element {
            NodeOrToken::Token(token) if is_trivia(token.kind()) => {
                next = token.next_sibling_or_token();
            }
            NodeOrToken::Node(node) if node.kind() == SyntaxKind::COMMAND => return Some(node),
            _ => return None,
        }
    }
    None
}

/// Whether `kind` is trivia (whitespace/newline/comment). Mirrors the parser's
/// private `Parser::is_trivia`; the trivia set is fixed by AGENTS.md decision #9.
fn is_trivia(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
    )
}

/// `\NewDocumentEnvironment{name}{spec}{begin}{end}` → an [`EnvironmentSig`] with
/// args from the xparse spec. The begin-code (group 2 — after `{name}` and `{spec}`)
/// is recorded for verbatim detection, as in [`scan_newenvironment`].
fn scan_xparse_environment(
    command: &SyntaxNode,
    db: &mut SignatureDb,
    env_bodies: &mut HashMap<SmolStr, DefBody>,
) {
    let Some(name) = nth_group_text(command, 0) else {
        return;
    };
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    let Some(spec) = nth_group(command, 1) else {
        return;
    };
    record_body(env_bodies, name, nth_group(command, 2).as_ref());
    db.insert_environment(
        name,
        environment_sig(xparse::parse_spec(&group_inner_source(&spec))),
    );
}

/// The `(arity, first_arg_optional)` pair for a LaTeX2e definition: the integer in
/// the first `[…]` optional (default `0`), and whether a *second* optional is
/// present (which makes the first argument optional).
fn newcommand_arity(command: &SyntaxNode) -> (usize, bool) {
    let optionals: Vec<SyntaxNode> = command
        .children()
        .filter(|child| child.kind() == SyntaxKind::OPTIONAL)
        .collect();
    let arity = optionals
        .first()
        .and_then(optional_number)
        .unwrap_or(0)
        .min(9); // LaTeX caps macro arity at 9.
    (arity, optionals.len() >= 2)
}

/// The integer inside an `OPTIONAL` node (`[2]` → `2`), or `None` if it isn't a
/// bare number.
fn optional_number(node: &SyntaxNode) -> Option<usize> {
    let text = node.text().to_string();
    let inner = text.strip_prefix('[').unwrap_or(&text);
    let inner = inner.strip_suffix(']').unwrap_or(inner);
    inner.trim().parse().ok()
}

/// Build the LaTeX2e argument slots: `arity` arguments, all mandatory `{…}` unless
/// `first_optional`, in which case the first is optional `[…]`.
fn latex2e_args(arity: usize, first_optional: bool) -> Vec<ArgSpec> {
    (0..arity)
        .map(|i| {
            if i == 0 && first_optional {
                ArgSpec {
                    required: false,
                    kind: ArgKind::Bracket,
                    content: ContentKind::Opaque,
                }
            } else {
                ArgSpec {
                    required: true,
                    kind: ArgKind::Brace,
                    content: ContentKind::Opaque,
                }
            }
        })
        .collect()
}

/// An [`EnvironmentSig`] for a scanned environment with the given args: a
/// reflowable, non-math, non-verbatim body (the only shape LaTeX2e/xparse
/// definitions give us without package-specific knowledge).
fn environment_sig(args: Vec<ArgSpec>) -> EnvironmentSig {
    EnvironmentSig {
        args: args.into(),
        verbatim_body: false,
        math: false,
        code: false,
        align: false,
        reflow: true,
        no_indent: false,
        // A user `\newenvironment` is not assumed to be a list; the built-in DB
        // is the source of truth for `\item`-bearing list layout.
        list: false,
        // Block-ness of a user-defined environment is unknown without
        // package-specific knowledge; default to non-block (the parser keeps the
        // conservative `PARAGRAPH` wrapper for it).
        block: false,
        // A scanned user environment carries no outline category; only the curated
        // built-in floats/theorem-likes show up in the document-symbol outline.
        outline: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{parse, reconstruct};

    fn db_of(src: &str) -> SignatureDb {
        // New parser-adjacent feature: assert losslessness on every input.
        assert_eq!(reconstruct(src), src, "reconstruct must round-trip");
        scan_definitions(&SyntaxNode::new_root(parse(src).green))
    }

    fn arg_kinds(args: &[ArgSpec]) -> Vec<ArgKind> {
        args.iter().map(|a| a.kind).collect()
    }

    #[test]
    fn newcommand_counts_mandatory_args() {
        let db = db_of("\\newcommand{\\foo}[2]{#1#2}\n");
        let sig = db.command("foo").expect("foo defined");
        assert_eq!(arg_kinds(&sig.args), vec![ArgKind::Brace, ArgKind::Brace]);
        assert!(sig.args.iter().all(|a| a.required));
    }

    #[test]
    fn newcommand_optional_first_arg() {
        let db = db_of("\\newcommand{\\foo}[2][d]{#1#2}\n");
        let sig = db.command("foo").expect("foo defined");
        assert_eq!(arg_kinds(&sig.args), vec![ArgKind::Bracket, ArgKind::Brace]);
        assert!(!sig.args[0].required);
        assert!(sig.args[1].required);
    }

    #[test]
    fn newcommand_zero_args() {
        let db = db_of("\\newcommand{\\foo}{bar}\n");
        assert!(db.command("foo").expect("foo defined").args.is_empty());
    }

    #[test]
    fn renew_and_provide_recognized() {
        let db = db_of("\\renewcommand{\\a}[1]{x}\\providecommand{\\b}[1]{y}\n");
        assert_eq!(db.command("a").unwrap().args.len(), 1);
        assert_eq!(db.command("b").unwrap().args.len(), 1);
    }

    #[test]
    fn secdef_redefinition_keeps_builtin_prose() {
        // jss.cls does `\renewcommand{\section}{\secdef \jsssimplesec \jsssimplesecnn}`.
        // The static scanner reads this as arity 0, but `\secdef` consumes the title at
        // expansion time, so the trust gate must *not* record a 0-arg override — the
        // curated built-in prose signature has to survive through the overlay.
        let db = db_of("\\renewcommand{\\section}{\\secdef \\a \\b}\n");
        assert!(
            db.command("section").is_none(),
            "the delegating redefinition must not be recorded as a scanned override"
        );
        let sigs = crate::semantic::signature::Signatures::new(&db);
        let sig = sigs.command("section").expect("built-in section survives");
        let last = sig.args.last().expect("section keeps its title argument");
        assert_eq!(
            last.content,
            crate::semantic::signature::ContentKind::Prose,
            "the title argument stays prose (reflowable)"
        );
    }

    #[test]
    fn genuine_zero_arg_redefinition_downgrades_builtin() {
        // A self-contained body with no delegation genuinely drops the argument, so the
        // 0-arg reading is trustworthy and *must* override the built-in — the gate must
        // not fire here (the failure mode the trust gate is careful to avoid).
        let db = db_of("\\renewcommand{\\section}{\\textbf{Fixed}}\n");
        let sig = db
            .command("section")
            .expect("genuine 0-arg redefinition is recorded");
        assert!(
            sig.args.is_empty(),
            "no delegation means the scanned 0-arg signature wins"
        );
    }

    #[test]
    fn secdef_redefinition_of_unknown_still_records() {
        // The gate only protects a *curated built-in*: a delegating redefinition of a
        // name with no built-in has nothing to preserve, so it records as normal (arity
        // 0), keeping the name available to completion.
        let db = db_of("\\renewcommand{\\mysec}{\\secdef \\a \\b}\n");
        let sig = db.command("mysec").expect("unknown name is still recorded");
        assert!(sig.args.is_empty());
    }

    #[test]
    fn newenvironment_args() {
        let db = db_of("\\newenvironment{thm}[1]{begin #1}{end}\n");
        let sig = db.environment("thm").expect("thm defined");
        assert_eq!(arg_kinds(&sig.args), vec![ArgKind::Brace]);
        assert!(sig.reflow);
        assert!(!sig.verbatim_body);
        assert!(!sig.math);
    }

    #[test]
    fn xparse_command_spec() {
        let db = db_of("\\NewDocumentCommand{\\foo}{m O{d} m}{x}\n");
        let sig = db.command("foo").expect("foo defined");
        assert_eq!(
            arg_kinds(&sig.args),
            vec![ArgKind::Brace, ArgKind::Bracket, ArgKind::Brace]
        );
    }

    #[test]
    fn xparse_environment_spec() {
        let db = db_of("\\NewDocumentEnvironment{env}{O{x} m}{a}{b}\n");
        let sig = db.environment("env").expect("env defined");
        assert_eq!(arg_kinds(&sig.args), vec![ArgKind::Bracket, ArgKind::Brace]);
    }

    #[test]
    fn unbraced_newcommand_extracted() {
        // `\newcommand\foo[2]{…}` parses with `\foo` as a sibling carrying the `[2]`;
        // the scanner reads the signature off that sibling.
        let db = db_of("\\newcommand\\foo[2]{#1#2}\n");
        let sig = db.command("foo").expect("foo defined");
        assert_eq!(arg_kinds(&sig.args), vec![ArgKind::Brace, ArgKind::Brace]);
        assert!(sig.args.iter().all(|a| a.required));
    }

    #[test]
    fn unbraced_optional_first_arg() {
        let db = db_of("\\newcommand\\foo[2][d]{#1#2}\n");
        let sig = db.command("foo").expect("foo defined");
        assert_eq!(arg_kinds(&sig.args), vec![ArgKind::Bracket, ArgKind::Brace]);
        assert!(!sig.args[0].required);
        assert!(sig.args[1].required);
    }

    #[test]
    fn unbraced_zero_args() {
        let db = db_of("\\newcommand\\foo{x}\n");
        assert!(db.command("foo").expect("foo defined").args.is_empty());
    }

    #[test]
    fn unbraced_spaced_binds() {
        // Trivia between the keyword and the name still binds.
        let db = db_of("\\newcommand \\foo[1]{x}\n");
        assert_eq!(db.command("foo").unwrap().args.len(), 1);
    }

    #[test]
    fn unbraced_renewcommand() {
        let db = db_of("\\renewcommand\\foo[1]{x}\n");
        assert_eq!(db.command("foo").unwrap().args.len(), 1);
    }

    #[test]
    fn unbraced_xparse_command() {
        let db = db_of("\\NewDocumentCommand\\foo{m O{d} m}{x}\n");
        let sig = db.command("foo").expect("foo defined");
        assert_eq!(
            arg_kinds(&sig.args),
            vec![ArgKind::Brace, ArgKind::Bracket, ArgKind::Brace]
        );
    }

    #[test]
    fn unbraced_stray_text_not_bound() {
        // Non-trivia text between the keyword and a later command breaks the bind:
        // neither name is a definition target.
        let db = db_of("\\newcommand foo \\bar{x}\n");
        assert!(db.command("foo").is_none());
        assert!(db.command("bar").is_none());
    }

    #[test]
    fn redefinition_last_wins() {
        let db = db_of("\\newcommand{\\foo}[1]{x}\\renewcommand{\\foo}[3]{y}\n");
        assert_eq!(db.command("foo").unwrap().args.len(), 3);
    }

    #[test]
    fn garbage_definition_degrades_to_no_insert() {
        // No name group at all: nothing inserted, no panic.
        let db = db_of("\\newcommand\n");
        assert!(db.command("foo").is_none());
    }

    #[test]
    fn nested_definition_collected() {
        let db = db_of("\\begin{document}\n\\newcommand{\\foo}[1]{x}\n\\end{document}\n");
        assert_eq!(db.command("foo").unwrap().args.len(), 1);
    }

    #[test]
    fn commented_definition_ignored() {
        let db = db_of("% \\newcommand{\\foo}[1]{x}\n");
        assert!(db.command("foo").is_none());
    }

    #[test]
    fn verbatim_makeother_flagged() {
        // `\@makeother\$` in the body others `$`, so the argument is verbatim. The
        // single argument becomes the implicit verbatim one, leaving no leading args.
        let db = db_of("\\newcommand\\shellcmd[1]{\\@makeother\\$#1}\n");
        let sig = db.command("shellcmd").expect("shellcmd defined");
        assert!(sig.verbatim);
        assert!(sig.args.is_empty());
    }

    #[test]
    fn verbatim_catcode_flagged() {
        // A `\catcode … 12` ("other") assignment is the same signal.
        let db = db_of("\\newcommand\\shellcmd[1]{\\catcode 36=12 #1}\n");
        assert!(db.command("shellcmd").expect("shellcmd defined").verbatim);
    }

    #[test]
    fn verbatim_dospecials_flagged() {
        // The classic verbatim setup loop.
        let db = db_of("\\newcommand\\shellcmd[1]{\\let\\do\\@makeother\\dospecials #1}\n");
        assert!(db.command("shellcmd").expect("shellcmd defined").verbatim);
    }

    #[test]
    fn verbatim_keeps_leading_args() {
        // Only the *final* argument is verbatim: a two-arg command keeps its first
        // (leading) slot and drops the last as the implicit verbatim argument.
        let db = db_of("\\newcommand\\mycode[2]{\\@makeother\\$#1#2}\n");
        let sig = db.command("mycode").expect("mycode defined");
        assert!(sig.verbatim);
        assert_eq!(arg_kinds(&sig.args), vec![ArgKind::Brace]);
    }

    #[test]
    fn verbatim_via_chained_helper() {
        // The catcode signal lives in a helper the command calls, not in its own
        // body; the chain is followed across scanned definitions.
        let db =
            db_of("\\newcommand\\setup{\\@makeother\\$}\\newcommand\\shellcmd[1]{\\setup#1}\n");
        assert!(db.command("shellcmd").expect("shellcmd defined").verbatim);
        // The arity-0 helper itself takes no argument, so it is never flagged.
        assert!(!db.command("setup").expect("setup defined").verbatim);
    }

    #[test]
    fn verbatim_chain_cycle_terminates() {
        // Mutually recursive helpers with no signal must terminate (visited guard)
        // and flag neither command.
        let db = db_of("\\newcommand\\a[1]{\\b#1}\\newcommand\\b[1]{\\a#1}\n");
        assert!(!db.command("a").expect("a defined").verbatim);
        assert!(!db.command("b").expect("b defined").verbatim);
    }

    #[test]
    fn ordinary_command_not_verbatim() {
        let db = db_of("\\newcommand\\foo[1]{\\emph{#1}}\n");
        assert!(!db.command("foo").expect("foo defined").verbatim);
    }

    #[test]
    fn verbatim_needs_an_argument() {
        // An arity-0 command grabs no `{…}` of its own, so a catcode signal in its
        // body does not make it a verbatim-*argument* command.
        let db = db_of("\\newcommand\\setup{\\@makeother\\$}\n");
        assert!(!db.command("setup").expect("setup defined").verbatim);
    }

    #[test]
    fn def_helper_chain_followed() {
        // The helper is defined with `\def`; its body is now scanned, so the chain from
        // `\shellcmd` through `\setup` to the catcode signal resolves and flags the caller.
        let db = db_of("\\def\\setup{\\@makeother\\$}\\newcommand\\shellcmd[1]{\\setup#1}\n");
        assert!(db.command("shellcmd").expect("shellcmd defined").verbatim);
        // The arity-0 helper itself takes no argument, so it is never flagged.
        assert!(!db.command("setup").expect("setup defined").verbatim);
    }

    #[test]
    fn def_direct_verbatim_flagged() {
        // A `\def` command whose own body others a special char is verbatim; its single
        // parameter becomes the implicit verbatim argument, leaving no leading args.
        let db = db_of("\\def\\shellcmd#1{\\@makeother\\$#1}\n");
        let sig = db.command("shellcmd").expect("shellcmd defined");
        assert!(sig.verbatim);
        assert!(sig.args.is_empty());
    }

    #[test]
    fn def_zero_params() {
        // No parameter text: the body attaches as the name command's child group.
        let db = db_of("\\def\\foo{x}\n");
        let sig = db.command("foo").expect("foo defined");
        assert!(sig.args.is_empty());
        assert!(!sig.verbatim);
    }

    #[test]
    fn def_counts_params() {
        // `#1#2` parameter text → arity 2, all mandatory brace slots.
        let db = db_of("\\def\\foo#1#2{#1#2}\n");
        let sig = db.command("foo").expect("foo defined");
        assert_eq!(arg_kinds(&sig.args), vec![ArgKind::Brace, ArgKind::Brace]);
    }

    #[test]
    fn def_variants_scanned() {
        // `\edef`/`\gdef`/`\xdef` share `\def`'s shape and are scanned the same way.
        let db = db_of("\\edef\\a#1{x}\\gdef\\b{y}\\xdef\\c#1{\\@makeother\\$#1}\n");
        assert_eq!(db.command("a").expect("a defined").args.len(), 1);
        assert!(db.command("b").expect("b defined").args.is_empty());
        let c = db.command("c").expect("c defined");
        assert!(c.verbatim);
        assert!(c.args.is_empty());
    }

    #[test]
    fn def_chain_through_def_helpers() {
        // A `\def` → `\def` helper chain still reaches the signal and flags the caller.
        let db = db_of(
            "\\def\\inner{\\@makeother\\$}\\def\\outer{\\inner}\\newcommand\\cmd[1]{\\outer#1}\n",
        );
        assert!(db.command("cmd").expect("cmd defined").verbatim);
    }

    #[test]
    fn verbatim_xparse_flagged() {
        let db = db_of("\\NewDocumentCommand\\shellcmd{m}{\\@makeother\\$#1}\n");
        let sig = db.command("shellcmd").expect("shellcmd defined");
        assert!(sig.verbatim);
        assert!(sig.args.is_empty());
    }

    #[test]
    fn env_makeother_flagged() {
        // `\@makeother\$` in the begin-code others `$`, so the environment body is
        // verbatim. The environment analog of `verbatim_makeother_flagged`.
        let db = db_of("\\newenvironment{shellenv}{\\@makeother\\$}{}\n");
        let sig = db.environment("shellenv").expect("shellenv defined");
        assert!(sig.verbatim_body);
        assert!(!sig.reflow); // a verbatim body is never reflowed
    }

    #[test]
    fn env_catcode_flagged() {
        // A `\catcode … 12` ("other") assignment in the begin-code is the same signal.
        let db = db_of("\\newenvironment{shellenv}[1]{\\catcode 36=12 }{}\n");
        let sig = db.environment("shellenv").expect("shellenv defined");
        assert!(sig.verbatim_body);
        // Declared args are kept (they are all leading; the body follows them).
        assert_eq!(arg_kinds(&sig.args), vec![ArgKind::Brace]);
    }

    #[test]
    fn env_via_chained_helper() {
        // The catcode signal lives in a helper the begin-code calls, not in the
        // begin-code itself; the chain is followed through the command bodies map.
        let db =
            db_of("\\newcommand\\setup{\\@makeother\\$}\\newenvironment{shellenv}{\\setup}{}\n");
        assert!(
            db.environment("shellenv")
                .expect("shellenv defined")
                .verbatim_body
        );
    }

    #[test]
    fn env_without_signal_not_flagged() {
        // An ordinary `\newenvironment` with no catcode setup stays reflowable.
        let db = db_of("\\newenvironment{remark}{\\par\\noindent\\textbf{Remark.}}{\\par}\n");
        let sig = db.environment("remark").expect("remark defined");
        assert!(!sig.verbatim_body);
        assert!(sig.reflow);
    }

    #[test]
    fn xparse_env_makeother_flagged() {
        // `\NewDocumentEnvironment`: the begin-code is group 2 (after name and spec).
        let db = db_of("\\NewDocumentEnvironment{shellenv}{O{x}}{\\dospecials}{}\n");
        let sig = db.environment("shellenv").expect("shellenv defined");
        assert!(sig.verbatim_body);
        assert_eq!(arg_kinds(&sig.args), vec![ArgKind::Bracket]);
    }

    fn sites_of(src: &str) -> Vec<DefSite> {
        assert_eq!(reconstruct(src), src, "reconstruct must round-trip");
        scan_definition_sites(&SyntaxNode::new_root(parse(src).green))
    }

    #[test]
    fn def_site_newcommand_braced_name_span() {
        let src = "\\newcommand{\\foo}[1]{#1}\n";
        let sites = sites_of(src);
        assert_eq!(sites.len(), 1);
        let site = &sites[0];
        assert_eq!(site.name, "foo");
        assert_eq!(site.kind, DefSiteKind::Command);
        assert_eq!(&src[site.name_range], "\\foo");
        assert_eq!(&src[site.range], "\\newcommand{\\foo}[1]{#1}");
    }

    #[test]
    fn def_site_newcommand_unbraced_name_span() {
        let src = "\\newcommand\\foo[1]{#1}\n";
        let sites = sites_of(src);
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].name, "foo");
        assert_eq!(&src[sites[0].name_range], "\\foo");
        assert_eq!(&src[sites[0].range], "\\newcommand\\foo[1]{#1}");
    }

    #[test]
    fn def_site_def_sibling_name_span() {
        let src = "\\def\\foo#1{#1}\n";
        let sites = sites_of(src);
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].name, "foo");
        assert_eq!(sites[0].kind, DefSiteKind::Command);
        assert_eq!(&src[sites[0].name_range], "\\foo");
    }

    #[test]
    fn def_site_xparse_command_name_span() {
        let src = "\\NewDocumentCommand{\\foo}{m O{d}}{x}\n";
        let sites = sites_of(src);
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].name, "foo");
        assert_eq!(&src[sites[0].name_range], "\\foo");
    }

    #[test]
    fn def_site_newenvironment_name_span() {
        let src = "\\newenvironment{myenv}{begin}{end}\n";
        let sites = sites_of(src);
        assert_eq!(sites.len(), 1);
        let site = &sites[0];
        assert_eq!(site.name, "myenv");
        assert_eq!(site.kind, DefSiteKind::Environment);
        assert_eq!(&src[site.name_range], "myenv");
    }

    #[test]
    fn def_site_xparse_environment_name_span() {
        let src = "\\NewDocumentEnvironment{myenv}{m}{a}{b}\n";
        let sites = sites_of(src);
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].name, "myenv");
        assert_eq!(sites[0].kind, DefSiteKind::Environment);
        assert_eq!(&src[sites[0].name_range], "myenv");
    }

    #[test]
    fn def_site_keeps_every_redefinition() {
        // Unlike `scan_definitions` (last wins), every site is a navigation target.
        let src = "\\newcommand{\\foo}{a}\n\\renewcommand{\\foo}{b}\n";
        let sites = sites_of(src);
        assert_eq!(sites.len(), 2);
        assert!(sites.iter().all(|s| s.name == "foo"));
        assert!(sites[0].name_range.start() < sites[1].name_range.start());
    }

    #[test]
    fn def_site_none_for_malformed() {
        assert!(sites_of("\\newcommand\n").is_empty());
        assert!(sites_of("\\newenvironment{}{a}{b}\n").is_empty());
    }
}
