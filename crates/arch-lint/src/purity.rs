//! Purity tiers for `lint-architecture` and the `purity-report` metric.
//!
//! Copied from `C:\dev\noether\crates\arch-lint\src\purity.rs` and adapted for
//! Workgraph (docs/rust-rewrite.md section 4.6). Differences from the source: the
//! `Legacy` marker and the three impure-LOC ceilings are gone; the `Core` rules take
//! a [`CoreRules`] value read from `tools/reports/core-rules.toml`; the baseline file
//! holds the shell logic ceiling (candidate LOC over `Shell` crates); `purity-report`
//! fails on an unlisted crate.
//!
//! Every crate has a tier line. Crates not listed in [`TierTable`] are `Impure`, and
//! the report and `lint-architecture` fail unless the tier file lists them as `Shell`
//! or `Tooling`.
//!
//! * `Pure` (semantic purity) rules apply to `Pure`, `StrictPure` and `Core` crates.
//! * `StrictPure` (syntactic purity) adds the strict-only rules, which apply to
//!   `StrictPure` and `Core` crates.
//! * `Core` is every `StrictPure` rule plus the structural rules in
//!   [`crate::core_tier`], checked by [`scan`] on top.
//!
//! Test-gated code (`#[test]`, `#[cfg(test)]`) is exempt, as in the other scans.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use quote::ToTokens;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};

use crate::core_tier::{self, CoreRules};
use crate::shell_io::{ShellNames, defined_types, does_shell_io};
use crate::{code_lines, is_test_gated, rel, rust_files, workspace_root};

/// Purity tier of a crate. Ordered from least to most strict, so `tier >=
/// StrictPure` means the strict-only rules apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PurityTier {
    Impure,
    Pure,
    StrictPure,
    Core,
}

/// Crate name to tier, as parsed from [`TIERS_PATH`]. A crate absent from the
/// table is `Impure`.
pub(crate) type TierTable = BTreeMap<String, PurityTier>;

/// Repo-relative tier file: one `crate-name Tier` pair per line.
const TIERS_PATH: &str = "tools/reports/purity-tiers.txt";

/// Tier of `krate` in `tiers`; unlisted crates are `Impure`.
pub(crate) fn purity_tier(tiers: &TierTable, krate: &str) -> PurityTier {
    tiers.get(krate).copied().unwrap_or(PurityTier::Impure)
}

/// Crates that are build and lint tooling rather than product code. A tiers-file
/// `Tooling` line adds more.
pub(crate) const BUILTIN_TOOLING: &[&str] = &["arch-lint"];

/// Parsed tier file: the checked tiers, the crates marked `Tooling` and the crates
/// marked `Shell`, plus the module lines. A `Tooling` or `Shell` crate is `Impure`
/// for the gate; the markers split the report and select the crates the shell logic
/// ceiling covers.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct TierFile {
    pub(crate) tiers: TierTable,
    pub(crate) tooling: BTreeSet<String>,
    /// Impure shell crates (the I/O edge): listed, and covered by the shell logic
    /// ceiling.
    pub(crate) shell: BTreeSet<String>,
    /// `crate::module` lines: for each module-tiered crate, the mark of each of its
    /// top-level modules. A module mark is a checked tier or `Shell`. A crate with
    /// an entry here has its root files (`lib.rs`, `main.rs`, `bin/`) on the crate
    /// line's tier, and each listed module on its own line's tier.
    pub(crate) modules: BTreeMap<String, BTreeMap<String, Mark>>,
}

/// True if `krate` is tooling: a built-in tooling crate or a `Tooling` line.
pub(crate) fn is_tooling(file: &TierFile, krate: &str) -> bool {
    BUILTIN_TOOLING.contains(&krate) || file.tooling.contains(krate)
}

/// The mark of a crate line: a checked tier, `Tooling`, `Shell`, or `None` for a
/// crate the tier file does not name.
pub(crate) fn crate_mark(file: &TierFile, krate: &str) -> Option<Mark> {
    if let Some(tier) = file.tiers.get(krate) {
        Some(Mark::Tier(*tier))
    } else if is_tooling(file, krate) {
        Some(Mark::Tooling)
    } else if file.shell.contains(krate) {
        Some(Mark::Shell)
    } else {
        None
    }
}

/// The mark that governs one source file of `krate`: its top-level module's
/// `crate::module` line when it has one, else the crate line. `module` is `None`
/// for the crate's root files.
pub(crate) fn file_mark(file: &TierFile, krate: &str, module: Option<&str>) -> Option<Mark> {
    let crate_level = crate_mark(file, krate)?;
    let module_level = module.and_then(|name| file.modules.get(krate)?.get(name));
    Some(module_level.copied().unwrap_or(crate_level))
}

/// The top-level module a source file belongs to, from its path under the crate's
/// `src` directory: `src/tasks.rs` and `src/tasks/x.rs` are both `tasks`. The root
/// files `lib.rs` and `main.rs`, and anything under `src/bin/`, are `None`.
pub(crate) fn module_of(src_dir: &Path, file: &Path) -> Option<String> {
    let relative = file.strip_prefix(src_dir).ok()?;
    let parts: Vec<&OsStr> = relative.iter().collect();
    match parts.as_slice() {
        [] => None,
        [single] => {
            let stem = single.to_str()?.strip_suffix(".rs")?;
            (!matches!(stem, "lib" | "main")).then(|| stem.to_string())
        }
        [dir, ..] if *dir == "bin" => None,
        [dir, ..] => dir.to_str().map(str::to_string),
    }
}

/// Reads and parses [`TIERS_PATH`] under `root`. A missing file is an error,
/// not an empty table.
pub(crate) fn load_tier_file(root: &Path) -> Result<TierFile, String> {
    let path = root.join(TIERS_PATH);
    let text =
        fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse_tier_file(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The checked tiers from [`TIERS_PATH`] under `root`.
pub(crate) fn load_tiers(root: &Path) -> Result<TierTable, String> {
    load_tier_file(root).map(|file| file.tiers)
}

/// One tier-file line's meaning: a checked tier, or the `Tooling` or `Shell`
/// marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mark {
    Tier(PurityTier),
    Tooling,
    Shell,
}

/// Parses the tier file text. Blank lines and `#` comments (whole-line or
/// trailing) are skipped. Errors name the 1-based line number. A crate may
/// appear once, as a tier, `Tooling` or `Shell`. A `crate::module` line tiers one
/// top-level module of that crate; it takes a checked tier or `Shell`, and its
/// crate must have a crate line.
pub(crate) fn parse_tier_file(text: &str) -> Result<TierFile, String> {
    let entries = text
        .lines()
        .enumerate()
        .map(|(index, raw)| (index + 1, raw.find('#').map_or(raw, |at| &raw[..at]).trim()))
        .filter(|(_, content)| !content.is_empty())
        .map(|(line_no, content)| parse_entry(line_no, content))
        .collect::<Result<Vec<(usize, &str, Mark)>, String>>()?;
    for (at, (line_no, key, _)) in entries.iter().enumerate() {
        if let Some((first, _, _)) = entries[..at].iter().find(|(_, earlier, _)| earlier == key) {
            return Err(format!(
                "line {line_no}: duplicate crate `{key}` (first listed on line {first})"
            ));
        }
    }
    let mut file = TierFile::default();
    for (_, key, mark) in &entries {
        if key.contains("::") {
            continue;
        }
        match mark {
            Mark::Tier(tier) => {
                file.tiers.insert((*key).to_string(), *tier);
            }
            Mark::Tooling => {
                file.tooling.insert((*key).to_string());
            }
            Mark::Shell => {
                file.shell.insert((*key).to_string());
            }
        }
    }
    for (line_no, key, mark) in &entries {
        let Some((krate, module)) = key.split_once("::") else {
            continue;
        };
        let listed = file.tiers.contains_key(krate)
            || file.tooling.contains(krate)
            || file.shell.contains(krate);
        if !listed {
            return Err(format!(
                "line {line_no}: module line `{key}` names crate `{krate}`, which has no crate line"
            ));
        }
        file.modules
            .entry(krate.to_string())
            .or_default()
            .insert(module.to_string(), *mark);
    }
    Ok(file)
}

/// True for a crate or module name: ASCII letters, digits, `_`, and (crates only)
/// `-`.
fn is_name(text: &str, allow_hyphen: bool) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || (allow_hyphen && c == '-'))
}

/// Parses one non-blank, comment-free line into `(line number, key, mark)`. The key
/// is the crate name, or `crate::module` for a module line.
fn parse_entry(line_no: usize, content: &str) -> Result<(usize, &str, Mark), String> {
    let fields: Vec<&str> = content.split_whitespace().collect();
    let &[key, tier_name] = fields.as_slice() else {
        return Err(format!(
            "line {line_no}: expected `crate-name Tier`, found `{content}`"
        ));
    };
    let (krate, module) = match key.split_once("::") {
        Some((krate, module)) => (krate, Some(module)),
        None => (key, None),
    };
    if !is_name(krate, true) {
        return Err(format!("line {line_no}: malformed crate name `{krate}`"));
    }
    if let Some(module) = module.filter(|module| !is_name(module, false)) {
        return Err(format!(
            "line {line_no}: malformed module name `{module}` in `{key}`"
        ));
    }
    let mark = match tier_name {
        "Pure" => Mark::Tier(PurityTier::Pure),
        "StrictPure" => Mark::Tier(PurityTier::StrictPure),
        "Core" => Mark::Tier(PurityTier::Core),
        "Tooling" => Mark::Tooling,
        "Shell" => Mark::Shell,
        other => {
            return Err(format!(
                "line {line_no}: unknown tier `{other}` (expected `Pure`, `StrictPure`, `Core`, `Tooling` or `Shell`)"
            ));
        }
    };
    if module.is_some() && mark == Mark::Tooling {
        return Err(format!(
            "line {line_no}: module line `{key}` takes `Pure`, `StrictPure`, `Core` or `Shell`, not `Tooling`"
        ));
    }
    Ok((line_no, key, mark))
}

/// Repo-relative baseline file for the shell logic ceiling.
const BASELINE_PATH: &str = "tools/reports/purity-baseline.txt";

/// The shell logic ceiling: non-test LOC of the pure candidates (functions that pass
/// the `Pure` rules, and those that also pass the `StrictPure` rules) summed over the
/// `Shell` crates. The baseline file holds the first measured value; the figures may
/// not exceed it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ceilings {
    pub(crate) pure: u64,
    pub(crate) strict: u64,
}

/// Parses the baseline text: one `figure value` pair per line for
/// `shell-pure-candidate-loc` and `shell-strict-candidate-loc`, each exactly once.
/// `#` comments and blank lines are allowed. Errors name the 1-based line number.
pub(crate) fn parse_baseline(text: &str) -> Result<Ceilings, String> {
    let (mut pure, mut strict) = (None, None);
    for (index, raw) in text.lines().enumerate() {
        let line_no = index + 1;
        let content = raw.find('#').map_or(raw, |at| &raw[..at]).trim();
        if content.is_empty() {
            continue;
        }
        let fields: Vec<&str> = content.split_whitespace().collect();
        let &[name, value] = fields.as_slice() else {
            return Err(format!(
                "line {line_no}: expected `figure value`, found `{content}`"
            ));
        };
        let slot = match name {
            "shell-pure-candidate-loc" => &mut pure,
            "shell-strict-candidate-loc" => &mut strict,
            other => {
                return Err(format!(
                    "line {line_no}: unknown figure `{other}` (expected `shell-pure-candidate-loc` or `shell-strict-candidate-loc`)"
                ));
            }
        };
        if slot.is_some() {
            return Err(format!("line {line_no}: duplicate figure `{name}`"));
        }
        let number = value
            .parse::<u64>()
            .map_err(|e| format!("line {line_no}: `{value}` is not a number: {e}"))?;
        *slot = Some(number);
    }
    let missing = |name: &str| format!("no `{name}` figure");
    Ok(Ceilings {
        pure: pure.ok_or_else(|| missing("shell-pure-candidate-loc"))?,
        strict: strict.ok_or_else(|| missing("shell-strict-candidate-loc"))?,
    })
}

