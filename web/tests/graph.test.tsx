import { cleanup, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { GraphEdge, GraphNode } from "../src/api/types";
import { GraphPage, layeredLayout } from "../src/pages/Graph";
import { mockApi, renderPage } from "./utils";

beforeEach(() => {
  // React Flow measures its container; jsdom has no ResizeObserver.
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function node(id: string): GraphNode {
  return {
    id,
    key: `WG-${id}`,
    title: `Task ${id}`,
    status: "backlog",
    priority: "medium",
    category: null,
    parent_id: null,
    project_ids: [],
    repository_ids: [],
    blocked: false,
  };
}

function dep(source: string, target: string): GraphEdge {
  return { source, target, kind: "depends_on", relation_type: null };
}

describe("layeredLayout", () => {
  it("puts linked nodes in dependency columns and grids unlinked nodes below them", () => {
    const nodes = ["a", "b", "c", ...Array.from({ length: 40 }, (_, i) => `u${i}`)].map(node);
    const pos = layeredLayout(nodes, [dep("a", "b"), dep("b", "c")]);
    expect(pos.get("a")).toEqual({ x: 0, y: 0 });
    expect(pos.get("b")).toEqual({ x: 280, y: 0 });
    expect(pos.get("c")).toEqual({ x: 560, y: 0 });
    const unlinked = Array.from({ length: 40 }, (_, i) => pos.get(`u${i}`)!);
    // below the single row of layers, spread over several columns rather than stacked in one
    expect(Math.min(...unlinked.map((p) => p.y))).toBe(192);
    expect(new Set(unlinked.map((p) => p.x)).size).toBeGreaterThan(3);
    expect(Math.max(...unlinked.map((p) => p.y))).toBeLessThan(40 * 96);
  });

  it("does not layer nodes joined only by a relation", () => {
    const pos = layeredLayout([node("a"), node("b")], [{ source: "a", target: "b", kind: "relation", relation_type: "related" }]);
    // both go to the grid (one column for two nodes), neither is placed a layer to the right
    expect(pos.get("a")).toEqual({ x: 0, y: 0 });
    expect(pos.get("b")).toEqual({ x: 0, y: 96 });
  });
});

describe("GraphPage", () => {
  it("reads filters from the URL, passes the repository to the API and hides unlinked tasks", async () => {
    const calls = mockApi({
      "GET /ws/main/graph": { nodes: ["a", "b", "u"].map(node), edges: [dep("a", "b")] },
      "GET /ws/main/repositories": [{ id: "r1", name: "rho" }],
      "GET /ws/main/projects": [],
    });
    renderPage(<GraphPage />, { path: "/graph", route: "/graph?repo=rho&linked=1" });
    await waitFor(() => expect(screen.getByTestId("graph-counts")).toHaveTextContent("2 tasks · 2 linked · 1 edges"));
    const graphCall = calls.find((c) => c.path === "/ws/main/graph")!;
    expect(graphCall.search.get("repository")).toBe("rho");
    expect(screen.getByRole("checkbox", { name: /linked only/ })).toBeChecked();
  });
});
