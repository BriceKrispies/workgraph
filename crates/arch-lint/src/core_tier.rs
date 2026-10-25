//! The `Core` purity tier: the structural rules that sit on top of every
//! `StrictPure` rule. `purity::scan` runs [`check_crate`] for each crate listed as
//! `Core` in `tools/reports/purity-tiers.txt`.
//!
//! Copied from `C:\dev\noether\crates\arch-lint\src\core_tier.rs` and adapted
//! (docs/rust-rewrite.md section 4.6). The noether-specific parts are gone: the world
//! types, the external dependency allowlist and the banned calls come from
//! `tools/reports/core-rules.toml` as a [`CoreRules`] value, and the `TenantId` key
//! rule is dropped. To extract a shared crate later, move this file and pass
//! [`CoreRules`] in unchanged.
//!
//! The rules:
//!
//! 1. no `dyn` trait object in non-test code;
//! 2. no `pub trait` (private helper traits are allowed);
//! 3. no non-persistent collection (`std::collections`, `Vec`, ...) inside a type
//!    reachable from a world root named in the rules file;
//! 4. no call to a banned function (clock and entropy reach the core only through
//!    `Stamp`), matched by path suffix such as `Instant::now`;
//! 5. a dependency allowlist: tier-listed workspace crates plus the rules file's
//!    `external_deps`.
//!
//! The unlisted-crate rule is `purity::unlisted_crate_violations`.
//!
//! Test-gated code is exempt, as in the other scans. Rule 3 does not see `String`
//! buffers built by mutation: a mutable `String` is caught only where `StrictPure`
//! catches the `mut` binding or assignment.

use std::collections::{BTreeMap, BTreeSet};

use quote::ToTokens;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};

use crate::is_test_gated;
use crate::purity::{TierTable, impl_item_attrs, item_attrs, trait_item_attrs};

/// The `Core` rules that differ between repositories, read from
/// `tools/reports/core-rules.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CoreRules {
    /// World types whose reachable field types must hold only persistent values.
    pub(crate) world_roots: Vec<String>,
    /// Crates outside the workspace that a `Core` crate may depend on (rule 5).
    pub(crate) external_deps: Vec<String>,
    /// Paths a `Core` crate may not call, such as `Instant::now` (rule 4).
    pub(crate) banned_calls: Vec<String>,
}

/// Repo-relative rules file.
pub(crate) const RULES_PATH: &str = "tools/reports/core-rules.toml";

/// Parses the rules file. The file is a TOML subset: `key = ["a", "b"]` string
/// arrays that may span lines, `#` comments and blank lines. All three keys are
/// required, each exactly once. Errors name the key.
pub(crate) fn parse_rules(text: &str) -> Result<CoreRules, String> {
    let mut stripped = String::new();
    for raw in text.lines() {
        let mut in_string = false;
        for c in raw.chars() {
            match c {
                '"' => in_string = !in_string,
                '#' if !in_string => break,
                _ => {}
            }
            stripped.push(c);
        }
        stripped.push('\n');
    }
    let mut world_roots = None;
    let mut external_deps = None;
    let mut banned_calls = None;
    let mut rest = stripped.trim();
    while !rest.is_empty() {
        let (key, after_key) = rest
            .split_once('=')
            .ok_or_else(|| format!("expected `key = [..]`, found `{rest}`"))?;
        let key = key.trim();
        let body = after_key
            .trim_start()
            .strip_prefix('[')
            .ok_or_else(|| format!("`{key}` is not an array"))?;
        let (items, tail) = body
            .split_once(']')
            .ok_or_else(|| format!("`{key}` array is not closed"))?;
        let values = parse_string_array(key, items)?;
        let slot = match key {
            "world_roots" => &mut world_roots,
            "external_deps" => &mut external_deps,
            "banned_calls" => &mut banned_calls,
            other => {
                return Err(format!(
                    "unknown key `{other}` (expected `world_roots`, `external_deps` or `banned_calls`)"
                ));
            }
        };
        if slot.is_some() {
            return Err(format!("duplicate key `{key}`"));
        }
        *slot = Some(values);
        rest = tail.trim();
    }
    let missing = |name: &str| format!("no `{name}` key");
    Ok(CoreRules {
        world_roots: world_roots.ok_or_else(|| missing("world_roots"))?,
        external_deps: external_deps.ok_or_else(|| missing("external_deps"))?,
        banned_calls: banned_calls.ok_or_else(|| missing("banned_calls"))?,
    })
}

