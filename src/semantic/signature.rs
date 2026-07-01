//! The built-in **signature database**: command/environment argument shapes plus
//! the semantic metadata a formatter/linter needs (sectioning level,
//! verbatim-ness, math-ness). This is the place where *meaning* is assigned to
//! names, kept strictly out of the parser (AGENTS.md decision #2).
//!
//! The data is fully static, so it lives in a process-wide [`LazyLock`],
//! consulted directly. Per-file `\newcommand`/`\newenvironment`/xparse
//! signatures are scanned by [`super::define`] into a separate, per-document
//! [`SignatureDb`] and overlaid via [`Signatures`] (scanned-first, built-in
//! fallback); the greedy parser's argument attachment is unaffected either way. A
//! salsa input only becomes necessary once that overlay must be cached across
//! queries (a later item, when an LSP consumer appears).
//!
//! ## Source of truth: one granular JSON file
//!
//! The built-in data is a single curated JSON file (`data/signatures.json`,
//! [`include_str!`]-ed, [`serde`]-deserialized) holding *all* the metadata in one
//! typed place — argument shapes *and* sectioning level / verbatim-ness /
//! math-ness together, keyed by name. This is the high-precision tier we maintain
//! by hand.
//!
//! Lower-precision external sources layer *underneath* this, ingested into the
//! same schema rather than replacing it. The TeXstudio/Kile **CWL corpus** is one
//! such tier: a
//! converter (`scripts/gen_cwl_signatures.py`) harvests command/environment names
//! and argument shapes from a curated package subset into `data/cwl_signatures.json`,
//! exposed by [`cwl`] and consulted *under* [`builtin`]. CWL is an import format,
//! never the source of truth: only names and arity cross over (every behavior flag
//! stays default), so it widens completion and arity coverage without its
//! low-confidence data reaching a lexer/formatter/outline behavior decision. The
//! file is compiled into a `phf` perfect-hash map at build time (`build.rs`) and
//! `include!`-ed as read-only statics — zero runtime parse or decompress.

use std::borrow::Cow;
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

/// How the formatter treats an argument's *content* — its whitespace and break
/// policy. Exactly one kind per slot (replaces the former mutually-exclusive
/// `prose`/`collapse` bools). Only meaningful for the formatter; the parser
/// ignores it (AGENTS.md decision #2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContentKind {
    /// Left exactly as authored: names, identifiers, code, or option lists
    /// (`\label`, the `\newcommand` body). The default, so an unmarked argument
    /// never reflows — for most arguments interior whitespace can matter (a
    /// `minipage`/`\parbox` body, a label key).
    #[default]
    Opaque,
    /// Running prose the formatter may reflow to the line width (e.g. a
    /// `\footnote`/`\caption` body, a sectioning title).
    Prose,
    /// A comma-separated token list whose interior whitespace is *insignificant*,
    /// so the formatter may collapse a multi-line authored form to a single line
    /// (a `\citep`/`\cite` key list). Unlike [`Prose`](ContentKind::Prose), the
    /// content is *not* reflowed to the width: the keys stay together as one atom;
    /// only incidental source line breaks inside the braces are normalized away,
    /// so `\citep{\n a,\n b\n}` formats identically to `\citep{a, b}` (determinism).
    TokenList,
}

/// One argument slot in a command/environment signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArgSpec {
    /// `true` for a mandatory `{…}` argument, `false` for an optional `[…]` one.
    pub required: bool,
    pub kind: ArgKind,
    /// How the formatter treats this argument's content. See [`ContentKind`].
    pub content: ContentKind,
}

