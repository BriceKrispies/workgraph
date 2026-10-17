import { useState } from "react";
import { Link } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { get, post } from "../api/client";
import {
  CATEGORIES,
  type Attempt,
  type EventOut,
  type Page,
  type TaskChanges,
  type TaskClassifications,
  type VerificationRun,
} from "../api/types";
import { Button, Card, Empty, ErrorBanner, KeyValue, Skeleton, StatusBadge, Table, inputClass, td } from "../components/ui";
import { describeEvent } from "../pages/Overview";
import { ago, dateTime, label, ms, num, pct, short } from "../lib/format";

export function HistoryPanel({ ws, taskKey }: { ws: string; taskKey: string }) {
  const history = useQuery({
    queryKey: ["task", ws, taskKey, "history"],
    queryFn: () => get<EventOut[]>(`/ws/${ws}/tasks/${taskKey}/history`),
  });
  if (history.error) return <ErrorBanner error={history.error} />;
  if (!history.data) return <Skeleton />;
  if (!history.data.length) return <Empty title="No history" />;
  return (
    <ol className="relative space-y-2 border-l border-slate-200 pl-4" data-testid="history">
      {[...history.data].reverse().map((e) => (
        <li key={e.id} className="relative">
          <span className="absolute -left-[21px] top-1.5 h-2 w-2 rounded-full bg-slate-400" />
          <div className="flex flex-wrap items-baseline gap-2">
            <span className="mono text-[12px] text-slate-800">{e.event_type}</span>
            <span className="text-[12px] text-slate-500">{e.actor}</span>
            <span className="text-[11px] text-slate-400" title={dateTime(e.occurred_at)}>
              {ago(e.occurred_at)}
            </span>
          </div>
          <div className="text-[12px] text-slate-600">{describeEvent(e)}</div>
        </li>
      ))}
    </ol>
  );
}

