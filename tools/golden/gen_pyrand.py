"""Golden vectors for `wg-pyrand`: CPython's `random.Random` output for fixed seeds.

Writes one JSON file per seed to `golden/pyrand/seed_<seed>.json`. Each file holds:

* `random`: the first 2000 values of `random()`;
* `getrandbits`: for k in 1..64, 16 consecutive `getrandbits(k)` results, drawn from one
  generator in k order;
* `choice`: for each population size n, 32 consecutive `choice(range(n))` results;
* `choices`: `choices(range(10), ...)` cases with no weights, integer weights, float weights
  and cumulative weights.

Output is deterministic: sorted keys, `\\n` line endings, UTF-8, so two runs are byte-identical.
"""

import json
import random
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT_DIR = ROOT / "golden" / "pyrand"

SEEDS = [0, 1, 2**32, 2**64 + 5, -1, -(2**64 + 5)]

RANDOM_COUNT = 2000
GETRANDBITS_DRAWS = 16
CHOICE_DRAWS = 32
CHOICE_SIZES = [1, 2, 3, 7, 10, 64, 100, 1000]
CHOICES_K = 50
CHOICES_POPULATION = list(range(10))
INT_WEIGHTS = [1, 2, 3, 5, 8, 13, 21, 34, 55, 89]
FLOAT_WEIGHTS = [0.1, 0.25, 0.5, 1.0, 2.0, 0.001, 3.5, 0.75, 1e-9, 10.0]


def _generator(seed: int) -> random.Random:
    # Deterministic reference output, not a security boundary.
    return random.Random(seed)  # noqa: S311


def vectors_for(seed: int) -> dict:
    out: dict = {"seed": seed}

    rng = _generator(seed)
    out["random"] = [rng.random() for _ in range(RANDOM_COUNT)]

    rng = _generator(seed)
    out["getrandbits"] = {
        str(k): [rng.getrandbits(k) for _ in range(GETRANDBITS_DRAWS)] for k in range(1, 65)
    }

    rng = _generator(seed)
    out["choice"] = {str(n): [rng.choice(range(n)) for _ in range(CHOICE_DRAWS)] for n in CHOICE_SIZES}

    rng = _generator(seed)
    cases = []
    cases.append(
        {
            "name": "uniform",
            "population": CHOICES_POPULATION,
            "weights": None,
            "cum_weights": None,
            "k": CHOICES_K,
            "result": rng.choices(CHOICES_POPULATION, k=CHOICES_K),
        }
    )
    cases.append(
        {
            "name": "int_weights",
            "population": CHOICES_POPULATION,
            "weights": INT_WEIGHTS,
            "cum_weights": None,
            "k": CHOICES_K,
            "result": rng.choices(CHOICES_POPULATION, weights=INT_WEIGHTS, k=CHOICES_K),
        }
    )
    cases.append(
        {
            "name": "float_weights",
            "population": CHOICES_POPULATION,
            "weights": FLOAT_WEIGHTS,
            "cum_weights": None,
            "k": CHOICES_K,
            "result": rng.choices(CHOICES_POPULATION, weights=FLOAT_WEIGHTS, k=CHOICES_K),
        }
    )
    cum = []
    total = 0.0
    for w in FLOAT_WEIGHTS:
        total += w
        cum.append(total)
    cases.append(
        {
            "name": "cum_weights",
            "population": CHOICES_POPULATION,
            "weights": None,
            "cum_weights": cum,
            "k": CHOICES_K,
            "result": rng.choices(CHOICES_POPULATION, cum_weights=cum, k=CHOICES_K),
        }
    )
    out["choices"] = cases
    return out


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    for seed in SEEDS:
        text = json.dumps(vectors_for(seed), sort_keys=True, indent=1, ensure_ascii=False) + "\n"
        (OUT_DIR / f"seed_{seed}.json").write_bytes(text.encode("utf-8"))


if __name__ == "__main__":
    main()
