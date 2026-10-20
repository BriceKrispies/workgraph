import { cleanup, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { GraphEdge, GraphNode } from "../src/api/types";
import { buildGroupedGraph, groupNodeId } from "../src/lib/graph";
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

function node(id: string, extra: Partial<GraphNode> = {}): GraphNode {
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
    tags: [],
    blocked: false,
    ...extra,
  };
}

function child(source: string, target: string): GraphEdge {
  return { source, target, kind: "parent", relation_type: null };
}

function dep(source: string, target: string): GraphEdge {
  return { source, target, kind: "depends_on", relation_type: null };
}

describe("layeredLayout", () => {
  it("puts linked nodes in dependency columns and grids unlinked nodes below them", () => {
    const nodes = ["a", "b", "c", ...Array.from({ length: 40 }, (_, i) => `u${i}`)].map((id) => node(id));
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

describe("buildGroupedGraph", () => {
  // batch b with children b1, b2 (b2 has its own child b21); a1, a2 tagged area:eng; loose u; x depends on b1
  const nodes = [
    node("b", { title: "rho implementation batch 1", status: "accepted" }),
    node("b1", { parent_id: "b", tags: ["area:eng"] }),
    node("b2", { parent_id: "b" }),
    node("b21", { parent_id: "b2" }),
    node("a1", { tags: ["area:eng"] }),
    node("a2", { tags: ["area:eng", "rho"] }),
    node("u"),
    node("x", { tags: ["area:docs"] }),
  ];
  const edges = [child("b", "b1"), child("b", "b2"), child("b2", "b21"), dep("b1", "x"), dep("a1", "x"), dep("a2", "x"), dep("a1", "a2")];
  const keyOf = (units: { kind: string; id: string }[]) => units.map((u) => `${u.kind}:${u.id}`).sort();

  it("groups by top-level parent, falls back to the area tag and keeps lone tasks as tasks", () => {
    const g = buildGroupedGraph(nodes, nodes, edges, "parent", new Set());
    expect(keyOf(g.units)).toEqual(["collapsed:group:WG-b", "collapsed:group:area:eng", "task:u", "task:x"]);
    const batch = g.groups.find((x) => x.key === "WG-b")!;
    expect(batch.title).toBe("rho implementation batch 1");
    expect(batch.members.map((m) => m.id).sort()).toEqual(["b", "b1", "b2", "b21"]);
  });

  it("keeps a hidden batch parent's children together", () => {
    const open = nodes.filter((n) => n.status !== "accepted");
    const g = buildGroupedGraph(open, nodes, edges.filter((e) => e.source !== "b"), "parent", new Set());
    expect(g.groups.find((x) => x.key === "WG-b")!.members.map((m) => m.id).sort()).toEqual(["b1", "b2", "b21"]);
  });

  it("drops edges inside a collapsed group and merges edges between the same two nodes", () => {
    const g = buildGroupedGraph(nodes, nodes, edges, "parent", new Set());
    const summary = g.edges.map((e) => `${e.source}->${e.target}:${e.kind}:${e.count}`).sort();
    expect(summary).toEqual(["group:WG-b->x:depends_on:1", "group:area:eng->x:depends_on:2"]);
  });

  it("lays out an expanded group's tasks inside its box, without the parent edges the box implies", () => {
    const g = buildGroupedGraph(nodes, nodes, edges, "parent", new Set(["WG-b"]));
    const box = g.units.find((u) => u.id === groupNodeId("WG-b"))!;
    expect(box.kind).toBe("expanded");
    if (box.kind !== "expanded") return;
    expect([...box.inner.keys()].sort()).toEqual(["b", "b1", "b2", "b21"]);
    for (const p of box.inner.values()) {
      expect(p.x).toBeGreaterThan(0);
      expect(p.y).toBeGreaterThan(0);
      expect(p.x).toBeLessThan(box.size.w);
      expect(p.y).toBeLessThan(box.size.h);
    }
    expect(g.edges.some((e) => e.kind === "parent")).toBe(false);
    expect(g.edges.map((e) => `${e.source}->${e.target}`)).toContain("b1->x");
  });

  it("groups by area tag alone in area mode, keeping parent edges", () => {
    const g = buildGroupedGraph(nodes, nodes, edges, "area", new Set(["unsorted"]));
    expect(g.groups.map((x) => x.key).sort()).toEqual(["area:docs", "area:eng", "unsorted"]);
    expect(g.groups.find((x) => x.key === "unsorted")!.title).toBe("no area tag");
    expect(g.edges.filter((e) => e.kind === "parent").map((e) => `${e.source}->${e.target}`).sort()).toEqual(["b->b2", "b->group:area:eng", "b2->b21"]);
  });

  it("places expanded boxes so they do not overlap other units", () => {
    const g = buildGroupedGraph(nodes, nodes, edges, "parent", new Set(["WG-b", "area:eng"]));
    const rects = g.units.map((u) => {
      const p = g.positions.get(u.id)!;
      const s = u.kind === "expanded" ? u.size : { w: 220, h: 80 };
      return { ...p, ...s };
    });
    for (let i = 0; i < rects.length; i++)
      for (let j = i + 1; j < rects.length; j++) {
        const [a, b] = [rects[i], rects[j]];
        const overlap = a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
        expect(overlap).toBe(false);
      }
  });
});

describe("GraphPage", () => {
  it("reads filters from the URL, passes the repository to the API and hides unlinked tasks", async () => {
    const calls = mockApi({
      "GET /ws/main/graph": { nodes: ["a", "b", "u"].map((id) => node(id)), edges: [dep("a", "b")] },
      "GET /ws/main/repositories": [{ id: "r1", name: "rho" }],
      "GET /ws/main/projects": [],
    });
    renderPage(<GraphPage />, { path: "/graph", route: "/graph?repo=rho&linked=1" });
    await waitFor(() => expect(screen.getByTestId("graph-counts")).toHaveTextContent("2 tasks · 2 linked · 1 edges"));
    const graphCall = calls.find((c) => c.path === "/ws/main/graph")!;
    expect(graphCall.search.get("repository")).toBe("rho");
    expect(screen.getByRole("checkbox", { name: /linked only/i })).toBeChecked();
  });

  it("hides closed tasks by default and counts groups", async () => {
    mockApi({
      "GET /ws/main/graph": {
        nodes: [node("p"), node("c1", { parent_id: "p" }), node("c2", { parent_id: "p", status: "accepted" }), node("u")],
        edges: [child("p", "c1"), child("p", "c2")],
      },
      "GET /ws/main/repositories": [],
      "GET /ws/main/projects": [],
    });
    renderPage(<GraphPage />, { path: "/graph", route: "/graph" });
    await waitFor(() => expect(screen.getByTestId("graph-counts")).toHaveTextContent("3 tasks · 2 linked · 1 edges · 1 groups"));
    expect(screen.getByRole("checkbox", { name: /open only/i })).toBeChecked();
    expect(screen.getByRole("combobox", { name: "Grouping" })).toHaveValue("parent");
  });
});
