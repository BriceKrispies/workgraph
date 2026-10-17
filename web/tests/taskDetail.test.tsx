import { fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TaskDetailPage, availableActions } from "../src/pages/TaskDetail";
import { mockApi, renderPage, task } from "./utils";

afterEach(() => vi.unstubAllGlobals());

const detail = (overrides: Record<string, unknown> = {}) => ({
  ...task(overrides),
  relations: [],
  comments: [],
  rollup: null,
  dependency_state: { satisfied: true, unsatisfied: [] },
});

const common = {
  "GET /ws/main/graph": { nodes: [], edges: [] },
  "GET /ws/main/repositories": [],
  "GET /ws/main/tasks/WG-1/history": [],
};

describe("TaskDetailPage", () => {
  it("maps lifecycle states to the actions the server allows", () => {
    expect(availableActions("ready")).toEqual(["claim", "run"]);
    expect(availableActions("running")).toEqual(["complete", "release"]);
    expect(availableActions("awaiting_review")).toContain("accept");
    expect(availableActions("accepted")).toEqual([]);
  });

  it("claims a ready task through the claim endpoint", async () => {
    const calls = mockApi({
      ...common,
      "GET /ws/main/tasks/WG-1": detail(),
      "POST /ws/main/tasks/WG-1/claim": detail({ status: "claimed" }),
    });
    renderPage(<TaskDetailPage />, { path: "/tasks/:ref", route: "/tasks/WG-1" });
    fireEvent.click(await screen.findByRole("button", { name: "Claim" }));
    await waitFor(() =>
      expect(calls.some((c) => c.method === "POST" && c.path === "/ws/main/tasks/WG-1/claim")).toBe(true),
    );
  });

  it("requires a reason for manual acceptance and posts it", async () => {
    const calls = mockApi({
      ...common,
      "GET /ws/main/tasks/WG-1": detail({ status: "awaiting_review" }),
      "POST /ws/main/tasks/WG-1/accept": { decision: "accepted" },
    });
    renderPage(<TaskDetailPage />, { path: "/tasks/:ref", route: "/tasks/WG-1" });
    fireEvent.click(await screen.findByRole("button", { name: "Accept manually" }));
    fireEvent.change(screen.getByLabelText("Reason"), { target: { value: "reviewed the diff" } });
    fireEvent.click(screen.getAllByRole("button", { name: "Accept manually" }).at(-1)!);
    await waitFor(() => expect(calls.some((c) => c.path === "/ws/main/tasks/WG-1/accept")).toBe(true));
    expect(calls.find((c) => c.path === "/ws/main/tasks/WG-1/accept")!.body).toEqual({
      manual: true,
      reason: "reviewed the diff",
    });
  });

  it("renders the description as safe markdown", async () => {
    mockApi({ ...common, "GET /ws/main/tasks/WG-1": detail({ description: "**bold** <b>raw</b>" }) });
    const { container } = renderPage(<TaskDetailPage />, { path: "/tasks/:ref", route: "/tasks/WG-1" });
    expect(await screen.findByText("bold")).toBeInTheDocument();
    expect(container.querySelector(".markdown b")).toBeNull();
  });
});
