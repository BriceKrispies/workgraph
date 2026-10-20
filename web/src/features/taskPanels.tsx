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
import {
  Button,
  Card,
  Diff,
  Empty,
  ErrorBanner,
  KeyValue,
  List,
  ListRow,
  Meta,
  Skeleton,
  StatusBadge,
  inputClass,
  selectClass,
} from "../components/ui";
import { describeEvent } from "../pages/Overview";
import { ago, dateTime, label, ms, num, pct, sentence, short } from "../lib/format";
import { toneOf } from "../lib/tone";

export function HistoryPanel({ ws, taskKey }: { ws: string; taskKey: string }) {
  const history = useQuery({
    queryKey: ["task", ws, taskKey, "history"],
    queryFn: () => get<EventOut[]>(`/ws/${ws}/tasks/${taskKey}/history`),
  });
  if (history.error) return <ErrorBanner error={history.error} />;
  if (!history.data) return <Skeleton />;
  if (!history.data.length) return <Empty title="No history" />;
  return (
    <ol className="relative ml-1 space-y-4 border-l-2 border-line pl-5" data-testid="history">
      {[...history.data].reverse().map((e) => {
        const to = (e.data as Record<string, unknown>).to;
        return (
          <li key={e.id} className="relative">
            <span className={`dot absolute -left-[26px] top-1.5 ring-4 ring-canvas tone-${typeof to === "string" ? toneOf(to) : "idle"}`} />
            <div className="break-words text-[14px] text-ink">{describeEvent(e) || e.event_type}</div>
            <Meta>
              <span className="mono text-[12px]">{e.event_type}</span>
              <span>{e.actor}</span>
              <span title={dateTime(e.occurred_at)}>{ago(e.occurred_at)}</span>
            </Meta>
          </li>
        );
      })}
    </ol>
  );
}

