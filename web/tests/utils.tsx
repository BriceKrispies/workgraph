import type { ReactElement } from "react";
import { render } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { vi } from "vitest";
import { WorkspaceProvider } from "../src/hooks/workspace";

export type Handler = (url: URL, init: RequestInit) => unknown;

export interface FetchCall {
  method: string;
  path: string;
  search: URLSearchParams;
  body: unknown;
  headers: Record<string, string>;
}

/** Install a fetch mock routing by "METHOD /path" (prefix /api/v1 stripped). */
export function mockApi(routes: Record<string, unknown>): FetchCall[] {
  const calls: FetchCall[] = [];
  const table: Record<string, unknown> = {
    "GET /meta": {
      version: "test",
      default_workspace: "main",
      execution_enabled: true,
      auto_schedule: false,
      agents: ["scripted"],
      llm: {},
    },
    "GET /workspaces": [
      { id: "w1", slug: "main", name: "Main", description: "", key_prefix: "WG", created_at: "2026-01-01T00:00:00Z" },
    ],
    ...routes,
  };
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string, init: RequestInit = {}) => {
      const url = new URL(input, "http://localhost");
      const method = (init.method ?? "GET").toUpperCase();
      const path = url.pathname.replace(/^\/api\/v1/, "");
      calls.push({
        method,
        path,
        search: url.searchParams,
        body: init.body ? JSON.parse(String(init.body)) : undefined,
        headers: (init.headers ?? {}) as Record<string, string>,
      });
      const entry = table[`${method} ${path}`];
      if (entry === undefined) {
        return new Response(JSON.stringify({ error: { code: "not_found", message: `no mock for ${method} ${path}` } }), {
          status: 404,
          headers: { "content-type": "application/json" },
        });
      }
      const value = typeof entry === "function" ? (entry as Handler)(url, init) : entry;
      return new Response(JSON.stringify(value), { status: 200, headers: { "content-type": "application/json" } });
    }),
  );
  return calls;
}

export function renderPage(element: ReactElement, { path = "/", route = "/" }: { path?: string; route?: string } = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[route]}>
        <WorkspaceProvider>
          <Routes>
            <Route path={path} element={element} />
          </Routes>
        </WorkspaceProvider>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

export function task(overrides: Record<string, unknown> = {}) {
  return {
    id: "t1",
    key: "WG-1",
    number: 1,
    title: "Fix login crash",
    description: "desc",
    acceptance_criteria: [],
    status: "ready",
    priority: "high",
    category: "bug_fix",
    tags: ["auth"],
    project_ids: [],
    repository_ids: [],
    parent_id: null,
    child_ids: [],
    depends_on: [],
    blocks: [],
    assignee: null,
    assigned_agent: null,
    claimed_by: null,
    auto_execute: false,
    version: 3,
    created_by: "human:x",
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    started_at: null,
    completed_at: null,
    accepted_at: null,
    archived_at: null,
    ...overrides,
  };
}