export function ChangesPanel({ ws, taskKey, repoNames }: { ws: string; taskKey: string; repoNames: Map<string, string> }) {
  const changes = useQuery({
    queryKey: ["task", ws, taskKey, "changes"],
    queryFn: () => get<TaskChanges>(`/ws/${ws}/tasks/${taskKey}/changes`, { min_confidence: 0 }),
  });
  const [showDiff, setShowDiff] = useState(false);
  const diff = useQuery({
    queryKey: ["task", ws, taskKey, "diff"],
    queryFn: () =>
      get<{ parts: { repository: string; kind: string; range?: string; sha?: string; diff: string }[]; missing_objects: unknown[]; truncated: boolean }>(
        `/ws/${ws}/tasks/${taskKey}/diff`,
        { max_bytes: 200000 },
      ),
    enabled: showDiff,
  });
  if (changes.error) return <ErrorBanner error={changes.error} />;
  if (!changes.data) return <Skeleton />;
  const { commits, cumulative_churn: churn, final_change_surface: surface, definitions } = changes.data;
  return (
    <div className="space-y-3">
      <div className="grid gap-3 md:grid-cols-2">
        <Card title="Cumulative churn">
          <KeyValue
            items={[
              ["commits", churn.commits],
              ["lines added", `+${churn.insertions}`],
              ["lines removed", `-${churn.deletions}`],
              ["files touched", churn.files_touched],
              ["repeated file edits", churn.repeated_file_edits],
              ["functions affected", churn.functions_affected],
            ]}
          />
          <p className="mt-2 text-[11px] text-slate-400">{definitions?.cumulative_churn}</p>
        </Card>
        <Card title="Final change surface">
          {surface.length ? (
            surface.map((s) => (
              <div key={s.repository_id} className="mb-2">
                <div className="mb-1 font-medium">{repoNames.get(s.repository_id) ?? short(s.repository_id)}</div>
                <KeyValue
                  items={[
                    ["range", <span className="mono text-[12px]">{`${short(s.base_sha, 10)}..${short(s.final_sha, 10)}`}</span>],
                    ["files", s.files_changed],
                    ["lines", `+${s.insertions} / -${s.deletions}`],
                    ["renames / binary", `${s.renames} / ${s.binary_files}`],
                    ["functions / types", `${s.functions_affected} / ${s.types_affected}`],
                  ]}
                />
              </div>
            ))
          ) : (
            <Empty title="Not computed">Computed when the task is accepted.</Empty>
          )}
          <p className="mt-2 text-[11px] text-slate-400">{definitions?.final_change_surface}</p>
        </Card>
      </div>
      <Card title={`Linked commits (${commits.length})`} bodyClass="p-0" actions={<Button size="sm" onClick={() => setShowDiff((v) => !v)}>{showDiff ? "hide diff" : "show diff"}</Button>}>
        {commits.length ? (
          <Table head={["Repository", "SHA", "Subject", "Lines", "Attribution", "When"]} testId="task-commits">
            {commits.map((c) => (
              <tr key={c.commit_id}>
                <td className={td}>
                  <Link className="text-sky-700 hover:underline" to={`/repositories/${c.repository}?sha=${c.sha}`}>
                    {c.repository}
                  </Link>
                </td>
                <td className={`${td} mono text-[12px]`}>{short(c.sha, 10)}{!c.reachable && <span className="ml-1 text-amber-600">(unreachable)</span>}</td>
                <td className={td}>{c.subject}</td>
                <td className={`${td} whitespace-nowrap`}>
                  <span className="text-emerald-700">+{c.insertions}</span> <span className="text-rose-700">-{c.deletions}</span>
                </td>
                <td className={td}>
                  {c.links.map((l, i) => (
                    <div key={i} className="whitespace-nowrap text-[12px]" title={JSON.stringify(l.evidence)}>
                      {label(l.source)} <span className="text-slate-400">({l.confidence.toFixed(2)})</span>
                    </div>
                  ))}
                </td>
                <td className={`${td} whitespace-nowrap text-slate-500`}>{ago(c.committed_at)}</td>
              </tr>
            ))}
          </Table>
        ) : (
          <div className="p-3">
            <Empty title="No linked commits">Commits are attributed via Task-ID trailers, agent runs or explicit links.</Empty>
          </div>
        )}
      </Card>
      {showDiff && (
        <Card title="Diff (read live from Git)">
          <ErrorBanner error={diff.error} />
          {!diff.data && !diff.error && <Skeleton />}
          {diff.data?.parts.map((part, i) => (
            <div key={i} className="mb-3">
              <div className="mb-1 text-[12px] font-medium">
                {part.repository} {part.range ?? short(part.sha, 10)}
              </div>
              <pre className="mono max-h-96 overflow-auto rounded bg-slate-900 p-2 text-[11px] text-slate-100">
                {part.diff.split("\n").map((line, n) => (
                  <div
                    key={n}
                    className={line.startsWith("+") && !line.startsWith("+++") ? "text-emerald-300" : line.startsWith("-") && !line.startsWith("---") ? "text-rose-300" : ""}
                  >
                    {line || " "}
                  </div>
                ))}
              </pre>
            </div>
          ))}
          {diff.data && diff.data.missing_objects.length > 0 && (
            <div className="text-[12px] text-amber-700">Some Git objects are no longer available: {JSON.stringify(diff.data.missing_objects)}</div>
          )}
        </Card>
      )}
    </div>
  );
}

export function AttemptsPanel({ ws, taskKey }: { ws: string; taskKey: string }) {
  const attempts = useQuery({
    queryKey: ["attempts", ws, taskKey],
    queryFn: () => get<Page<Attempt>>(`/ws/${ws}/tasks/${taskKey}/attempts`),
  });
  if (attempts.error) return <ErrorBanner error={attempts.error} />;
  if (!attempts.data) return <Skeleton />;
  if (!attempts.data.items.length) return <Empty title="No attempts yet">Start an agent run or record an attempt via MCP.</Empty>;
  return (
    <Table head={["#", "Status", "Agent", "Model", "Duration", "Queue", "Usage", "Exit", "Started"]} testId="task-attempts">
      {attempts.data.items.map((a) => (
        <tr key={a.id}>
          <td className={td}>
            <Link to={`/execution?attempt=${a.id}`} className="text-sky-700 hover:underline">
              #{a.number}
            </Link>
            {a.parent_attempt_id && <div className="text-[11px] text-slate-400">subagent</div>}
            {a.retry_of_attempt_id && <div className="text-[11px] text-slate-400">retry</div>}
          </td>
          <td className={td}>
            <StatusBadge status={a.status} />
          </td>
          <td className={td}>{a.provider}</td>
          <td className={`${td} text-slate-500`}>{a.model ?? "—"}</td>
          <td className={td}>{ms(a.duration_ms)}</td>
          <td className={td}>{ms(a.queue_wait_ms)}</td>
          <td className={`${td} text-[12px]`}>
            {Object.keys(a.usage).length ? (
              <span title={a.usage_source ?? ""}>
                {num(a.usage.input_tokens)} in / {num(a.usage.output_tokens)} out
              </span>
            ) : (
              <span className="text-slate-400">not measured</span>
            )}
          </td>
          <td className={`${td} text-[12px]`}>
            {label(a.exit_reason)}
            {a.error && <div className="max-w-xs truncate text-rose-700" title={a.error}>{a.error}</div>}
          </td>
          <td className={`${td} whitespace-nowrap text-slate-500`}>{ago(a.started_at)}</td>
        </tr>
      ))}
    </Table>
  );
}

