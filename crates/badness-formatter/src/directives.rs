//! Internal-path shim: the comment-directive scanner lives in the parser crate,
//! below both of its consumers (this crate and the root crate's linter).

pub use badness_parser::directives::*;
