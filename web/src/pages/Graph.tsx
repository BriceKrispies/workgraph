import { useMemo, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { Background, Controls, MarkerType, MiniMap, Position, ReactFlow, type Edge, type Node } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import type { GraphNode } from "../api/types";
import { Button, Checkbox, Empty, ErrorBanner, Loading, PageHeader, selectClass } from "../components/ui";
import { useDesktop } from "../hooks/media";
import { TONE_HEX, toneOf, type Tone } from "../lib/tone";
import { useGraph, useProjectNames, useRepoNames } from "../hooks/lookups";
import { useWorkspace } from "../hooks/workspace";
import { label } from "../lib/format";
import { TASK_W, buildGroupedGraph, linkedIds, type GroupMode, type TaskGroup } from "../lib/graph";

export { layeredLayout, linkedIds } from "../lib/graph";

// node fill: the status tone washed into the surface, so it works in both themes
const fill = (status: string) => `color-mix(in oklab, var(--tone-${toneOf(status)}) 16%, var(--wg-surface))`;

const GROUP_MODES: { value: GroupMode; label: string }[] = [
  { value: "parent", label: "Group by parent or batch, then area" },
  { value: "area", label: "Group by area tag" },
  { value: "none", label: "No grouping" },
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
      <span className="shrink-0 text-ink-3">{group.members.length} tasks</span>
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
          <span key={status} className={`tone-${toneOf(status)}`} style={{ flexGrow: n, background: "var(--t)" }} />
        ))}
      </div>
      <div className="mt-0.5 truncate text-[10px] text-ink-3">
        {counts
          .slice(0, 3)
          .map(([status, n]) => `${n} ${label(status)}`)
          .join(", ")}
        {blocked > 0 && <span className="text-warn">, {blocked} blocked</span>}
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
  const desktop = useDesktop();
  const [optionsOpen, setOptionsOpen] = useState(false);

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
        tone: toneOf(n.status),
        label: (
          <div className="text-left">
            <div className="flex justify-between gap-2 text-[11px]">
              <span className="mono font-semibold">{n.key}</span>
              <span className="text-ink-3">{label(n.status)}</span>
            </div>
            <div className="line-clamp-2 text-[12px] leading-tight">{n.title}</div>
            {n.repository_ids.length > 0 && (
              <div className="mt-0.5 truncate text-[10px] text-ink-3">
                {n.repository_ids.map((r) => repoNames.get(r) ?? r.slice(0, 6)).join(", ")}
              </div>
            )}
          </div>
        ),
      },
      style: {
        width: TASK_W,
        background: fill(n.status),
        color: "var(--wg-ink)",
        border: n.blocked ? "2px solid var(--tone-blocked)" : "1px solid var(--wg-line-strong)",
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
            background: "var(--wg-surface)",
            color: "var(--wg-ink)",
            border: "1px solid var(--wg-ink-3)",
            borderRadius: 8,
            padding: 6,
            boxShadow: "3px 3px 0 var(--wg-line-strong), 6px 6px 0 var(--wg-line)",
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
                  className="nodrag shrink-0 rounded-md border border-line-strong bg-surface px-2 py-0.5 text-[11px] text-ink hover:bg-sunken"
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
            background: "color-mix(in oklab, var(--wg-sunken) 70%, transparent)",
            color: "var(--wg-ink)",
            border: "1px dashed var(--wg-ink-3)",
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
          stroke: e.kind === "depends_on" ? "var(--wg-ink-2)" : e.kind === "parent" ? "var(--wg-ink-3)" : "var(--tone-active)",
          strokeDasharray: e.kind === "depends_on" ? undefined : "4 3",
          strokeWidth: e.count > 1 ? Math.min(1 + Math.log2(e.count), 5) : 1,
        },
        markerEnd: { type: MarkerType.ArrowClosed },
        labelStyle: { fontSize: 10, fill: "var(--wg-ink-2)" },
        labelBgStyle: { fill: "var(--wg-surface)" },
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

  const optionCount = [project, repository, linkedOnly, !openOnly, !showRelations, groupMode !== "parent"].filter(Boolean).length;
  return (
    <div className="flex flex-col">
      <PageHeader
        title="Task graph"
        actions={
          <Button
            icon="filter"
            aria-expanded={optionsOpen}
            onClick={() => setOptionsOpen((v) => !v)}
            className={`lg:hidden ${optionCount ? "border-accent text-accent" : ""}`}
          >
            {optionCount ? `View ${optionCount}` : "View"}
          </Button>
        }
      />
      <div className={`${optionsOpen ? "flex" : "hidden"} mb-3 flex-col gap-3 rounded-2xl border border-line bg-surface p-4 lg:flex lg:border-0 lg:bg-transparent lg:p-0`}>
        <div className="grid gap-2 sm:grid-cols-3 lg:flex lg:flex-wrap lg:items-center">
          <select aria-label="Project filter" className={`${selectClass} w-full lg:w-48`} value={project} onChange={(e) => setParam("project", e.target.value)}>
            <option value="">All projects</option>
            {[...projects.values()].map((p) => (
              <option key={p.id} value={p.slug}>
                {p.name}
              </option>
            ))}
          </select>
          <select aria-label="Repository filter" className={`${selectClass} w-full lg:w-48`} value={repository} onChange={(e) => setParam("repo", e.target.value)}>
            <option value="">All repositories</option>
            {[...new Set(repoNames.values())].sort().map((name) => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </select>
          <select
            aria-label="Grouping"
            className={`${selectClass} w-full lg:w-72`}
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
        </div>
        <div className="flex flex-wrap items-center gap-x-5 gap-y-0">
          <Checkbox label="Open only" checked={openOnly} onChange={(v) => setParam("open", v ? null : "0")} />
          <Checkbox label="Linked only" checked={linkedOnly} onChange={(v) => setParam("linked", v ? "1" : null)} />
          <Checkbox label="Relations" checked={showRelations} onChange={(v) => setParam("relations", v ? null : "0")} />
          {groupMode !== "none" && (
            <div className="flex gap-1">
              <Button size="sm" variant="ghost" onClick={() => setParam("expand", groupKeys.join(","))}>
                Expand all
              </Button>
              <Button size="sm" variant="ghost" onClick={() => setParam("expand", null)}>
                Collapse all
              </Button>
            </div>
          )}
        </div>
        <ul className="flex flex-wrap gap-x-4 gap-y-1 text-[12px] text-ink-3">
          <li>Solid arrow: dependency, pointing at the dependent task</li>
          <li>Dashed: parent to child</li>
          <li style={{ color: "var(--tone-active-ink)" }}>Dashed purple: relation</li>
          <li className="text-warn">Orange border: blocked</li>
          {groupMode !== "none" && <li>Stacked card: a group, tap to expand. Thick edge: merged edges</li>}
          <li>Unlinked tasks sit in a grid below the layers</li>
        </ul>
      </div>
      {graph.data && (
        <div className="mb-2 px-1 text-[13px] text-ink-3 tnum" data-testid="graph-counts">
          {counts.tasks} tasks · {counts.linked} linked · {counts.edges} edges
          {groupMode !== "none" && ` · ${counts.groups} groups`}
        </div>
      )}
      <ErrorBanner error={graph.error} />
      <div className="-mx-4 overflow-hidden border-y border-line bg-surface lg:mx-0 lg:rounded-2xl lg:border">
        {graph.isLoading ? (
          <Loading />
        ) : nodes.length === 0 ? (
          <Empty title="No tasks to show">Loosen the view options to see more.</Empty>
        ) : (
          <div className="h-[calc(100dvh-var(--tabbar-h)-13rem)] min-h-[420px] lg:h-[calc(100dvh-16rem)] lg:min-h-[520px]" data-testid="task-graph">
            <ReactFlow
              // remount on filter changes so the view is fitted to what is now shown
              key={`${project}|${repository}|${openOnly}|${linkedOnly}|${showRelations}|${groupMode}`}
              nodes={nodes}
              edges={edges}
              fitView
              minZoom={0.02}
              nodesDraggable={desktop}
              colorMode="system"
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
              <Background gap={16} color="var(--wg-line)" />
              <Controls showInteractive={false} />
              {desktop && (
                <MiniMap
                  pannable
                  zoomable
                  bgColor="var(--wg-sunken)"
                  maskColor="color-mix(in oklab, var(--wg-canvas) 60%, transparent)"
                  nodeColor={(node) => TONE_HEX[((node.data as { tone?: Tone }).tone ?? "idle") as Tone]}
                />
              )}
            </ReactFlow>
          </div>
        )}
      </div>
    </div>
  );
}
