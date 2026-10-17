import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { get } from "../api/client";
import type { GraphEdge, GraphNode, Project, Repository } from "../api/types";

export function useGraph(ws: string, project?: string) {
  return useQuery({
    queryKey: ["graph", ws, project ?? ""],
    queryFn: () => get<{ nodes: GraphNode[]; edges: GraphEdge[] }>(`/ws/${ws}/graph`, { project }),
  });
}

/** id -> node lookup for showing task keys instead of UUIDs. */
export function useTaskIndex(ws: string): Map<string, GraphNode> {
  const graph = useGraph(ws);
  return useMemo(() => new Map((graph.data?.nodes ?? []).map((n) => [n.id, n])), [graph.data]);
}

export function useRepoNames(ws: string): Map<string, string> {
  const repos = useQuery({ queryKey: ["repositories", ws], queryFn: () => get<Repository[]>(`/ws/${ws}/repositories`) });
  return useMemo(() => new Map((repos.data ?? []).map((r) => [r.id, r.name])), [repos.data]);
}

export function useProjectNames(ws: string): Map<string, Project> {
  const projects = useQuery({ queryKey: ["projects", ws], queryFn: () => get<Project[]>(`/ws/${ws}/projects`) });
  return useMemo(() => new Map((projects.data ?? []).map((p) => [p.id, p])), [projects.data]);
}
