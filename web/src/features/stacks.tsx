import { Link } from "react-router-dom";
import type { EventOut, StackItemState, StackStatus } from "../api/types";
import { Empty, ErrorBanner, Skeleton } from "../components/ui";
import { useStack, useStackHistory, useTaskStacks } from "../hooks/stacks";
import { useWorkspace } from "../hooks/workspace";
import { label } from "../lib/format";
import { describeStackEvent, type TimelineEntry, type Tone } from "../lib/stacks";
import { STRUCK, toneOf } from "../lib/tone";

const TONE_CLASS: Record<Tone, string> = {
  ok: "tone-done",
  fail: "tone-failed",
  warn: "tone-blocked",
  info: "tone-idle",
};

export function StackStatusBadge({ status }: { status: StackStatus }) {
  return (
    <span data-testid="stack-status" className={`badge tone-${toneOf(status)} ${STRUCK.has(status) ? "is-struck" : ""}`}>
      {label(status)}
    </span>
  );
}

export function ItemStateBadge({ state, count }: { state: StackItemState; count?: number }) {
  return (
    <span data-testid={`state-${state}`} className={`badge tone-${toneOf(state)} ${count === 0 ? "opacity-50" : ""}`}>
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
    <div className="mt-3 flex flex-wrap items-center gap-2 text-[14px]" data-testid="task-stack-links">
      {parentOf.data && (
        <Link className="inline-flex min-h-9 items-center gap-2 rounded-full border border-line bg-surface px-3 font-medium text-ink hover:border-accent" to={`/stacks/${parentOf.data.parent_key}`}>
          Stack for {parentOf.data.parent_key} <StackStatusBadge status={parentOf.data.status} />
        </Link>
      )}
      {items.map((stack) => (
        <Link key={stack.id} className="inline-flex min-h-9 items-center gap-2 rounded-full border border-line bg-surface px-3 font-medium text-ink hover:border-accent" to={`/stacks/${stack.parent_key}`}>
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
    <ol className="relative ml-1 space-y-4 border-l-2 border-line pl-5" data-testid="stack-timeline">
      {rows.map(({ event, entry }) => (
        <TimelineRow key={event.seq} event={event} entry={entry} />
      ))}
    </ol>
  );
}

function TimelineRow({ event, entry }: { event: EventOut; entry: TimelineEntry }) {
  return (
    <li
      className={`relative ${entry.culprit ? "-ml-2 rounded-xl bg-danger-soft px-2 py-2" : ""}`}
      data-testid={entry.run ? `timeline-${entry.run.kind}-${entry.run.passed ? "passed" : "failed"}` : undefined}
      data-culprit={entry.culprit ? "true" : undefined}
    >
      <span className={`dot absolute top-1.5 ring-4 ring-canvas ${entry.culprit ? "-left-[18px]" : "-left-[26px]"} ${TONE_CLASS[entry.tone]}`} />
      <div className={`flex flex-wrap items-baseline gap-2 text-[14px] font-medium ${entry.tone === "info" ? "text-ink" : TONE_CLASS[entry.tone]}`} style={entry.tone === "info" ? undefined : { color: "var(--t-ink)" }}>
        <span>{entry.title}</span>
        {entry.culprit && <span className="text-[12px] font-semibold text-danger">Culprit</span>}
      </div>
      <div className="mt-0.5 flex flex-wrap items-baseline gap-x-3 gap-y-0.5 text-[13px] text-ink-3">
        {entry.detail && <span className="text-ink-2">{entry.detail}</span>}
        <span className="mono text-[12px]">{event.event_type}</span>
        <span className="mono text-[12px]">#{event.seq}</span>
      </div>
    </li>
  );
}
