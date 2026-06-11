//! The built-in **signature database**: command/environment argument shapes plus
//! the semantic metadata a formatter/linter needs (sectioning level,
//! verbatim-ness, math-ness). This is the structural analog of ravel's `rindex/`
//! — the place where *meaning* is assigned to names, kept strictly out of the
//! parser (AGENTS.md decision #2).
//!
//! The data is fully static, so — like ravel's `BASE_R`/`BUNDLED` statics, and
//! unlike its harvested `LibraryIndex` salsa input — it lives in a process-wide
//! [`LazyLock`], consulted directly. A salsa input only becomes necessary once
//! per-file `\newcommand`/`xparse` signatures must merge in (a separate, later
//! item); the greedy parser's argument attachment is unaffected until then.
//!
//! ## Source of truth: one granular JSON file
//!
//! The built-in data is a single curated JSON file (`data/signatures.json`,
//! [`include_str!`]-ed, [`serde`]-deserialized) holding *all* the metadata in one
//! typed place — argument shapes *and* sectioning level / verbatim-ness /
//! math-ness together, keyed by name. This is the high-precision tier we maintain
//! by hand, the analog of ravel's `PackageIndex` schema.
//!
//! Lower-precision external sources layer *underneath* this, ingested into the
//! same schema rather than replacing it (mirroring ravel's `installed > base >
//! bundled` precedence): the TeXstudio/Kile **CWL corpus** (arg shapes only, broad
//! coverage) and, later, per-file `\newcommand`/`xparse` scanning. Both are
//! deferred; when the CWL corpus is wanted, a converter merges it into this JSON
//! shape — CWL is an import format, never the source of truth.

use std::collections::HashMap;
use std::sync::LazyLock;

use serde::Deserialize;
use smol_str::SmolStr;

/// Which bracket delimits an argument. TeX has no other real argument grouping at
/// the surface level the formatter cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgKind {
    /// A mandatory `{…}` argument.
    Brace,
    /// An optional `[…]` argument.
    Bracket,
}

/// One argument slot in a command/environment signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArgSpec {
    /// `true` for a mandatory `{…}` argument, `false` for an optional `[…]` one.
    pub required: bool,
    pub kind: ArgKind,
}

/// The signature of a control sequence.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommandSig {
    /// The ordered argument slots.
    pub args: Vec<ArgSpec>,
    /// `Some(level)` for a sectioning command, where `0` is the outermost
    /// (`\part`) and larger numbers nest deeper. Relative depth only.
    pub sectioning: Option<u8>,
    /// `true` for commands whose argument is raw text the formatter must not
    /// reshape (`\verb`, `\lstinline`). Their delimiter syntax is handled in the
    /// lexer; this flag is the semantic record.
    pub verbatim: bool,
}

/// The signature of an environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentSig {
    /// The ordered argument slots that follow `\begin{name}` (e.g. `tabular`'s
    /// column spec), *excluding* the name group itself.
    pub args: Vec<ArgSpec>,
    /// `true` for environments whose body is raw text (`verbatim`, `lstlisting`,
    /// `minted`, …) and must never be reflowed.
    pub verbatim_body: bool,
    /// `true` for math environments (`equation`, `align`, …).
    pub math: bool,
    /// `true` when the body is ordinary prose the formatter may reflow. Derived as
    /// `!(verbatim_body || math)`. (Reflow itself is a later item; this is the
    /// recorded intent.)
    pub reflow: bool,
}

/// The built-in command and environment signatures, keyed by name (without the
/// leading `\` for commands, the bare name for environments). Case-sensitive, as
/// LaTeX names are (`Verbatim` ≠ `verbatim`).
#[derive(Debug, Default)]
pub struct SignatureDb {
    commands: HashMap<SmolStr, CommandSig>,
    environments: HashMap<SmolStr, EnvironmentSig>,
}

impl SignatureDb {
    /// The signature of command `name` (without the leading `\`), if known.
    pub fn command(&self, name: &str) -> Option<&CommandSig> {
        self.commands.get(name)
    }

    /// The signature of environment `name`, if known.
    pub fn environment(&self, name: &str) -> Option<&EnvironmentSig> {
        self.environments.get(name)
    }
}

/// The bundled, curated signature data (see module docs).
const SIGNATURES_JSON: &str = include_str!("../../data/signatures.json");

static DB: LazyLock<SignatureDb> =
    LazyLock::new(|| parse(SIGNATURES_JSON).expect("bundled data/signatures.json must be valid"));

/// The process-wide built-in signature database.
pub fn builtin() -> &'static SignatureDb {
    &DB
}

// --- On-disk schema (serde) ---------------------------------------------------
//
// A thin deserialization mirror of the in-memory types, kept separate so the
// public API stays free of serde concerns and the JSON can use a compact,
// hand-authorable spelling (`"req"`/`"opt"` for arguments; flags defaulting to
// false; `reflow` derived rather than stored).

/// One argument as written in the JSON: `"req"` (mandatory `{…}`) or `"opt"`
/// (optional `[…]`).
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum RawArg {
    Req,
    Opt,
}

