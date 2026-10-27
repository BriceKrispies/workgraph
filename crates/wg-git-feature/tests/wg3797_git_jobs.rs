//! The git feature's job kinds reach the worker: `git_sync` is collected by
//! `HandlerRegistry::from_features` and is not part of the base registry.

use wg_runtime::HandlerRegistry;

#[test]
fn the_worker_registers_git_sync_from_the_git_feature() {
    let registry = HandlerRegistry::from_features(&[&wg_git_feature::FEATURE]);
    assert!(registry.handlers.contains_key("git_sync"));
}

#[test]
fn the_base_registry_does_not_wire_git_sync() {
    assert!(
        !HandlerRegistry::standard()
            .handlers
            .contains_key("git_sync")
    );
}
