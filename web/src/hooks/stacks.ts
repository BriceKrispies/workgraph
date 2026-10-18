import { useQuery } from "@tanstack/react-query";
import { ApiError, get } from "../api/client";
import type { EventOut, Stack } from "../api/types";

/** Stacks of the workspace; landed and abandoned ones only when asked for. */
export function useStacks(ws: string, includeInactive: boolean) {
  return useQuery({
    queryKey: ["stacks", ws, includeInactive ? "all" : "active"],
    queryFn: () => get<Stack[]>(`/ws/${ws}/stacks`, { include_inactive: includeInactive }),
  });
}

/** The most recent stack of a parent task, or null when the task has none. */
export function useStack(ws: string, ref: string) {
  return useQuery({
    queryKey: ["stacks", ws, ref],
    queryFn: () => getOrNull<Stack>(`/ws/${ws}/stacks/${ref}`),
  });
}

export function useStackHistory(ws: string, ref: string) {
  return useQuery({
    queryKey: ["stacks", ws, ref, "history"],
    queryFn: () => get<EventOut[]>(`/ws/${ws}/stacks/${ref}/history`),
  });
}

/** Every stack a task is an item of; empty when it is in none. */
export function useTaskStacks(ws: string, taskKey: string) {
  return useQuery({
    queryKey: ["task", ws, taskKey, "stacks"],
    queryFn: async () => (await getOrNull<Stack[]>(`/ws/${ws}/tasks/${taskKey}/stacks`)) ?? [],
  });
}

async function getOrNull<T>(path: string): Promise<T | null> {
  try {
    return await get<T>(path);
  } catch (error) {
    if (error instanceof ApiError && error.status === 404) return null;
    throw error;
  }
}
