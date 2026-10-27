//! I/O through shell types, for the shell pure-candidate analysis.
//!
//! A shell function that runs git, spawns a process or talks to a session does its I/O in a
//! method of a type that lives in a `Shell` crate or module (`Git`, `Proc`, `Session`, `App`),
//! not in a `std::fs` or `std::process` call the Pure rules see. Without this rule such a
//! function looks pure, and adding an artificial `&mut` parameter was the only way to hide
//! it. This pass is syntactic: it treats a function as doing I/O when it
//!
//! * calls a method on a binding whose declared type (a parameter or `let` annotation, or a
//!   `Type::ctor(..)` / `Type { .. }` initializer) is a type defined in a `Shell` source, or
//! * calls or names a lower-case function through a shell type (`Git::default_branch`) or a path
//!   into a `Shell` crate or module (`wg_proc::run_capture`).
//!
//! Building a shell value with a struct literal counts as well. It does not infer types through field accesses, closure parameters without annotations or
//! return values, so it errs towards "pure" wherever the receiver is not clearly a shell type.

use std::collections::{BTreeMap, BTreeSet};

use syn::visit::{self, Visit};

use crate::purity::UseMap;

/// The shell-tier names of a workspace.
#[derive(Debug, Default, Clone)]
pub(crate) struct ShellNames {
    /// Struct, enum, union, trait and alias names defined in `Shell` sources and in no other
    /// source.
    pub(crate) types: BTreeSet<String>,
    /// Crates (as Rust identifiers) whose crate line is `Shell`.
    pub(crate) crates: BTreeSet<String>,
    /// Per module-tiered crate (identifier), its `Shell` modules.
    pub(crate) shell_modules: BTreeMap<String, BTreeSet<String>>,
    /// Per module-tiered crate (identifier), its listed modules that are not `Shell`.
    pub(crate) other_modules: BTreeMap<String, BTreeSet<String>>,
}

impl ShellNames {
    /// True if a path's leading segments name a `Shell` crate or one of its `Shell` modules.
    fn path_in_shell(&self, segs: &[String]) -> bool {
        let [krate, rest @ ..] = segs else {
            return false;
        };
        let second = rest.first();
        if let Some(shell) = self.shell_modules.get(krate)
            && second.is_some_and(|m| shell.contains(m))
        {
            return true;
        }
        self.crates.contains(krate)
            && !second.is_some_and(|m| {
                self.other_modules
                    .get(krate)
                    .is_some_and(|others| others.contains(m))
            })
    }
}

/// Names of the type items (struct, enum, union, trait, alias) at any depth of `ast`, outside
/// test-gated code.
pub(crate) fn defined_types(ast: &syn::File) -> BTreeSet<String> {
    struct Defs(BTreeSet<String>);
    impl<'ast> Visit<'ast> for Defs {
        fn visit_item(&mut self, node: &'ast syn::Item) {
            if crate::is_test_gated(crate::purity::item_attrs(node)) {
                return;
            }
            let name = match node {
                syn::Item::Struct(i) => Some(&i.ident),
                syn::Item::Enum(i) => Some(&i.ident),
                syn::Item::Union(i) => Some(&i.ident),
                syn::Item::Trait(i) => Some(&i.ident),
                syn::Item::Type(i) => Some(&i.ident),
                _ => None,
            };
            if let Some(name) = name {
                self.0.insert(name.to_string());
            }
            visit::visit_item(self, node);
        }
    }
    let mut defs = Defs(BTreeSet::new());
    defs.visit_file(ast);
    defs.0
}

/// True if a function does I/O through a shell type, per the module rules.
pub(crate) fn does_shell_io(
    sig: &syn::Signature,
    block: &syn::Block,
    names: &ShellNames,
    uses: &UseMap,
) -> bool {
    let mut scan = IoScan {
        names,
        uses,
        bound: BTreeSet::new(),
        found: false,
    };
    scan.visit_signature(sig);
    scan.visit_block(block);
    scan.found
}

struct IoScan<'a> {
    names: &'a ShellNames,
    uses: &'a UseMap,
    /// Bindings whose type is a shell type.
    bound: BTreeSet<String>,
    found: bool,
}

/// The last path segment of a type, through references and parentheses.
fn type_head(ty: &syn::Type) -> Option<String> {
    match ty {
        syn::Type::Reference(r) => type_head(&r.elem),
        syn::Type::Paren(p) => type_head(&p.elem),
        syn::Type::Group(g) => type_head(&g.elem),
        syn::Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
        _ => None,
    }
}

fn path_segments(path: &syn::Path) -> Vec<String> {
    path.segments.iter().map(|s| s.ident.to_string()).collect()
}

fn ident_of(pat: &syn::Pat) -> Option<String> {
    match pat {
        syn::Pat::Ident(i) => Some(i.ident.to_string()),
        syn::Pat::Reference(r) => ident_of(&r.pat),
        syn::Pat::Paren(p) => ident_of(&p.pat),
        _ => None,
    }
}

