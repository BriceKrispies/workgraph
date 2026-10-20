import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { get } from "../api/client";
import { TASK_STATUSES, type EventOut, type Project, type Repository, type Row, type WorkspaceSummary } from "../api/types";
import { Button, Card, Empty, ErrorBanner, Meta, PageHeader, Skeleton, Stat, StatusBadge } from "../components/ui";
import { CategoryBars, Distribution, PeriodSeries } from "../features/charts";
import { useWorkspace } from "../hooks/workspace";
import { ago, label, pct, sentence } from "../lib/format";
import { STRUCK, toneOf } from "../lib/tone";

export function describeEvent(e: EventOut): string {
  const d = e.data as Record<string, unknown>;
  if (typeof d.from === "string" && typeof d.to === "string") {
    return `${label(d.from)} → ${label(d.to)}${typeof d.reason === "string" && d.reason ? ` (${d.reason})` : ""}`;
  }
  if (typeof d.key === "string" && typeof d.title === "string") return `${d.key}: ${d.title}`;
  if (typeof d.message === "string" && d.message) return d.message;
  if (d.changes && typeof d.changes === "object") return `changed ${Object.keys(d.changes).join(", ")}`;
  if (typeof d.claimed_by === "string") return `claimed by ${d.claimed_by}`;
  if (typeof d.summary === "string" && d.summary) return d.summary;
  if (typeof d.excerpt === "string" && d.excerpt) return d.excerpt;
  if (typeof d.task_key === "string") return d.task_key;
  return Object.entries(d)
    .filter(([k, v]) => !k.startsWith("_") && (typeof v === "string" || typeof v === "number"))
    .slice(0, 3)
    .map(([k, v]) => `${k}=${String(v)}`)
    .join(" ");
}

