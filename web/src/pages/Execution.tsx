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
  List,
  ListRow,
  Meta as MetaLine,
  Modal,
  Notice,
  PageHeader,
  SectionTitle,
  Segmented,
  Skeleton,
  Stat,
  StatusBadge,
  inputClass,
  selectClass,
  textareaClass,
} from "../components/ui";
import { useTaskIndex } from "../hooks/lookups";
import { useDesktop } from "../hooks/media";
import { useWorkspace } from "../hooks/workspace";
import { ago, label, ms, num, sentence, short } from "../lib/format";
import { toneOf } from "../lib/tone";

const ACTIVE = ["queued", "preparing", "running", "verifying"];

export function ExecutionPage() {
  const { ws } = useWorkspace();
  const desktop = useDesktop();
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
  const select = (id: string | null) => setParams(id ? { attempt: id } : {}, { replace: !!selected && !!id });

  return (
    <div>
      <PageHeader
        title="Agent runs"
        actions={
          <Button variant="primary" icon="plus" onClick={() => setStarting(true)}>
            <span className="sm:hidden">Start</span>
            <span className="hidden sm:inline">Start run</span>
          </Button>
        }
      />
      <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
        <Stat label="Running" value={runningCount} tone="active" />
        <Stat label="Queued" value={queuedCount} tone="ready" />
        <Stat label="Live workers" value={liveWorkers.length} tone={liveWorkers.length ? "done" : "idle"} hint={liveWorkers.length ? undefined : "Start one: workgraph worker run"} />
        <Stat label="Queued jobs" value={jobs.data?.stats.queued ?? 0} hint={`${jobs.data?.stats.dead ?? 0} dead, all kinds`} />
      </div>

      <div className="mt-5 grid gap-4 lg:grid-cols-[minmax(0,3fr)_minmax(0,2fr)] lg:items-start">
        <section className="min-w-0">
          <SectionTitle
            actions={
              <Segmented
                label="Attempt scope"
                value={scope}
                onChange={setScope}
                options={[
                  ["all", "All"],
                  ["active", "Active"],
                ]}
              />
            }
          >
            Attempts
          </SectionTitle>
          <ErrorBanner error={attempts.error} />
          {!attempts.data ? (
            <Card bodyClass="p-0">
              <Skeleton rows={6} />
            </Card>
          ) : attempts.data.items.length === 0 ? (
            <Card>
              <Empty title={scope === "active" ? "Nothing running" : "No attempts"}>Agent runs and attempts reported from elsewhere appear here.</Empty>
            </Card>
          ) : (
            <List testId="attempts">
              {attempts.data.items.map((a) => (
                <ListRow key={a.id} tone={toneOf(a.status)} selected={selected === a.id} onClick={() => select(a.id)}>
                  <div className="flex items-center gap-2">
                    <span className="mono text-[13px] font-medium">{index.get(a.task_id)?.key ?? short(a.task_id)}</span>
                    <span className="text-[13px] text-ink-3">
                      #{a.number}
                      {a.parent_attempt_id && ", subagent"}
                    </span>
                    <StatusBadge status={a.status} />
                    <span className="ml-auto whitespace-nowrap text-[13px] text-ink-3">{ago(a.created_at)}</span>
                  </div>
                  {index.get(a.task_id)?.title && <div className="mt-1 line-clamp-1 text-[14px] text-ink-2">{index.get(a.task_id)?.title}</div>}
                  <MetaLine>
                    <span>{a.provider}</span>
                    <span>Ran {ms(a.duration_ms)}</span>
                    <span>Queued {ms(a.queue_wait_ms)}</span>
                    {a.exit_reason && <span>{sentence(a.exit_reason)}</span>}
                  </MetaLine>
                </ListRow>
              ))}
            </List>
          )}
        </section>

        <div className="min-w-0 space-y-4">
          {desktop &&
            (selected ? (
              <AttemptPanel attemptId={selected} />
            ) : (
              <Card title="Attempt">
                <Empty title="Select an attempt">Its details, live progress and log appear here.</Empty>
              </Card>
            ))}
          <section>
            <SectionTitle>Workers</SectionTitle>
            {workers.data?.length ? (
              <List>
                {workers.data.slice(0, 8).map((w) => (
                  <ListRow key={w.id} tone={w.status === "running" ? "done" : "idle"}>
                    <div className="flex items-center gap-2">
                      <span className="mono min-w-0 truncate text-[13px]">{w.id}</span>
                      <StatusBadge status={w.status === "running" ? "ok" : w.status} />
                    </div>
                    <MetaLine>
                      <span>
                        {w.current_jobs.length} of {w.concurrency} slots busy
                      </span>
                      <span>{w.jobs_completed} done</span>
                      <span>{w.jobs_failed} failed</span>
                      <span>Heartbeat {ago(w.heartbeat_at)}</span>
                    </MetaLine>
                  </ListRow>
                ))}
              </List>
            ) : (
              <Card>
                <Empty title="No workers have run">
                  Background jobs need <span className="mono">workgraph worker run</span> or <span className="mono">workgraph serve</span>.
                </Empty>
              </Card>
            )}
          </section>
        </div>
      </div>

      {!desktop && (
        <Modal title="Attempt" open={!!selected} onClose={() => select(null)} wide>
          {selected && <AttemptPanel attemptId={selected} bare />}
        </Modal>
      )}
      <StartRunDialog open={starting} onClose={() => setStarting(false)} onStarted={(id) => select(id)} />
    </div>
  );
}

