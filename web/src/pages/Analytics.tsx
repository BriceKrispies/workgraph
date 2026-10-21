import { useState, type ReactNode } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { get, post } from "../api/client";
import {
  CATEGORIES,
  type AnalyticsResult,
  type Bottlenecks,
  type MetricDefinition,
  type Project,
  type Row,
  type SlowCall,
} from "../api/types";
import {
  Button,
  Card,
  Empty,
  ErrorBanner,
  Field,
  List,
  ListRow,
  Meta,
  PageHeader,
  Skeleton,
  Table,
  Tabs,
  inputBase,
  selectClass,
  td,
} from "../components/ui";
import { CategoryBars, Distribution, PeriodSeries } from "../features/charts";
import { useWorkspace } from "../hooks/workspace";
import { label, ms, num, pct, sentence } from "../lib/format";

interface Filters {
  since: string;
  until: string;
  category: string;
  project: string;
}

type TabId = "throughput" | "execution" | "agents" | "code" | "coupling" | "failures" | "history" | "definitions";

function useMetric(ws: string, metric: string, groupBy: string[], filters: Filters, period = "week") {
  return useQuery({
    queryKey: ["analytics", ws, metric, groupBy, filters, period],
    queryFn: () =>
      post<AnalyticsResult>(`/ws/${ws}/analytics/query`, {
        metric,
        group_by: groupBy,
        period,
        filters: {
          since: filters.since ? new Date(filters.since).toISOString() : null,
          until: filters.until ? new Date(filters.until).toISOString() : null,
          category: filters.category || null,
          project: filters.project || null,
        },
      }),
  });
}

function MetricCard({
  title,
  result,
  children,
  className,
}: {
  title: string;
  result: { data?: AnalyticsResult; error: unknown; isLoading: boolean };
  children: (rows: Row[]) => ReactNode;
  className?: string;
}) {
  return (
    <Card
      title={title}
      className={className}
      footer={
        result.data && (
          <span title={result.data.provenance.sql}>
            {result.data.metric.definition} {result.data.metric.caveats.join(" ")}
          </span>
        )
      }
    >
      <ErrorBanner error={result.error} />
      {result.isLoading ? <Skeleton /> : result.data ? children(result.data.rows) : null}
    </Card>
  );
}

