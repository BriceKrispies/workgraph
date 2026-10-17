import { useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { get, post } from "../api/client";
import type { Attempt, Job, Meta, Page, RunOut, Worker } from "../api/types";
import {
  Button,
  Card,
  Empty,
  ErrorBanner,
  Field,
  KeyValue,
  Modal,
  Skeleton,
  Stat,
  StatusBadge,
  Table,
  inputClass,
  td,
} from "../components/ui";
import { useTaskIndex } from "../hooks/lookups";
import { useWorkspace } from "../hooks/workspace";
import { ago, label, ms, num, short } from "../lib/format";

const ACTIVE = ["queued", "preparing", "running", "verifying"];

export function ExecutionPage() {
  const { ws } = useWorkspace();
  const [params, setParams] = useSearchParams();
  const selected = params.get("attempt");
  const [scope, setScope] = useState<"active" | "all">("all");
  const [starting, setStarting] = useState(false);
  const index = useTaskIndex(ws);

  const attempts = useQuery({
    queryKey: ["attempts", ws, scope],
    queryFn: () => get<Page<Attempt>>(`/ws/${ws}/attempts`, { status: scope === "active" ? ACTIVE : undefined, limit: 100 }),
    refetchInterval: (q) => (q.state.data?.items.some((a) => ACTIVE.includes(a.status)) ? 3000 : false),
  });
  const workers = useQuery({ queryKey: ["workers"], queryFn: () => get<Worker[]>("/workers"), refetchInterval: 10000 });
  const jobs = useQuery({
    queryKey: ["jobs", ws, "agent_run"],
    queryFn: () => get<{ items: Job[]; total: number; stats: Record<string, number> }>("/jobs", { workspace: ws, kind: "agent_run", limit: 20 }),
  });
  const runningCount = attempts.data?.items.filter((a) => a.status === "running" || a.status === "verifying").length ?? 0;
  const queuedCount = attempts.data?.items.filter((a) => a.status === "queued" || a.status === "preparing").length ?? 0;
  const liveWorkers = workers.data?.filter((w) => w.status === "running") ?? [];

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold text-slate-900">Agent execution</h1>
        <Button variant="primary" onClick={() => setStarting(true)}>
          Start run
        </Button>
      </div>
      <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
        <Stat label="Running" value={runningCount} />
        <Stat label="Queued" value={queuedCount} />
        <Stat label="Live workers" value={liveWorkers.length} hint={liveWorkers.length ? undefined : "start one: workgraph worker run"} />
        <Stat label="Queued jobs (all kinds)" value={jobs.data?.stats.queued ?? 0} hint={`${jobs.data?.stats.dead ?? 0} dead`} />
      </div>
      <div className="grid gap-4 xl:grid-cols-5">
        <Card
          title="Attempts"
          className="xl:col-span-3"
          bodyClass="p-0"
          actions={
            <select aria-label="Attempt scope" className="rounded border border-slate-300 px-1.5 py-0.5 text-[12px]" value={scope} onChange={(e) => setScope(e.target.value as "active" | "all")}>
              <option value="all">all</option>
              <option value="active">active</option>
            </select>
          }
        >
          <ErrorBanner error={attempts.error} />
          {!attempts.data ? (
            <Skeleton rows={6} />
          ) : attempts.data.items.length === 0 ? (
            <div className="p-3">
              <Empty title="No attempts">Agent runs and externally reported attempts appear here.</Empty>
            </div>
          ) : (
            <Table head={["Task", "#", "Status", "Agent", "Duration", "Queue", "Exit", "When"]} testId="attempts">
              {attempts.data.items.map((a) => (
                <tr
                  key={a.id}
                  className={`cursor-pointer hover:bg-slate-50 ${selected === a.id ? "bg-sky-50" : ""}`}
                  onClick={() => setParams({ attempt: a.id })}
                >
                  <td className={`${td} mono text-[12px]`}>{index.get(a.task_id)?.key ?? short(a.task_id)}</td>
                  <td className={td}>
                    {a.number}
                    {a.parent_attempt_id && <span className="ml-1 text-[11px] text-slate-400">sub</span>}
                  </td>
                  <td className={td}>
                    <StatusBadge status={a.status} />
                  </td>
                  <td className={td}>{a.provider}</td>
                  <td className={td}>{ms(a.duration_ms)}</td>
                  <td className={td}>{ms(a.queue_wait_ms)}</td>
                  <td className={`${td} text-[12px] text-slate-600`}>{label(a.exit_reason)}</td>
                  <td className={`${td} whitespace-nowrap text-slate-500`}>{ago(a.created_at)}</td>
                </tr>
              ))}
            </Table>
          )}
        </Card>
        <div className="space-y-4 xl:col-span-2">
          {selected ? <AttemptPanel attemptId={selected} /> : <Card title="Attempt"><Empty title="Select an attempt">Details, live progress and logs appear here.</Empty></Card>}
          <Card title="Workers" bodyClass="p-0">
            {workers.data?.length ? (
              <Table head={["Worker", "Status", "Running", "Done", "Failed", "Heartbeat"]}>
                {workers.data.slice(0, 8).map((w) => (
                  <tr key={w.id}>
                    <td className={`${td} mono text-[11px]`}>{w.id}</td>
                    <td className={td}>
                      <StatusBadge status={w.status === "running" ? "ok" : w.status} />
                    </td>
                    <td className={td}>
                      {w.current_jobs.length}/{w.concurrency}
                    </td>
                    <td className={td}>{w.jobs_completed}</td>
                    <td className={td}>{w.jobs_failed}</td>
                    <td className={`${td} text-slate-500`}>{ago(w.heartbeat_at)}</td>
                  </tr>
                ))}
              </Table>
            ) : (
              <div className="p-3">
                <Empty title="No workers have run">Background jobs need `workgraph worker run` or `workgraph serve`.</Empty>
              </div>
            )}
          </Card>
        </div>
      </div>
      <StartRunDialog open={starting} onClose={() => setStarting(false)} onStarted={(id) => setParams({ attempt: id })} />
    </div>
  );
}

