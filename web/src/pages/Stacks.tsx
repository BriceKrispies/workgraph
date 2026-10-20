import { useState } from "react";
import { Link, useParams } from "react-router-dom";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { post } from "../api/client";
import type { Stack, StackIntegrationJobs, StackVerificationJob } from "../api/types";
import { ItemStateBadge, StackStatusBadge, StackTimeline } from "../features/stacks";
import {
  ActionBar,
  Button,
  Card,
  Checkbox,
  Empty,
  ErrorBanner,
  KeyValue,
  List,
  ListRow,
  Loading,
  Meta,
  Notice,
  PageHeader,
  SectionTitle,
  Skeleton,
} from "../components/ui";
import { useStack, useStacks } from "../hooks/stacks";
import { useWorkspace } from "../hooks/workspace";
import { ago, dateTime, short } from "../lib/format";
import { ITEM_STATES, countStates, inMergeOrder, parseItemDetail } from "../lib/stacks";
import { toneOf } from "../lib/tone";

export function StacksPage() {
  const { ws } = useWorkspace();
  const [inactive, setInactive] = useState(false);
  const stacks = useStacks(ws, inactive);
  return (
    <div>
      <PageHeader title="Stacks">
        <p className="max-w-prose text-[15px] text-ink-2">A parent task's children, merged in order onto an integration branch and verified together.</p>
      </PageHeader>
      <div className="mb-2 px-1">
        <Checkbox label="Include landed and abandoned" checked={inactive} onChange={setInactive} />
      </div>
      <ErrorBanner error={stacks.error} />
      {!stacks.data ? (
        <Card bodyClass="p-0">
          <Skeleton />
        </Card>
      ) : stacks.data.length === 0 ? (
        <Card>
          <Empty title="No stacks">
            Stack a parent task's children with <span className="mono">workgraph stack create PARENT</span>.
          </Empty>
        </Card>
      ) : (
        <List testId="stacks">
          {stacks.data.map((s) => (
            <ListRow key={s.id} testId="stack-row" to={`/stacks/${s.parent_key}`} tone={toneOf(s.status)}>
              <div className="flex items-center gap-2">
                <span className="mono text-[13px] text-ink-3">{s.parent_key}</span>
                <StackStatusBadge status={s.status} />
                <span className="ml-auto text-[13px] text-ink-3">{ago(s.updated_at)}</span>
              </div>
              <div className="mt-1 font-medium leading-snug">{s.parent_title}</div>
              <StateCounts stack={s} />
              {s.repositories.length > 0 && <Meta>{s.repositories.map((r) => r.repository_name).join(", ")}</Meta>}
            </ListRow>
          ))}
        </List>
      )}
    </div>
  );
}