export function OverviewPage() {
  const { ws } = useWorkspace();
  const [allActivity, setAllActivity] = useState(false);
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
  const byStatus = s?.tasks_by_status ?? {};
  const total = Object.values(byStatus).reduce((a, b) => a + b, 0);
  const review = (byStatus.awaiting_review ?? 0) + (byStatus.awaiting_verification ?? 0);
  const shownEvents = allActivity ? events.data : events.data?.slice(0, 8);

  return (
    <div>
      <PageHeader title="Overview">
        {s?.workspace.description && <p className="max-w-prose text-[15px] text-ink-2">{s.workspace.description}</p>}
      </PageHeader>
      <ErrorBanner error={summary.error} onRetry={() => summary.refetch()} />

      {s ? (
        <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
          <Stat label="Ready" value={s.ready_tasks} tone="ready" to="/tasks?status=ready" />
          <Stat label="Awaiting review" value={review} tone="review" to="/tasks?status=awaiting_review" />
          <Stat label="Running attempts" value={s.running_attempts} tone="active" to="/execution" hint={`${s.queued_jobs} queued jobs`} />
          <Stat
            label="Dead jobs"
            value={s.dead_jobs}
            tone={s.dead_jobs ? "failed" : "idle"}
            to="/automation?tab=jobs"
            hint={s.dead_jobs ? "Retry or cancel them" : "None waiting"}
          />
        </div>
      ) : (
        <Skeleton rows={2} />
      )}

      <div className="mt-4 grid gap-4 lg:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]">
        <div className="min-w-0 space-y-4">
          {s && (
            <Card
              title="Lifecycle"
              actions={
                <span className="text-[13px] text-ink-3">
                  <span className="font-semibold text-ink tnum" data-testid="stat-Tasks">
                    {total}
                  </span>{" "}
                  tasks
                </span>
              }
            >
              <Lifecycle counts={byStatus} total={total} />
            </Card>
          )}

          <Card title="Recent activity" bodyClass="p-0">
            {events.data?.length ? (
              <>
                <ol className="divide-y divide-line" data-testid="recent-activity">
                  {shownEvents!.map((e) => (
                    <li key={e.id} className="px-4 py-3">
                      <div className="break-words text-[14px] text-ink">{describeEvent(e) || e.event_type}</div>
                      <Meta>
                        <span className="mono text-[12px]">{e.event_type}</span>
                        <span>{e.actor}</span>
                        <span className="ml-auto">{ago(e.occurred_at)}</span>
                      </Meta>
                    </li>
                  ))}
                </ol>
                {events.data.length > 8 && (
                  <div className="border-t border-line p-2">
                    <Button variant="ghost" size="sm" className="w-full" onClick={() => setAllActivity((v) => !v)}>
                      {allActivity ? "Show less" : `Show all ${events.data.length}`}
                    </Button>
                  </div>
                )}
              </>
            ) : events.isLoading ? (
              <Skeleton />
            ) : (
              <Empty title="No activity yet">Events appear as tasks are created and worked on.</Empty>
            )}
          </Card>

          <Card title="Throughput per week">
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
        </div>

        <div className="min-w-0 space-y-4">
          <Card title="Execution">
            {overview.data ? (
              <div className="space-y-5">
                <div>
                  <div className="mb-2 text-[13px] font-medium text-ink-2">Attempt duration</div>
                  <Distribution row={overview.data.execution_duration[0]} unit="ms" />
                </div>
                <div>
                  <div className="mb-2 text-[13px] font-medium text-ink-2">Creation to acceptance</div>
                  <Distribution row={overview.data.task_elapsed[0]} unit="ms" />
                </div>
                <div className="flex items-baseline justify-between border-t border-line pt-3 text-[14px]">
                  <span className="text-ink-2">First-pass success</span>
                  <span>
                    <span className="font-semibold tnum">{pct(overview.data.first_pass_success[0]?.value)}</span>{" "}
                    <span className="text-[12px] text-ink-3">n = {overview.data.first_pass_success[0]?.n ?? 0}</span>
                  </span>
                </div>
              </div>
            ) : (
              <Skeleton />
            )}
          </Card>

          <Card
            title="Repositories"
            bodyClass="p-0"
            actions={
              s && (
                <span className="text-[13px] text-ink-3">
                  <span className="font-semibold text-ink tnum" data-testid="stat-Repositories">
                    {s.repositories}
                  </span>{" "}
                  registered
                </span>
              )
            }
          >
            {repos.data?.length ? (
              <ul className="divide-y divide-line">
                {repos.data.map((r) => (
                  <li key={r.id}>
                    <Link to={`/repositories/${r.name}`} className="flex min-h-12 items-center justify-between gap-2 px-4 py-2 hover:bg-sunken">
                      <span className="truncate font-medium">{r.name}</span>
                      <StatusBadge status={r.health_status} />
                    </Link>
                  </li>
                ))}
              </ul>
            ) : repos.isLoading ? (
              <Skeleton />
            ) : (
              <Empty title="No repositories">
                Register one with <span className="mono">workgraph repo register PATH</span>.
              </Empty>
            )}
          </Card>

          <Card title="Projects" bodyClass="p-0">
            {projects.data?.length ? (
              <ul className="divide-y divide-line">
                {projects.data.map((p) => (
                  <li key={p.id}>
                    <Link to={`/tasks?project=${p.slug}`} className="flex min-h-12 items-center justify-between gap-2 px-4 py-2 hover:bg-sunken">
                      <span className="truncate font-medium">{p.name}</span>
                      <span className="shrink-0 text-[13px] text-ink-3">
                        {p.repository_ids.length} {p.repository_ids.length === 1 ? "repo" : "repos"}
                      </span>
                    </Link>
                  </li>
                ))}
              </ul>
            ) : projects.isLoading ? (
              <Skeleton />
            ) : (
              <Empty title="No projects">
                Create one with <span className="mono">workgraph project create</span>.
              </Empty>
            )}
          </Card>

          <Card title="Tasks by category">
            {overview.data ? <CategoryBars rows={overview.data.by_category} category="category" /> : <Skeleton />}
          </Card>
        </div>
      </div>
    </div>
  );
}

/** One bar for the whole workspace, in lifecycle order, with a tappable legend. */
function Lifecycle({ counts, total }: { counts: Record<string, number>; total: number }) {
  if (!total) return <Empty title="No tasks yet">Create one from the Tasks tab.</Empty>;
  const present = [...TASK_STATUSES.filter((st) => counts[st]), ...Object.keys(counts).filter((st) => !TASK_STATUSES.includes(st as never) && counts[st])];
  return (
    <div>
      <div className="flex h-3 gap-[2px] overflow-hidden rounded-full" role="img" aria-label="Tasks by status">
        {present.map((st) => (
          <span key={st} className={`tone-${toneOf(st)}`} style={{ flexGrow: counts[st], background: "var(--t)", minWidth: 4, opacity: STRUCK.has(st) ? 0.35 : 1 }} />
        ))}
      </div>
      <ul className="mt-3 grid grid-cols-2 gap-x-4 sm:grid-cols-3 xl:grid-cols-4">
        {present.map((st) => (
          <li key={st}>
            <Link to={`/tasks?status=${st}`} className="flex min-h-9 items-center gap-2 rounded-lg text-[14px] hover:text-accent">
              <span className={`dot tone-${toneOf(st)}`} style={{ opacity: STRUCK.has(st) ? 0.35 : 1 }} />
              <span className="min-w-0 flex-1 truncate text-ink-2">{sentence(st)}</span>
              <span className="font-semibold tnum">{counts[st]}</span>
            </Link>
          </li>
        ))}
      </ul>
    </div>
  );
}
