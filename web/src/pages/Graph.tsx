import { useMemo } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { Background, Controls, MarkerType, MiniMap, Position, ReactFlow, type Edge, type Node } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import type { GraphNode } from "../api/types";
import { Card, Empty, ErrorBanner, Loading, inputBase } from "../components/ui";
import { useGraph, useProjectNames, useRepoNames } from "../hooks/lookups";
import { useWorkspace } from "../hooks/workspace";
import { label } from "../lib/format";
import { TASK_W, buildGroupedGraph, linkedIds, type GroupMode, type TaskGroup } from "../lib/graph";

export { layeredLayout, linkedIds } from "../lib/graph";

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

// darker shades of STATUS_FILL for the status bar on a collapsed group
const STATUS_BAR: Record<string, string> = {
  accepted: "#4ade80",
  failed: "#fb7185",
  blocked: "#fbbf24",
  running: "#a78bfa",
  claimed: "#818cf8",
  ready: "#38bdf8",
  awaiting_review: "#2dd4bf",
  awaiting_verification: "#22d3ee",
  cancelled: "#cbd5e1",
  backlog: "#94a3b8",
};

const GROUP_MODES: { value: GroupMode; label: string }[] = [
  { value: "parent", label: "group: parent / batch, then area" },
  { value: "area", label: "group: area tag" },
  { value: "none", label: "no grouping" },
];

function statusCounts(members: GraphNode[]): [string, number][] {
  const counts = new Map<string, number>();
  for (const m of members) counts.set(m.status, (counts.get(m.status) ?? 0) + 1);
  return [...counts].sort((a, b) => b[1] - a[1]);
}

function GroupHeading({ group }: { group: TaskGroup }) {
  return (
    <div className="flex justify-between gap-2 text-[11px]">
      <span className="mono truncate font-semibold">{group.headKey ?? group.key}</span>
      <span className="shrink-0 text-slate-500">{group.members.length} tasks</span>
    </div>
  );
}

