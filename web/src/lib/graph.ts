import type { GraphEdge, GraphNode } from "../api/types";

/** Layout cell of one task node, gaps included. */
export const COL_W = 280;
export const ROW_H = 96;
export const TASK_W = 220;
const TASK_H = 80;
const GROUP_PAD = 16;
const GROUP_HEADER = 40;

export type GroupMode = "parent" | "area" | "none";

export interface Size {
  w: number;
  h: number;
}

const CELL: Size = { w: COL_W, h: ROW_H };

/** Nodes touched by at least one of the given edges. */
export function linkedIds(edges: Pick<GraphEdge, "source" | "target">[]): Set<string> {
  return new Set(edges.flatMap((e) => [e.source, e.target]));
}

/**
 * Layered layout: a node's column is the longest path from a root over dependency and parent edges.
 * Nodes with no dependency or parent edge are shelf-packed below the layers, so a mostly unlinked
 * backlog does not become one column thousands of rows tall. `sizeOf` gives each node's cell (gaps
 * included); by default every node is one task cell.
 */
export function layeredLayout(
  nodes: { id: string }[],
  edges: (Pick<GraphEdge, "source" | "target" | "kind"> & { relation_type?: string | null })[],
  sizeOf: (id: string) => Size = () => CELL,
): Map<string, { x: number; y: number }> {
  const incoming = new Map<string, string[]>();
  for (const n of nodes) incoming.set(n.id, []);
  const structural = edges.filter((e) => e.kind !== "relation" && incoming.has(e.source) && incoming.has(e.target));
  for (const e of structural) incoming.get(e.target)!.push(e.source);
  const linked = linkedIds(structural);
  const level = new Map<string, number>();
  const visiting = new Set<string>();
  const depth = (id: string): number => {
    if (level.has(id)) return level.get(id)!;
    if (visiting.has(id)) return 0; // tasks cannot form cycles, but groups of tasks can
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
  let x = 0;
  let layersHeight = 0;
  for (const d of [...columns.keys()].sort((a, b) => a - b)) {
    const ids = columns.get(d)!;
    let y = 0;
    for (const id of ids) {
      positions.set(id, { x, y });
      y += sizeOf(id).h;
    }
    layersHeight = Math.max(layersHeight, y);
    x += Math.max(...ids.map((id) => sizeOf(id).w));
  }
  if (isolated.length) {
    const top = layersHeight ? layersHeight + ROW_H : 0;
    // roughly square in screen space, and never narrower than the layers above it
    const area = isolated.reduce((sum, id) => sum + sizeOf(id).w * sizeOf(id).h, 0);
    const rowWidth = Math.max(x, COL_W * Math.ceil(Math.sqrt(area) / COL_W));
    let cx = 0;
    let cy = top;
    let rowHeight = 0;
    for (const id of isolated) {
      const s = sizeOf(id);
      if (cx > 0 && cx + s.w > rowWidth) {
        cx = 0;
        cy += rowHeight;
        rowHeight = 0;
      }
      positions.set(id, { x: cx, y: cy });
      cx += s.w;
      rowHeight = Math.max(rowHeight, s.h);
    }
  }
  return positions;
}

export interface TaskGroup {
  /** Stable, readable key used in the URL: a parent task key, an `area:` tag, or `unsorted`. */
  key: string;
  title: string;
  /** The parent task heading a parent group. */
  headKey: string | null;
  members: GraphNode[];
}

const UNSORTED = "unsorted";

/**
 * Which group a task belongs to. In `parent` mode a task joins the group of its top-level ancestor
 * (a batch parent and everything under it), and a task outside any hierarchy falls back to its
 * `area:` tag. `byId` should hold every node, so a hidden (e.g. accepted) batch parent still holds
 * its open children together.
 */
export function groupKeyOf(n: GraphNode, mode: GroupMode, byId: Map<string, GraphNode>, parentIds: Set<string>): string {
  const area = n.tags?.find((t) => t.startsWith("area:"));
  if (mode === "area") return area ?? UNSORTED;
  let root = n;
  const seen = new Set<string>();
  while (root.parent_id && byId.has(root.parent_id) && !seen.has(root.id)) {
    seen.add(root.id);
    root = byId.get(root.parent_id)!;
  }
  if (root !== n || parentIds.has(n.id)) return root.key;
  return area ?? UNSORTED;
}

export function groupTasks(visible: GraphNode[], all: GraphNode[], mode: GroupMode): TaskGroup[] {
  const byId = new Map(all.map((n) => [n.id, n]));
  const byKey = new Map(all.map((n) => [n.key, n]));
  const parentIds = new Set(all.flatMap((n) => (n.parent_id ? [n.parent_id] : [])));
  const groups = new Map<string, TaskGroup>();
  for (const n of visible) {
    const key = groupKeyOf(n, mode, byId, parentIds);
    let g = groups.get(key);
    if (!g) {
      const head = byKey.get(key);
      const title = head ? head.title : key === UNSORTED ? (mode === "area" ? "no area tag" : "no parent or area tag") : key.slice("area:".length);
      g = { key, title, headKey: head ? head.key : null, members: [] };
      groups.set(key, g);
    }
    g.members.push(n);
  }
  return [...groups.values()];
}

export type Unit =
  | { kind: "task"; id: string; task: GraphNode; group: TaskGroup | null }
  | { kind: "collapsed"; id: string; group: TaskGroup }
  | { kind: "expanded"; id: string; group: TaskGroup; size: Size; inner: Map<string, { x: number; y: number }> };

export interface GroupedEdge {
  id: string;
  source: string;
  target: string;
  /** The strongest kind among the merged edges: depends_on, then parent, then relation. */
  kind: GraphEdge["kind"];
  relation_type: string | null;
  count: number;
}

export interface GroupedGraph {
  /** Top-level units laid out on the canvas: plain tasks, collapsed groups and expanded group boxes. */
  units: Unit[];
  positions: Map<string, { x: number; y: number }>;
  edges: GroupedEdge[];
  groups: TaskGroup[];
}

export const groupNodeId = (key: string) => `group:${key}`;

const KIND_RANK: Record<GraphEdge["kind"], number> = { depends_on: 0, parent: 1, relation: 2 };

/**
 * Groups the visible tasks and lays them out. A group with more than one task is a single node
 * unless its key is in `expanded`, in which case it becomes a box with its tasks laid out inside.
 * Edges whose ends fall in the same collapsed group disappear; edges between the same two nodes
 * are merged and counted. In `parent` mode, parent edges inside a group are implied by the box.
 */
export function buildGroupedGraph(
  visible: GraphNode[],
  all: GraphNode[],
  edges: GraphEdge[],
  mode: GroupMode,
  expanded: Set<string>,
): GroupedGraph {
  if (mode === "none") {
    const units: Unit[] = visible.map((t) => ({ kind: "task", id: t.id, task: t, group: null }));
    const out = edges.map((e, i) => ({ ...e, id: `${e.kind}-${e.source}-${e.target}-${i}`, count: 1 }));
    return { units, positions: layeredLayout(visible, edges), edges: out, groups: [] };
  }
  const groups = groupTasks(visible, all, mode);
  const groupOf = new Map<string, TaskGroup>();
  for (const g of groups) for (const m of g.members) groupOf.set(m.id, g);
  const grouped = (g: TaskGroup) => g.members.length > 1;
  const internal = (e: GraphEdge) => mode === "parent" && e.kind === "parent" && groupOf.get(e.source) === groupOf.get(e.target);
  const shown = edges.filter((e) => groupOf.has(e.source) && groupOf.has(e.target) && !internal(e));

  const units: Unit[] = [];
  for (const g of groups) {
    if (!grouped(g)) {
      units.push({ kind: "task", id: g.members[0].id, task: g.members[0], group: null });
    } else if (expanded.has(g.key)) {
      const ids = new Set(g.members.map((m) => m.id));
      const inner = layeredLayout(g.members, shown.filter((e) => ids.has(e.source) && ids.has(e.target)));
      const xs = [...inner.values()].map((p) => p.x);
      const ys = [...inner.values()].map((p) => p.y);
      for (const [id, p] of inner) inner.set(id, { x: p.x + GROUP_PAD, y: p.y + GROUP_HEADER });
      const size = { w: Math.max(...xs) + TASK_W + 2 * GROUP_PAD, h: Math.max(...ys) + TASK_H + GROUP_HEADER + GROUP_PAD };
      units.push({ kind: "expanded", id: groupNodeId(g.key), group: g, size, inner });
    } else {
      units.push({ kind: "collapsed", id: groupNodeId(g.key), group: g });
    }
  }
  // biggest boxes first, so shelf packing leaves fewer holes; plain tasks keep their order at the end
  const rank = (u: Unit) => (u.kind === "expanded" ? 0 : u.kind === "collapsed" ? 1 : 2);
  units.sort((a, b) => rank(a) - rank(b) || (rank(a) < 2 ? memberCount(b) - memberCount(a) : 0));

  // where an edge end is drawn, and which top-level unit it belongs to for layout
  const drawnAt = (id: string) => {
    const g = groupOf.get(id)!;
    return grouped(g) && !expanded.has(g.key) ? groupNodeId(g.key) : id;
  };
  const unitOf = (id: string) => {
    const g = groupOf.get(id)!;
    return grouped(g) ? groupNodeId(g.key) : id;
  };

  const merged = new Map<string, GroupedEdge>();
  const outer: Pick<GraphEdge, "source" | "target" | "kind">[] = [];
  for (const e of shown) {
    const source = drawnAt(e.source);
    const target = drawnAt(e.target);
    if (source === target) continue;
    const key = `${source}->${target}`;
    const prev = merged.get(key);
    if (!prev) merged.set(key, { id: key, source, target, kind: e.kind, relation_type: e.relation_type, count: 1 });
    else {
      prev.count += 1;
      if (KIND_RANK[e.kind] < KIND_RANK[prev.kind]) Object.assign(prev, { kind: e.kind, relation_type: e.relation_type });
    }
    const us = unitOf(e.source);
    const ut = unitOf(e.target);
    if (us !== ut) outer.push({ source: us, target: ut, kind: e.kind });
  }
  const sizes = new Map(units.map((u) => [u.id, u.kind === "expanded" ? { w: u.size.w + COL_W - TASK_W, h: u.size.h + 40 } : CELL]));
  const positions = layeredLayout(units, outer, (id) => sizes.get(id) ?? CELL);
  return { units, positions, edges: [...merged.values()], groups };
}

function memberCount(u: Unit): number {
  return u.kind === "task" ? 1 : u.group.members.length;
}
