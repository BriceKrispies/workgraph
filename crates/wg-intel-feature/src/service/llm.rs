//! Shell: the inference providers the classifier and the embedder call, built from the configured
//! `llm.providers` entries (`LLMRegistry.get` and `integrations/llm/registry.py`'s `build`).
//!
//! The services take a [`Providers`] so that tests answer from a fake provider; [`Live`] builds the
//! real clients: the Ollama and OpenAI-compatible clients of `wg-net`, and the offline heuristic
//! provider of `wg-intel`. The outcomes of a call and their errors are pure, in [`crate::spec::llm`].

use serde_json::{Map, Value as Json};
use wg_intel::{EMBED_DIM, HeuristicError};
use wg_net::{Net, Ollama, OpenAiCompatible};
use wg_netspec::client::Endpoint;
use wg_registry::providers::{entry_type, llm_provider};
use wg_types::WgError;

use crate::spec::llm::{CallError, Generation, generation_of};

/// The default base URL of an Ollama provider (`OllamaProviderConfig.base_url`).
const OLLAMA_URL: &str = "http://127.0.0.1:11434";

/// The temperature of every structured call (`generate_json`'s default).
const TEMPERATURE: f64 = 0.0;

/// One configured inference provider.
pub trait Provider {
    /// `generate_json`: an answer that is a JSON object matching `schema`.
    fn generate_json(
        &mut self,
        model: &str,
        system: &str,
        prompt: &str,
        schema: &Json,
    ) -> Result<Generation, CallError>;

    /// `embed`: one vector per text, in order.
    fn embed(&mut self, model: &str, texts: &[String]) -> Result<Vec<Vec<f64>>, CallError>;
}

/// Builds the provider `name` of a dumped configuration (`LLMRegistry.get`).
pub trait Providers {
    fn provider(
        &mut self,
        name: &str,
        config: &Map<String, Json>,
    ) -> Result<Box<dyn Provider>, WgError>;
}

/// The providers of the real endpoints.
pub struct Live;

impl Providers for Live {
    fn provider(
        &mut self,
        name: &str,
        config: &Map<String, Json>,
    ) -> Result<Box<dyn Provider>, WgError> {
        let entry = llm_provider(config, name).ok_or_else(|| {
            WgError::not_found(format!("inference provider '{name}' is not configured"))
        })?;
        let key = entry
            .get("api_key_env")
            .and_then(Json::as_str)
            .and_then(|var| std::env::var(var).ok());
        Ok(match entry_type(entry) {
            "ollama" => Box::new(LiveOllama(Ollama {
                endpoint: Endpoint::from_config(entry, OLLAMA_URL, key),
                net: Net::default(),
            })),
            "openai_compatible" => Box::new(LiveOpenAi(OpenAiCompatible {
                name: name.to_owned(),
                endpoint: Endpoint::from_config(entry, "", key),
                net: Net::default(),
            })),
            _ => Box::new(Heuristic),
        })
    }
}

struct LiveOllama(Ollama);

struct LiveOpenAi(OpenAiCompatible);

impl Provider for LiveOllama {
    fn generate_json(
        &mut self,
        model: &str,
        system: &str,
        prompt: &str,
        schema: &Json,
    ) -> Result<Generation, CallError> {
        self.0
            .generate_json(model, system, prompt, schema, TEMPERATURE)
            .map(|generated| generation_of(generated.output, generated.duration_ms))
            .map_err(CallError::Provider)
    }

    fn embed(&mut self, model: &str, texts: &[String]) -> Result<Vec<Vec<f64>>, CallError> {
        self.0.embed(model, texts).map_err(CallError::Provider)
    }
}

impl Provider for LiveOpenAi {
    fn generate_json(
        &mut self,
        model: &str,
        system: &str,
        prompt: &str,
        schema: &Json,
    ) -> Result<Generation, CallError> {
        self.0
            .generate_json(model, system, prompt, schema, TEMPERATURE)
            .map(|generated| generation_of(generated.output, generated.duration_ms))
            .map_err(CallError::Provider)
    }

    fn embed(&mut self, model: &str, texts: &[String]) -> Result<Vec<Vec<f64>>, CallError> {
        self.0.embed(model, texts).map_err(CallError::Provider)
    }
}

/// The offline heuristic provider (`HeuristicProvider`). It needs no network.
pub struct Heuristic;

impl Provider for Heuristic {
    fn generate_json(
        &mut self,
        _model: &str,
        _system: &str,
        prompt: &str,
        schema: &Json,
    ) -> Result<Generation, CallError> {
        let title = schema.get("title").and_then(Json::as_str);
        match wg_intel::generate_json(title, prompt) {
            Ok(Json::Object(data)) => Ok(Generation {
                data,
                duration_ms: 0,
            }),
            Ok(_) => Err(CallError::Raised(WgError::validation(
                "heuristic provider produced a non-object output",
            ))),
            Err(HeuristicError::Unsupported(title)) => Err(CallError::Unsupported(format!(
                "heuristic provider cannot produce '{title}' outputs"
            ))),
            Err(other) => Err(CallError::Raised(WgError::validation(other.to_string()))),
        }
    }

    fn embed(&mut self, _model: &str, texts: &[String]) -> Result<Vec<Vec<f64>>, CallError> {
        Ok(texts
            .iter()
            .map(|text| wg_intel::hash_embedding(text, EMBED_DIM))
            .collect())
    }
}
