import { useState } from "react";
import { Link, useParams } from "react-router-dom";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { post } from "../api/client";
import type { Stack, StackIntegrationJobs, StackVerificationJob } from "../api/types";
import { ItemStateBadge, StackStatusBadge, StackTimeline } from "../features/stacks";
import { Button, Card, Empty, ErrorBanner, KeyValue, Loading, Skeleton, Table, td } from "../components/ui";
import { useStack, useStacks } from "../hooks/stacks";
import { useWorkspace } from "../hooks/workspace";
import { ago, dateTime, short } from "../lib/format";
import { ITEM_STATES, countStates, inMergeOrder, parseItemDetail } from "../lib/stacks";

export function StacksPage() {
  const { ws } = useWorkspace();
  const [inactive, setInactive] = useState(false);
  const stacks = useStacks(ws, inactive);
  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h1 className="text-xl font-semibold text-slate-900">Stacks</h1>
        <label className="flex items-center gap-2 text-sm text-slate-600">
          <input type="checkbox" checked={inactive} onChange={(e) => setInactive(e.target.checked)} /> Include landed and
          abandoned
        </label>
      </div>
      <ErrorBanner error={stacks.error} />
      <Card bodyClass="p-0">
        {!stacks.data ? (
          <Skeleton />
        ) : stacks.data.length === 0 ? (
          <div className="p-4">
            <Empty title="No stacks">
              Stack a parent task's children with <span className="mono">workgraph stack create PARENT</span>.
            </Empty>
          </div>
        ) : (
          <Table head={["Parent", "Status", "Repositories", "Tasks by state", "Updated"]} testId="stacks">
            {stacks.data.map((s) => (
              <tr key={s.id} data-testid="stack-row">
                <td className={td}>
                  <Link to={`/stacks/${s.parent_key}`} className="font-medium text-sky-700 hover:underline">
                    <span className="mono">{s.parent_key}</span> {s.parent_title}
                  </Link>
                </td>
                <td className={td}>
                  <StackStatusBadge status={s.status} />
                </td>
                <td className={td}>{s.repositories.map((r) => r.repository_name).join(", ")}</td>
                <td className={td}>
                  <StateCounts stack={s} />
                </td>
                <td className={`${td} text-slate-500`}>{ago(s.updated_at)}</td>
              </tr>
            ))}
          </Table>
        )}
      </Card>
    </div>
  );
}

function StateCounts({ stack }: { stack: Stack }) {
  const counts = countStates(stack.items);
  const present = ITEM_STATES.filter((state) => counts[state] > 0);
  if (!present.length) return <span className="text-slate-400">no tasks</span>;
  return (
    <div className="flex flex-wrap gap-1">
      {present.map((state) => (
        <ItemStateBadge key={state} state={state} count={counts[state]} />
      ))}
    </div>
  );
}

