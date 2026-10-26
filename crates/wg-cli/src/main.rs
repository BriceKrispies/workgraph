//! The `workgraph` binary.

use std::process::ExitCode;

fn main() -> ExitCode {
    ExitCode::from(wg_cli::run())
}