/// Checks each measured figure against its ceiling; every figure over its ceiling
/// is named in the error.
pub(crate) fn check_ceilings(measured: Ceilings, ceilings: Ceilings) -> Result<(), String> {
    let over: Vec<String> = [
        ("shell-pure-candidate-loc", measured.pure, ceilings.pure),
        (
            "shell-strict-candidate-loc",
            measured.strict,
            ceilings.strict,
        ),
    ]
    .into_iter()
    .filter(|(_, measured, ceiling)| measured > ceiling)
    .map(|(name, measured, ceiling)| format!("{name} {measured} exceeds its ceiling {ceiling}"))
    .collect();
    if over.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} ({BASELINE_PATH}); logic that could be pure must move into a Pure or Core crate, or the baseline is lowered or raised only by a reviewed edit",
            over.join("; ")
        ))
    }
}

/// Interior-mutability type names (`Pure` rule).
const INTERIOR_MUT: &[&str] = &[
    "Cell",
    "RefCell",
    "UnsafeCell",
    "OnceCell",
    "ArcSwap",
    "ArcSwapOption",
    "Mutex",
    "RwLock",
];

/// `std` / `core` modules whose use is forbidden in `Pure` crates (`Pure` rule).
const BANNED_STD_MODULES: &[&str] = &["time", "env", "fs", "net", "process", "thread"];

/// One finding. `strict` marks a `StrictPure`-only rule.
#[derive(Debug, Clone)]
struct Violation {
    strict: bool,
    line: usize,
    text: String,
}

/// Lint pass for `lint-architecture`: reports every violation of the rules that
/// apply to each non-`Impure` crate at its tier, every `Core` structural
/// violation, and every crate under `crates/` that the tier file does not list.
pub(crate) fn scan(root: &Path, file: &TierFile, rules: &CoreRules, violations: &mut Vec<String>) {
    let tiers = &file.tiers;
    let names = crate_names(root);
    violations.extend(unlisted_crate_violations(&names, file));
    let mut_traits = mut_trait_names(root);
    for name in names {
        violations.extend(unlisted_module_violations(root, &name, file));
        let src_dir = root.join("crates").join(&name).join("src");
        let manifest_path = root.join("crates").join(&name).join("Cargo.toml");
        // Each file is checked at the tier its module (or the crate) gives it.
        let mut core_files = Vec::new();
        for src in rust_files(&src_dir) {
            let module = module_of(&src_dir, &src);
            let Some(Mark::Tier(tier)) = file_mark(file, &name, module.as_deref()) else {
                continue;
            };
            let path = rel(root, &src);
            for v in file_violations(&src, &mut_traits) {
                if v.strict && tier < PurityTier::StrictPure {
                    continue;
                }
                violations.push(format!(
                    "{path}:{}: {} purity violation: {}",
                    v.line,
                    if v.strict { "StrictPure" } else { "Pure" },
                    v.text
                ));
            }
            if tier == PurityTier::Core {
                core_files.push(src);
            }
        }
        let parsed: Vec<(String, syn::File)> = core_files
            .iter()
            .filter_map(|src| Some((rel(root, src), parse(src)?)))
            .collect();
        if file.modules.contains_key(&name) {
            // A module-tiered crate: the dependency allowlist is applied per path in
            // each Core file, not to the whole manifest.
            let manifest = fs::read_to_string(&manifest_path).unwrap_or_default();
            for (path, ast) in &parsed {
                violations.extend(core_tier::check_module_reach(
                    &name, path, ast, &manifest, file, rules,
                ));
            }
            violations.extend(core_tier::check_crate(&name, &parsed, "", tiers, rules));
        } else if purity_tier(tiers, &name) == PurityTier::Core {
            let manifest = fs::read_to_string(&manifest_path).unwrap_or_default();
            violations.extend(core_tier::check_crate(
                &name, &parsed, &manifest, tiers, rules,
            ));
        }
    }
}

/// For a module-tiered crate: each top-level module the crate's source declares or
/// has a file for must have a `crate::module` line, and each such line must name a
/// module that exists. Test-only modules (`#[cfg(test)] mod x;`) are not modules of
/// the product and need no line. A crate without module lines is not checked here.
pub(crate) fn unlisted_module_violations(root: &Path, krate: &str, file: &TierFile) -> Vec<String> {
    let Some(listed) = file.modules.get(krate) else {
        return Vec::new();
    };
    let src_dir = root.join("crates").join(krate).join("src");
    let (declared_live, declared_test) = root_declarations(&src_dir);
    let mut present = declared_live.clone();
    for src in rust_files(&src_dir) {
        if let Some(module) = module_of(&src_dir, &src)
            && !declared_test.contains(&module)
        {
            present.insert(module);
        }
    }
    let known: BTreeSet<String> = present.iter().chain(&declared_test).cloned().collect();
    let unlisted = present
        .iter()
        .filter(|module| !listed.contains_key(module.as_str()))
        .map(|module| {
            format!("{TIERS_PATH}: module `{krate}::{module}` is not listed; add a `{krate}::{module} Tier` line")
        });
    let stale = listed
        .keys()
        .filter(|module| !known.contains(module.as_str()))
        .map(|module| {
            format!("{TIERS_PATH}: module `{krate}::{module}` is listed but crate `{krate}` has no such module")
        });
    unlisted.chain(stale).collect()
}

/// The module names a crate's root files declare: `(live, test-only)`. A module
/// declared only under `cfg(test)` is test code.
fn root_declarations(src_dir: &Path) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut live = BTreeSet::new();
    let mut test_only = BTreeSet::new();
    for root_file in ["lib.rs", "main.rs"] {
        let Some(ast) = parse(&src_dir.join(root_file)) else {
            continue;
        };
        for item in &ast.items {
            if let syn::Item::Mod(module) = item {
                let name = module.ident.to_string();
                if is_test_gated(&module.attrs) {
                    test_only.insert(name);
                } else {
                    live.insert(name);
                }
            }
        }
    }
    (live, test_only)
}

/// Every crate directory must be named in the tier file (as a tier, `Tooling` or
/// `Shell` line) or be a built-in tooling crate. An unlisted crate would
/// otherwise default to `Impure` and skip every rule silently.
pub(crate) fn unlisted_crate_violations(names: &[String], file: &TierFile) -> Vec<String> {
    names
        .iter()
        .filter(|name| {
            !BUILTIN_TOOLING.contains(&name.as_str())
                && !file.tiers.contains_key(name.as_str())
                && !file.tooling.contains(name.as_str())
                && !file.shell.contains(name.as_str())
        })
        .map(|name| {
            format!(
                "{TIERS_PATH}: crate `{name}` is not listed; add a tier, `Tooling` or `Shell` line"
            )
        })
        .collect()
}

/// The label printed in the tier column: the checked tier, `Shell`, `Tooling`, or
/// `unlisted` for a crate the tier file does not name.
fn tier_label(file: &TierFile, name: &str) -> &'static str {
    mark_label(crate_mark(file, name))
}

/// The label of a mark, as the report prints it.
fn mark_label(mark: Option<Mark>) -> &'static str {
    match mark {
        Some(Mark::Tier(PurityTier::Core)) => "Core",
        Some(Mark::Tier(PurityTier::StrictPure)) => "StrictPure",
        Some(Mark::Tier(PurityTier::Pure)) => "Pure",
        Some(Mark::Tooling) => "Tooling",
        Some(Mark::Shell) => "Shell",
        Some(Mark::Tier(PurityTier::Impure)) | None => "unlisted",
    }
}

/// True if a crate has any `Shell` source: its crate line is `Shell`, or one of its
/// module lines is.
fn has_shell_source(file: &TierFile, krate: &str) -> bool {
    file.shell.contains(krate)
        || file
            .modules
            .get(krate)
            .is_some_and(|modules| modules.values().any(|mark| *mark == Mark::Shell))
}

/// `purity-report` with no flags: per-crate tier, non-test LOC and violation
/// counts, LOC per tier, totals, and the shell logic ceiling. Fails if a crate
/// under `crates/` has no tier line.
pub fn report() -> Result<(), String> {
    println!("== purity-report");
    let root = workspace_root();
    let tier_file = load_tier_file(&root)?;
    let mut_traits = mut_trait_names(&root);
    println!(
        "{:<22} {:<11} {:>8} {:>10} {:>10}",
        "crate", "tier", "loc", "pure-viol", "strict-viol"
    );
    let mut total_loc = 0;
    let mut total_pure = 0;
    let mut total_strict = 0;
    let mut by_tier: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut unlisted = Vec::new();
    let mut module_problems = Vec::new();
    for name in crate_names(&root) {
        let label = tier_label(&tier_file, &name);
        let src_dir = root.join("crates").join(&name).join("src");
        let mut loc = 0;
        let mut pure = 0;
        let mut strict = 0;
        for file in rust_files(&src_dir) {
            let file_loc = non_test_loc(&file);
            loc += file_loc;
            // A file's LOC counts under its own tier, which a module line can set apart
            // from the crate line.
            let file_label = mark_label(file_mark(
                &tier_file,
                &name,
                module_of(&src_dir, &file).as_deref(),
            ));
            *by_tier.entry(file_label).or_default() += file_loc;
            for v in file_violations(&file, &mut_traits) {
                if v.strict {
                    strict += 1;
                } else {
                    pure += 1;
                }
            }
        }
        println!("{name:<22} {label:<11} {loc:>8} {pure:>10} {strict:>10}");
        total_loc += loc;
        total_pure += pure;
        total_strict += strict;
        if label == "unlisted" {
            unlisted.push(name.clone());
        }
        module_problems.extend(unlisted_module_violations(&root, &name, &tier_file));
    }
    println!(
        "totals: loc {total_loc}, pure-violations {total_pure}, strict-violations {total_strict}"
    );
    for label in ["Core", "StrictPure", "Pure", "Shell", "Tooling", "unlisted"] {
        println!(
            "{label} LOC: {}",
            by_tier.get(label).copied().unwrap_or_default()
        );
    }
    let measured = shell_candidate_loc(&root, &tier_file, &mut_traits);
    println!(
        "shell logic: pure candidate LOC {}, strict candidate LOC {}",
        measured.pure, measured.strict
    );
    match read_baseline(&root) {
        Ok(c) => println!("baseline:    pure {}, strict {}", c.pure, c.strict),
        Err(e) => println!("baseline:    {e}"),
    }
    let mut problems = Vec::new();
    if !unlisted.is_empty() {
        problems.push(format!(
            "{TIERS_PATH}: crate{} not listed: {}; add a tier, `Tooling` or `Shell` line",
            if unlisted.len() == 1 { " is" } else { "s are" },
            unlisted
                .iter()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    problems.extend(module_problems);
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("; "))
    }
}

// --- `purity-report` flags: --violations and --candidates ------------------

/// What `purity-report` prints for a given argument list.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReportMode {
    /// No flag: the per-crate summary.
    Summary,
    /// `--violations <crate>`: every Pure and StrictPure finding in the crate.
    Violations(String),
    /// `--candidates [<crate>]`: extraction candidates in one crate, or in every
    /// `Shell` crate when no crate is named.
    Candidates(Option<String>),
    /// `--gate`: the shell logic ceiling check.
    Gate,
}

