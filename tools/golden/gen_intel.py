"""Golden vectors for the intelligence layer (GV-09).

Writes golden/intel/*.json from the Python implementation:

* classify.json   - heuristic classification through the provider prompt path, with the
                    taxonomy prompt, input hash and TaskClassification validation
* decompose.json  - checklist decomposition of generated and test-derived descriptions
* narrative.json  - templated narrative from facts documents, including invalid input
* embedding.json  - hash embeddings (256 dimensions) of generated texts
* taxonomy.json   - classifier prompt building, input hashes, schema validation of model output
* fts.json        - FTS5 MATCH expressions for generated and adversarial user queries
* search.json     - cosine similarity and reciprocal rank fusion as in SearchService.search

Run: uv run python tools/golden/gen_intel.py
Output is deterministic: sorted keys, LF line endings, UTF-8, fixed PRNG seed.
"""

from __future__ import annotations

import json
import random
from pathlib import Path
from typing import Any

from pydantic import ValidationError

from workgraph.classification.taxonomy import (
    PROMPT_VERSION,
    TAXONOMY_VERSION,
    TaskClassification,
    build_prompt,
    input_hash,
    output_schema,
)
from workgraph.integrations.llm.heuristic import (
    CATEGORY_KEYWORDS,
    DOMAINS,
    EMBED_DIM,
    MODEL,
    HeuristicProvider,
    classify_text,
    decompose_text,
    hash_embedding,
    narrative_from_facts,
)
from workgraph.search.indexer import to_fts_query
from workgraph.search.service import RRF_K, cosine

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "golden" / "intel"
SEED = 20261025

FILLER = [
    "the",
    "a",
    "service",
    "update",
    "new",
    "old",
    "flow",
    "user",
    "report",
    "module",
    "retry",
    "queue",
    "cache",
    "config",
    "checkout",
    "page",
    "login",
    "session",
    "payment",
    "dark",
    "mode",
    "query",
    "handler",
    "endpoint",
    "tests",
    "README",
    "then",
    "after",
    "once",
    "and",
    "with",
]
ODD = [
    "api-gateway",
    "auth-service",
    "web-client",
    "kube-proxy",
    "CVE-2026-1234",
    "e2e.",
    "Bug:",
    "fix!",
    "(ci)",
    "Ünïcode",
    "日本語",
    "emoji🙂",
    "AND",
    "OR",
    "NOT",
    "NEAR(",
    '"quoted"',
    "*star*",
    "a'b",
    "x:y",
    "-neg",
    "^caret",
    "ßtraße",
    "İstanbul",
    "naïve",
    "tab\there",
    "new\nline",
    "\\back",
    "semi;colon",
    "under_score",
    "dot.sep",
    "[bracket]",
    "{brace}",
    "a/b",
]
TITLE_CASES = [str, str.lower, str.upper]


def vocabulary() -> list[str]:
    words: set[str] = set(FILLER) | set(ODD)
    for keywords in CATEGORY_KEYWORDS.values():
        words.update(keyword for keyword, _weight in keywords)
    for names in DOMAINS.values():
        words.update(names)
    return sorted(words)


def gen_text(rng: random.Random, pool: list[str], count: int) -> str:
    return " ".join(rng.choice(pool) for _ in range(count))


def gen_description(rng: random.Random, pool: list[str]) -> str:
    lines = []
    for _ in range(rng.randint(0, 5)):
        prefix = rng.choice(["", "", "- ", "* ", "1. ", "2) ", "  - "])
        lines.append(prefix + gen_text(rng, pool, rng.randint(1, 12)))
    return "\n".join(lines)


def record_error(exc: Exception) -> dict[str, str]:
    return {"error": type(exc).__name__, "message": str(exc)}