export function VerificationPanel({ ws, taskKey }: { ws: string; taskKey: string }) {
  const client = useQueryClient();
  const runs = useQuery({
    queryKey: ["task", ws, taskKey, "verification"],
    queryFn: () => get<VerificationRun[]>(`/ws/${ws}/tasks/${taskKey}/verification`),
  });
  const checks = useQuery({
    queryKey: ["task", ws, taskKey, "checks"],
    queryFn: () => get<Record<string, unknown>[]>(`/ws/${ws}/tasks/${taskKey}/checks`),
  });
  const verify = useMutation({
    mutationFn: () => post<{ job_id: string }>(`/ws/${ws}/tasks/${taskKey}/verify`),
    onSuccess: () => client.invalidateQueries({ queryKey: ["task"] }),
  });
  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between">
        <div className="text-[12px] text-slate-500">
          {checks.data?.length ? `${checks.data.length} approved check(s) apply: ${checks.data.map((c) => c.name).join(", ")}` : "No verification configured — acceptance must be recorded manually."}
        </div>
        <Button size="sm" disabled={!checks.data?.length} busy={verify.isPending} onClick={() => verify.mutate()}>
          Run verification
        </Button>
      </div>
      <ErrorBanner error={runs.error ?? verify.error} />
      {verify.data && <div className="text-[12px] text-slate-500">Queued job {short(verify.data.job_id)} (needs a running worker).</div>}
      {!runs.data ? (
        <Skeleton />
      ) : !runs.data.length ? (
        <Empty title="No verification runs" />
      ) : (
        runs.data.map((run) => (
          <Card
            key={run.id}
            title={
              <span className="flex items-center gap-2 normal-case tracking-normal">
                <StatusBadge status={run.status} /> run {short(run.id)} · {ago(run.created_at)} · by {run.triggered_by}
              </span>
            }
            bodyClass="p-0"
          >
            <div className="px-3 py-1.5 text-[12px] text-slate-500">
              manifest: {run.manifest.map((m) => `${m.repository}@${short(m.sha ?? "", 10)}`).join(", ") || "—"}
            </div>
            <Table head={["Check", "Kind", "Required", "Status", "Exit", "Duration", "Failures"]}>
              {run.checks.map((c) => (
                <tr key={c.id}>
                  <td className={td}>
                    <details>
                      <summary className="cursor-pointer">{c.name}</summary>
                      <pre className="mono mt-1 max-h-48 overflow-auto rounded bg-slate-900 p-2 text-[11px] text-slate-100">{c.output_excerpt || "(no output)"}</pre>
                    </details>
                  </td>
                  <td className={td}>{c.kind}</td>
                  <td className={td}>{c.required ? "required" : "optional"}</td>
                  <td className={td}>
                    <StatusBadge status={c.status} />
                  </td>
                  <td className={td}>{c.exit_code ?? "—"}</td>
                  <td className={td}>{ms(c.duration_ms)}</td>
                  <td className={`${td} mono text-[11px] text-rose-700`}>{c.failure_signatures.join(", ")}</td>
                </tr>
              ))}
            </Table>
          </Card>
        ))
      )}
    </div>
  );
}