/// The signature of a control sequence.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommandSig {
    /// The ordered argument slots. A [`Cow`] so the build-time CWL tier can hold a
    /// `'static` slice baked into the binary (see [`command`]) while the runtime
    /// builtin/scanned paths own a `Vec`.
    pub args: Cow<'static, [ArgSpec]>,
    /// `Some(level)` for a sectioning command, where `0` is the outermost
    /// (`\part`) and larger numbers nest deeper. Relative depth only.
    pub sectioning: Option<u8>,
    /// `true` for commands whose final argument is raw text the formatter must
    /// not reshape (`\verb`, `\lstinline`, `\url`, `\code`). The lexer captures
    /// that argument as one `VERB` token — a balanced `{…}` group or a
    /// `\verb`-style delimiter run, chosen by its first character. Any leading,
    /// non-verbatim arguments (e.g. `\mintinline`'s language) are declared in
    /// `args`; the verbatim argument itself is implicit and not listed there.
    pub verbatim: bool,
    /// `true` for horizontal-rule commands (`\hline`, `\midrule`, `\toprule`, …).
    /// In an alignment environment a physical line made up solely of rule
    /// commands is a *passthrough* line the formatter keeps between grid rows
    /// rather than treating as a cell (see the grid lowering in `formatter`).
    pub rule: bool,
    /// `true` for *inline* commands that sit in running text (`\citep`, `\ref`,
    /// `\emph`, `\textbf`, …) rather than occupying their own line. Paragraph reflow
    /// treats such a command as an atom that flows into the fill even when the author
    /// isolated it on its own source line, instead of preserving it as a
    /// command-only line (the way a `\usepackage`/`\section` line is kept). For a
    /// command that *also* has a `prose` argument this additionally flattens the
    /// command into the paragraph so its body wraps as running text with the `{`/`}`
    /// glued to the adjacent words. Block-level commands that head their own line
    /// (`\section`, `\caption`) leave this `false`. Only meaningful to the formatter;
    /// the parser ignores it.
    pub inline: bool,
}

/// How an environment appears in the document-symbol outline, if at all. A small
/// curated category over the `block` environments: only floats and theorem-likes
/// earn an outline entry, so layout environments (`center`, `quote`, `frame`, …)
/// stay out of the symbol tree. Drives `SymbolKind` selection in the LSP layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutlineKind {
    /// A float (`figure`, `table`, and their starred forms).
    Float,
    /// A theorem-like environment (`theorem`, `lemma`, `proof`, …).
    Theorem,
}

/// The signature of an environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentSig {
    /// The ordered argument slots that follow `\begin{name}` (e.g. `tabular`'s
    /// column spec), *excluding* the name group itself. A [`Cow`] for the same
    /// reason as [`CommandSig::args`]: a `'static` slice for the CWL tier, an owned
    /// `Vec` for the runtime paths.
    pub args: Cow<'static, [ArgSpec]>,
    /// `true` for environments whose body is raw text (`verbatim`, `lstlisting`,
    /// `minted`, …) and must never be reflowed.
    pub verbatim_body: bool,
    /// `true` for math environments (`equation`, `align`, …).
    pub math: bool,
    /// `true` for environments whose body is *real parsed code*, not prose —
    /// the doc/ltxdoc `macrocode`/`macrocode*` (whose body is LaTeX/expl3 code,
    /// parsed and re-lexed under the package regime, *not* an opaque verbatim
    /// blob like `verbatim_body`). The formatter preserves the body's layout and
    /// never reflows it as prose; the distinction from `verbatim_body` is that the
    /// content is a real CST, not a single `VERBATIM_BODY` token.
    pub code: bool,
    /// `true` for alignment environments whose `&` columns the formatter lays out
    /// into a grid (`align`, `pmatrix`, …). Independent of `math`: every flagged
    /// environment here is also math, but the formatter consults this flag, not
    /// `math`, to decide column alignment.
    pub align: bool,
    /// `true` when the body is ordinary prose the formatter may reflow. Derived as
    /// `!(verbatim_body || math || code)`. (Reflow itself is a later item; this is
    /// the recorded intent.)
    pub reflow: bool,
    /// `true` for sectioning-level *containers* whose body the formatter must
    /// *not* indent (`document`, the appendix-package `appendix`, …). The shared
    /// property is that the body is whole sections/paragraphs — content at the
    /// same structural altitude as the sections the container sits among, not leaf
    /// content like a `figure` or `minipage` — which is conventionally written
    /// flush to the margin. The body is still laid out on its own lines, just at
    /// the surrounding indentation level rather than nested one step in.
    pub no_indent: bool,
    /// `true` for list environments (`itemize`, `enumerate`, `description`, …)
    /// whose `\item`s the formatter lays out one per line, reflowing each item's
    /// body with continuation lines hanging-indented under the item text.
    pub list: bool,
    /// `true` for block/display environments that occupy their own vertical space
    /// (`figure`, `center`, lists, display math, verbatim, …). The parser uses this
    /// to avoid wrapping a lone such environment in a redundant `PARAGRAPH`. Derived
    /// as `block_explicit || math || list || no_indent`.
    pub block: bool,
    /// `Some(_)` for an environment that earns a document-symbol outline entry — a
    /// float or a theorem-like. `None` for everything else. Only meaningful to the
    /// language server's `documentSymbol`; the parser and formatter ignore it.
    pub outline: Option<OutlineKind>,
}

