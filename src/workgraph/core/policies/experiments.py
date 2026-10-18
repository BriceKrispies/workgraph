"""Experiment protocol validation.

A protocol pins everything a trial depends on before the first trial runs: the model per arm (by
full id), the brief, the size metric, the band range, the staircase and the budget. Bare model
aliases are rejected because the same alias has resolved to different models over time.
"""

from __future__ import annotations

from typing import Any

from workgraph.core.errors import ValidationFailed

# Aliases resolve to whatever model is current, so a trial could run a different model than its arm names.
MODEL_ALIASES = frozenset({"haiku", "sonnet", "opus", "fable", "inherit"})

REQUIRED_KEYS = frozenset({"arms", "brief_template", "primary_size", "bands", "staircase", "budget"})
# ``oracle_template`` overrides the replay oracle (an argv list), ``env`` adds variables to oracle, gate and
# setup runs (e.g. a shared CARGO_TARGET_DIR), ``pool`` selects the candidate tasks (tag, repository), and
# ``setup`` lists commands run in each trial's fresh clone before the trial starts, and ``clone`` selects how
# the clone is made: "shared" (default) keeps alternates to the source object store, "isolated" copies only the
# base branch's objects, so the solution's objects are absent from the clone.
OPTIONAL_KEYS = frozenset(
    {"refine", "repeat", "control", "gates", "seed", "oracle_template", "env", "pool", "setup", "clone"}
)
CLONE_MODES = ("shared", "isolated")
STAIRCASE_KEYS = ("per_visit", "up", "down", "max_reversals", "max_trials")


def _at_least_one(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value >= 1


def _is_int(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def model_problem(model: object) -> str | None:
    """Why a model id is not acceptable for a trial, or None when it is a pinned full id."""
    if not isinstance(model, str) or not model.strip():
        return "is not a non-empty string"
    if model.strip().lower() in MODEL_ALIASES:
        return f"is the alias {model!r}; pin a full model id such as 'claude-haiku-5-5'"
    # A pinned id names a version ("claude-haiku-5-5"); a family name such as "claude-haiku" is still an alias.
    if not any(ch.isdigit() for ch in model):
        return "is not a pinned model id (expected a full id such as 'claude-haiku-5-5')"
    return None


def _positive_ints(section: object, name: str, keys: tuple[str, ...], problems: list[str]) -> None:
    if not isinstance(section, dict):
        problems.append(f"{name} must be an object")
        return
    for key in keys:
        value = section.get(key)
        if not _at_least_one(value):
            problems.append(f"{name}.{key} must be an integer >= 1")
    unknown = sorted(set(section) - set(keys))
    if unknown:
        problems.append(f"{name} has unknown keys: {', '.join(unknown)}")


def protocol_problems(protocol: object) -> list[str]:
    """Every problem with a protocol, so one attempt reports them all."""
    if not isinstance(protocol, dict):
        return ["protocol must be a JSON object"]
    problems: list[str] = []
    missing = sorted(REQUIRED_KEYS - set(protocol))
    if missing:
        problems.append(f"missing keys: {', '.join(missing)}")
    unknown = sorted(set(protocol) - REQUIRED_KEYS - OPTIONAL_KEYS)
    if unknown:
        problems.append(f"unknown keys: {', '.join(unknown)}")

    arms = protocol.get("arms")
    if "arms" in protocol:
        if not isinstance(arms, list) or not arms:
            problems.append("arms must be a non-empty list of {name, model} objects")
        else:
            names: list[str] = []
            for index, arm in enumerate(arms):
                label = f"arms[{index}]"
                if not isinstance(arm, dict) or set(arm) != {"name", "model"}:
                    problems.append(f"{label} must be an object with exactly name and model")
                    continue
                name = arm["name"]
                if not isinstance(name, str) or not name.strip():
                    problems.append(f"{label}.name must be a non-empty string")
                elif name in names:
                    problems.append(f"{label}.name {name!r} is used by another arm")
                else:
                    names.append(name)
                problem = model_problem(arm["model"])
                if problem:
                    problems.append(f"{label}.model {problem}")

    for key in ("brief_template", "primary_size"):
        if key in protocol and (not isinstance(protocol[key], str) or not protocol[key].strip()):
            problems.append(f"{key} must be a non-empty string")

    if "bands" in protocol:
        bands = protocol["bands"]
        if not isinstance(bands, dict) or set(bands) != {"min", "max"}:
            problems.append("bands must be an object with exactly min and max")
        elif not (_is_int(bands["min"]) and _is_int(bands["max"])):
            problems.append("bands.min and bands.max must be integers")
        elif bands["min"] > bands["max"]:
            problems.append("bands.min must not exceed bands.max")

    if "staircase" in protocol:
        _positive_ints(protocol["staircase"], "staircase", STAIRCASE_KEYS, problems)
    if "budget" in protocol:
        _positive_ints(protocol["budget"], "budget", ("trials",), problems)

    for key in ("refine", "repeat", "control"):
        if key in protocol and not isinstance(protocol[key], dict):
            problems.append(f"{key} must be an object")
    control = protocol.get("control")
    if isinstance(control, dict) and "model" in control:
        problem = model_problem(control["model"])
        if problem:
            problems.append(f"control.model {problem}")

    if "gates" in protocol:
        gates = protocol["gates"]
        if not isinstance(gates, list) or not all(
            isinstance(gate, list) and gate and all(isinstance(part, str) and part for part in gate)
            for gate in gates
        ):
            problems.append("gates must be a list of non-empty command lists of strings")
    if "setup" in protocol:
        setup = protocol["setup"]
        if not isinstance(setup, list) or not all(
            isinstance(command, list) and command and all(isinstance(part, str) and part for part in command)
            for command in setup
        ):
            problems.append("setup must be a list of non-empty command lists of strings")
    if "oracle_template" in protocol:
        template = protocol["oracle_template"]
        if (
            not isinstance(template, list)
            or not template
            or not all(isinstance(p, str) and p for p in template)
        ):
            problems.append("oracle_template must be a non-empty list of strings")
    if "env" in protocol:
        env = protocol["env"]
        if not isinstance(env, dict) or not all(
            isinstance(k, str) and isinstance(v, str) for k, v in env.items()
        ):
            problems.append("env must be an object of string values")
    if "clone" in protocol and protocol["clone"] not in CLONE_MODES:
        problems.append(f"clone must be one of: {', '.join(CLONE_MODES)}")
    if "pool" in protocol and not isinstance(protocol["pool"], dict):
        problems.append("pool must be an object")
    if "seed" in protocol and not _is_int(protocol["seed"]):
        problems.append("seed must be an integer")
    return problems


def validate_protocol(protocol: object) -> dict[str, Any]:
    """Return the protocol unchanged when valid; otherwise raise naming every problem."""
    problems = protocol_problems(protocol)
    if problems:
        raise ValidationFailed(
            f"invalid experiment protocol: {'; '.join(problems)}", details={"problems": problems}
        )
    assert isinstance(protocol, dict)
    return protocol
