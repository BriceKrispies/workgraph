"""Declarative base and shared column helpers."""

from __future__ import annotations

from datetime import datetime
from typing import Any

from sqlalchemy import MetaData
from sqlalchemy.orm import DeclarativeBase, Mapped, mapped_column

from workgraph.core.clock import new_id, utcnow
from workgraph.persistence.types import JSONText, UTCDateTime

NAMING_CONVENTION = {
    "ix": "ix_%(column_0_label)s",
    "uq": "uq_%(table_name)s_%(column_0_name)s",
    "ck": "ck_%(table_name)s_%(constraint_name)s",
    "fk": "fk_%(table_name)s_%(column_0_name)s_%(referred_table_name)s",
    "pk": "pk_%(table_name)s",
}


class Base(DeclarativeBase):
    metadata = MetaData(naming_convention=NAMING_CONVENTION)
    type_annotation_map = {
        datetime: UTCDateTime(),
        dict[str, Any]: JSONText(),
        list[Any]: JSONText(),
        list[str]: JSONText(),
        list[dict[str, Any]]: JSONText(),
    }


def pk() -> Mapped[str]:
    return mapped_column(primary_key=True, default=new_id)


def created_col() -> Mapped[datetime]:
    return mapped_column(default=utcnow, nullable=False)


def updated_col() -> Mapped[datetime]:
    return mapped_column(default=utcnow, onupdate=utcnow, nullable=False)