def gen_classify_cases(rng: random.Random, pool: list[str]) -> list[dict[str, Any]]:
    provider = HeuristicProvider()
    schema = output_schema()
    cases = []
    samples = [
        ("Fix crash in checkout page", "", [], []),
        ("Add dark mode feature", "", [], []),
        ("Refactor payment module", "", [], []),
        ("Update README documentation", "", [], []),
        ("Investigate slow query latency", "", [], []),
        ("Login token refresh fails", "OAuth token expiry", [], []),
        ("Big feature", "- Design the API\n- Then implement the endpoint\n- Then write the docs", [], []),
        ("", "", [], []),
        ("Ünïcode 日本語 emoji🙂", "NEAR( AND OR NOT", ["a'b", "x:y"], ["security"]),
    ]
    for _ in range(120):
        title = rng.choice(TITLE_CASES)(gen_text(rng, pool, rng.randint(1, 9)))
        description = gen_description(rng, pool)
        criteria = [gen_text(rng, pool, rng.randint(2, 8)) for _ in range(rng.randint(0, 4))]
        tags = [rng.choice(pool) for _ in range(rng.randint(0, 3))]
        samples.append((title, description, criteria, tags))
    for index, (title, description, criteria, tags) in enumerate(samples):
        prompt = build_prompt(title, description, criteria, tags)
        response = provider.generate_json(MODEL, system="s", prompt=prompt, schema=schema)
        validated = TaskClassification.model_validate(response.data).model_dump(mode="json")
        cases.append(
            {
                "id": f"classify-{index:03d}",
                "title": title,
                "description": description,
                "criteria": criteria,
                "tags": tags,
                "text_classify": classify_text(f"{title}\n{description}"),
                "prompt": prompt,
                "provider_data": response.data,
                "validated": validated,
                "input_hash": input_hash(title, description, criteria),
            }
        )
    return cases


def gen_decompose_cases(rng: random.Random, pool: list[str]) -> list[dict[str, Any]]:
    texts = [
        "- Design the API\n- Then implement the endpoint\n- Then write the docs",
        "1. Pick a store\n2) Migrate once the store is ready\n3. Cut over after verification",
        "* one\n* two then three",
        "no bullets here at all",
        "",
    ]
    for _ in range(60):
        items = [gen_text(rng, pool, rng.randint(1, 8)) for _ in range(rng.randint(1, 14))]
        bullet = rng.choice(["- ", "* ", "1. ", "3) "])
        texts.append("\n".join(bullet + item for item in items))
    cases = []
    for index, text in enumerate(texts):
        cases.append(
            {
                "id": f"decompose-{index:03d}",
                "description": text,
                "provider_data": HeuristicProvider()
                .generate_json(
                    MODEL, system="s", prompt=f"<task>{text}</task>", schema={"title": "TaskDecomposition"}
                )
                .data,
                "direct": decompose_text(text),
            }
        )
    return cases


def gen_narrative_cases(rng: random.Random, pool: list[str]) -> list[dict[str, Any]]:
    inputs: list[str] = [
        '{"summary": "Week done", "highlights": ["a", "b"], "opportunities": ["c"]}',
        "{}",
        '{"highlights": ' + json.dumps([str(i) for i in range(12)]) + "}",
        '{"summary": "", "opportunities": ' + json.dumps(["x"] * 7) + "}",
        '{"summary": null, "highlights": null, "opportunities": null}',
        '{"highlights": [1, 2.5, true, null, {"k": "v"}]}',
        "not json",
        "",
        "[1, 2]",
        '"just a string"',
    ]
    for _ in range(40):
        facts = {
            "summary": gen_text(rng, pool, rng.randint(0, 6)) if rng.random() < 0.8 else None,
            "highlights": [gen_text(rng, pool, rng.randint(1, 5)) for _ in range(rng.randint(0, 10))],
            "opportunities": [gen_text(rng, pool, rng.randint(1, 4)) for _ in range(rng.randint(0, 7))],
        }
        if rng.random() < 0.2:
            facts.pop("opportunities")
        inputs.append(json.dumps(facts, ensure_ascii=False))
    cases = []
    for index, facts_json in enumerate(inputs):
        try:
            output: dict[str, Any] = {"result": narrative_from_facts(facts_json)}
        except Exception as exc:
            output = {"raised": record_error(exc)}
        cases.append({"id": f"narrative-{index:03d}", "facts": facts_json, **output})
    return cases


def gen_embedding_cases(rng: random.Random, pool: list[str]) -> dict[str, Any]:
    texts = [
        "",
        "the",
        "OAuth token expiry",
        "Login token refresh fails",
        "Ünïcode 日本語 emoji🙂",
        "a a a a a a",
    ]
    texts += [gen_text(rng, pool, rng.randint(1, 15)) for _ in range(30)]
    return {
        "model": f"hash-embed-{EMBED_DIM}",
        "dimensions": EMBED_DIM,
        "cases": [
            {"id": f"embed-{i:03d}", "text": t, "vector": hash_embedding(t)} for i, t in enumerate(texts)
        ],
    }


