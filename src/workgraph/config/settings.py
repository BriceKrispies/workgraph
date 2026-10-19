"""Validated configuration files.

Two layers exist:

* ``config.toml`` - global settings (server, providers, agents, job limits).
* ``workspaces/<slug>.toml`` - per-workspace overrides merged over the global file.

Configuration files are *trusted* input written by the human operator. Commands
that Workgraph executes (agent executables, verification checks) are only ever
taken from trusted configuration or from database records created by an actor
holding the ``config:manage`` permission - never from task text.
"""

from __future__ import annotations

import os
import tomllib
from pathlib import Path
from typing import Annotated, Any, Literal

import tomli_w
from pydantic import BaseModel, ConfigDict, Field, ValidationError, field_validator

from workgraph.config.paths import AppPaths
from workgraph.core.errors import ConfigError
from workgraph.core.fsutil import atomic_write_text

CONFIG_SCHEMA_VERSION = 2


class _Strict(BaseModel):
    model_config = ConfigDict(extra="forbid")


class ServerConfig(_Strict):
    host: str = "127.0.0.1"
    port: int = Field(default=8765, ge=1, le=65535)
    allow_non_loopback: bool = False

    @field_validator("host")
    @classmethod
    def _host(cls, value: str) -> str:
        if not value.strip():
            raise ValueError("host must not be empty")
        return value


class OllamaProviderConfig(_Strict):
    type: Literal["ollama"] = "ollama"
    base_url: str = "http://127.0.0.1:11434"
    timeout_seconds: float = Field(default=120.0, gt=0)
    max_retries: int = Field(default=2, ge=0, le=10)
    backoff_seconds: float = Field(default=1.0, ge=0)
    api_key_env: str | None = None


class HeuristicProviderConfig(_Strict):
    """Offline deterministic provider (keyword classifier + hashed embeddings)."""

    type: Literal["heuristic"] = "heuristic"


class OpenAICompatibleProviderConfig(_Strict):
    """Any server implementing the OpenAI chat/embeddings HTTP API (llama.cpp, vLLM, LM Studio)."""

    type: Literal["openai_compatible"] = "openai_compatible"
    base_url: str
    timeout_seconds: float = Field(default=120.0, gt=0)
    max_retries: int = Field(default=2, ge=0, le=10)
    backoff_seconds: float = Field(default=1.0, ge=0)
    api_key_env: str | None = None


ProviderConfig = Annotated[
    OllamaProviderConfig | HeuristicProviderConfig | OpenAICompatibleProviderConfig,
    Field(discriminator="type"),
]


def _default_providers() -> dict[
    str, OllamaProviderConfig | HeuristicProviderConfig | OpenAICompatibleProviderConfig
]:
    return {"ollama": OllamaProviderConfig(), "heuristic": HeuristicProviderConfig()}


class ModelSelection(_Strict):
    provider: str
    model: str


class LLMConfig(_Strict):
    providers: dict[str, ProviderConfig] = Field(default_factory=_default_providers)
    classification: ModelSelection = Field(
        default_factory=lambda: ModelSelection(provider="ollama", model="qwen2.5:7b-instruct")
    )
    embeddings: ModelSelection = Field(
        default_factory=lambda: ModelSelection(provider="ollama", model="nomic-embed-text")
    )
    summarization: ModelSelection = Field(
        default_factory=lambda: ModelSelection(provider="ollama", model="qwen2.5:7b-instruct")
    )
    auto_classify: bool = True
    auto_embed: bool = True


class ClaudeCodeAgentConfig(_Strict):
    type: Literal["claude_code"] = "claude_code"
    executable: str = "claude"
    model: str | None = None
    permission_mode: Literal["acceptEdits", "auto", "bypassPermissions", "manual", "dontAsk", "plan"] = (
        "acceptEdits"
    )
    allowed_tools: list[str] = Field(default_factory=list)
    max_turns: int | None = Field(default=None, ge=1)
    max_budget_usd: float | None = Field(default=None, gt=0)
    timeout_seconds: float = Field(default=3600.0, gt=0)
    extra_args: list[str] = Field(default_factory=list)


