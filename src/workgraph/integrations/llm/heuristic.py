"""Deterministic offline provider.

This is a real, fully offline provider - not a mock - intended for machines without
a local model server, for reproducible demos and for tests. Its outputs are
explicitly heuristic (and recorded with provider ``heuristic``):

* ``TaskClassification`` - weighted keyword rules over the task text.
* ``TaskDecomposition``  - splits acceptance criteria / bullet lists into children.
* ``Narrative``          - a templated summary of the supplied facts.
* embeddings             - signed feature hashing of word unigrams and bigrams.

Prompts mark the content to analyse with ``<task>...</task>`` or
``<facts>...</facts>`` so instructions never contaminate the analysis.
"""

from __future__ import annotations

import hashlib
import json
import math
import re
import time
from typing import Any

from workgraph.integrations.llm.base import (
    CapabilityNotSupportedError,
    InvalidModelOutputError,
    ProviderCapabilities,
    StructuredResponse,
)

MODEL = "heuristic-v1"
EMBED_DIM = 256

CATEGORY_KEYWORDS: dict[str, list[tuple[str, float]]] = {
    "bug_fix": [
        ("bug", 3),
        ("fix", 2.5),
        ("crash", 3),
        ("error", 1.5),
        ("broken", 2.5),
        ("regression", 3),
        ("incorrect", 2),
        ("fails", 2),
        ("exception", 2),
        ("wrong", 1.5),
        ("issue", 1),
    ],
    "feature": [
        ("add", 1.5),
        ("feature", 3),
        ("implement", 2),
        ("support", 1.5),
        ("new", 1.5),
        ("allow", 1.5),
        ("introduce", 2),
        ("enable", 1.5),
        ("endpoint", 1.5),
        ("ui", 1),
    ],
    "refactor": [
        ("refactor", 4),
        ("restructure", 3),
        ("cleanup", 2.5),
        ("clean up", 2.5),
        ("simplify", 2.5),
        ("extract", 2),
        ("rename", 2),
        ("decouple", 3),
        ("reorganize", 3),
    ],
    "maintenance": [
        ("upgrade", 2.5),
        ("bump", 3),
        ("dependency", 2),
        ("dependencies", 2),
        ("deprecat", 2),
        ("chore", 3),
        ("update version", 3),
        ("housekeeping", 3),
    ],
    "infrastructure": [
        ("ci", 2.5),
        ("pipeline", 2.5),
        ("docker", 3),
        ("kubernetes", 3),
        ("deploy", 3),
        ("terraform", 3),
        ("build system", 3),
        ("github actions", 3),
        ("infrastructure", 4),
    ],
    "performance": [
        ("performance", 4),
        ("slow", 3),
        ("latency", 3),
        ("optimiz", 3),
        ("speed up", 3),
        ("memory", 2),
        ("faster", 2.5),
        ("cache", 2),
        ("throughput", 3),
    ],
    "security": [
        ("security", 4),
        ("vulnerab", 4),
        ("cve", 4),
        ("auth", 2),
        ("xss", 4),
        ("injection", 3.5),
        ("csrf", 4),
        ("permission", 2),
        ("encrypt", 3),
        ("secret", 2.5),
        ("token", 1.5),
    ],
    "testing": [
        ("test", 3),
        ("tests", 3),
        ("coverage", 3),
        ("flaky", 3.5),
        ("pytest", 3),
        ("e2e", 2.5),
        ("unit test", 3),
        ("integration test", 3),
    ],
    "documentation": [
        ("doc", 2.5),
        ("docs", 3),
        ("documentation", 4),
        ("readme", 4),
        ("guide", 2),
        ("tutorial", 3),
        ("typo", 3),
        ("comment", 1.5),
    ],
    "research": [
        ("investigate", 3.5),
        ("research", 4),
        ("spike", 4),
        ("evaluate", 2.5),
        ("explore", 2.5),
        ("prototype", 2.5),
        ("compare", 2),
        ("feasibility", 3.5),
    ],
    "architecture": [
        ("architecture", 4),
        ("design", 2),
        ("adr", 3.5),
        ("boundary", 2.5),
        ("module", 1.5),
        ("migration plan", 3),
        ("system design", 4),
        ("service split", 3.5),
        ("monolith", 3),
    ],
}
DOMAINS = {
    "frontend": ["ui", "react", "css", "page", "button", "frontend", "component", "web client", "browser"],
    "backend": ["api", "endpoint", "server", "service", "database", "query", "backend", "handler"],
    "data": ["etl", "analytics", "pipeline", "warehouse", "duckdb", "report", "metrics"],
    "devops": ["ci", "deploy", "docker", "kubernetes", "terraform", "pipeline"],
    "auth": ["auth", "login", "oauth", "session", "token", "password", "permission"],
    "sdk": ["sdk", "client library", "package", "library"],
}
STOP = {
    "the",
    "a",
    "an",
    "and",
    "or",
    "to",
    "of",
    "in",
    "for",
    "on",
    "with",
    "is",
    "be",
    "this",
    "that",
    "it",
}


def _extract(marker: str, prompt: str) -> str:
    match = re.search(rf"<{marker}>(.*?)</{marker}>", prompt, re.DOTALL)
    if not match:
        raise InvalidModelOutputError(f"heuristic provider requires <{marker}>...</{marker}> in the prompt")
    return match.group(1)


