import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError, api, buildUrl, errorMessage, getToken } from "../src/api/client";

afterEach(() => {
  vi.unstubAllGlobals();
  document.head.innerHTML = "";
});

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

describe("api client", () => {
  it("reads the token injected by the backend and sends it as a bearer token", async () => {
    document.head.innerHTML = '<meta name="workgraph-token" content="secret-123">';
    expect(getToken()).toBe("secret-123");
    const fetchMock = vi.fn(async () => json({ ok: true }));
    vi.stubGlobal("fetch", fetchMock);
    await api("/meta");
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("/api/v1/meta");
    expect((init.headers as Record<string, string>).Authorization).toBe("Bearer secret-123");
  });

  it("parses structured API errors", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => json({ error: { code: "version_conflict", message: "task changed", details: { expected: 1 } } }, 409)),
    );
    const error = await api("/x", { method: "PATCH", body: {} }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).status).toBe(409);
    expect((error as ApiError).code).toBe("version_conflict");
    expect(errorMessage(error)).toBe("task changed");
  });

  it("serialises query parameters including repeated values and skips empties", () => {
    expect(buildUrl("/ws/main/tasks", { status: ["ready", "blocked"], q: "", limit: 5, project: undefined })).toBe(
      "/api/v1/ws/main/tasks?status=ready&status=blocked&limit=5",
    );
  });

  it("includes validation details in error messages", () => {
    const error = new ApiError(422, "validation_failed", "invalid request", {
      errors: [{ msg: "field required", loc: ["body", "title"] }],
    });
    expect(errorMessage(error)).toBe("invalid request: field required (body.title)");
  });
});
