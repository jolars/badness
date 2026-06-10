//! Label definitions and reference uses — the data of the per-file
//! label/reference model. Mirror of ravel's `semantic/binding.rs`: `Vec`-stored
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
    /// Range of the whole `\label{…}` command, for diagnostics / go-to-def.
    pub range: TextRange,
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
    /// Range of the enclosing command. **Known limitation:** keys split from a
    /// single `\cref{a,b,c}` share this command range; per-key sub-ranges are
    /// deferred until go-to-def needs them (Phase 7).
    pub range: TextRange,
    /// Set by the resolve pass when `name` matches a `\label` in *this* file.
    pub resolved: bool,
}
