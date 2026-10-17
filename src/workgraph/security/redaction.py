"""Redaction of secrets and sensitive payloads before anything is persisted."""

from __future__ import annotations

import re
from typing import Any

SECRET_KEY_RE = re.compile(
    r"(pass(word|wd)?|secret|token|api[_-]?key|auth(orization)?|credential|private[_-]?key|session[_-]?key|cookie)",
    re.IGNORECASE,
)
SECRET_VALUE_PATTERNS = [
    re.compile(r"sk-[A-Za-z0-9_-]{16,}"),  # OpenAI / Anthropic style keys
    re.compile(r"sk-ant-[A-Za-z0-9_-]{16,}"),
    re.compile(r"gh[pousr]_[A-Za-z0-9]{20,}"),  # GitHub tokens
    re.compile(r"github_pat_[A-Za-z0-9_]{20,}"),
    re.compile(r"AKIA[0-9A-Z]{16}"),  # AWS access key id
    re.compile(r"xox[baprs]-[A-Za-z0-9-]{10,}"),  # Slack
    re.compile(r"(?i)bearer\s+[A-Za-z0-9._~+/=-]{12,}"),
    re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----"),
    re.compile(r"(?i)(password|passwd|pwd|token|secret|api[_-]?key)\s*[=:]\s*\S+"),
    re.compile(r"eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}"),  # JWT
]
REDACTED = "[REDACTED]"


def redact_text(value: str, max_chars: int | None = None) -> str:
    for pattern in SECRET_VALUE_PATTERNS:
        value = pattern.sub(REDACTED, value)
    if max_chars is not None and len(value) > max_chars:
        value = value[:max_chars] + f"... [truncated {len(value) - max_chars} chars]"
    return value


def redact(value: Any, *, max_chars: int = 4000, depth: int = 0) -> Any:
    """Recursively redact secret-looking keys and values, bounding size and depth."""
    if depth > 8:
        return "[nested too deep]"
    if isinstance(value, dict):
        out: dict[str, Any] = {}
        for i, (key, item) in enumerate(value.items()):
            if i >= 200:
                out["..."] = f"{len(value) - 200} more keys"
                break
            if SECRET_KEY_RE.search(str(key)):
                out[str(key)] = REDACTED
            else:
                out[str(key)] = redact(item, max_chars=max_chars, depth=depth + 1)
        return out
    if isinstance(value, list):
        items = [redact(v, max_chars=max_chars, depth=depth + 1) for v in value[:100]]
        if len(value) > 100:
            items.append(f"... {len(value) - 100} more items")
        return items
    if isinstance(value, str):
        return redact_text(value, max_chars)
    return value


def summarize(value: Any) -> Any:
    """Shape-only summary (keys, types and sizes) for payloads that must not be stored."""
    if isinstance(value, dict):
        return {str(k): summarize(v) for k, v in list(value.items())[:50]}
    if isinstance(value, list):
        return {"type": "list", "length": len(value)}
    if isinstance(value, str):
        return {"type": "str", "length": len(value)}
    return {"type": type(value).__name__}