impl From<RawArg> for ArgSpec {
    fn from(raw: RawArg) -> Self {
        match raw {
            RawArg::Req => ArgSpec {
                required: true,
                kind: ArgKind::Brace,
            },
            RawArg::Opt => ArgSpec {
                required: false,
                kind: ArgKind::Bracket,
            },
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawCommand {
    #[serde(default)]
    args: Vec<RawArg>,
    #[serde(default)]
    sectioning: Option<u8>,
    #[serde(default)]
    verbatim: bool,
}

impl From<RawCommand> for CommandSig {
    fn from(raw: RawCommand) -> Self {
        CommandSig {
            args: raw.args.into_iter().map(ArgSpec::from).collect(),
            sectioning: raw.sectioning,
            verbatim: raw.verbatim,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawEnvironment {
    #[serde(default)]
    args: Vec<RawArg>,
    #[serde(default, rename = "verbatimBody")]
    verbatim_body: bool,
    #[serde(default)]
    math: bool,
}

impl From<RawEnvironment> for EnvironmentSig {
    fn from(raw: RawEnvironment) -> Self {
        EnvironmentSig {
            args: raw.args.into_iter().map(ArgSpec::from).collect(),
            verbatim_body: raw.verbatim_body,
            math: raw.math,
            // A body is reflowable prose unless it is verbatim or math.
            reflow: !(raw.verbatim_body || raw.math),
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawDb {
    #[serde(default)]
    commands: HashMap<String, RawCommand>,
    #[serde(default)]
    environments: HashMap<String, RawEnvironment>,
}

/// Deserialize the bundled JSON into a [`SignatureDb`].
fn parse(json: &str) -> serde_json::Result<SignatureDb> {
    let raw: RawDb = serde_json::from_str(json)?;
    Ok(SignatureDb {
        commands: raw
            .commands
            .into_iter()
            .map(|(name, sig)| (SmolStr::new(name), sig.into()))
            .collect(),
        environments: raw
            .environments
            .into_iter()
            .map(|(name, sig)| (SmolStr::new(name), sig.into()))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_json_loads() {
        // Exercises the bundled file through the real loader; a malformed or
        // unknown-field entry would panic here.
        let db = builtin();
        assert!(db.command("section").is_some());
        assert!(db.environment("tabular").is_some());
    }

    #[test]
    fn loads_and_resolves_known_commands() {
        let db = builtin();
        assert_eq!(db.command("frac").map(|c| c.args.len()), Some(2));
        assert!(db.command("frac").unwrap().args.iter().all(|a| a.required));
    }

    #[test]
    fn optional_then_mandatory_order_preserved() {
        let args = &builtin().command("includegraphics").unwrap().args;
        assert_eq!(args.len(), 2);
        assert_eq!(args[0].kind, ArgKind::Bracket);
        assert!(!args[0].required);
        assert_eq!(args[1].kind, ArgKind::Brace);
        assert!(args[1].required);
    }

    #[test]
    fn mixed_argument_order_round_trips() {
        // `\newcommand{cmd}[nargs]{def}` — mandatory, optional, mandatory.
        let args = &builtin().command("newcommand").unwrap().args;
        let kinds: Vec<_> = args.iter().map(|a| a.kind).collect();
        assert_eq!(
            kinds,
            vec![ArgKind::Brace, ArgKind::Bracket, ArgKind::Brace]
        );
    }

    #[test]
    fn sectioning_levels_assigned() {
        let db = builtin();
        assert_eq!(db.command("part").unwrap().sectioning, Some(0));
        assert_eq!(db.command("section").unwrap().sectioning, Some(2));
        assert_eq!(db.command("subsubsection").unwrap().sectioning, Some(4));
        // A sectioning command still carries its argument shape.
        assert_eq!(db.command("section").unwrap().args.len(), 2);
        assert!(db.command("textbf").unwrap().sectioning.is_none());
    }

    #[test]
    fn verbatim_commands_flagged() {
        assert!(builtin().command("verb").unwrap().verbatim);
        assert!(builtin().command("lstinline").unwrap().verbatim);
        assert!(!builtin().command("textbf").unwrap().verbatim);
    }

    #[test]
    fn environment_argument_shapes() {
        let db = builtin();
        let tabular = db.environment("tabular").unwrap();
        assert_eq!(tabular.args.len(), 2);
        assert_eq!(tabular.args[0].kind, ArgKind::Bracket); // [pos]
        assert_eq!(tabular.args[1].kind, ArgKind::Brace); // {cols}
        assert!(db.environment("verbatim").unwrap().args.is_empty());
    }

    #[test]
    fn environment_flags_and_derived_reflow() {
        let db = builtin();
        let lstlisting = db.environment("lstlisting").unwrap();
        assert!(lstlisting.verbatim_body);
        assert!(!lstlisting.reflow);
        let equation = db.environment("equation").unwrap();
        assert!(equation.math);
        assert!(!equation.reflow);
        // A plain content environment reflows and is neither verbatim nor math.
        let tabular = db.environment("tabular").unwrap();
        assert!(!tabular.verbatim_body);
        assert!(!tabular.math);
        assert!(tabular.reflow);
    }

    #[test]
    fn unknown_names_resolve_to_none() {
        let db = builtin();
        assert!(db.command("definitelynotacommand").is_none());
        assert!(db.environment("definitelynotanenv").is_none());
    }

    #[test]
    fn rejects_unknown_fields() {
        // A typo'd field must fail loudly rather than be silently ignored.
        let err = parse(r#"{ "commands": { "x": { "sektioning": 2 } } }"#);
        assert!(err.is_err());
    }

    #[test]
    fn empty_document_is_valid() {
        let db = parse("{}").expect("empty object is valid");
        assert!(db.command("anything").is_none());
    }
}
