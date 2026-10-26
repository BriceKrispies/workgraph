//! The OpenAI-compatible client (`integrations/llm/openai_compatible.py`): model listing,
//! strict JSON-Schema chat and embeddings. Requests, retries and parsing are `wg-netspec`'s.

use serde_json::Value;
use wg_model::{HttpCall, HttpResponse};
use wg_netspec::ProviderError;
use wg_netspec::client::{Endpoint, Generated};
use wg_netspec::openai as spec;
use wg_netspec::request::parse_body;

use crate::client::provider_request;
use crate::host::Net;

/// A client for one OpenAI-compatible server, known by its configured name.
pub struct OpenAiCompatible {
    pub name: String,
    pub endpoint: Endpoint,
    pub net: Net,
}

impl OpenAiCompatible {
    /// The served model ids, sorted.
    pub fn list_models(&mut self) -> Result<Vec<String>, ProviderError> {
        let call = spec::models_call(
            &self.endpoint.base_url,
            self.endpoint.api_key.as_deref(),
            self.endpoint.timeout_seconds,
        );
        let (response, _) = self.send(&call)?;
        Ok(spec::model_ids(&parse_body(&response.body)))
    }

    /// Whether `model` is served, and the sentence that says so.
    pub fn check_model(&mut self, model: &str) -> (bool, String) {
        match self.list_models() {
            Ok(models) => spec::check_model(&models, model),
            Err(err) => (false, err.message().to_owned()),
        }
    }

    /// A chat completion whose content must be a JSON object matching `schema`.
    pub fn generate_json(
        &mut self,
        model: &str,
        system: &str,
        prompt: &str,
        schema: &Value,
        temperature: f64,
    ) -> Result<Generated, ProviderError> {
        let started = self.net.monotonic_ms();
        let call = spec::chat_call(
            &self.endpoint.base_url,
            self.endpoint.api_key.as_deref(),
            self.endpoint.timeout_seconds,
            model,
            system,
            prompt,
            schema,
            temperature,
        );
        let (response, attempts) = self.send(&call)?;
        let output = spec::parse_chat(&parse_body(&response.body), model)?;
        Ok(Generated {
            output,
            attempts,
            duration_ms: self.net.monotonic_ms().saturating_sub(started),
        })
    }

    /// One embedding per text, in order.
    pub fn embed(&mut self, model: &str, texts: &[String]) -> Result<Vec<Vec<f64>>, ProviderError> {
        let call = spec::embed_call(
            &self.endpoint.base_url,
            self.endpoint.api_key.as_deref(),
            self.endpoint.timeout_seconds,
            model,
            texts,
        );
        let (response, _) = self.send(&call)?;
        spec::parse_embeddings(&parse_body(&response.body), texts.len())
    }

    /// Sends a request with the endpoint's retries; a non-success status is mapped to its error.
    fn send(&mut self, call: &HttpCall) -> Result<(HttpResponse, u32), ProviderError> {
        let label = spec::label(&self.name, &self.endpoint.base_url);
        let (result, attempts) =
            provider_request(&mut self.net, call, &label, self.endpoint.backoff());
        let response = result?;
        match spec::status_error(&self.name, response.status, &response.body) {
            Some(err) => Err(err),
            None => Ok((response, attempts)),
        }
    }
}
