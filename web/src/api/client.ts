/** Minimal typed client for the Workgraph REST API (/api/v1). */

export class ApiError extends Error {
  readonly status: number;
  readonly code: string;
  readonly details: Record<string, unknown>;

  constructor(status: number, code: string, message: string, details: Record<string, unknown> = {}) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
    this.details = details;
  }
}

export const API_BASE = "/api/v1";

/** The backend embeds the local API token in index.html; dev servers may use VITE_WORKGRAPH_TOKEN. */
export function getToken(): string | null {
  if (typeof document !== "undefined") {
    const meta = document.querySelector('meta[name="workgraph-token"]');
    const value = meta?.getAttribute("content");
    if (value) return value;
  }
  const fromEnv = import.meta.env?.VITE_WORKGRAPH_TOKEN as string | undefined;
  return fromEnv || null;
}

export type Query = Record<string, string | number | boolean | null | undefined | (string | number)[]>;

export function buildUrl(path: string, query?: Query): string {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query ?? {})) {
    if (value === undefined || value === null || value === "") continue;
    if (Array.isArray(value)) value.forEach((v) => params.append(key, String(v)));
    else params.append(key, String(value));
  }
  const qs = params.toString();
  return `${API_BASE}${path}${qs ? `?${qs}` : ""}`;
}

async function parseError(response: Response): Promise<ApiError> {
  let body: unknown = null;
  try {
    body = await response.json();
  } catch {
    // non-JSON error body
  }
  const err = (body as { error?: { code?: string; message?: string; details?: Record<string, unknown> } } | null)?.error;
  if (err) return new ApiError(response.status, err.code ?? "error", err.message ?? response.statusText, err.details ?? {});
  const detail = (body as { detail?: unknown } | null)?.detail;
  return new ApiError(response.status, "http_error", typeof detail === "string" ? detail : `${response.status} ${response.statusText}`);
}

export async function api<T>(
  path: string,
  options: { method?: string; query?: Query; body?: unknown; signal?: AbortSignal } = {},
): Promise<T> {
  const headers: Record<string, string> = { Accept: "application/json" };
  const token = getToken();
  if (token) headers.Authorization = `Bearer ${token}`;
  let body: BodyInit | undefined;
  if (options.body !== undefined) {
    headers["Content-Type"] = "application/json";
    body = JSON.stringify(options.body);
  }
  const response = await fetch(buildUrl(path, options.query), {
    method: options.method ?? (options.body !== undefined ? "POST" : "GET"),
    headers,
    body,
    signal: options.signal,
  });
  if (!response.ok) throw await parseError(response);
  if (response.status === 204) return undefined as T;
  const type = response.headers.get("content-type") ?? "";
  if (!type.includes("application/json")) return (await response.blob()) as unknown as T;
  return (await response.json()) as T;
}

export const get = <T>(path: string, query?: Query) => api<T>(path, { query });
export const post = <T>(path: string, body: unknown = {}, query?: Query) => api<T>(path, { method: "POST", body, query });
export const patch = <T>(path: string, body: unknown) => api<T>(path, { method: "PATCH", body });
export const put = <T>(path: string, body: unknown) => api<T>(path, { method: "PUT", body });
export const del = <T>(path: string) => api<T>(path, { method: "DELETE" });

export function errorMessage(error: unknown): string {
  if (error instanceof ApiError) {
    const errors = error.details?.errors;
    if (Array.isArray(errors) && errors.length) {
      const first = errors[0] as { msg?: string; loc?: unknown[] };
      return `${error.message}: ${first.msg ?? ""}${first.loc ? ` (${first.loc.join(".")})` : ""}`;
    }
    return error.message;
  }
  return error instanceof Error ? error.message : String(error);
}