function CollapsedGroup({ group }: { group: TaskGroup }) {
  const counts = statusCounts(group.members);
  const blocked = group.members.filter((m) => m.blocked).length;
  return (
    <div className="text-left">
      <GroupHeading group={group} />
      <div className="line-clamp-2 text-[12px] font-medium leading-tight">{group.title}</div>
      <div className="mt-1 flex h-1.5 overflow-hidden rounded-full" aria-hidden>
        {counts.map(([status, n]) => (
          <span key={status} style={{ flexGrow: n, background: STATUS_BAR[status] ?? "#94a3b8" }} />
        ))}
      </div>
      <div className="mt-0.5 truncate text-[10px] text-slate-500">
        {counts
          .slice(0, 3)
          .map(([status, n]) => `${n} ${label(status)}`)
          .join(" · ")}
        {blocked > 0 && <span className="text-amber-600"> · {blocked} blocked</span>}
      </div>
    </div>
  );
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
  const openOnly = params.get("open") !== "0";
  const linkedOnly = params.get("linked") === "1";
  const showRelations = params.get("relations") !== "0";
  const groupMode = (GROUP_MODES.find((m) => m.value === params.get("group"))?.value ?? "parent") as GroupMode;
  const expandParam = params.get("expand") ?? "";
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
  const toggleGroup = (key: string) => {
    const keys = new Set(expandParam ? expandParam.split(",") : []);
    if (keys.has(key)) keys.delete(key);
    else keys.add(key);
    setParam("expand", [...keys].join(","));
  };
  const graph = useGraph(ws, project || undefined, repository || undefined);

  const { nodes, edges, counts, groupKeys } = useMemo(() => {
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
    const expanded = new Set(expandParam ? expandParam.split(",") : []);
    const grouped = buildGroupedGraph(visible, data.nodes, visibleEdges, groupMode, expanded);
    const blocked = new Set(data.nodes.filter((n) => n.blocked).map((n) => n.id));

    const taskNode = (n: GraphNode, position: { x: number; y: number }, parentId?: string): Node => ({
      id: n.id,
      position,
      parentId,
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
        width: TASK_W,
        background: STATUS_FILL[n.status] ?? "#fff",
        border: n.blocked ? "2px solid #d97706" : "1px solid #cbd5e1",
        borderRadius: 8,
        padding: 6,
      },
    });

    // React Flow wants a box before the tasks inside it
    const rfNodes: Node[] = [];
    for (const u of grouped.units) {
      const position = grouped.positions.get(u.id) ?? { x: 0, y: 0 };
      if (u.kind === "task") {
        rfNodes.push(taskNode(u.task, position));
      } else if (u.kind === "collapsed") {
        rfNodes.push({
          id: u.id,
          position,
          sourcePosition: Position.Right,
          targetPosition: Position.Left,
          data: { label: <CollapsedGroup group={u.group} />, groupKey: u.group.key },
          style: {
            width: TASK_W,
            background: "#fff",
            border: "1px solid #64748b",
            borderRadius: 8,
            padding: 6,
            boxShadow: "3px 3px 0 #cbd5e1, 6px 6px 0 #e2e8f0",
            cursor: "pointer",
          },
        });
      } else {
        rfNodes.push({
          id: u.id,
          position,
          selectable: false,
          data: {
            label: (
              <div className="flex items-start justify-between gap-2 text-left">
                <div className="min-w-0">
                  <GroupHeading group={u.group} />
                  <div className="truncate text-[12px] font-medium">{u.group.title}</div>
                </div>
                <button
                  type="button"
                  className="nodrag shrink-0 rounded border border-slate-300 bg-white px-1.5 text-[11px] hover:bg-slate-100"
                  onClick={(e) => {
                    e.stopPropagation();
                    toggleGroup(u.group.key);
                  }}
                >
                  collapse
                </button>
              </div>
            ),
            groupKey: u.group.key,
          },
          style: {
            width: u.size.w,
            height: u.size.h,
            background: "rgba(241, 245, 249, 0.6)",
            border: "1px dashed #64748b",
            borderRadius: 10,
            padding: "6px 10px",
          },
        });
        for (const m of u.group.members) rfNodes.push(taskNode(m, u.inner.get(m.id) ?? { x: 0, y: 0 }, u.id));
      }
    }
    const rfEdges: Edge[] = grouped.edges.map((e) => {
      const kindLabel = e.kind === "relation" ? label(e.relation_type) : e.kind === "parent" ? "child" : undefined;
      return {
        id: e.id,
        source: e.source,
        target: e.target,
        label: e.count > 1 ? `${e.count} edges` : kindLabel,
        animated: e.kind === "depends_on" && e.count === 1 && blocked.has(e.target),
        style: {
          stroke: e.kind === "depends_on" ? "#475569" : e.kind === "parent" ? "#94a3b8" : "#a855f7",
          strokeDasharray: e.kind === "depends_on" ? undefined : "4 3",
          strokeWidth: e.count > 1 ? Math.min(1 + Math.log2(e.count), 5) : 1,
        },
        markerEnd: { type: MarkerType.ArrowClosed },
        labelStyle: { fontSize: 10 },
      };
    });
    const groupCount = grouped.groups.filter((g) => g.members.length > 1).length;
    return {
      nodes: rfNodes,
      edges: rfEdges,
      counts: { tasks: visible.length, linked: linked.size, edges: visibleEdges.length, groups: groupCount },
      groupKeys: grouped.groups.filter((g) => g.members.length > 1).map((g) => g.key),
    };
  }, [graph.data, openOnly, linkedOnly, showRelations, repoNames, groupMode, expandParam]);

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
          <select
            aria-label="Grouping"
            className={`${inputBase} w-60`}
            value={groupMode}
            onChange={(e) => {
              setParams(
                (prev) => {
                  const next = new URLSearchParams(prev);
                  if (e.target.value === "parent") next.delete("group");
                  else next.set("group", e.target.value);
                  next.delete("expand");
                  return next;
                },
                { replace: true },
              );
            }}
          >
            {GROUP_MODES.map((m) => (
              <option key={m.value} value={m.value}>
                {m.label}
              </option>
            ))}
          </select>
          {groupMode !== "none" && (
            <>
              <button type="button" className="text-xs text-slate-600 underline" onClick={() => setParam("expand", groupKeys.join(","))}>
                expand all
              </button>
              <button type="button" className="text-xs text-slate-600 underline" onClick={() => setParam("expand", null)}>
                collapse all
              </button>
            </>
          )}
          <label className="flex items-center gap-1">
            <input type="checkbox" checked={openOnly} onChange={(e) => setParam("open", e.target.checked ? null : "0")} /> open only
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
        {groupMode !== "none" && <span>stacked card = group, click to expand · thick edge = merged edges</span>}
        <span>unlinked tasks are gridded below the layers</span>
        {graph.data && (
          <span className="ml-auto" data-testid="graph-counts">
            {counts.tasks} tasks · {counts.linked} linked · {counts.edges} edges
            {groupMode !== "none" && ` · ${counts.groups} groups`}
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
              key={`${project}|${repository}|${openOnly}|${linkedOnly}|${showRelations}|${groupMode}`}
              nodes={nodes}
              edges={edges}
              fitView
              minZoom={0.02}
              nodesDraggable
              onNodeClick={(_, node) => {
                const groupKey = (node.data as { groupKey?: string }).groupKey;
                if (groupKey) {
                  // an expanded box collapses through its button, so a stray click on it does nothing
                  if (!expandParam.split(",").includes(groupKey)) toggleGroup(groupKey);
                  return;
                }
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
