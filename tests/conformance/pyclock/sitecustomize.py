"""Precise wall clock for recording Python goldens on Windows.

CPython 3.12 reads `datetime.now()` from the coarse system clock, which ticks every 15.6 ms
and can lag real time by that much. The conformance runner attributes each timestamp to the
step whose wall-clock window holds it, so a lagging stamp lands in the previous step's window
and the golden numbers it `<ts@k-1>` instead of `<ts@k>`. This module is picked up through
`--env PYTHONPATH=tests/conformance/pyclock` when recording; it anchors the clock once, at a
tick edge, and then advances it with the monotonic high-resolution counter.
"""

import datetime as _dt
import time as _time


def _anchor() -> tuple[int, int]:
    first = _time.time_ns()
    while True:
        wall = _time.time_ns()
        if wall != first:
            return wall, _time.perf_counter_ns()


_WALL_NS, _PERF_NS = _anchor()


class _Datetime(_dt.datetime):
    @classmethod
    def now(cls, tz=None):
        ns = _WALL_NS + (_time.perf_counter_ns() - _PERF_NS)
        return _build(cls, ns, tz)

    @classmethod
    def utcnow(cls):
        return cls.now(_dt.UTC).replace(tzinfo=None)


def _build(cls, ns: int, tz):
    seconds, nanos = divmod(ns, 1_000_000_000)
    base = _dt.datetime(1970, 1, 1, tzinfo=_dt.UTC) + _dt.timedelta(seconds=seconds, microseconds=nanos // 1000)
    if tz is None:
        base = base.astimezone().replace(tzinfo=None)
    else:
        base = base.astimezone(tz)
    return cls(
        base.year, base.month, base.day, base.hour, base.minute, base.second, base.microsecond, base.tzinfo
    )


_dt.datetime = _Datetime
