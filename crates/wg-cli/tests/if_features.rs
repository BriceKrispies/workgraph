//! The binary's feature list registers the decisions feature: its command group, its six routes
//! and its three tools (docs/rust-rewrite.md 4.16). The front ends take the same list, so a
//! feature missing here is missing from the CLI, the HTTP API and the MCP server alike.

use wg_cli::commands::FEATURES;

#[test]
fn the_feature_list_registers_the_decisions_group_routes_and_tools() {
    let decisions = FEATURES
        .iter()
        .find(|feature| feature.name == "decisions")
        .expect("the decisions feature is in FEATURES");

    let groups: Vec<&str> = decisions.cli.iter().map(|group| group.name).collect();
    assert_eq!(groups, ["decision"]);

    let route_ids: Vec<&str> = decisions
        .routes
        .iter()
        .flat_map(|group| group.specs.iter().map(|spec| spec.id))
        .collect();
    assert_eq!(route_ids, ["H125", "H126", "H129", "H130", "H127", "H128"]);

    let tools: Vec<&str> = wg_decisions::mcp::TABLE
        .iter()
        .map(|(name, _)| *name)
        .collect();
    assert_eq!(
        tools,
        ["task_request_decision", "decision_list", "task_decisions"]
    );
    assert_eq!(decisions.tools.len(), 1);

    let registered = wg_server::groups::routes_with(FEATURES);
    assert!(registered.iter().any(|route| route.id == "H130"));
}
