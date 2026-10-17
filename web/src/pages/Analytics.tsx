import { useState } from "react";
import { Link } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { get, post } from "../api/client";
import { CATEGORIES, type AnalyticsResult, type MetricDefinition, type Project, type Row } from "../api/types";
import { Card, Empty, ErrorBanner, Skeleton, Table, Tabs, inputBase, td } from "../components/ui";
import { CategoryBars, Distribution, PeriodSeries } from "../features/charts";
import { useWorkspace } from "../hooks/workspace";
import { label, ms, num, pct } from "../lib/format";

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
}: {
  title: string;
  result: { data?: AnalyticsResult; error: unknown; isLoading: boolean };
  children: (rows: Row[]) => React.ReactNode;
}) {
  return (
    <Card title={title}>
      <ErrorBanner error={result.error} />
      {result.isLoading ? <Skeleton /> : result.data ? children(result.data.rows) : null}
      {result.data && (
        <p className="mt-2 text-[11px] text-slate-400" title={result.data.provenance.sql}>
          {result.data.metric.definition} {result.data.metric.caveats.join(" ")}
        </p>
      )}
    </Card>
  );
}

export function AnalyticsPage() {
  const { ws } = useWorkspace();
  const [tab, setTab] = useState<TabId>("throughput");
  const [filters, setFilters] = useState<Filters>({ since: "", until: "", category: "", project: "" });
  const projects = useQuery({ queryKey: ["projects", ws], queryFn: () => get<Project[]>(`/ws/${ws}/projects`) });
  const set = (key: keyof Filters, value: string) => setFilters((f) => ({ ...f, [key]: value }));

  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-end justify-between gap-2">
        <h1 className="text-xl font-semibold text-slate-900">Analytics</h1>
        <div className="flex flex-wrap items-end gap-2 text-sm">
          <label className="text-[12px] text-slate-500">
            since
            <input type="date" aria-label="since" className={`${inputBase} w-36`} value={filters.since} onChange={(e) => set("since", e.target.value)} />
          </label>
          <label className="text-[12px] text-slate-500">
            until
            <input type="date" aria-label="until" className={`${inputBase} w-36`} value={filters.until} onChange={(e) => set("until", e.target.value)} />
          </label>
          <select aria-label="Category" className={`${inputBase} w-36`} value={filters.category} onChange={(e) => set("category", e.target.value)}>
            <option value="">any category</option>
            {CATEGORIES.map((c) => (
              <option key={c} value={c}>
                {label(c)}
              </option>
            ))}
          </select>
          <select aria-label="Project" className={`${inputBase} w-36`} value={filters.project} onChange={(e) => set("project", e.target.value)}>
            <option value="">any project</option>
            {projects.data?.map((p) => (
              <option key={p.id} value={p.slug}>
                {p.name}
              </option>
            ))}
          </select>
        </div>
      </div>
      <p className="text-[12px] text-slate-500">
        Measured from recorded tasks, attempts, checks and commits. Groups differ in the work they received — differences are associations, not causes.
      </p>
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
      {tab === "throughput" && <Throughput ws={ws} filters={filters} />}
      {tab === "execution" && <Execution ws={ws} filters={filters} />}
      {tab === "agents" && <Agents ws={ws} filters={filters} />}
      {tab === "code" && <Code ws={ws} filters={filters} />}
      {tab === "coupling" && <Coupling ws={ws} project={filters.project} />}
      {tab === "failures" && <Failures ws={ws} filters={filters} />}
      {tab === "history" && <History ws={ws} category={filters.category} />}
      {tab === "definitions" && <Definitions />}
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
      <Card title="Tasks per week" className="lg:col-span-2">
        <ErrorBanner error={created.error ?? completed.error ?? accepted.error} />
        {created.data && completed.data && accepted.data ? (
          <PeriodSeries
            height={220}
            series={[
              { name: "created", rows: created.data.rows },
              { name: "completed (claimed)", rows: completed.data.rows },
              { name: "accepted", rows: accepted.data.rows },
            ]}
          />
        ) : (
          <Skeleton />
        )}
        <p className="mt-1 text-[11px] text-slate-400">Completion is an agent's or person's claim; acceptance requires verification or a recorded decision.</p>
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
            <div className="text-2xl font-semibold">{pct(rows[0].value)}</div>
            <div className="text-[12px] text-slate-500">
              {rows[0].numerator} of {rows[0].denominator}
            </div>
          </div>
        ) : (
          <span className="text-slate-400">no samples</span>
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
      <Card title="Agent / model comparison" bodyClass="p-0">
        <ErrorBanner error={completion.error} />
        {!completion.data ? (
          <Skeleton />
        ) : completion.data.rows.length === 0 ? (
          <div className="p-3">
            <Empty title="No finished attempts" />
          </div>
        ) : (
          <Table head={["Provider", "Model", "Succeeded", "Completion rate", "Median duration", "p90 duration", "Verification pass"]} testId="agent-comparison">
            {completion.data.rows.map((r) => {
              const d = durations.get(key(r));
              const v = verifications.get(key(r));
              return (
                <tr key={key(r)}>
                  <td className={td}>{r.provider}</td>
                  <td className={td}>{r.model ?? "(unspecified)"}</td>
                  <td className={td}>
                    {r.numerator}/{r.denominator}
                  </td>
                  <td className={`${td} font-medium`}>{pct(r.value)}</td>
                  <td className={td}>{ms(d?.median)}</td>
                  <td className={td}>{ms(d?.p90)}</td>
                  <td className={td}>{v ? `${pct(v.value)} (${v.numerator}/${v.denominator})` : "—"}</td>
                </tr>
              );
            })}
          </Table>
        )}
        <p className="p-3 text-[11px] text-slate-400">{completion.data?.metric.caveats.join(" ")}</p>
      </Card>
      <div className="grid gap-4 lg:grid-cols-2">
        <MetricCard title="Completion rate by agent and category" result={byCategory}>
          {(rows) => (
            <Table head={["Provider", "Category", "Rate", "n"]}>
              {rows.map((r) => (
                <tr key={key(r)}>
                  <td className={td}>{r.provider}</td>
                  <td className={td}>{label(String(r.category ?? ""))}</td>
                  <td className={td}>{pct(r.value)}</td>
                  <td className={td}>{r.denominator}</td>
                </tr>
              ))}
            </Table>
          )}
        </MetricCard>
        <Card title="Usage (measured only)">
          <Table head={["Provider", "Input tokens", "Output tokens", "Cost", "Attempts measured"]}>
            {(tokensIn.data?.rows ?? []).map((r) => {
              const out = tokensOut.data?.rows.find((o) => o.provider === r.provider);
              const c = cost.data?.rows.find((o) => o.provider === r.provider);
              return (
                <tr key={String(r.provider)}>
                  <td className={td}>{r.provider}</td>
                  <td className={td}>{num(r.value)}</td>
                  <td className={td}>{num(out?.value)}</td>
                  <td className={td}>{c && Number(c.n) > 0 ? `$${Number(c.value).toFixed(4)}` : "not measured"}</td>
                  <td className={td}>{r.n}</td>
                </tr>
              );
            })}
          </Table>
          {!tokensIn.data?.rows.length && <Empty title="No measured usage">Providers that do not report usage are never estimated.</Empty>}
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
        <Card title="Co-change matrix (tasks touching both)">
          {names.length ? (
            <table className="text-[12px]" data-testid="coupling-matrix">
              <thead>
                <tr>
                  <th />
                  {names.map((n) => (
                    <th key={n} className="px-1 pb-1 text-left font-medium text-slate-500" style={{ writingMode: "vertical-rl" }}>
                      {n}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {names.map((a) => (
                  <tr key={a}>
                    <th className="pr-2 text-right font-medium text-slate-600">{a}</th>
                    {names.map((b) => {
                      const v = r.matrix[a][b];
                      const diag = a === b;
                      const intensity = diag ? 0 : v / max;
                      return (
                        <td
                          key={b}
                          title={diag ? `${v} tasks touch ${a}` : `${v} tasks changed ${a} and ${b}`}
                          className="h-9 w-9 border border-white text-center tabular-nums"
                          style={{ background: diag ? "#f1f5f9" : `rgba(14,165,233,${0.08 + intensity * 0.8})`, color: intensity > 0.6 ? "white" : undefined }}
                        >
                          {v}
                        </td>
                      );
                    })}
                  </tr>
                ))}
              </tbody>
            </table>
          ) : (
            <Empty title="No repositories" />
          )}
        </Card>
        <Card title="Coordination">
          <div className="grid grid-cols-2 gap-2 text-[13px]">
            <div>Tasks with code: <b>{r.summary.tasks_with_code ?? 0}</b></div>
            <div>Cross-repository tasks: <b>{r.summary.cross_repository_tasks ?? 0}</b></div>
            <div>Avg repos / task: <b>{num(r.summary.average_repositories_per_task, 2)}</b></div>
            <div>Integration failure rate: <b>{pct(r.summary.integration_failure_rate)}</b></div>
            <div>Median elapsed (cross-repo): <b>{ms(r.summary.median_elapsed_ms_cross_repo)}</b></div>
            <div>Median elapsed (single repo): <b>{ms(r.summary.median_elapsed_ms_single_repo)}</b></div>
          </div>
          {r.coupling_candidates.length > 0 && (
            <div className="mt-3">
              <div className="text-[12px] font-semibold text-slate-600">Coupling candidates (evidence, not verdicts)</div>
              <ul className="list-disc pl-5 text-[12px]">
                {r.coupling_candidates.map((c) => (
                  <li key={c.observation}>{c.observation}</li>
                ))}
              </ul>
            </div>
          )}
          {r.notes.map((n) => (
            <p key={n} className="mt-2 text-[11px] text-slate-400">
              {n}
            </p>
          ))}
        </Card>
      </div>
      <Card title="Observed relationships (click to trace evidence)" bodyClass="p-0">
        {r.edges.length ? (
          <Table head={["Repositories", "Co-changes", "Jaccard", "Relationship"]}>
            {r.edges.map((e, i) => (
              <tr key={i} className="cursor-pointer hover:bg-slate-50" onClick={() => setSelected(i === selected ? null : i)}>
                <td className={td}>
                  {e.source_name} ⇄ {e.target_name}
                  {selected === i && (
                    <ul className="mt-1 space-y-0.5 text-[12px]">
                      {e.evidence.map((ev) => (
                        <li key={ev.task_key}>
                          <Link to={`/tasks/${ev.task_key}`} className="mono text-sky-700">
                            {ev.task_key}
                          </Link>{" "}
                          {Object.entries(ev.commits)
                            .map(([repo, shas]) => `${repo}: ${shas.map((s) => s.slice(0, 8)).join(" ")}`)
                            .join(" · ")}
                        </li>
                      ))}
                    </ul>
                  )}
                </td>
                <td className={td}>{e.co_changes}</td>
                <td className={td}>{num(e.jaccard, 2)}</td>
                <td className={td}>{label(e.relationship)}</td>
              </tr>
            ))}
            {r.declared_only.map((d) => (
              <tr key={`${d.source_name}-${d.target_name}`}>
                <td className={td}>
                  {d.source_name} → {d.target_name}
                </td>
                <td className={td}>0</td>
                <td className={td}>—</td>
                <td className={td}>declared only</td>
              </tr>
            ))}
          </Table>
        ) : (
          <div className="p-3">
            <Empty title="No cross-repository changes recorded" />
          </div>
        )}
      </Card>
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
        <Card title="Verification pass rate">
          {vpass.data ? <PeriodSeries type="line" series={[{ name: "pass rate", rows: vpass.data.rows }]} /> : <Skeleton />}
        </Card>
      </div>
      <Card title="Failure patterns" bodyClass="p-0">
        <ErrorBanner error={report.error} />
        {!report.data ? (
          <Skeleton />
        ) : report.data.findings.length === 0 ? (
          <div className="p-3">
            <Empty title="No recurring failure patterns detected" />
          </div>
        ) : (
          <Table head={["Severity", "Finding", "Kind", "n", "Evidence"]} testId="findings">
            {report.data.findings.map((f) => (
              <tr key={f.kind + f.title}>
                <td className={td}>{f.severity}</td>
                <td className={td}>{f.title}</td>
                <td className={`${td} text-slate-500`}>{label(f.kind)}</td>
                <td className={td}>{f.sample_size}</td>
                <td className={`${td} text-[12px]`}>
                  {Array.isArray(f.evidence.tasks)
                    ? (f.evidence.tasks as string[]).slice(0, 6).map((k) => (
                        <Link key={k} to={`/tasks/${k}`} className="mono mr-1 text-sky-700">
                          {k}
                        </Link>
                      ))
                    : JSON.stringify(f.evidence).slice(0, 80)}
                </td>
              </tr>
            ))}
          </Table>
        )}
        {report.data && <p className="p-3 text-[11px] text-slate-400">{report.data.note}</p>}
      </Card>
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
  if (!history.data.length) return <Empty title="No accepted tasks yet" />;
  return (
    <Card title="Historical accepted tasks" bodyClass="p-0">
      <Table head={["Task", "Category", "Attempts", "Execution", "Elapsed", "Commits", "Churn", "Final", "Reopened"]} testId="history-table">
        {history.data.map((r) => (
          <tr key={String(r.key)}>
            <td className={td}>
              <Link to={`/tasks/${r.key}`} className="mono text-[12px] text-sky-700">
                {r.key}
              </Link>{" "}
              {r.title}
            </td>
            <td className={td}>{label(String(r.category ?? ""))}</td>
            <td className={td}>{r.attempts}</td>
            <td className={td}>{ms(r.execution_ms)}</td>
            <td className={td}>{ms(r.elapsed_ms)}</td>
            <td className={td}>{r.commits}</td>
            <td className={td}>{Number(r.churn_insertions ?? 0) + Number(r.churn_deletions ?? 0)}</td>
            <td className={td}>{r.final_insertions === null ? "—" : Number(r.final_insertions) + Number(r.final_deletions ?? 0)}</td>
            <td className={td}>{r.reopen_count}</td>
          </tr>
        ))}
      </Table>
    </Card>
  );
}

function Definitions() {
  const defs = useQuery({ queryKey: ["metric-definitions"], queryFn: () => get<MetricDefinition[]>("/analytics/metrics") });
  if (defs.error) return <ErrorBanner error={defs.error} />;
  if (!defs.data) return <Skeleton />;
  return (
    <Card title="Metric definitions" bodyClass="p-0">
      <Table head={["Metric", "Family", "Statistic", "Unit", "Definition", "Dimensions"]}>
        {defs.data.map((d) => (
          <tr key={d.id}>
            <td className={`${td} mono text-[12px]`}>{d.id}</td>
            <td className={td}>{d.family}</td>
            <td className={td}>{d.statistic}</td>
            <td className={td}>{d.unit}</td>
            <td className={td}>
              {d.definition}
              {d.caveats.map((c) => (
                <div key={c} className="text-[11px] text-slate-400">
                  {c}
                </div>
              ))}
            </td>
            <td className={`${td} text-[11px] text-slate-500`}>{d.dimensions.join(", ")}</td>
          </tr>
        ))}
      </Table>
    </Card>
  );
}