fn parse_string_array(key: &str, items: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for item in items.split(',').map(str::trim).filter(|i| !i.is_empty()) {
        let value = item
            .strip_prefix('"')
            .and_then(|i| i.strip_suffix('"'))
            .ok_or_else(|| format!("`{key}`: `{item}` is not a quoted string"))?;
        out.push(value.to_string());
    }
    Ok(out)
}

/// Reads and parses the rules file under `root`. A missing file is an error.
pub(crate) fn load_rules(root: &std::path::Path) -> Result<CoreRules, String> {
    let path = root.join(RULES_PATH);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse_rules(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Non-persistent collection type names (rule 3).
const COLLECTIONS: &[&str] = &[
    "Vec",
    "VecDeque",
    "LinkedList",
    "BinaryHeap",
    "HashMap",
    "HashSet",
    "BTreeMap",
    "BTreeSet",
];

/// One `Core` finding, rendered as `path:line: Core rule N: text`.
#[derive(Debug, PartialEq, Eq)]
struct Finding {
    path: String,
    line: usize,
    rule: u8,
    text: String,
}

impl Finding {
    fn render(&self) -> String {
        format!(
            "{}:{}: Core rule {}: {}",
            self.path, self.line, self.rule, self.text
        )
    }
}

/// Checks one `Core` crate: `files` are its parsed `src` files with their
/// repo-relative paths, `manifest` is its `Cargo.toml` text, `tiers` names the
/// workspace crates that are tier-listed, and `rules` is the rules file. Returns
/// one rendered line per finding.
pub(crate) fn check_crate(
    krate: &str,
    files: &[(String, syn::File)],
    manifest: &str,
    tiers: &TierTable,
    rules: &CoreRules,
) -> Vec<String> {
    let mut findings = Vec::new();
    for (path, ast) in files.iter().filter(|f| !is_test_gated(&f.1.attrs)) {
        let mut scan = CoreScan {
            path: path.as_str(),
            banned: &rules.banned_calls,
            found: Vec::new(),
        };
        scan.visit_file(ast);
        findings.extend(scan.found);
    }
    findings.extend(collection_findings(files, rules));
    findings.extend(dependency_findings(krate, manifest, tiers, rules));
    findings.iter().map(Finding::render).collect()
}

// --- rules 1, 2 and 4: the syntax walk ---------------------------------------

/// Walks non-test code for `dyn` (rule 1), `pub trait` (rule 2) and banned calls
/// (rule 4).
struct CoreScan<'a> {
    path: &'a str,
    banned: &'a [String],
    found: Vec<Finding>,
}

impl CoreScan<'_> {
    fn flag(&mut self, rule: u8, line: usize, text: String) {
        self.found.push(Finding {
            path: self.path.to_string(),
            line,
            rule,
            text,
        });
    }
}