function StateCounts({ stack }: { stack: Stack }) {
  const counts = countStates(stack.items);
  const present = ITEM_STATES.filter((state) => counts[state] > 0);
  if (!present.length) return <Meta>No tasks yet</Meta>;
  return (
    <div className="mt-2 flex flex-wrap gap-1.5">
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
  const unfinished = s.items.filter((i) => i.state !== "verified");
  const counts = countStates(s.items);
  return (
    <div>
      <PageHeader
        back={{ to: "/stacks", label: "Stacks" }}
        eyebrow={
          <>
            <Link to={`/tasks/${s.parent_key}`} className="mono text-accent hover:underline">
              {s.parent_key}
            </Link>
            <StackStatusBadge status={s.status} />
          </>
        }
        title={s.parent_title}
      />
      <div className="lg:mb-4">
        <ActionBar testId="stack-actions">
          <Button busy={integrate.isPending} disabled={inactive} onClick={() => integrate.mutate()}>
            Integrate
          </Button>
          <Button variant="primary" busy={verify.isPending} disabled={inactive} onClick={() => verify.mutate()}>
            Verify
          </Button>
        </ActionBar>
      </div>

      <div className="space-y-4">
        <ErrorBanner error={integrate.error ?? verify.error} />
        {notice && !integrate.error && !verify.error && <Notice>{notice}</Notice>}
        {s.status === "verified" && unfinished.length === 0 && (
          <Notice tone="ok" testId="stack-landing">
            Verified on the integration tip. Accepting the parent lands it on the default branch:{" "}
            <span className="mono font-medium">workgraph task accept {s.parent_key}</span>
          </Notice>
        )}
        {s.status === "verified" && unfinished.length > 0 && (
          <Notice tone="warn" testId="stack-landing">
            Verified on the integration tip, but it cannot land while children are unfinished:{" "}
            <span className="mono font-medium">{unfinished.map((i) => `${i.task_key} (${i.state})`).join(", ")}</span>. Finish them or
            remove them from the stack with <span className="mono">workgraph stack remove {s.parent_key} &lt;TASK&gt;</span>.
          </Notice>
        )}

        <div className="grid gap-4 xl:grid-cols-[minmax(0,3fr)_minmax(0,2fr)] xl:items-start">
          <div className="min-w-0 space-y-4">
            <section>
              <SectionTitle>Tasks in merge order</SectionTitle>
              <div className="scroll-x -mx-4 mb-3 flex gap-1.5 px-4">
                {ITEM_STATES.map((state) => (
                  <ItemStateBadge key={state} state={state} count={counts[state]} />
                ))}
              </div>
              {s.items.length === 0 ? (
                <Card>
                  <Empty title="No tasks in this stack" />
                </Card>
              ) : (
                <List testId="stack-items">
                  {inMergeOrder(s.items).map((item) => {
                    const detail = parseItemDetail(item.detail);
                    return (
                      <ListRow key={item.id} testId="stack-item" to={`/tasks/${item.task_key}`} tone={toneOf(item.state)}>
                        <div className="flex items-start gap-3">
                          <span
                            className="mono flex h-7 min-w-7 shrink-0 items-center justify-center rounded-lg bg-sunken px-1 text-[13px] font-medium text-ink-2"
                            data-testid="merge-order"
                            title="Merge order"
                          >
                            {item.merge_order ?? "—"}
                          </span>
                          <div className="min-w-0 flex-1">
                            <div className="flex flex-wrap items-center gap-2">
                              <span className="mono text-[13px] text-ink-3">{item.task_key}</span>
                              <ItemStateBadge state={item.state} />
                              <span className="ml-auto text-[13px] text-ink-3">{ago(item.updated_at)}</span>
                            </div>
                            <div className="mt-0.5 font-medium leading-snug">{item.task_title}</div>
                            <div className="mt-1 space-y-0.5 text-[13px] text-ink-2 empty:hidden">
                              {detail.conflicts.map((c) => (
                                <div key={c.repository}>
                                  <span className="font-medium">Conflict in {c.repository}:</span>{" "}
                                  <span className="mono break-all text-[12px]">{c.paths.length ? c.paths.join(", ") : "no paths reported"}</span>
                                </div>
                              ))}
                              {detail.ejection && (
                                <div>
                                  Ejected at prefix {detail.ejection.position ?? "?"}
                                  {detail.ejection.failedChecks.length ? `: ${detail.ejection.failedChecks.join(", ")}` : ""}
                                </div>
                              )}
                              {detail.reason && <div>{detail.reason}</div>}
                            </div>
                          </div>
                        </div>
                      </ListRow>
                    );
                  })}
                </List>
              )}
            </section>

            <section>
              <SectionTitle>Repositories</SectionTitle>
              <List testId="stack-repositories">
                {s.repositories.map((r) => (
                  <ListRow key={r.repository_id}>
                    <div className="font-medium">{r.repository_name}</div>
                    <dl className="mt-2 grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1 text-[13px]">
                      <dt className="text-ink-3">Base</dt>
                      <dd className="mono truncate">{short(r.base_sha, 12)}</dd>
                      <dt className="text-ink-3">Branch</dt>
                      <dd className="mono truncate">{r.integration_branch}</dd>
                      <dt className="text-ink-3">Tip</dt>
                      <dd className="mono truncate">{r.integration_tip ? short(r.integration_tip, 12) : "—"}</dd>
                    </dl>
                  </ListRow>
                ))}
              </List>
            </section>
          </div>

          <div className="min-w-0 space-y-4">
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
        </div>
      </div>
    </div>
  );
}
