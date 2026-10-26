//! The DTOs, one module per area.
//!
//! Each DTO module exports `NAME` and `FIELDS` and a `from_model` constructor; see the crate
//! documentation for the conventions. Later wire tasks add their modules here and their DTOs to
//! [`implemented`].

pub mod agent_time_out;
pub mod agents;
pub mod analytics_compare_body;
pub mod analytics_compare_out;
pub mod analytics_estimate_body;
pub mod analytics_export_body;
pub mod analytics_filters;
pub mod analytics_mcp_shapes;
pub mod analytics_overview;
pub mod analytics_query;
pub mod analytics_query_out;
pub mod analytics_report_entry;
pub mod attempt;
pub mod attempt_dicts;
pub mod bodies;
pub mod classification_disagreement;
pub mod classification_evaluation;
pub mod classification_job;
pub mod classification_out;
pub mod classification_task;
pub mod claude;
pub mod comment;
pub mod commit_link;
pub mod contract_check;
pub mod coupling_out;
pub mod dependency_state;
pub mod estimate_out;
pub mod event;
pub mod experiment;
pub mod experiment_shapes;
pub mod failures_out;
pub mod graph_edge;
pub mod graph_node;
pub mod health;
pub mod job;
pub mod llm;
pub mod mcp_outputs;
pub mod mcp_shapes;
pub mod metric_definition;
pub mod page;
pub mod predictor;
pub mod project;
pub mod project_summary;
pub mod relation;
pub mod repository;
pub mod repository_brief;
pub mod repository_checkout;
pub mod repository_remote;
pub mod repository_status;
pub mod retro_body;
pub mod retrospective_out;
pub mod scheduler;
pub mod search_result;
pub mod settings;
pub mod stack;
pub mod stack_create;
pub mod stack_item;
pub mod stack_item_add;
pub mod stack_repository;
pub mod subagents_out;
pub mod task;
pub mod task_brief;
pub mod task_changes;
pub mod task_detail;
pub mod task_diff;
pub mod task_file;
pub mod task_graph;
pub mod task_history_row;
pub mod task_link;
pub mod task_rollup;
pub mod trial;
pub mod verification;
pub mod worker;
pub mod workspace;
pub mod workspace_summary;

/// The DTOs that exist: the OpenAPI component schema name and the field names in order.
pub fn implemented() -> Vec<(&'static str, &'static [&'static str])> {
    let mut list = vec![
        (workspace::NAME, workspace::FIELDS.as_slice()),
        (project::NAME, project::FIELDS.as_slice()),
        (
            workspace_summary::NAME,
            workspace_summary::FIELDS.as_slice(),
        ),
        (project_summary::NAME, project_summary::FIELDS.as_slice()),
        (repository::NAME, repository::FIELDS.as_slice()),
        (
            repository_remote::NAME,
            repository_remote::FIELDS.as_slice(),
        ),
        (
            repository_checkout::NAME,
            repository_checkout::FIELDS.as_slice(),
        ),
        (repository_brief::NAME, repository_brief::FIELDS.as_slice()),
        (
            repository_status::NAME,
            repository_status::FIELDS.as_slice(),
        ),
        (task::NAME, task::FIELDS.as_slice()),
        (task_detail::NAME, task_detail::FIELDS.as_slice()),
        (task_brief::NAME, task_brief::FIELDS.as_slice()),
        (task_file::NAME, task_file::FIELDS.as_slice()),
        (task_link::NAME, task_link::FIELDS.as_slice()),
        (task_rollup::NAME, task_rollup::FIELDS.as_slice()),
        (dependency_state::NAME, dependency_state::FIELDS.as_slice()),
        (comment::NAME, comment::FIELDS.as_slice()),
        (relation::NAME, relation::FIELDS.as_slice()),
        (graph_node::NAME, graph_node::FIELDS.as_slice()),
        (graph_edge::NAME, graph_edge::FIELDS.as_slice()),
        (task_graph::NAME, task_graph::FIELDS.as_slice()),
        (page::NAME, page::FIELDS.as_slice()),
        (stack::NAME, stack::FIELDS.as_slice()),
        (stack_item::NAME, stack_item::FIELDS.as_slice()),
        (stack_repository::NAME, stack_repository::FIELDS.as_slice()),
        (stack_create::NAME, stack_create::FIELDS.as_slice()),
        (stack_item_add::NAME, stack_item_add::FIELDS.as_slice()),
        (event::NAME, event::FIELDS.as_slice()),
        (
            analytics_compare_body::NAME,
            analytics_compare_body::FIELDS.as_slice(),
        ),
        (
            analytics_estimate_body::NAME,
            analytics_estimate_body::FIELDS.as_slice(),
        ),
        (
            analytics_export_body::NAME,
            analytics_export_body::FIELDS.as_slice(),
        ),
        (
            analytics_filters::NAME,
            analytics_filters::FIELDS.as_slice(),
        ),
        (analytics_query::NAME, analytics_query::FIELDS.as_slice()),
        (retro_body::NAME, retro_body::FIELDS.as_slice()),
    ];
    list.extend(bodies::ALL.iter().map(|body| (body.name, body.fields)));
    list
}