def gen_taxonomy_cases(rng: random.Random, pool: list[str]) -> dict[str, Any]:
    prompts = []
    for index in range(20):
        title = gen_text(rng, pool, rng.randint(1, 6))
        description = gen_description(rng, pool)
        criteria = [gen_text(rng, pool, rng.randint(1, 5)) for _ in range(rng.randint(0, 22))]
        tags = [rng.choice(pool) for _ in range(rng.randint(0, 3))]
        prompts.append(
            {
                "id": f"prompt-{index:03d}",
                "title": title,
                "description": description,
                "criteria": criteria,
                "tags": tags,
                "prompt": build_prompt(title, description, criteria, tags),
                "input_hash": input_hash(title, description, criteria),
            }
        )
    payloads: list[dict[str, Any]] = [
        {
            "category": "bug_fix",
            "subtype": "bug_fix:backend",
            "domain": "backend",
            "complexity": "small",
            "confidence": 0.5,
            "tags": ["  a  ", "", "a", " b"],
            "affected_systems": ["  x"],
        },
        {
            "category": "other",
            "subtype": "other",
            "domain": "general",
            "complexity": "trivial",
            "confidence": 0,
        },
        {"category": "nope", "subtype": "x", "domain": "x", "complexity": "small", "confidence": 0.1},
        {"category": "feature", "subtype": "x", "domain": "x", "complexity": "huge", "confidence": 0.1},
        {"category": "feature", "subtype": "x", "domain": "x", "complexity": "small", "confidence": 1.5},
        {"category": "feature", "subtype": "x", "domain": "x", "complexity": "small", "confidence": -0.1},
        {
            "category": "feature",
            "subtype": "x",
            "domain": "x",
            "complexity": "small",
            "confidence": 0.3,
            "extra": 1,
        },
        {
            "category": "feature",
            "subtype": "s" * 101,
            "domain": "x",
            "complexity": "small",
            "confidence": 0.3,
        },
        {
            "category": "feature",
            "subtype": "x",
            "domain": "x",
            "complexity": "small",
            "confidence": 0.3,
            "tags": [f"t{i}" for i in range(11)],
        },
        {
            "category": "feature",
            "subtype": "x",
            "domain": "x",
            "complexity": "small",
            "confidence": 0.3,
            "rationale": "r" * 2001,
        },
        {"category": "feature", "subtype": "x", "domain": "x", "complexity": "small", "confidence": True},
        {"category": "feature", "subtype": "x", "domain": "x", "complexity": "small", "confidence": "0.3"},
        {"complexity": "small", "confidence": 0.3},
        {"category": "feature", "subtype": "x", "domain": "x", "complexity": "small", "confidence": "NaN"},
    ]
    validations = []
    for index, payload in enumerate(payloads):
        try:
            model = TaskClassification.model_validate(payload)
            result: dict[str, Any] = {"valid": True, "value": model.model_dump(mode="json")}
        except ValidationError as exc:
            errors = [f"{e['type']}:{'.'.join(str(p) for p in e['loc'])}" for e in exc.errors()]
            result = {"valid": False, "errors": errors}
        validations.append({"id": f"validate-{index:03d}", "payload": payload, **result})
    return {
        "taxonomy_version": TAXONOMY_VERSION,
        "prompt_version": PROMPT_VERSION,
        "prompts": prompts,
        "validations": validations,
    }


def gen_fts_cases(rng: random.Random) -> list[dict[str, Any]]:
    queries = [
        "token refresh",
        "Login token refresh fails",
        "",
        "   ",
        "!!!",
        '"quoted phrase"',
        "NEAR(a b, 2)",
        "a OR b AND NOT c",
        "prefix*",
        "col:value",
        "-excluded",
        "^start",
        "ünïcode 日本語",
        "emoji🙂 test",
        "tab\tsplit\nnewline",
        'a\'b "c" `d`',
        "\x00nul\x00",
        "under_score dot.sep",
        "api-gateway CVE-2026-1234",
        " ".join(f"tok{i}" for i in range(45)),
        " ".join(["dup"] * 50 + ["unique"]),
    ]
    alphabet = [*"ab c\"*-():^'\\.,;!?[]{}", "AND", "OR", "NEAR", "é", "日", "🙂", "\t"]
    for _ in range(60):
        queries.append("".join(rng.choice(alphabet) for _ in range(rng.randint(0, 30))))
    cases = []
    for index, query in enumerate(queries):
        cases.append(
            {
                "id": f"fts-{index:03d}",
                "query": query,
                "all": to_fts_query(query),
                "prefix_false": to_fts_query(query, prefix=False),
                "match_any": to_fts_query(query, match_any=True),
                "match_any_no_prefix": to_fts_query(query, prefix=False, match_any=True),
            }
        )
    return cases


