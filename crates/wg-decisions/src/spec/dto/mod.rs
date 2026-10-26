//! The decision DTOs: the output values of the decision routes and tools, and the request bodies'
//! names and fields (`DecisionOut`, `DecisionOptionOut`, ...). Moved from `wg-wire` with the
//! feature; each module mirrors one OpenAPI component schema.

pub mod decision;
pub mod decision_choose;
pub mod decision_option;
pub mod decision_option_in;
pub mod decision_request;
pub mod decision_task;
pub mod decision_with_task;
pub mod decision_withdraw;
