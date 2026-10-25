//! `arch-lint` binary: the architecture lints and the purity report.
//!
//! ```text
//! cargo run -q -p arch-lint -- lint-architecture
//! cargo run -q -p arch-lint -- purity-report
//! ```
//!
//! The `lint-arch` and `purity` cargo aliases in `.cargo/config.toml` wrap these
//! two commands. Adapted from noether's `arch-lint` binary.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map_or("help", String::as_str);

    let result = match cmd {
        "lint-architecture" => arch_lint::lint_architecture(),
        "purity-report" => arch_lint::purity_report_with(&args[1..]),
        "help" | "-h" | "--help" => {
            println!(
                "arch-lint\n\nCommands:\n  \
                 lint-architecture  tier rules, Core rules, unlisted crates, shell logic ceiling\n  \
                 purity-report     per-crate purity tier, LOC and violation counts\n  \
                 purity-report --violations <crate>\n  \
                 purity-report --candidates [<crate>]  shell crates' extraction worklist\n  \
                 purity-report --gate  shell logic ceiling check"
            );
            Ok(())
        }
        other => Err(format!(
            "unknown command {other:?}; run `cargo run -p arch-lint -- help`"
        )),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("arch-lint: {msg}");
            ExitCode::FAILURE
        }
    }
}
