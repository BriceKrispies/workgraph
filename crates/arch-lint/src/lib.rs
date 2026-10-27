//! Purity tiers, the `Core` tier rules and the purity report for Workgraph.
//!
//! `purity.rs` and `core_tier.rs` are copied from
//! `C:\dev\noether\crates\arch-lint\src` and adapted (docs/rust-rewrite.md
//! section 4.6); noether's own architecture lints are not copied. The crate depends
//! only on `syn`, `proc-macro2` and `quote`, so the checks run without building the
//! product crates.
//!
//! * `lint-architecture` ([`lint_architecture`]) applies each crate's tier rules and
//!   the `Core` rules from `tools/reports/core-rules.toml`, fails on a crate the
//!   tier file does not list, fails on a top-level module of a module-tiered crate
//!   that has no `crate::module` line, and runs the shell logic ceiling.
//! * `purity-report` ([`purity_report_with`]) prints tiers and LOC, and with
//!   `--violations <crate>`, `--candidates [<crate>]` or `--gate` the detail views.
//!
//! Like `xtask` in noether, this is tooling and not product code.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

mod core_tier;
mod purity;
mod shell_io;

pub use purity::{gate as purity_gate, report as purity_report, report_with as purity_report_with};

/// Source-scanning architecture lints: every tier rule for every listed crate, the
/// `Core` rules, the unlisted-crate rule and the shell logic ceiling. Prints each
/// violation and returns an error summarising the count.
pub fn lint_architecture() -> Result<(), String> {
    println!("== lint-architecture");
    let root = workspace_root();
    let tier_file = purity::load_tier_file(&root)?;
    let rules = core_tier::load_rules(&root)?;
    let mut violations = Vec::new();
    purity::scan(&root, &tier_file, &rules, &mut violations);
    if let Err(message) = purity::gate(&root) {
        violations.push(message);
    }
    if violations.is_empty() {
        println!("  ok: no architecture violations");
        Ok(())
    } else {
        for violation in &violations {
            println!("  {violation}");
        }
        Err(format!("{} architecture violation(s)", violations.len()))
    }
}

/// The workspace root is the parent of `crates/`, derived from this file's
/// compile-time location so the tool works from any cwd.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// Every `.rs` file under `dir`, recursively. A missing directory gives none.
pub fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_rust_files(dir, &mut out);
    out.sort();
    out
}

fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, out);
        } else if path.extension() == Some(OsStr::new("rs")) {
            out.push(path);
        }
    }
}

/// Returns `(1-based line number, code-only line)` pairs, skipping blank lines and
/// lines that are entirely comments.
fn code_lines(file: &Path) -> Vec<(usize, String)> {
    let Ok(content) = fs::read_to_string(file) else {
        return Vec::new();
    };
    content
        .lines()
        .enumerate()
        .filter_map(|(i, raw)| {
            let trimmed = raw.trim_start();
            let is_comment = trimmed.starts_with("//") || trimmed.starts_with('*');
            if is_comment || trimmed.is_empty() {
                None
            } else {
                Some((i + 1, raw.to_string()))
            }
        })
        .collect()
}

/// `file` relative to `root`, with `/` separators.
pub fn rel(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .display()
        .to_string()
        .replace('\\', "/")
}

/// Returns `true` if `attrs` gate the item to test builds (`#[test]` or a `cfg`
/// mentioning `test`), so its body is exempt from the architecture lints.
pub fn is_test_gated(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("test")
            || matches!(&attr.meta, syn::Meta::List(list)
                if list.path.is_ident("cfg") && list.tokens.to_string().contains("test"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs(src: &str) -> Vec<syn::Attribute> {
        let item: syn::ItemFn = syn::parse_str(&format!("{src} fn f() {{}}")).unwrap();
        item.attrs
    }

    #[test]
    fn test_gating_sees_test_and_cfg_test() {
        assert!(is_test_gated(&attrs("#[test]")));
        assert!(is_test_gated(&attrs("#[cfg(test)]")));
        assert!(!is_test_gated(&attrs("#[inline]")));
        assert!(!is_test_gated(&attrs("#[cfg(unix)]")));
    }

    #[test]
    fn workspace_root_holds_the_crates_directory() {
        assert!(workspace_root().join("crates").is_dir());
    }

    #[test]
    fn rel_uses_forward_slashes() {
        let root = Path::new("root");
        assert_eq!(rel(root, &root.join("crates").join("x.rs")), "crates/x.rs");
    }

    #[test]
    fn rust_files_of_a_missing_directory_is_empty() {
        assert!(rust_files(Path::new("/nonexistent-workgraph-dir")).is_empty());
    }
}
