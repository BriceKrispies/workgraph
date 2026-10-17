import { fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TasksPage } from "../src/pages/Tasks";
import { mockApi, renderPage, task } from "./utils";

afterEach(() => vi.unstubAllGlobals());

describe("TasksPage", () => {
  it("renders task rows and refetches with filters", async () => {
    const calls = mockApi({
      "GET /ws/main/projects": [],
      "GET /ws/main/repositories": [],
      "GET /ws/main/tasks": (url: URL) => {
        const blocked = url.searchParams.get("status") === "blocked";
        return {
          items: blocked ? [] : [task(), task({ id: "t2", key: "WG-2", title: "Add dark mode", status: "accepted" })],
          total: blocked ? 0 : 2,
          limit: 50,
          offset: 0,
          next_offset: null,
        };
      },
    });
    renderPage(<TasksPage />, { path: "/tasks", route: "/tasks" });
    expect(await screen.findByText("Fix login crash")).toBeInTheDocument();
    expect(screen.getByText("Add dark mode")).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Status"), { target: { value: "blocked" } });
    expect(await screen.findByText("No tasks match")).toBeInTheDocument();
    const last = calls.filter((c) => c.path === "/ws/main/tasks").at(-1)!;
    expect(last.search.get("status")).toBe("blocked");
  });

  it("applies bulk changes to selected tasks", async () => {
    const calls = mockApi({
      "GET /ws/main/projects": [],
      "GET /ws/main/repositories": [],
      "GET /ws/main/tasks": { items: [task()], total: 1, limit: 50, offset: 0, next_offset: null },
      "POST /ws/main/tasks/bulk": { updated: ["WG-1"], failed: {} },
    });
    renderPage(<TasksPage />, { path: "/tasks", route: "/tasks" });
    fireEvent.click(await screen.findByLabelText("select WG-1"));
    fireEvent.change(screen.getByLabelText("Bulk status"), { target: { value: "blocked" } });
    await waitFor(() => expect(calls.some((c) => c.method === "POST" && c.path === "/ws/main/tasks/bulk")).toBe(true));
    const post = calls.find((c) => c.path === "/ws/main/tasks/bulk")!;
    expect(post.body).toEqual({ tasks: ["WG-1"], update: { status: "blocked" } });
  });
});