impl IoScan<'_> {
    fn is_shell_type(&self, name: &str) -> bool {
        self.names.types.contains(name)
    }

    /// True if an expression path as written, or as the imports resolve it, goes through a
    /// shell type or a shell crate or module: `Git::default_branch`, `wg_proc::run_capture`.
    fn shell_path(&self, segs: &[String]) -> bool {
        self.uses.resolve(segs).iter().any(|full| {
            full.len() >= 2
                && (self.names.path_in_shell(full) || self.is_shell_type(&full[full.len() - 2]))
        })
    }

    /// True if an expression constructs a shell type: `T::ctor(..)`, `T::ctor(..)?` or `T { .. }`.
    fn constructs_shell(&self, expr: &syn::Expr) -> bool {
        match expr {
            syn::Expr::Struct(s) => s
                .path
                .segments
                .last()
                .is_some_and(|seg| self.is_shell_type(&seg.ident.to_string())),
            syn::Expr::Call(c) => match &*c.func {
                syn::Expr::Path(p) => {
                    let segs = path_segments(&p.path);
                    segs.len() >= 2 && self.is_shell_type(&segs[segs.len() - 2])
                }
                _ => false,
            },
            syn::Expr::Try(t) => self.constructs_shell(&t.expr),
            syn::Expr::Paren(p) => self.constructs_shell(&p.expr),
            syn::Expr::Reference(r) => self.constructs_shell(&r.expr),
            _ => false,
        }
    }

    fn receiver_is_shell(&self, expr: &syn::Expr) -> bool {
        match expr {
            syn::Expr::Path(p) => p
                .path
                .get_ident()
                .is_some_and(|id| self.bound.contains(&id.to_string())),
            syn::Expr::Paren(p) => self.receiver_is_shell(&p.expr),
            syn::Expr::Reference(r) => self.receiver_is_shell(&r.expr),
            syn::Expr::Unary(u) => self.receiver_is_shell(&u.expr),
            other => self.constructs_shell(other),
        }
    }
}

impl<'ast> Visit<'ast> for IoScan<'_> {
    fn visit_pat_type(&mut self, node: &'ast syn::PatType) {
        if let (Some(name), Some(head)) = (ident_of(&node.pat), type_head(&node.ty))
            && self.is_shell_type(&head)
        {
            self.bound.insert(name);
        }
        visit::visit_pat_type(self, node);
    }

    fn visit_local(&mut self, node: &'ast syn::Local) {
        if let (Some(name), Some(init)) = (ident_of(&node.pat), &node.init)
            && self.constructs_shell(&init.expr)
        {
            self.bound.insert(name);
        }
        visit::visit_local(self, node);
    }

    fn visit_expr_struct(&mut self, node: &'ast syn::ExprStruct) {
        // Building the I/O handle (`Git { path, .. }`) is shell glue too.
        if self.constructs_shell(&syn::Expr::Struct(node.clone())) {
            self.found = true;
        }
        visit::visit_expr_struct(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if self.receiver_is_shell(&node.receiver) {
            self.found = true;
        }
        visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
        let segs = path_segments(&node.path);
        // A lower-case last segment is a function or method, not an enum variant or constant.
        let function = segs
            .last()
            .is_some_and(|s| s.starts_with(|c: char| c.is_lowercase() || c == '_'));
        if function && self.shell_path(&segs) {
            self.found = true;
        }
        visit::visit_expr_path(self, node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> ShellNames {
        ShellNames {
            types: ["Git", "Session"]
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            crates: BTreeSet::from(["wg_proc".to_string()]),
            shell_modules: BTreeMap::from([(
                "wg_feat".to_string(),
                BTreeSet::from(["service".to_string()]),
            )]),
            other_modules: BTreeMap::from([(
                "wg_feat".to_string(),
                BTreeSet::from(["spec".to_string()]),
            )]),
        }
    }

    /// Whether the first function of `src` does shell I/O.
    fn io(src: &str) -> bool {
        let ast = syn::parse_file(src).unwrap();
        let uses = UseMap::of_file(&ast);
        let Some(syn::Item::Fn(f)) = ast.items.iter().find(|i| matches!(i, syn::Item::Fn(_)))
        else {
            panic!("no fn in {src}");
        };
        does_shell_io(&f.sig, &f.block, &names(), &uses)
    }

    #[test]
    fn methods_on_a_shell_typed_param_are_io() {
        assert!(io("fn f(git: &Git) -> String { git.head() }"));
        assert!(io("fn f(git: &mut Git) { git.run() }"));
    }

    #[test]
    fn methods_on_a_constructed_shell_value_are_io() {
        assert!(io("fn f() { let g = Git { path: p() }; g.head(); }"));
        assert!(io("fn f() { let g = Git::open(p())?; g.head(); }"));
        assert!(io("fn f() { Git::open(p()).head(); }"));
    }

    #[test]
    fn functions_through_shell_types_and_paths_are_io() {
        assert!(io("fn f() { call(Git::default_branch); }"));
        assert!(io("fn f() { wg_proc::run_capture(); }"));
        assert!(io("use wg_proc::run_capture; fn f() { run_capture(); }"));
        assert!(io("fn f() { wg_feat::service::go(); }"));
        assert!(io("use wg_proc as p; fn f() { p::run_capture(); }"));
    }

    #[test]
    fn arithmetic_and_non_shell_receivers_are_not_io() {
        assert!(!io("fn f(a: u32, b: u32) -> u32 { a + b }"));
        assert!(!io("fn f(v: &Vec<u32>) -> usize { v.len() }"));
        assert!(!io("fn f(s: Spec) -> u32 { s.size() }"));
        assert!(!io("fn f() { wg_feat::spec::parse(); }"));
        assert!(!io("fn f() { let k = Kind::Some; }"));
        assert!(!io("fn f() -> Mode { Git::Variant }"));
    }
}