function AttemptPanel({ attemptId, bare }: { attemptId: string; bare?: boolean }) {
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
  if (!run.data) {
    const body = run.error ? <ErrorBanner error={run.error} /> : <Skeleton />;
    return bare ? body : <Card title="Attempt">{body}</Card>;
  }
  const a = run.data.attempt;
  const progress = run.data.recent_events.filter((e) => e.event_type.startsWith("attempt."));
  const buttons = (
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
  );
  const body = (
    <div className="space-y-5">
      {bare && (
        <div className="flex flex-wrap items-center gap-2">
          <span className="font-semibold">Attempt {a.number}</span>
          <span className="mono text-[13px] text-ink-3">{short(a.id)}</span>
          <div className="ml-auto flex gap-1.5">{buttons}</div>
        </div>
      )}
      <ErrorBanner error={act.error} />
      <KeyValue
        items={[
          ["status", <StatusBadge status={a.status} />],
          ["job", run.data.job_status ? sentence(run.data.job_status) : "—"],
          ["agent", `${a.provider} (${a.adapter})`],
          ["model", a.model],
          ["session", a.session_id ? <span className="mono text-[12px]">{a.session_id}</span> : null],
          ["duration", ms(a.duration_ms)],
          ["queue wait", ms(a.queue_wait_ms)],
          ["usage", Object.keys(a.usage).length ? `${num(a.usage.input_tokens)} in, ${num(a.usage.output_tokens)} out (${a.usage_source})` : "Not measured"],
          ["cost", a.cost_usd !== null ? `$${a.cost_usd.toFixed(4)}` : "Not measured"],
          ["exit", label(a.exit_reason)],
          ["error", a.error ? <span className="text-danger">{a.error}</span> : null],
          ["summary", a.summary],
        ]}
      />
      <div>
        <div className="mb-2 text-[14px] font-semibold">Progress</div>
        <ol className="max-h-56 space-y-1.5 overflow-y-auto text-[13px]" data-testid="attempt-progress">
          {progress.length ? (
            progress.map((e) => (
              <li key={e.id} className="flex gap-3">
                <span className="mono w-20 shrink-0 truncate text-[12px] text-ink-3">{e.event_type.replace("attempt.", "")}</span>
                <span className="min-w-0 break-words">{String(e.data.message ?? e.data.to ?? e.data.status ?? "")}</span>
              </li>
            ))
          ) : (
            <li className="text-ink-3">No progress events</li>
          )}
        </ol>
      </div>
      <div>
        <div className="mb-2 flex items-baseline justify-between text-[14px] font-semibold">
          Agent log
          {log.data?.total_lines ? <span className="text-[12px] font-normal text-ink-3">{log.data.total_lines} lines</span> : null}
        </div>
        <pre className="mono max-h-80 overflow-auto rounded-xl bg-code p-3 text-[11.5px] leading-relaxed text-code-ink">
          {log.data?.available ? log.data.lines.join("\n") || "(empty)" : "(no log yet)"}
        </pre>
      </div>
      <Link to={`/tasks/${a.task_id}`} className="inline-flex min-h-10 items-center text-[14px] font-medium text-accent hover:underline">
        Open task
      </Link>
    </div>
  );
  return bare ? (
    body
  ) : (
    <Card title={`Attempt ${a.number}`} actions={buttons}>
      {body}
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
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          start.mutate();
        }}
      >
        {!meta.data?.execution_enabled && <Notice tone="warn">Agent execution is turned off. Turn it on under Settings, Execution and jobs.</Notice>}
        <Field label="Task key">
          <input className={`${inputClass} mono`} value={task} onChange={(e) => setTask(e.target.value)} required placeholder="WG-12" autoCapitalize="characters" />
        </Field>
        <Field label="Agent adapter">
          <select className={`${selectClass} w-full`} value={agent} onChange={(e) => setAgent(e.target.value)}>
            <option value="">Default</option>
            {meta.data?.agents.map((a) => (
              <option key={a}>{a}</option>
            ))}
          </select>
        </Field>
        <Field label="Additional instructions" hint="Optional">
          <textarea className={`${textareaClass} h-24`} value={instructions} onChange={(e) => setInstructions(e.target.value)} />
        </Field>
        <ErrorBanner error={start.error} />
        <Button type="submit" variant="primary" className="w-full md:ml-auto md:flex md:w-auto" busy={start.isPending}>
          Queue run
        </Button>
      </form>
    </Modal>
  );
}
