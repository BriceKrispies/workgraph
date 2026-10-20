/** Every status in the product maps onto one of eight lifecycle tones (see the .tone-* classes in index.css). */
export type Tone = "idle" | "ready" | "blocked" | "claimed" | "active" | "review" | "done" | "failed";

const TONES: Record<string, Tone> = {
  // tasks
  backlog: "idle",
  ready: "ready",
  blocked: "blocked",
  claimed: "claimed",
  running: "active",
  awaiting_verification: "review",
  awaiting_review: "review",
  accepted: "done",
  failed: "failed",
  cancelled: "idle",
  // attempts, jobs, checks, runs
  queued: "ready",
  preparing: "claimed",
  verifying: "review",
  succeeded: "done",
  passed: "done",
  timed_out: "blocked",
  dead: "failed",
  error: "failed",
  skipped: "blocked",
  previewed: "ready",
  not_matched: "idle",
  // health
  ok: "done",
  degraded: "blocked",
  unavailable: "failed",
  // stacks and stack items
  open: "idle",
  integrating: "claimed",
  verified: "done",
  landed: "done",
  abandoned: "idle",
  pending: "idle",
  merged: "claimed",
  conflict: "blocked",
  ejected: "failed",
};

export const toneOf = (status: string | null | undefined): Tone => TONES[status ?? ""] ?? "idle";

/** Statuses that are finished without success, drawn struck through. */
export const STRUCK = new Set(["cancelled", "abandoned"]);

/** Concrete colours for canvases that cannot read CSS variables (minimap). */
export const TONE_HEX: Record<Tone, string> = {
  idle: "#94a0b2",
  ready: "#1f8fd1",
  blocked: "#d99000",
  claimed: "#5965e0",
  active: "#8b5cf6",
  review: "#0aa2a8",
  done: "#1e9e62",
  failed: "#e0405c",
};