/// Parses the arguments that follow `purity-report`.
pub(crate) fn parse_report_args(args: &[String]) -> Result<ReportMode, String> {
    let parts: Vec<&str> = args.iter().map(String::as_str).collect();
    match parts.as_slice() {
        [] => Ok(ReportMode::Summary),
        ["--violations", krate] => Ok(ReportMode::Violations((*krate).to_string())),
        ["--violations"] => Err("`--violations` needs a crate name".to_string()),
        ["--gate"] => Ok(ReportMode::Gate),
        ["--candidates"] => Ok(ReportMode::Candidates(None)),
        ["--candidates", krate] => Ok(ReportMode::Candidates(Some((*krate).to_string()))),
        [flag @ ("--violations" | "--candidates"), ..] => {
            Err(format!("`{flag}` takes at most one crate name"))
        }
        [other, ..] => Err(format!(
            "unknown option `{other}`; expected `--violations <crate>`, `--candidates [<crate>]` or `--gate`"
        )),
    }
}

/// `purity-report` with its flags (see `parse_report_args`).
pub fn report_with(args: &[String]) -> Result<(), String> {
    match parse_report_args(args)? {
        ReportMode::Summary => report(),
        ReportMode::Violations(krate) => violations_report(&krate),
        ReportMode::Candidates(krate) => candidates_report(krate.as_deref()),
        ReportMode::Gate => gate(&workspace_root()),
    }
}

fn require_crate(root: &Path, krate: &str) -> Result<(), String> {
    if crate_names(root).iter().any(|name| name.as_str() == krate) {
        Ok(())
    } else {
        Err(format!("unknown crate `{krate}`"))
    }
}

/// `--violations <crate>`: each Pure and StrictPure finding with file:line and
/// the rule broken, whatever the crate's tier.
fn violations_report(krate: &str) -> Result<(), String> {
    let root = workspace_root();
    require_crate(&root, krate)?;
    let tier = purity_tier(&load_tiers(&root)?, krate);
    let mut_traits = mut_trait_names(&root);
    let mut rows: Vec<(String, usize, bool, String)> = Vec::new();
    for file in rust_files(&root.join("crates").join(krate).join("src")) {
        let path = rel(&root, &file);
        rows.extend(
            file_violations(&file, &mut_traits)
                .into_iter()
                .map(|v| (path.clone(), v.line, v.strict, v.text)),
        );
    }
    rows.sort();
    println!("== purity-violations: {krate} ({tier:?})");
    for (path, line, strict, text) in &rows {
        let rule = if *strict { "StrictPure" } else { "Pure" };
        println!("{path}:{line}: {rule}: {text}");
    }
    let strict = rows.iter().filter(|row| row.2).count();
    println!("total: {} Pure, {strict} StrictPure", rows.len() - strict);
    Ok(())
}

/// Per-function facts the candidate selection needs. `pure_ok` / `strict_ok`
/// are the function's own findings (signature and body) against the Pure and
/// StrictPure rules; `callees` are the names it calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FnFacts {
    name: String,
    pure_ok: bool,
    strict_ok: bool,
    callees: BTreeSet<String>,
    /// The function does I/O through a shell type (see `shell_io`).
    io: bool,
    /// The subset of `callees` called as `f(..)`, not as `.m(..)`; shell I/O spreads to a
    /// caller through these, followed transitively.
    path_callees: BTreeSet<String>,
}

/// Candidate flags `(pure, strict)` for each function, in input order. A
/// function is a candidate only if its own findings are clean and every
/// same-crate callee (matched by name, all same-named definitions) is clean at
/// the same tier. Callees are checked one level deep, not transitively.
pub(crate) fn candidate_flags(facts: &[FnFacts]) -> Vec<(bool, bool)> {
    let mut defs: BTreeMap<&str, Vec<&FnFacts>> = BTreeMap::new();
    for fact in facts {
        defs.entry(fact.name.as_str()).or_default().push(fact);
    }
    // Names that do shell I/O, directly or through a chain of `f(..)` calls: a thin wrapper
    // over an I/O helper is I/O glue, however clean its own body looks.
    let mut io_names: BTreeSet<&str> = facts
        .iter()
        .filter(|f| f.io)
        .map(|f| f.name.as_str())
        .collect();
    loop {
        let before = io_names.len();
        let spread: Vec<&str> = facts
            .iter()
            .filter(|f| f.path_callees.iter().any(|c| io_names.contains(c.as_str())))
            .map(|f| f.name.as_str())
            .collect();
        io_names.extend(spread);
        if io_names.len() == before {
            break;
        }
    }
    facts
        .iter()
        .map(|fact| {
            if io_names.contains(fact.name.as_str()) {
                return (false, false);
            }
            let callees_pass = |ok: fn(&FnFacts) -> bool| {
                fact.callees
                    .iter()
                    .all(|callee| match defs.get(callee.as_str()) {
                        None => true,
                        Some(same_named) => same_named.iter().all(|def| ok(def)),
                    })
            };
            (
                fact.pure_ok && callees_pass(|f| f.pure_ok),
                fact.strict_ok && callees_pass(|f| f.strict_ok),
            )
        })
        .collect()
}

/// One function found in a crate, with its facts and its non-test LOC.
#[derive(Debug)]
struct FnSite {
    facts: FnFacts,
    line: usize,
    loc: usize,
}

/// A listed candidate.
struct Candidate {
    path: String,
    line: usize,
    loc: usize,
    name: String,
    strict: bool,
    /// The function's file is `Shell` (the shell logic ceiling counts it).
    shell: bool,
}

/// Every free function and impl method in one crate, with its facts.
fn crate_candidates(
    root: &Path,
    krate: &str,
    mut_traits: &BTreeSet<String>,
    tier_file: &TierFile,
    shell_names: &ShellNames,
) -> Vec<Candidate> {
    let src_dir = root.join("crates").join(krate).join("src");
    let mut sites: Vec<(String, bool, FnSite)> = Vec::new();
    for file in rust_files(&src_dir) {
        let path = rel(root, &file);
        let shell =
            file_mark(tier_file, krate, module_of(&src_dir, &file).as_deref()) == Some(Mark::Shell);
        sites.extend(
            fn_sites(&file, mut_traits, shell_names)
                .into_iter()
                .map(|site| (path.clone(), shell, site)),
        );
    }
    let facts: Vec<FnFacts> = sites
        .iter()
        .map(|(_, _, site)| site.facts.clone())
        .collect();
    let mut found: Vec<Candidate> = sites
        .into_iter()
        .zip(candidate_flags(&facts))
        .filter_map(|((path, shell, site), (pure, strict))| {
            pure.then_some(Candidate {
                path,
                line: site.line,
                loc: site.loc,
                name: site.facts.name,
                strict,
                shell,
            })
        })
        .collect();
    found.sort_by(|a, b| {
        b.loc
            .cmp(&a.loc)
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.line.cmp(&b.line))
    });
    found
}

fn fn_sites(file: &Path, mut_traits: &BTreeSet<String>, shell: &ShellNames) -> Vec<FnSite> {
    let Some(ast) = parse(file) else {
        return Vec::new();
    };
    let code: Vec<usize> = code_lines(file).into_iter().map(|(line, _)| line).collect();
    sites_from_ast(&ast, &code, mut_traits, shell)
}

/// Collects the functions of one parsed file. `code` lists the code line
/// numbers, so a function's LOC is the code lines inside its span.
fn sites_from_ast(
    ast: &syn::File,
    code: &[usize],
    mut_traits: &BTreeSet<String>,
    shell: &ShellNames,
) -> Vec<FnSite> {
    if is_test_gated(&ast.attrs) {
        return Vec::new();
    }
    let uses = UseMap::of_file(ast);
    let mut collector = FnCollector {
        mut_traits,
        code,
        uses: &uses,
        shell,
        sites: Vec::new(),
    };
    collector.visit_file(ast);
    collector.sites
}

struct FnCollector<'a> {
    mut_traits: &'a BTreeSet<String>,
    code: &'a [usize],
    uses: &'a UseMap,
    shell: &'a ShellNames,
    sites: Vec<FnSite>,
}

impl FnCollector<'_> {
    fn record(&mut self, sig: &syn::Signature, block: &syn::Block, span: (usize, usize)) {
        let mut scan = Scan {
            mut_traits: self.mut_traits,
            found: Vec::new(),
            let_depth: 0,
            trait_impl: false,
            uses: self.uses,
        };
        if sig_has_mut_ref(sig) {
            scan.pure(span_lines(sig).0, "`&mut` in the signature of a function");
        }
        scan.visit_signature(sig);
        scan.visit_block(block);
        let mut calls = CallCollector {
            uses: self.uses,
            names: BTreeSet::new(),
            paths: BTreeSet::new(),
        };
        calls.visit_block(block);
        let found = scan.found;
        // I/O through a method of a shell type is I/O, whatever the signature says.
        let io = does_shell_io(sig, block, self.shell, self.uses);
        self.sites.push(FnSite {
            facts: FnFacts {
                name: sig.ident.to_string(),
                pure_ok: !io && found.iter().all(|v| v.strict),
                strict_ok: !io && found.is_empty(),
                callees: calls.names,
                io,
                path_callees: calls.paths,
            },
            line: span.0,
            loc: self
                .code
                .iter()
                .filter(|line| (span.0..=span.1).contains(*line))
                .count(),
        });
    }
}

impl<'ast> Visit<'ast> for FnCollector<'_> {
    fn visit_item(&mut self, node: &'ast syn::Item) {
        if !is_test_gated(item_attrs(node)) {
            visit::visit_item(self, node);
        }
    }

    fn visit_impl_item(&mut self, node: &'ast syn::ImplItem) {
        if !is_test_gated(impl_item_attrs(node)) {
            visit::visit_impl_item(self, node);
        }
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.record(&node.sig, &node.block, span_lines(node));
        visit::visit_item_fn(self, node);
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.record(&node.sig, &node.block, span_lines(node));
        visit::visit_impl_item_fn(self, node);
    }
}

/// Names of the functions a body calls: `f(..)` by its last path segment and
/// `.m(..)` by its method name. An imported name is also recorded under what it imports
/// (`use exec::default_branch as git_default_branch` records both).
struct CallCollector<'a> {
    uses: &'a UseMap,
    names: BTreeSet<String>,
    /// The names called as `f(..)`.
    paths: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for CallCollector<'_> {
    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = &*node.func
            && let Some(segment) = path.path.segments.last()
        {
            self.paths.insert(segment.ident.to_string());
            self.names.insert(segment.ident.to_string());
            let segs: Vec<String> = path
                .path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect();
            for full in self.uses.resolve(&segs) {
                self.paths.extend(full.last().cloned());
                self.names.extend(full.last().cloned());
            }
        }
        visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        self.names.insert(node.method.to_string());
        visit::visit_expr_method_call(self, node);
    }
}

