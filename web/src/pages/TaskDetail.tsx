import { useState } from "react";
import { Link, useParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { get, patch, post } from "../api/client";
import type { Attempt, Meta, TaskDetail } from "../api/types";
import { Markdown } from "../components/Markdown";
import {
  ActionBar,
  ActionSheet,
  Button,
  Card,
  ErrorBanner,
  Field,
  IconButton,
  KeyValue,
  Loading,
  Modal,
  Notice,
  PageHeader,
  PriorityBadge,
  StatusBadge,
  Tabs,
  Tag,
  selectClass,
  textareaClass,
} from "../components/ui";
import { TaskForm } from "../features/TaskForm";
import { TaskStackLinks } from "../features/stacks";
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
import { ago, dateTime, label, pct, sentence } from "../lib/format";

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
  const [menu, setMenu] = useState(false);
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
  const canRun = !!meta.data?.execution_enabled;
  const busy = (kind: string) => action.isPending && action.variables?.kind === kind;

  return (
    <div>
      <PageHeader
        back={{ to: "/tasks", label: "Tasks" }}
        eyebrow={
          <>
            <span className="mono text-ink-2">{t.key}</span>
            {t.parent_id && (
              <span>
                Child of{" "}
                <Link className="mono text-accent hover:underline" to={`/tasks/${key(t.parent_id)}`}>
                  {key(t.parent_id)}
                </Link>
              </span>
            )}
            <span title="Version, bumped on every change">v{t.version}</span>
          </>
        }
        title={t.title}
        actions={<IconButton icon="dots" label="More task actions" onClick={() => setMenu(true)} />}
      >
        <div className="flex flex-wrap items-center gap-2">
          <StatusBadge status={t.status} />
          <PriorityBadge priority={t.priority} />
          {t.category && <Tag>{sentence(t.category)}</Tag>}
          {t.tags.map((tag) => (
            <Tag key={tag}>#{tag}</Tag>
          ))}
        </div>
        <TaskStackLinks taskKey={t.key} />
      </PageHeader>

      {actions.length > 0 && (
        <div className="lg:mb-4">
          <ActionBar testId="task-actions">
            {actions.includes("claim") && (
              <Button variant="primary" onClick={() => action.mutate({ kind: "claim" })} busy={busy("claim")}>
                Claim
              </Button>
            )}
            {actions.includes("run") && (
              <Button onClick={() => setDialog("run")} disabled={!canRun} title={canRun ? "" : "Agent execution is turned off in Settings"}>
                Run agent
              </Button>
            )}
            {actions.includes("release") && (
              <Button variant="ghost" onClick={() => action.mutate({ kind: "release" })} busy={busy("release")}>
                Release
              </Button>
            )}
            {actions.includes("complete") && (
              <Button variant="primary" onClick={() => setDialog("complete")}>
                Complete
              </Button>
            )}
            {actions.includes("reject") && (
              <Button variant="danger" className="max-lg:px-3" onClick={() => setDialog("reject")}>
                Reject
              </Button>
            )}
            {actions.includes("accept_manual") && (
              <Button className="max-lg:px-3" onClick={() => setDialog("accept_manual")}>
                Accept manually
              </Button>
            )}
            {actions.includes("accept") && (
              <Button variant="primary" className="max-lg:px-3" onClick={() => action.mutate({ kind: "accept" })} busy={busy("accept")} title="Accept on the strength of passing verification">
                Accept
              </Button>
            )}
          </ActionBar>
        </div>
      )}
      <div className="mb-4 empty:hidden">
        <ErrorBanner error={action.error} />
      </div>
      {t.dependency_state && !t.dependency_state.satisfied && (
        <div className="mb-4">
          <Notice tone="warn">
            Blocked by{" "}
            {t.dependency_state.unsatisfied.map((u, i) => (
              <span key={u.key}>
                {i > 0 && ", "}
                <Link to={`/tasks/${u.key}`} className="mono font-medium text-ink underline underline-offset-2">
                  {u.key}
                </Link>
              </span>
            ))}
          </Notice>
        </div>
      )}

      {/* Phones read top to bottom in the order given by order-*; wide screens use two columns. */}
      <div className="flex flex-col gap-4 xl:grid xl:grid-cols-[minmax(0,2fr)_minmax(0,1fr)] xl:items-start">
        <div className="contents xl:flex xl:min-w-0 xl:flex-col xl:gap-4">
          <Card title="Description" className="order-1 xl:order-none">
            {t.description ? <Markdown source={t.description} /> : <p className="text-ink-3">No description.</p>}
            {t.acceptance_criteria.length > 0 && (
              <div className="mt-4 border-t border-line pt-3">
                <div className="text-[14px] font-semibold text-ink">Acceptance criteria</div>
                <ul className="mt-2 space-y-2">
                  {t.acceptance_criteria.map((c) => (
                    <li key={c} className="flex gap-2.5 text-[14px] leading-snug">
                      <span className="mt-[3px] h-4 w-4 shrink-0 rounded border border-line-strong" aria-hidden="true" />
                      <span className="min-w-0">{c}</span>
                    </li>
                  ))}
                </ul>
              </div>
            )}
          </Card>
          <div className="order-4 min-w-0 xl:order-none">
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
            <div className="pt-4">
              {tab === "history" && <HistoryPanel ws={ws} taskKey={t.key} />}
              {tab === "changes" && <ChangesPanel ws={ws} taskKey={t.key} repoNames={repoNames} />}
              {tab === "attempts" && <AttemptsPanel ws={ws} taskKey={t.key} />}
              {tab === "verification" && <VerificationPanel ws={ws} taskKey={t.key} />}
              {tab === "classification" && <ClassificationPanel ws={ws} taskKey={t.key} />}
              {tab === "insights" && <InsightsPanel ws={ws} taskKey={t.key} />}
            </div>
          </div>
        </div>
        <div className="contents xl:flex xl:min-w-0 xl:flex-col xl:gap-4">
          <Card title="Details" className="order-2 xl:order-none">
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
          <Card title="Dependencies" className="order-3 xl:order-none">
            <KeyValue
              items={[
                ["depends on", t.depends_on.length ? <TaskLinks ids={t.depends_on} keyOf={key} /> : "none"],
                ["blocks", t.blocks.length ? <TaskLinks ids={t.blocks} keyOf={key} /> : "none"],
                ["children", t.child_ids.length ? <TaskLinks ids={t.child_ids} keyOf={key} /> : null],
                ["relations", t.relations.length ? t.relations.map((r) => `${label(r.relation_type)} ${key(r.source_task_id === t.id ? r.target_task_id : r.source_task_id)}`).join(", ") : null],
              ]}
            />
            {t.rollup && (
              <div className="mt-4">
                <div className="flex justify-between text-[13px] text-ink-2">
                  <span>
                    {t.rollup.accepted} of {t.rollup.total_descendants} descendants accepted
                  </span>
                  <span className="font-semibold tnum">{pct(t.rollup.progress)}</span>
                </div>
                <div className="mt-1.5 h-2 overflow-hidden rounded-full bg-sunken ring-1 ring-line">
                  <div className="tone-done h-full rounded-full" style={{ width: `${t.rollup.progress * 100}%`, background: "var(--t)" }} />
                </div>
              </div>
            )}
          </Card>
          <Card title={`Comments (${t.comments.length})`} className="order-5 xl:order-none" bodyClass="p-0">
            {t.comments.length > 0 && (
              <ol className="max-h-[32rem] divide-y divide-line overflow-y-auto">
                {t.comments.map((c) => (
                  <li key={c.id} className="px-4 py-3">
                    <div className="mb-1 flex items-baseline justify-between gap-2 text-[13px]">
                      <span className="truncate font-medium text-ink">{c.author}</span>
                      <span className="shrink-0 text-ink-3">
                        {c.kind !== "comment" && `${c.kind}, `}
                        {ago(c.created_at)}
                      </span>
                    </div>
                    <div className="text-[14px]">
                      <Markdown source={c.body} />
                    </div>
                  </li>
                ))}
              </ol>
            )}
            <form
              className="space-y-2 border-t border-line p-4 first:border-t-0"
              onSubmit={(e) => {
                e.preventDefault();
                if (comment.trim()) addComment.mutate();
              }}
            >
              <textarea
                aria-label="New comment"
                className={`${textareaClass} h-20`}
                value={comment}
                onChange={(e) => setComment(e.target.value)}
                placeholder="Add a comment (Markdown)"
              />
              <ErrorBanner error={addComment.error} />
              <div className="flex justify-end">
                <Button type="submit" busy={addComment.isPending} disabled={!comment.trim()}>
                  Comment
                </Button>
              </div>
            </form>
          </Card>
        </div>
      </div>

      <ActionSheet
        title={t.key}
        open={menu}
        onClose={() => setMenu(false)}
        actions={[
          { label: "Edit task", onClick: () => setEditing(true) },
          ...(!actions.includes("run") ? [{ label: "Run agent", onClick: () => setDialog("run"), disabled: !canRun, hint: canRun ? undefined : "Agent execution is turned off in Settings" }] : []),
          ...MANUAL_STATUSES.filter((s) => s !== t.status).map((s) => ({
            label: `Move to ${label(s)}`,
            danger: s === "cancelled",
            onClick: () => action.mutate({ kind: "status", value: s }),
          })),
        ]}
      />
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
          className="space-y-4"
          onSubmit={(e) => {
            e.preventDefault();
            if (dialog) action.mutate({ kind: dialog, value: text });
          }}
        >
          {dialog === "run" ? (
            <Field label="Agent adapter" hint="Leave on default to use the configured agent.">
              <select className={`${selectClass} w-full`} value={text} onChange={(e) => setText(e.target.value)}>
                <option value="">Default</option>
                {meta.data?.agents.map((a) => (
                  <option key={a}>{a}</option>
                ))}
              </select>
            </Field>
          ) : (
            <Field label={dialog === "complete" ? "Summary" : "Reason"}>
              <textarea className={`${textareaClass} h-28`} value={text} onChange={(e) => setText(e.target.value)} required={dialog !== "complete"} />
            </Field>
          )}
          {dialog === "accept_manual" && <p className="text-[13px] text-ink-3">The decision and reason are kept permanently in the task history.</p>}
          <ErrorBanner error={action.error} />
          <Button type="submit" className="w-full md:ml-auto md:flex md:w-auto" variant={dialog === "reject" ? "danger" : "primary"} busy={action.isPending}>
            {dialogTitle(dialog)}
          </Button>
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
    <span className="flex flex-wrap gap-x-3 gap-y-1">
      {ids.map((id) => (
        <Link key={id} to={`/tasks/${keyOf(id)}`} className="mono text-[13px] text-accent hover:underline">
          {keyOf(id)}
        </Link>
      ))}
    </span>
  );
}
