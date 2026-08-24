//! The built-in **signature database**: command/environment argument shapes plus
//! the semantic metadata a formatter/linter needs (sectioning level,
//! verbatim-ness, math-ness). Meaning is assigned here rather than in the parser.
//!
//! The data is fully static, so it lives in a process-wide [`LazyLock`],
//! consulted directly. Per-file `\newcommand`/`\newenvironment`/xparse
//! signatures are scanned by [`super::define`] into a separate, per-document
//! [`SignatureDb`] and overlaid via [`Signatures`] (scanned-first, built-in
//! fallback). The greedy parser's argument attachment is unaffected either way.
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

use crate::syntax::{SyntaxKind, SyntaxNode};

/// Which bracket delimits an argument. TeX has no other real argument grouping at
/// the surface level the formatter cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgKind {
    /// An argument delimited by `{…}`.
    Brace,
    /// An argument delimited by `[…]`.
    Bracket,
}

/// The TeX mode a command or environment argument is proven to establish.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArgumentDomain {
    /// No safe text-or-math claim is available.
    #[default]
    Unknown,
    /// The argument is parsed and interpreted as math.
    Math,
    /// The argument is parsed and interpreted as text.
    Text,
}

/// How the formatter treats an argument's *content* — its whitespace and break
/// policy. This metadata is for formatter consumers; the parser ignores it.
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
    /// content participates in the surrounding paragraph fill only at its
    /// top-level commas, so an over-width citation can wrap between keys without
    /// splitting a key or detaching its delimiters. Incidental source line breaks
    /// are normalized away, so `\citep{\n a,\n b\n}` formats identically to
    /// `\citep{a, b}` (determinism).
    TokenList,
    /// A `key=value` list consumed by a keyval-family processor — keyval, xkeyval,
    /// pgfkeys, l3keys, or LaTeX's own option-list scanner — every one of which
    /// strips spaces around keys and values before acting on them. That is what
    /// licenses the formatter to break the list at a comma the author *glued*
    /// (`[xmin=-5,xmax=5]`), materializing a space token TeX will see: in a keyval
    /// argument the space is discarded, so the typeset output cannot change.
    ///
    /// The distinction is load-bearing and was settled by compiling both spellings:
    /// keyval brackets (`\usepackage`, `\includegraphics`, `tikz`/`pgfplots`,
    /// `lstlisting`) render identically, while *textual* optionals do not
    /// (`\item[red,green]`, `\caption[short,list]`, `\cite[see,also]`, and a
    /// `\newcommand` default all gain a visible space). So this flag must never be
    /// set on an argument whose content is typeset — hold it to the same curated
    /// standard as the math-env routing.
    ///
    /// Unlike [`TokenList`](ContentKind::TokenList), which flows inline with its
    /// surrounding paragraph, a keyval list expands as its own delimited group.
    Keyval,
}

/// One argument slot in a command/environment signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArgSpec {
    /// Whether the argument must be present, independently of its delimiter kind.
    pub required: bool,
    pub kind: ArgKind,
    /// How the formatter treats this argument's content. See [`ContentKind`].
    pub content: ContentKind,
    /// The mode established by this positional argument, independently of
    /// [`ContentKind`].
    pub domain: ArgumentDomain,
    /// Whether this braced argument is read under a curated raw-text lexer mode.
    /// Independent of formatter [`ContentKind`] and text-or-math
    /// [`ArgumentDomain`].
    pub verbatim: bool,
}

/// Match an attached group to the next positional signature slot.
///
/// Omitted optional slots are skipped regardless of their delimiter kind. A
/// mismatched group never consumes a pending required slot, and an unmatched
/// group leaves `slot` at that required slot.
pub fn match_arg_slot(args: &[ArgSpec], slot: &mut usize, kind: ArgKind) -> Option<ArgSpec> {
    match_arg_slot_index(args, slot, kind).map(|index| args[index])
}

/// The index-returning form of [`match_arg_slot`], used by signature help.
pub fn match_arg_slot_index(args: &[ArgSpec], slot: &mut usize, kind: ArgKind) -> Option<usize> {
    while *slot < args.len() {
        let index = *slot;
        let spec = args[index];
        if spec.kind == kind {
            *slot += 1;
            return Some(index);
        }
        if !spec.required {
            *slot += 1;
            continue;
        }
        return None;
    }
    None
}