class CommandAgentConfig(_Strict):
    """A generic executable speaking the Workgraph JSON-lines agent protocol."""

    type: Literal["command"] = "command"
    executable: str
    args: list[str] = Field(default_factory=list)
    env: dict[str, str] = Field(default_factory=dict)
    timeout_seconds: float = Field(default=1800.0, gt=0)
    supports_progress: bool = True
    supports_usage: bool = False
    supports_resume: bool = False


AgentConfig = Annotated[ClaudeCodeAgentConfig | CommandAgentConfig, Field(discriminator="type")]


def _default_agents() -> dict[str, ClaudeCodeAgentConfig | CommandAgentConfig]:
    return {"claude_code": ClaudeCodeAgentConfig()}


class ExecutionConfig(_Strict):
    enabled: bool = False
    """Agent code execution is opt-in. Task tracking and Git analysis never require it."""
    default_agent: str = "claude_code"
    max_concurrent_runs: int = Field(default=2, ge=1, le=64)
    max_attempts_per_task: int = Field(default=3, ge=1, le=20)
    auto_schedule: bool = False
    isolation: Literal["worktree", "in_place"] = "worktree"
    allow_push: bool = False
    allow_merge: bool = False


class JobLimitsConfig(_Strict):
    worker_concurrency: int = Field(default=4, ge=1, le=64)
    gpu_concurrency: int = Field(default=1, ge=1, le=16)
    per_agent_concurrency: int = Field(default=2, ge=1, le=64)
    per_repository_concurrency: int = Field(default=1, ge=1, le=64)
    verify_concurrency: int = Field(default=1, ge=1, le=64)
    lease_seconds: int = Field(default=60, ge=5, le=3600)
    default_max_attempts: int = Field(default=5, ge=1, le=50)


class RetentionConfig(_Strict):
    telemetry_payload_days: int = Field(default=30, ge=1)
    run_log_days: int = Field(default=90, ge=1)


class ClaudeIntegrationConfig(_Strict):
    capture_tool_payloads: bool = False
    max_payload_chars: int = Field(default=4000, ge=0)


class AutomationConfig(_Strict):
    max_depth: int = Field(default=3, ge=1, le=10)


class StacksConfig(_Strict):
    auto_accept: bool = True
    """Land a stack as soon as its full verification passes, instead of waiting for the parent's acceptance."""


class VerificationConfig(_Strict):
    auto_accept: bool = True
    """Accept a task as soon as its full verification passes, instead of waiting for `workgraph task accept`."""


class GlobalConfig(_Strict):
    schema_version: int = CONFIG_SCHEMA_VERSION
    default_workspace: str | None = None
    server: ServerConfig = Field(default_factory=ServerConfig)
    llm: LLMConfig = Field(default_factory=LLMConfig)
    agents: dict[str, AgentConfig] = Field(default_factory=_default_agents)
    execution: ExecutionConfig = Field(default_factory=ExecutionConfig)
    jobs: JobLimitsConfig = Field(default_factory=JobLimitsConfig)
    stacks: StacksConfig = Field(default_factory=StacksConfig)
    verification: VerificationConfig = Field(default_factory=VerificationConfig)
    retention: RetentionConfig = Field(default_factory=RetentionConfig)
    claude: ClaudeIntegrationConfig = Field(default_factory=ClaudeIntegrationConfig)
    automation: AutomationConfig = Field(default_factory=AutomationConfig)


class WorkspaceOverrides(_Strict):
    """Per-workspace file. Every section is optional and overrides the global value."""

    schema_version: int = CONFIG_SCHEMA_VERSION
    llm: dict[str, Any] | None = None
    agents: dict[str, AgentConfig] | None = None
    execution: dict[str, Any] | None = None
    jobs: dict[str, Any] | None = None
    stacks: dict[str, Any] | None = None
    verification: dict[str, Any] | None = None


def _migrate(raw: dict[str, Any]) -> dict[str, Any]:
    """Upgrade older config documents in place to the current schema."""
    version = int(raw.get("schema_version", 1))
    if version < 2:
        # v1 kept agent execution under "agents.enabled"; v2 moved it into [execution].
        agents = raw.get("agents")
        if isinstance(agents, dict) and "enabled" in agents:
            raw.setdefault("execution", {})["enabled"] = bool(agents.pop("enabled"))
        version = 2
    raw["schema_version"] = version
    return raw