export function StackDetailPage() {
  const { ref = "" } = useParams();
  const { ws } = useWorkspace();
  const client = useQueryClient();
  const stack = useStack(ws, ref);
  const [notice, setNotice] = useState<string | null>(null);
  const invalidate = () => {
    void client.invalidateQueries({ queryKey: ["stacks"] });
    void client.invalidateQueries({ queryKey: ["task"] });
  };
  const integrate = useMutation({
    mutationFn: () => post<StackIntegrationJobs>(`/ws/${ws}/stacks/${ref}/integrate`),
    onSuccess: (out) => {
      setNotice(`Queued integration in ${out.jobs.length} repositor${out.jobs.length === 1 ? "y" : "ies"}.`);
      invalidate();
    },
  });
  const verify = useMutation({
    mutationFn: () => post<StackVerificationJob>(`/ws/${ws}/stacks/${ref}/verify`),
    onSuccess: (out) => {
      setNotice(`Queued verification of the integration tip (job ${short(out.job_id, 8)}).`);
      invalidate();
    },
  });

  if (stack.isLoading) return <Loading />;
  if (!stack.data) return <ErrorBanner error={stack.error ?? new Error(`No stack for ${ref}`)} />;
  const s = stack.data;
  const inactive = s.status === "landed" || s.status === "abandoned";
  const counts = countStates(s.items);
  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="text-sm text-slate-500">
            <Link to="/stacks" className="hover:underline">
              Stacks
            </Link>{" "}
            / <span className="mono">{s.parent_key}</span>
          </div>
          <h1 className="mt-0.5 flex flex-wrap items-center gap-2 text-xl font-semibold text-slate-900">
            <Link to={`/tasks/${s.parent_key}`} className="hover:underline">
              {s.parent_title}
            </Link>
            <StackStatusBadge status={s.status} />
          </h1>
        </div>
        <div className="flex flex-wrap gap-1.5" data-testid="stack-actions">
          <Button busy={integrate.isPending} disabled={inactive} onClick={() => integrate.mutate()}>
            Integrate
          </Button>
          <Button busy={verify.isPending} disabled={inactive} onClick={() => verify.mutate()}>
            Verify
          </Button>
        </div>
      </div>
      <ErrorBanner error={integrate.error ?? verify.error} />
      {notice && !integrate.error && !verify.error && <div className="text-[12px] text-slate-500">{notice}</div>}
      {s.status === "verified" && (
        <div className="rounded-md border border-teal-200 bg-teal-50 px-3 py-2 text-sm text-teal-900" data-testid="stack-landing">
          Verified on the integration tip. Accepting the parent lands it on the default branch:{" "}
          <span className="mono">workgraph task accept {s.parent_key}</span>
        </div>
      )}

      <Card title="Repositories" bodyClass="p-0">
        <Table head={["Repository", "Base", "Integration branch", "Tip"]} testId="stack-repositories">
          {s.repositories.map((r) => (
            <tr key={r.repository_id}>
              <td className={`${td} font-medium`}>{r.repository_name}</td>
              <td className={`${td} mono text-[12px]`}>{short(r.base_sha, 12)}</td>
              <td className={`${td} mono text-[12px]`}>{r.integration_branch}</td>
              <td className={`${td} mono text-[12px]`}>{r.integration_tip ? short(r.integration_tip, 12) : "—"}</td>
            </tr>
          ))}
        </Table>
      </Card>

      <Card title="Tasks in merge order" bodyClass="p-0">
        <div className="flex flex-wrap gap-1.5 px-3 pt-2.5">
          {ITEM_STATES.map((state) => (
            <ItemStateBadge key={state} state={state} count={counts[state]} />
          ))}
        </div>
        {s.items.length === 0 ? (
          <div className="p-4">
            <Empty title="No tasks in this stack" />
          </div>
        ) : (
          <Table head={["Merge order", "Task", "State", "Detail", "Updated"]} testId="stack-items">
            {inMergeOrder(s.items).map((item) => {
              const detail = parseItemDetail(item.detail);
              return (
                <tr key={item.id} data-testid="stack-item">
                  <td className={`${td} mono`}>{item.merge_order ?? "—"}</td>
                  <td className={td}>
                    <Link to={`/tasks/${item.task_key}`} className="font-medium text-sky-700 hover:underline">
                      <span className="mono">{item.task_key}</span> {item.task_title}
                    </Link>
                  </td>
                  <td className={td}>
                    <ItemStateBadge state={item.state} />
                  </td>
                  <td className={`${td} text-[12px] text-slate-600`}>
                    {detail.conflicts.map((c) => (
                      <div key={c.repository}>
                        <span className="font-medium">Conflict in {c.repository}:</span>{" "}
                        <span className="mono">{c.paths.length ? c.paths.join(", ") : "no paths reported"}</span>
                      </div>
                    ))}
                    {detail.ejection && (
                      <div>
                        Ejected at prefix {detail.ejection.position ?? "?"}
                        {detail.ejection.failedChecks.length ? `: ${detail.ejection.failedChecks.join(", ")}` : ""}
                      </div>
                    )}
                    {detail.reason && <div>{detail.reason}</div>}
                  </td>
                  <td className={`${td} text-slate-500`}>{ago(item.updated_at)}</td>
                </tr>
              );
            })}
          </Table>
        )}
      </Card>

      <Card title="Timeline">
        <StackTimeline ws={ws} ref={s.parent_key} />
      </Card>

      <Card title="Stack">
        <KeyValue
          items={[
            ["Created", dateTime(s.created_at)],
            ["Updated", dateTime(s.updated_at)],
            ["Id", <span key="id" className="mono text-[12px]">{s.id}</span>],
          ]}
        />
      </Card>
    </div>
  );
}
