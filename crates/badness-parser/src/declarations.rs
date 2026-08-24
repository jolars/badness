//! Project declarations for constructs syntax or semantics cannot infer from
//! source without macro expansion.
//!
//! Declarations supplement the aliases found by [`crate::semantic::define`].
//! They can describe environments defined in another file or through constructs
//! the definition scanner does not recognize.
//!
//! Environment declarations name spellings, not pairings: shape gates still
//! decide whether the source supports a construct. Command declarations are
//! semantic-only ref/cite aliases and never affect tree shape.
//!
//! The schema follows three rules:
//!
//! 1. Each syntactic category has its own name map.
//! 2. `like` copies a built-in entry from the same category. Cross-category
//!    relationships use explicit fields such as [`EnvironmentDecl::begin`].
//! 3. [`Declarations::resolve`] performs validation after deserialization so
//!    errors can identify the original configuration key.
//!
//! These types are shared by all parser front ends. Their serialized field names
//! are public API.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

use crate::parser::lexer::is_control_word_name;
use crate::semantic::builder::{is_cite_command, ref_command};
use crate::semantic::signature::{EnvironmentSig, SignatureDb, builtin};

/// A control-word name as written in a declaration, stored **without** the
/// leading backslash — the spelling every signature and `ParseCtx` map is keyed
/// by.
///
/// Users write `\bea`, which in TOML wants a literal string (`'\bea'`) to avoid
/// escaping. Both spellings are accepted and normalize to the same value: a
/// control word can never itself contain a backslash, so there is nothing to
/// disambiguate. Normalization lives in the type rather than at one call site so
/// every front end gets it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct CommandName(SmolStr);

impl CommandName {
    /// Normalize `name` by stripping one leading backslash, if present.
    pub fn new(name: &str) -> Self {
        Self(SmolStr::new(name.strip_prefix('\\').unwrap_or(name)))
    }

    /// The name without its leading backslash.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for CommandName {
    fn from(name: &str) -> Self {
        Self::new(name)
    }
}

impl fmt::Display for CommandName {
    /// Renders *with* the backslash, since that is how a diagnostic should spell
    /// it back to the user.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\\{}", self.0)
    }
}

impl<'de> Deserialize<'de> for CommandName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(Self::new(&raw))
    }
}

/// One `[environments.<name>]` entry: what the environment named by the key
/// behaves like, and which command spellings stand in for its delimiters.
///
/// The key is the environment's *own* name, whether or not it is one the
/// built-in database knows. That is what lets a single entry serve both shapes
/// the issue asked for — `\begin{myenv} … \end{myenv}` needing only behavior,
/// and `\startmyenv … \endmyenv` needing behavior *and* spellings — without a
/// union-typed entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct EnvironmentDecl {
    /// The curated built-in environment whose behavior this one copies — math,
    /// alignment, list-ness, verbatim-ness, and every flag added later.
    ///
    /// Resolved against the built-in database alone, never the CWL tier or
    /// scanned definitions, for the same reason the alias arm of
    /// `Signatures::environment_at` is: a declaration supplies a *spelling*, and
    /// behavior always comes from curated data. An unknown target is an error
    /// rather than a silent no-op, because a mistyped `like = "algin"` is
    /// otherwise invisible.
    pub like: Option<SmolStr>,
    /// Command spellings that stand in for this environment's `\begin{…}`
    /// (`\bea`, `\startmyenv`). Any of them opens the environment; the closers
    /// in [`end`](Self::end) close it — and so does the literal `\end{…}`, which
    /// is why either list may stand alone (issue #117).
    pub begin: Vec<CommandName>,
    /// Command spellings that stand in for this environment's `\end{…}`. Kept a
    /// separate list rather than begin/end tuples because pairing is by *kind*,
    /// not by index: `\bea … \eea` pairs whichever spellings the author used.
    pub end: Vec<CommandName>,
}

impl EnvironmentDecl {
    /// Whether this entry declares delimiter spellings (as opposed to behavior
    /// alone).
    pub fn has_delimiters(&self) -> bool {
        !self.begin.is_empty() || !self.end.is_empty()
    }
}

/// The name-keyed `[environments]` map. A type alias so the CLI's `Config` can
/// name the field's type without restating the key type.
pub type EnvironmentDecls = BTreeMap<SmolStr, EnvironmentDecl>;

/// One `[commands.<name>]` entry: the built-in reference or citation command
/// whose semantic behavior the project command copies.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct CommandDecl {
    /// The built-in reference or citation command whose key behavior is copied.
    pub like: Option<SmolStr>,
}

/// The name-keyed `[commands]` declaration map.
pub type CommandDecls = BTreeMap<SmolStr, CommandDecl>;

/// Every declaration a project makes, as authored — unresolved and unvalidated.
///
/// `BTreeMap` rather than `HashMap` so iteration order is deterministic:
/// resolution reports errors in the order the user reads them, and the value
/// ends up on a salsa input whose equality must not depend on hash order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Declarations {
    /// The `[commands.<name>]` semantic aliases.
    pub commands: CommandDecls,
    /// The `[environments.<name>]` entries.
    pub environments: EnvironmentDecls,
}

