//! badness-parser — the lossless CST parser, semantic model, and
//! command-signature database behind [badness](https://badness.dev/), for
//! LaTeX (`.tex`, `.sty`/`.cls`, `.dtx`, `.ins`) and BibTeX (`.bib`).
//!
//! The parser treats input as generic TeX surface syntax and always produces a
//! lossless rowan tree: `reconstruct(text) == text`, byte for byte. Semantics
//! (arity, verbatim-ness, sectioning) are layered on top in [`semantic`],
//! never inside the grammar.
//!
//! The optional `schema` feature derives `schemars::JsonSchema` for the
//! project-declaration wire types used by configuration front ends.

#![deny(clippy::debug_assert_with_mut_call)]

macro_rules! impl_rowan_lang {
    ($language:ty, $kind:ty, $name:literal) => {
        const _: () = assert!(
            <$kind>::ROOT as u16 + 1 == <$kind>::__LAST as u16,
            "ROOT must be the final syntax kind"
        );

        impl From<$kind> for rowan::SyntaxKind {
            fn from(kind: $kind) -> Self {
                Self(kind as u16)
            }
        }

        impl rowan::Language for $language {
            type Kind = $kind;

            fn kind_from_raw(raw: rowan::SyntaxKind) -> $kind {
                assert!(
                    raw.0 <= <$kind>::ROOT as u16,
                    "invalid {} SyntaxKind discriminant: {}",
                    $name,
                    raw.0
                );
                // SAFETY: the kind is a contiguous `#[repr(u16)]` enum from zero
                // through `ROOT`, and the assertion bounds the raw value to it.
                unsafe { std::mem::transmute::<u16, $kind>(raw.0) }
            }

            fn kind_to_raw(kind: $kind) -> rowan::SyntaxKind {
                kind.into()
            }
        }
    };
}

pub mod ast;
pub mod bib;
pub mod declarations;
pub mod directives;
mod error;
pub mod parser;
pub mod semantic;
pub mod syntax;

pub use error::SyntaxError;

// Re-export rowan so embedders can name the exact tree types this crate is
// built against without pinning a matching rowan version themselves.
pub use rowan;
