//! The Ollama client (`integrations/llm/ollama.py`): model listing, JSON-constrained chat and
//! embeddings over the HTTP API. Requests, retries and parsing are `wg-netspec`'s.

use serde_json::Value;
use wg_model::{HttpCall, HttpResponse};
use wg_netspec::ProviderError;
use wg_netspec::client::{Endpoint, Generated};
use wg_netspec::ollama as spec;
use wg_netspec::request::parse_body;
use wg_netspec::retry::Backoff;

use crate::client::provider_request;
use crate::host::Net;

/// The timeout of the model listing, in seconds.
const LIST_TIMEOUT_SECONDS: f64 = 10.0;

/// A client for one Ollama server.
pub struct Ollama {
    pub endpoint: Endpoint,
    pub net: Net,
}

impl Ollama {
    /// The installed model names, sorted. The listing is never retried.
    pub fn list_models(&mut self) -> Result<Vec<String>, ProviderError> {
        let call = spec::tags_call(
            &self.endpoint.base_url,
            self.endpoint.api_key.as_deref(),
            LIST_TIMEOUT_SECONDS,
        );
        let no_retries = Backoff {
            max_retries: 0,
            base_seconds: 0.0,
        };
        let label = spec::label(&self.endpoint.base_url);
        let (result, _) = provider_request(&mut self.net, &call, &label, no_retries);
        let response = result?;
        if response.status >= 400 {
            return Err(ProviderError::Unavailable(format!(
                "ollama returned {}",
                response.status
            )));
        }
        Ok(spec::model_names(&parse_body(&response.body)))
    }

    /// Whether `model` is pulled, and the sentence that says so.
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
        let (response, attempts) = self.post(&call, model)?;
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
        let (response, _) = self.post(&call, model)?;
        spec::parse_embeddings(&parse_body(&response.body), texts.len())
    }

    /// Sends a request with the endpoint's retries; a non-success status is mapped to its error.
    fn post(&mut self, call: &HttpCall, model: &str) -> Result<(HttpResponse, u32), ProviderError> {
        let label = spec::label(&self.endpoint.base_url);
        let (result, attempts) =
            provider_request(&mut self.net, call, &label, self.endpoint.backoff());
        let response = result?;
        match spec::status_error(model, response.status, &response.body) {
            Some(err) => Err(err),
            None => Ok((response, attempts)),
        }
    }
}
