import { Link } from "react-router-dom";
import type { EventOut, StackItemState, StackStatus } from "../api/types";
import { Empty, ErrorBanner, Skeleton } from "../components/ui";
import { useStack, useStackHistory, useTaskStacks } from "../hooks/stacks";
import { useWorkspace } from "../hooks/workspace";
import { label } from "../lib/format";
import { describeStackEvent, type TimelineEntry, type Tone } from "../lib/stacks";

const STACK_STATUS_COLORS: Record<StackStatus, string> = {
  open: "bg-slate-100 text-slate-700",
  integrating: "bg-indigo-100 text-indigo-800",
  verifying: "bg-cyan-100 text-cyan-800",
  verified: "bg-teal-100 text-teal-800",
  landed: "bg-emerald-100 text-emerald-800",
  abandoned: "bg-slate-200 text-slate-500 line-through",
};

const ITEM_STATE_COLORS: Record<StackItemState, string> = {
  pending: "bg-slate-100 text-slate-600",
  ready: "bg-sky-100 text-sky-800",
  merged: "bg-indigo-100 text-indigo-800",
  conflict: "bg-amber-100 text-amber-800",
  ejected: "bg-rose-100 text-rose-800",
  verified: "bg-emerald-100 text-emerald-800",
};

const TONE_DOT: Record<Tone, string> = {
  ok: "bg-emerald-500",
  fail: "bg-rose-500",
  warn: "bg-amber-500",
  info: "bg-slate-400",
};

const TONE_TEXT: Record<Tone, string> = {
  ok: "text-emerald-800",
  fail: "text-rose-800",
  warn: "text-amber-800",
  info: "text-slate-700",
};

export function StackStatusBadge({ status }: { status: StackStatus }) {
  return (
    <span
      data-testid="stack-status"
      className={`inline-block whitespace-nowrap rounded px-1.5 py-0.5 text-[11px] font-medium ${STACK_STATUS_COLORS[status]}`}
    >
      {label(status)}
    </span>
  );
}

export function ItemStateBadge({ state, count }: { state: StackItemState; count?: number }) {
  return (
    <span
      data-testid={`state-${state}`}
      className={`inline-block whitespace-nowrap rounded px-1.5 py-0.5 text-[11px] font-medium ${ITEM_STATE_COLORS[state]}`}
    >
      {count === undefined ? label(state) : `${count} ${label(state)}`}
    </span>
  );
}

/** Stack-level links on a task: the stacks it is an item of, and the stack it parents. */
export function TaskStackLinks({ taskKey }: { taskKey: string }) {
  const { ws } = useWorkspace();
  const memberOf = useTaskStacks(ws, taskKey);
  const parentOf = useStack(ws, taskKey);
  const items = memberOf.data ?? [];
  if (!parentOf.data && items.length === 0) return null;
  return (
    <div className="mt-1.5 flex flex-wrap items-center gap-2 text-[12px] text-slate-600" data-testid="task-stack-links">
      {parentOf.data && (
        <Link className="font-medium text-sky-700 hover:underline" to={`/stacks/${parentOf.data.parent_key}`}>
          Stack for {parentOf.data.parent_key} <StackStatusBadge status={parentOf.data.status} />
        </Link>
      )}
      {items.map((stack) => (
        <Link key={stack.id} className="font-medium text-sky-700 hover:underline" to={`/stacks/${stack.parent_key}`}>
          In stack {stack.parent_key} <StackStatusBadge status={stack.status} />
        </Link>
      ))}
    </div>
  );
}

/** The stack's events in order, with verification runs, bisect probes and the culprit called out. */
export function StackTimeline({ ws, ref }: { ws: string; ref: string }) {
  const history = useStackHistory(ws, ref);
  if (history.error) return <ErrorBanner error={history.error} />;
  if (!history.data) return <Skeleton />;
  if (!history.data.length) return <Empty title="No events yet" />;
  const rows = history.data.map((event: EventOut) => ({ event, entry: describeStackEvent(event) }));
  return (
    <ol className="relative space-y-3 border-l border-slate-200 pl-4" data-testid="stack-timeline">
      {rows.map(({ event, entry }) => (
        <TimelineRow key={event.seq} event={event} entry={entry} />
      ))}
    </ol>
  );
}

function TimelineRow({ event, entry }: { event: EventOut; entry: TimelineEntry }) {
  return (
    <li
      className={`relative ${entry.culprit ? "rounded-md bg-rose-50 px-2 py-1 ring-1 ring-rose-200" : ""}`}
      data-testid={entry.run ? `timeline-${entry.run.kind}-${entry.run.passed ? "passed" : "failed"}` : undefined}
      data-culprit={entry.culprit ? "true" : undefined}
    >
      <span className={`absolute -left-[21px] top-1.5 h-2 w-2 rounded-full ${TONE_DOT[entry.tone]}`} />
      <div className={`flex flex-wrap items-baseline gap-2 text-[13px] font-medium ${TONE_TEXT[entry.tone]}`}>
        <span>{entry.title}</span>
        {entry.culprit && <span className="text-[11px] font-semibold uppercase text-rose-700">culprit</span>}
      </div>
      <div className="flex flex-wrap items-baseline gap-2 text-[12px] text-slate-500">
        <span className="mono">{event.event_type}</span>
        {entry.detail && <span>{entry.detail}</span>}
        <span className="mono text-slate-400">#{event.seq}</span>
      </div>
    </li>
  );
}
