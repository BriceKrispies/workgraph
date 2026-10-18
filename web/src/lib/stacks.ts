import type { EventOut, StackItem, StackItemState } from "../api/types";
import { label, short } from "./format";

export const ITEM_STATES: StackItemState[] = ["pending", "ready", "merged", "conflict", "ejected", "verified"];

export interface ItemConflict {
  repository: string;
  paths: string[];
  instruction: string;
}

export interface ItemEjection {
  position: number | null;
  failedChecks: string[];
}

export interface ItemDetail {
  conflicts: ItemConflict[];
  ejection: ItemEjection | null;
  reason: string | null;
}

/** Reads an item's detail, which the server stores as JSON text (free text from an operator is kept as the reason). */
export function parseItemDetail(text: string | null | undefined): ItemDetail {
  const empty: ItemDetail = { conflicts: [], ejection: null, reason: null };
  if (!text) return empty;
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return { ...empty, reason: text };
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return empty;
  const d = parsed as Record<string, unknown>;
  const conflicts = Object.entries((d.conflicts ?? {}) as Record<string, Record<string, unknown>>).map(
    ([repository, c]) => ({
      repository,
      paths: Array.isArray(c.paths) ? c.paths.map(String) : [],
      instruction: typeof c.instruction === "string" ? c.instruction : "",
    }),
  );
  const ejected = d.ejection as Record<string, unknown> | undefined;
  const ejection = ejected
    ? {
        position: typeof ejected.position === "number" ? ejected.position : null,
        failedChecks: Array.isArray(ejected.failed_checks) ? ejected.failed_checks.map(String) : [],
      }
    : null;
  return { conflicts, ejection, reason: typeof d.reason === "string" ? d.reason : null };
}

/** The items in merge order; items not yet merged follow the merged ones, in the order they were added. */
export function inMergeOrder(items: StackItem[]): StackItem[] {
  return [...items].sort((a, b) => {
    if (a.merge_order === b.merge_order) return 0;
    if (a.merge_order === null) return 1;
    if (b.merge_order === null) return -1;
    return a.merge_order - b.merge_order;
  });
}

/** How many items of a stack are in each state, in state order. */
export function countStates(items: StackItem[]): Record<StackItemState, number> {
  const counts = Object.fromEntries(ITEM_STATES.map((s) => [s, 0])) as Record<StackItemState, number>;
  for (const item of items) counts[item.state] += 1;
  return counts;
}

export type Tone = "ok" | "fail" | "warn" | "info";

export interface TimelineEntry {
  id: string;
  tone: Tone;
  title: string;
  detail: string | null;
  /** The item that a bisection identified as the culprit. */
  culprit: boolean;
  /** A verification run or bisect probe, with its result, for the run-level rows. */
  run: { kind: "run" | "probe"; passed: boolean } | null;
}

const str = (value: unknown) => (typeof value === "string" ? value : "");
const list = (value: unknown) => (Array.isArray(value) ? value.map(String) : []);
const checks = (names: string[]) => (names.length ? `failed: ${names.join(", ")}` : null);

/** One timeline row for a stack event, or for a verification run event attributed to the stack. */
export function describeStackEvent(event: EventOut): TimelineEntry {
  const d = event.data;
  const base = { id: event.id, culprit: false, run: null };
  switch (event.event_type) {
    case "verification.completed": {
      const passed = d.status === "passed";
      return {
        ...base,
        tone: passed ? "ok" : "fail",
        title: `Verification run ${short(event.entity_id, 8)} ${passed ? "passed" : "failed"}`,
        detail: checks(list(d.failed_checks)),
        run: { kind: "run", passed },
      };
    }
    case "stack.bisect_probe": {
      const passed = d.passed === true;
      return {
        ...base,
        tone: passed ? "ok" : "fail",
        title: `Bisect probe, prefix ${String(d.position)}: ${passed ? "passed" : "failed"}`,
        detail: checks(list(d.failed_checks)),
        run: { kind: "probe", passed },
      };
    }
    case "stack.item_ejected":
      return {
        ...base,
        tone: "fail",
        title: `Culprit ${str(d.task_key)} ejected at prefix ${String(d.position)}`,
        detail: checks(list(d.failed_checks)),
        culprit: true,
      };
    case "stack.verified":
      return { ...base, tone: "ok", title: "Verified on the integration tip", detail: str(d.reason) || null };
    case "stack.verification_started":
      return { ...base, tone: "info", title: "Verification started", detail: null };
    case "stack.verification_failed":
      return { ...base, tone: "fail", title: "Verification failed on the integration tip", detail: checks(list(d.failed_checks)) };
    case "stack.verification_failed_on_base":
      return { ...base, tone: "fail", title: "Verification fails on the base; nothing ejected", detail: str(d.reason) || null };
    case "stack.integration_merged":
      return { ...base, tone: "ok", title: `Merged ${str(d.task_key)} into ${str(d.repository)}`, detail: null };
    case "stack.integration_conflict": {
      const paths = list(d.paths);
      return {
        ...base,
        tone: "warn",
        title: `Conflict: ${str(d.task_key)} in ${str(d.repository)}`,
        detail: paths.length ? paths.join(", ") : null,
      };
    }
    case "stack.item_state_changed":
      return {
        ...base,
        tone: d.to === "ejected" || d.to === "conflict" ? "warn" : "info",
        title: `${str(d.task_key)}: ${label(str(d.from))} → ${label(str(d.to))}`,
        detail: null,
      };
    case "stack.item_added":
      return { ...base, tone: "info", title: `${str(d.task_key)} added to the stack`, detail: null };
    case "stack.item_removed":
      return { ...base, tone: "info", title: `${str(d.task_key)} removed from the stack`, detail: null };
    case "stack.created":
      return { ...base, tone: "info", title: "Stack created", detail: str(d.integration_branch) || null };
    case "stack.rebuilt":
      return { ...base, tone: "info", title: "Integration branch rebuilt from base", detail: null };
    case "stack.status_changed":
      return { ...base, tone: "info", title: `Stack ${label(str(d.from))} → ${label(str(d.to))}`, detail: str(d.reason) || null };
    case "stack.landed":
      return { ...base, tone: "ok", title: "Landed on the default branch", detail: null };
    case "stack.landing_refused":
      return { ...base, tone: "fail", title: "Landing refused", detail: str(d.error) || null };
    case "stack.land_requested":
      return { ...base, tone: "warn", title: "Landing deferred until the default branch's stack is rebuilt", detail: null };
    case "stack.item_released":
      return { ...base, tone: "info", title: `${str(d.task_key)} accepted on its own and released`, detail: null };
    case "stack.parent_rejected":
    case "stack.parent_not_reviewable":
      return { ...base, tone: "warn", title: label(event.event_type.split(".")[1]), detail: str(d.reason) || null };
    default:
      return { ...base, tone: "info", title: label(event.event_type), detail: null };
  }
}
