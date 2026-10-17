"""The controlled, versioned task taxonomy and the classifier output schema."""

from __future__ import annotations

import hashlib
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, field_validator

TAXONOMY_VERSION = "2026.1"
PROMPT_VERSION = "classify-v1"

CATEGORIES = (
    "bug_fix",
    "feature",
    "refactor",
    "maintenance",
    "infrastructure",
    "performance",
    "security",
    "testing",
    "documentation",
    "research",
    "architecture",
    "other",
)
CATEGORY_DESCRIPTIONS = {
    "bug_fix": "Correcting behaviour that is wrong or broken.",
    "feature": "New user- or developer-facing capability.",
    "refactor": "Restructuring code without changing behaviour.",
    "maintenance": "Dependency upgrades, chores, deprecations, housekeeping.",
    "infrastructure": "CI/CD, build, deployment, environments, tooling.",
    "performance": "Speed, latency, memory or throughput improvements.",
    "security": "Vulnerabilities, authn/authz, secrets, hardening.",
    "testing": "Adding or fixing tests, coverage, flakiness.",
    "documentation": "Docs, READMEs, guides, comments.",
    "research": "Investigation, spikes, evaluations, prototypes.",
    "architecture": "System design, boundaries, large structural decisions.",
    "other": "Does not fit any category above.",
}
COMPLEXITY = ("trivial", "small", "medium", "large", "very_large")

Category = Literal[
    "bug_fix",
    "feature",
    "refactor",
    "maintenance",
    "infrastructure",
    "performance",
    "security",
    "testing",
    "documentation",
    "research",
    "architecture",
    "other",
]
Complexity = Literal["trivial", "small", "medium", "large", "very_large"]


class TaskClassification(BaseModel):
    """Structured output requested from the classifier (also its JSON Schema)."""

    model_config = ConfigDict(title="TaskClassification", extra="forbid")

    category: Category
    subtype: str = Field(max_length=100)
    domain: str = Field(max_length=100)
    affected_systems: list[str] = Field(default_factory=list, max_length=10)
    complexity: Complexity
    tags: list[str] = Field(default_factory=list, max_length=10)
    confidence: float = Field(ge=0.0, le=1.0)
    rationale: str = Field(default="", max_length=2000)

    @field_validator("tags", "affected_systems")
    @classmethod
    def _clean(cls, values: list[str]) -> list[str]:
        return list(dict.fromkeys(v.strip()[:100] for v in values if v and v.strip()))


def output_schema() -> dict[str, object]:
    schema = TaskClassification.model_json_schema()
    schema["title"] = "TaskClassification"
    return schema


SYSTEM_PROMPT = (
    "You classify software engineering tasks into a fixed taxonomy. Respond with JSON that matches "
    "the provided schema exactly. Base the answer only on the task text. Use 'other' when nothing fits. "
    "Confidence is your calibrated probability that the category is correct."
)


def build_prompt(title: str, description: str, criteria: list[str], tags: list[str]) -> str:
    lines = ["Categories:"]
    lines += [f"- {name}: {CATEGORY_DESCRIPTIONS[name]}" for name in CATEGORIES]
    lines += [
        f"Complexity levels: {', '.join(COMPLEXITY)}",
        "",
        "Task to classify:",
        "<task>",
        f"Title: {title}",
    ]
    if description:
        lines += ["Description:", description[:8000]]
    if criteria:
        lines += ["Acceptance criteria:"] + [f"- {c}" for c in criteria[:20]]
    if tags:
        lines.append("Existing tags: " + ", ".join(tags))
    lines.append("</task>")
    return "\n".join(lines)


def input_hash(title: str, description: str, criteria: list[str]) -> str:
    material = "\x1f".join([PROMPT_VERSION, TAXONOMY_VERSION, title, description, *criteria])
    return hashlib.sha256(material.encode("utf-8")).hexdigest()