export function ClassificationPanel({ ws, taskKey }: { ws: string; taskKey: string }) {
  const client = useQueryClient();
  const data = useQuery({
    queryKey: ["task", ws, taskKey, "classification"],
    queryFn: () => get<TaskClassifications>(`/ws/${ws}/tasks/${taskKey}/classification`),
  });
  const [category, setCategory] = useState("");
  const [note, setNote] = useState("");
  const request = useMutation({
    mutationFn: () => post(`/ws/${ws}/tasks/${taskKey}/classification`),
    onSuccess: () => client.invalidateQueries({ queryKey: ["task"] }),
  });
  const confirm = useMutation({
    mutationFn: () => post(`/ws/${ws}/tasks/${taskKey}/classification/confirm`, { category, note }),
    onSuccess: () => {
      setNote("");
      void client.invalidateQueries({ queryKey: ["task"] });
      void client.invalidateQueries({ queryKey: ["tasks"] });
    },
  });
  if (data.error) return <ErrorBanner error={data.error} />;
  if (!data.data) return <Skeleton />;
  const current = data.data.current;
  return (
    <div className="space-y-3">
      <ErrorBanner error={request.error ?? confirm.error} />
      <div className="grid gap-3 md:grid-cols-2">
        <Card title="Current classification" actions={<Button size="sm" busy={request.isPending} onClick={() => request.mutate()}>Re-classify</Button>}>
          {current ? (
            <KeyValue
              items={[
                ["category", <strong>{label(current.category)}</strong>],
                ["subtype", current.subtype],
                ["domain", current.domain],
                ["complexity", current.complexity],
                ["confidence", current.confidence !== null ? pct(current.confidence) : null],
                ["source", current.source === "manual" ? "human-confirmed" : `${current.provider}/${current.model}`],
                ["prompt / taxonomy", `${current.prompt_version ?? "—"} / ${current.taxonomy_version}`],
                ["rationale", current.rationale],
              ]}
            />
          ) : (
            <Empty title="Not classified yet" />
          )}
          {request.isSuccess && <div className="mt-2 text-[12px] text-slate-500">Classification job queued.</div>}
          <p className="mt-2 text-[11px] text-slate-400">{data.data.note}</p>
        </Card>
        <Card title="Confirm manually">
          <form
            className="space-y-2"
            onSubmit={(e) => {
              e.preventDefault();
              confirm.mutate();
            }}
          >
            <select aria-label="Confirmed category" className={inputClass} value={category} onChange={(e) => setCategory(e.target.value)} required>
              <option value="">choose category…</option>
              {CATEGORIES.map((c) => (
                <option key={c} value={c}>
                  {label(c)}
                </option>
              ))}
            </select>
            <input className={inputClass} placeholder="note (optional)" value={note} onChange={(e) => setNote(e.target.value)} />
            <Button type="submit" variant="primary" size="sm" busy={confirm.isPending} disabled={!category}>
              Confirm classification
            </Button>
            <p className="text-[11px] text-slate-400">Human classifications are authoritative and join the gold evaluation set.</p>
          </form>
        </Card>
      </div>
      <Card title={`History${data.data.disagreement ? " — models disagree" : ""}`} bodyClass="p-0">
        <Table head={["When", "Source", "Model", "Category", "Confidence", "Validation", "Current"]}>
          {data.data.history.map((c) => (
            <tr key={c.id}>
              <td className={`${td} text-slate-500`}>{ago(c.created_at)}</td>
              <td className={td}>{c.source}</td>
              <td className={`${td} text-[12px]`}>{c.provider ? `${c.provider}/${c.model}` : "—"}</td>
              <td className={td}>{label(c.category)}</td>
              <td className={td}>{c.confidence !== null ? pct(c.confidence) : "—"}</td>
              <td className={`${td} text-[12px]`}>
                {c.validation_status}
                {c.error && <div className="text-rose-700">{c.error}</div>}
              </td>
              <td className={td}>{c.is_current ? "✓" : ""}</td>
            </tr>
          ))}
        </Table>
      </Card>
    </div>
  );
}

interface SimilarResult {
  method: string;
  similar: { task_id: string; key: string; title: string; status: string; category: string | null; score: number; repository_overlap: number; implementation: { sha: string; subject: string }[] }[];
}