/// Match an attached raw `VERB` token to the next positional verbatim slot.
/// Existing whole-command captures (`\url`, `\lstinline`, …) are implicit and
/// therefore match no slot.
pub fn match_verbatim_arg_slot(args: &[ArgSpec], slot: &mut usize) -> Option<ArgSpec> {
    while *slot < args.len() {
        let spec = args[*slot];
        if spec.verbatim {
            *slot += 1;
            return Some(spec);
        }
        if !spec.required {
            *slot += 1;
            continue;
        }
        return None;
    }
    None
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
    /// that argument as one `VERB` token. Any leading, non-verbatim arguments
    /// (e.g. `\mintinline`'s language) are declared in `args`; the verbatim
    /// argument itself is implicit and not listed there.
    pub verbatim: bool,
    /// `true` when the verbatim argument may also be a `\verb`-style delimiter
    /// run (`\lstinline|…|`, `\url|…|`) instead of a balanced `{…}` group.
    /// Braced-only commands (`\code`, `\path`) capture nothing when no brace
    /// follows and lex normally — the name may be an unrelated user macro
    /// (`\code` as a math operator, TikZ's `\path (0,0)`), and a wrong
    /// delimiter capture swallows text across the line. Only meaningful when
    /// `verbatim` is set.
    pub verbatim_delimited: bool,
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
    /// `true` for *block-level* commands that conventionally own their physical
    /// line (`\usepackage`, `\newcommand`, `\maketitle`, …): package/class
    /// loading, preamble machinery, definitions, and document structure. Prose
    /// reflow places such a command on its own line whatever trivia the author
    /// wrote, instead of preserving the line only when the source happened to
    /// break there (the lone-newline predicate trivia-invariant layout forbids).
    /// Sectioning commands are block-level too, implied by [`Self::sectioning`]
    /// at the formatter's query, so entries carrying `sectioning` do not also
    /// set this. Curated-only, like [`Self::verbatim_delimited`]: never from
    /// the CWL tier or scanned definitions — an unknown macro's block-ness is
    /// undecidable without meaning, so those fall back to the formatter's
    /// residual authored-break rule. `\caption` and `\label` are deliberately
    /// excluded (a glued `\caption{…} \label{…}` pair must stay untouched), as
    /// are `\item` (owned by list layout) and `\input` (its TeX-primitive bare
    /// form `\input docstrip.tex` leaves the filename outside the node). Only
    /// meaningful to the formatter; the parser ignores it.
    pub block: bool,
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
    /// `true` for environments whose *name argument* is xparse `v`-type (l3doc's
    /// `macro`/`function`/`variable`, declared `{ O{} +v }`): besides the usual
    /// braced form, the argument may be delimited `\verb`-style
    /// (`\begin{macro}+\@@_compile_{:+`), chosen precisely when the name holds
    /// unbalanced braces. The lexer captures that delimited form as one opaque
    /// `VERB` token; the braced form lexes normally. Curated only — a wrong
    /// grant swallows real text, so the CWL/user tiers never set it.
    pub verbatim_arg: bool,
    /// `true` for math environments (`equation`, `align`, …).
    pub math: bool,
    /// `true` for environments whose body is *real parsed code*, not prose —
    /// the doc/ltxdoc `macrocode`/`macrocode*` (whose body is LaTeX/expl3 code,
    /// parsed and re-lexed under the package regime, *not* an opaque verbatim
    /// blob like `verbatim_body`). The formatter preserves the body's layout and
    /// never reflows it as prose; the distinction from `verbatim_body` is that the
    /// content is a real CST, not a single `VERBATIM_BODY` token.
    pub code: bool,
    /// `true` for environments whose body is a sequence of *delimited
    /// statements* rather than running prose — the TikZ/pgf picture family,
    /// whose content is `;`-terminated paths (`\draw … ;`, `\node … ;`). The
    /// parser wraps each run up to a top-level `;` in a `STATEMENT` node, and
    /// the formatter derives one-statement-per-line, the continuation hang,
    /// and unit-boundary wrapping (`semantic::tikz`) from it.
    ///
    /// The flag also carries a **whitespace-safety claim**, the
    /// [`ContentKind::Keyval`] pattern: whitespace *between* a flagged body's
    /// statements is insignificant to the package that consumes them, which is
    /// what licenses the formatter to open a new line at a statement seam the
    /// author glued (`…;\draw`) — an inserted space token TeX sees but the
    /// package discards. `task typeset:check` carries the proving case
    /// (`tests/typeset/statement_seams.tex`).
    ///
    /// Distinct from [`code`](Self::code), which is the `.dtx` documentation
    /// layer's `macrocode` — code re-lexed under the package regime, a fact about
    /// *lexing*. This one is a fact about statement structure and layout, so the
    /// two must not be conflated: a future consumer of `code` is asking a `.dtx`
    /// question.
    ///
    /// **Curated tier only.** The statement terminator is package grammar, not a
    /// TeX-surface fact, so nothing mechanical can derive this; the CWL codegen
    /// and the runtime definition scan never set it. A wrong grant reshapes
    /// layout for the whole body *and* asserts the whitespace claim above, so
    /// hold it to the standard of the `math` routing flag.
    pub statement_body: bool,
    /// `true` when a top-level `label` entry in the environment's first optional
    /// argument creates a LaTeX label definition. This is narrower than
    /// [`ContentKind::Keyval`]: many key-value processors have a `label` key whose
    /// meaning is unrelated to `\label`, so only curated package facts may set it.
    /// Project declarations inherit the fact through `like`; the CWL and scanned
    /// tiers never grant it.
    pub label_key: bool,
    /// `true` for alignment environments whose `&` columns the formatter lays out
    /// into a grid (`align`, `pmatrix`, …). Independent of `math`: every flagged
    /// environment here is also math, but the formatter consults this flag, not
    /// `math`, to decide column alignment.
    pub align: bool,
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
    /// `true` when this environment is explicitly known to occupy its own vertical
    /// space (`figure`, `center`, verbatim, …). Math, list, and no-indent
    /// environments are inherently block-level and are included by [`Self::block`].
    pub block_explicit: bool,
    /// `Some(_)` for an environment that earns a document-symbol outline entry — a
    /// float or a theorem-like. `None` for everything else. Only meaningful to the
    /// language server's `documentSymbol`; the parser and formatter ignore it.
    pub outline: Option<OutlineKind>,
}

impl EnvironmentSig {
    /// Whether the body is ordinary prose that the formatter may reflow.
    pub const fn reflow(&self) -> bool {
        !(self.verbatim_body || self.math || self.code || self.statement_body)
    }

    /// Whether the environment occupies its own vertical space.
    pub const fn block(&self) -> bool {
        self.block_explicit || self.math || self.list || self.no_indent
    }
}

// --- const constructors (shared by the runtime JSON path and build-time codegen)
//
// The build script (`build.rs`) emits the CWL tier as a `phf` map whose values
// are calls to these `const fn`s, so the static data is baked into the binary
// with no runtime parse (see `cwl`).

