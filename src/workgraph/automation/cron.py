"""Minimal 5-field cron expressions (minute hour day-of-month month day-of-week), UTC."""

from __future__ import annotations

from datetime import datetime, timedelta

from workgraph.core.errors import ValidationFailed

_RANGES = [(0, 59), (0, 23), (1, 31), (1, 12), (0, 6)]


def _parse_field(spec: str, low: int, high: int) -> set[int]:
    values: set[int] = set()
    for part in spec.split(","):
        step = 1
        if "/" in part:
            part, step_text = part.split("/", 1)
            if not step_text.isdigit() or int(step_text) <= 0:
                raise ValidationFailed(f"invalid cron step {step_text!r}")
            step = int(step_text)
        if part == "*":
            start, end = low, high
        elif "-" in part:
            a, b = part.split("-", 1)
            if not (a.isdigit() and b.isdigit()):
                raise ValidationFailed(f"invalid cron range {part!r}")
            start, end = int(a), int(b)
        elif part.isdigit():
            start = end = int(part)
        else:
            raise ValidationFailed(f"invalid cron field {part!r}")
        if start < low or end > high or start > end:
            raise ValidationFailed(f"cron value {part!r} outside {low}-{high}")
        values.update(range(start, end + 1, step))
    return values


class Cron:
    def __init__(self, expression: str) -> None:
        fields = expression.split()
        if len(fields) != 5:
            raise ValidationFailed("cron expressions need 5 fields: minute hour day month weekday")
        self.expression = expression
        self.minutes, self.hours, self.days, self.months, self.weekdays = (
            _parse_field(f, lo, hi) for f, (lo, hi) in zip(fields, _RANGES, strict=True)
        )
        self._dom_any = fields[2] == "*"
        self._dow_any = fields[4] == "*"

    def matches(self, moment: datetime) -> bool:
        weekday = (moment.weekday() + 1) % 7  # cron: Sunday = 0
        if self._dom_any or self._dow_any:
            day_ok = moment.day in self.days and weekday in self.weekdays
        else:  # classic cron: either day-of-month or day-of-week may match
            day_ok = moment.day in self.days or weekday in self.weekdays
        return (
            moment.minute in self.minutes
            and moment.hour in self.hours
            and moment.month in self.months
            and day_ok
        )

    def next_after(self, moment: datetime) -> datetime:
        candidate = moment.replace(second=0, microsecond=0) + timedelta(minutes=1)
        for _ in range(366 * 24 * 60):
            if self.matches(candidate):
                return candidate
            candidate += timedelta(minutes=1)
        raise ValidationFailed(f"cron expression {self.expression!r} never fires")
