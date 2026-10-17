from __future__ import annotations

import pytest
from pydantic import ValidationError

from workgraph.core.errors import ValidationFailed
from workgraph.verification.configs import ApplicableCheck, CheckSpec, checks_for_tier, parse_tier


def applicable(name: str, tier: str | None = None) -> ApplicableCheck:
    raw: dict[str, object] = {"name": name, "command": ["pytest"]}
    if tier is not None:
        raw["tier"] = tier
    return ApplicableCheck(
        config_id="cfg",
        config_name="ci",
        scope_type="workspace",
        scope_id="ws",
        spec=CheckSpec.model_validate(raw),
    )


def test_check_spec_tier_defaults_to_full():
    assert CheckSpec(name="tests", command=["pytest"]).tier == "full"


def test_stored_config_without_tier_loads_as_full():
    stored = {"name": "tests", "kind": "test", "command": ["pytest", "-q"], "required": True}
    assert CheckSpec.model_validate(stored).tier == "full"


def test_check_spec_parses_explicit_tier_and_rejects_unknown_values():
    assert CheckSpec(name="lint", command=["ruff", "check"], tier="fast").tier == "fast"
    with pytest.raises(ValidationError):
        CheckSpec.model_validate({"name": "lint", "command": ["ruff", "check"], "tier": "slow"})
    with pytest.raises(ValidationError):
        CheckSpec.model_validate({"name": "lint", "command": ["ruff"], "tiers": "fast"})


def test_checks_for_tier_filters_preserving_order():
    checks = [applicable("lint", "fast"), applicable("unit"), applicable("types", "fast")]
    assert [c.spec.name for c in checks_for_tier(checks, "fast")] == ["lint", "types"]
    assert [c.spec.name for c in checks_for_tier(checks, "full")] == ["unit"]
    assert [c.spec.name for c in checks_for_tier(checks, None)] == ["lint", "unit", "types"]
    assert checks_for_tier([], "fast") == []


def test_parse_tier_accepts_known_tiers_and_none():
    assert parse_tier(None) is None
    assert parse_tier("fast") == "fast"
    assert parse_tier("full") == "full"
    with pytest.raises(ValidationFailed):
        parse_tier("slow")