def _deep_merge(base: dict[str, Any], override: dict[str, Any]) -> dict[str, Any]:
    merged = dict(base)
    for key, value in override.items():
        if isinstance(value, dict) and isinstance(merged.get(key), dict):
            merged[key] = _deep_merge(merged[key], value)
        else:
            merged[key] = value
    return merged


class ConfigStore:
    """Reads, validates, migrates and atomically writes configuration files."""

    def __init__(self, paths: AppPaths) -> None:
        self.paths = paths

    def _read_toml(self, path: Path) -> dict[str, Any]:
        if not path.exists():
            return {}
        try:
            return tomllib.loads(path.read_text(encoding="utf-8"))
        except tomllib.TOMLDecodeError as exc:
            raise ConfigError(f"{path} is not valid TOML: {exc}") from exc

    def load_global(self) -> GlobalConfig:
        raw = _migrate(self._read_toml(self.paths.config_file))
        try:
            return GlobalConfig.model_validate(raw)
        except ValidationError as exc:
            raise ConfigError(
                f"{self.paths.config_file} failed validation", details={"errors": exc.errors()}
            ) from exc

    def save_global(self, config: GlobalConfig) -> None:
        data = config.model_dump(mode="json", exclude_none=True)
        atomic_write_text(self.paths.config_file, tomli_w.dumps(data))

    def workspace_file(self, slug: str) -> Path:
        return self.paths.workspace_config_dir / f"{slug}.toml"

    def load_workspace_overrides(self, slug: str) -> WorkspaceOverrides:
        raw = _migrate(self._read_toml(self.workspace_file(slug)))
        try:
            return WorkspaceOverrides.model_validate(raw)
        except ValidationError as exc:
            raise ConfigError(
                f"{self.workspace_file(slug)} failed validation", details={"errors": exc.errors()}
            ) from exc

    def save_workspace_overrides(self, slug: str, overrides: WorkspaceOverrides) -> None:
        data = overrides.model_dump(mode="json", exclude_none=True)
        atomic_write_text(self.workspace_file(slug), tomli_w.dumps(data))

    def effective(self, workspace_slug: str | None) -> GlobalConfig:
        """Global configuration with the workspace's overrides applied."""
        base = self.load_global()
        if not workspace_slug:
            return base
        overrides = self.load_workspace_overrides(workspace_slug).model_dump(
            mode="json", exclude_none=True, exclude={"schema_version"}
        )
        if not overrides:
            return base
        merged = _deep_merge(base.model_dump(mode="json"), overrides)
        try:
            return GlobalConfig.model_validate(merged)
        except ValidationError as exc:
            raise ConfigError(
                f"workspace overrides for {workspace_slug} produce an invalid configuration",
                details={"errors": exc.errors()},
            ) from exc

    def validate_files(self) -> list[str]:
        """Return human-readable problems across all config files (empty when valid)."""
        problems: list[str] = []
        try:
            self.load_global()
        except ConfigError as exc:
            problems.append(f"{exc.message}: {exc.details.get('errors', '')}")
        if self.paths.workspace_config_dir.exists():
            for file in sorted(self.paths.workspace_config_dir.glob("*.toml")):
                try:
                    self.effective(file.stem)
                except ConfigError as exc:
                    problems.append(f"{exc.message}: {exc.details.get('errors', '')}")
        return problems

    def migrate_files(self) -> list[Path]:
        """Rewrite config files in the current schema. Returns rewritten paths."""
        rewritten: list[Path] = []
        if self.paths.config_file.exists():
            raw = self._read_toml(self.paths.config_file)
            if int(raw.get("schema_version", 1)) < CONFIG_SCHEMA_VERSION:
                self.save_global(self.load_global())
                rewritten.append(self.paths.config_file)
        if self.paths.workspace_config_dir.exists():
            for file in sorted(self.paths.workspace_config_dir.glob("*.toml")):
                raw = self._read_toml(file)
                if int(raw.get("schema_version", 1)) < CONFIG_SCHEMA_VERSION:
                    self.save_workspace_overrides(file.stem, self.load_workspace_overrides(file.stem))
                    rewritten.append(file)
        return rewritten


def secret_from_env(name: str | None) -> str | None:
    """Credentials are only ever read from the environment, never from config files."""
    if not name:
        return None
    return os.environ.get(name)
