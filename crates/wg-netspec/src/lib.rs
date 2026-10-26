//! wg-netspec: the pure half of `wg-net` (docs/rust-rewrite.md sections 4.3 and 4.6, SH-18).
//!
//! The provider clients and the Jira client are blocking HTTP clients with retries. Everything in
//! them that is a function of values lives here, so the shell crate keeps only the transport and
//! the sleeping:
//!
//! * [`request`] and [`text`]: an [`HttpCall`](wg_model::HttpCall) for each request, the query
//!   string encoding and the Python-style text helpers the messages use;
//! * [`retry`]: which attempts are retried, how long to wait, and the error an exhausted retry
//!   leaves behind;
//! * [`ollama`], [`openai`] and [`jira`]: each provider's endpoints, its payloads, its JSON
//!   parsing and its error mapping, plus the JQL Jira search is built from ([`jql`]).

pub mod client;
pub mod error;
pub mod jira;
pub mod jql;
pub mod ollama;
pub mod openai;
pub mod output;
pub mod request;
pub mod retry;
pub mod text;

pub use client::{Endpoint, Generated};
pub use error::{ProviderError, TrackerError};
pub use output::Structured;