// --- const constructors (shared by the runtime JSON path and build-time codegen)
//
// The build script (`build.rs`) emits the CWL tier as a `phf` map whose values
// are calls to these `const fn`s, so the static data is baked into the binary
// with no runtime parse (see `cwl`). They are the single home of the `reflow`/
// `block` *derivations*, reused by `From<RawEnvironment>` below so the JSON path
// (builtin DB, scanned defs) and the codegen path can never derive them
// differently.

/// `reflow`: a body is reflowable prose unless it is verbatim, math, or code.
pub(crate) const fn derive_reflow(verbatim_body: bool, math: bool, code: bool) -> bool {
    !(verbatim_body || math || code)
}

/// `block`: math, lists, and no-indent containers are inherently block/display;
/// the explicit flag covers the rest (figure, center, verbatim, theorem-likes, …).
pub(crate) const fn derive_block(
    block_explicit: bool,
    math: bool,
    list: bool,
    no_indent: bool,
) -> bool {
    block_explicit || math || list || no_indent
}

/// One argument slot, const-constructible for the codegen path.
pub(crate) const fn arg(required: bool, kind: ArgKind, content: ContentKind) -> ArgSpec {
    ArgSpec {
        required,
        kind,
        content,
    }
}

/// A command signature over a `'static` argument slice (the codegen path).
pub(crate) const fn command(
    args: &'static [ArgSpec],
    sectioning: Option<u8>,
    verbatim: bool,
    rule: bool,
    inline: bool,
) -> CommandSig {
    CommandSig {
        args: Cow::Borrowed(args),
        sectioning,
        verbatim,
        rule,
        inline,
    }
}

/// An environment signature over a `'static` argument slice (the codegen path),
/// applying the same `reflow`/`block` derivations as the JSON path.
#[allow(clippy::too_many_arguments)]
pub(crate) const fn environment(
    args: &'static [ArgSpec],
    verbatim_body: bool,
    math: bool,
    code: bool,
    align: bool,
    no_indent: bool,
    list: bool,
    block_explicit: bool,
    outline: Option<OutlineKind>,
) -> EnvironmentSig {
    EnvironmentSig {
        args: Cow::Borrowed(args),
        verbatim_body,
        math,
        code,
        align,
        reflow: derive_reflow(verbatim_body, math, code),
        no_indent,
        list,
        block: derive_block(block_explicit, math, list, no_indent),
        outline,
    }
}

