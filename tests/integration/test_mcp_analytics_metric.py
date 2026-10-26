"""MCP analytics_query: an unknown metric is a typed validation_failed naming the known metrics."""

from __future__ import annotations

import pytest
from mcp import Client

from workgraph.analytics.metrics import METRICS_BY_ID
from workgraph.analytics.service import AnalyticsQuery
from workgraph.mcp.server import build_server


def test_query_model_rejects_unknown_metric_naming_known_metrics():
    with pytest.raises(ValueError, match="known metrics") as info:
        AnalyticsQuery(metric="no_such_metric")
    message = str(info.value)
    assert "no_such_metric" in message
    assert "tasks_accepted" in message and "cumulative_churn" in message


async def test_mcp_unknown_metric_returns_validation_failed(app, human, ws):
    server = build_server(app, agent_name="tester")
    async with Client(server) as client:
        result = await client.call_tool("analytics_query", {"metric": "no_such_metric"})
    assert result.is_error
    text = str(result.content)
    assert "validation_failed" in text
    assert "UnexpectedToolError" not in text
    assert "no_such_metric" in text
    for known in ("tasks_accepted", "cumulative_churn"):
        assert known in METRICS_BY_ID and known in text