function AttemptPanel({ attemptId }: { attemptId: string }) {
  const client = useQueryClient();
  const run = useQuery({
    queryKey: ["run", attemptId],
    queryFn: () => get<RunOut>(`/attempts/${attemptId}`, { events: 200 }),
    refetchInterval: (q) => (q.state.data && ACTIVE.includes(q.state.data.attempt.status) ? 2000 : false),
  });
  const active = !!run.data && ACTIVE.includes(run.data.attempt.status);
  const log = useQuery({
    queryKey: ["run", attemptId, "log"],
    queryFn: () => get<{ lines: string[]; available: boolean; total_lines?: number }>(`/attempts/${attemptId}/log`, { tail: 300 }),
    refetchInterval: active ? 2000 : false,
  });
  const act = useMutation({
    mutationFn: (kind: "cancel" | "retry" | "rerun") => post<unknown>(`/attempts/${attemptId}/${kind}`, kind === "cancel" ? { reason: "cancelled from the web UI" } : {}),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: ["attempts"] });
      void client.invalidateQueries({ queryKey: ["run"] });
    },
  });
  if (!run.data) return <Card title="Attempt">{run.error ? <ErrorBanner error={run.error} /> : <Skeleton />}</Card>;
  const a = run.data.attempt;
  const progress = run.data.recent_events.filter((e) => e.event_type.startsWith("attempt."));
  return (
    <Card
      title={`Attempt #${a.number} · ${short(a.id)}`}
      actions={
        <>
          {active && (
            <Button size="sm" variant="danger" busy={act.isPending} onClick={() => act.mutate("cancel")}>
              Cancel
            </Button>
          )}
          {!active && a.status !== "succeeded" && (
            <Button size="sm" busy={act.isPending} onClick={() => act.mutate("retry")}>
              Retry
            </Button>
          )}
          {!active && (
            <Button size="sm" variant="ghost" busy={act.isPending} onClick={() => act.mutate("rerun")}>
              Rerun
            </Button>
          )}
        </>
      }
    >
      <ErrorBanner error={act.error} />
      <KeyValue
        items={[
          ["status", <StatusBadge status={a.status} />],
          ["job", run.data.job_status ?? "—"],
          ["agent", `${a.provider} (${a.adapter})`],
          ["model", a.model],
          ["session", a.session_id],
          ["duration", ms(a.duration_ms)],
          ["queue wait", ms(a.queue_wait_ms)],
          ["usage", Object.keys(a.usage).length ? `${num(a.usage.input_tokens)} in / ${num(a.usage.output_tokens)} out (${a.usage_source})` : "not measured"],
          ["cost", a.cost_usd !== null ? `$${a.cost_usd.toFixed(4)}` : "not measured"],
          ["exit", label(a.exit_reason)],
          ["error", a.error ? <span className="text-rose-700">{a.error}</span> : null],
          ["summary", a.summary],
        ]}
      />
      <div className="mt-3">
        <div className="mb-1 text-[12px] font-semibold uppercase tracking-wide text-slate-500">Progress</div>
        <ol className="max-h-48 space-y-0.5 overflow-y-auto text-[12px]" data-testid="attempt-progress">
          {progress.length ? (
            progress.map((e) => (
              <li key={e.id} className="flex gap-2">
                <span className="mono shrink-0 text-slate-400">{e.event_type.replace("attempt.", "")}</span>
                <span className="truncate">{String(e.data.message ?? e.data.to ?? e.data.status ?? "")}</span>
              </li>
            ))
          ) : (
            <li className="text-slate-400">no progress events</li>
          )}
        </ol>
      </div>
      <div className="mt-3">
        <div className="mb-1 text-[12px] font-semibold uppercase tracking-wide text-slate-500">
          Agent log {log.data?.total_lines ? `(${log.data.total_lines} lines)` : ""}
        </div>
        <pre className="mono max-h-64 overflow-auto rounded bg-slate-900 p-2 text-[11px] text-slate-100">
          {log.data?.available ? log.data.lines.join("\n") || "(empty)" : "(no log yet)"}
        </pre>
      </div>
      <Link to={`/tasks/${a.task_id}`} className="mt-2 inline-block text-[12px] text-sky-700 hover:underline">
        open task →
      </Link>
    </Card>
  );
}