/// The built-in command and environment signatures, keyed by name (without the
/// leading `\` for commands, the bare name for environments). Case-sensitive, as
/// LaTeX names are (`Verbatim` ≠ `verbatim`).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
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

    /// All known command names (without the leading `\`), in arbitrary order.
    /// Backs name completion, which unions these with the per-document scanned
    /// definitions; the lookup methods stay the only refinement path.
    pub fn command_names(&self) -> impl Iterator<Item = &str> {
        self.commands.keys().map(SmolStr::as_str)
    }

    /// All known environment names, in arbitrary order. See [`command_names`].
    ///
    /// [`command_names`]: Self::command_names
    pub fn environment_names(&self) -> impl Iterator<Item = &str> {
        self.environments.keys().map(SmolStr::as_str)
    }

    /// Record a command signature, replacing any existing entry for `name`. Used
    /// by the per-file definition scan ([`super::define`]) to populate a fresh DB;
    /// the built-in DB is built from JSON and never mutated. A redefinition wins,
    /// mirroring TeX's last-`\newcommand`-wins behavior.
    pub fn insert_command(&mut self, name: impl Into<SmolStr>, sig: CommandSig) {
        self.commands.insert(name.into(), sig);
    }

    /// Record an environment signature, replacing any existing entry for `name`.
    pub fn insert_environment(&mut self, name: impl Into<SmolStr>, sig: EnvironmentSig) {
        self.environments.insert(name.into(), sig);
    }

    /// Merge every command and environment of `other` into `self`, with `other`
    /// winning on a name clash (last-definition-wins, like an individual
    /// `insert_*`). Used to fold a loaded package's scanned definitions into a
    /// document's merged signature scope; the caller orders the merges so the
    /// document's own definitions are applied last and override any package.
    pub fn merge_from(&mut self, other: &SignatureDb) {
        for (name, sig) in &other.commands {
            self.commands.insert(name.clone(), sig.clone());
        }
        for (name, sig) in &other.environments {
            self.environments.insert(name.clone(), sig.clone());
        }
    }
}

/// A two-tier signature lookup: a per-document [`SignatureDb`] of scanned
/// `\newcommand`/`\newenvironment`/xparse definitions consulted first, falling back
/// to the process-wide [`builtin`] DB. Cheap to copy (it borrows the scanned DB),
/// so it threads through the formatter's lowering like a context handle.
///
/// Scanned-first matches TeX scoping intuition: a locally (re)defined command
/// shadows a built-in of the same name. (We do not yet model *where* a definition
/// becomes visible — a whole-file union — which is sound for the formatter's arity
/// needs; lexical/conditional visibility is out of scope, per AGENTS.md #1.)
#[derive(Debug, Clone, Copy)]
pub struct Signatures<'a> {
    user: &'a SignatureDb,
}

impl<'a> Signatures<'a> {
    /// Resolve against `user` first, then the built-in DB.
    pub fn new(user: &'a SignatureDb) -> Self {
        Self { user }
    }

    /// The signature of command `name`: scanned definition first, then the curated
    /// built-in, then the bulk CWL tier. CWL is consulted last and contributes only
    /// argument arity (its behavior flags are all default), so a CWL-only command is
    /// laid out like any unknown command, just with its argument count known.
    pub fn command(&self, name: &str) -> Option<&'a CommandSig> {
        self.user
            .command(name)
            .or_else(|| builtin().command(name))
            .or_else(|| cwl().command(name))
    }

    /// The signature of environment `name`: scanned, then built-in, then CWL. See
    /// [`command`] for why the CWL tier is safe to consult here.
    ///
    /// [`command`]: Self::command
    pub fn environment(&self, name: &str) -> Option<&'a EnvironmentSig> {
        self.user
            .environment(name)
            .or_else(|| builtin().environment(name))
            .or_else(|| cwl().environment(name))
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

/// The type of the build-generated CWL maps: a name-keyed perfect-hash map. The
/// generated `static`s are spelled with this alias, so the dependency on `phf` is
/// visible in checked-in source (not only in the generated file).
type CwlSigMap<V> = phf::Map<&'static str, V>;

// The bulk CWL tier is generated by `build.rs` from `data/cwl_signatures.json`
// into two `CwlSigMap`s (`CWL_COMMANDS`, `CWL_ENVIRONMENTS`) whose values are
// `command(...)`/`environment(...)`/`arg(...)` const-constructor calls — so the
// data is baked into the binary as read-only statics with *zero* runtime parse
// or decompress (it was a ~4.5 ms one-time `LazyLock` decompress+JSON-parse; now
// ~0). The included file references the const constructors and `CwlSigMap` here.
include!(concat!(env!("OUT_DIR"), "/cwl_signatures.rs"));

