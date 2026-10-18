import { useMemo } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { Background, Controls, MarkerType, MiniMap, Position, ReactFlow, type Edge, type Node } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import type { GraphEdge, GraphNode } from "../api/types";
import { Card, Empty, ErrorBanner, Loading, inputBase } from "../components/ui";
import { useGraph, useProjectNames, useRepoNames } from "../hooks/lookups";
import { useWorkspace } from "../hooks/workspace";
import { label } from "../lib/format";

const STATUS_FILL: Record<string, string> = {
  accepted: "#dcfce7",
  failed: "#ffe4e6",
  blocked: "#fef3c7",
  running: "#ede9fe",
  claimed: "#e0e7ff",
  ready: "#e0f2fe",
  awaiting_review: "#ccfbf1",
  awaiting_verification: "#cffafe",
  cancelled: "#f1f5f9",
  backlog: "#f8fafc",
};

const COL_W = 280;
const ROW_H = 96;

/** Nodes touched by at least one of the given edges. */
export function linkedIds(edges: GraphEdge[]): Set<string> {
  return new Set(edges.flatMap((e) => [e.source, e.target]));
}

/**
 * Layered layout: a node's column is the longest path from a root over dependency and parent edges.
 * Nodes with no dependency or parent edge are packed into a grid below the layers, so a mostly
 * unlinked backlog does not become one column thousands of rows tall.
 */
export function layeredLayout(nodes: GraphNode[], edges: GraphEdge[]): Map<string, { x: number; y: number }> {
  const incoming = new Map<string, string[]>();
  for (const n of nodes) incoming.set(n.id, []);
  const structural = edges.filter((e) => e.kind !== "relation" && incoming.has(e.source) && incoming.has(e.target));
  for (const e of structural) incoming.get(e.target)!.push(e.source);
  const linked = linkedIds(structural);
  const level = new Map<string, number>();
  const visiting = new Set<string>();
  const depth = (id: string): number => {
    if (level.has(id)) return level.get(id)!;
    if (visiting.has(id)) return 0; // defensive: cycles are rejected server-side
    visiting.add(id);
    const preds = incoming.get(id) ?? [];
    const d = preds.length ? Math.max(...preds.map(depth)) + 1 : 0;
    visiting.delete(id);
    level.set(id, d);
    return d;
  };
  const columns = new Map<number, string[]>();
  const isolated: string[] = [];
  for (const n of nodes) {
    if (!linked.has(n.id)) {
      isolated.push(n.id);
      continue;
    }
    const d = depth(n.id);
    columns.set(d, [...(columns.get(d) ?? []), n.id]);
  }
  const positions = new Map<string, { x: number; y: number }>();
  for (const [d, ids] of columns) ids.forEach((id, i) => positions.set(id, { x: d * COL_W, y: i * ROW_H }));
  if (isolated.length) {
    const layerRows = Math.max(0, ...[...columns.values()].map((ids) => ids.length));
    const top = layerRows ? (layerRows + 1) * ROW_H : 0;
    // roughly square in screen space, and never narrower than the layers above it
    const gridCols = Math.max(columns.size, Math.ceil(Math.sqrt((isolated.length * ROW_H) / COL_W)));
    isolated.forEach((id, i) => positions.set(id, { x: (i % gridCols) * COL_W, y: top + Math.floor(i / gridCols) * ROW_H }));
  }
  return positions;
}

