import { useState } from "react";
import { Link, useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { get, patch, post } from "../api/client";
import type { Attempt, Meta, TaskDetail } from "../api/types";
import { Markdown } from "../components/Markdown";
import {
  Button,
  Card,
  ErrorBanner,
  Field,
  KeyValue,
  Loading,
  Modal,
  PriorityBadge,
  StatusBadge,
  Tabs,
  Tag,
  inputBase,
  inputClass,
} from "../components/ui";
import { TaskForm } from "../features/TaskForm";
import {
  AttemptsPanel,
  ChangesPanel,
  ClassificationPanel,
  HistoryPanel,
  InsightsPanel,
  VerificationPanel,
} from "../features/taskPanels";
import { useRepoNames, useTaskIndex } from "../hooks/lookups";
import { useWorkspace } from "../hooks/workspace";
import { ago, dateTime, label, pct } from "../lib/format";

type TabId = "history" | "changes" | "attempts" | "verification" | "classification" | "insights";
const MANUAL_STATUSES = ["backlog", "ready", "blocked", "cancelled"] as const;

/** Actions available for the task's current status (server enforces the rules too). */
export function availableActions(status: string): string[] {
  switch (status) {
    case "ready":
      return ["claim", "run"];
    case "failed":
      return ["claim", "run"];
    case "claimed":
    case "running":
      return ["complete", "release"];
    case "awaiting_verification":
    case "awaiting_review":
      return ["accept", "accept_manual", "reject"];
    default:
      return [];
  }
}

export function TaskDetailPage() {
  const { ref = "" } = useParams();
  const { ws } = useWorkspace();
  const client = useQueryClient();
  const index = useTaskIndex(ws);
  const repoNames = useRepoNames(ws);
  const [tab, setTab] = useState<TabId>("history");
  const [editing, setEditing] = useState(false);
  const [dialog, setDialog] = useState<null | "accept_manual" | "reject" | "complete" | "run">(null);
  const [text, setText] = useState("");
  const [comment, setComment] = useState("");

  const task = useQuery({ queryKey: ["task", ws, ref], queryFn: () => get<TaskDetail>(`/ws/${ws}/tasks/${ref}`) });
  const meta = useQuery({ queryKey: ["meta"], queryFn: () => get<Meta>("/meta") });

  const refresh = () => {
    void client.invalidateQueries({ queryKey: ["task"] });
    void client.invalidateQueries({ queryKey: ["tasks"] });
    void client.invalidateQueries({ queryKey: ["graph"] });
    void client.invalidateQueries({ queryKey: ["attempts"] });
  };
  const action = useMutation({
    mutationFn: async ({ kind, value }: { kind: string; value?: string }) => {
      const base = `/ws/${ws}/tasks/${ref}`;
      switch (kind) {
        case "claim":
          return post(`${base}/claim`, {});
        case "release":
          return post(`${base}/release`, { reason: "released from the web UI" });
        case "complete":
          return post(`${base}/complete`, { summary: value ?? "" });
        case "accept":
          return post(`${base}/accept`, { manual: false, reason: "" });
        case "accept_manual":
          return post(`${base}/accept`, { manual: true, reason: value ?? "" });
        case "reject":
          return post(`${base}/reject`, { reason: value ?? "" });
        case "run":
          return post<Attempt>(`/ws/${ws}/runs`, { task: ref, agent: value || null });
        case "status":
          return patch(base, { status: value, expected_version: task.data?.version, reason: "changed in the web UI" });
        case "archive":
          return post(`${base}/archive`, {});
        default:
          throw new Error(`unknown action ${kind}`);
      }
    },
    onSuccess: () => {
      setDialog(null);
      setText("");
      refresh();
    },
  });
  const addComment = useMutation({
    mutationFn: () => post(`/ws/${ws}/tasks/${ref}/comments`, { body: comment }),
    onSuccess: () => {
      setComment("");
      refresh();
    },
  });

  if (task.isLoading) return <Loading />;
  if (task.error || !task.data) return <ErrorBanner error={task.error} onRetry={() => task.refetch()} />;
  const t = task.data;
  const actions = availableActions(t.status);
  const key = (id: string) => index.get(id)?.key ?? id.slice(0, 8);

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2 text-sm text-slate-500">
            <Link to="/tasks" className="hover:underline">
              Tasks
            </Link>
            <span>/</span>
            <span className="mono">{t.key}</span>
            {t.parent_id && (
              <>
                <span>· parent</span>
                <Link className="mono text-sky-700 hover:underline" to={`/tasks/${key(t.parent_id)}`}>
                  {key(t.parent_id)}
                </Link>
              </>
            )}
          </div>
          <h1 className="mt-0.5 text-xl font-semibold text-slate-900">{t.title}</h1>
          <div className="mt-1 flex flex-wrap items-center gap-2">
            <StatusBadge status={t.status} />
            <PriorityBadge priority={t.priority} />
            {t.category && <Tag>{label(t.category)}</Tag>}
            {t.tags.map((tag) => (
              <Tag key={tag}>#{tag}</Tag>
            ))}
            <span className="text-[12px] text-slate-400">v{t.version}</span>
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-1.5" data-testid="task-actions">
          {actions.includes("claim") && (
            <Button onClick={() => action.mutate({ kind: "claim" })} busy={action.isPending}>
              Claim
            </Button>
          )}
          {actions.includes("run") && (
            <Button onClick={() => setDialog("run")} disabled={!meta.data?.execution_enabled} title={meta.data?.execution_enabled ? "" : "agent execution is disabled in settings"}>
              Run agent
            </Button>
          )}
          {actions.includes("complete") && <Button onClick={() => setDialog("complete")}>Complete</Button>}
          {actions.includes("release") && (
            <Button variant="ghost" onClick={() => action.mutate({ kind: "release" })}>
              Release
            </Button>
          )}
          {actions.includes("accept") && (
            <Button variant="primary" onClick={() => action.mutate({ kind: "accept" })} busy={action.isPending}>
              Accept (verified)
            </Button>
          )}
          {actions.includes("accept_manual") && <Button onClick={() => setDialog("accept_manual")}>Accept manually</Button>}
          {actions.includes("reject") && (
            <Button variant="danger" onClick={() => setDialog("reject")}>
              Reject
            </Button>
          )}
          <select
            aria-label="Change status"
            className={`${inputBase} w-36`}
            value=""
            onChange={(e) => e.target.value && action.mutate({ kind: "status", value: e.target.value })}
          >
            <option value="">move to…</option>
            {MANUAL_STATUSES.filter((s) => s !== t.status).map((s) => (
              <option key={s} value={s}>
                {label(s)}
              </option>
            ))}
          </select>
          <Button variant="ghost" onClick={() => setEditing(true)}>
            Edit
          </Button>
        </div>
      </div>
      <ErrorBanner error={action.error} />
      <div className="grid gap-4 xl:grid-cols-3">
        <div className="space-y-4 xl:col-span-2">
          <Card title="Description">
            <Markdown source={t.description} />
            {t.acceptance_criteria.length > 0 && (
              <div className="mt-3">
                <div className="text-[12px] font-semibold uppercase tracking-wide text-slate-500">Acceptance criteria</div>
                <ul className="mt-1 list-disc pl-5">
                  {t.acceptance_criteria.map((c) => (
                    <li key={c}>{c}</li>
                  ))}
                </ul>
              </div>
            )}
          </Card>
          <div>
            <Tabs<TabId>
              value={tab}
              onChange={setTab}
              tabs={[
                ["history", "History"],
                ["changes", "Commits & changes"],
                ["attempts", "Attempts"],
                ["verification", "Verification"],
                ["classification", "Classification"],
                ["insights", "Similar & estimate"],
              ]}
            />
            <div className="pt-3">
              {tab === "history" && <HistoryPanel ws={ws} taskKey={t.key} />}
              {tab === "changes" && <ChangesPanel ws={ws} taskKey={t.key} repoNames={repoNames} />}
              {tab === "attempts" && <AttemptsPanel ws={ws} taskKey={t.key} />}
              {tab === "verification" && <VerificationPanel ws={ws} taskKey={t.key} />}
              {tab === "classification" && <ClassificationPanel ws={ws} taskKey={t.key} />}
              {tab === "insights" && <InsightsPanel ws={ws} taskKey={t.key} />}
            </div>
          </div>
        </div>
        <div className="space-y-4">
          <Card title="Details">
            <KeyValue
              items={[
                ["assignee", t.assignee],
                ["claimed by", t.claimed_by],
                ["agent", t.assigned_agent],
                ["repositories", t.repository_ids.map((id) => repoNames.get(id) ?? id.slice(0, 6)).join(", ")],
                ["created", <span title={dateTime(t.created_at)}>{ago(t.created_at)} by {t.created_by}</span>],
                ["started", t.started_at ? ago(t.started_at) : null],
                ["completed", t.completed_at ? ago(t.completed_at) : null],
                ["accepted", t.accepted_at ? ago(t.accepted_at) : null],
              ]}
            />
          </Card>
          <Card title="Dependencies">
            {t.dependency_state && !t.dependency_state.satisfied && (
              <div className="mb-2 rounded bg-amber-50 px-2 py-1 text-[12px] text-amber-800">
                Blocked by {t.dependency_state.unsatisfied.map((u) => u.key).join(", ")}
              </div>
            )}
            <KeyValue
              items={[
                ["depends on", t.depends_on.length ? <TaskLinks ids={t.depends_on} keyOf={key} /> : "none"],
                ["blocks", t.blocks.length ? <TaskLinks ids={t.blocks} keyOf={key} /> : "none"],
                ["children", t.child_ids.length ? <TaskLinks ids={t.child_ids} keyOf={key} /> : null],
                ["relations", t.relations.length ? t.relations.map((r) => `${label(r.relation_type)} ${key(r.source_task_id === t.id ? r.target_task_id : r.source_task_id)}`).join(", ") : null],
              ]}
            />
            {t.rollup && (
              <div className="mt-2">
                <div className="flex justify-between text-[12px] text-slate-600">
                  <span>
                    {t.rollup.accepted}/{t.rollup.total_descendants} descendants accepted
                  </span>
                  <span>{pct(t.rollup.progress)}</span>
                </div>
                <div className="mt-1 h-1.5 rounded bg-slate-200">
                  <div className="h-1.5 rounded bg-emerald-500" style={{ width: `${t.rollup.progress * 100}%` }} />
                </div>
              </div>
            )}
          </Card>
          <Card title={`Comments (${t.comments.length})`}>
            <div className="max-h-96 space-y-2 overflow-y-auto">
              {t.comments.map((c) => (
                <div key={c.id} className="rounded border border-slate-100 bg-slate-50 p-2">
                  <div className="mb-0.5 flex justify-between text-[11px] text-slate-500">
                    <span>
                      {c.author} · {c.kind}
                    </span>
                    <span>{ago(c.created_at)}</span>
                  </div>
                  <Markdown source={c.body} />
                </div>
              ))}
            </div>
            <form
              className="mt-2 space-y-1.5"
              onSubmit={(e) => {
                e.preventDefault();
                if (comment.trim()) addComment.mutate();
              }}
            >
              <textarea aria-label="New comment" className={`${inputClass} h-16`} value={comment} onChange={(e) => setComment(e.target.value)} placeholder="Add a comment (Markdown)" />
              <ErrorBanner error={addComment.error} />
              <Button type="submit" size="sm" busy={addComment.isPending} disabled={!comment.trim()}>
                Comment
              </Button>
            </form>
          </Card>
        </div>
      </div>
      <Modal title={`Edit ${t.key}`} open={editing} onClose={() => setEditing(false)} wide>
        <TaskForm
          task={t}
          onDone={() => {
            setEditing(false);
            refresh();
          }}
        />
      </Modal>
      <Modal title={dialogTitle(dialog)} open={dialog !== null} onClose={() => setDialog(null)}>
        <form
          className="space-y-3"
          onSubmit={(e) => {
            e.preventDefault();
            if (dialog) action.mutate({ kind: dialog, value: text });
          }}
        >
          {dialog === "run" ? (
            <Field label="Agent adapter" hint="Leave empty for the configured default">
              <select className={inputClass} value={text} onChange={(e) => setText(e.target.value)}>
                <option value="">default</option>
                {meta.data?.agents.map((a) => (
                  <option key={a}>{a}</option>
                ))}
              </select>
            </Field>
          ) : (
            <Field label={dialog === "complete" ? "Summary" : "Reason"}>
              <textarea className={`${inputClass} h-20`} value={text} onChange={(e) => setText(e.target.value)} required={dialog !== "complete"} />
            </Field>
          )}
          {dialog === "accept_manual" && <p className="text-[12px] text-slate-500">The decision and reason are recorded permanently in the task history.</p>}
          <ErrorBanner error={action.error} />
          <div className="flex justify-end">
            <Button type="submit" variant={dialog === "reject" ? "danger" : "primary"} busy={action.isPending}>
              {dialogTitle(dialog)}
            </Button>
          </div>
        </form>
      </Modal>
    </div>
  );
}

function dialogTitle(dialog: string | null): string {
  return { accept_manual: "Accept manually", reject: "Reject", complete: "Report completion", run: "Start agent run" }[dialog ?? ""] ?? "";
}

function TaskLinks({ ids, keyOf }: { ids: string[]; keyOf: (id: string) => string }) {
  return (
    <span className="flex flex-wrap gap-1.5">
      {ids.map((id) => (
        <Link key={id} to={`/tasks/${keyOf(id)}`} className="mono text-[12px] text-sky-700 hover:underline">
          {keyOf(id)}
        </Link>
      ))}
    </span>
  );
}