/// One argument slot, const-constructible for the codegen path.
pub(crate) const fn arg(required: bool, kind: ArgKind, content: ContentKind) -> ArgSpec {
    ArgSpec {
        required,
        kind,
        content,
        domain: ArgumentDomain::Unknown,
        verbatim: false,
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
        // The codegen (CWL) tier is arity-only, so the delimiter facet — like
        // every behavior flag — never comes from it.
        verbatim_delimited: false,
        rule,
        inline,
        // Curated-only facet, like the delimiter one: block-ness never comes
        // from the codegen (CWL) tier.
        block: false,
    }
}

/// An environment signature over a `'static` argument slice (the codegen path),
/// storing the explicit source facts from the generated data.
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
        // The codegen (CWL) tier is arity-only, so the verbatim-argument facet —
        // like every behavior flag — never comes from it.
        verbatim_arg: false,
        math,
        code,
        // Curated-only facet, like the verbatim-argument one: a statement body is
        // package grammar the mechanical tier cannot see.
        statement_body: false,
        // A key named `label` is not enough to prove `\label` semantics, so the
        // mechanical CWL tier can never grant this fact.
        label_key: false,
        align,
        no_indent,
        list,
        block_explicit,
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
    /// Which loaded package (by file stem) a command signature came from, when
    /// it was merged with an explicit origin via [`merge_from`](Self::merge_from).
    /// Absent for the document's own definitions and for every static tier
    /// (built-in/CWL DBs never carry origins). A side map rather than a
    /// `CommandSig` field so the phf-generated static tables stay untouched.
    command_origins: HashMap<SmolStr, SmolStr>,
    /// The environment mirror of [`command_origins`](Self::command_origins).
    environment_origins: HashMap<SmolStr, SmolStr>,
    /// File-local *environment aliases*, opener side: a command name (without the
    /// leading `\`) whose definition body is exactly `\begin{X}`, mapped to the
    /// target environment `X`. Populated only by the per-file definition scan
    /// ([`super::define`]), which admits an alias solely when `X` is a curated
    /// built-in environment that is non-verbatim and takes no arguments, and when
    /// both halves of the pair are defined in the same file.
    ///
    /// A *side map*, deliberately not an [`EnvironmentSig`] cloned under the alias
    /// name: the alias is a command, not an environment, so it must not appear in
    /// [`environment_names`](Self::environment_names) (that would offer
    /// `\begin{bea}` to completion) and must not mask a real
    /// `\newenvironment{bea}`. Nor may a *literal* `\begin{bea}` acquire the
    /// target's behavior — which is why the only lookup that consults this map is
    /// [`Signatures::environment_at`], keyed on the node so it can tell an alias
    /// delimiter from a spelled-out environment that happens to share the name.
    /// The plain name-keyed [`Signatures::environment`] never reads it.
    env_begin_aliases: HashMap<SmolStr, SmolStr>,
    /// The closer mirror of [`env_begin_aliases`](Self::env_begin_aliases): a
    /// command whose body is exactly `\end{X}`. Kept separate rather than tagged
    /// with a side, so the parser's opener and closer indices cannot be confused
    /// and [`Signatures::environment`] can consult the opener side alone.
    env_end_aliases: HashMap<SmolStr, SmolStr>,
    /// Which environment signatures came from a project *declaration*
    /// ([`crate::declarations`]) rather than from a scan.
    ///
    /// Provenance, like the origin maps above, and needed for the same kind of
    /// reason: [`Signatures::environment_at`] resolves an alias target against
    /// *curated* data only, so it has to tell a declared entry (curated — `like`
    /// copies a built-in and resolves against nothing else) from a scanned
    /// `\newenvironment` of the same name (not curated, and deliberately unable
    /// to lend an alias its behavior). Without the mark the two are
    /// indistinguishable once merged into one scope.
    declared_environments: std::collections::HashSet<SmolStr>,
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

    /// The environment `name` opens, if the per-file scan recorded it as an
    /// environment alias ([`env_begin_aliases`](Self::env_begin_aliases)). The name
    /// carries no leading `\`.
    pub fn env_begin_alias(&self, name: &str) -> Option<&str> {
        self.env_begin_aliases.get(name).map(SmolStr::as_str)
    }

    /// The closer mirror of [`env_begin_alias`](Self::env_begin_alias).
    pub fn env_end_alias(&self, name: &str) -> Option<&str> {
        self.env_end_aliases.get(name).map(SmolStr::as_str)
    }

    /// Every recorded opener alias, as `(alias, target)` pairs in arbitrary order.
    /// Backs the parser's projection of the map into its parse context.
    pub fn env_begin_aliases(&self) -> impl Iterator<Item = (&str, &str)> {
        self.env_begin_aliases
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// The closer mirror of [`env_begin_aliases`](Self::env_begin_aliases).
    pub fn env_end_aliases(&self) -> impl Iterator<Item = (&str, &str)> {
        self.env_end_aliases
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Record an opener alias, replacing any existing entry for `name`.
    pub fn insert_env_begin_alias(&mut self, name: impl Into<SmolStr>, target: impl Into<SmolStr>) {
        self.env_begin_aliases.insert(name.into(), target.into());
    }

    /// Record a closer alias, replacing any existing entry for `name`.
    pub fn insert_env_end_alias(&mut self, name: impl Into<SmolStr>, target: impl Into<SmolStr>) {
        self.env_end_aliases.insert(name.into(), target.into());
    }

    /// The package (file stem) whose merge supplied the current signature of
    /// command `name`, if it came from a package
    /// ([`merge_from`](Self::merge_from) with `Some(origin)`) rather than the
    /// document or a static tier.
    pub fn command_origin(&self, name: &str) -> Option<&str> {
        self.command_origins.get(name).map(SmolStr::as_str)
    }

    /// The environment mirror of [`command_origin`](Self::command_origin).
    pub fn environment_origin(&self, name: &str) -> Option<&str> {
        self.environment_origins.get(name).map(SmolStr::as_str)
    }

    /// Record a command signature, replacing any existing entry for `name`. Used
    /// by the per-file definition scan ([`super::define`]) to populate a fresh DB;
    /// the built-in DB is built from JSON and never mutated. A redefinition wins,
    /// mirroring TeX's last-`\newcommand`-wins behavior; any recorded package
    /// origin is cleared, since it described the entry being replaced.
    pub fn insert_command(&mut self, name: impl Into<SmolStr>, sig: CommandSig) {
        let name = name.into();
        self.command_origins.remove(&name);
        self.commands.insert(name, sig);
    }

    /// Record an environment signature, replacing any existing entry for `name`.
    pub fn insert_environment(&mut self, name: impl Into<SmolStr>, sig: EnvironmentSig) {
        let name = name.into();
        self.environment_origins.remove(&name);
        self.declared_environments.remove(&name);
        self.environments.insert(name, sig);
    }

    /// Record an environment signature that came from a project *declaration*,
    /// replacing any existing entry for `name` and marking its provenance. See
    /// [`declared_environments`](Self::declared_environments) for why the mark
    /// exists.
    pub fn insert_declared_environment(&mut self, name: impl Into<SmolStr>, sig: EnvironmentSig) {
        let name = name.into();
        self.insert_environment(name.clone(), sig);
        self.declared_environments.insert(name);
    }

    /// Whether `name`'s signature came from a project declaration.
    pub fn is_declared_environment(&self, name: &str) -> bool {
        self.declared_environments.contains(name)
    }

    /// Merge every command and environment of `other` into `self`, with `other`
    /// winning on a name clash (last-definition-wins, like an individual
    /// `insert_*`). Used to fold a loaded package's scanned definitions into a
    /// document's merged signature scope; the caller orders the merges so the
    /// document's own definitions are applied last and override any package.
    ///
    /// When `origin` is `Some`, it replaces the provenance of every merged
    /// signature. When it is `None`, each entry inherits `other`'s provenance,
    /// clearing stale provenance when `other` has none.
    pub fn merge_from(&mut self, other: &SignatureDb, origin: Option<&str>) {
        for (name, sig) in &other.commands {
            match origin
                .map(SmolStr::new)
                .or_else(|| other.command_origins.get(name).cloned())
            {
                Some(origin) => {
                    self.command_origins.insert(name.clone(), origin);
                }
                None => {
                    self.command_origins.remove(name);
                }
            }
            self.commands.insert(name.clone(), sig.clone());
        }
        for (name, sig) in &other.environments {
            match origin
                .map(SmolStr::new)
                .or_else(|| other.environment_origins.get(name).cloned())
            {
                Some(origin) => {
                    self.environment_origins.insert(name.clone(), origin);
                }
                None => {
                    self.environment_origins.remove(name);
                }
            }
            // Declared-ness describes the *current* entry, exactly as the origin
            // above does: an overwrite from a non-declared source clears it, so a
            // scanned definition merged over a declared name cannot leave the
            // alias resolver believing the entry is still curated.
            if origin.is_none() && other.is_declared_environment(name) {
                self.declared_environments.insert(name.clone());
            } else {
                self.declared_environments.remove(name);
            }
            self.environments.insert(name.clone(), sig.clone());
        }
        for (name, target) in &other.env_begin_aliases {
            self.env_begin_aliases.insert(name.clone(), target.clone());
        }
        for (name, target) in &other.env_end_aliases {
            self.env_end_aliases.insert(name.clone(), target.clone());
        }
    }

    /// Overlay a project's resolved [declarations](crate::declarations) as the
    /// **top tier** of this scope: a declaration is the user explicitly
    /// correcting an inference, so it wins over scanned definitions and loaded
    /// packages alike.
    ///
    /// A named entry rather than `merge_from(declared.as_db(), None)` at each call
    /// site, so the precedence rule is stated once and the two scope builders
    /// (the CLI's `collect_package_signatures` and the salsa `scope_signatures`)
    /// cannot disagree about where in the order it goes.
    pub fn merge_declarations(&mut self, declared: &crate::declarations::ResolvedDeclarations) {
        self.merge_from(declared.as_db(), None);
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
    /// Environment *aliases* are deliberately **not** consulted: an alias names a
    /// command, and a name alone cannot tell a `\bea`-opened delimiter from a
    /// literal `\begin{bea}` that happens to spell the same word. Resolve those
    /// through [`environment_at`](Self::environment_at), which has the node.
    ///
    /// [`command`]: Self::command
    pub fn environment(&self, name: &str) -> Option<&'a EnvironmentSig> {
        self.user
            .environment(name)
            .or_else(|| builtin().environment(name))
            .or_else(|| cwl().environment(name))
    }

    /// The signature `name` was *declared* with, if the scope carries one. The
    /// curated half of the alias resolution above; never a scanned entry.
    fn declared_environment(&self, name: &str) -> Option<&'a EnvironmentSig> {
        self.user
            .is_declared_environment(name)
            .then(|| self.user.environment(name))
            .flatten()
    }

    /// The signature governing `node` — an `ENVIRONMENT` or its `BEGIN` — which is
    /// [`environment`](Self::environment) except that an *environment-alias*
    /// delimiter resolves through the alias map instead.
    ///
    /// This is the node-keyed lookup every layout decision wants, because an alias
    /// `BEGIN` (a bare control word, [`Begin::is_alias`]) and a literal `\begin{X}`
    /// are indistinguishable once reduced to a name. Only the former inherits the
    /// target's behavior; a literal `\begin{bea}` in a file that also defines `\bea`
    /// as an alias is an unrelated environment of that name and stays unknown.
    ///
    /// The alias arm resolves against **curated data only**, for the same reason
    /// the parser's `ParseCtx::is_math_environment` does: an alias declares a
    /// *spelling*, never a *semantic*, so every behavior flag still comes from
    /// curated data. That means [`builtin`] plus the scope's *declared* entries
    /// — a declaration is curated (`like` copies a built-in entry and resolves
    /// against nothing else), which is what lets `\startmyenv … \endmyenv` reach
    /// the behavior of a `myenv` that has no built-in counterpart. A scanned
    /// `\newenvironment` of the same name still lends an alias nothing.
    ///
    /// [`Begin::is_alias`]: crate::ast::Begin::is_alias
    pub fn environment_at(&self, node: &SyntaxNode) -> Option<&'a EnvironmentSig> {
        use crate::ast::{AstNode, Begin, child};
        let begin = match node.kind() {
            SyntaxKind::BEGIN => Begin::cast(node.clone())?,
            _ => child::<Begin>(node)?,
        };
        let name = begin.name()?;
        if begin.is_alias() {
            return self.user.env_begin_alias(&name).and_then(|target| {
                self.declared_environment(target)
                    .or_else(|| builtin().environment(target))
            });
        }
        self.environment(&name)
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
/// `"prose"`, `"tokenList"`, or `"keyval"`. Mirrors [`ContentKind`].
#[derive(Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "camelCase")]
enum RawContentKind {
    #[default]
    Opaque,
    Prose,
    TokenList,
    Keyval,
}

#[derive(Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "lowercase")]
enum RawArgumentDomain {
    #[default]
    Unknown,
    Math,
    Text,
}

impl From<RawArgumentDomain> for ArgumentDomain {
    fn from(raw: RawArgumentDomain) -> Self {
        match raw {
            RawArgumentDomain::Unknown => ArgumentDomain::Unknown,
            RawArgumentDomain::Math => ArgumentDomain::Math,
            RawArgumentDomain::Text => ArgumentDomain::Text,
        }
    }
}

impl From<RawContentKind> for ContentKind {
    fn from(raw: RawContentKind) -> Self {
        match raw {
            RawContentKind::Opaque => ContentKind::Opaque,
            RawContentKind::Prose => ContentKind::Prose,
            RawContentKind::TokenList => ContentKind::TokenList,
            RawContentKind::Keyval => ContentKind::Keyval,
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
        #[serde(default)]
        domain: RawArgumentDomain,
        #[serde(default)]
        verbatim: bool,
    },
}

impl From<RawArg> for ArgSpec {
    fn from(raw: RawArg) -> Self {
        match raw {
            RawArg::Short(kind) => ArgSpec {
                required: kind.required(),
                kind: kind.kind(),
                content: ContentKind::Opaque,
                domain: ArgumentDomain::Unknown,
                verbatim: false,
            },
            RawArg::Full {
                kind,
                content,
                domain,
                verbatim,
            } => ArgSpec {
                required: kind.required(),
                kind: kind.kind(),
                content: content.into(),
                domain: domain.into(),
                verbatim,
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
    #[serde(default, rename = "verbatimDelimited")]
    verbatim_delimited: bool,
    #[serde(default)]
    rule: bool,
    #[serde(default)]
    inline: bool,
    #[serde(default)]
    block: bool,
}

impl From<RawCommand> for CommandSig {
    fn from(raw: RawCommand) -> Self {
        CommandSig {
            args: Cow::Owned(raw.args.into_iter().map(ArgSpec::from).collect()),
            sectioning: raw.sectioning,
            verbatim: raw.verbatim,
            verbatim_delimited: raw.verbatim_delimited,
            rule: raw.rule,
            inline: raw.inline,
            block: raw.block,
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
    #[serde(default, rename = "verbatimArg")]
    verbatim_arg: bool,
    #[serde(default)]
    math: bool,
    #[serde(default)]
    code: bool,
    #[serde(default, rename = "statementBody")]
    statement_body: bool,
    #[serde(default, rename = "labelKey")]
    label_key: bool,
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
        EnvironmentSig {
            args: Cow::Owned(raw.args.into_iter().map(ArgSpec::from).collect()),
            verbatim_body: raw.verbatim_body,
            verbatim_arg: raw.verbatim_arg,
            math: raw.math,
            code: raw.code,
            statement_body: raw.statement_body,
            label_key: raw.label_key,
            align: raw.align,
            no_indent: raw.no_indent,
            list: raw.list,
            block_explicit: raw.block,
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
        command_origins: HashMap::new(),
        environment_origins: HashMap::new(),
        // Aliases are a per-file scan product only; the curated JSON never carries any.
        env_begin_aliases: HashMap::new(),
        env_end_aliases: HashMap::new(),
        // The built-in tier *is* the curated data a declaration copies from, so
        // nothing in it is itself declared.
        declared_environments: std::collections::HashSet::new(),
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
    fn block_commands_flagged() {
        let db = builtin();
        assert!(db.command("usepackage").unwrap().block);
        assert!(db.command("newcommand").unwrap().block);
        assert!(db.command("maketitle").unwrap().block);
        assert!(db.command("title").unwrap().block);
        // A block command still carries its argument shape (the curated tier
        // masks CWL wholesale, so dropping the args here would lose them).
        assert_eq!(db.command("usepackage").unwrap().args.len(), 2);
        // Deliberate exclusions: a glued `\caption{…} \label{…}` pair must stay
        // untouched, and inline commands are the opposite claim.
        assert!(!db.command("caption").unwrap().block);
        assert!(!db.command("label").unwrap().block);
        assert!(!db.command("textbf").unwrap().block);
        // Sectioning implies block at the formatter's query; the entries do not
        // double-flag.
        assert!(!db.command("section").unwrap().block);
    }

    #[test]
    fn no_command_is_both_inline_and_block() {
        // `inline` says a command flows into the fill; `block` says it owns its
        // line. A curated entry claiming both would leave the formatter's
        // dispatch order deciding, silently.
        for name in builtin().command_names() {
            let sig = builtin().command(name).unwrap();
            assert!(
                !(sig.inline && sig.block),
                "`\\{name}` is flagged both inline and block"
            );
        }
    }

    #[test]
    fn verbatim_commands_flagged() {
        assert!(builtin().command("verb").unwrap().verbatim);
        assert!(builtin().command("lstinline").unwrap().verbatim);
        assert!(!builtin().command("textbf").unwrap().verbatim);
        // The delimiter form is opt-in: `\lstinline|…|` has it, the braced-only
        // `\code`/`\path` (jss, url) do not — their names collide with common
        // user macros (issue #53).
        assert!(builtin().command("lstinline").unwrap().verbatim_delimited);
        assert!(!builtin().command("code").unwrap().verbatim_delimited);
        assert!(!builtin().command("path").unwrap().verbatim_delimited);
    }

    #[test]
    fn content_kind_parses_from_both_forms() {
        // The string shorthand defaults content to `Opaque`; the object form's
        // `content` discriminant sets it.
        let db = parse(
            r#"{ "commands": {
                "short": { "args": ["req"] },
                "full":  { "args": ["opt", { "kind": "req", "content": "prose" }] },
                "kv":    { "args": [{ "kind": "opt", "content": "keyval" }, "req"] },
                "list":  { "args": [{ "kind": "req", "content": "tokenList" }] }
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
        // Every wire spelling round-trips, `keyval` included — the flag the
        // optional-argument layout reads to license splitting a glued comma.
        let kv = &db.command("kv").unwrap().args;
        assert_eq!(kv[0].kind, ArgKind::Bracket);
        assert_eq!(kv[0].content, ContentKind::Keyval);
        assert_eq!(kv[1].content, ContentKind::Opaque);
        let list = &db.command("list").unwrap().args;
        assert_eq!(list[0].content, ContentKind::TokenList);
    }

    #[test]
    fn positional_verbatim_defaults_off_and_parses_from_full_form() {
        let db = parse(
            r#"{ "commands": {
                "short": { "args": ["req"] },
                "raw": { "args": [{ "kind": "req", "verbatim": true }, "req"] }
            } }"#,
        )
        .expect("valid positional verbatim schema");

        assert!(!db.command("short").unwrap().args[0].verbatim);
        let raw = &db.command("raw").unwrap().args;
        assert!(raw[0].verbatim);
        assert!(!raw[1].verbatim);
    }

    #[test]
    fn positional_verbatim_slot_keeps_later_groups_aligned() {
        let args = &builtin().command("href").unwrap().args;
        let mut slot = 0;

        assert_eq!(
            match_arg_slot_index(args, &mut slot, ArgKind::Bracket),
            Some(0)
        );
        assert!(match_verbatim_arg_slot(args, &mut slot).is_some());
        assert_eq!(
            match_arg_slot_index(args, &mut slot, ArgKind::Brace),
            Some(2)
        );
    }

    #[test]
    fn argument_domain_defaults_and_json_values_are_independent_of_content() {
        let db = parse(
            r#"{ "commands": {
                "short": { "args": ["req"] },
                "math": { "args": [{ "kind": "req", "content": "prose", "domain": "math" }] },
                "text": { "args": [{ "kind": "opt", "domain": "text" }] }
            } }"#,
        )
        .unwrap();
        assert_eq!(
            db.command("short").unwrap().args[0].domain,
            ArgumentDomain::Unknown
        );
        assert_eq!(
            db.command("math").unwrap().args[0].domain,
            ArgumentDomain::Math
        );
        assert_eq!(
            db.command("math").unwrap().args[0].content,
            ContentKind::Prose
        );
        assert_eq!(
            db.command("text").unwrap().args[0].domain,
            ArgumentDomain::Text
        );
        assert!(
            cwl()
                .command("multicolumn")
                .unwrap()
                .args
                .iter()
                .all(|arg| arg.domain == ArgumentDomain::Unknown)
        );
    }

    #[test]
    fn positional_matching_skips_only_omitted_optionals() {
        let args = &builtin().command("sqrt").unwrap().args;
        let mut slot = 0;
        assert_eq!(
            match_arg_slot_index(args, &mut slot, ArgKind::Brace),
            Some(1)
        );
        assert_eq!(slot, 2);

        let mut slot = 0;
        assert_eq!(
            match_arg_slot_index(args, &mut slot, ArgKind::Bracket),
            Some(0)
        );
        assert_eq!(
            match_arg_slot_index(args, &mut slot, ArgKind::Brace),
            Some(1)
        );
        assert_eq!(match_arg_slot_index(args, &mut slot, ArgKind::Brace), None);

        let frac = &builtin().command("frac").unwrap().args;
        let mut slot = 0;
        assert_eq!(
            match_arg_slot_index(frac, &mut slot, ArgKind::Bracket),
            None
        );
        assert_eq!(slot, 0);
        assert_eq!(
            match_arg_slot_index(frac, &mut slot, ArgKind::Brace),
            Some(0)
        );

        let optional_brace_then_required_bracket = crate::semantic::xparse::parse_spec("d{} r[]");
        let mut slot = 0;
        assert_eq!(
            match_arg_slot_index(
                &optional_brace_then_required_bracket,
                &mut slot,
                ArgKind::Bracket,
            ),
            Some(1)
        );

        let required_bracket_then_brace = crate::semantic::xparse::parse_spec("r[] m");
        let mut slot = 0;
        assert_eq!(
            match_arg_slot_index(&required_bracket_then_brace, &mut slot, ArgKind::Brace),
            None
        );
        assert_eq!(slot, 0);
        assert_eq!(
            match_arg_slot_index(&required_bracket_then_brace, &mut slot, ArgKind::Bracket),
            Some(0)
        );
    }

    #[test]
    fn bundled_prose_args_flagged() {
        // Prose content is also a positive text-domain claim, while a name-bearing
        // command leaves every slot opaque and unknown.
        for name in builtin().command_names() {
            for argument in builtin().command(name).unwrap().args.iter() {
                if argument.content == ContentKind::Prose {
                    assert_eq!(argument.domain, ArgumentDomain::Text, "\\{name}");
                }
            }
        }
        let footnote = &builtin().command("footnote").unwrap().args;
        assert!(footnote.iter().any(|a| a.content == ContentKind::Prose));
        let section = &builtin().command("section").unwrap().args;
        assert!(
            section
                .iter()
                .all(|argument| argument.domain == ArgumentDomain::Text)
        );
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
    fn environment_derived_flags_follow_source_mutation() {
        let mut sig = EnvironmentSig::from(RawEnvironment::default());
        assert!(sig.reflow());
        assert!(!sig.block());

        sig.verbatim_body = true;
        assert!(!sig.reflow());

        sig.block_explicit = true;
        assert!(sig.block());
        sig.block_explicit = false;

        sig.math = true;
        assert!(sig.block());
    }

    #[test]
    fn environment_flags_and_derived_reflow() {
        let db = builtin();
        let lstlisting = db.environment("lstlisting").unwrap();
        assert!(lstlisting.verbatim_body);
        assert!(!lstlisting.reflow());
        let equation = db.environment("equation").unwrap();
        assert!(equation.math);
        assert!(!equation.reflow());
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
            assert!(env.reflow());
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
            assert!(!env.reflow());
        }
    }

    /// Verbatim bodies whose defining code no in-file scan can reach: the kernel's
    /// `filecontents` (it `\@makeother`s `\dospecials`, `%` included, so the body is
    /// written out byte-for-byte) and ltxdockit's listings-based `ltxcode`/
    /// `ltxexample`, defined in an external class. Curation is the only place these
    /// facts can live. Smoke-test issue #98 (`plk/biblatex`).
    #[test]
    fn externally_defined_verbatim_environments() {
        let db = builtin();
        for name in ["filecontents", "filecontents*"] {
            let env = db.environment(name).unwrap();
            assert!(env.verbatim_body, "{name} body is written verbatim");
            assert!(!env.reflow());
            // `\begin{filecontents}[force]{\jobname.bib}`: the two leading args are
            // structured; everything after them is the opaque body.
            assert_eq!(env.args.len(), 2, "{name} arity");
            assert_eq!(env.args[0].kind, ArgKind::Bracket);
            assert_eq!(env.args[1].kind, ArgKind::Brace);
        }
        for name in ["ltxcode", "ltxexample"] {
            let env = db.environment(name).unwrap();
            assert!(env.verbatim_body, "{name} body is opaque");
            assert!(!env.reflow());
            // `\lstnewenvironment{…}[1][]` — one optional `\lstset` argument.
            assert_eq!(env.args.len(), 1, "{name} arity");
            assert_eq!(env.args[0].kind, ArgKind::Bracket);
        }
    }

    #[test]
    fn block_flag_is_explicit_or_derived() {
        let db = builtin();
        // Explicitly flagged display environments.
        assert!(db.environment("figure").unwrap().block());
        assert!(db.environment("center").unwrap().block());
        assert!(db.environment("verbatim").unwrap().block());
        // Derived from `math`, `list`, and `no_indent` respectively.
        assert!(db.environment("equation").unwrap().block());
        assert!(db.environment("itemize").unwrap().block());
        assert!(db.environment("document").unwrap().block());
        // The new explicit flag leaves `reflow` derivation untouched: `center`
        // is a block env but still reflows its prose body.
        assert!(db.environment("center").unwrap().reflow());
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
            assert!(env.block(), "{name} is a block env");
            assert!(env.reflow(), "{name} body reflows as prose");
            assert!(!env.code, "{name} is not a code env");
        }
        // `macrocode`/`macrocode*` are code-not-prose: real parsed code (not an
        // opaque verbatim blob), so `code` is set, `reflow` is off, and
        // `verbatim_body` stays off (otherwise the lexer would swallow the body).
        for name in ["macrocode", "macrocode*"] {
            let env = db.environment(name).unwrap_or_else(|| panic!("{name} env"));
            assert!(env.code, "{name} is code");
            assert!(!env.reflow(), "{name} never reflows");
            assert!(!env.verbatim_body, "{name} body is parsed, not verbatim");
            assert!(env.block(), "{name} is a block env");
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
        assert!(plain.reflow());
        let codeish = db.environment("codeish").unwrap();
        assert!(codeish.code);
        assert!(!codeish.reflow());
        assert!(!codeish.verbatim_body);
    }

    #[test]
    fn statement_body_flag_parses_and_drives_reflow() {
        // Like `code`, `statementBody` defaults false and suppresses reflow
        // without making the body verbatim — but it is a distinct flag, because
        // `code` is the `.dtx` "re-lexed under the package regime" fact and this
        // one is about layout only.
        let db = parse(
            r#"{ "environments": {
                "plain": {},
                "stmt": { "statementBody": true }
            } }"#,
        )
        .expect("valid statementBody schema");
        let plain = db.environment("plain").unwrap();
        assert!(!plain.statement_body);
        assert!(plain.reflow());
        let stmt = db.environment("stmt").unwrap();
        assert!(stmt.statement_body);
        assert!(!stmt.reflow());
        assert!(!stmt.code);
        assert!(!stmt.verbatim_body);
    }

    #[test]
    fn label_key_flag_is_curated_and_defaults_false() {
        let db = parse(
            r#"{
              "environments": {
                "plain": {},
                "labels": { "labelKey": true }
              }
            }"#,
        )
        .expect("valid labelKey schema");
        assert!(!db.environment("plain").unwrap().label_key);
        assert!(db.environment("labels").unwrap().label_key);

        assert!(builtin().environment("frame").unwrap().label_key);
        assert!(builtin().environment("lstlisting").unwrap().label_key);
        assert!(!builtin().environment("tikzpicture").unwrap().label_key);
    }

    /// The curated TikZ/pgf picture family: bodies of `;`-terminated path
    /// statements, which the formatter lays out one statement per authored line
    /// rather than filling as prose (issue #114).
    #[test]
    fn picture_environments_are_statement_bodies() {
        let db = builtin();
        for name in [
            "tikzpicture",
            "pgfpicture",
            "scope",
            "pgfonlayer",
            "axis",
            "loglogaxis",
            "semilogxaxis",
            "semilogyaxis",
            "groupplot",
            "polaraxis",
            "ternaryaxis",
        ] {
            let env = db.environment(name).unwrap_or_else(|| panic!("{name} env"));
            assert!(env.statement_body, "{name} holds statements, not prose");
            assert!(!env.reflow(), "{name} never reflows as prose");
            assert!(!env.code, "{name} is not `.dtx` macrocode");
            assert!(!env.verbatim_body, "{name} body is parsed, not verbatim");
            assert!(env.block(), "{name} is a block env");
        }
        // The curated tier masks the CWL entry wholesale, so the option bracket
        // these carry there has to be restated by hand or it is lost.
        for name in [
            "tikzpicture",
            "scope",
            "axis",
            "loglogaxis",
            "semilogxaxis",
            "semilogyaxis",
            "groupplot",
            "polaraxis",
            "ternaryaxis",
        ] {
            let env = db.environment(name).unwrap();
            assert_eq!(env.args.len(), 1, "{name} takes an option bracket");
            assert!(!env.args[0].required, "{name} option is optional");
            assert_eq!(env.args[0].content, ContentKind::Keyval, "{name} keyval");
        }
        // `\begin{pgfonlayer}{background}` names its layer; `pgfpicture` is bare.
        assert_eq!(db.environment("pgfonlayer").unwrap().args.len(), 1);
        assert!(db.environment("pgfonlayer").unwrap().args[0].required);
        assert!(db.environment("pgfpicture").unwrap().args.is_empty());
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
        // The converter guard: every CWL command/environment is names, arity, and the
        // mechanical `%keyvals` argument mark only, so none of its low-confidence
        // *behaviour* data can flip a formatter/lexer/outline decision.
        let db = cwl();
        for sig in db.command_sigs() {
            assert!(sig.sectioning.is_none());
            assert!(!sig.verbatim && !sig.rule && !sig.inline && !sig.block);
            // `Keyval` is the one content kind the tier may carry, and only on an
            // optional: `%keyvals` on a mandatory group is real but unconsumed, so
            // the converter drops it rather than record an unvalidated claim.
            assert!(sig.args.iter().all(|a| match a.content {
                ContentKind::Opaque => true,
                ContentKind::Keyval => !a.required,
                _ => false,
            }));
        }
        for sig in db.environment_sigs() {
            assert!(!sig.verbatim_body && !sig.math && !sig.code && !sig.align);
            assert!(!sig.no_indent && !sig.list && !sig.block());
            assert!(sig.outline.is_none());
            assert!(sig.args.iter().all(|a| match a.content {
                ContentKind::Opaque => true,
                ContentKind::Keyval => !a.required,
                _ => false,
            }));
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
        assert!(sig.sectioning.is_none() && !sig.inline && !sig.verbatim && !sig.block);
    }

    /// A minimal one-command DB for the origin-merge tests.
    fn db_with_command(name: &str) -> SignatureDb {
        let mut db = SignatureDb::default();
        db.insert_command(name, CommandSig::default());
        db
    }

    #[test]
    fn merge_with_package_origin_records_origin() {
        let mut scope = SignatureDb::default();
        scope.merge_from(&db_with_command("myfoo"), Some("mypkg"));
        assert_eq!(scope.command_origin("myfoo"), Some("mypkg"));
        assert!(scope.command("myfoo").is_some());
    }

    #[test]
    fn plain_merge_clears_origin_on_shadow() {
        // The document overlay: its scanned defs carry no origins, so merging
        // them last strips the package provenance of a shadowed name.
        let mut scope = SignatureDb::default();
        scope.merge_from(&db_with_command("dup"), Some("mypkg"));
        scope.merge_from(&db_with_command("dup"), None);
        assert_eq!(scope.command_origin("dup"), None);
        assert!(scope.command("dup").is_some());
    }

    #[test]
    fn later_package_merge_overwrites_origin() {
        let mut scope = SignatureDb::default();
        scope.merge_from(&db_with_command("shared"), Some("first"));
        scope.merge_from(&db_with_command("shared"), Some("second"));
        assert_eq!(scope.command_origin("shared"), Some("second"));
    }

    #[test]
    fn insert_clears_origin() {
        let mut scope = SignatureDb::default();
        scope.merge_from(&db_with_command("myfoo"), Some("mypkg"));
        scope.insert_command("myfoo", CommandSig::default());
        assert_eq!(scope.command_origin("myfoo"), None);
    }

    #[test]
    fn merge_propagates_existing_origins() {
        // Merging a scope that itself carries origins (a package's own scope
        // pulled a dependency) keeps them.
        let mut inner = SignatureDb::default();
        inner.merge_from(&db_with_command("dep"), Some("deppkg"));
        let mut scope = SignatureDb::default();
        scope.merge_from(&inner, None);
        assert_eq!(scope.command_origin("dep"), Some("deppkg"));
    }
}
