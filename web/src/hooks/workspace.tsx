import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { get } from "../api/client";
import type { Meta, Workspace } from "../api/types";

interface WorkspaceState {
  ws: string;
  setWs: (slug: string) => void;
  workspaces: Workspace[];
  meta: Meta | undefined;
}

const Ctx = createContext<WorkspaceState | null>(null);
const STORAGE_KEY = "workgraph.workspace";

function stored(): string | null {
  try {
    return window.localStorage.getItem(STORAGE_KEY);
  } catch {
    return null;
  }
}

export function WorkspaceProvider({ children }: { children: ReactNode }) {
  const meta = useQuery({ queryKey: ["meta"], queryFn: () => get<Meta>("/meta") });
  const list = useQuery({ queryKey: ["workspaces"], queryFn: () => get<Workspace[]>("/workspaces") });
  const [ws, setWsState] = useState<string>(stored() ?? "");

  useEffect(() => {
    if (!list.data) return;
    const slugs = list.data.map((w) => w.slug);
    if (!ws || !slugs.includes(ws)) {
      const fallback = meta.data?.default_workspace ?? slugs[0] ?? "";
      if (fallback && fallback !== ws) setWsState(fallback);
    }
  }, [list.data, meta.data, ws]);

  const setWs = (slug: string) => {
    setWsState(slug);
    try {
      window.localStorage.setItem(STORAGE_KEY, slug);
    } catch {
      // storage may be unavailable; the selection still applies for this session
    }
  };

  return <Ctx.Provider value={{ ws, setWs, workspaces: list.data ?? [], meta: meta.data }}>{children}</Ctx.Provider>;
}

export function useWorkspace(): WorkspaceState {
  const value = useContext(Ctx);
  if (!value) throw new Error("useWorkspace outside WorkspaceProvider");
  return value;
}