/// `--candidates [<crate>]`: functions whose own signature and body pass the
/// Pure rules (and, separately, the StrictPure rules), with non-test LOC.
fn candidates_report(only: Option<&str>) -> Result<(), String> {
    let root = workspace_root();
    let tier_file = load_tier_file(&root)?;
    let mut_traits = mut_trait_names(&root);
    let shell_names = shell_names(&root, &tier_file);
    let crates: Vec<String> = match only {
        Some(krate) => {
            require_crate(&root, krate)?;
            vec![krate.to_string()]
        }
        None => crate_names(&root)
            .into_iter()
            .filter(|name| has_shell_source(&tier_file, name))
            .collect(),
    };
    println!("== purity-candidates");
    println!("  A candidate is a free fn or impl method whose signature and body pass the Pure");
    println!("  rules; `strict` marks the ones that also pass the StrictPure rules. A `&mut` in");
    println!("  the signature disqualifies even a private fn. One level of call checking: each");
    println!("  same-crate callee (matched by name) must also pass the same rules on its own");
    println!("  body; callees are not followed further, and calls into other crates are not");
    println!("  checked. loc is non-test code lines in the function.");
    let mut all_pure_loc = 0;
    let mut all_strict_loc = 0;
    for krate in &crates {
        let tier = tier_label(&tier_file, krate);
        // With no crate named, only the `Shell` sources (the ceiling's scope) are listed.
        let candidates: Vec<Candidate> =
            crate_candidates(&root, krate, &mut_traits, &tier_file, &shell_names)
                .into_iter()
                .filter(|c| only.is_some() || c.shell)
                .collect();
        let pure_loc: usize = candidates.iter().map(|c| c.loc).sum();
        let strict: Vec<&Candidate> = candidates.iter().filter(|c| c.strict).collect();
        let strict_loc: usize = strict.iter().map(|c| c.loc).sum();
        println!();
        println!(
            "{krate} ({tier}): pure candidates {} fns, {pure_loc} LOC; strict candidates {} fns, {strict_loc} LOC",
            candidates.len(),
            strict.len()
        );
        println!("  {:>6}  {:<6}  location  function", "loc", "strict");
        for c in &candidates {
            let strict_col = if c.strict { "yes" } else { "no" };
            println!(
                "  {:>6}  {strict_col:<6}  {}:{}  {}",
                c.loc, c.path, c.line, c.name
            );
        }
        all_pure_loc += pure_loc;
        all_strict_loc += strict_loc;
    }
    println!();
    println!(
        "listed crates: pure candidate LOC {all_pure_loc}, strict candidate LOC {all_strict_loc}"
    );
    Ok(())
}

/// Shell logic ceiling: the pure and strict candidate LOC summed over the `Shell`
/// sources (`Shell` crates and `Shell` modules), as `purity-report --candidates`
/// lists them.
fn shell_candidate_loc(root: &Path, file: &TierFile, mut_traits: &BTreeSet<String>) -> Ceilings {
    let shell_names = shell_names(root, file);
    let mut total = Ceilings::default();
    for name in crate_names(root)
        .into_iter()
        .filter(|name| has_shell_source(file, name))
    {
        for c in crate_candidates(root, &name, mut_traits, file, &shell_names)
            .into_iter()
            .filter(|c| c.shell)
        {
            total.pure += c.loc as u64;
            if c.strict {
                total.strict += c.loc as u64;
            }
        }
    }
    total
}

/// The gate (`purity-report --gate`, also run by `lint-architecture`): the shell
/// logic figures must not exceed the ceilings in `tools/reports/purity-baseline.txt`.
pub fn gate(root: &Path) -> Result<(), String> {
    let file = load_tier_file(root)?;
    let measured = shell_candidate_loc(root, &file, &mut_trait_names(root));
    let ceilings = read_baseline(root)?;
    check_ceilings(measured, ceilings)?;
    println!(
        "  ok: shell logic pure candidate LOC {} <= {}, strict {} <= {}",
        measured.pure, ceilings.pure, measured.strict, ceilings.strict
    );
    Ok(())
}

