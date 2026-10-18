import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { EventOut, Stack, StackItem } from "../src/api/types";
import { TaskStackLinks } from "../src/features/stacks";
import { StackDetailPage, StacksPage } from "../src/pages/Stacks";
import { describeStackEvent, parseItemDetail } from "../src/lib/stacks";
import { mockApi, renderPage } from "./utils";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const TS = "2026-01-01T00:00:00Z";

function item(overrides: Partial<StackItem> & Pick<StackItem, "task_key" | "state">): StackItem {
  return {
    id: `i-${overrides.task_key}`,
    stack_id: "s1",
    task_id: `t-${overrides.task_key}`,
    task_title: `Change ${overrides.task_key}`,
    merge_order: null,
    detail: null,
    created_at: TS,
    updated_at: TS,
    ...overrides,
  };
}

const conflictDetail = JSON.stringify({
  merges: {},
  conflicts: {
    svc: { repository: "svc", paths: ["src/auth.py", "src/session.py"], instruction: "rebase onto the tip" },
  },
});
const ejectionDetail = JSON.stringify({
  merges: {},
  conflicts: {},
  ejection: { position: 2, verification_run_id: "r-fail", failed_checks: ["unit tests"], signatures: {} },
});

const stack: Stack = {
  id: "s1",
  workspace_id: "w1",
  parent_task_id: "p1",
  parent_key: "WG-1",
  parent_title: "Session reliability",
  status: "verified",
  repositories: [
    {
      repository_id: "r1",
      repository_name: "svc",
      base_sha: "0123456789abcdef0123456789abcdef01234567",
      integration_branch: "wg/stack/wg-1",
      integration_tip: "fedcba9876543210fedcba9876543210fedcba98",
    },
  ],
  // Deliberately out of merge order: the page must show them in merge order.
  items: [
    item({ task_key: "WG-4", state: "ejected", merge_order: 3, detail: ejectionDetail }),
    item({ task_key: "WG-2", state: "verified", merge_order: 1 }),
    item({ task_key: "WG-3", state: "conflict", merge_order: 2, detail: conflictDetail }),
    item({ task_key: "WG-5", state: "pending" }),
  ],
  created_at: TS,
  updated_at: TS,
};

const landed: Stack = {
  ...stack,
  id: "s2",
  parent_key: "WG-9",
  parent_title: "Old initiative",
  status: "landed",
  items: [item({ task_key: "WG-10", state: "verified", merge_order: 1 })],
};

function event(seq: number, event_type: string, entity_type: string, entity_id: string, data: Record<string, unknown>): EventOut {
  return {
    seq,
    id: `e${seq}`,
    event_type,
    entity_type,
    entity_id,
    actor: "human:tester",
    actor_kind: "human",
    occurred_at: TS,
    attempt_id: null,
    data,
  };
}

const history: EventOut[] = [
  event(1, "stack.created", "stack", "s1", { parent_key: "WG-1", integration_branch: "wg/stack/wg-1" }),
  event(2, "verification.completed", "verification_run", "run-tip", { status: "failed", failed_checks: ["unit tests"] }),
  event(3, "stack.verification_failed", "stack", "s1", { run_id: "run-tip", failed_checks: ["unit tests"] }),
  event(4, "stack.bisect_probe", "stack", "s1", { position: 2, passed: false, run_id: "run-p2", failed_checks: ["unit tests"] }),
  event(5, "stack.bisect_probe", "stack", "s1", { position: 1, passed: true, run_id: "run-p1", failed_checks: [] }),
  event(6, "stack.item_ejected", "stack", "s1", { task_key: "WG-3", position: 2, verification_run_id: "run-p2", failed_checks: ["unit tests"] }),
  event(7, "verification.completed", "verification_run", "run-final", { status: "passed", failed_checks: [] }),
  event(8, "stack.verified", "stack", "s1", { run_id: "run-final", reason: "full checks passed" }),
];

const routes = {
  "GET /ws/main/stacks": [stack, landed],
  "GET /ws/main/stacks/WG-1": stack,
  "GET /ws/main/stacks/WG-1/history": history,
};

describe("StacksPage", () => {
  it("lists active stacks with item counts by state, and asks for landed ones on request", async () => {
    const calls = mockApi({ ...routes, "GET /ws/main/stacks": [stack] });
    renderPage(<StacksPage />, { path: "/stacks", route: "/stacks" });
    const table = await screen.findByTestId("stacks");
    const row = within(table).getAllByTestId("stack-row")[0];
    expect(row).toHaveTextContent("WG-1");
    expect(row).toHaveTextContent("Session reliability");
    expect(within(row).getByTestId("state-pending")).toHaveTextContent("1 pending");
    expect(within(row).getByTestId("state-verified")).toHaveTextContent("1 verified");
    expect(within(row).getByTestId("state-conflict")).toHaveTextContent("1 conflict");
    expect(within(row).getByTestId("state-ejected")).toHaveTextContent("1 ejected");
    expect(within(row).queryByTestId("state-merged")).toBeNull();
    const listed = calls.find((c) => c.method === "GET" && c.path === "/ws/main/stacks");
    expect(listed?.search.get("include_inactive")).toBe("false");

    fireEvent.click(screen.getByRole("checkbox", { name: /Include landed and abandoned/ }));
    await waitFor(() =>
      expect(calls.some((c) => c.path === "/ws/main/stacks" && c.search.get("include_inactive") === "true")).toBe(true),
    );
  });
});