impl Declarations {
    /// Whether the project declares nothing at all — the overwhelmingly common
    /// case, and the one the parse must not pay anything for.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty() && self.environments.is_empty()
    }

    /// Check every rule and project the declarations into a
    /// [`ResolvedDeclarations`]: an environment signature per `like`, and the
    /// delimiter spellings as opener and closer alias entries.
    ///
    /// Environment behavior and delimiter aliases resolve into a [`SignatureDb`]
    /// so they fold into the existing scope machinery. Command aliases stay in a
    /// separate deterministic map because they must not become parser or
    /// formatter signatures.
    ///
    /// **Every failure is an error, never a silent no-op.** A declaration that
    /// quietly does nothing is the worst outcome available here: the user sees
    /// unchanged output and has no way to tell a typo from an unimplemented
    /// feature. Errors surface in key order (the map is a `BTreeMap`), so the
    /// message is stable across runs.
    ///
    /// An entry that declares behavior alone is unrestricted — `like =
    /// "lstlisting"` is exactly how a project names a verbatim environment the
    /// definition scan cannot find. The extra restrictions below apply only to
    /// an entry that declares *delimiter spellings*, since those are the ones a
    /// command has to stand in for. An entry that declares **nothing** is the
    /// one shape rejected for saying too little rather than too much.
    ///
    /// One side alone is fine (issue #117): the literal `\begin{X}`/`\end{X}` is
    /// a spelling of each side too, so `begin = ['\bsplit']` with no `end`
    /// declares an opener the written-out `\end{split}` closes. This used to be
    /// two errors, on the reasoning that a half-declared pair could never pair.
    pub fn resolve(&self) -> Result<ResolvedDeclarations, DeclarationError> {
        let mut db = SignatureDb::default();
        let mut commands = BTreeMap::new();

        for (name, entry) in &self.commands {
            let error = |kind| DeclarationError {
                key: dotted_key(["commands", name]),
                kind,
            };
            if !is_control_word_name(name) {
                return Err(error(DeclarationErrorKind::InvalidCommandName {
                    name: SmolStr::new(name),
                }));
            }
            if is_builtin_command(name) {
                return Err(error(DeclarationErrorKind::BuiltinCommandName {
                    name: SmolStr::new(name),
                }));
            }
            let target = entry
                .like
                .as_ref()
                .ok_or_else(|| error(DeclarationErrorKind::EmptyCommandEntry))?;
            // The family tables alone decide what `like` copies, so they are the
            // whole test. Requiring a `signatures.json` entry too would reject the
            // targets a wrapper most needs: that file carries layout data and omits
            // most of the ref/cite families, `\cpageref` — the only list-valued page
            // reference — among them.
            if ref_command(target).is_none() && !is_cite_command(target) {
                return Err(DeclarationError {
                    key: dotted_key(["commands", name, "like"]),
                    kind: DeclarationErrorKind::UnknownCommandLikeTarget {
                        target: target.clone(),
                    },
                });
            }
            commands.insert(name.clone(), target.clone());
        }

        // Which entry already claimed a spelling, so a second claim is an error
        // rather than a last-writer-wins surprise.
        let mut claimed: BTreeMap<SmolStr, SmolStr> = BTreeMap::new();

        for (name, entry) in &self.environments {
            let error = |kind| DeclarationError {
                key: dotted_key(["environments", name]),
                kind,
            };

            // An entry that says nothing is the one shape resolution could
            // otherwise wave through, and it is exactly the shape a typo takes:
            // `deny_unknown_fields` catches a misspelled key, but a user who
            // wrote the header and nothing under it gets an entry that parses,
            // resolves, and does nothing.
            if entry.like.is_none() && !entry.has_delimiters() {
                return Err(error(DeclarationErrorKind::EmptyEntry));
            }

            // `like` first: it decides the behavior every later rule reads.
            let declared = match &entry.like {
                Some(target) => {
                    let sig = builtin()
                        .environment(target)
                        .ok_or_else(|| DeclarationError {
                            key: dotted_key(["environments", name, "like"]),
                            kind: DeclarationErrorKind::UnknownLikeTarget {
                                target: target.clone(),
                            },
                        })?;
                    db.insert_declared_environment(name.clone(), sig.clone());
                    Some(sig)
                }
                None => None,
            };

            if !entry.has_delimiters() {
                continue;
            }

            // A delimiter command has to stand in for *something*: an entry with
            // no `like` falls back to the built-in of the same name, and an
            // environment that is neither is one nothing downstream could
            // resolve.
            let sig: &EnvironmentSig = declared
                .or_else(|| builtin().environment(name))
                .ok_or_else(|| error(DeclarationErrorKind::UndeclaredTarget))?;
            if sig.verbatim_body {
                return Err(error(DeclarationErrorKind::VerbatimTarget));
            }
            if !sig.args.is_empty() {
                return Err(error(DeclarationErrorKind::TargetTakesArguments));
            }

            for (side, spellings) in [("begin", &entry.begin), ("end", &entry.end)] {
                let error = |kind| DeclarationError {
                    key: dotted_key(["environments", name, side]),
                    kind,
                };
                for spelling in spellings {
                    // Named apart from the general not-a-control-word rule
                    // because it is a *different mistake with a different fix*,
                    // and the one the issue-#117 reporter actually made: reaching
                    // for `end = ['\end{split}']` to say "closed by the written
                    // -out delimiter". That is the default now, so the fix is to
                    // delete the key — advice the generic message cannot give.
                    if let Some(env) = literal_delimiter_target(spelling.as_str()) {
                        return Err(error(DeclarationErrorKind::SpellingIsALiteralDelimiter {
                            name: spelling.clone(),
                            environment: SmolStr::new(env),
                        }));
                    }
                    if !is_control_word_name(spelling.as_str()) {
                        return Err(error(DeclarationErrorKind::NotAControlWord {
                            name: spelling.clone(),
                        }));
                    }
                    // A spelling the curated database already knows as a command
                    // is a mistake we can name: `begin = ['\emph']` would turn
                    // every `\emph` in the project into an environment opener
                    // wherever the shape gate let it pair. Curated tier only,
                    // for the same reason `like` is: the CWL tier carries every
                    // package's names, so rejecting against it would refuse a
                    // spelling on the say-so of a package the project never
                    // loads. That leaves the check partial by construction — it
                    // catches the arity-bearing commands, where a wrong pairing
                    // also mis-attaches arguments — and it is a backstop, not
                    // the safety property. The shape gate is still what keeps a
                    // wrong declaration from corrupting a tree.
                    if builtin().command(spelling.as_str()).is_some() {
                        return Err(error(DeclarationErrorKind::SpellingIsABuiltinCommand {
                            name: spelling.clone(),
                        }));
                    }
                    let key = SmolStr::new(spelling.as_str());
                    if let Some(first) = claimed.get(&key) {
                        // Repeating a spelling *within* one entry is a different
                        // mistake from two entries fighting over it, and reading
                        // "already declared as a delimiter of `eqnarray`" under
                        // `environments.eqnarray.begin` helps nobody.
                        return Err(error(if first == name {
                            DeclarationErrorKind::RepeatedDelimiter {
                                name: spelling.clone(),
                            }
                        } else {
                            DeclarationErrorKind::DuplicateDelimiter {
                                name: spelling.clone(),
                                first: first.clone(),
                            }
                        }));
                    }
                    claimed.insert(key.clone(), name.clone());
                    if side == "begin" {
                        db.insert_env_begin_alias(key, name.clone());
                    } else {
                        db.insert_env_end_alias(key, name.clone());
                    }
                }
            }
        }
        Ok(ResolvedDeclarations { db, commands })
    }
}