interface Dist {
  n: number;
  p25: number | null;
  median: number | null;
  p75: number | null;
  p90: number | null;
}

interface EstimateResult {
  status: string;
  message?: string;
  sample_size?: number;
  method?: string;
  basis?: { key: string; title: string; score: number; reasons: string[] }[];
  comparable_tasks?: { key: string; title: string; score: number; reasons: string[] }[];
  distributions?: Record<string, Dist>;
  agent_recommendations?: { provider: string; model: string; attempts: number; success_rate: number | null; median_duration_ms: number | null }[];
  caveats?: string[];
  uncertainty?: string;
}

export function InsightsPanel({ ws, taskKey }: { ws: string; taskKey: string }) {
  const similar = useQuery({ queryKey: ["task", ws, taskKey, "similar"], queryFn: () => get<SimilarResult>(`/ws/${ws}/tasks/${taskKey}/similar`) });
  const estimate = useQuery({ queryKey: ["task", ws, taskKey, "estimate"], queryFn: () => get<EstimateResult>(`/ws/${ws}/tasks/${taskKey}/estimate`) });
  const fmt = (name: string, v: number | null) => (v === null ? "—" : name.endsWith("_ms") ? ms(v) : num(v));
  return (
    <div className="grid gap-3 lg:grid-cols-2">
      <Card title={`Similar tasks${similar.data ? ` (${similar.data.method})` : ""}`} bodyClass="p-0">
        <ErrorBanner error={similar.error} />
        {!similar.data ? (
          !similar.error && <Skeleton />
        ) : similar.data.similar.length ? (
          <Table head={["Task", "Score", "Status", "Implementation"]}>
            {similar.data.similar.map((s) => (
              <tr key={s.task_id}>
                <td className={td}>
                  <Link to={`/tasks/${s.key}`} className="mono text-[12px] text-sky-700 hover:underline">
                    {s.key}
                  </Link>{" "}
                  {s.title}
                </td>
                <td className={td}>{s.score.toFixed(2)}</td>
                <td className={td}>
                  <StatusBadge status={s.status} />
                </td>
                <td className={`${td} mono text-[11px]`}>{s.implementation.map((i) => short(i.sha, 8)).join(" ")}</td>
              </tr>
            ))}
          </Table>
        ) : (
          <div className="p-3">
            <Empty title="No comparable tasks yet" />
          </div>
        )}
      </Card>
      <Card title="Historical estimate">
        <ErrorBanner error={estimate.error} />
        {!estimate.data ? (
          !estimate.error && <Skeleton />
        ) : estimate.data.status !== "ok" ? (
          <div className="space-y-1 text-sm">
            <div className="font-medium text-amber-700">Insufficient data</div>
            <div className="text-slate-600">{estimate.data.message}</div>
          </div>
        ) : (
          <div className="space-y-2 text-[12px]">
            <div className="text-slate-500">
              Based on {estimate.data.sample_size} comparable tasks ({estimate.data.method}). {estimate.data.uncertainty}
            </div>
            <Table head={["Measure", "n", "p25", "median", "p75", "p90"]}>
              {Object.entries(estimate.data.distributions ?? {}).map(([name, d]) => (
                <tr key={name}>
                  <td className={td}>{label(name)}</td>
                  <td className={td}>{d.n}</td>
                  <td className={td}>{fmt(name, d.p25)}</td>
                  <td className={`${td} font-medium`}>{fmt(name, d.median)}</td>
                  <td className={td}>{fmt(name, d.p75)}</td>
                  <td className={td}>{fmt(name, d.p90)}</td>
                </tr>
              ))}
            </Table>
            <div>
              <div className="font-medium text-slate-600">Basis</div>
              {estimate.data.basis?.map((b) => (
                <div key={b.key}>
                  <Link to={`/tasks/${b.key}`} className="mono text-sky-700">
                    {b.key}
                  </Link>{" "}
                  {b.reasons.join("; ")}
                </div>
              ))}
            </div>
            {estimate.data.caveats?.map((c) => (
              <p key={c} className="text-[11px] text-slate-400">
                {c}
              </p>
            ))}
          </div>
        )}
      </Card>
    </div>
  );
}
