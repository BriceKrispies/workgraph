#![allow(clippy::expect_used, clippy::unwrap_used)]
//! The Ollama and OpenAI-compatible clients against in-process servers: the requests they send,
//! the retries and their backoff, and the mapping of answers to errors.

use crate::common;

use serde_json::{Value, json};
use wg_net::{Endpoint, Net, Ollama, OpenAiCompatible, ProviderError};

use common::{Fake, Reply, delays_of, net, recording_sleep, refused_url};

fn endpoint(base_url: &str, max_retries: u32) -> Endpoint {
    Endpoint {
        base_url: base_url.to_owned(),
        api_key: None,
        timeout_seconds: 5.0,
        max_retries,
        backoff_seconds: 1.0,
    }
}

fn schema() -> Value {
    json!({"title": "TaskClassification", "type": "object"})
}

fn chat_reply(model: &str, content: &str) -> Reply {
    Reply::json(
        200,
        &json!({
            "model": model,
            "message": {"role": "assistant", "content": content},
            "done": true,
            "eval_count": 42,
        }),
    )
}

#[test]
fn ollama_lists_models_sorted_and_sends_the_key() {
    let fake = Fake::start(|_| {
        Reply::json(
            200,
            &json!({"models": [{"name": "qwen:latest"}, {"name": "alpha"}]}),
        )
    });
    let mut settings = endpoint(&fake.url, 2);
    settings.api_key = Some("key".to_owned());
    let mut client = Ollama {
        endpoint: settings,
        net: Net::default(),
    };

    let models = client.list_models().expect("listing succeeds");

    assert_eq!(models, ["alpha", "qwen:latest"]);
    let request = &fake.requests()[0];
    assert_eq!(request.path(), "/api/tags");
    assert_eq!(request.header("Authorization"), Some("Bearer key"));
}

#[test]
fn ollama_check_model_accepts_the_latest_tag() {
    let fake = Fake::start(|_| {
        Reply::json(
            200,
            &json!({"models": [{"name": "qwen:latest"}, {"name": "alpha"}]}),
        )
    });
    let mut client = Ollama {
        endpoint: endpoint(&fake.url, 0),
        net: Net::default(),
    };

    assert_eq!(
        client.check_model("qwen"),
        (true, "qwen available".to_owned())
    );
    assert_eq!(
        client.check_model("nope"),
        (
            false,
            "model 'nope' is not pulled (available: alpha, qwen:latest); run `ollama pull nope`"
                .to_owned()
        )
    );
}

