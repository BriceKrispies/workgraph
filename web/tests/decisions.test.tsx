import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DecisionsPage } from "../src/pages/Decisions";
import { mockApi, renderPage } from "./utils";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const openDecision = {
  id: "d1",
  task_id: "t1",
  task_key: "WG-1",
  question: "Which database should the cache use?",
  options: [
    { id: "a", label: "Use SQLite", detail: "Simple, one file.", recommended: true },
    { id: "b", label: "Use Redis", detail: "Shared across hosts.", recommended: false },
  ],
  status: "open",
  chosen_option_id: null,
  other_text: null,
  note: null,
  decided_by: null,
  decided_at: null,
  created_by: "agent:x",
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
  version: 1,
  task: {
    key: "WG-1",
    title: "Fix login crash",
    status: "awaiting_review",
    priority: "high",
    tags: ["auth"],
    description: "The login page crashes on refresh.",
  },
};

describe("DecisionsPage", () => {
  it("renders open decisions with their options", async () => {
    mockApi({
      "GET /ws/main/decisions": (url: URL) => ({ items: url.searchParams.get("status") === "open" ? [openDecision] : [] }),
    });
    renderPage(<DecisionsPage />, { path: "/decisions", route: "/decisions" });
    expect(await screen.findByText("Which database should the cache use?")).toBeInTheDocument();
    expect(screen.getByText("Fix login crash")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "WG-1" })).toHaveAttribute("href", "/tasks/WG-1");
    expect(screen.getByRole("radio", { name: /Use SQLite/ })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /Use Redis/ })).toBeInTheDocument();
    expect(screen.getByText("Recommended")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Decide" })).toBeDisabled();
  });

  it("posts the selected option when Decide is clicked", async () => {
    const calls = mockApi({
      "GET /ws/main/decisions": (url: URL) => ({ items: url.searchParams.get("status") === "open" ? [openDecision] : [] }),
      "POST /ws/main/decisions/d1/choose": { ...openDecision, status: "decided", chosen_option_id: "a" },
    });
    renderPage(<DecisionsPage />, { path: "/decisions", route: "/decisions" });
    fireEvent.click(await screen.findByRole("radio", { name: /Use SQLite/ }));
    const decide = screen.getByRole("button", { name: "Decide" });
    expect(decide).toBeEnabled();
    fireEvent.click(decide);
    await waitFor(() => expect(calls.some((c) => c.method === "POST" && c.path === "/ws/main/decisions/d1/choose")).toBe(true));
    const post = calls.find((c) => c.path === "/ws/main/decisions/d1/choose")!;
    expect(post.body).toEqual({ option_id: "a" });
  });

  it("sends other_text and the note when Other is chosen", async () => {
    const calls = mockApi({
      "GET /ws/main/decisions": (url: URL) => ({ items: url.searchParams.get("status") === "open" ? [openDecision] : [] }),
      "POST /ws/main/decisions/d1/choose": { ...openDecision, status: "decided", other_text: "Use Postgres" },
    });
    renderPage(<DecisionsPage />, { path: "/decisions", route: "/decisions" });
    fireEvent.click(await screen.findByRole("radio", { name: "Other" }));
    fireEvent.change(screen.getByLabelText("Other answer"), { target: { value: "  Use Postgres " } });
    fireEvent.change(screen.getByLabelText(/Note/), { target: { value: "Keep it boring" } });
    fireEvent.click(screen.getByRole("button", { name: "Decide" }));
    await waitFor(() => expect(calls.some((c) => c.method === "POST" && c.path === "/ws/main/decisions/d1/choose")).toBe(true));
    const post = calls.find((c) => c.path === "/ws/main/decisions/d1/choose")!;
    expect(post.body).toEqual({ other_text: "Use Postgres", note: "Keep it boring" });
  });
});