/// Handle to the lower-precision **CWL tier**: a broad set of command/environment
/// names plus argument shapes harvested from the TeXstudio CWL corpus (a curated
/// package subset; see `scripts/gen_cwl_signatures.py`). It carries *names and
/// arity only* — every behavior flag (`content`/`verbatim`/`sectioning`/`math`/…) is
/// left at its default — so it can widen completion and the formatter's arity
/// lookup without its low-confidence data ever reaching a lexer/outline behavior
/// decision. Consulted strictly *under* [`builtin`] (via [`Signatures`]); the
/// curated tier always wins. A ZST over the generated `phf` statics, so its query
/// methods mirror [`SignatureDb`]'s without owning a heap map.
#[derive(Debug, Clone, Copy)]
pub struct CwlDb;

impl CwlDb {
    /// The signature of command `name` (without the leading `\`), if in the tier.
    pub fn command(&self, name: &str) -> Option<&'static CommandSig> {
        CWL_COMMANDS.get(name)
    }

    /// The signature of environment `name`, if in the tier.
    pub fn environment(&self, name: &str) -> Option<&'static EnvironmentSig> {
        CWL_ENVIRONMENTS.get(name)
    }

    /// All CWL command names (without the leading `\`), in arbitrary order. The
    /// `&str` lifetime is tied to `&self` (not `'static`) so it unifies with the
    /// borrowed scanned-definition names in a completion `chain` (see
    /// `completion::command_candidates`), exactly like [`SignatureDb::command_names`].
    pub fn command_names(&self) -> impl Iterator<Item = &str> {
        CWL_COMMANDS.keys().map(|name| &**name)
    }

    /// All CWL environment names, in arbitrary order. See [`command_names`].
    ///
    /// [`command_names`]: Self::command_names
    pub fn environment_names(&self) -> impl Iterator<Item = &str> {
        CWL_ENVIRONMENTS.keys().map(|name| &**name)
    }

    /// All CWL command signatures (introspection; backs the invariant tests).
    pub fn command_sigs(&self) -> impl Iterator<Item = &'static CommandSig> {
        CWL_COMMANDS.values()
    }

    /// All CWL environment signatures (introspection; backs the invariant tests).
    pub fn environment_sigs(&self) -> impl Iterator<Item = &'static EnvironmentSig> {
        CWL_ENVIRONMENTS.values()
    }
}

static CWL: CwlDb = CwlDb;

/// The process-wide CWL tier (see [`CwlDb`]).
pub fn cwl() -> &'static CwlDb {
    &CWL
}

// --- On-disk schema (serde) ---------------------------------------------------
//
// A thin deserialization mirror of the in-memory types, kept separate so the
// public API stays free of serde concerns and the JSON can use a compact,
// hand-authorable spelling (`"req"`/`"opt"` for arguments; flags defaulting to
// false; `reflow` derived rather than stored).

/// An argument's bracket as written in the JSON: `"req"` (mandatory `{…}`) or
/// `"opt"` (optional `[…]`).
#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum RawArgKind {
    Req,
    Opt,
}

impl RawArgKind {
    fn required(self) -> bool {
        matches!(self, RawArgKind::Req)
    }

    fn kind(self) -> ArgKind {
        match self {
            RawArgKind::Req => ArgKind::Brace,
            RawArgKind::Opt => ArgKind::Bracket,
        }
    }
}

/// An argument's content kind as written in the JSON: `"opaque"` (default),
/// `"prose"`, or `"tokenList"`. Mirrors [`ContentKind`].
#[derive(Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "camelCase")]
enum RawContentKind {
    #[default]
    Opaque,
    Prose,
    TokenList,
}