def classify_text(text: str) -> dict[str, Any]:
    lowered = text.lower()
    words = re.findall(r"[a-z0-9]+", lowered)
    scores: dict[str, float] = {}
    for category, keywords in CATEGORY_KEYWORDS.items():
        score = 0.0
        for keyword, weight in keywords:
            if " " in keyword:
                score += weight * lowered.count(keyword)
            else:
                score += weight * sum(1 for w in words if w.startswith(keyword))
        scores[category] = score
    ranked = sorted(scores.items(), key=lambda kv: (-kv[1], kv[0]))
    best, best_score = ranked[0]
    second = ranked[1][1] if len(ranked) > 1 else 0.0
    if best_score <= 0:
        category, confidence = "other", 0.2
    else:
        category = best
        confidence = round(min(0.9, 0.35 + 0.5 * (best_score - second) / (best_score + 1e-9)), 3)
    domain = max(DOMAINS, key=lambda d: (sum(lowered.count(k) for k in DOMAINS[d]), d))
    if sum(lowered.count(k) for k in DOMAINS[domain]) == 0:
        domain = "general"
    length = len(words)
    complexity = (
        "trivial"
        if length < 15
        else "small"
        if length < 60
        else "medium"
        if length < 200
        else "large"
        if length < 500
        else "very_large"
    )
    tokens = [w for w in words if w not in STOP and len(w) > 3]
    freq: dict[str, int] = {}
    for token in tokens:
        freq[token] = freq.get(token, 0) + 1
    tags = [t for t, _ in sorted(freq.items(), key=lambda kv: (-kv[1], kv[0]))[:4]]
    systems = sorted({m for m in re.findall(r"\b([a-z0-9]+(?:-[a-z0-9]+)+)\b", lowered) if len(m) > 4})[:5]
    return {
        "category": category,
        "subtype": f"{category}:{domain}",
        "domain": domain,
        "affected_systems": systems,
        "complexity": complexity,
        "tags": tags,
        "confidence": confidence,
        "rationale": f"keyword score {best_score:.1f} (runner-up {second:.1f}); heuristic rules, not a model judgement",
    }


def decompose_text(text: str) -> dict[str, Any]:
    items = [m.strip(" -*\t") for m in re.findall(r"^\s*(?:[-*]|\d+[.)])\s+(.+)$", text, re.MULTILINE)]
    children = []
    for i, item in enumerate(items[:12]):
        children.append(
            {
                "ref": f"c{i + 1}",
                "title": item[:200],
                "description": f"Derived from parent checklist item: {item}",
                "depends_on": [f"c{i}"]
                if i > 0 and re.search(r"\b(then|after|once)\b", item.lower())
                else [],
                "repositories": [],
            }
        )
    return {"children": children, "rationale": "split from the task's bullet/checklist items (heuristic)"}


def narrative_from_facts(facts_json: str) -> dict[str, Any]:
    try:
        facts = json.loads(facts_json)
    except json.JSONDecodeError as exc:
        raise InvalidModelOutputError("facts block is not valid JSON") from exc
    highlights = facts.get("highlights") or []
    summary = facts.get("summary") or "Engineering activity summary."
    return {
        "summary": summary,
        "observations": [str(h) for h in highlights[:8]],
        "opportunities": [str(o) for o in (facts.get("opportunities") or [])[:5]],
    }


def hash_embedding(text: str, dim: int = EMBED_DIM) -> list[float]:
    words = [w for w in re.findall(r"[a-z0-9]+", text.lower()) if w not in STOP]
    features = words + [f"{a}_{b}" for a, b in zip(words, words[1:], strict=False)]
    vector = [0.0] * dim
    for feature in features:
        digest = hashlib.blake2b(feature.encode("utf-8"), digest_size=8).digest()
        index = int.from_bytes(digest[:4], "little") % dim
        sign = 1.0 if digest[4] & 1 else -1.0
        vector[index] += sign
    norm = math.sqrt(sum(v * v for v in vector)) or 1.0
    return [v / norm for v in vector]


class HeuristicProvider:
    kind = "heuristic"

    def __init__(self, name: str = "heuristic") -> None:
        self.name = name

    def capabilities(self) -> ProviderCapabilities:
        return ProviderCapabilities(structured_output=True, embeddings=True, local=True, deterministic=True)

    def list_models(self) -> list[str]:
        return [MODEL, f"hash-embed-{EMBED_DIM}"]

    def check_model(self, model: str) -> tuple[bool, str]:
        ok = model in self.list_models()
        return (
            ok,
            "built-in" if ok else f"unknown heuristic model {model!r}; use {MODEL} or hash-embed-{EMBED_DIM}",
        )

    def generate_json(
        self, model: str, *, system: str, prompt: str, schema: dict[str, Any], temperature: float = 0.0
    ) -> StructuredResponse:
        started = time.monotonic()
        title = schema.get("title")
        if title == "TaskClassification":
            data = classify_text(_extract("task", prompt))
        elif title == "TaskDecomposition":
            data = decompose_text(_extract("task", prompt))
        elif title == "Narrative":
            data = narrative_from_facts(_extract("facts", prompt))
        else:
            raise CapabilityNotSupportedError(f"heuristic provider cannot produce {title!r} outputs")
        return StructuredResponse(
            data=data,
            raw_text=json.dumps(data),
            model=MODEL,
            duration_ms=int((time.monotonic() - started) * 1000),
        )

    def embed(self, model: str, texts: list[str]) -> list[list[float]]:
        return [hash_embedding(t) for t in texts]