function Figures({ items }: { items: [string, React.ReactNode][] }) {
  return (
    <dl className="grid grid-cols-2 gap-2 sm:grid-cols-3">
      {items.map(([k, v]) => (
        <div key={k} className="rounded-xl bg-sunken px-3 py-2">
          <dt className="text-[12px] text-ink-3">{k}</dt>
          <dd className="text-[17px] font-semibold tnum">{v}</dd>
        </div>
      ))}
    </dl>
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
    <div className="space-y-4">
      <Card title="Cumulative churn" footer={definitions?.cumulative_churn}>
        <Figures
          items={[
            ["Commits", churn.commits],
            ["Lines added", `+${churn.insertions}`],
            ["Lines removed", `−${churn.deletions}`],
            ["Files touched", churn.files_touched],
            ["Repeated file edits", churn.repeated_file_edits],
            ["Functions affected", churn.functions_affected],
          ]}
        />
      </Card>
      <Card title="Final change surface" footer={definitions?.final_change_surface}>
        {surface.length ? (
          <div className="space-y-4">
            {surface.map((s) => (
              <div key={s.repository_id}>
                <div className="mb-2 font-medium">{repoNames.get(s.repository_id) ?? short(s.repository_id)}</div>
                <KeyValue
                  items={[
                    ["range", <span className="mono text-[12px]">{`${short(s.base_sha, 10)}..${short(s.final_sha, 10)}`}</span>],
                    ["files", s.files_changed],
                    ["lines", <Diff added={s.insertions} removed={s.deletions} />],
                    ["renames, binary", `${s.renames}, ${s.binary_files}`],
                    ["functions, types", `${s.functions_affected}, ${s.types_affected}`],
                  ]}
                />
              </div>
            ))}
          </div>
        ) : (
          <Empty title="Not computed yet">Computed when the task is accepted.</Empty>
        )}
      </Card>
      <div>
        <div className="mb-2 flex items-center justify-between px-1">
          <h3 className="text-[15px] font-semibold">Linked commits ({commits.length})</h3>
          {commits.length > 0 && (
            <Button size="sm" onClick={() => setShowDiff((v) => !v)}>
              {showDiff ? "Hide diff" : "Show diff"}
            </Button>
          )}
        </div>
        {commits.length ? (
          <List testId="task-commits">
            {commits.map((c) => (
              <ListRow key={c.commit_id} to={`/repositories/${c.repository}?sha=${c.sha}`}>
                <div className="line-clamp-2 font-medium leading-snug">{c.subject}</div>
                <Meta>
                  <span className="mono text-[12px] text-ink-2">{short(c.sha, 10)}</span>
                  <span>{c.repository}</span>
                  <Diff added={c.insertions} removed={c.deletions} />
                  {!c.reachable && <span className="text-warn">Unreachable</span>}
                  <span>{ago(c.committed_at)}</span>
                </Meta>
                <div className="mt-1 text-[12px] text-ink-3">
                  {c.links.map((l, i) => (
                    <span key={i} className="mr-3" title={JSON.stringify(l.evidence)}>
                      via {label(l.source)} ({l.confidence.toFixed(2)})
                    </span>
                  ))}
                </div>
              </ListRow>
            ))}
          </List>
        ) : (
          <Card>
            <Empty title="No linked commits">Commits are attributed through Task-ID trailers, agent runs or explicit links.</Empty>
          </Card>
        )}
      </div>
      {showDiff && (
        <Card title="Diff, read live from Git" bodyClass="p-0">
          <div className="p-4 empty:hidden">
            <ErrorBanner error={diff.error} />
          </div>
          {!diff.data && !diff.error && <Skeleton />}
          {diff.data?.parts.map((part, i) => (
            <div key={i} className="border-t border-line first:border-t-0">
              <div className="px-4 py-2 text-[13px] font-medium">
                {part.repository} <span className="mono text-ink-3">{part.range ?? short(part.sha, 10)}</span>
              </div>
              <pre className="mono max-h-[28rem] overflow-auto bg-code px-4 py-3 text-[11.5px] leading-relaxed text-code-ink">
                {part.diff.split("\n").map((line, n) => (
                  <div
                    key={n}
                    className={line.startsWith("+") && !line.startsWith("+++") ? "text-[#7ee2a8]" : line.startsWith("-") && !line.startsWith("---") ? "text-[#ff9aa9]" : line.startsWith("@@") ? "text-[#8fc7ff]" : ""}
                  >
                    {line || " "}
                  </div>
                ))}
              </pre>
            </div>
          ))}
          {diff.data && diff.data.missing_objects.length > 0 && (
            <div className="px-4 py-3 text-[13px] text-warn">Some Git objects are no longer available: {JSON.stringify(diff.data.missing_objects)}</div>
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
  if (!attempts.data.items.length)
    return (
      <Card>
        <Empty title="No attempts yet">Start an agent run, or report an attempt through MCP.</Empty>
      </Card>
    );
  return (
    <List testId="task-attempts">
      {attempts.data.items.map((a) => (
        <ListRow key={a.id} to={`/execution?attempt=${a.id}`} tone={toneOf(a.status)}>
          <div className="flex items-center gap-2">
            <span className="font-semibold">Attempt {a.number}</span>
            <StatusBadge status={a.status} />
            {a.parent_attempt_id && <span className="text-[12px] text-ink-3">Subagent</span>}
            {a.retry_of_attempt_id && <span className="text-[12px] text-ink-3">Retry</span>}
            <span className="ml-auto text-[13px] text-ink-3">{ago(a.started_at)}</span>
          </div>
          <Meta>
            <span>
              {a.provider}
              {a.model && ` / ${a.model}`}
            </span>
            <span>Ran {ms(a.duration_ms)}</span>
            <span>Queued {ms(a.queue_wait_ms)}</span>
            {Object.keys(a.usage).length > 0 ? (
              <span title={a.usage_source ?? ""}>
                {num(a.usage.input_tokens)} in, {num(a.usage.output_tokens)} out
              </span>
            ) : (
              <span>Usage not measured</span>
            )}
            {a.exit_reason && <span>{sentence(a.exit_reason)}</span>}
          </Meta>
          {a.error && <div className="mt-1 line-clamp-2 text-[13px] text-danger">{a.error}</div>}
        </ListRow>
      ))}
    </List>
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
    <div className="space-y-4">
      <div className="flex flex-col gap-3 rounded-2xl border border-line bg-surface p-4 sm:flex-row sm:items-center sm:justify-between">
        <div className="text-[14px] text-ink-2">
          {checks.data?.length
            ? `${checks.data.length} approved ${checks.data.length === 1 ? "check applies" : "checks apply"}: ${checks.data.map((c) => c.name).join(", ")}`
            : "No verification is configured, so acceptance has to be recorded manually."}
        </div>
        <Button disabled={!checks.data?.length} busy={verify.isPending} onClick={() => verify.mutate()}>
          Run verification
        </Button>
      </div>
      <ErrorBanner error={runs.error ?? verify.error} />
      {verify.data && <Meta>Queued job {short(verify.data.job_id)}. A running worker picks it up.</Meta>}
      {!runs.data ? (
        <Skeleton />
      ) : !runs.data.length ? (
        <Card>
          <Empty title="No verification runs" />
        </Card>
      ) : (
        runs.data.map((run) => (
          <Card
            key={run.id}
            title={
              <span className="flex flex-wrap items-center gap-2 font-medium">
                <StatusBadge status={run.status} />
                <span className="mono text-[13px]">run {short(run.id)}</span>
              </span>
            }
            actions={<span className="text-[13px] text-ink-3">{ago(run.created_at)}</span>}
            bodyClass="p-0"
            footer={
              <>
                Triggered by {run.triggered_by}. Manifest: {run.manifest.map((m) => `${m.repository}@${short(m.sha ?? "", 10)}`).join(", ") || "none"}
              </>
            }
          >
            <ul className="divide-y divide-line">
              {run.checks.map((c) => (
                <li key={c.id}>
                  <details className="group">
                    <summary className={`rail tone-${toneOf(c.status)} flex min-h-12 cursor-pointer list-none items-center gap-3 px-4 py-2.5 hover:bg-sunken`}>
                      <div className="min-w-0 flex-1">
                        <div className="flex flex-wrap items-center gap-2">
                          <span className="font-medium">{c.name}</span>
                          <StatusBadge status={c.status} />
                        </div>
                        <Meta>
                          <span>{c.kind}</span>
                          <span>{c.required ? "Required" : "Optional"}</span>
                          <span>Exit {c.exit_code ?? "—"}</span>
                          <span>{ms(c.duration_ms)}</span>
                        </Meta>
                        {c.failure_signatures.length > 0 && <div className="mono mt-1 break-all text-[12px] text-danger">{c.failure_signatures.join(", ")}</div>}
                      </div>
                      <span className="text-[12px] text-ink-3 group-open:hidden">Output</span>
                    </summary>
                    <pre className="mono max-h-72 overflow-auto bg-code px-4 py-3 text-[11.5px] text-code-ink">{c.output_excerpt || "(no output)"}</pre>
                  </details>
                </li>
              ))}
            </ul>
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
    <div className="space-y-4">
      <ErrorBanner error={request.error ?? confirm.error} />
      <div className="grid gap-4 md:grid-cols-2">
        <Card
          title="Current classification"
          actions={
            <Button size="sm" busy={request.isPending} onClick={() => request.mutate()}>
              Re-classify
            </Button>
          }
          footer={data.data.note}
        >
          {current ? (
            <KeyValue
              items={[
                ["category", <strong>{sentence(current.category)}</strong>],
                ["subtype", current.subtype],
                ["domain", current.domain],
                ["complexity", current.complexity],
                ["confidence", current.confidence !== null ? pct(current.confidence) : null],
                ["source", current.source === "manual" ? "Confirmed by a person" : `${current.provider}/${current.model}`],
                ["prompt, taxonomy", `${current.prompt_version ?? "—"}, ${current.taxonomy_version}`],
                ["rationale", current.rationale],
              ]}
            />
          ) : (
            <Empty title="Not classified yet" />
          )}
          {request.isSuccess && <div className="mt-3 text-[13px] text-ink-3">Classification job queued.</div>}
        </Card>
        <Card title="Confirm manually" footer="A person's classification is authoritative and joins the gold evaluation set.">
          <form
            className="space-y-3"
            onSubmit={(e) => {
              e.preventDefault();
              confirm.mutate();
            }}
          >
            <select aria-label="Confirmed category" className={`${selectClass} w-full`} value={category} onChange={(e) => setCategory(e.target.value)} required>
              <option value="">Choose a category…</option>
              {CATEGORIES.map((c) => (
                <option key={c} value={c}>
                  {sentence(c)}
                </option>
              ))}
            </select>
            <input className={inputClass} placeholder="Note (optional)" value={note} onChange={(e) => setNote(e.target.value)} />
            <Button type="submit" variant="primary" className="w-full sm:w-auto" busy={confirm.isPending} disabled={!category}>
              Confirm classification
            </Button>
          </form>
        </Card>
      </div>
      <div>
        <h3 className="mb-2 px-1 text-[15px] font-semibold">
          History{data.data.disagreement && <span className="ml-2 text-[13px] font-normal text-warn">Models disagree</span>}
        </h3>
        <List>
          {data.data.history.map((c) => (
            <ListRow key={c.id} tone={c.is_current ? "done" : undefined}>
              <div className="flex flex-wrap items-center gap-2">
                <span className="font-medium">{sentence(c.category)}</span>
                {c.is_current && <span className="text-[12px] font-medium text-ok">Current</span>}
                <span className="ml-auto text-[13px] text-ink-3">{ago(c.created_at)}</span>
              </div>
              <Meta>
                <span>{c.source}</span>
                {c.provider && <span>{`${c.provider}/${c.model}`}</span>}
                {c.confidence !== null && <span>{pct(c.confidence)} confidence</span>}
                <span>{c.validation_status}</span>
              </Meta>
              {c.error && <div className="mt-1 text-[13px] text-danger">{c.error}</div>}
            </ListRow>
          ))}
        </List>
      </div>
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
    <div className="grid gap-4 lg:grid-cols-2">
      <div className="min-w-0">
        <h3 className="mb-2 px-1 text-[15px] font-semibold">
          Similar tasks {similar.data && <span className="font-normal text-ink-3">by {similar.data.method}</span>}
        </h3>
        <ErrorBanner error={similar.error} />
        {!similar.data ? (
          !similar.error && <Skeleton />
        ) : similar.data.similar.length ? (
          <List>
            {similar.data.similar.map((s) => (
              <ListRow key={s.task_id} to={`/tasks/${s.key}`} tone={toneOf(s.status)}>
                <div className="flex items-center gap-2">
                  <span className="mono text-[13px] text-ink-3">{s.key}</span>
                  <StatusBadge status={s.status} />
                  <span className="ml-auto text-[13px] text-ink-3 tnum">score {s.score.toFixed(2)}</span>
                </div>
                <div className="mt-1 line-clamp-2 font-medium leading-snug">{s.title}</div>
                {s.implementation.length > 0 && <Meta className="mono text-[12px]">{s.implementation.map((i) => short(i.sha, 8)).join(" ")}</Meta>}
              </ListRow>
            ))}
          </List>
        ) : (
          <Card>
            <Empty title="No comparable tasks yet" />
          </Card>
        )}
      </div>
      <Card title="Historical estimate">
        <ErrorBanner error={estimate.error} />
        {!estimate.data ? (
          !estimate.error && <Skeleton />
        ) : estimate.data.status !== "ok" ? (
          <div className="space-y-1 text-[14px]">
            <div className="font-medium text-warn">Not enough data</div>
            <div className="text-ink-2">{estimate.data.message}</div>
          </div>
        ) : (
          <div className="space-y-4 text-[13px]">
            <p className="text-ink-2">
              Based on {estimate.data.sample_size} comparable tasks ({estimate.data.method}). {estimate.data.uncertainty}
            </p>
            <div className="space-y-2">
              {Object.entries(estimate.data.distributions ?? {}).map(([name, d]) => (
                <div key={name} className="rounded-xl bg-sunken p-3">
                  <div className="flex items-baseline justify-between">
                    <span className="font-medium">{sentence(name.replace(/_ms$/, ""))}</span>
                    <span className="text-ink-3">n = {d.n}</span>
                  </div>
                  <div className="mt-2 grid grid-cols-4 gap-1 text-center">
                    {(["p25", "median", "p75", "p90"] as const).map((k) => (
                      <div key={k}>
                        <div className="text-[11px] text-ink-3">{k}</div>
                        <div className={`tnum ${k === "median" ? "font-semibold" : ""}`}>{fmt(name, d[k])}</div>
                      </div>
                    ))}
                  </div>
                </div>
              ))}
            </div>
            {!!estimate.data.basis?.length && (
              <div>
                <div className="mb-1 font-medium text-ink-2">Basis</div>
                <ul className="space-y-1">
                  {estimate.data.basis.map((b) => (
                    <li key={b.key}>
                      <Link to={`/tasks/${b.key}`} className="mono text-accent">
                        {b.key}
                      </Link>{" "}
                      <span className="text-ink-2">{b.reasons.join("; ")}</span>
                    </li>
                  ))}
                </ul>
              </div>
            )}
            {estimate.data.caveats?.map((c) => (
              <p key={c} className="text-[12px] text-ink-3">
                {c}
              </p>
            ))}
          </div>
        )}
      </Card>
    </div>
  );
}
