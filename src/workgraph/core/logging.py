"""Structured JSON logging on top of the standard library."""

from __future__ import annotations

import json
import logging
import sys
from datetime import UTC, datetime
from logging.handlers import RotatingFileHandler
from pathlib import Path
from typing import Any

_RESERVED = set(logging.LogRecord("", 0, "", 0, "", (), None).__dict__) | {"message", "asctime"}


class JsonFormatter(logging.Formatter):
    def format(self, record: logging.LogRecord) -> str:
        payload: dict[str, Any] = {
            "ts": datetime.fromtimestamp(record.created, UTC).isoformat(),
            "level": record.levelname.lower(),
            "logger": record.name,
            "msg": record.getMessage(),
        }
        for key, value in record.__dict__.items():
            if key not in _RESERVED and not key.startswith("_"):
                payload[key] = value
        if record.exc_info:
            payload["exc"] = self.formatException(record.exc_info)
        return json.dumps(payload, default=str)


_configured = False


def configure_logging(level: str = "INFO", log_dir: Path | None = None, *, stderr: bool = True) -> None:
    """Configure the ``workgraph`` logger tree once per process."""
    global _configured  # noqa: PLW0603 - process-wide one-time setup
    root = logging.getLogger("workgraph")
    root.setLevel(level.upper())
    if _configured:
        return
    formatter = JsonFormatter()
    if stderr:
        handler = logging.StreamHandler(sys.stderr)
        handler.setFormatter(formatter)
        root.addHandler(handler)
    if log_dir is not None:
        log_dir.mkdir(parents=True, exist_ok=True)
        file_handler = RotatingFileHandler(
            log_dir / "workgraph.log", maxBytes=5_000_000, backupCount=5, encoding="utf-8"
        )
        file_handler.setFormatter(formatter)
        root.addHandler(file_handler)
    root.propagate = False
    _configured = True


def get_logger(name: str) -> logging.Logger:
    return logging.getLogger(name if name.startswith("workgraph") else f"workgraph.{name}")
