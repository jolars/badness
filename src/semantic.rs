//! Compatibility wrapper over the semantic layer in the `badness-parser`
//! crate, plus the CLI-side concerns that do not belong in the published
//! parser: signature loading from disk and the salsa-backed package-scope
//! resolution ([`load`]).

pub use badness_parser::semantic::*;

pub mod load;

pub use load::{
    DiskPackageSource, PackageSource, collect_package_signatures, disk_scope_signatures,
};
