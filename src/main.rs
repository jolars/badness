//! Phase 0 CLI stub: round-trips its input through the CST and prints it back,
//! asserting losslessness. A real `clap`-based CLI (`badness fmt`, …) arrives in
//! Phase 2.

use std::io::Read;
use std::process::ExitCode;

fn main() -> ExitCode {
    let input = match std::env::args().nth(1) {
        Some(path) => match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) => {
                eprintln!("badness: cannot read {path}: {err}");
                return ExitCode::FAILURE;
            }
        },
        None => {
            let mut buf = String::new();
            if let Err(err) = std::io::stdin().read_to_string(&mut buf) {
                eprintln!("badness: cannot read stdin: {err}");
                return ExitCode::FAILURE;
            }
            buf
        }
    };

    let out = badness::parser::reconstruct(&input);
    if out != input {
        eprintln!("badness: internal error — losslessness invariant violated");
        return ExitCode::FAILURE;
    }
    print!("{out}");
    ExitCode::SUCCESS
}