fn read_baseline(root: &Path) -> Result<Ceilings, String> {
    let path = root.join(BASELINE_PATH);
    let text =
        fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse_baseline(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Crate directory names under `crates/`, sorted.
/// The shell-tier names of the workspace: the types defined in `Shell` sources (minus any
/// name that a non-shell source also defines), and the `Shell` crates and modules.
fn shell_names(root: &Path, file: &TierFile) -> ShellNames {
    let ident = |krate: &str| krate.replace('-', "_");
    let mut shell_types = BTreeSet::new();
    let mut other_types = BTreeSet::new();
    let mut names = ShellNames::default();
    for krate in crate_names(root) {
        let src_dir = root.join("crates").join(&krate).join("src");
        if crate_mark(file, &krate) == Some(Mark::Shell) {
            names.crates.insert(ident(&krate));
        }
        for (module, mark) in file.modules.get(&krate).into_iter().flatten() {
            let set = if *mark == Mark::Shell {
                &mut names.shell_modules
            } else {
                &mut names.other_modules
            };
            set.entry(ident(&krate)).or_default().insert(module.clone());
        }
        for source in rust_files(&src_dir) {
            let Some(ast) = parse(&source) else {
                continue;
            };
            let mark = file_mark(file, &krate, module_of(&src_dir, &source).as_deref());
            let target = if mark == Some(Mark::Shell) {
                &mut shell_types
            } else {
                &mut other_types
            };
            target.extend(defined_types(&ast));
        }
    }
    names.types = shell_types.difference(&other_types).cloned().collect();
    names
}

fn crate_names(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(root.join("crates"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

/// Non-blank, non-comment lines outside test-gated items.
fn non_test_loc(file: &Path) -> usize {
    let ast = parse(file);
    // A file gated as a whole (`#![cfg(test)]`, e.g. a test-only module file)
    // is test code end to end.
    if ast.as_ref().is_some_and(|ast| is_test_gated(&ast.attrs)) {
        return 0;
    }
    let ranges = ast.map(|ast| test_ranges(&ast)).unwrap_or_default();
    code_lines(file)
        .iter()
        .filter(|(line, _)| !ranges.iter().any(|(s, e)| (*s..=*e).contains(line)))
        .count()
}

/// Names of workspace traits with at least one `&mut self` method, used to
/// decide which `dyn Trait` types are impure in `Pure` crates.
fn mut_trait_names(root: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for file in rust_files(&root.join("crates")) {
        if let Some(ast) = parse(&file) {
            let mut finder = MutTraits(&mut names);
            finder.visit_file(&ast);
        }
    }
    names
}

fn parse(file: &Path) -> Option<syn::File> {
    let src = fs::read_to_string(file).ok()?;
    syn::parse_file(&src).ok()
}

fn file_violations(file: &Path, mut_traits: &BTreeSet<String>) -> Vec<Violation> {
    match parse(file) {
        // A file gated as a whole (`#![cfg(test)]`) is test code end to end.
        Some(ast) if is_test_gated(&ast.attrs) => Vec::new(),
        Some(ast) => {
            let uses = UseMap::of_file(&ast);
            let mut scan = Scan {
                mut_traits,
                found: Vec::new(),
                let_depth: 0,
                trait_impl: false,
                uses: &uses,
            };
            scan.visit_file(&ast);
            scan.found
        }
        None => vec![Violation {
            strict: false,
            line: 0,
            text: format!("could not parse {} for the purity scan", file.display()),
        }],
    }
}

// --- syntax helpers --------------------------------------------------------

pub(crate) fn item_attrs(item: &syn::Item) -> &[syn::Attribute] {
    match item {
        syn::Item::Const(i) => &i.attrs,
        syn::Item::Enum(i) => &i.attrs,
        syn::Item::ExternCrate(i) => &i.attrs,
        syn::Item::Fn(i) => &i.attrs,
        syn::Item::ForeignMod(i) => &i.attrs,
        syn::Item::Impl(i) => &i.attrs,
        syn::Item::Macro(i) => &i.attrs,
        syn::Item::Mod(i) => &i.attrs,
        syn::Item::Static(i) => &i.attrs,
        syn::Item::Struct(i) => &i.attrs,
        syn::Item::Trait(i) => &i.attrs,
        syn::Item::TraitAlias(i) => &i.attrs,
        syn::Item::Type(i) => &i.attrs,
        syn::Item::Union(i) => &i.attrs,
        syn::Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

pub(crate) fn impl_item_attrs(item: &syn::ImplItem) -> &[syn::Attribute] {
    match item {
        syn::ImplItem::Const(i) => &i.attrs,
        syn::ImplItem::Fn(i) => &i.attrs,
        syn::ImplItem::Type(i) => &i.attrs,
        syn::ImplItem::Macro(i) => &i.attrs,
        _ => &[],
    }
}

pub(crate) fn trait_item_attrs(item: &syn::TraitItem) -> &[syn::Attribute] {
    match item {
        syn::TraitItem::Const(i) => &i.attrs,
        syn::TraitItem::Fn(i) => &i.attrs,
        syn::TraitItem::Type(i) => &i.attrs,
        syn::TraitItem::Macro(i) => &i.attrs,
        _ => &[],
    }
}

/// Line span of every test-gated item, for the LOC metric.
fn test_ranges(ast: &syn::File) -> Vec<(usize, usize)> {
    let mut spans = TestSpans(Vec::new());
    spans.visit_file(ast);
    spans.0
}

fn span_lines<T: Spanned>(node: &T) -> (usize, usize) {
    let span = node.span();
    (span.start().line, span.end().line)
}

struct TestSpans(Vec<(usize, usize)>);

impl<'ast> Visit<'ast> for TestSpans {
    fn visit_item(&mut self, node: &'ast syn::Item) {
        if is_test_gated(item_attrs(node)) {
            self.0.push(span_lines(node));
        } else {
            visit::visit_item(self, node);
        }
    }

    fn visit_impl_item(&mut self, node: &'ast syn::ImplItem) {
        if is_test_gated(impl_item_attrs(node)) {
            self.0.push(span_lines(node));
        } else {
            visit::visit_impl_item(self, node);
        }
    }

    fn visit_trait_item(&mut self, node: &'ast syn::TraitItem) {
        if is_test_gated(trait_item_attrs(node)) {
            self.0.push(span_lines(node));
        } else {
            visit::visit_trait_item(self, node);
        }
    }
}

struct MutTraits<'a>(&'a mut BTreeSet<String>);

impl<'ast> Visit<'ast> for MutTraits<'_> {
    fn visit_item(&mut self, node: &'ast syn::Item) {
        if !is_test_gated(item_attrs(node)) {
            visit::visit_item(self, node);
        }
    }

    fn visit_item_trait(&mut self, node: &'ast syn::ItemTrait) {
        let takes_mut_self = node
            .items
            .iter()
            .any(|item| matches!(item, syn::TraitItem::Fn(f) if receiver_is_mut_ref(&f.sig)));
        if takes_mut_self {
            self.0.insert(node.ident.to_string());
        }
        visit::visit_item_trait(self, node);
    }
}

fn receiver_is_mut_ref(sig: &syn::Signature) -> bool {
    matches!(
        sig.inputs.first(),
        Some(syn::FnArg::Receiver(r)) if r.reference.is_some() && r.mutability.is_some()
    )
}

/// True if any reference inside a type is `&mut`.
fn type_has_mut_ref(ty: &syn::Type) -> bool {
    struct Finder(bool);
    impl<'ast> Visit<'ast> for Finder {
        fn visit_type_reference(&mut self, node: &'ast syn::TypeReference) {
            self.0 |= node.mutability.is_some();
            visit::visit_type_reference(self, node);
        }
    }
    let mut finder = Finder(false);
    finder.visit_type(ty);
    finder.0
}

fn sig_has_mut_ref(sig: &syn::Signature) -> bool {
    sig.inputs.iter().any(|arg| match arg {
        syn::FnArg::Receiver(r) => {
            (r.reference.is_some() && r.mutability.is_some()) || type_has_mut_ref(&r.ty)
        }
        syn::FnArg::Typed(pt) => !is_formatter_sink(&pt.ty) && type_has_mut_ref(&pt.ty),
    })
}

/// `&mut Formatter` is the std formatting sink a `Display`/`Debug` impl must
/// accept; writing to it is rendering, not shared state, so neither the `Pure`
/// signature rule nor the `StrictPure` `&mut` rule counts it.
fn is_formatter_sink(ty: &syn::Type) -> bool {
    matches!(ty, syn::Type::Reference(r) if is_formatter_ref(r))
}

/// `&mut Formatter` (any path ending in `Formatter`) as a reference type.
fn is_formatter_ref(r: &syn::TypeReference) -> bool {
    r.mutability.is_some()
        && matches!(&*r.elem, syn::Type::Path(p) if p
            .path
            .segments
            .last()
            .is_some_and(|seg| seg.ident == "Formatter"))
}

/// Flattens a `use` tree into full paths (`std::time::Instant`, ...).
pub(crate) fn use_paths(tree: &syn::UseTree, prefix: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    match tree {
        syn::UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            use_paths(&p.tree, prefix, out);
            prefix.pop();
        }
        syn::UseTree::Name(n) => {
            let mut path = prefix.clone();
            if n.ident != "self" {
                path.push(n.ident.to_string());
            }
            out.push(path);
        }
        syn::UseTree::Rename(r) => {
            let mut path = prefix.clone();
            if r.ident != "self" {
                path.push(r.ident.to_string());
            }
            out.push(path);
        }
        syn::UseTree::Glob(_) => out.push(prefix.clone()),
        syn::UseTree::Group(g) => {
            for item in &g.items {
                use_paths(item, prefix, out);
            }
        }
    }
}

/// The `use` declarations of one file, for resolving a name written at a call
/// site back to the full path it was imported from. Scoping is ignored: a `use`
/// anywhere in the file (also inside a function or module) maps its name for the
/// whole file, which can only over-report, never miss an import.
#[derive(Debug, Default, Clone)]
pub(crate) struct UseMap {
    /// Local name (the last segment, or the `as` rename) to the imported path.
    names: BTreeMap<String, Vec<String>>,
    /// Prefixes of glob imports (`use std::fs::*` gives `std::fs`).
    globs: Vec<Vec<String>>,
}

/// Names a glob import of `prefix` is known to bring into scope, for the banned
/// path classes. A glob hides which names a call site uses, so only these
/// well-known I/O names (and, for `std`, the banned module names) are resolved.
fn glob_members(prefix: &[String]) -> &'static [&'static str] {
    let last = prefix.last().map(String::as_str).unwrap_or("");
    let std_rooted = matches!(prefix.first().map(String::as_str), Some("std" | "core"));
    if prefix.len() == 1 && std_rooted {
        return BANNED_STD_MODULES;
    }
    if prefix.first().is_some_and(|s| s == "rand") {
        return &[
            "random",
            "thread_rng",
            "rng",
            "Rng",
            "RngCore",
            "SeedableRng",
            "StdRng",
            "SmallRng",
            "ThreadRng",
            "OsRng",
            "gen_range",
            "rngs",
            "seq",
            "distributions",
            "prelude",
        ];
    }
    if !std_rooted {
        return &[];
    }
    match last {
        "fs" => &[
            "File",
            "OpenOptions",
            "DirEntry",
            "ReadDir",
            "Metadata",
            "Permissions",
            "read",
            "read_to_string",
            "read_dir",
            "read_link",
            "write",
            "copy",
            "rename",
            "remove_file",
            "remove_dir",
            "remove_dir_all",
            "create_dir",
            "create_dir_all",
            "metadata",
            "symlink_metadata",
            "canonicalize",
            "hard_link",
            "set_permissions",
            "exists",
        ],
        "process" => &[
            "Command",
            "Child",
            "Stdio",
            "ExitStatus",
            "Output",
            "exit",
            "abort",
            "id",
        ],
        "env" => &[
            "var",
            "vars",
            "var_os",
            "vars_os",
            "args",
            "args_os",
            "current_dir",
            "set_var",
            "remove_var",
            "set_current_dir",
            "current_exe",
            "temp_dir",
            "home_dir",
        ],
        "net" => &[
            "TcpStream",
            "TcpListener",
            "UdpSocket",
            "SocketAddr",
            "ToSocketAddrs",
            "IpAddr",
            "Ipv4Addr",
            "Ipv6Addr",
        ],
        "time" => &["SystemTime", "Instant", "UNIX_EPOCH"],
        "thread" => &[
            "spawn",
            "sleep",
            "scope",
            "Builder",
            "JoinHandle",
            "current",
            "yield_now",
            "park",
            "available_parallelism",
        ],
        _ => &[],
    }
}

impl UseMap {
    /// Collects the `use` items of the non-test code of a file.
    pub(crate) fn of_file(ast: &syn::File) -> Self {
        let mut collector = UseCollector(Self::default());
        collector.visit_file(ast);
        collector.0
    }

    fn add(&mut self, tree: &syn::UseTree, prefix: &mut Vec<String>) {
        match tree {
            syn::UseTree::Path(p) => {
                prefix.push(p.ident.to_string());
                self.add(&p.tree, prefix);
                prefix.pop();
            }
            syn::UseTree::Name(n) => {
                let mut path = prefix.clone();
                if n.ident == "self" {
                    // `use std::fs::{self}` imports `std::fs` under `fs`.
                    if let Some(name) = path.last().cloned() {
                        self.names.insert(name, path);
                    }
                } else {
                    path.push(n.ident.to_string());
                    self.names.insert(n.ident.to_string(), path);
                }
            }
            syn::UseTree::Rename(r) => {
                let mut path = prefix.clone();
                if r.ident != "self" {
                    path.push(r.ident.to_string());
                }
                if r.rename != "_" {
                    self.names.insert(r.rename.to_string(), path);
                }
            }
            syn::UseTree::Glob(_) => self.globs.push(prefix.clone()),
            syn::UseTree::Group(g) => {
                for item in &g.items {
                    self.add(item, prefix);
                }
            }
        }
    }

    /// Every full path `segs` could stand for: the path as written, plus its
    /// first segment replaced by what that name imports, plus each glob prefix
    /// that is known to bring the first segment into scope.
    pub(crate) fn resolve(&self, segs: &[String]) -> Vec<Vec<String>> {
        let mut out = vec![segs.to_vec()];
        let Some(first) = segs.first() else {
            return out;
        };
        if let Some(full) = self.names.get(first) {
            let mut path = full.clone();
            path.extend_from_slice(&segs[1..]);
            out.push(path);
        } else {
            for glob in &self.globs {
                if glob_members(glob).contains(&first.as_str()) {
                    let mut path = glob.clone();
                    path.extend_from_slice(segs);
                    out.push(path);
                }
            }
        }
        out
    }
}

struct UseCollector(UseMap);

impl<'ast> Visit<'ast> for UseCollector {
    fn visit_item(&mut self, node: &'ast syn::Item) {
        if is_test_gated(item_attrs(node)) {
            return;
        }
        if let syn::Item::Use(u) = node {
            self.0.add(&u.tree, &mut Vec::new());
        }
        visit::visit_item(self, node);
    }

    fn visit_impl_item(&mut self, node: &'ast syn::ImplItem) {
        if !is_test_gated(impl_item_attrs(node)) {
            visit::visit_impl_item(self, node);
        }
    }
}

/// Compound-assignment operators (`x += e`, ...), which syn parses as binary
/// expressions.
fn is_compound_assign(op: &syn::BinOp) -> bool {
    matches!(
        op,
        syn::BinOp::AddAssign(_)
            | syn::BinOp::SubAssign(_)
            | syn::BinOp::MulAssign(_)
            | syn::BinOp::DivAssign(_)
            | syn::BinOp::RemAssign(_)
            | syn::BinOp::BitXorAssign(_)
            | syn::BinOp::BitAndAssign(_)
            | syn::BinOp::BitOrAssign(_)
            | syn::BinOp::ShlAssign(_)
            | syn::BinOp::ShrAssign(_)
    )
}

// --- the visitor -----------------------------------------------------------

struct Scan<'a> {
    mut_traits: &'a BTreeSet<String>,
    found: Vec<Violation>,
    /// Depth of `let` initializers being visited; assignments inside them are
    /// exempt from the StrictPure assignment rule.
    let_depth: usize,
    /// Whether the impl being visited implements a trait.
    trait_impl: bool,
    /// The file's imports, so `fs::read` after `use std::fs` is `std::fs::read`.
    uses: &'a UseMap,
}

impl Scan<'_> {
    fn pure(&mut self, line: usize, text: impl Into<String>) {
        self.found.push(Violation {
            strict: false,
            line,
            text: text.into(),
        });
    }

    fn strict(&mut self, line: usize, text: impl Into<String>) {
        self.found.push(Violation {
            strict: true,
            line,
            text: text.into(),
        });
    }

    /// `Pure` rules on a path, whether it is an expression, a type or a `use`.
    /// The path is checked as written and as its imports resolve it.
    fn check_path(&mut self, segs: &[String], line: usize) {
        let mut seen = BTreeSet::new();
        for full in self.uses.resolve(segs) {
            for text in path_findings(&full) {
                if seen.insert(text.clone()) {
                    self.pure(line, text);
                }
            }
        }
    }
}

/// The `Pure` findings on one fully resolved path.
fn path_findings(segs: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(name) = segs
        .iter()
        .find(|s| INTERIOR_MUT.contains(&s.as_str()) || s.starts_with("Atomic"))
    {
        out.push(format!("interior-mutability type `{name}`"));
    }
    let std_module = segs
        .windows(2)
        .find(|w| {
            matches!(w[0].as_str(), "std" | "core") && BANNED_STD_MODULES.contains(&w[1].as_str())
        })
        .map(|w| w[1].clone());
    if let Some(module) = std_module {
        out.push(format!("use of `std::{module}`"));
    }
    if segs.first().is_some_and(|s| s == "rand") {
        out.push("use of `rand`".to_string());
    }
    out
}

impl<'ast> Visit<'ast> for Scan<'_> {
    fn visit_item(&mut self, node: &'ast syn::Item) {
        if !is_test_gated(item_attrs(node)) {
            visit::visit_item(self, node);
        }
    }

    fn visit_impl_item(&mut self, node: &'ast syn::ImplItem) {
        if !is_test_gated(impl_item_attrs(node)) {
            visit::visit_impl_item(self, node);
        }
    }

    fn visit_trait_item(&mut self, node: &'ast syn::TraitItem) {
        if !is_test_gated(trait_item_attrs(node)) {
            visit::visit_trait_item(self, node);
        }
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if matches!(node.vis, syn::Visibility::Public(_)) && sig_has_mut_ref(&node.sig) {
            let line = span_lines(&node.sig).0;
            self.pure(line, "`&mut` in the signature of a public function");
        }
        visit::visit_item_fn(self, node);
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        if node.unsafety.is_some() {
            self.pure(span_lines(node).0, "`unsafe` impl");
        }
        let previous = self.trait_impl;
        self.trait_impl = node.trait_.is_some();
        visit::visit_item_impl(self, node);
        self.trait_impl = previous;
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        let public = matches!(node.vis, syn::Visibility::Public(_));
        if (self.trait_impl || public) && sig_has_mut_ref(&node.sig) {
            let line = span_lines(&node.sig).0;
            self.pure(
                line,
                "`&mut` in the signature of a public method or trait method",
            );
        }
        visit::visit_impl_item_fn(self, node);
    }

    fn visit_trait_item_fn(&mut self, node: &'ast syn::TraitItemFn) {
        if sig_has_mut_ref(&node.sig) {
            let line = span_lines(&node.sig).0;
            self.pure(line, "`&mut` in a trait method signature");
        }
        visit::visit_trait_item_fn(self, node);
    }

    fn visit_item_trait(&mut self, node: &'ast syn::ItemTrait) {
        if node.unsafety.is_some() {
            self.pure(span_lines(node).0, "`unsafe` trait");
        }
        visit::visit_item_trait(self, node);
    }

    fn visit_signature(&mut self, node: &'ast syn::Signature) {
        if node.unsafety.is_some() {
            let line = span_lines(node).0;
            self.pure(line, "`unsafe` fn");
        }
        visit::visit_signature(self, node);
    }

    fn visit_item_static(&mut self, node: &'ast syn::ItemStatic) {
        if node
            .mutability
            .to_token_stream()
            .to_string()
            .contains("mut")
        {
            self.pure(span_lines(node).0, "`static mut`");
        }
        visit::visit_item_static(self, node);
    }

    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        let mut paths = Vec::new();
        use_paths(&node.tree, &mut Vec::new(), &mut paths);
        let line = span_lines(node).0;
        for segs in &paths {
            self.check_path(segs, line);
        }
    }

    fn visit_path(&mut self, node: &'ast syn::Path) {
        let segs: Vec<String> = node.segments.iter().map(|s| s.ident.to_string()).collect();
        self.check_path(&segs, span_lines(node).0);
        visit::visit_path(self, node);
    }

    fn visit_expr_unsafe(&mut self, node: &'ast syn::ExprUnsafe) {
        self.pure(span_lines(node).0, "`unsafe` block");
        visit::visit_expr_unsafe(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        let name = node.method.to_string();
        if name == "unwrap" && node.args.is_empty() {
            self.pure(span_lines(node).0, "`.unwrap()` call");
        } else if name == "expect" {
            self.pure(span_lines(node).0, "`.expect(` call");
        }
        visit::visit_expr_method_call(self, node);
    }

    fn visit_type_trait_object(&mut self, node: &'ast syn::TypeTraitObject) {
        let flagged = node.bounds.iter().find_map(|bound| match bound {
            syn::TypeParamBound::Trait(tb) => tb
                .path
                .segments
                .last()
                .map(|s| s.ident.to_string())
                .filter(|name| self.mut_traits.contains(name)),
            _ => None,
        });
        if let Some(name) = flagged {
            self.pure(
                span_lines(node).0,
                format!("`dyn {name}` where `{name}` has a `&mut self` method"),
            );
        }
        visit::visit_type_trait_object(self, node);
    }

    fn visit_pat_ident(&mut self, node: &'ast syn::PatIdent) {
        if node.mutability.is_some() {
            self.strict(span_lines(node).0, "`mut` binding");
        }
        visit::visit_pat_ident(self, node);
    }

    fn visit_receiver(&mut self, node: &'ast syn::Receiver) {
        // Not recursed: the receiver's type would be reported a second time.
        if node.mutability.is_some() {
            let what = if node.reference.is_some() {
                "`&mut self`"
            } else {
                "`mut self` binding"
            };
            self.strict(span_lines(node).0, what);
        }
    }

    fn visit_type_reference(&mut self, node: &'ast syn::TypeReference) {
        if node.mutability.is_some() && !is_formatter_ref(node) {
            self.strict(span_lines(node).0, "`&mut` type");
        }
        visit::visit_type_reference(self, node);
    }

    fn visit_expr_reference(&mut self, node: &'ast syn::ExprReference) {
        if node.mutability.is_some() {
            self.strict(span_lines(node).0, "`&mut` expression");
        }
        visit::visit_expr_reference(self, node);
    }

    fn visit_local(&mut self, node: &'ast syn::Local) {
        self.let_depth += 1;
        visit::visit_local(self, node);
        self.let_depth -= 1;
    }

    fn visit_expr_assign(&mut self, node: &'ast syn::ExprAssign) {
        if self.let_depth == 0 {
            self.strict(span_lines(node).0, "assignment outside a `let` initializer");
        }
        visit::visit_expr_assign(self, node);
    }

    fn visit_expr_binary(&mut self, node: &'ast syn::ExprBinary) {
        if is_compound_assign(&node.op) && self.let_depth == 0 {
            self.strict(
                span_lines(node).0,
                "compound assignment outside a `let` initializer",
            );
        }
        visit::visit_expr_binary(self, node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_tiers(text: &str) -> Result<TierTable, String> {
        parse_tier_file(text).map(|file| file.tiers)
    }

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_string()).collect()
    }

    fn fact(name: &str, pure_ok: bool, strict_ok: bool, callees: &[&str]) -> FnFacts {
        FnFacts {
            name: name.to_string(),
            pure_ok,
            strict_ok,
            callees: callees.iter().map(|c| (*c).to_string()).collect(),
            io: false,
            path_callees: BTreeSet::new(),
        }
    }

    /// Parses `src` as one file and collects its functions; every line counts as
    /// code.
    fn sites_of(src: &str) -> Vec<FnSite> {
        let ast = syn::parse_file(src).unwrap();
        let code: Vec<usize> = (1..=src.lines().count()).collect();
        sites_from_ast(&ast, &code, &BTreeSet::new(), &ShellNames::default())
    }

    fn names_and_flags(sites: &[FnSite]) -> Vec<(&str, bool, bool)> {
        sites
            .iter()
            .map(|s| (s.facts.name.as_str(), s.facts.pure_ok, s.facts.strict_ok))
            .collect()
    }

    #[test]
    fn a_fn_calling_git_methods_on_a_shell_param_is_not_a_candidate() {
        let names = ShellNames {
            types: BTreeSet::from(["Git".to_string()]),
            ..ShellNames::default()
        };
        let src =
            "fn head(git: &Git) -> String { git.head() }\nfn add(a: u32, b: u32) -> u32 { a + b }";
        let ast = syn::parse_file(src).unwrap();
        let code: Vec<usize> = (1..=src.lines().count()).collect();
        let sites = sites_from_ast(&ast, &code, &BTreeSet::new(), &names);
        assert_eq!(
            names_and_flags(&sites),
            vec![("head", false, false), ("add", true, true)]
        );
        // Without the shell type known, the same function looks pure.
        let blind = sites_from_ast(&ast, &code, &BTreeSet::new(), &ShellNames::default());
        assert_eq!(names_and_flags(&blind)[0], ("head", true, true));
    }

    #[test]
    fn shell_io_spreads_to_callers_through_path_calls() {
        let mut io = fact("run", true, true, &[]);
        io.io = true;
        let mut wrapper = fact("out", true, true, &["run"]);
        wrapper.path_callees = BTreeSet::from(["run".to_string()]);
        let mut outer = fact("changes", true, true, &["out"]);
        outer.path_callees = BTreeSet::from(["out".to_string()]);
        let flags = candidate_flags(&[io, wrapper, outer]);
        assert_eq!(flags, [(false, false); 3]);
    }

    #[test]
    fn a_file_gated_as_a_whole_has_no_violations() {
        let dir = std::env::temp_dir().join("arch-lint-gated-file-test");
        std::fs::create_dir_all(&dir).unwrap();
        let body = "fn load() -> u8 { Some(1).unwrap() }";
        let plain = dir.join("plain.rs");
        let gated = dir.join("gated.rs");
        std::fs::write(&plain, body).unwrap();
        std::fs::write(
            &gated,
            format!(
                "#![cfg(test)]
{body}"
            ),
        )
        .unwrap();
        assert_eq!(file_violations(&plain, &BTreeSet::new()).len(), 1);
        assert!(file_violations(&gated, &BTreeSet::new()).is_empty());
    }

    #[test]
    fn shell_marker_is_listed_impure_product() {
        let file = parse_tier_file(
            "wg-store Shell
wg-types StrictPure
",
        )
        .unwrap();
        assert!(file.shell.contains("wg-store"));
        assert!(!is_tooling(&file, "wg-store"));
        assert_eq!(purity_tier(&file.tiers, "wg-store"), PurityTier::Impure);
        let names = vec!["wg-store".to_string(), "wg-types".to_string()];
        assert!(unlisted_crate_violations(&names, &file).is_empty());
    }

    #[test]
    fn tooling_marker_is_impure_and_not_in_tier_table() {
        let file = parse_tier_file("wg-x Tooling\nwg-types StrictPure\n").unwrap();
        assert_eq!(file.tiers.len(), 1);
        assert!(file.tooling.contains("wg-x"));
        assert!(is_tooling(&file, "wg-x"));
        assert!(!is_tooling(&file, "wg-types"));
        assert_eq!(purity_tier(&file.tiers, "wg-x"), PurityTier::Impure);
    }

    #[test]
    fn crate_listed_as_tooling_and_tier_is_a_duplicate() {
        let err = parse_tier_file("xtask Tooling\nxtask Pure\n").unwrap_err();
        assert!(err.contains("duplicate crate `xtask`"), "{err}");
    }

    #[test]
    fn unknown_tier_message_names_tooling() {
        let err = parse_tier_file("wg-types Impure\n").unwrap_err();
        assert!(err.contains("`Tooling`"), "{err}");
    }

    #[test]
    fn parses_report_modes() {
        assert_eq!(parse_report_args(&args(&[])).unwrap(), ReportMode::Summary);
        assert_eq!(
            parse_report_args(&args(&["--violations", "wg-types"])).unwrap(),
            ReportMode::Violations("wg-types".to_string())
        );
        assert_eq!(
            parse_report_args(&args(&["--candidates"])).unwrap(),
            ReportMode::Candidates(None)
        );
        assert_eq!(
            parse_report_args(&args(&["--candidates", "wg-server"])).unwrap(),
            ReportMode::Candidates(Some("wg-server".to_string()))
        );
    }

    #[test]
    fn rejects_bad_report_args() {
        let missing = parse_report_args(&args(&["--violations"])).unwrap_err();
        assert!(missing.contains("needs a crate name"), "{missing}");
        let extra = parse_report_args(&args(&["--candidates", "a", "b"])).unwrap_err();
        assert!(extra.contains("at most one crate name"), "{extra}");
        let unknown = parse_report_args(&args(&["--frobnicate"])).unwrap_err();
        assert!(unknown.contains("unknown option"), "{unknown}");
    }

    #[test]
    fn candidate_needs_own_clean_body() {
        assert_eq!(
            candidate_flags(&[fact("a", false, false, &[])]),
            vec![(false, false)]
        );
    }

    #[test]
    fn strict_clean_fn_is_also_pure() {
        assert_eq!(
            candidate_flags(&[fact("a", true, true, &[])]),
            vec![(true, true)]
        );
        assert_eq!(
            candidate_flags(&[fact("b", true, false, &[])]),
            vec![(true, false)]
        );
    }

    #[test]
    fn non_candidate_callee_disqualifies_caller() {
        let facts = [
            fact("caller", true, true, &["helper"]),
            fact("helper", false, false, &[]),
        ];
        assert_eq!(
            candidate_flags(&facts),
            vec![(false, false), (false, false)]
        );
    }

    #[test]
    fn candidate_callee_keeps_caller() {
        let facts = [
            fact("caller", true, true, &["helper"]),
            fact("helper", true, true, &[]),
        ];
        assert_eq!(candidate_flags(&facts)[0], (true, true));
    }

    #[test]
    fn strict_caller_needs_strict_callee() {
        let facts = [
            fact("caller", true, true, &["helper"]),
            fact("helper", true, false, &[]),
        ];
        assert_eq!(candidate_flags(&facts)[0], (true, false));
    }

    #[test]
    fn callee_name_matches_every_same_named_definition() {
        let facts = [
            fact("caller", true, true, &["new"]),
            fact("new", true, true, &[]),
            fact("new", false, false, &[]),
        ];
        assert_eq!(candidate_flags(&facts)[0], (false, false));
    }

    #[test]
    fn callee_outside_the_crate_is_ignored() {
        assert_eq!(
            candidate_flags(&[fact("caller", true, true, &["std_thing"])]),
            vec![(true, true)]
        );
    }

    #[test]
    fn scan_flags_mut_signature_and_unwrap_and_skips_tests() {
        let src = "\
fn takes(v: &mut Vec<u8>) { v.push(1); }
fn reads(v: &[u8]) -> u8 { v.first().copied().unwrap() }
fn clean(a: u8) -> u8 { a + 1 }
#[test]
fn t() { let x: Option<u8> = None; x.unwrap(); }
#[cfg(test)]
mod tests { fn helper() {} }
";
        let sites = sites_of(src);
        assert_eq!(
            names_and_flags(&sites),
            vec![
                ("takes", false, false),
                ("reads", false, false),
                ("clean", true, true)
            ]
        );
        assert_eq!(sites[2].line, 3);
        assert_eq!(sites[2].loc, 1);
    }

    /// Findings of a whole file, as `file_violations` computes them.
    fn violations_of(src: &str) -> Vec<Violation> {
        let ast = syn::parse_file(src).unwrap();
        let uses = UseMap::of_file(&ast);
        let mut scan = Scan {
            mut_traits: &BTreeSet::new(),
            found: Vec::new(),
            let_depth: 0,
            trait_impl: false,
            uses: &uses,
        };
        scan.visit_file(&ast);
        scan.found
    }

    /// Every import form of every banned path class: the call site only names
    /// the imported item, yet the function must have a Pure violation of its own.
    const IMPORT_FIXTURES: &[(&str, &str)] = &[
        ("use std::fs;", "fs::read(p)"),
        ("use std::fs::File;", "File::open(p)"),
        ("use std::{fs, process::Command};", "fs::read(p)"),
        ("use std::{fs, process::Command};", "Command::new(p)"),
        ("use std::fs as f;", "f::read(p)"),
        ("use std::fs::File as F;", "F::open(p)"),
        ("use std::fs::*;", "read(p)"),
        ("use std::fs::*;", "File::open(p)"),
        ("use std::fs::{self};", "fs::read(p)"),
        ("use std::fs::{self as f};", "f::read(p)"),
        ("use std::{io, fs::{read, File}};", "read(p)"),
        ("use std::*;", "fs::read(p)"),
        ("use core::*;", "time::Duration::ZERO"),
        ("use std::process;", "process::exit(1)"),
        ("use std::process::exit;", "exit(1)"),
        ("use std::process::*;", "exit(1)"),
        ("use std::env;", "env::var(p)"),
        ("use std::env::var as v;", "v(p)"),
        ("use std::env::*;", "args()"),
        ("use std::net::TcpStream;", "TcpStream::connect(p)"),
        ("use std::net as n;", "n::TcpStream::connect(p)"),
        ("use std::net::*;", "TcpStream::connect(p)"),
        ("use std::time::SystemTime;", "SystemTime::now()"),
        ("use std::time::SystemTime as T;", "T::now()"),
        ("use std::time as t;", "t::SystemTime::now()"),
        ("use std::time::*;", "Instant::now()"),
        ("use std::thread;", "thread::sleep(d)"),
        ("use std::thread::sleep;", "sleep(d)"),
        ("use std::thread::*;", "spawn(f)"),
        ("use rand::random;", "random()"),
        ("use rand as r;", "r::random()"),
        ("use rand::{thread_rng, Rng};", "thread_rng()"),
        ("use rand::*;", "thread_rng()"),
        ("use rand::rngs::StdRng as S;", "S::from_entropy()"),
        ("use std::cell::RefCell as Cell2;", "Cell2::new(1)"),
        ("use std::sync::atomic::AtomicU8 as A;", "A::new(1)"),
        ("use std::sync::{Mutex as M, Arc};", "M::new(1)"),
    ];

    #[test]
    fn use_imported_banned_paths_are_found_at_the_call_site() {
        for (import, call) in IMPORT_FIXTURES {
            // The function sits in its own file with the import, and the call
            // is a statement so the body, not the `use`, must carry the finding.
            let src = format!("{import}\nfn f(p: &str, d: u8, f: u8) {{ let _ = {call}; }}\n");
            let sites = sites_of(&src);
            assert!(
                !sites[0].facts.pure_ok,
                "`{import}` then `{call}` is still a pure candidate"
            );
            assert!(
                violations_of(&src).iter().any(|v| v.line == 2),
                "`{import}` then `{call}` has no violation on the call line"
            );
        }
    }

    #[test]
    fn a_function_inside_a_block_import_is_found_too() {
        let src = "fn f(p: &str) { use std::fs; let _ = fs::read(p); }\n";
        assert!(!sites_of(src)[0].facts.pure_ok);
    }

    #[test]
    fn full_paths_and_local_shadows_keep_their_old_verdicts() {
        // A full path was always found.
        assert!(
            !sites_of("fn f(p: &str) { let _ = std::fs::read(p); }\n")[0]
                .facts
                .pure_ok
        );
        // `fs` here is a crate module, not `std::fs`, and a name that no import
        // maps stays clean.
        let clean = "use crate::fs;\nuse std::collections::BTreeMap;\n\
                     fn f(p: &str) { let _ = fs::read(p); }\n\
                     fn g(m: &BTreeMap<u8, u8>) -> usize { m.len() }\n\
                     fn h(p: &str) -> usize { read(p) }\n";
        assert!(sites_of(clean).iter().all(|s| s.facts.pure_ok));
    }

    #[test]
    fn test_gated_imports_do_not_taint_other_code() {
        let src = "#[cfg(test)]\nmod tests { use std::fs; }\n\
                   fn f(p: &str) { let _ = fs::read(p); }\n";
        assert!(sites_of(src)[0].facts.pure_ok);
    }

    #[test]
    fn mut_self_method_is_not_pure_but_shared_self_is() {
        let sites =
            sites_of("struct S; impl S { fn bump(&mut self) {} fn get(&self) -> u8 { 1 } }\n");
        assert_eq!(
            names_and_flags(&sites),
            vec![("bump", false, false), ("get", true, true)]
        );
    }

    #[test]
    fn call_sites_are_collected_from_bodies() {
        let sites = sites_of("fn a() { b(); x.c(); crate::d(); }\nfn b() {}\n");
        let callees: Vec<&str> = sites[0].facts.callees.iter().map(String::as_str).collect();
        assert_eq!(callees, vec!["b", "c", "d"]);
    }

    #[test]
    fn parses_pairs_skipping_blanks_and_comments() {
        let text = "# header\n\n  wg-types StrictPure  \nwg-json Pure # trailing\n";
        let tiers = parse_tiers(text).unwrap();
        assert_eq!(tiers.len(), 2);
        assert_eq!(purity_tier(&tiers, "wg-types"), PurityTier::StrictPure);
        assert_eq!(purity_tier(&tiers, "wg-json"), PurityTier::Pure);
    }

    #[test]
    fn unlisted_crate_is_impure() {
        let tiers = parse_tiers("wg-types StrictPure\n").unwrap();
        assert_eq!(purity_tier(&tiers, "xtask"), PurityTier::Impure);
    }

    #[test]
    fn empty_file_is_an_empty_table() {
        assert!(parse_tiers("# nothing yet\n\n").unwrap().is_empty());
    }

    #[test]
    fn rejects_unknown_tier_with_line_number() {
        let err = parse_tiers("wg-types StrictPure\nwg-json Impure\n").unwrap_err();
        assert!(err.starts_with("line 2:"), "{err}");
        assert!(err.contains("unknown tier `Impure`"), "{err}");
    }

    #[test]
    fn rejects_duplicate_crate_naming_both_lines() {
        let err = parse_tiers("wg-types StrictPure\n\nwg-types Pure\n").unwrap_err();
        assert!(err.starts_with("line 3:"), "{err}");
        assert!(err.contains("duplicate crate `wg-types`"), "{err}");
        assert!(err.contains("first listed on line 1"), "{err}");
    }

    #[test]
    fn rejects_malformed_lines_with_line_number() {
        let one_field = parse_tiers("wg-types\n").unwrap_err();
        assert!(one_field.starts_with("line 1: expected"), "{one_field}");
        let three = parse_tiers("# c\nwg-types StrictPure extra\n").unwrap_err();
        assert!(three.starts_with("line 2: expected"), "{three}");
        let bad_chars = parse_tiers("ker/nel StrictPure\n").unwrap_err();
        assert!(
            bad_chars.starts_with("line 1: malformed crate name"),
            "{bad_chars}"
        );
    }

    #[test]
    fn committed_tier_file_parses() {
        let tiers = load_tiers(&workspace_root()).unwrap();
        assert_eq!(purity_tier(&tiers, "wg-types"), PurityTier::Core);
        assert_eq!(purity_tier(&tiers, "arch-lint"), PurityTier::Impure);
    }

    #[test]
    fn missing_tier_file_is_an_error() {
        let missing = Path::new("/nonexistent-workgraph-root-for-test");
        assert!(load_tiers(missing).is_err());
    }
    #[test]
    fn builtin_tooling_crates_need_no_marker() {
        let file = parse_tier_file("").unwrap();
        assert!(is_tooling(&file, "arch-lint"));
        assert!(!is_tooling(&file, "wg-server"));
    }

    #[test]
    fn legacy_is_no_longer_a_tier() {
        let err = parse_tier_file("wg-old Legacy\n").unwrap_err();
        assert!(err.contains("unknown tier `Legacy`"), "{err}");
    }

    #[test]
    fn labels_name_each_kind_of_crate() {
        let file = parse_tier_file("a Core\nb Pure\nc Shell\nd Tooling\n").unwrap();
        assert_eq!(tier_label(&file, "a"), "Core");
        assert_eq!(tier_label(&file, "b"), "Pure");
        assert_eq!(tier_label(&file, "c"), "Shell");
        assert_eq!(tier_label(&file, "d"), "Tooling");
        assert_eq!(tier_label(&file, "e"), "unlisted");
        assert_eq!(tier_label(&file, "arch-lint"), "Tooling");
    }

    #[test]
    fn baseline_parses_two_named_figures() {
        let text =
            "# c\nshell-pure-candidate-loc 376 # trailing\n\nshell-strict-candidate-loc 12\n";
        assert_eq!(
            parse_baseline(text).unwrap(),
            Ceilings {
                pure: 376,
                strict: 12
            }
        );
    }

    #[test]
    fn baseline_rejects_bad_files_with_line_numbers() {
        let bare = parse_baseline("21709\n").unwrap_err();
        assert!(
            bare.starts_with("line 1: expected `figure value`"),
            "{bare}"
        );
        let unknown = parse_baseline("total 1\n").unwrap_err();
        assert!(
            unknown.starts_with("line 1: unknown figure `total`"),
            "{unknown}"
        );
        let not_number = parse_baseline("shell-pure-candidate-loc lots\n").unwrap_err();
        assert!(
            not_number.starts_with("line 1: `lots` is not a number"),
            "{not_number}"
        );
        let twice =
            parse_baseline("shell-pure-candidate-loc 1\nshell-pure-candidate-loc 2\n").unwrap_err();
        assert!(twice.starts_with("line 2: duplicate figure"), "{twice}");
        let missing = parse_baseline("shell-pure-candidate-loc 1\n").unwrap_err();
        assert_eq!(missing, "no `shell-strict-candidate-loc` figure");
    }

    #[test]
    fn ceiling_check_passes_at_the_ceiling_and_names_each_figure_over_it() {
        let ceilings = Ceilings { pure: 5, strict: 3 };
        assert!(check_ceilings(ceilings, ceilings).is_ok());
        let err = check_ceilings(Ceilings { pure: 6, strict: 3 }, ceilings).unwrap_err();
        assert!(
            err.contains("shell-pure-candidate-loc 6 exceeds its ceiling 5"),
            "{err}"
        );
        assert!(!err.contains("shell-strict-candidate-loc 3"), "{err}");
        let err = check_ceilings(Ceilings { pure: 9, strict: 9 }, ceilings).unwrap_err();
        assert_eq!(err.matches("exceeds its ceiling").count(), 2, "{err}");
    }

    #[test]
    fn committed_baseline_parses() {
        assert!(read_baseline(&workspace_root()).is_ok());
    }

    #[test]
    fn parses_the_gate_flag() {
        assert_eq!(
            parse_report_args(&args(&["--gate"])).unwrap(),
            ReportMode::Gate
        );
    }

    // --- module lines -----------------------------------------------------

    #[test]
    fn module_lines_tier_the_modules_of_a_listed_crate() {
        let file = parse_tier_file("wg-a Shell\nwg-a::tasks Core\nwg-a::report StrictPure\n")
            .expect("parses");
        let modules = &file.modules["wg-a"];
        assert_eq!(modules["tasks"], Mark::Tier(PurityTier::Core));
        assert_eq!(modules["report"], Mark::Tier(PurityTier::StrictPure));
        assert!(file.tiers.is_empty(), "module lines are not crate lines");
        assert!(file.shell.contains("wg-a"));
    }

    #[test]
    fn module_line_errors_name_the_line() {
        let tooling = parse_tier_file("wg-a Shell\nwg-a::tasks Tooling\n").unwrap_err();
        assert!(tooling.starts_with("line 2:"), "{tooling}");
        assert!(tooling.contains("not `Tooling`"), "{tooling}");
        let no_crate = parse_tier_file("wg-a Shell\nwg-b::tasks Core\n").unwrap_err();
        assert!(no_crate.starts_with("line 2:"), "{no_crate}");
        assert!(no_crate.contains("has no crate line"), "{no_crate}");
        let bad_name = parse_tier_file("wg-a Shell\nwg-a::bad-name Core\n").unwrap_err();
        assert!(
            bad_name.contains("malformed module name `bad-name`"),
            "{bad_name}"
        );
        let twice = parse_tier_file("wg-a Shell\nwg-a::x Core\nwg-a::x Pure\n").unwrap_err();
        assert!(twice.contains("duplicate crate `wg-a::x`"), "{twice}");
    }

    #[test]
    fn module_of_maps_files_to_their_top_level_module() {
        let src = Path::new("crates/wg-a/src");
        let module = |file: &str| module_of(src, &Path::new("crates/wg-a/src").join(file));
        assert_eq!(module("tasks.rs").as_deref(), Some("tasks"));
        assert_eq!(module("tasks/mod.rs").as_deref(), Some("tasks"));
        assert_eq!(module("tasks/store/x.rs").as_deref(), Some("tasks"));
        assert_eq!(module("lib.rs"), None);
        assert_eq!(module("main.rs"), None);
        assert_eq!(module("bin/tool.rs"), None);
    }

    #[test]
    fn a_file_takes_its_module_line_over_the_crate_line() {
        let file = parse_tier_file("wg-a Shell\nwg-a::tasks Core\n").unwrap();
        assert_eq!(
            file_mark(&file, "wg-a", Some("tasks")),
            Some(Mark::Tier(PurityTier::Core))
        );
        assert_eq!(file_mark(&file, "wg-a", Some("other")), Some(Mark::Shell));
        assert_eq!(file_mark(&file, "wg-a", None), Some(Mark::Shell));
        assert_eq!(file_mark(&file, "wg-x", None), None);
    }

    /// Writes the fixture crates under a per-test directory of the temp dir.
    fn fixture_root(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let root = std::env::temp_dir().join("arch-lint-tests").join(name);
        let _ = fs::remove_dir_all(&root);
        for (path, text) in files {
            let full = root.join("crates").join(path);
            fs::create_dir_all(full.parent().expect("parent")).expect("mkdir");
            fs::write(full, text).expect("write");
        }
        root
    }

    /// Runs `scan` on a fixture with the given tier file text.
    fn scan_fixture(root: &Path, tiers: &str) -> Vec<String> {
        let file = parse_tier_file(tiers).expect("tier file parses");
        let rules = CoreRules {
            world_roots: Vec::new(),
            external_deps: vec!["rpds".to_string()],
            banned_calls: Vec::new(),
        };
        let mut violations = Vec::new();
        scan(root, &file, &rules, &mut violations);
        violations
    }

    /// A module-tiered fixture: `wg-base` is Core; `wg-a` is Shell at the root, with
    /// Core `tasks`, Pure `report` and Shell `shell` modules. `lib.rs` declares the
    /// three modules, and `tasks.rs` is conforming.
    fn module_fixture(name: &str, extra: &[(&str, &str)]) -> std::path::PathBuf {
        let mut files = vec![
            ("wg-base/Cargo.toml", "[package]\nname = \"wg-base\"\n"),
            ("wg-base/src/lib.rs", "pub fn id(v: u8) -> u8 {\n    v\n}\n"),
            (
                "wg-a/Cargo.toml",
                "[package]\nname = \"wg-a\"\n\n[dependencies]\nwg-base = { path = \"../wg-base\" }\n",
            ),
            (
                "wg-a/src/lib.rs",
                "pub mod report;\npub mod shell;\npub mod tasks;\n",
            ),
            (
                "wg-a/src/tasks.rs",
                "use wg_base::id;\n\npub fn next(v: u8) -> u8 {\n    id(v) + 1\n}\n",
            ),
            (
                "wg-a/src/report.rs",
                "pub fn total(v: &[u8]) -> u32 {\n    v.iter().map(|x| u32::from(*x)).sum()\n}\n",
            ),
            (
                "wg-a/src/shell.rs",
                "use std::fs;\n\npub fn read(path: &str) -> std::io::Result<Vec<u8>> {\n    fs::read(path)\n}\n",
            ),
        ];
        files.extend_from_slice(extra);
        fixture_root(name, &files)
    }

    const MODULE_TIERS: &str =
        "wg-base Core\nwg-a Shell\nwg-a::tasks Core\nwg-a::report Pure\nwg-a::shell Shell\n";

    #[test]
    fn module_tiered_crate_passes() {
        let root = module_fixture("module-tiered-passes", &[]);
        let found = scan_fixture(&root, MODULE_TIERS);
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn unlisted_module_fails() {
        let root = module_fixture(
            "unlisted-module-fails",
            &[
                (
                    "wg-a/src/lib.rs",
                    "pub mod report;\npub mod shell;\npub mod tasks;\npub mod extra;\n",
                ),
                ("wg-a/src/extra.rs", "pub fn extra() -> u8 {\n    1\n}\n"),
            ],
        );
        let found = scan_fixture(&root, MODULE_TIERS);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].contains("module `wg-a::extra` is not listed"),
            "{found:?}"
        );
    }

    #[test]
    fn listed_module_without_a_source_fails() {
        let root = module_fixture(
            "stale-module-fails",
            &[(
                "wg-a/src/lib.rs",
                "pub mod report;\npub mod shell;\npub mod tasks;\n",
            )],
        );
        let tiers = format!("{MODULE_TIERS}wg-a::gone Core\n");
        let found = scan_fixture(&root, &tiers);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("`wg-a::gone` is listed"), "{found:?}");
    }

    #[test]
    fn core_module_importing_a_shell_module_fails() {
        let root = module_fixture(
            "core-imports-shell-fails",
            &[(
                "wg-a/src/tasks.rs",
                "use crate::shell::read;\n\npub fn next(v: u8) -> u8 {\n    v\n}\n",
            )],
        );
        let found = scan_fixture(&root, MODULE_TIERS);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].starts_with("crates/wg-a/src/tasks.rs:1: Core rule 6:"),
            "{found:?}"
        );
        assert!(
            found[0].contains("module `shell`, which is not a Core module"),
            "{found:?}"
        );
    }

    #[test]
    fn pure_module_gets_the_pure_rules() {
        let root = module_fixture(
            "pure-module-rules",
            &[(
                "wg-a/src/report.rs",
                "pub fn now() -> bool {\n    std::env::var(\"X\").is_ok()\n}\n",
            )],
        );
        let found = scan_fixture(&root, MODULE_TIERS);
        assert!(
            found
                .iter()
                .any(|line| line.starts_with("crates/wg-a/src/report.rs:")
                    && line.contains("Pure purity violation")),
            "{found:?}"
        );
        assert!(
            found.iter().all(|line| !line.contains("Core rule")),
            "{found:?}"
        );
    }

    #[test]
    fn shell_ceiling_counts_only_shell_modules() {
        // `clean` in the Shell module is a pure candidate (3 code lines); `next` in
        // the Core module is also pure but is not Shell, so the ceiling skips it.
        let root = module_fixture(
            "ceiling-counts-shell-modules",
            &[(
                "wg-a/src/shell.rs",
                "pub fn clean(a: u8) -> u8 {\n    a + 1\n}\n",
            )],
        );
        let file = parse_tier_file(MODULE_TIERS).expect("parses");
        let measured = shell_candidate_loc(&root, &file, &BTreeSet::new());
        assert_eq!(measured.pure, 3);
        assert_eq!(measured.strict, 3);
    }
}
