import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { get } from "../api/client";
import type { EventOut, Project, Repository, Row, WorkspaceSummary } from "../api/types";
import { Card, Empty, ErrorBanner, Skeleton, Stat, StatusBadge, Table, td } from "../components/ui";
import { CategoryBars, Distribution, PeriodSeries } from "../features/charts";
import { useWorkspace } from "../hooks/workspace";
import { ago, label, pct } from "../lib/format";

export function describeEvent(e: EventOut): string {
  const d = e.data as Record<string, unknown>;
  if (typeof d.from === "string" && typeof d.to === "string") {
    return `${label(d.from)} → ${label(d.to)}${typeof d.reason === "string" && d.reason ? ` (${d.reason})` : ""}`;
  }
  if (typeof d.key === "string" && typeof d.title === "string") return `${d.key}: ${d.title}`;
  if (typeof d.message === "string" && d.message) return d.message;
  if (d.changes && typeof d.changes === "object") return `changed ${Object.keys(d.changes).join(", ")}`;
  if (typeof d.task_key === "string") return d.task_key;
  return Object.entries(d)
    .filter(([k, v]) => !k.startsWith("_") && (typeof v === "string" || typeof v === "number"))
    .slice(0, 3)
    .map(([k, v]) => `${k}=${String(v)}`)
    .join(" ");
}

export function OverviewPage() {
  const { ws } = useWorkspace();
  const summary = useQuery({ queryKey: ["summary", ws], queryFn: () => get<WorkspaceSummary>(`/workspaces/${ws}`) });
  const projects = useQuery({ queryKey: ["projects", ws], queryFn: () => get<Project[]>(`/ws/${ws}/projects`) });
  const repos = useQuery({ queryKey: ["repositories", ws], queryFn: () => get<Repository[]>(`/ws/${ws}/repositories`) });
  const events = useQuery({
    queryKey: ["events", ws, "recent"],
    queryFn: () => get<EventOut[]>(`/ws/${ws}/events`, { limit: 25, descending: true }),
  });
  const overview = useQuery({
    queryKey: ["analytics", ws, "overview"],
    queryFn: () => get<Record<string, Row[]>>(`/ws/${ws}/analytics/overview`),
  });

  const s = summary.data;
  const total = s ? Object.values(s.tasks_by_status).reduce((a, b) => a + b, 0) : 0;
  return (
    <div className="space-y-4">
      <div className="flex items-baseline justify-between">
        <h1 className="text-xl font-semibold text-slate-900">{s?.workspace.name ?? ws}</h1>
        <span className="text-sm text-slate-500">{s?.workspace.description}</span>
      </div>
      <ErrorBanner error={summary.error} onRetry={() => summary.refetch()} />
      {s ? (
        <div className="grid grid-cols-2 gap-3 md:grid-cols-4 xl:grid-cols-7">
          <Stat label="Tasks" value={total} />
          <Stat label="Ready" value={s.ready_tasks} />
          <Stat label="Accepted" value={s.tasks_by_status.accepted ?? 0} />
          <Stat label="Running attempts" value={s.running_attempts} />
          <Stat label="Queued jobs" value={s.queued_jobs} />
          <Stat label="Dead jobs" value={s.dead_jobs} hint={s.dead_jobs ? "see Automation › jobs" : undefined} />
          <Stat label="Repositories" value={s.repositories} hint={`${s.projects} projects`} />
        </div>
      ) : (
        <Skeleton rows={2} />
      )}
      {s && total > 0 && (
        <Card title="Tasks by status">
          <div className="flex flex-wrap gap-3">
            {Object.entries(s.tasks_by_status).map(([status, count]) => (
              <Link key={status} to={`/tasks?status=${status}`} className="flex items-center gap-1.5 hover:opacity-80">
                <StatusBadge status={status} /> <span className="font-medium tabular-nums">{count}</span>
              </Link>
            ))}
          </div>
        </Card>
      )}
      <div className="grid gap-4 lg:grid-cols-3">
        <Card title="Throughput (tasks per week)" className="lg:col-span-2">
          <ErrorBanner error={overview.error} />
          {overview.data ? (
            <PeriodSeries
              series={[
                { name: "created", rows: overview.data.created_by_period },
                { name: "completed", rows: overview.data.completed_by_period },
                { name: "accepted", rows: overview.data.accepted_by_period },
              ]}
            />
          ) : (
            !overview.error && <Skeleton />
          )}
        </Card>
        <Card title="Execution">
          {overview.data ? (
            <div className="space-y-3">
              <div>
                <div className="mb-1 text-[12px] font-medium text-slate-600">Attempt duration</div>
                <Distribution row={overview.data.execution_duration[0]} unit="ms" />
              </div>
              <div>
                <div className="mb-1 text-[12px] font-medium text-slate-600">Creation → acceptance</div>
                <Distribution row={overview.data.task_elapsed[0]} unit="ms" />
              </div>
              <div className="text-[12px]">
                First-pass success: <span className="font-semibold">{pct(overview.data.first_pass_success[0]?.value)}</span>{" "}
                <span className="text-slate-400">(n={overview.data.first_pass_success[0]?.n ?? 0})</span>
              </div>
            </div>
          ) : (
            <Skeleton />
          )}
        </Card>
      </div>
      <div className="grid gap-4 lg:grid-cols-3">
        <Card title="Projects">
          {projects.data?.length ? (
            <ul className="space-y-1">
              {projects.data.map((p) => (
                <li key={p.id} className="flex justify-between">
                  <Link to={`/tasks?project=${p.slug}`} className="text-sky-700 hover:underline">
                    {p.name}
                  </Link>
                  <span className="text-slate-500">{p.repository_ids.length} repos</span>
                </li>
              ))}
            </ul>
          ) : projects.isLoading ? (
            <Skeleton />
          ) : (
            <Empty title="No projects">Create one with `workgraph project create`.</Empty>
          )}
        </Card>
        <Card title="Repositories">
          {repos.data?.length ? (
            <ul className="space-y-1">
              {repos.data.map((r) => (
                <li key={r.id} className="flex items-center justify-between gap-2">
                  <Link to={`/repositories/${r.name}`} className="text-sky-700 hover:underline">
                    {r.name}
                  </Link>
                  <StatusBadge status={r.health_status} />
                </li>
              ))}
            </ul>
          ) : repos.isLoading ? (
            <Skeleton />
          ) : (
            <Empty title="No repositories">Register one with `workgraph repo register PATH`.</Empty>
          )}
        </Card>
        <Card title="Tasks by category">
          {overview.data ? <CategoryBars rows={overview.data.by_category} category="category" height={150} /> : <Skeleton />}
        </Card>
      </div>
      <Card title="Recent activity" bodyClass="p-0">
        {events.data?.length ? (
          <Table head={["When", "Event", "Actor", "Details"]} testId="recent-activity">
            {events.data.map((e) => (
              <tr key={e.id}>
                <td className={`${td} whitespace-nowrap text-slate-500`}>{ago(e.occurred_at)}</td>
                <td className={`${td} mono text-[12px]`}>{e.event_type}</td>
                <td className={`${td} text-slate-600`}>{e.actor}</td>
                <td className={`${td} text-slate-600`}>{describeEvent(e)}</td>
              </tr>
            ))}
          </Table>
        ) : events.isLoading ? (
          <Skeleton />
        ) : (
          <div className="p-3">
            <Empty title="No activity yet" />
          </div>
        )}
      </Card>
    </div>
  );
}
