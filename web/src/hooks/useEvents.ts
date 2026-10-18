import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { API_BASE, getToken } from "../api/client";
import type { EventOut } from "../api/types";

type Listener = (event: EventOut) => void;
const listeners = new Set<Listener>();

/** Subscribe to the workspace SSE stream once; invalidates queries on relevant events. */
export function useEventStream(ws: string): { connected: boolean } {
  const client = useQueryClient();
  const [connected, setConnected] = useState(false);
  const timer = useRef<number | null>(null);

  useEffect(() => {
    if (!ws || typeof EventSource === "undefined") return;
    const token = getToken();
    const url = `${API_BASE}/ws/${encodeURIComponent(ws)}/events/stream${token ? `?token=${encodeURIComponent(token)}` : ""}`;
    const source = new EventSource(url);
    const pending = new Set<string>();
    const flush = () => {
      timer.current = null;
      const keys = [...pending];
      pending.clear();
      for (const key of keys) void client.invalidateQueries({ queryKey: [key] });
    };
    const schedule = (key: string) => {
      pending.add(key);
      if (timer.current === null) timer.current = window.setTimeout(flush, 400);
    };
    source.onopen = () => setConnected(true);
    source.onerror = () => setConnected(false);
    source.addEventListener("domain", (raw) => {
      const event = JSON.parse((raw as MessageEvent<string>).data) as EventOut;
      listeners.forEach((l) => l(event));
      const prefix = event.event_type.split(".")[0];
      schedule("events");
      if (["task", "classification", "verification", "attempt", "git", "agent_session"].includes(prefix)) {
        schedule("tasks");
        schedule("task");
        schedule("summary");
        schedule("graph");
      }
      if (["attempt", "verification", "worktree"].includes(prefix)) {
        schedule("attempts");
        schedule("run");
      }
      if (prefix === "automation" || prefix === "notification") schedule("automation");
      if (prefix === "repository" || prefix === "git") schedule("repositories");
      if (prefix === "stack" || prefix === "verification") schedule("stacks");
    });
    source.addEventListener("queue", () => {
      schedule("jobs");
      schedule("workers");
      schedule("summary");
    });
    return () => {
      source.close();
      if (timer.current !== null) window.clearTimeout(timer.current);
      setConnected(false);
    };
  }, [ws, client]);

  return { connected };
}

export function onDomainEvent(listener: Listener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}
