//! Label definitions and reference uses — the data of the per-file
//! label/reference model. Mirror of arity's `semantic/binding.rs`: `Vec`-stored
//! records addressed by newtype ids.

use rowan::TextRange;
use smol_str::SmolStr;

/// Index of a [`LabelDef`] in [`SemanticModel::labels`](super::SemanticModel).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LabelId(pub(crate) u32);

impl LabelId {
    pub(crate) fn from_index(i: usize) -> Self {
        Self(i as u32)
    }
}

/// Index of a [`LabelRef`] in [`SemanticModel::refs`](super::SemanticModel).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RefId(pub(crate) u32);

impl RefId {
    pub(crate) fn from_index(i: usize) -> Self {
        Self(i as u32)
    }
}

/// A `\label{key}` definition site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelDef {
    pub name: SmolStr,
    /// Range of the `\label{key}` command — the control word through its key
    /// group — for diagnostics / go-to-def. Excludes any *second* group the
    /// greedy parser may have over-attached (`\label`'s arity is unknown at parse
    /// time; see `builder::label_range`).
    pub range: TextRange,
    /// Range of just the key text inside the braces (`sec:intro` in
    /// `\label{sec:intro}`), trimmed of surrounding whitespace. The precise span a
    /// rename rewrites — narrower than [`range`](Self::range), which spans the whole
    /// command.
    pub key_range: TextRange,
    /// Set by the resolve pass when any reference in this file uses `name`.
    /// Per-file only — a label referenced solely from an `\input`-ed file looks
    /// unreferenced here until the (deferred) cross-file resolver lands.
    pub referenced: bool,
}

/// Which reference-family command produced a [`LabelRef`]. A small explicit
/// table (the analog of `project::IncludeKind`) kept distinct so later passes
/// can honor differences (`\cref` capitalization, `\nameref` text, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RefCommand {
    // Single-key (LaTeX / amsmath / hyperref).
    Ref,
    PageRef,
    EqRef,
    AutoRef,
    NameRef,
    // Comma-separated key list (cleveref / varioref).
    Cref,
    CrefUpper,
    Vref,
    VrefUpper,
    CpageRef,
}

impl RefCommand {
    /// Whether this command accepts a comma-separated key list (cleveref /
    /// varioref) rather than a single key.
    pub fn is_key_list(self) -> bool {
        matches!(
            self,
            RefCommand::Cref
                | RefCommand::CrefUpper
                | RefCommand::Vref
                | RefCommand::VrefUpper
                | RefCommand::CpageRef
        )
    }
}

/// A reference *use* site — one per key. A `\cref{a,b,c}` produces three
/// `LabelRef`s, each carrying the same command kind and command range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelRef {
    pub name: SmolStr,
    pub command: RefCommand,
    /// Range of the enclosing command, shared by all keys split from one
    /// `\cref{a,b,c}` — used for go-to-def / find-references navigation, which
    /// jumps to the whole command.
    pub range: TextRange,
    /// Range of just this key inside the braces (`b` in `\cref{a,b}`), trimmed of
    /// surrounding whitespace. Unlike [`range`](Self::range), this *is* per-key, so
    /// a rename rewrites exactly one key of a list command.
    pub key_range: TextRange,
    /// Set by the resolve pass when `name` matches a `\label` in *this* file.
    pub resolved: bool,
}

/// A citation *use* site — one per key. A `\cite{a,b}` produces two
/// `CitationRef`s. Citations are always cross-file: cite keys live in `.bib`
/// files, so there is no in-file resolution (no `resolved` flag); the
/// `undefined-citation` lint resolves them against the project's bibliography via
/// [`crate::project::citations::ResolvedCitations`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitationRef {
    /// The cite key, as authored.
    pub name: SmolStr,
    /// The citation command that introduced it, sans backslash (`cite`,
    /// `parencite`, `nocite`, …) — informational, for diagnostics.
    pub command: SmolStr,
    /// Range of the enclosing command (shared by keys split from one `\cite{a,b}`,
    /// like [`LabelRef::range`]).
    pub range: TextRange,
    /// Range of just this key inside the braces (`b` in `\cite{a,b}`), trimmed of
    /// surrounding whitespace — the precise span a rename rewrites.
    pub key_range: TextRange,
}