def fuse(
    lexical: list[tuple[str, float]], semantic: list[tuple[str, float]], limit: int
) -> list[dict[str, Any]]:
    """Transcription of the reciprocal rank fusion block in SearchService.search (service.py)."""
    if not semantic:
        # SearchService.search returns the lexical list unchanged when no semantic hits exist.
        return [
            {"task_id": tid, "score": round(-rank, 4), "lexical_rank": i, "semantic_rank": None}
            for i, (tid, rank) in enumerate(lexical[:limit], start=1)
        ]
    fused: dict[str, dict[str, Any]] = {}
    scores: dict[str, float] = {}
    for rank, (tid, _bm25) in enumerate(lexical, start=1):
        scores[tid] = scores.get(tid, 0.0) + 1.0 / (RRF_K + rank)
        fused[tid] = {"task_id": tid, "lexical_rank": rank, "semantic_rank": None}
    for rank, (tid, _sim) in enumerate(semantic, start=1):
        scores[tid] = scores.get(tid, 0.0) + 1.0 / (RRF_K + rank)
        base = fused.get(tid, {"task_id": tid, "lexical_rank": None, "semantic_rank": None})
        fused[tid] = {**base, "semantic_rank": rank}
    ordered = sorted(fused.values(), key=lambda h: -scores[h["task_id"]])[:limit]
    return [{**h, "score": round(scores[h["task_id"]], 6)} for h in ordered]


def gen_search_cases(rng: random.Random) -> dict[str, Any]:
    ids = [f"t-{i:02d}" for i in range(15)]
    fusion = []
    for index in range(40):
        lex_ids = rng.sample(ids, rng.randint(0, 12))
        sem_ids = rng.sample(ids, rng.randint(0, 12))
        lexical = [(tid, round(rng.uniform(0.0, 9.0), 4)) for tid in lex_ids]
        lexical.sort(key=lambda pair: pair[1])
        semantic = [(tid, round(rng.uniform(-1.0, 1.0), 4)) for tid in sem_ids]
        semantic.sort(key=lambda pair: -pair[1])
        limit = rng.randint(1, 20)
        fusion.append(
            {
                "id": f"rrf-{index:03d}",
                "limit": limit,
                "lexical": [{"task_id": t, "bm25": b} for t, b in lexical],
                "semantic": [{"task_id": t, "similarity": s} for t, s in semantic],
                "fused": fuse(lexical, semantic, limit),
            }
        )
    cosines = []
    vector_cases: list[tuple[list[float], list[float]]] = [
        ([1.0, 0.0], [1.0, 0.0]),
        ([1.0, 0.0], [0.0, 1.0]),
        ([1.0, 2.0, 3.0], [-1.0, 0.5, 2.0]),
        ([0.0, 0.0], [1.0, 1.0]),
        ([1.0, 2.0], [1.0, 2.0, 3.0]),
        ([], []),
        ([1e-30, 1e-30], [1e-30, 2e-30]),
        ([1e30, -1e30, 3.5], [2.25, 1e30, -7.0]),
    ]
    for _ in range(40):
        dim = rng.randint(1, 8)
        vector_cases.append(
            (
                [rng.uniform(-1.0, 1.0) for _ in range(dim)],
                [rng.uniform(-1.0, 1.0) for _ in range(dim)],
            )
        )
    for index, (a, b) in enumerate(vector_cases):
        value = cosine(a, b)
        cosines.append({"id": f"cos-{index:03d}", "a": a, "b": b, "cosine": value})
    return {"rrf_k": RRF_K, "cosine": cosines, "fusion": fusion}


def dump(name: str, payload: Any) -> None:
    text = json.dumps(payload, sort_keys=True, indent=2, ensure_ascii=False) + "\n"
    (OUT / f"{name}.json").write_bytes(text.encode("utf-8"))


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    rng = random.Random(SEED)  # noqa: S311 - reproducible fixture generation, not security
    pool = vocabulary()
    dump("classify", {"model": MODEL, "cases": gen_classify_cases(rng, pool)})
    dump("decompose", {"model": MODEL, "cases": gen_decompose_cases(rng, pool)})
    dump("narrative", {"cases": gen_narrative_cases(rng, pool)})
    dump("embedding", gen_embedding_cases(rng, pool))
    dump("taxonomy", gen_taxonomy_cases(rng, pool))
    dump("fts", {"cases": gen_fts_cases(rng)})
    dump("search", gen_search_cases(rng))


if __name__ == "__main__":
    main()