export function GraphPage() {
  const { ws } = useWorkspace();
  const navigate = useNavigate();
  const projects = useProjectNames(ws);
  const repoNames = useRepoNames(ws);
  // Filters live in the URL so a view can be linked to and captured by `workgraph ui screenshot`.
  const [params, setParams] = useSearchParams();
  const project = params.get("project") ?? "";
  const repository = params.get("repo") ?? "";
  const openOnly = params.get("open") === "1";
  const linkedOnly = params.get("linked") === "1";
  const showRelations = params.get("relations") !== "0";
  const setParam = (key: string, value: string | null) =>
    setParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        if (value) next.set(key, value);
        else next.delete(key);
        return next;
      },
      { replace: true },
    );
  const graph = useGraph(ws, project || undefined, repository || undefined);

  const { nodes, edges, counts } = useMemo(() => {
    const data = graph.data ?? { nodes: [], edges: [] };
    let visible = data.nodes.filter((n) => !openOnly || !["accepted", "cancelled"].includes(n.status));
    let ids = new Set(visible.map((n) => n.id));
    let visibleEdges = data.edges.filter((e) => ids.has(e.source) && ids.has(e.target) && (showRelations || e.kind !== "relation"));
    const linked = linkedIds(visibleEdges);
    if (linkedOnly) {
      visible = visible.filter((n) => linked.has(n.id));
      ids = new Set(visible.map((n) => n.id));
      visibleEdges = visibleEdges.filter((e) => ids.has(e.source) && ids.has(e.target));
    }
    const positions = layeredLayout(visible, visibleEdges);
    const rfNodes: Node[] = visible.map((n) => ({
      id: n.id,
      position: positions.get(n.id) ?? { x: 0, y: 0 },
      sourcePosition: Position.Right,
      targetPosition: Position.Left,
      data: {
        label: (
          <div className="text-left">
            <div className="flex justify-between gap-2 text-[11px]">
              <span className="mono font-semibold">{n.key}</span>
              <span className="text-slate-500">{label(n.status)}</span>
            </div>
            <div className="line-clamp-2 text-[12px] leading-tight">{n.title}</div>
            {n.repository_ids.length > 0 && (
              <div className="mt-0.5 truncate text-[10px] text-slate-500">
                {n.repository_ids.map((r) => repoNames.get(r) ?? r.slice(0, 6)).join(" · ")}
              </div>
            )}
          </div>
        ),
      },
      style: {
        width: 220,
        background: STATUS_FILL[n.status] ?? "#fff",
        border: n.blocked ? "2px solid #d97706" : "1px solid #cbd5e1",
        borderRadius: 8,
        padding: 6,
      },
    }));
    const rfEdges: Edge[] = visibleEdges.map((e, i) => ({
      id: `${e.kind}-${e.source}-${e.target}-${i}`,
      source: e.source,
      target: e.target,
      label: e.kind === "relation" ? label(e.relation_type) : e.kind === "parent" ? "child" : undefined,
      animated: e.kind === "depends_on" && data.nodes.find((n) => n.id === e.target)?.blocked === true,
      style: {
        stroke: e.kind === "depends_on" ? "#475569" : e.kind === "parent" ? "#94a3b8" : "#a855f7",
        strokeDasharray: e.kind === "depends_on" ? undefined : "4 3",
      },
      markerEnd: { type: MarkerType.ArrowClosed },
      labelStyle: { fontSize: 10 },
    }));
    return { nodes: rfNodes, edges: rfEdges, counts: { tasks: visible.length, linked: linked.size, edges: visibleEdges.length } };
  }, [graph.data, openOnly, linkedOnly, showRelations, repoNames]);

  return (
    <div className="flex h-full flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h1 className="text-xl font-semibold text-slate-900">Task graph</h1>
        <div className="flex flex-wrap items-center gap-3 text-sm">
          <select aria-label="Project filter" className={`${inputBase} w-44`} value={project} onChange={(e) => setParam("project", e.target.value)}>
            <option value="">all projects</option>
            {[...projects.values()].map((p) => (
              <option key={p.id} value={p.slug}>
                {p.name}
              </option>
            ))}
          </select>
          <select aria-label="Repository filter" className={`${inputBase} w-44`} value={repository} onChange={(e) => setParam("repo", e.target.value)}>
            <option value="">all repositories</option>
            {[...new Set(repoNames.values())].sort().map((name) => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </select>
          <label className="flex items-center gap-1">
            <input type="checkbox" checked={openOnly} onChange={(e) => setParam("open", e.target.checked ? "1" : null)} /> open only
          </label>
          <label className="flex items-center gap-1">
            <input type="checkbox" checked={linkedOnly} onChange={(e) => setParam("linked", e.target.checked ? "1" : null)} /> linked only
          </label>
          <label className="flex items-center gap-1">
            <input type="checkbox" checked={showRelations} onChange={(e) => setParam("relations", e.target.checked ? null : "0")} /> relations
          </label>
        </div>
      </div>
      <div className="flex flex-wrap gap-3 text-[11px] text-slate-500">
        <span>— dependency (arrow points to the dependent task)</span>
        <span>- - parent → child</span>
        <span className="text-purple-600">- - relation</span>
        <span className="text-amber-600">orange border = blocked</span>
        <span>unlinked tasks are gridded below the layers</span>
        {graph.data && (
          <span className="ml-auto" data-testid="graph-counts">
            {counts.tasks} tasks · {counts.linked} linked · {counts.edges} edges
          </span>
        )}
      </div>
      <ErrorBanner error={graph.error} />
      <Card className="min-h-[520px] flex-1" bodyClass="h-full min-h-[520px] p-0">
        {graph.isLoading ? (
          <Loading />
        ) : nodes.length === 0 ? (
          <div className="p-4">
            <Empty title="No tasks to show" />
          </div>
        ) : (
          <div className="h-[calc(100vh-220px)] min-h-[520px]" data-testid="task-graph">
            <ReactFlow
              // remount on filter changes so the view is fitted to what is now shown
              key={`${project}|${repository}|${openOnly}|${linkedOnly}|${showRelations}`}
              nodes={nodes}
              edges={edges}
              fitView
              minZoom={0.02}
              nodesDraggable
              onNodeClick={(_, node) => {
                const n = graph.data?.nodes.find((g) => g.id === node.id);
                if (n) navigate(`/tasks/${n.key}`);
              }}
            >
              <Background gap={16} color="#e2e8f0" />
              <Controls />
              <MiniMap pannable zoomable nodeColor={(node) => String((node.style as { background?: string } | undefined)?.background ?? "#cbd5e1")} />
            </ReactFlow>
          </div>
        )}
      </Card>
    </div>
  );
}