describe("StackDetailPage", () => {
  it("shows repositories, then items in merge order with their state and detail", async () => {
    mockApi(routes);
    renderPage(<StackDetailPage />, { path: "/stacks/:ref", route: "/stacks/WG-1" });
    const repos = await screen.findByTestId("stack-repositories");
    expect(repos).toHaveTextContent("svc");
    expect(repos).toHaveTextContent("0123456789ab");
    expect(repos).toHaveTextContent("wg/stack/wg-1");
    expect(repos).toHaveTextContent("fedcba987654");

    const rows = within(screen.getByTestId("stack-items")).getAllByTestId("stack-item");
    expect(rows.map((r) => r.querySelector("td")?.textContent)).toEqual(["1", "2", "3", "—"]);
    expect(rows[1]).toHaveTextContent("WG-3");
    expect(within(rows[1]).getByTestId("state-conflict")).toBeInTheDocument();
    expect(rows[1]).toHaveTextContent("Conflict in svc: src/auth.py, src/session.py");
    expect(rows[2]).toHaveTextContent("Ejected at prefix 2: unit tests");
    expect(within(rows[0]).getByTestId("state-verified")).toBeInTheDocument();
    expect(within(rows[3]).getByTestId("state-pending")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /WG-3/ })).toHaveAttribute("href", "/tasks/WG-3");
  });

  it("renders the bisection timeline with runs, probes and the culprit", async () => {
    mockApi(routes);
    renderPage(<StackDetailPage />, { path: "/stacks/:ref", route: "/stacks/WG-1" });
    const timeline = await screen.findByTestId("stack-timeline");
    expect(within(timeline).getByText("Bisect probe, prefix 2: failed")).toBeInTheDocument();
    expect(within(timeline).getByText("Bisect probe, prefix 1: passed")).toBeInTheDocument();
    expect(within(timeline).getAllByTestId("timeline-probe-failed")).toHaveLength(1);
    expect(within(timeline).getAllByTestId("timeline-probe-passed")).toHaveLength(1);
    expect(within(timeline).getAllByTestId("timeline-run-failed")).toHaveLength(1);
    expect(within(timeline).getAllByTestId("timeline-run-passed")).toHaveLength(1);
    const culprit = timeline.querySelector('[data-culprit="true"]');
    expect(culprit).toHaveTextContent("Culprit WG-3 ejected at prefix 2");
    // The fixture is verified but has unfinished children, so landing is blocked rather than offered.
    const landing = screen.getByTestId("stack-landing");
    expect(landing).toHaveTextContent("cannot land while children are unfinished");
    expect(landing).toHaveTextContent("WG-3 (conflict)");
    expect(landing).not.toHaveTextContent("workgraph task accept");
  });

  it("queues integration and verification through the API", async () => {
    const calls = mockApi({
      ...routes,
      "POST /ws/main/stacks/WG-1/integrate": { stack: "WG-1", jobs: [{ repository: "svc", job_id: "j1" }] },
      "POST /ws/main/stacks/WG-1/verify": { stack: "WG-1", job_id: "j2" },
    });
    renderPage(<StackDetailPage />, { path: "/stacks/:ref", route: "/stacks/WG-1" });
    await screen.findByTestId("stack-items");
    fireEvent.click(screen.getByRole("button", { name: "Integrate" }));
    expect(await screen.findByText("Queued integration in 1 repository.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Verify" }));
    expect(await screen.findByText(/Queued verification of the integration tip/)).toBeInTheDocument();
    expect(calls.map((c) => `${c.method} ${c.path}`)).toEqual(
      expect.arrayContaining(["POST /ws/main/stacks/WG-1/integrate", "POST /ws/main/stacks/WG-1/verify"]),
    );
  });
});

describe("stack helpers", () => {
  it("reads item detail stored as JSON and keeps free text as the reason", () => {
    expect(parseItemDetail(conflictDetail).conflicts).toEqual([
      { repository: "svc", paths: ["src/auth.py", "src/session.py"], instruction: "rebase onto the tip" },
    ]);
    expect(parseItemDetail(ejectionDetail).ejection).toEqual({ position: 2, failedChecks: ["unit tests"] });
    expect(parseItemDetail("waiting for review")).toEqual({ conflicts: [], ejection: null, reason: "waiting for review" });
    expect(parseItemDetail(null).conflicts).toEqual([]);
  });

  it("describes bisect probes and ejections as timeline rows", () => {
    expect(describeStackEvent(history[3])).toMatchObject({ tone: "fail", culprit: false, run: { kind: "probe", passed: false } });
    expect(describeStackEvent(history[5])).toMatchObject({ tone: "fail", culprit: true });
    expect(describeStackEvent(history[6])).toMatchObject({ tone: "ok", run: { kind: "run", passed: true } });
  });
});

describe("TaskStackLinks", () => {
  it("links a task that is an item of a stack, and no stack when it is in none", async () => {
    mockApi({ "GET /ws/main/tasks/WG-3/stacks": [stack], "GET /ws/main/tasks/WG-7/stacks": [] });
    const { unmount } = renderPage(<TaskStackLinks taskKey="WG-3" />, { path: "/tasks/:ref", route: "/tasks/WG-3" });
    const link = await screen.findByRole("link", { name: /In stack WG-1/ });
    expect(link).toHaveAttribute("href", "/stacks/WG-1");
    unmount();
    const other = renderPage(<TaskStackLinks taskKey="WG-7" />, { path: "/tasks/:ref", route: "/tasks/WG-7" });
    await waitFor(() => expect(other.container.querySelector('[data-testid="task-stack-links"]')).toBeNull());
  });
});
