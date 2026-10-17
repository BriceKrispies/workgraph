"""The acting identity and causal metadata carried through every service call."""

from __future__ import annotations

import getpass
from dataclasses import dataclass, field, replace

from workgraph.core.clock import new_id
from workgraph.core.models.enums import ActorKind
from workgraph.security.permissions import (
    DEFAULT_AGENT_PERMISSIONS,
    HUMAN_PERMISSIONS,
    SYSTEM_PERMISSIONS,
    Permission,
)


@dataclass(frozen=True)
class RequestContext:
    actor: str
    actor_kind: ActorKind
    permissions: frozenset[Permission]
    correlation_id: str = field(default_factory=new_id)
    causation_id: str | None = None
    attempt_id: str | None = None
    automation_depth: int = 0

    def caused_by(self, event_id: str | None) -> RequestContext:
        return replace(self, causation_id=event_id)

    def with_attempt(self, attempt_id: str | None) -> RequestContext:
        return replace(self, attempt_id=attempt_id)

    def deeper(self, causation_id: str | None) -> RequestContext:
        return replace(self, causation_id=causation_id, automation_depth=self.automation_depth + 1)

    @classmethod
    def human(cls, name: str | None = None) -> RequestContext:
        try:
            user = name or getpass.getuser()
        except Exception:  # getuser can fail in stripped environments
            user = "local"
        return cls(actor=f"human:{user}", actor_kind=ActorKind.HUMAN, permissions=HUMAN_PERMISSIONS)

    @classmethod
    def agent(cls, name: str, permissions: frozenset[Permission] | None = None) -> RequestContext:
        return cls(
            actor=f"agent:{name}",
            actor_kind=ActorKind.AGENT,
            permissions=permissions if permissions is not None else DEFAULT_AGENT_PERMISSIONS,
        )

    @classmethod
    def system(cls, component: str = "workgraph") -> RequestContext:
        return cls(actor=f"system:{component}", actor_kind=ActorKind.SYSTEM, permissions=SYSTEM_PERMISSIONS)

    @classmethod
    def automation(cls, rule_name: str, permissions: frozenset[Permission], depth: int) -> RequestContext:
        return cls(
            actor=f"automation:{rule_name}",
            actor_kind=ActorKind.AUTOMATION,
            permissions=permissions,
            automation_depth=depth,
        )