impl<'ast> Visit<'ast> for CoreScan<'_> {
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

    fn visit_item_trait(&mut self, node: &'ast syn::ItemTrait) {
        if matches!(node.vis, syn::Visibility::Public(_)) {
            let line = node.span().start().line;
            let text = format!(
                "`pub trait {}`: state must not sit behind a port",
                node.ident
            );
            self.flag(2, line, text);
        }
        visit::visit_item_trait(self, node);
    }

    fn visit_type_trait_object(&mut self, node: &'ast syn::TypeTraitObject) {
        let line = node.span().start().line;
        self.flag(1, line, "`dyn` trait object".to_string());
        visit::visit_type_trait_object(self, node);
    }

    fn visit_path(&mut self, node: &'ast syn::Path) {
        let segments: Vec<String> = node.segments.iter().map(|s| s.ident.to_string()).collect();
        let joined = segments.join("::");
        for banned in self.banned {
            if joined == *banned || joined.ends_with(&format!("::{banned}")) {
                let line = node.span().start().line;
                let text = format!("`{banned}`: clock and entropy come in through `Stamp`");
                self.flag(4, line, text);
            }
        }
        visit::visit_path(self, node);
    }
}

/// Identifier tokens of a syntax node, split on anything that is not part of a name.
fn idents(node: &impl ToTokens) -> Vec<String> {
    node.to_token_stream()
        .to_string()
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect()
}

// --- rule 3: no non-persistent collection in a world-reachable type ---------

/// A non-test `struct`, `enum` or `type` item and the field types it holds.
struct TypeDef {
    name: String,
    path: String,
    types: Vec<syn::Type>,
}

/// Collects every non-test type definition in one file.
struct DefCollector {
    path: String,
    defs: Vec<TypeDef>,
}

impl DefCollector {
    fn define(&mut self, name: String, types: Vec<syn::Type>) {
        self.defs.push(TypeDef {
            name,
            path: self.path.clone(),
            types,
        });
    }
}

impl<'ast> Visit<'ast> for DefCollector {
    fn visit_item(&mut self, node: &'ast syn::Item) {
        if is_test_gated(item_attrs(node)) {
            return;
        }
        match node {
            syn::Item::Struct(s) => {
                let types = s.fields.iter().map(|f| f.ty.clone()).collect();
                self.define(s.ident.to_string(), types);
            }
            syn::Item::Enum(e) => {
                let types = e
                    .variants
                    .iter()
                    .flat_map(|v| v.fields.iter().map(|f| f.ty.clone()))
                    .collect();
                self.define(e.ident.to_string(), types);
            }
            syn::Item::Type(t) => self.define(t.ident.to_string(), vec![(*t.ty).clone()]),
            _ => {}
        }
        visit::visit_item(self, node);
    }
}

/// Every type name a world type reaches, starting from the world roots that the
/// crate defines and following the names that appear in field types.
fn reachable<'a>(defs: &'a [TypeDef], rules: &'a CoreRules) -> BTreeSet<&'a str> {
    let mut by_name: BTreeMap<&str, Vec<&TypeDef>> = BTreeMap::new();
    for def in defs {
        by_name.entry(def.name.as_str()).or_default().push(def);
    }
    let known: BTreeSet<&str> = by_name.keys().copied().collect();
    let mut reached: BTreeSet<&str> = rules
        .world_roots
        .iter()
        .map(String::as_str)
        .filter(|root| known.contains(root))
        .collect();
    let mut queue: Vec<&str> = reached.iter().copied().collect();
    while let Some(name) = queue.pop() {
        for def in by_name.get(name).into_iter().flatten() {
            for ty in &def.types {
                for ident in idents(ty) {
                    if let Some(&hit) = known.get(ident.as_str())
                        && reached.insert(hit)
                    {
                        queue.push(hit);
                    }
                }
            }
        }
    }
    reached
}

fn collection_findings(files: &[(String, syn::File)], rules: &CoreRules) -> Vec<Finding> {
    let mut defs = Vec::new();
    for (path, ast) in files {
        let mut collector = DefCollector {
            path: path.clone(),
            defs: Vec::new(),
        };
        collector.visit_file(ast);
        defs.extend(collector.defs);
    }
    let reached = reachable(&defs, rules);
    let mut out = Vec::new();
    for def in defs.iter().filter(|d| reached.contains(d.name.as_str())) {
        for ty in &def.types {
            let mut scan = BannedCollections {
                def: def.name.as_str(),
                path: def.path.as_str(),
                found: Vec::new(),
            };
            scan.visit_type(ty);
            out.extend(scan.found);
        }
    }
    out
}