impl From<RawContentKind> for ContentKind {
    fn from(raw: RawContentKind) -> Self {
        match raw {
            RawContentKind::Opaque => ContentKind::Opaque,
            RawContentKind::Prose => ContentKind::Prose,
            RawContentKind::TokenList => ContentKind::TokenList,
        }
    }
}

/// One argument as written in the JSON. Either the compact string shorthand
/// (`"req"` / `"opt"`, the common case, content defaulting to `"opaque"`) or an
/// object form `{ "kind": "req", "content": "prose" }` / `{ "kind": "req",
/// "content": "tokenList" }` that additionally marks the argument's content kind
/// (see [`ContentKind`]).
#[derive(Deserialize)]
#[serde(untagged)]
enum RawArg {
    Short(RawArgKind),
    Full {
        kind: RawArgKind,
        #[serde(default)]
        content: RawContentKind,
    },
}

impl From<RawArg> for ArgSpec {
    fn from(raw: RawArg) -> Self {
        match raw {
            RawArg::Short(kind) => ArgSpec {
                required: kind.required(),
                kind: kind.kind(),
                content: ContentKind::Opaque,
            },
            RawArg::Full { kind, content } => ArgSpec {
                required: kind.required(),
                kind: kind.kind(),
                content: content.into(),
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
    #[serde(default)]
    rule: bool,
    #[serde(default)]
    inline: bool,
}

impl From<RawCommand> for CommandSig {
    fn from(raw: RawCommand) -> Self {
        CommandSig {
            args: Cow::Owned(raw.args.into_iter().map(ArgSpec::from).collect()),
            sectioning: raw.sectioning,
            verbatim: raw.verbatim,
            rule: raw.rule,
            inline: raw.inline,
        }
    }
}

/// An environment's outline category as written in the JSON: `"float"` or
/// `"theorem"` (absent → `None`, no outline entry).
#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum RawOutlineKind {
    Float,
    Theorem,
}

impl From<RawOutlineKind> for OutlineKind {
    fn from(raw: RawOutlineKind) -> Self {
        match raw {
            RawOutlineKind::Float => OutlineKind::Float,
            RawOutlineKind::Theorem => OutlineKind::Theorem,
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
    #[serde(default)]
    code: bool,
    #[serde(default)]
    align: bool,
    #[serde(default, rename = "noIndent")]
    no_indent: bool,
    #[serde(default)]
    list: bool,
    #[serde(default)]
    block: bool,
    #[serde(default)]
    outline: Option<RawOutlineKind>,
}

impl From<RawEnvironment> for EnvironmentSig {
    fn from(raw: RawEnvironment) -> Self {
        // The `reflow`/`block` derivations live in `derive_reflow`/`derive_block`
        // (shared with the codegen path); only `args` differs (owned here).
        EnvironmentSig {
            args: Cow::Owned(raw.args.into_iter().map(ArgSpec::from).collect()),
            verbatim_body: raw.verbatim_body,
            math: raw.math,
            code: raw.code,
            align: raw.align,
            reflow: derive_reflow(raw.verbatim_body, raw.math, raw.code),
            no_indent: raw.no_indent,
            list: raw.list,
            block: derive_block(raw.block, raw.math, raw.list, raw.no_indent),
            outline: raw.outline.map(OutlineKind::from),
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawDb {
    /// An optional top-level provenance header (the generated `cwl_signatures.json`
    /// carries one); accepted and discarded so `deny_unknown_fields` still rejects
    /// genuine typos elsewhere.
    #[serde(default, rename = "_comment")]
    _comment: Option<serde::de::IgnoredAny>,
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
    fn outline_categories_assigned() {
        let db = builtin();
        assert_eq!(
            db.environment("figure").unwrap().outline,
            Some(OutlineKind::Float)
        );
        assert_eq!(
            db.environment("table*").unwrap().outline,
            Some(OutlineKind::Float)
        );
        assert_eq!(
            db.environment("theorem").unwrap().outline,
            Some(OutlineKind::Theorem)
        );
        // A block layout environment is not outline-worthy.
        assert_eq!(db.environment("center").unwrap().outline, None);
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
    fn content_kind_parses_from_both_forms() {
        // The string shorthand defaults content to `Opaque`; the object form's
        // `content` discriminant sets it.
        let db = parse(
            r#"{ "commands": {
                "short": { "args": ["req"] },
                "full":  { "args": ["opt", { "kind": "req", "content": "prose" }] }
            } }"#,
        )
        .expect("valid content schema");
        let short = &db.command("short").unwrap().args;
        assert_eq!(short[0].content, ContentKind::Opaque);
        let full = &db.command("full").unwrap().args;
        assert_eq!(full[0].kind, ArgKind::Bracket);
        assert_eq!(full[0].content, ContentKind::Opaque); // no `content` → default
        assert_eq!(full[1].kind, ArgKind::Brace);
        assert_eq!(full[1].content, ContentKind::Prose);
    }

    #[test]
    fn bundled_prose_args_flagged() {
        // A representative prose-bearing command marks its mandatory body slot,
        // while a name-bearing command leaves every slot opaque.
        let footnote = &builtin().command("footnote").unwrap().args;
        assert!(footnote.iter().any(|a| a.content == ContentKind::Prose));
        let label = &builtin().command("label").unwrap().args;
        assert!(label.iter().all(|a| a.content == ContentKind::Opaque));
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
        // `equation` is math but not an alignment environment (no `&` columns).
        assert!(!equation.align);
        // An alignment environment carries the `align` flag (and is also math).
        let align = db.environment("align").unwrap();
        assert!(align.math);
        assert!(align.align);
        let pmatrix = db.environment("pmatrix").unwrap();
        assert!(pmatrix.math);
        assert!(pmatrix.align);
        // `tabular` is an alignment environment (its `&` columns grid-align) but,
        // unlike the math families, it is not math.
        let tabular = db.environment("tabular").unwrap();
        assert!(!tabular.verbatim_body);
        assert!(!tabular.math);
        assert!(tabular.align);
        assert!(!tabular.list);
        // List environments carry the `list` flag (and still reflow their bodies).
        for name in ["itemize", "enumerate", "description"] {
            let env = db.environment(name).unwrap();
            assert!(env.list, "{name} should be a list environment");
            assert!(env.reflow);
            assert!(!env.math);
        }
        // jss/Sweave verbatim environments are curated built-ins: their bodies are
        // opaque (preserved verbatim, never reflowed).
        for name in [
            "Code",
            "CodeInput",
            "CodeOutput",
            "Sinput",
            "Soutput",
            "Scode",
        ] {
            let env = db.environment(name).unwrap();
            assert!(env.verbatim_body, "{name} should be a verbatim environment");
            assert!(!env.reflow);
        }
    }

    #[test]
    fn block_flag_is_explicit_or_derived() {
        let db = builtin();
        // Explicitly flagged display environments.
        assert!(db.environment("figure").unwrap().block);
        assert!(db.environment("center").unwrap().block);
        assert!(db.environment("verbatim").unwrap().block);
        // Derived from `math`, `list`, and `no_indent` respectively.
        assert!(db.environment("equation").unwrap().block);
        assert!(db.environment("itemize").unwrap().block);
        assert!(db.environment("document").unwrap().block);
        // The new explicit flag leaves `reflow` derivation untouched: `center`
        // is a block env but still reflows its prose body.
        assert!(db.environment("center").unwrap().reflow);
    }

    #[test]
    fn doc_ltxdoc_signatures() {
        let db = builtin();
        // doc/ltxdoc driver commands each take one mandatory argument.
        for name in ["DocInput", "DescribeMacro", "DescribeEnv", "StopEventually"] {
            let cmd = db
                .command(name)
                .unwrap_or_else(|| panic!("{name} signature"));
            assert_eq!(cmd.args.len(), 1, "{name} arity");
            assert!(cmd.args[0].required, "{name} arg is mandatory");
        }
        // The `macro`/`environment` doc envs document one item and are block
        // containers, but their body is ordinary prose (it still reflows).
        for name in ["macro", "environment"] {
            let env = db.environment(name).unwrap_or_else(|| panic!("{name} env"));
            assert_eq!(env.args.len(), 1, "{name} arity");
            assert!(env.block, "{name} is a block env");
            assert!(env.reflow, "{name} body reflows as prose");
            assert!(!env.code, "{name} is not a code env");
        }
        // `macrocode`/`macrocode*` are code-not-prose: real parsed code (not an
        // opaque verbatim blob), so `code` is set, `reflow` is off, and
        // `verbatim_body` stays off (otherwise the lexer would swallow the body).
        for name in ["macrocode", "macrocode*"] {
            let env = db.environment(name).unwrap_or_else(|| panic!("{name} env"));
            assert!(env.code, "{name} is code");
            assert!(!env.reflow, "{name} never reflows");
            assert!(!env.verbatim_body, "{name} body is parsed, not verbatim");
            assert!(env.block, "{name} is a block env");
        }
    }

    #[test]
    fn code_flag_parses_and_drives_reflow() {
        // The `code` flag defaults false and, when set, suppresses reflow without
        // making the body verbatim.
        let db = parse(
            r#"{ "environments": {
                "plain": {},
                "codeish": { "code": true }
            } }"#,
        )
        .expect("valid code schema");
        let plain = db.environment("plain").unwrap();
        assert!(!plain.code);
        assert!(plain.reflow);
        let codeish = db.environment("codeish").unwrap();
        assert!(codeish.code);
        assert!(!codeish.reflow);
        assert!(!codeish.verbatim_body);
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

    #[test]
    fn cwl_tier_loads_and_covers_long_tail() {
        // Exercises the gzipped bundle through the real decompress+parse path, and
        // confirms the curated package subset reached the tier (a command unlikely
        // to be in the hand-curated built-in DB).
        let db = cwl();
        assert!(db.command("siunitx").is_some() || db.command("SI").is_some());
        assert!(
            db.command_names().count() > 1000,
            "the CWL subset should contribute a broad name set"
        );
    }

    #[test]
    fn cwl_entries_carry_only_arity_no_behavior_flags() {
        // The converter guard: every CWL command/environment is names+arity only, so
        // none of its low-confidence data can flip a formatter/lexer/outline decision.
        let db = cwl();
        for sig in db.command_sigs() {
            assert!(sig.sectioning.is_none());
            assert!(!sig.verbatim && !sig.rule && !sig.inline);
            assert!(sig.args.iter().all(|a| a.content == ContentKind::Opaque));
        }
        for sig in db.environment_sigs() {
            assert!(!sig.verbatim_body && !sig.math && !sig.code && !sig.align);
            assert!(!sig.no_indent && !sig.list && !sig.block);
            assert!(sig.outline.is_none());
        }
    }

    #[test]
    fn curated_builtin_wins_over_cwl_tier() {
        // `Signatures` resolves a name present in both tiers to the curated entry,
        // never the bulk CWL one — proven via a curated-only flag (`\section` is a
        // sectioning command in the built-in DB; the CWL tier never sets that).
        let empty = SignatureDb::default();
        let sigs = Signatures::new(&empty);
        assert!(
            cwl().command("section").is_some(),
            "test premise: in CWL tier"
        );
        assert_eq!(sigs.command("section").unwrap().sectioning, Some(2));
    }

    #[test]
    fn cwl_only_name_resolves_through_signatures() {
        // A name only the CWL tier knows still resolves (arity coverage win), with
        // all behavior flags at their conservative defaults.
        let empty = SignatureDb::default();
        let sigs = Signatures::new(&empty);
        let Some(name) = cwl()
            .command_names()
            .find(|n| builtin().command(n).is_none())
        else {
            panic!("expected at least one CWL-only command name");
        };
        let sig = sigs.command(name).expect("CWL-only name resolves");
        assert!(sig.sectioning.is_none() && !sig.inline && !sig.verbatim);
    }
}
