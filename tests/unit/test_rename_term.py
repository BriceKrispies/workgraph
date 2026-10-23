from __future__ import annotations

import pytest

from workgraph.core.errors import ValidationFailed
from workgraph.core.services.rename import Term


def renamed(text: str, *keep: str) -> str:
    return Term("Rho", "Nyn", keep).apply(text)[0]


def test_every_spelling_of_the_word_moves():
    assert renamed("rho Rho RHO") == "nyn Nyn NYN"
    assert renamed("crates/rho-kernel/src/a.rho") == "crates/nyn-kernel/src/a.nyn"
    assert (
        renamed("rho_kernel RhoKernel RHO_VULKAN_TIER rho.exe")
        == "nyn_kernel NynKernel NYN_VULKAN_TIER nyn.exe"
    )


def test_a_word_that_only_contains_it_is_left_alone():
    assert (
        renamed("neighbourhood rhombus Rhodes RHOCAP01 librho")
        == "neighbourhood rhombus Rhodes RHOCAP01 librho"
    )


def test_newline_and_tab_escapes_are_boundaries():
    assert renamed(r'"\nrho" "\trho" "xnrho"') == r'"\nnyn" "\tnyn" "xnrho"'


def test_kept_tokens_survive_beside_renamed_ones():
    assert renamed("`RHO_CORP` and RHO_TIER read rho.rnd1 beside a.rho", "`RHO_CORP`", "rho.rnd1") == (
        "`RHO_CORP` and NYN_TIER read rho.rnd1 beside a.nyn"
    )


def test_the_count_is_the_number_of_sites():
    assert Term("Rho", "Nyn").apply("rho, Rho and rhombus")[1] == 2


def test_json_strings_and_keys_are_renamed_and_nothing_else():
    value = {"command": ["bash", "scripts/rho.sh"], "env": {"RHO_TIER": "device"}, "timeout": 9, "ok": True}
    out, sites = Term("Rho", "Nyn").deep(value)
    assert out == {
        "command": ["bash", "scripts/nyn.sh"],
        "env": {"NYN_TIER": "device"},
        "timeout": 9,
        "ok": True,
    }
    assert sites == 2


@pytest.mark.parametrize(("old", "new"), [("rho-lang", "nyn"), ("Rho", "n1n"), ("Rho", "rho"), ("", "Nyn")])
def test_a_name_must_be_one_new_ascii_word(old, new):
    with pytest.raises(ValidationFailed):
        Term(old, new)