export function AnalyticsPage() {
  const { ws } = useWorkspace();
  const [params, setParams] = useSearchParams();
  const tabs: TabId[] = ["throughput", "execution", "agents", "code", "coupling", "failures", "history", "definitions"];
  const requested = params.get("tab") as TabId | null;
  const tab: TabId = requested && tabs.includes(requested) ? requested : "throughput";
  const setTab = (next: TabId) => setParams(next === "throughput" ? {} : { tab: next });
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [filters, setFilters] = useState<Filters>({ since: "", until: "", category: "", project: "" });
  const projects = useQuery({ queryKey: ["projects", ws], queryFn: () => get<Project[]>(`/ws/${ws}/projects`) });
  const set = (key: keyof Filters, value: string) => setFilters((f) => ({ ...f, [key]: value }));
  const active = Object.values(filters).filter(Boolean).length;

  return (
    <div>
      <PageHeader
        title="Analytics"
        actions={
          <Button icon="filter" aria-expanded={filtersOpen} onClick={() => setFiltersOpen((v) => !v)} className={`lg:hidden ${active ? "border-accent text-accent" : ""}`}>
            {active ? `Filters ${active}` : "Filters"}
          </Button>
        }
      >
        <p className="max-w-prose text-[14px] text-ink-2">
          Measured from recorded tasks, attempts, checks and commits. Groups differ in the work they received, so differences are associations, not causes.
        </p>
      </PageHeader>
      <div className={`${filtersOpen ? "grid" : "hidden"} mb-4 grid-cols-2 gap-3 rounded-2xl border border-line bg-surface p-4 lg:grid lg:grid-cols-4 lg:border-0 lg:bg-transparent lg:p-0`}>
        <Field label="Since">
          <input type="date" aria-label="since" className={`${inputBase} w-full min-w-0`} value={filters.since} onChange={(e) => set("since", e.target.value)} />
        </Field>
        <Field label="Until">
          <input type="date" aria-label="until" className={`${inputBase} w-full min-w-0`} value={filters.until} onChange={(e) => set("until", e.target.value)} />
        </Field>
        <Field label="Category">
          <select aria-label="Category" className={`${selectClass} w-full min-w-0`} value={filters.category} onChange={(e) => set("category", e.target.value)}>
            <option value="">Any category</option>
            {CATEGORIES.map((c) => (
              <option key={c} value={c}>
                {sentence(c)}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Project">
          <select aria-label="Project" className={`${selectClass} w-full min-w-0`} value={filters.project} onChange={(e) => set("project", e.target.value)}>
            <option value="">Any project</option>
            {projects.data?.map((p) => (
              <option key={p.id} value={p.slug}>
                {p.name}
              </option>
            ))}
          </select>
        </Field>
      </div>
      <Tabs<TabId>
        value={tab}
        onChange={setTab}
        tabs={[
          ["throughput", "Throughput"],
          ["execution", "Execution"],
          ["agents", "Agents & models"],
          ["code", "Code"],
          ["coupling", "Coupling"],
          ["failures", "Failures"],
          ["history", "History"],
          ["definitions", "Definitions"],
        ]}
      />
      <div className="pt-4">
        {tab === "throughput" && <Throughput ws={ws} filters={filters} />}
        {tab === "execution" && <Execution ws={ws} filters={filters} />}
        {tab === "agents" && <Agents ws={ws} filters={filters} />}
        {tab === "code" && <Code ws={ws} filters={filters} />}
        {tab === "coupling" && <Coupling ws={ws} project={filters.project} />}
        {tab === "failures" && <Failures ws={ws} filters={filters} />}
        {tab === "history" && <History ws={ws} category={filters.category} />}
        {tab === "definitions" && <Definitions />}
      </div>
    </div>
  );
}

function Throughput({ ws, filters }: { ws: string; filters: Filters }) {
  const created = useMetric(ws, "tasks_created", ["period"], filters);
  const completed = useMetric(ws, "tasks_completed", ["period"], filters);
  const accepted = useMetric(ws, "tasks_accepted", ["period"], filters);
  const open = useMetric(ws, "open_backlog", ["status"], filters);
  const categories = useMetric(ws, "tasks_created", ["category"], filters);
  const blocked = useMetric(ws, "blocked_tasks", ["project"], filters);
  return (
    <div className="grid gap-4 lg:grid-cols-2">
      <Card
        title="Tasks per week"
        className="lg:col-span-2"
        footer="Completion is an agent's or person's claim. Acceptance needs verification or a recorded decision."
      >
        <ErrorBanner error={created.error ?? completed.error ?? accepted.error} />
        {created.data && completed.data && accepted.data ? (
          <PeriodSeries
            height={240}
            series={[
              { name: "created", rows: created.data.rows },
              { name: "completed (claimed)", rows: completed.data.rows },
              { name: "accepted", rows: accepted.data.rows },
            ]}
          />
        ) : (
          <Skeleton />
        )}
      </Card>
      <MetricCard title="Open backlog by status" result={open}>
        {(rows) => <CategoryBars rows={rows} category="status" />}
      </MetricCard>
      <MetricCard title="Tasks by category" result={categories}>
        {(rows) => <CategoryBars rows={rows} category="category" />}
      </MetricCard>
      <MetricCard title="Blocked work by project" result={blocked}>
        {(rows) => <CategoryBars rows={rows} category="project" />}
      </MetricCard>
    </div>
  );
}

function DistCard({ ws, metric, title, unit, filters }: { ws: string; metric: string; title: string; unit: "ms" | "count" | "lines" | "ratio"; filters: Filters }) {
  const result = useMetric(ws, metric, [], filters);
  return (
    <MetricCard title={title} result={result}>
      {(rows) => <Distribution row={rows[0]} unit={unit} />}
    </MetricCard>
  );
}

function RatioCard({ ws, metric, title, filters }: { ws: string; metric: string; title: string; filters: Filters }) {
  const result = useMetric(ws, metric, [], filters);
  return (
    <MetricCard title={title} result={result}>
      {(rows) =>
        rows[0] && Number(rows[0].denominator) > 0 ? (
          <div>
            <div className="text-[28px] font-semibold leading-none tracking-tight tnum">{pct(rows[0].value)}</div>
            <div className="mt-1.5 text-[13px] text-ink-3">
              {rows[0].numerator} of {rows[0].denominator}
            </div>
            <div className="mt-3 h-2 overflow-hidden rounded-full bg-sunken ring-1 ring-line">
              <div className="h-full rounded-full bg-accent" style={{ width: `${Number(rows[0].value) * 100}%` }} />
            </div>
          </div>
        ) : (
          <span className="text-[14px] text-ink-3">No samples yet</span>
        )
      }
    </MetricCard>
  );
}

function Execution({ ws, filters }: { ws: string; filters: Filters }) {
  const outcomes = useMetric(ws, "attempt_outcomes", ["status"], filters);
  return (
    <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
      <DistCard ws={ws} filters={filters} metric="queue_wait" title="Queue waiting time" unit="ms" />
      <DistCard ws={ws} filters={filters} metric="execution_duration" title="Active execution time" unit="ms" />
      <DistCard ws={ws} filters={filters} metric="task_elapsed" title="Total task elapsed time" unit="ms" />
      <DistCard ws={ws} filters={filters} metric="cycle_time" title="Cycle time" unit="ms" />
      <DistCard ws={ws} filters={filters} metric="attempts_per_task" title="Attempts per accepted task" unit="count" />
      <RatioCard ws={ws} filters={filters} metric="first_pass_success" title="First-pass success" />
      <RatioCard ws={ws} filters={filters} metric="retry_rate" title="Retry frequency" />
      <RatioCard ws={ws} filters={filters} metric="attempt_success_rate" title="Attempt success rate" />
      <MetricCard title="Attempt outcomes" result={outcomes}>
        {(rows) => <CategoryBars rows={rows} category="status" />}
      </MetricCard>
    </div>
  );
}

function Figure({ name, value, strong }: { name: string; value: ReactNode; strong?: boolean }) {
  return (
    <div className="min-w-0">
      <div className="text-[12px] text-ink-3">{name}</div>
      <div className={`truncate tnum ${strong ? "text-[17px] font-semibold" : "text-[14px]"}`}>{value}</div>
    </div>
  );
}

function AgentTime({ ws, filters }: { ws: string; filters: Filters }) {
  const since = filters.since ? new Date(filters.since).toISOString() : null;
  const until = filters.until ? new Date(filters.until).toISOString() : null;
  const report = useQuery({
    queryKey: ["agent-time", ws, since, until],
    queryFn: () => get<Bottlenecks>(`/ws/${ws}/analytics/bottlenecks`, { since, until, top: 10 }),
  });
  const data = report.data;
  const minutes = (seconds: number) => `${(seconds / 60).toFixed(1)} min`;
  return (
    <Card title="Where subagent time goes" className="lg:col-span-2">
      <ErrorBanner error={report.error} />
      {!data ? (
        report.isLoading ? <Skeleton /> : null
      ) : data.totals.runs === 0 ? (
        <Empty title="No timed subagent runs in this window" />
      ) : (
        <div className="space-y-4">
          <p className="text-[12px] text-slate-600">
            {data.totals.runs} runs, {(data.totals.span_seconds / 3600).toFixed(2)} agent-hours of wall time. Waiting on background work{" "}
            {pct(data.totals.waiting_share)}, polling loops {pct(data.totals.polling_share)}, model {pct(data.totals.model_share)} of accounted
            time. Peak concurrency {data.concurrency.peak_during_window} live agents.
          </p>
          <CategoryBars
            height={200}
            rows={data.classes.map((row) => ({ class: label(row.class), minutes: row.seconds / 60 }) as Row)}
            category="class"
            value="minutes"
            format={(v) => `${v.toFixed(0)} min`}
          />
          <Table head={["Seconds", "Class", "Live agents", "Agent", "Call"]} testId="slowest-calls">
            {data.slowest_calls.map((call: SlowCall, index) => (
              <tr key={index}>
                <td className={td}>{call.seconds.toFixed(0)}</td>
                <td className={td}>{label(call.class)}</td>
                <td className={td}>{call.concurrent_runs ?? "—"}</td>
                <td className={td}>{call.description}</td>
                <td className={`${td} max-w-[28rem] truncate font-mono text-[11px]`} title={call.command ?? ""}>
                  {call.command ?? "—"}
                </td>
              </tr>
            ))}
          </Table>
          <Table head={["Task", "Runs", "Wall time", "Top classes"]}>
            {data.by_task.map((row) => (
              <tr key={row.key}>
                <td className={td}>{row.key}</td>
                <td className={td}>{row.runs}</td>
                <td className={td}>{minutes(row.span_seconds)}</td>
                <td className={td}>{row.top_classes.map((c) => `${label(c.class)} ${minutes(c.seconds)}`).join(", ")}</td>
              </tr>
            ))}
          </Table>
          {data.notes.map((note) => (
            <p key={note} className="text-[11px] text-slate-400">
              {note}
            </p>
          ))}
        </div>
      )}
    </Card>
  );
}

function Agents({ ws, filters }: { ws: string; filters: Filters }) {
  const completion = useMetric(ws, "agent_completion_rate", ["provider", "model"], filters);
  const duration = useMetric(ws, "agent_duration", ["provider", "model"], filters);
  const verify = useMetric(ws, "agent_verification_pass_rate", ["provider", "model"], filters);
  const byCategory = useMetric(ws, "agent_completion_rate", ["provider", "category"], filters);
  const tokensIn = useMetric(ws, "input_tokens", ["provider"], filters);
  const tokensOut = useMetric(ws, "output_tokens", ["provider"], filters);
  const cost = useMetric(ws, "cost", ["provider"], filters);
  const key = (r: Row) => `${r.provider ?? ""}|${r.model ?? r.category ?? ""}`;
  const durations = new Map((duration.data?.rows ?? []).map((r) => [key(r), r]));
  const verifications = new Map((verify.data?.rows ?? []).map((r) => [key(r), r]));
  return (
    <div className="space-y-4">
      <section>
        <h2 className="mb-2 px-1 text-[17px] font-semibold lg:text-[15px]">Agent and model comparison</h2>
        <ErrorBanner error={completion.error} />
        {!completion.data ? (
          <Card bodyClass="p-0">
            <Skeleton />
          </Card>
        ) : completion.data.rows.length === 0 ? (
          <Card>
            <Empty title="No finished attempts" />
          </Card>
        ) : (
          <List testId="agent-comparison">
            {completion.data.rows.map((r) => {
              const d = durations.get(key(r));
              const v = verifications.get(key(r));
              return (
                <ListRow key={key(r)}>
                  <div className="flex flex-wrap items-baseline gap-x-2">
                    <span className="font-semibold">{String(r.provider)}</span>
                    <span className="text-[14px] text-ink-2">{r.model ? String(r.model) : "(unspecified model)"}</span>
                  </div>
                  <div className="mt-2 grid grid-cols-2 gap-x-4 gap-y-2 sm:grid-cols-4">
                    <Figure name="Completion rate" value={`${pct(r.value)} (${r.numerator}/${r.denominator})`} strong />
                    <Figure name="Median duration" value={ms(d?.median)} />
                    <Figure name="p90 duration" value={ms(d?.p90)} />
                    <Figure name="Verification pass" value={v ? `${pct(v.value)} (${v.numerator}/${v.denominator})` : "—"} />
                  </div>
                </ListRow>
              );
            })}
          </List>
        )}
        {completion.data && <p className="mt-2 px-1 text-[12px] text-ink-3">{completion.data.metric.caveats.join(" ")}</p>}
      </section>
      <div className="grid gap-4 lg:grid-cols-2">
        <AgentTime ws={ws} filters={filters} />
        <MetricCard title="Completion rate by agent and category" result={byCategory}>
          {(rows) =>
            rows.length ? (
              <ul className="-my-2 divide-y divide-line">
                {rows.map((r) => (
                  <li key={key(r)} className="flex items-baseline gap-3 py-2.5 text-[14px]">
                    <span className="min-w-0 flex-1 truncate">
                      {String(r.provider)} <span className="text-ink-3">{sentence(String(r.category ?? ""))}</span>
                    </span>
                    <span className="font-semibold tnum">{pct(r.value)}</span>
                    <span className="w-14 text-right text-[12px] text-ink-3 tnum">n = {String(r.denominator)}</span>
                  </li>
                ))}
              </ul>
            ) : (
              <Empty title="No data yet" />
            )
          }
        </MetricCard>
        <Card title="Usage" footer="Measured usage only. Providers that do not report usage are never estimated.">
          {tokensIn.data?.rows.length ? (
            <ul className="-my-2 divide-y divide-line">
              {tokensIn.data.rows.map((r) => {
                const out = tokensOut.data?.rows.find((o) => o.provider === r.provider);
                const c = cost.data?.rows.find((o) => o.provider === r.provider);
                return (
                  <li key={String(r.provider)} className="py-2.5">
                    <div className="font-medium">{String(r.provider)}</div>
                    <div className="mt-1.5 grid grid-cols-2 gap-2 sm:grid-cols-4">
                      <Figure name="Input tokens" value={num(r.value)} />
                      <Figure name="Output tokens" value={num(out?.value)} />
                      <Figure name="Cost" value={c && Number(c.n) > 0 ? `$${Number(c.value).toFixed(4)}` : "Not measured"} />
                      <Figure name="Attempts measured" value={String(r.n)} />
                    </div>
                  </li>
                );
              })}
            </ul>
          ) : (
            <Empty title="No measured usage" />
          )}
        </Card>
      </div>
    </div>
  );
}

function Code({ ws, filters }: { ws: string; filters: Filters }) {
  const volume = useMetric(ws, "repository_change_volume", ["repository"], filters);
  const classes = useMetric(ws, "structural_distribution", ["file_class"], filters);
  const hotspots = useMetric(ws, "module_hotspots", ["module"], filters);
  return (
    <div className="space-y-4">
      <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
        <DistCard ws={ws} filters={filters} metric="cumulative_churn" title="Cumulative churn (lines)" unit="lines" />
        <DistCard ws={ws} filters={filters} metric="final_change_size" title="Final change size (lines)" unit="lines" />
        <DistCard ws={ws} filters={filters} metric="rework_ratio" title="Rework indicator (churn ÷ final)" unit="ratio" />
        <DistCard ws={ws} filters={filters} metric="files_touched" title="Files affected" unit="count" />
        <DistCard ws={ws} filters={filters} metric="functions_affected" title="Functions affected" unit="count" />
        <DistCard ws={ws} filters={filters} metric="repeated_file_edits" title="Repeated file edits" unit="count" />
      </div>
      <div className="grid gap-4 lg:grid-cols-3">
        <MetricCard title="Change volume by repository" result={volume}>
          {(rows) => <CategoryBars rows={rows} category="repository" />}
        </MetricCard>
        <MetricCard title="Lines by file class" result={classes}>
          {(rows) => <CategoryBars rows={rows} category="file_class" />}
        </MetricCard>
        <MetricCard title="Module hotspots" result={hotspots}>
          {(rows) => <CategoryBars rows={[...rows].sort((a, b) => Number(b.value) - Number(a.value)).slice(0, 12)} category="module" />}
        </MetricCard>
      </div>
    </div>
  );
}

interface CouplingReport {
  repositories: { id: string; name: string; tasks_touching: number }[];
  edges: {
    source_name: string;
    target_name: string;
    co_changes: number;
    jaccard: number | null;
    relationship: string;
    evidence: { task_key: string; commits: Record<string, string[]> }[];
  }[];
  declared_only: { source_name: string; target_name: string }[];
  matrix: Record<string, Record<string, number>>;
  summary: Record<string, number | null>;
  coupling_candidates: { observation: string }[];
  modules_with_cross_repo_impact: { module: string; cross_repo_tasks: number; tasks: string[] }[];
  notes: string[];
}

function Coupling({ ws, project }: { ws: string; project: string }) {
  const report = useQuery({
    queryKey: ["analytics", ws, "coupling", project],
    queryFn: () => get<CouplingReport>(`/ws/${ws}/analytics/coupling`, { project: project || undefined }),
  });
  const [selected, setSelected] = useState<number | null>(null);
  if (report.error) return <ErrorBanner error={report.error} />;
  if (!report.data) return <Skeleton />;
  const r = report.data;
  const names = Object.keys(r.matrix);
  const max = Math.max(1, ...names.flatMap((a) => names.filter((b) => a !== b).map((b) => r.matrix[a][b])));
  return (
    <div className="space-y-4">
      <div className="grid gap-4 lg:grid-cols-2">
        <Card title="Co-change matrix" footer="Number of tasks that changed both repositories.">
          {names.length ? (
            <div className="scroll-x -mx-4 px-4">
              <table className="text-[12px]" data-testid="coupling-matrix">
                <thead>
                  <tr>
                    <th />
                    {names.map((n) => (
                      <th key={n} className="px-1 pb-1 text-left font-medium text-ink-3" style={{ writingMode: "vertical-rl" }}>
                        {n}
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {names.map((a) => (
                    <tr key={a}>
                      <th className="max-w-28 truncate pr-2 text-right font-medium text-ink-2">{a}</th>
                      {names.map((b) => {
                        const v = r.matrix[a][b];
                        const diag = a === b;
                        const intensity = diag ? 0 : v / max;
                        return (
                          <td
                            key={b}
                            title={diag ? `${v} tasks touch ${a}` : `${v} tasks changed ${a} and ${b}`}
                            className="h-10 w-10 border-2 border-surface text-center tnum"
                            style={{
                              background: diag ? "var(--wg-sunken)" : `color-mix(in oklab, var(--wg-accent) ${8 + intensity * 80}%, var(--wg-surface))`,
                              color: intensity > 0.55 ? "var(--wg-accent-ink)" : undefined,
                              borderRadius: 6,
                            }}
                          >
                            {v}
                          </td>
                        );
                      })}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : (
            <Empty title="No repositories" />
          )}
        </Card>
        <Card title="Coordination">
          <div className="grid grid-cols-2 gap-3">
            <Figure name="Tasks with code" value={r.summary.tasks_with_code ?? 0} strong />
            <Figure name="Cross-repository tasks" value={r.summary.cross_repository_tasks ?? 0} strong />
            <Figure name="Repositories per task" value={num(r.summary.average_repositories_per_task, 2)} strong />
            <Figure name="Integration failure rate" value={pct(r.summary.integration_failure_rate)} strong />
            <Figure name="Median elapsed, cross-repo" value={ms(r.summary.median_elapsed_ms_cross_repo)} />
            <Figure name="Median elapsed, single repo" value={ms(r.summary.median_elapsed_ms_single_repo)} />
          </div>
          {r.coupling_candidates.length > 0 && (
            <div className="mt-4 border-t border-line pt-3">
              <div className="text-[14px] font-semibold">Coupling candidates</div>
              <p className="text-[12px] text-ink-3">Evidence, not verdicts.</p>
              <ul className="mt-1 list-disc space-y-1 pl-5 text-[14px]">
                {r.coupling_candidates.map((c) => (
                  <li key={c.observation}>{c.observation}</li>
                ))}
              </ul>
            </div>
          )}
          {r.notes.map((n) => (
            <p key={n} className="mt-3 text-[12px] text-ink-3">
              {n}
            </p>
          ))}
        </Card>
      </div>
      <section>
        <h2 className="mb-2 px-1 text-[17px] font-semibold lg:text-[15px]">Observed relationships</h2>
        {r.edges.length || r.declared_only.length ? (
          <List>
            {r.edges.map((e, i) => (
              <ListRow key={i} onClick={() => setSelected(i === selected ? null : i)} selected={selected === i} trailing={<span />}>
                <div className="font-medium">
                  {e.source_name} ⇄ {e.target_name}
                </div>
                <Meta>
                  <span>{e.co_changes} co-changes</span>
                  <span>Jaccard {num(e.jaccard, 2)}</span>
                  <span>{sentence(e.relationship)}</span>
                  <span className="text-accent">{selected === i ? "Hide evidence" : "Show evidence"}</span>
                </Meta>
                {selected === i && (
                  <ul className="mt-2 space-y-1 text-[13px]">
                    {e.evidence.map((ev) => (
                      <li key={ev.task_key} className="break-words">
                        <Link to={`/tasks/${ev.task_key}`} onClick={(x) => x.stopPropagation()} className="mono font-medium text-accent">
                          {ev.task_key}
                        </Link>{" "}
                        <span className="mono text-[12px] text-ink-2">
                          {Object.entries(ev.commits)
                            .map(([repo, shas]) => `${repo}: ${shas.map((s) => s.slice(0, 8)).join(" ")}`)
                            .join("; ")}
                        </span>
                      </li>
                    ))}
                  </ul>
                )}
              </ListRow>
            ))}
            {r.declared_only.map((d) => (
              <ListRow key={`${d.source_name}-${d.target_name}`}>
                <div className="font-medium">
                  {d.source_name} → {d.target_name}
                </div>
                <Meta>Declared only, no co-changes recorded</Meta>
              </ListRow>
            ))}
          </List>
        ) : (
          <Card>
            <Empty title="No cross-repository changes recorded" />
          </Card>
        )}
      </section>
    </div>
  );
}

interface Finding {
  kind: string;
  title: string;
  severity: string;
  sample_size: number;
  evidence: Record<string, unknown>;
}

const SEVERITY_TONE: Record<string, string> = { high: "tone-failed", medium: "tone-blocked", low: "tone-idle" };

function Failures({ ws, filters }: { ws: string; filters: Filters }) {
  const report = useQuery({ queryKey: ["analytics", ws, "failures"], queryFn: () => get<{ findings: Finding[]; note: string }>(`/ws/${ws}/analytics/failures`) });
  const trend = useMetric(ws, "check_failures", ["period", "kind"], filters);
  const vpass = useMetric(ws, "verification_pass_rate", ["period"], filters);
  return (
    <div className="space-y-4">
      <div className="grid gap-4 lg:grid-cols-2">
        <Card title="Check failures per week">
          <ErrorBanner error={trend.error} />
          {trend.data ? (
            <PeriodSeries
              series={[...new Set(trend.data.rows.map((r) => String(r.kind)))].map((kind) => ({
                name: kind,
                rows: trend.data!.rows.filter((r) => r.kind === kind),
              }))}
            />
          ) : (
            <Skeleton />
          )}
        </Card>
        <Card title="Verification pass rate">{vpass.data ? <PeriodSeries type="line" series={[{ name: "pass rate", rows: vpass.data.rows }]} /> : <Skeleton />}</Card>
      </div>
      <section>
        <h2 className="mb-2 px-1 text-[17px] font-semibold lg:text-[15px]">Failure patterns</h2>
        <ErrorBanner error={report.error} />
        {!report.data ? (
          <Card bodyClass="p-0">
            <Skeleton />
          </Card>
        ) : report.data.findings.length === 0 ? (
          <Card>
            <Empty title="No recurring failure patterns detected" />
          </Card>
        ) : (
          <List testId="findings">
            {report.data.findings.map((f) => (
              <ListRow key={f.kind + f.title}>
                <div className="flex items-start gap-2">
                  <span className={`badge ${SEVERITY_TONE[f.severity] ?? "tone-idle"} mt-0.5`}>{f.severity}</span>
                  <span className="min-w-0 font-medium leading-snug">{f.title}</span>
                </div>
                <Meta>
                  <span>{sentence(f.kind)}</span>
                  <span>n = {f.sample_size}</span>
                  {Array.isArray(f.evidence.tasks) ? (
                    (f.evidence.tasks as string[]).slice(0, 6).map((k) => (
                      <Link key={k} to={`/tasks/${k}`} className="mono text-[12px] font-medium text-accent">
                        {k}
                      </Link>
                    ))
                  ) : (
                    <span className="mono break-all text-[12px]">{JSON.stringify(f.evidence).slice(0, 80)}</span>
                  )}
                </Meta>
              </ListRow>
            ))}
          </List>
        )}
        {report.data && <p className="mt-2 px-1 text-[12px] text-ink-3">{report.data.note}</p>}
      </section>
    </div>
  );
}

function History({ ws, category }: { ws: string; category: string }) {
  const history = useQuery({
    queryKey: ["analytics", ws, "history", category],
    queryFn: () => get<Row[]>(`/ws/${ws}/analytics/history`, { category: category || undefined, limit: 100 }),
  });
  if (history.error) return <ErrorBanner error={history.error} />;
  if (!history.data) return <Skeleton />;
  if (!history.data.length)
    return (
      <Card>
        <Empty title="No accepted tasks yet" />
      </Card>
    );
  return (
    <List testId="history-table">
      {history.data.map((r) => (
        <ListRow key={String(r.key)} to={`/tasks/${r.key}`} tone="done">
          <div className="flex items-baseline gap-2">
            <span className="mono text-[13px] text-ink-3">{String(r.key)}</span>
            <span className="text-[13px] text-ink-3">{sentence(String(r.category ?? ""))}</span>
          </div>
          <div className="mt-0.5 font-medium leading-snug">{String(r.title)}</div>
          <Meta>
            <span>
              {String(r.attempts)} {Number(r.attempts) === 1 ? "attempt" : "attempts"}
            </span>
            <span>Ran {ms(r.execution_ms as number)}</span>
            <span>Elapsed {ms(r.elapsed_ms as number)}</span>
            <span>{String(r.commits)} commits</span>
            <span>Churn {Number(r.churn_insertions ?? 0) + Number(r.churn_deletions ?? 0)}</span>
            <span>Final {r.final_insertions === null ? "—" : Number(r.final_insertions) + Number(r.final_deletions ?? 0)}</span>
            {Number(r.reopen_count) > 0 && <span className="text-warn">Reopened {String(r.reopen_count)}</span>}
          </Meta>
        </ListRow>
      ))}
    </List>
  );
}

function Definitions() {
  const defs = useQuery({ queryKey: ["metric-definitions"], queryFn: () => get<MetricDefinition[]>("/analytics/metrics") });
  if (defs.error) return <ErrorBanner error={defs.error} />;
  if (!defs.data) return <Skeleton />;
  return (
    <List>
      {defs.data.map((d) => (
        <ListRow key={d.id}>
          <div className="mono text-[13px] font-medium">{d.id}</div>
          <p className="mt-1 text-[14px] text-ink">{d.definition}</p>
          {d.caveats.map((c) => (
            <p key={c} className="mt-1 text-[12px] text-ink-3">
              {c}
            </p>
          ))}
          <Meta>
            <span>{d.family}</span>
            <span>{d.statistic}</span>
            <span>{d.unit}</span>
            {d.dimensions.length > 0 && <span>By {d.dimensions.map(label).join(", ")}</span>}
          </Meta>
        </ListRow>
      ))}
    </List>
  );
}
