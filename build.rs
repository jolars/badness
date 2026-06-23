//! Build script: gzip the bulk CWL signature data into `OUT_DIR`.
//!
//! `data/cwl_signatures.json` is the reviewable, generated source of truth (see
//! `scripts/gen_cwl_signatures.py`), but at ~400 KB it is too large to embed raw.
//! We compress it here so `src/semantic/signature.rs` can `include_bytes!` the
//! ~56 KB gzip artifact and decompress it once on first use. Keeping the
//! compression at build time means a single checked-in file, with no second copy
//! to drift out of sync.

use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use flate2::Compression;
use flate2::write::GzEncoder;

fn main() {
    println!("cargo:rerun-if-changed=data/cwl_signatures.json");
    println!("cargo:rerun-if-changed=build.rs");

    let json = std::fs::read("data/cwl_signatures.json")
        .expect("data/cwl_signatures.json must exist (run `task cwl:sync`)");

    let out = Path::new(&env::var("OUT_DIR").unwrap()).join("cwl_signatures.json.gz");
    let mut encoder = GzEncoder::new(
        BufWriter::new(File::create(&out).unwrap()),
        Compression::best(),
    );
    encoder.write_all(&json).unwrap();
    encoder.finish().unwrap();
}