/// A project's declarations, checked and projected into signature data by
/// [`Declarations::resolve`].
///
/// The environment signature tier plus semantic-only command aliases. This is
/// the only signature data the parser accepts: a value can only come from a
/// declaration block, so `parse_with_declarations` cannot be handed a document's
/// merged package/definition scope. The parser reads only `db`; semantic-model
/// construction reads `commands`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedDeclarations {
    db: SignatureDb,
    commands: BTreeMap<SmolStr, SmolStr>,
}

impl ResolvedDeclarations {
    /// The declared tier as signature data, for merging into a document's scope
    /// (where it is the top tier: a declaration is the user explicitly
    /// correcting an inference).
    pub fn as_db(&self) -> &SignatureDb {
        &self.db
    }

    /// The built-in semantic target of a declared command alias.
    pub fn command_like(&self, name: &str) -> Option<&str> {
        self.commands.get(name).map(SmolStr::as_str)
    }

    /// The declared command names, in deterministic order.
    pub fn command_names(&self) -> impl Iterator<Item = &str> {
        self.commands.keys().map(SmolStr::as_str)
    }

    /// Whether nothing was declared — the common case, and the one that must
    /// cost the parse nothing.
    pub fn is_empty(&self) -> bool {
        self.db == SignatureDb::default() && self.commands.is_empty()
    }

    /// The parse-facing half of this block: the environment signature tier, with
    /// the semantic command aliases dropped.
    ///
    /// The two halves are split so a reader depends only on the one it uses. A
    /// command alias provably cannot change a tree
    /// (`command_declarations_do_not_change_the_parse_tree`), so an incremental
    /// front end can hold this half behind its own firewall and leave every parse
    /// standing when only `[commands]` changed. The environment tier is what
    /// [`parse_with_declarations`](crate::parser::parse_with_declarations) and the
    /// signature scope read.
    pub fn parse_tier(&self) -> Self {
        Self {
            db: self.db.clone(),
            commands: BTreeMap::new(),
        }
    }

    /// The semantic-facing half: the command aliases alone.
    ///
    /// The counterpart of [`parse_tier`](Self::parse_tier), read by
    /// [`SemanticModel::build_with_declarations`](crate::semantic::SemanticModel::build_with_declarations),
    /// which never looks at the environment tier. Together the two halves
    /// partition the block: nothing is in both, and nothing in neither.
    pub fn semantic_tier(&self) -> Self {
        Self {
            db: SignatureDb::default(),
            commands: self.commands.clone(),
        }
    }
}

/// A rule [`Declarations::resolve`] rejected, with the dotted key of the entry
/// that broke it (`environments.myenv.like`) so the CLI can point at the line
/// the user wrote.
///
/// The key is a `String` rather than a borrowed path because the error outlives
/// the borrow of the config in every caller, and this crate is wasm-clean: it
/// knows nothing about the file the key came from, which is the CLI's to add.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclarationError {
    pub key: String,
    pub kind: DeclarationErrorKind,
}

/// Why a declaration was rejected. Each variant is a rule from
/// `AGENTS.md` decision #12 or its architecture section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclarationErrorKind {
    /// A `[commands.<name>]` entry without its required `like` target.
    EmptyCommandEntry,
    /// A command map key that could never lex as one control word.
    InvalidCommandName { name: SmolStr },
    /// A declaration attempted to reclassify a curated built-in command.
    BuiltinCommandName { name: SmolStr },
    /// `like` did not name a curated reference or citation command.
    UnknownCommandLikeTarget { target: SmolStr },
    /// An entry with no keys at all. Nothing to reject it on rule grounds, and
    /// nothing for it to do either — which is the outcome this module exists to
    /// avoid.
    EmptyEntry,
    /// `like` named something the curated built-in database does not have.
    /// Never resolved against the CWL tier or scanned definitions: behavior
    /// comes from curated data only.
    UnknownLikeTarget { target: SmolStr },
    /// Delimiter spellings for a verbatim environment. Not conservatism but TeX
    /// truth, which is why it is rejected rather than merely discouraged.
    VerbatimTarget,
    /// Delimiter spellings for an environment that takes arguments. A bare
    /// control word carries none, and attaching them from the target's
    /// signature would be arity-directed grouping from declaration data.
    TargetTakesArguments,
    /// Delimiter spellings for an environment whose behavior is unknown — no
    /// `like`, and no built-in of that name.
    UndeclaredTarget,
    /// A spelling two entries both claim. Silently letting the last one win
    /// would make the pairing depend on map order.
    DuplicateDelimiter { name: CommandName, first: SmolStr },
    /// A spelling one entry lists twice — across its two sides, or twice on
    /// one. The [`DuplicateDelimiter`](Self::DuplicateDelimiter) mistake seen
    /// from inside a single entry, where naming the "other" entry is no help.
    RepeatedDelimiter { name: CommandName },
    /// A spelling that *is* the written-out delimiter (`\end{split}`) rather
    /// than a command standing in for one. A special case of
    /// [`NotAControlWord`](Self::NotAControlWord) with its own fix: the literal
    /// delimiter is already a spelling of both sides, so the key is redundant.
    SpellingIsALiteralDelimiter {
        name: CommandName,
        environment: SmolStr,
    },
    /// A spelling the lexer could never produce as one control word, so it
    /// could never match anything.
    NotAControlWord { name: CommandName },
    /// A spelling the curated database already knows as a command. Not a
    /// no-op — it would take effect, on a command the project did not mean to
    /// redefine.
    SpellingIsABuiltinCommand { name: CommandName },
}

/// Whether the curated data already knows `name` as a command.
///
/// All three sources have to be asked, because `signatures.json` is not a
/// superset of the ref/cite family tables: it carries layout data, and most of
/// the families (`\cpageref`, `\supercite`, `\Textcite`, …) have no entry there.
/// Asking it alone would let a declaration reclassify one of them: `like = "ref"`
/// on `\cpageref` would demote a list-valued page reference to a single-key
/// `\ref`, which is exactly what this gate exists to prevent.
fn is_builtin_command(name: &str) -> bool {
    builtin().command(name).is_some() || ref_command(name).is_some() || is_cite_command(name)
}