#[test]
fn ollama_generate_json_retries_503_with_backoff_then_succeeds() {
    let replies = std::sync::Mutex::new(std::collections::VecDeque::from(vec![
        Reply::text(503, "busy"),
        Reply::text(503, "busy"),
        chat_reply("qwen-test", r#"{"category":"feature"}"#),
    ]));
    let fake = Fake::start(move |_| {
        replies
            .lock()
            .expect("script lock")
            .pop_front()
            .unwrap_or_else(|| Reply::text(500, "exhausted"))
    });
    let (sleep, delays) = recording_sleep();
    let mut client = Ollama {
        endpoint: endpoint(&fake.url, 2),
        net: net(sleep),
    };

    let generated = client
        .generate_json("qwen-test", "system", "<task>x</task>", &schema(), 0.0)
        .expect("third attempt succeeds");

    assert_eq!(generated.attempts, 3);
    assert_eq!(generated.output.data["category"], "feature");
    assert_eq!(generated.output.model, "qwen-test");
    assert_eq!(generated.output.metadata["eval_count"], 42);
    assert_eq!(delays_of(&delays), [1.0, 2.0]);
    let body = fake.requests()[0].json();
    assert_eq!(body["format"], schema());
    assert_eq!(body["stream"], false);
    assert_eq!(body["options"]["temperature"], 0.0);
    assert_eq!(body["messages"][1]["content"], "<task>x</task>");
}

#[test]
fn ollama_gives_up_after_its_retries_as_unavailable() {
    let fake = Fake::start(|_| Reply::text(503, "busy"));
    let (sleep, delays) = recording_sleep();
    let mut client = Ollama {
        endpoint: endpoint(&fake.url, 2),
        net: net(sleep),
    };

    let error = client
        .generate_json("qwen-test", "system", "prompt", &schema(), 0.0)
        .expect_err("every attempt is 503");

    let ProviderError::Unavailable(message) = error else {
        panic!("expected Unavailable, got {error:?}");
    };
    assert!(
        message.contains("unreachable after 3 attempt(s)"),
        "{message}"
    );
    assert!(message.contains("HTTP 503: busy"), "{message}");
    assert_eq!(fake.requests().len(), 3);
    assert_eq!(delays_of(&delays), [1.0, 2.0]);
}

#[test]
fn ollama_connection_failures_are_retried_then_unavailable() {
    let (sleep, delays) = recording_sleep();
    let mut client = Ollama {
        endpoint: endpoint(&refused_url(), 1),
        net: net(sleep),
    };

    let error = client
        .embed("embed-test", &["x".to_owned()])
        .expect_err("nothing listens");

    assert!(
        matches!(error, ProviderError::Unavailable(ref m) if m.contains("unreachable after 2 attempt(s)"))
    );
    assert_eq!(delays_of(&delays), [1.0]);
}

#[test]
fn ollama_unknown_model_is_model_unavailable() {
    let fake = Fake::start(|_| Reply::json(404, &json!({"error": "model not pulled"})));
    let mut client = Ollama {
        endpoint: endpoint(&fake.url, 2),
        net: Net::default(),
    };

    let error = client
        .generate_json("ghost", "system", "prompt", &schema(), 0.0)
        .expect_err("404 is an error");

    assert_eq!(
        error,
        ProviderError::ModelUnavailable(
            "ollama model 'ghost' is unavailable: model not pulled".to_owned()
        )
    );
    assert_eq!(fake.requests().len(), 1, "a 404 is not retried");
}

#[test]
fn ollama_non_json_content_is_invalid_output() {
    let fake = Fake::start(|_| chat_reply("broken", "this is not json"));
    let mut client = Ollama {
        endpoint: endpoint(&fake.url, 0),
        net: Net::default(),
    };

    let error = client
        .generate_json("broken", "system", "prompt", &schema(), 0.0)
        .expect_err("content is not JSON");

    assert!(
        matches!(error, ProviderError::InvalidOutput(ref m) if m.starts_with("model returned non-JSON output"))
    );
}

#[test]
fn ollama_embeddings_come_back_in_order() {
    let fake = Fake::start(|request| {
        let count = request.json()["input"].as_array().map_or(0, Vec::len);
        let vectors: Vec<Value> = (0..count).map(|n| json!([n as f64, 0.5])).collect();
        Reply::json(200, &json!({"embeddings": vectors}))
    });
    let mut client = Ollama {
        endpoint: endpoint(&fake.url, 0),
        net: Net::default(),
    };

    let vectors = client
        .embed("embed-test", &["a".to_owned(), "b".to_owned()])
        .expect("embedding succeeds");

    assert_eq!(vectors, vec![vec![0.0, 0.5], vec![1.0, 0.5]]);
    assert_eq!(fake.requests()[0].path(), "/api/embed");
}

#[test]
fn ollama_embedding_count_mismatch_is_invalid_output() {
    let fake = Fake::start(|_| Reply::json(200, &json!({"embeddings": [[1.0]]})));
    let mut client = Ollama {
        endpoint: endpoint(&fake.url, 0),
        net: Net::default(),
    };

    let error = client
        .embed("embed-test", &["a".to_owned(), "b".to_owned()])
        .expect_err("one vector for two texts");

    assert_eq!(
        error,
        ProviderError::InvalidOutput("ollama returned an unexpected embedding payload".to_owned())
    );
}

#[test]
fn openai_models_are_sorted_and_checked() {
    let fake = Fake::start(|_| Reply::json(200, &json!({"data": [{"id": "b"}, {"id": "a"}]})));
    let mut client = OpenAiCompatible {
        name: "local".to_owned(),
        endpoint: endpoint(&fake.url, 0),
        net: Net::default(),
    };

    assert_eq!(client.list_models().expect("listing succeeds"), ["a", "b"]);
    assert_eq!(client.check_model("a"), (true, "a available".to_owned()));
    assert_eq!(
        client.check_model("z"),
        (false, "'z' not served".to_owned())
    );
    assert_eq!(fake.requests()[0].path(), "/v1/models");
}

#[test]
fn openai_chat_sends_a_strict_json_schema_and_reads_the_object() {
    let fake = Fake::start(|_| {
        Reply::json(
            200,
            &json!({
                "model": "served-model",
                "choices": [{"message": {"content": r#"{"category":"bug"}"#}}],
            }),
        )
    });
    let mut client = OpenAiCompatible {
        name: "local".to_owned(),
        endpoint: endpoint(&fake.url, 0),
        net: Net::default(),
    };

    let generated = client
        .generate_json("requested", "system", "prompt", &schema(), 0.2)
        .expect("chat succeeds");

    assert_eq!(generated.output.data["category"], "bug");
    assert_eq!(generated.output.model, "served-model");
    let body = fake.requests()[0].json();
    assert_eq!(body["model"], "requested");
    assert_eq!(body["temperature"], 0.2);
    assert_eq!(body["response_format"]["type"], "json_schema");
    assert_eq!(
        body["response_format"]["json_schema"]["name"],
        "TaskClassification"
    );
    assert_eq!(body["response_format"]["json_schema"]["strict"], true);
}

#[test]
fn openai_unexpected_completion_payload_is_invalid_output() {
    let fake = Fake::start(|_| Reply::json(200, &json!({"choices": []})));
    let mut client = OpenAiCompatible {
        name: "local".to_owned(),
        endpoint: endpoint(&fake.url, 0),
        net: Net::default(),
    };

    let error = client
        .generate_json("requested", "system", "prompt", &schema(), 0.0)
        .expect_err("no choices");

    assert!(
        matches!(error, ProviderError::InvalidOutput(ref m) if m.starts_with("unexpected completion payload"))
    );
}

#[test]
fn openai_unknown_model_is_model_unavailable_and_other_errors_are_invalid_output() {
    let fake = Fake::start(|request| {
        if request.path() == "/v1/chat/completions" {
            Reply::text(404, "no such model")
        } else {
            Reply::text(400, "bad")
        }
    });
    let mut client = OpenAiCompatible {
        name: "local".to_owned(),
        endpoint: endpoint(&fake.url, 2),
        net: Net::default(),
    };

    let missing = client
        .generate_json("ghost", "system", "prompt", &schema(), 0.0)
        .expect_err("404");
    assert_eq!(
        missing,
        ProviderError::ModelUnavailable("local: no such model".to_owned())
    );
    assert_eq!(fake.requests().len(), 1, "404 is not retried");

    let rejected = client.embed("embed", &["x".to_owned()]).expect_err("400");
    assert_eq!(
        rejected,
        ProviderError::InvalidOutput("local error 400: bad".to_owned())
    );
}

#[test]
fn openai_embeddings_are_ordered_by_index() {
    let fake = Fake::start(|_| {
        Reply::json(
            200,
            &json!({"data": [
                {"index": 1, "embedding": [3.0]},
                {"index": 0, "embedding": [1.0]},
            ]}),
        )
    });
    let mut client = OpenAiCompatible {
        name: "local".to_owned(),
        endpoint: endpoint(&fake.url, 0),
        net: Net::default(),
    };

    let vectors = client
        .embed("embed", &["a".to_owned(), "b".to_owned()])
        .expect("embedding succeeds");

    assert_eq!(vectors, vec![vec![1.0], vec![3.0]]);
    assert_eq!(fake.requests()[0].path(), "/v1/embeddings");
}

#[test]
fn openai_bearer_key_is_sent_on_every_request() {
    let fake = Fake::start(|_| Reply::json(200, &json!({"data": []})));
    let mut settings = endpoint(&fake.url, 0);
    settings.api_key = Some("local-key".to_owned());
    let mut client = OpenAiCompatible {
        name: "local".to_owned(),
        endpoint: settings,
        net: Net::default(),
    };

    client.list_models().expect("listing succeeds");

    assert_eq!(
        fake.requests()[0].header("Authorization"),
        Some("Bearer local-key")
    );
}
