//! The unauthenticated health probe and the `meta` document. Both are dicts the handlers build
//! from the version and the configuration, so they take those values as arguments.

use wg_json::Value;

use crate::wire_dto;

wire_dto! {
    /// `GET /api/v1/health`: always `ok` while the process answers.
    pub struct HealthOut<'a>("HealthOut") dict health(version: &'a str) {
        status: &'static str = "ok",
        version: &'a str = version,
    }
}

wire_dto! {
    /// The `llm` object of `meta`.
    pub struct MetaLlm("MetaLlm") {
        pub classification: Value,
        pub embeddings: Value,
    }
}

wire_dto! {
    /// `GET /api/v1/meta`. `agents` are the configured agent names; `classification` and
    /// `embeddings` are the dumped LLM sections of the configuration.
    pub struct MetaOut<'a>("MetaOut")
    dict meta(
        version: &'a str,
        default_workspace: Option<&'a str>,
        execution_enabled: bool,
        auto_schedule: bool,
        agents: &'a [String],
        classification: Value,
        embeddings: Value
    ) {
        version: &'a str = version,
        default_workspace: Option<&'a str> = default_workspace,
        execution_enabled: bool = execution_enabled,
        auto_schedule: bool = auto_schedule,
        agents: &'a [String] = agents,
        llm: MetaLlm = MetaLlm { classification, embeddings },
    }
}