/// The environment named by `spelling` when it is the written-out delimiter
/// (`end{split}`, `begin{split}` — the leading backslash is already stripped by
/// [`CommandName`]), or `None` for an ordinary command name.
///
/// Deliberately shape-only, with no check that the name is one badness knows: a
/// user who writes `end = ['\end{myenv}']` made this mistake whether or not
/// `myenv` exists, and pointing at the wrong rule would send them looking for a
/// missing `like`.
fn literal_delimiter_target(spelling: &str) -> Option<&str> {
    let rest = spelling
        .strip_prefix("begin")
        .or_else(|| spelling.strip_prefix("end"))?;
    let name = rest.strip_prefix('{')?.strip_suffix('}')?.trim();
    (!name.is_empty()).then_some(name)
}

/// Join `segments` into a TOML dotted key, quoting any segment that is not a
/// bare key so the result can be pasted back into `badness.toml`.
///
/// An environment may be named anything, and `environments.my.env` would point
/// at a key the user never wrote.
fn dotted_key<'a>(segments: impl IntoIterator<Item = &'a str>) -> String {
    let mut key = String::new();
    for segment in segments {
        if !key.is_empty() {
            key.push('.');
        }
        let bare = !segment.is_empty()
            && segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if bare {
            key.push_str(segment);
        } else {
            key.push('"');
            key.push_str(&segment.replace('\\', "\\\\").replace('"', "\\\""));
            key.push('"');
        }
    }
    key
}