/// Flags a non-persistent collection named inside a world-reachable type.
struct BannedCollections<'a> {
    def: &'a str,
    path: &'a str,
    found: Vec<Finding>,
}

impl<'ast> Visit<'ast> for BannedCollections<'_> {
    fn visit_type_path(&mut self, node: &'ast syn::TypePath) {
        let segments: Vec<String> = node
            .path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect();
        let last = segments.last().cloned().unwrap_or_default();
        let what = if segments.iter().any(|s| s == "collections") {
            Some(format!("`{}`", segments.join("::")))
        } else if COLLECTIONS.contains(&last.as_str()) {
            Some(format!("`{last}`"))
        } else {
            None
        };
        if let Some(what) = what {
            self.found.push(Finding {
                path: self.path.to_string(),
                line: node.span().start().line,
                rule: 3,
                text: format!(
                    "non-persistent collection {what} in `{}`, which a world type reaches",
                    self.def
                ),
            });
        }
        visit::visit_type_path(self, node);
    }
}

// --- rule 5: the dependency allowlist ---------------------------------------

fn dependency_findings(
    krate: &str,
    manifest: &str,
    tiers: &TierTable,
    rules: &CoreRules,
) -> Vec<Finding> {
    manifest_dependencies(manifest)
        .into_iter()
        .filter(|(_, name)| !rules.external_deps.contains(name) && !tiers.contains_key(name))
        .map(|(line, name)| Finding {
            path: format!("crates/{krate}/Cargo.toml"),
            line,
            rule: 5,
            text: format!(
                "dependency `{name}` is not on the Core allowlist \
                 (tier-listed workspace crates, {})",
                rules.external_deps.join(", ")
            ),
        })
        .collect()
}

