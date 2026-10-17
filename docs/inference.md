# Inference providers (Ollama and others)

Model-dependent features are optional enrichments that run as background jobs:

| Capability | Used for | Selection |
| --- | --- | --- |
| Structured generation | classification, decomposition, retrospective narrative | `llm.classification`, `llm.summarization` |
| Embeddings | semantic and hybrid search, similar tasks, estimation | `llm.embeddings` |

## Providers

| Type | Notes |
| --- | --- |
| `ollama` | HTTP API (`/api/chat` with a JSON Schema `format`, `/api/embed`, `/api/tags`). Default. |
| `openai_compatible` | Any server with `/v1/chat/completions` (`response_format: json_schema`), `/v1/embeddings`, `/v1/models` (llama.cpp server, vLLM, LM Studio). |
| `heuristic` | Built-in, deterministic and offline: keyword classifier, checklist decomposer, templated narrative, feature-hashed embeddings (`hash-embed-256`). Clearly labelled as heuristic in every result. |

All HTTP providers have configurable endpoints, timeouts, retries with exponential backoff on
transport errors and retryable statuses, and optional bearer credentials read from an environment
variable.

## Ollama setup

```sh
ollama pull qwen2.5:7b-instruct
ollama pull nomic-embed-text
uv run workgraph doctor
```

`doctor` reports whether each configured model is pulled. Change models with:

```sh
uv run workgraph config set llm.classification.model llama3.1:8b
uv run workgraph config set llm.embeddings.model mxbai-embed-large
uv run workgraph search --reindex
```

Remote or non-default endpoints: `uv run workgraph config set llm.providers.ollama.base_url http://192.168.1.20:11434`.

## When the model is unavailable

Workgraph never needs Ollama for basic operation. A classification job against an unreachable
server or a model that is not pulled fails with `error_kind = provider_unavailable`, retries with
backoff and finally lands in the dead-letter queue (`workgraph job list --status dead`), where it
can be retried after fixing the setup (`workgraph job retry <id>`). The failed attempt is also
recorded as a classification row with `validation_status = provider_unavailable`.

## Classification provenance and policy

Every classification stores provider, model, prompt version (`classify-v1`), taxonomy version
(`2026.1`), input hash, raw output, parsed fields, confidence, validation status, errors and
duration. Rules for the *current* classification:

1. A manual (`workgraph classify confirm`) classification is authoritative and never replaced by a model.
2. A newer result from the same provider and model replaces that model's earlier result.
3. A result from a different provider/model is stored as an alternative; it becomes current only
   when promoted (`classify promote` or `--promote`). Disagreements are recorded as events and
   listed by `workgraph classify disagreements`.

Bulk re-classification (`workgraph classify reclassify`) appends a new batch; nothing is deleted.

## Evaluation

Confirmed classifications are added to the gold-standard set automatically; add more with
`workgraph classify gold --category bug_fix --task WG-3` or `--title "..."`. Then:

```sh
uv run workgraph classify evaluate --provider ollama --model qwen2.5:7b-instruct
uv run workgraph classify evaluate --provider heuristic --model heuristic-v1
uv run workgraph classify evaluations
```

Each evaluation stores accuracy, macro F1, per-class precision/recall/F1 and the confusion matrix.

Classifications and narratives are **interpretations**. Metrics and estimates are computed from
measured data only; models are used to organise or explain established evidence.