impl fmt::Display for DeclarationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}`: {}", self.key, self.kind)
    }
}

impl fmt::Display for DeclarationErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyCommandEntry => write!(
                f,
                "declares nothing; add `like` naming a reference or citation command"
            ),
            Self::InvalidCommandName { name } => write!(
                f,
                "`{name}` is not a control-word name; command names may contain letters, `@`, `_`, or `:`"
            ),
            Self::BuiltinCommandName { name } => write!(
                f,
                "`\\{name}` is already a curated LaTeX command; command declarations may only name project commands"
            ),
            Self::UnknownCommandLikeTarget { target } => write!(
                f,
                "unknown reference or citation command `{target}`; `like` must name a curated ref/cite command"
            ),
            Self::EmptyEntry => write!(
                f,
                "declares nothing; add `like` to say what the environment behaves like, or \
                 `begin`/`end` to give it delimiter spellings"
            ),
            Self::UnknownLikeTarget { target } => write!(
                f,
                "unknown environment `{target}`; `like` must name an environment badness \
                 knows about"
            ),
            Self::VerbatimTarget => write!(
                f,
                "a command cannot stand in for a verbatim environment's delimiters, because \
                 TeX never expands the closer — the verbatim scanner has already swallowed \
                 it. Declare the environment name on its own, without `begin`/`end`"
            ),
            Self::TargetTakesArguments => write!(
                f,
                "the environment takes arguments, which a delimiter command cannot carry; \
                 declare the environment name on its own, without `begin`/`end`"
            ),
            Self::UndeclaredTarget => write!(
                f,
                "declares delimiters for an environment badness does not know; add `like` \
                 to say what it behaves like"
            ),
            Self::DuplicateDelimiter { name, first } => write!(
                f,
                "`{name}` is already declared as a delimiter of `{first}`"
            ),
            Self::RepeatedDelimiter { name } => {
                write!(
                    f,
                    "`{name}` is listed twice as a delimiter of this environment"
                )
            }
            Self::SpellingIsALiteralDelimiter { name, environment } => write!(
                f,
                "`{name}` is the delimiter itself, not a command standing in for one — and \
                 badness already pairs a declared spelling with the written-out \
                 `\\begin{{{environment}}}`/`\\end{{{environment}}}`, so this key can be \
                 removed"
            ),
            Self::NotAControlWord { name } => write!(
                f,
                "`{name}` is not a control word; a delimiter must be a name of letters"
            ),
            Self::SpellingIsABuiltinCommand { name } => write!(
                f,
                "`{name}` is already a LaTeX command badness knows; a delimiter spelling must \
                 be a command of your own, or the declaration would change what `{name}` means \
                 everywhere in the project"
            ),
        }
    }
}

impl std::error::Error for DeclarationError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_json(json: &str) -> Declarations {
        serde_json::from_str(json).expect("deserializes")
    }

    #[test]
    fn empty_declarations_are_the_default() {
        assert!(Declarations::default().is_empty());
        assert!(from_json("{}").is_empty());
    }

    #[test]
    fn a_command_may_declare_a_reference_family() {
        let decls = from_json(r#"{"commands": {"eqrefs": {"like": "cref"}}}"#);
        assert_eq!(decls.commands["eqrefs"].like.as_deref(), Some("cref"));
        assert!(!decls.is_empty());
    }

    #[test]
    fn an_entry_may_declare_behavior_alone() {
        let decls = from_json(r#"{"environments": {"myenv": {"like": "align"}}}"#);
        let entry = &decls.environments["myenv"];
        assert_eq!(entry.like.as_deref(), Some("align"));
        assert!(!entry.has_delimiters());
        assert!(!decls.is_empty());
    }

    #[test]
    fn an_entry_may_declare_delimiters_alone() {
        let decls =
            from_json(r#"{"environments": {"eqnarray": {"begin": ["\\bea"], "end": ["\\eea"]}}}"#);
        let entry = &decls.environments["eqnarray"];
        assert_eq!(entry.like, None);
        assert!(entry.has_delimiters());
        assert_eq!(entry.begin, vec![CommandName::new("bea")]);
        assert_eq!(entry.end, vec![CommandName::new("eea")]);
    }

    /// The `\startmyenv … \endmyenv` shape from the issue: an environment with no
    /// built-in counterpart, reached only through commands. One entry covers it.
    #[test]
    fn an_entry_may_declare_both() {
        let decls = from_json(
            r#"{"environments": {"mytheorem": {
                 "like": "theorem",
                 "begin": ["\\startmyenv"],
                 "end": ["\\endmyenv"]
               }}}"#,
        );
        let entry = &decls.environments["mytheorem"];
        assert_eq!(entry.like.as_deref(), Some("theorem"));
        assert_eq!(entry.begin, vec![CommandName::new("startmyenv")]);
    }

    /// TOML users write `'\bea'`; a leading backslash is optional and both
    /// spellings must reach the same key, since a control word can never
    /// contain one.
    #[test]
    fn a_leading_backslash_is_optional_and_normalized_away() {
        assert_eq!(CommandName::new("\\bea"), CommandName::new("bea"));
        assert_eq!(CommandName::new("\\bea").as_str(), "bea");
        let decls = from_json(r#"{"environments": {"e": {"begin": ["bea", "\\bea"]}}}"#);
        assert_eq!(
            decls.environments["e"].begin,
            vec![CommandName::new("bea"), CommandName::new("bea")]
        );
    }

    /// Only *one* backslash is stripped, so a control symbol keeps its shape and
    /// resolution can reject it by name rather than silently seeing a word.
    #[test]
    fn only_one_backslash_is_stripped() {
        assert_eq!(CommandName::new("\\\\").as_str(), "\\");
    }

    /// A diagnostic should spell the name back the way the user wrote it.
    #[test]
    fn display_restores_the_backslash() {
        assert_eq!(CommandName::new("bea").to_string(), "\\bea");
    }

    #[test]
    fn a_misspelled_key_is_rejected_rather_than_ignored() {
        let err = serde_json::from_str::<Declarations>(
            r#"{"environments": {"myenv": {"liek": "align"}}}"#,
        )
        .expect_err("unknown field is rejected");
        assert!(err.to_string().contains("liek"), "{err}");

        let err = serde_json::from_str::<Declarations>(r#"{"enviroments": {}}"#)
            .expect_err("unknown section is rejected");
        assert!(err.to_string().contains("enviroments"), "{err}");

        let err =
            serde_json::from_str::<Declarations>(r#"{"commands": {"eqrefs": {"liek": "cref"}}}"#)
                .expect_err("unknown command field is rejected");
        assert!(err.to_string().contains("liek"), "{err}");
    }

    /// The wire spellings are public API (module docs), so a field rename must
    /// fail a test rather than silently break every user's config.
    #[test]
    fn wire_spellings_are_pinned() {
        let decls = from_json(
            r#"{"commands": {"eqrefs": {"like": "cref"}},
                "environments": {"myenv": {"like": "align", "begin": ["\\b"], "end": ["\\e"]}}}"#,
        );
        let json = serde_json::to_value(&decls).expect("serializes");
        assert_eq!(json["commands"]["eqrefs"]["like"], "cref");
        let entry = &json["environments"]["myenv"];
        assert_eq!(entry["like"], "align");
        assert_eq!(entry["begin"][0], "b");
        assert_eq!(entry["end"][0], "e");
    }

    /// Deterministic iteration: resolution reports errors in the order the user
    /// reads them, and the value lands on a salsa input.
    #[test]
    fn environments_iterate_in_name_order() {
        let decls = from_json(r#"{"environments": {"zed": {}, "alpha": {}, "mid": {}}}"#);
        let names: Vec<&str> = decls.environments.keys().map(SmolStr::as_str).collect();
        assert_eq!(names, ["alpha", "mid", "zed"]);
    }

    // --- resolution

    fn resolve(json: &str) -> SignatureDb {
        from_json(json).resolve().expect("resolves").as_db().clone()
    }

    fn resolve_err(json: &str) -> DeclarationError {
        from_json(json).resolve().expect_err("is rejected")
    }

    #[test]
    fn nothing_declared_resolves_to_nothing() {
        assert!(from_json("{}").resolve().expect("resolves").is_empty());
    }

    #[test]
    fn command_families_resolve_with_target_behavior() {
        let declared = from_json(
            r#"{"commands": {
                 "one": {"like": "eqref"},
                 "many": {"like": "cref"},
                 "sources": {"like": "parencite"},
                 "everything": {"like": "nocite"}
               }}"#,
        )
        .resolve()
        .expect("resolves");

        assert_eq!(declared.command_like("one"), Some("eqref"));
        assert_eq!(declared.command_like("many"), Some("cref"));
        assert_eq!(declared.command_like("sources"), Some("parencite"));
        assert_eq!(declared.command_like("everything"), Some("nocite"));
        assert!(
            declared.as_db().command("many").is_none(),
            "semantic aliases must not become formatter/parser signatures"
        );
        assert_eq!(
            declared.command_names().collect::<Vec<_>>(),
            vec!["everything", "many", "one", "sources"]
        );
    }

    /// Most of the ref/cite families have no `signatures.json` entry, so a `like`
    /// target must be checked against the family tables and nothing else. A
    /// wrapper around `\cpageref` — the only list-valued page reference, and the
    /// one shape `\pageref` cannot stand in for — has no other target to name.
    #[test]
    fn like_may_name_a_family_command_absent_from_the_signature_database() {
        for target in ["cpageref", "supercite", "Textcite", "fnotecite"] {
            assert!(
                builtin().command(target).is_none(),
                "{target} is in signatures.json; pick another for this test"
            );
            let json = format!(r#"{{"commands": {{"wrapper": {{"like": "{target}"}}}}}}"#);
            let declared = from_json(&json).resolve().expect("resolves");
            assert_eq!(declared.command_like("wrapper"), Some(target));
        }
    }

    /// The reclassification gate has to consult the same tables: `\cpageref`
    /// splits its key list and `\ref` does not, so accepting this entry would turn
    /// every `\cpageref{a,b}` in the project into one undefined key `a,b`.
    #[test]
    fn a_family_command_absent_from_the_signature_database_may_not_be_redeclared() {
        for name in ["cpageref", "supercite", "Textcite", "fnotecite"] {
            let json = format!(r#"{{"commands": {{"{name}": {{"like": "ref"}}}}}}"#);
            let err = resolve_err(&json);
            assert_eq!(err.key, format!("commands.{name}"), "{err}");
            assert!(matches!(
                err.kind,
                DeclarationErrorKind::BuiltinCommandName { .. }
            ));
        }
    }

    /// The halves partition the block, which is what lets an incremental front
    /// end depend on one without depending on the other.
    #[test]
    fn the_two_tiers_partition_the_block() {
        let declared = from_json(
            r#"{
                 "commands": {"myref": {"like": "cref"}},
                 "environments": {"mycode": {"like": "lstlisting"}}
               }"#,
        )
        .resolve()
        .expect("resolves");

        let parse = declared.parse_tier();
        assert_eq!(parse.as_db(), declared.as_db());
        assert_eq!(parse.command_names().count(), 0);

        let semantic = declared.semantic_tier();
        assert_eq!(semantic.as_db(), &SignatureDb::default());
        assert_eq!(semantic.command_like("myref"), Some("cref"));

        // Neither half alone is the block, and an edit confined to one leaves the
        // other's half untouched — the property the firewall rests on.
        assert!(!parse.is_empty() && !semantic.is_empty());
        let recommanded = from_json(
            r#"{
                 "commands": {"myref": {"like": "eqref"}},
                 "environments": {"mycode": {"like": "lstlisting"}}
               }"#,
        )
        .resolve()
        .expect("resolves");
        assert_ne!(recommanded, declared);
        assert_eq!(recommanded.parse_tier(), parse);
        assert_ne!(recommanded.semantic_tier(), semantic);
    }

    #[test]
    fn invalid_command_declarations_are_rejected() {
        for (json, key) in [
            (r#"{"commands": {"empty": {}}}"#, "commands.empty"),
            (
                r#"{"commands": {"wrapper": {"like": "emph"}}}"#,
                "commands.wrapper.like",
            ),
            (
                r#"{"commands": {"bad-name": {"like": "ref"}}}"#,
                "commands.bad-name",
            ),
            (
                r#"{"commands": {"section": {"like": "ref"}}}"#,
                "commands.section",
            ),
        ] {
            let err = resolve_err(json);
            assert_eq!(err.key, key, "{err}");
        }
    }

    #[test]
    fn command_declarations_do_not_change_the_parse_tree() {
        use crate::parser::{LatexFlavor, parse_with_declarations, parse_with_flavor};

        let src = "\\eqrefs{a,b}\n";
        let declared = from_json(r#"{"commands": {"eqrefs": {"like": "cref"}}}"#)
            .resolve()
            .expect("resolves");
        assert_eq!(
            parse_with_declarations(src, LatexFlavor::Document, &declared).green,
            parse_with_flavor(src, LatexFlavor::Document).green
        );
    }

    /// `like` copies the curated entry wholesale, so every behavior flag —
    /// including ones added later — comes along without being named in config.
    #[test]
    fn like_copies_the_builtin_entry() {
        let db = resolve(r#"{"environments": {"myenv": {"like": "align"}}}"#);
        let sig = db.environment("myenv").expect("declared");
        assert_eq!(sig, builtin().environment("align").expect("builtin"));
        assert!(sig.math && sig.align);
    }

    /// The parked `codeexample` knob: naming a verbatim environment is exactly
    /// what an entry with no delimiters is for.
    #[test]
    fn like_may_name_a_verbatim_environment() {
        let db = resolve(r#"{"environments": {"mycode": {"like": "lstlisting"}}}"#);
        assert!(db.environment("mycode").expect("declared").verbatim_body);
    }

    /// An argument-taking target is fine too, as long as no command has to stand
    /// in for the delimiters: `\begin{mytab}{ll}` carries its own arguments.
    #[test]
    fn like_may_name_an_argument_taking_environment() {
        let db = resolve(r#"{"environments": {"mytab": {"like": "tabular"}}}"#);
        assert!(!db.environment("mytab").expect("declared").args.is_empty());
    }

    #[test]
    fn a_mistyped_like_target_is_an_error_not_a_silent_no_op() {
        let err = resolve_err(r#"{"environments": {"myenv": {"like": "algin"}}}"#);
        assert_eq!(err.key, "environments.myenv.like");
        assert!(matches!(
            err.kind,
            DeclarationErrorKind::UnknownLikeTarget { .. }
        ));
        assert!(err.to_string().contains("algin"), "{err}");
    }

    /// `like` resolves against the curated tier alone. The CWL tier carries
    /// names and arity with every behavior flag left at its default, so copying
    /// from it would hand back a signature that says nothing.
    #[test]
    fn like_does_not_resolve_against_the_cwl_tier() {
        let cwl_only = crate::semantic::signature::cwl()
            .environment_names()
            .find(|name| builtin().environment(name).is_none())
            .expect("the CWL tier has an environment the curated one does not")
            .to_string();
        let err = resolve_err(&format!(
            r#"{{"environments": {{"myenv": {{"like": "{cwl_only}"}}}}}}"#
        ));
        assert!(matches!(
            err.kind,
            DeclarationErrorKind::UnknownLikeTarget { .. }
        ));
    }

    /// The issue's own case: spellings for an environment badness already knows,
    /// needing no `like` at all.
    #[test]
    fn delimiters_for_a_builtin_environment_need_no_like() {
        let db =
            resolve(r#"{"environments": {"eqnarray": {"begin": ["\\bea"], "end": ["\\eea"]}}}"#);
        assert_eq!(db.env_begin_alias("bea"), Some("eqnarray"));
        assert_eq!(db.env_end_alias("eea"), Some("eqnarray"));
        // Behavior still comes from the built-in entry, so nothing is cloned in
        // under the environment's own name.
        assert!(db.environment("eqnarray").is_none());
    }

    /// The `\startmyenv … \endmyenv` shape: behavior *and* spellings, one entry.
    #[test]
    fn delimiters_and_like_resolve_together() {
        let db = resolve(
            r#"{"environments": {"mytheorem": {
                 "like": "theorem",
                 "begin": ["\\startmyenv"],
                 "end": ["\\endmyenv"]
               }}}"#,
        );
        assert_eq!(db.env_begin_alias("startmyenv"), Some("mytheorem"));
        assert!(db.environment("mytheorem").is_some());
    }

    /// Several spellings may open the same environment; pairing is by kind, not
    /// by index, so the lists need not be the same length.
    #[test]
    fn an_environment_may_have_several_spellings_per_side() {
        let db = resolve(
            r#"{"environments": {"eqnarray": {
                 "begin": ["\\bea", "\\beqa"], "end": ["\\eea"]
               }}}"#,
        );
        assert_eq!(db.env_begin_alias("bea"), Some("eqnarray"));
        assert_eq!(db.env_begin_alias("beqa"), Some("eqnarray"));
    }

    /// Issue #117: one side alone resolves, because the literal delimiter is a
    /// spelling of the other side. Both directions, since the two used to be
    /// symmetric errors.
    #[test]
    fn one_side_alone_resolves() {
        let db = resolve(r#"{"environments": {"eqnarray": {"begin": ["\\bea"]}}}"#);
        assert_eq!(db.env_begin_alias("bea"), Some("eqnarray"));
        assert_eq!(db.env_end_alias("bea"), None);

        let db = resolve(r#"{"environments": {"eqnarray": {"end": ["\\eea"]}}}"#);
        assert_eq!(db.env_end_alias("eea"), Some("eqnarray"));
        assert_eq!(db.env_begin_alias("eea"), None);
    }

    /// The target rules are what bound a wrong declaration, so each is
    /// re-checked on the one-sided shape the old requirement hid.
    #[test]
    fn one_side_alone_still_obeys_every_target_rule() {
        for json in [
            r#"{"environments": {"verbatim": {"begin": ["\\bv"]}}}"#,
            r#"{"environments": {"verbatim": {"end": ["\\ev"]}}}"#,
        ] {
            assert_eq!(resolve_err(json).kind, DeclarationErrorKind::VerbatimTarget);
        }
        assert_eq!(
            resolve_err(r#"{"environments": {"tabular": {"begin": ["\\bt"]}}}"#).kind,
            DeclarationErrorKind::TargetTakesArguments
        );
        assert_eq!(
            resolve_err(r#"{"environments": {"myenv": {"end": ["\\e"]}}}"#).kind,
            DeclarationErrorKind::UndeclaredTarget
        );
    }

    /// TeX truth, not conservatism: the closer alias is never expanded, because
    /// the verbatim scanner has already swallowed it.
    #[test]
    fn delimiters_for_a_verbatim_environment_are_rejected() {
        let err =
            resolve_err(r#"{"environments": {"verbatim": {"begin": ["\\bv"], "end": ["\\ev"]}}}"#);
        assert_eq!(err.kind, DeclarationErrorKind::VerbatimTarget);

        // Reached through `like` as well as by name.
        let err = resolve_err(
            r#"{"environments": {"mycode": {
                 "like": "lstlisting", "begin": ["\\bc"], "end": ["\\ec"]
               }}}"#,
        );
        assert_eq!(err.kind, DeclarationErrorKind::VerbatimTarget);
    }

    #[test]
    fn delimiters_for_an_argument_taking_environment_are_rejected() {
        let err =
            resolve_err(r#"{"environments": {"tabular": {"begin": ["\\bt"], "end": ["\\et"]}}}"#);
        assert_eq!(err.kind, DeclarationErrorKind::TargetTakesArguments);
    }

    #[test]
    fn delimiters_for_an_unknown_environment_ask_for_like() {
        let err = resolve_err(r#"{"environments": {"myenv": {"begin": ["\\b"], "end": ["\\e"]}}}"#);
        assert_eq!(err.kind, DeclarationErrorKind::UndeclaredTarget);
        assert!(err.to_string().contains("like"), "{err}");
    }

    /// The one shape that says too little. A header with nothing under it
    /// parses, breaks no rule, and does nothing — the outcome every other rule
    /// here exists to prevent.
    #[test]
    fn an_entry_that_declares_nothing_is_an_error() {
        let err = resolve_err(r#"{"environments": {"myenv": {}}}"#);
        assert_eq!(err.key, "environments.myenv");
        assert_eq!(err.kind, DeclarationErrorKind::EmptyEntry);
        assert!(err.to_string().contains("like"), "{err}");
    }

    /// A spelling badness already knows as a command would *take effect* rather
    /// than do nothing, on a command the project never meant to touch.
    #[test]
    fn a_spelling_that_is_already_a_builtin_command_is_rejected() {
        let err =
            resolve_err(r#"{"environments": {"center": {"begin": ["\\emph"], "end": ["\\ec"]}}}"#);
        assert_eq!(err.key, "environments.center.begin");
        assert!(matches!(
            err.kind,
            DeclarationErrorKind::SpellingIsABuiltinCommand { .. }
        ));
        assert!(err.to_string().contains("emph"), "{err}");
    }

    /// The check reads the curated tier alone, so a name only the bulk CWL tier
    /// carries is still a project's to spell — the same scoping `like` has, and
    /// for the same reason: CWL knows every package, including ones the project
    /// never loads.
    #[test]
    fn a_cwl_only_command_name_is_still_available_as_a_spelling() {
        let cwl_only = crate::semantic::signature::cwl()
            .command_names()
            .find(|name| {
                builtin().command(name).is_none() && is_control_word_name(name) && name.len() > 2
            })
            .expect("the CWL tier has a command the curated one does not")
            .to_string();
        let db = resolve(&format!(
            r#"{{"environments": {{"center": {{"begin": ["{cwl_only}"], "end": ["\\ec"]}}}}}}"#
        ));
        assert_eq!(db.env_begin_alias(&cwl_only), Some("center"));
    }

    /// The error key is a dotted key the user can paste back, so a name that is
    /// not a bare TOML key is quoted the way they had to write it.
    #[test]
    fn the_error_key_quotes_a_name_that_is_not_a_bare_key() {
        let err = resolve_err(r#"{"environments": {"my.env": {}}}"#);
        assert_eq!(err.key, r#"environments."my.env""#);
        let err = resolve_err(r#"{"environments": {"my env": {"like": "algin"}}}"#);
        assert_eq!(err.key, r#"environments."my env".like"#);
    }

    /// Two entries claiming one spelling would otherwise resolve by map order.
    #[test]
    fn a_spelling_may_not_be_claimed_twice() {
        let err = resolve_err(
            r#"{"environments": {
                 "align": {"begin": ["\\bx"], "end": ["\\ex"]},
                 "equation": {"begin": ["\\bx"], "end": ["\\ey"]}
               }}"#,
        );
        assert_eq!(
            err.kind,
            DeclarationErrorKind::DuplicateDelimiter {
                name: CommandName::new("bx"),
                first: SmolStr::new("align"),
            }
        );
    }

    /// Including across the two sides, where the two maps would each claim it —
    /// reported as the *repeat* it is, since naming the owning entry would just
    /// name the entry the error is already keyed to.
    #[test]
    fn a_spelling_may_not_be_both_opener_and_closer() {
        let err = resolve_err(r#"{"environments": {"align": {"begin": ["\\x"], "end": ["\\x"]}}}"#);
        assert_eq!(err.key, "environments.align.end");
        assert_eq!(
            err.kind,
            DeclarationErrorKind::RepeatedDelimiter {
                name: CommandName::new("x"),
            }
        );
    }

    /// The exact key the issue-#117 reporter wrote. It is not a control word,
    /// so the general rule already caught it — but only to say "a delimiter must
    /// be a name of letters", which does not tell them that what they were
    /// reaching for is now the default and the key should simply go.
    #[test]
    fn the_written_out_delimiter_is_rejected_with_its_own_advice() {
        let err = resolve_err(
            r#"{"environments": {"split": {"begin": ["\\bsplit"], "end": ["\\end{split}"]}}}"#,
        );
        assert_eq!(err.key, "environments.split.end");
        assert!(
            matches!(
                err.kind,
                DeclarationErrorKind::SpellingIsALiteralDelimiter { .. }
            ),
            "{err:?}"
        );
        let rendered = err.to_string();
        assert!(rendered.contains("\\end{split}"), "{rendered}");
        assert!(rendered.contains("removed"), "{rendered}");

        // The opening side, and a name badness does not curate: the shape is the
        // mistake, so neither changes which rule fires.
        for json in [
            r#"{"environments": {"split": {"begin": ["\\begin{split}"]}}}"#,
            r#"{"environments": {"split": {"end": ["\\end{myenv}"]}}}"#,
        ] {
            assert!(
                matches!(
                    resolve_err(json).kind,
                    DeclarationErrorKind::SpellingIsALiteralDelimiter { .. }
                ),
                "{json}"
            );
        }

        // A command that merely *starts* with those letters is an ordinary
        // spelling, not the delimiter.
        let db = resolve(r#"{"environments": {"center": {"begin": ["\\beginning"]}}}"#);
        assert_eq!(db.env_begin_alias("beginning"), Some("center"));
    }

    /// A spelling the lexer would split into two tokens can never match, so
    /// accepting it would be a silent no-op.
    #[test]
    fn a_spelling_that_could_never_lex_as_one_control_word_is_rejected() {
        for bad in ["b ea", "bea2", "", "b-ea"] {
            let json = format!(
                r#"{{"environments": {{"align": {{"begin": ["{bad}"], "end": ["\\ex"]}}}}}}"#
            );
            let err = resolve_err(&json);
            assert!(
                matches!(err.kind, DeclarationErrorKind::NotAControlWord { .. }),
                "`{bad}` should be rejected, got {err:?}"
            );
        }
    }

    /// `@` and expl3's `_`/`:` are letters in the regimes a `.sty` is read
    /// under, and a declaration does not say which file it will apply to.
    #[test]
    fn a_spelling_may_use_letters_of_any_catcode_regime() {
        let db =
            resolve(r#"{"environments": {"align": {"begin": ["\\my@b"], "end": ["\\my_e:n"]}}}"#);
        assert_eq!(db.env_begin_alias("my@b"), Some("align"));
        assert_eq!(db.env_end_alias("my_e:n"), Some("align"));
    }

    /// The resolved tier is a `SignatureDb`, so it folds into a document's scope
    /// with the merge the scanned tier already uses — which is what step 5 of
    /// the plan needs and why the return type is not bespoke.
    #[test]
    fn the_resolved_tier_merges_like_any_other() {
        let declared = resolve(
            r#"{"environments": {"myenv": {"like": "align"}, "eqnarray": {
                 "begin": ["\\bea"], "end": ["\\eea"]
               }}}"#,
        );
        let mut scope = SignatureDb::default();
        scope.merge_from(&declared);
        assert!(scope.environment("myenv").is_some());
        assert_eq!(scope.env_begin_alias("bea"), Some("eqnarray"));
    }

    // --- resolution reaching the semantic layer

    /// Parse `src` under `json`'s declarations and resolve the signature
    /// governing its first `ENVIRONMENT` node, through the scope a document
    /// would build: scanned definitions first, declarations overlaid on top.
    fn environment_sig_at(src: &str, json: &str) -> Option<EnvironmentSig> {
        scope_and_sig_at(src, json).1
    }

    /// [`environment_sig_at`], also returning the name-keyed answer for the
    /// alias's target, so a test can show the two lookups diverge.
    fn scope_and_sig_at(src: &str, json: &str) -> (Option<EnvironmentSig>, Option<EnvironmentSig>) {
        use crate::parser::{LatexFlavor, parse_with_declarations};
        use crate::semantic::define::scan_definitions;
        use crate::semantic::signature::Signatures;
        use crate::syntax::{SyntaxKind, SyntaxNode};

        let decls = from_json(json).resolve().expect("resolves");
        let parsed = parse_with_declarations(src, LatexFlavor::Document, &decls);
        let root = SyntaxNode::new_root(parsed.green);
        let mut scope = scan_definitions(&root);
        scope.merge_declarations(&decls);
        let node = root
            .descendants()
            .find(|n| n.kind() == SyntaxKind::ENVIRONMENT)
            .expect("an environment");
        let sigs = Signatures::new(&scope);
        (
            sigs.environment("eqnarray").cloned(),
            sigs.environment_at(&node).cloned(),
        )
    }

    /// The sharp edge: an alias whose target is *itself* declared. Resolving the
    /// target against `builtin()` alone would find nothing, so `\startmyenv`
    /// would pair and then inherit no behavior at all.
    #[test]
    fn a_declared_alias_resolves_to_a_declared_target() {
        let sig = environment_sig_at(
            "\\startmyenv x \\endmyenv\n",
            r#"{"environments": {"myenv": {
                 "like": "align", "begin": ["\\startmyenv"], "end": ["\\endmyenv"]
               }}}"#,
        )
        .expect("the alias resolves");
        assert_eq!(&sig, builtin().environment("align").expect("curated"));
    }

    /// And the rule that edge must not break: a *scanned* definition still lends
    /// an alias nothing. Here `eqnarray` is redefined in the file, but the alias
    /// resolves to the curated entry, because only curated data may reach it.
    #[test]
    fn a_scanned_definition_still_lends_an_alias_nothing() {
        let (scanned, sig) = scope_and_sig_at(
            "\\newenvironment{eqnarray}{}{}\n\\bea x \\eea\n",
            r#"{"environments": {"eqnarray": {"begin": ["\\bea"], "end": ["\\eea"]}}}"#,
        );
        let sig = sig.expect("the alias resolves");
        assert_eq!(&sig, builtin().environment("eqnarray").expect("curated"));
        // The scan really did land a shadowing entry, and the *name*-keyed
        // lookup sees it — so the two answers genuinely diverge here, and the
        // alias took the curated one.
        let scanned = scanned.expect("the scan records the redefinition");
        assert!(!scanned.math, "the scanned redefinition is not math");
        assert!(sig.math, "the alias resolves to the curated entry");
    }
}