/// The `(line, name)` of every dependency in a manifest's `dependencies`,
/// `dev-dependencies` and `build-dependencies` tables, including the `[..dependencies.name]`
/// and `[target.'cfg(..)'.dependencies]` forms.
fn manifest_dependencies(manifest: &str) -> Vec<(usize, String)> {
    const SECTIONS: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];
    let mut out = Vec::new();
    let mut in_deps = false;
    for (index, raw) in manifest.lines().enumerate() {
        let line_no = index + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(header) = line.strip_prefix('[') {
            let header = header.trim_end_matches(']').trim();
            let single_table = SECTIONS.iter().find_map(|section| {
                header
                    .strip_prefix(section)
                    .and_then(|rest| rest.strip_prefix('.'))
            });
            if let Some(name) = single_table {
                out.push((line_no, name.trim_matches('"').to_string()));
                in_deps = false;
            } else {
                in_deps = SECTIONS.contains(&header)
                    || (header.starts_with("target.")
                        && SECTIONS
                            .iter()
                            .any(|section| header.ends_with(&format!(".{section}"))));
            }
            continue;
        }
        if let (true, Some((key, _))) = (in_deps, line.split_once('=')) {
            let name = key.split('.').next().unwrap_or_default().trim();
            let name = name.trim_matches('"');
            if !name.is_empty() {
                out.push((line_no, name.to_string()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::purity::{PurityTier, parse_tier_file, unlisted_crate_violations};

    fn parse_files(sources: &[(&str, &str)]) -> Vec<(String, syn::File)> {
        sources
            .iter()
            .map(|(path, src)| {
                let ast = syn::parse_file(src).expect("fixture parses");
                ((*path).to_string(), ast)
            })
            .collect()
    }

    /// The committed rules file: fixtures take their world type names from it.
    fn rules() -> CoreRules {
        load_rules(&crate::workspace_root()).expect("committed rules file parses")
    }

    /// First and second world roots of the committed rules file.
    fn roots() -> (String, String) {
        let rules = rules();
        (rules.world_roots[0].clone(), rules.world_roots[1].clone())
    }

    /// Tier table naming `wg-types` and `wg-json` as tier-listed crates.
    fn tiers() -> TierTable {
        ["wg-types", "wg-json"]
            .iter()
            .map(|name| ((*name).to_string(), PurityTier::StrictPure))
            .collect()
    }

    fn check(krate: &str, sources: &[(&str, &str)], manifest: &str) -> Vec<String> {
        check_crate(krate, &parse_files(sources), manifest, &tiers(), &rules())
    }

    /// How many findings name `rule`.
    fn hits(found: &[String], rule: u8) -> usize {
        let tag = format!("Core rule {rule}:");
        found.iter().filter(|line| line.contains(&tag)).count()
    }

    const NO_DEPS: &str = "[package]\nname = \"fixture\"\n";

    // The rules file.

    #[test]
    fn committed_rules_file_parses() {
        let rules = rules();
        assert!(rules.world_roots.iter().any(|r| r == "TaskSlice"));
        assert!(rules.world_roots.iter().any(|r| r == "StackFlow"));
        assert!(rules.external_deps.iter().any(|d| d == "rpds"));
        assert!(rules.banned_calls.iter().any(|c| c == "Instant::now"));
    }

    #[test]
    fn rules_parse_multiline_arrays_and_comments() {
        let text = "# c\nworld_roots = [\"A\", # trailing\n  \"B\"]\n\
                    external_deps = []\nbanned_calls = [\"X::y\"] # t\n";
        let rules = parse_rules(text).expect("parses");
        assert_eq!(rules.world_roots, vec!["A", "B"]);
        assert!(rules.external_deps.is_empty());
        assert_eq!(rules.banned_calls, vec!["X::y"]);
    }

    #[test]
    fn rules_reject_bad_files_naming_the_key() {
        let missing = parse_rules("world_roots = []\nexternal_deps = []\n").unwrap_err();
        assert_eq!(missing, "no `banned_calls` key");
        let unknown = parse_rules("frob = []\n").unwrap_err();
        assert!(unknown.contains("unknown key `frob`"), "{unknown}");
        let twice = parse_rules("world_roots = []\nworld_roots = []\n").unwrap_err();
        assert!(twice.contains("duplicate key `world_roots`"), "{twice}");
        let bare = parse_rules("world_roots = [A]\n").unwrap_err();
        assert!(bare.contains("not a quoted string"), "{bare}");
        let open = parse_rules("world_roots = [\"A\"\n").unwrap_err();
        assert!(open.contains("not closed"), "{open}");
    }

    // Rule 1: no `dyn`.

    #[test]
    fn rule1_fires_on_dyn_in_non_test_code() {
        let src = "pub fn run(f: Box<dyn Fn() -> u8>) -> u8 { f() }\n";
        let found = check("wg-x", &[("crates/wg-x/src/lib.rs", src)], NO_DEPS);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].starts_with("crates/wg-x/src/lib.rs:1: Core rule 1:"));
    }

    #[test]
    fn rule1_silent_on_impl_trait_and_test_only_dyn() {
        let src = "pub fn run(f: impl Fn() -> u8) -> u8 { f() }\n\
                   #[cfg(test)]\nmod tests { fn helper(_: Box<dyn Fn()>) {} }\n\
                   #[test]\nfn t() { let _: Box<dyn Fn()> = Box::new(|| ()); }\n";
        let found = check("wg-x", &[("crates/wg-x/src/lib.rs", src)], NO_DEPS);
        assert_eq!(hits(&found, 1), 0, "{found:?}");
    }

    // Rule 2: no `pub trait`.

    #[test]
    fn rule2_fires_on_pub_trait() {
        let src = "pub trait Port { fn get(&self) -> u8; }\n";
        let found = check("wg-x", &[("crates/wg-x/src/lib.rs", src)], NO_DEPS);
        assert_eq!(hits(&found, 2), 1, "{found:?}");
    }

    #[test]
    fn rule2_silent_on_private_and_crate_visible_traits() {
        let src = "trait Helper { fn get(&self) -> u8; }\n\
                   pub(crate) trait Local { fn get(&self) -> u8; }\n\
                   #[cfg(test)]\npub trait Fixture {}\n";
        let found = check("wg-x", &[("crates/wg-x/src/lib.rs", src)], NO_DEPS);
        assert_eq!(hits(&found, 2), 0, "{found:?}");
    }

    // Rule 3: no non-persistent collection reachable from a world type.

    #[test]
    fn rule3_fires_on_vec_in_world_type() {
        let (task, _) = roots();
        let src = format!("pub struct {task} {{ records: Vec<u8> }}\n");
        let found = check("wg-x", &[("crates/wg-x/src/lib.rs", &src)], NO_DEPS);
        assert_eq!(hits(&found, 3), 1, "{found:?}");
    }

    #[test]
    fn rule3_fires_through_a_reachable_field_and_std_collections_paths() {
        let (task, _) = roots();
        let src = format!(
            "pub struct {task} {{ inner: Inner }}\n\
             struct Inner {{ index: Option<std::collections::BTreeMap<u8, u8>> }}\n"
        );
        let found = check("wg-x", &[("crates/wg-x/src/lib.rs", &src)], NO_DEPS);
        assert_eq!(hits(&found, 3), 1, "{found:?}");
        assert!(found[0].contains("`Inner`"), "{found:?}");
    }

    #[test]
    fn rule3_follows_reachability_across_files() {
        let (_, flow) = roots();
        let world = format!("pub struct {flow} {{ pub tasks: tasks::Table }}\n");
        let table = "pub struct Table { rows: HashSet<u8> }\n";
        let found = check(
            "wg-x",
            &[
                ("crates/wg-x/src/lib.rs", &world),
                ("crates/wg-x/src/tasks.rs", table),
            ],
            NO_DEPS,
        );
        assert_eq!(hits(&found, 3), 1, "{found:?}");
        assert!(
            found[0].starts_with("crates/wg-x/src/tasks.rs:1:"),
            "{found:?}"
        );
    }

    #[test]
    fn rule3_silent_on_unreachable_types_persistent_collections_and_test_items() {
        let (task, _) = roots();
        let src = format!(
            "pub struct {task} {{ rows: rpds::HashTrieMap<u8, u8> }}\n\
             struct Scratch {{ buffer: Vec<u8> }}\n\
             pub fn build(v: Vec<u8>) -> Vec<u8> {{ v }}\n\
             #[cfg(test)]\nstruct Probe {{ seen: Vec<u8> }}\n"
        );
        let found = check("wg-x", &[("crates/wg-x/src/lib.rs", &src)], NO_DEPS);
        assert_eq!(hits(&found, 3), 0, "{found:?}");
    }

    // Rule 4: banned calls.

    #[test]
    fn rule4_fires_on_each_banned_call_path() {
        let src = "pub fn a() { let _ = OffsetDateTime::now_utc(); }\n\
                   pub fn b() { let _ = time::OffsetDateTime::now_utc(); }\n\
                   pub fn c() { let _ = std::time::SystemTime::now(); }\n\
                   pub fn d() { let _ = std::time::Instant::now(); }\n\
                   pub fn e() { let _ = uuid::Uuid::new_v4(); }\n";
        let found = check("wg-x", &[("crates/wg-x/src/lib.rs", src)], NO_DEPS);
        assert_eq!(hits(&found, 4), 5, "{found:?}");
    }

    #[test]
    fn rule4_silent_on_similar_names_and_test_code() {
        let src = "pub fn a(now: u8) -> u8 { now }\n\
                   pub fn b() { let _ = Stamp::now_utc_of(1); }\n\
                   pub fn c() { let _ = MyInstant::now(); }\n\
                   #[cfg(test)]\nmod tests { fn t() { let _ = std::time::Instant::now(); } }\n";
        let found = check("wg-x", &[("crates/wg-x/src/lib.rs", src)], NO_DEPS);
        assert_eq!(hits(&found, 4), 0, "{found:?}");
    }

    // Rule 5: the dependency allowlist.

    #[test]
    fn rule5_fires_on_a_dependency_outside_the_allowlist() {
        let manifest = "[package]\nname = \"fixture\"\n\n\
                        [dependencies]\n\
                        rpds = { workspace = true }\n\
                        wg-types.workspace = true\n\
                        tokio = \"1\"\n\n\
                        [dev-dependencies.proptest]\nversion = \"1\"\n\n\
                        [lints]\nworkspace = true\n";
        let found = check("wg-x", &[], manifest);
        assert_eq!(hits(&found, 5), 2, "{found:?}");
        assert!(found.iter().any(|f| f.contains("`tokio`")), "{found:?}");
        assert!(found.iter().any(|f| f.contains("`proptest`")), "{found:?}");
        assert!(found[0].starts_with("crates/wg-x/Cargo.toml:"), "{found:?}");
    }

    #[test]
    fn rule5_silent_on_allowlisted_and_tier_listed_dependencies() {
        let manifest = "[dependencies]\nrpds = \"1\"\nserde = { workspace = true }\n\
                        serde_json = { workspace = true }\n\
                        wg-json = { path = \"../wg-json\" }\n\
                        [target.'cfg(unix)'.dependencies]\nwg-types = { workspace = true }\n";
        let found = check("wg-x", &[], manifest);
        assert_eq!(hits(&found, 5), 0, "{found:?}");
    }

    // A conforming fixture crate passes every rule.

    #[test]
    fn conforming_fixture_crate_is_silent() {
        let (task, flow) = roots();
        let lib = format!(
            "pub struct {task} {{ tasks: Tasks }}\n\
             pub struct {flow} {{ log: Log }}\n\
             pub struct Tasks {{ rows: rpds::HashTrieMap<u8, Row> }}\n\
             pub struct Log {{ entries: rpds::Vector<u8> }}\n\
             pub struct Row {{ id: u8 }}\n\
             pub enum Event {{ Created(Row) }}\n\
             impl Row {{ pub fn id(&self) -> u8 {{ self.id }} }}\n\
             pub fn step(w: impl Fn(u8) -> u8) -> u8 {{ w(1) }}\n"
        );
        let other = "pub(crate) trait Clock { fn now(&self) -> u8; }\n\
                     pub fn names(v: Vec<u8>) -> Vec<u8> { v }\n";
        let found = check(
            "wg-x",
            &[
                ("crates/wg-x/src/lib.rs", &lib),
                ("crates/wg-x/src/clock.rs", other),
            ],
            "[dependencies]\nrpds = \"1\"\nwg-types = { workspace = true }\n",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    // The unlisted-crate rule, and the tier-file parse for `Core`.

    #[test]
    fn core_is_a_tier_mark() {
        let file = parse_tier_file("wg-model Core\n").expect("parses");
        assert_eq!(file.tiers.get("wg-model"), Some(&PurityTier::Core));
        assert!(PurityTier::Core > PurityTier::StrictPure);
    }

    #[test]
    fn unlisted_crate_fails_and_listed_or_builtin_crates_pass() {
        let file = parse_tier_file("wg-types StrictPure\nwg-cli Shell\nwg-conformance Tooling\n")
            .expect("parses");
        let names: Vec<String> = [
            "wg-types",
            "wg-cli",
            "wg-conformance",
            "wg-new",
            "arch-lint",
        ]
        .iter()
        .map(|name| (*name).to_string())
        .collect();
        let found = unlisted_crate_violations(&names, &file);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("`wg-new`"), "{found:?}");
    }
}
