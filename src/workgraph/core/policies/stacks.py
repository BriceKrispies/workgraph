"""Stack item state machine.

Items track a child task's place in its parent's stack. They are deliberately not task
lifecycle states: a child stays ``awaiting_verification`` while its item moves through the stack.
"""

from __future__ import annotations

from workgraph.core.errors import InvalidTransitionError
from workgraph.core.models.enums import StackItemState

S = StackItemState

STACK_ITEM_TRANSITIONS: dict[StackItemState, frozenset[StackItemState]] = {
    # A child is pending until its attempt passes the fast tier.
    S.PENDING: frozenset({S.READY}),
    # A ready item is merged onto the integration branch, or conflicts with it. A new attempt sends it back.
    S.READY: frozenset({S.MERGED, S.CONFLICT, S.PENDING}),
    # A merged item leaves the integration branch when it is ejected, or when a culprit it depends on is.
    S.MERGED: frozenset({S.VERIFIED, S.EJECTED, S.READY, S.CONFLICT, S.PENDING}),
    # A conflicting or ejected child goes back to its agent; once fixed it is ready again.
    S.CONFLICT: frozenset({S.READY, S.PENDING, S.EJECTED}),
    S.EJECTED: frozenset({S.PENDING, S.READY}),
    # Verified items are replayed when the integration branch is rebuilt on a moved main, and leave the
    # branch again when a later full verification of the tip fails on them.
    S.VERIFIED: frozenset({S.READY, S.EJECTED, S.PENDING, S.CONFLICT}),
}

# Items that are no longer on the integration branch and may therefore leave the stack.
REMOVABLE_STATES = frozenset({S.PENDING, S.READY, S.CONFLICT, S.EJECTED})


def check_stack_item_transition(current: StackItemState, target: StackItemState) -> None:
    if current == target:
        return
    if target not in STACK_ITEM_TRANSITIONS[current]:
        raise InvalidTransitionError(
            f"cannot move stack item from {current} to {target}",
            details={"from": current, "to": target, "allowed": sorted(STACK_ITEM_TRANSITIONS[current])},
        )