function StartRunDialog({ open, onClose, onStarted }: { open: boolean; onClose: () => void; onStarted: (id: string) => void }) {
  const { ws } = useWorkspace();
  const client = useQueryClient();
  const meta = useQuery({ queryKey: ["meta"], queryFn: () => get<Meta>("/meta") });
  const [task, setTask] = useState("");
  const [agent, setAgent] = useState("");
  const [instructions, setInstructions] = useState("");
  const start = useMutation({
    mutationFn: () => post<Attempt>(`/ws/${ws}/runs`, { task, agent: agent || null, instructions: instructions || null }),
    onSuccess: (attempt) => {
      void client.invalidateQueries({ queryKey: ["attempts"] });
      onStarted(attempt.id);
      onClose();
    },
  });
  return (
    <Modal title="Start agent run" open={open} onClose={onClose}>
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          start.mutate();
        }}
      >
        {!meta.data?.execution_enabled && (
          <div className="rounded bg-amber-50 px-2 py-1 text-[12px] text-amber-800">
            Agent execution is disabled. Enable it under Settings (execution.enabled).
          </div>
        )}
        <Field label="Task key">
          <input className={`${inputClass} mono`} value={task} onChange={(e) => setTask(e.target.value)} required placeholder="WG-12" />
        </Field>
        <Field label="Agent adapter">
          <select className={inputClass} value={agent} onChange={(e) => setAgent(e.target.value)}>
            <option value="">default</option>
            {meta.data?.agents.map((a) => (
              <option key={a}>{a}</option>
            ))}
          </select>
        </Field>
        <Field label="Additional instructions (optional)">
          <textarea className={`${inputClass} h-20`} value={instructions} onChange={(e) => setInstructions(e.target.value)} />
        </Field>
        <ErrorBanner error={start.error} />
        <div className="flex justify-end">
          <Button type="submit" variant="primary" busy={start.isPending}>
            Queue run
          </Button>
        </div>
      </form>
    </Modal>
  );
}
