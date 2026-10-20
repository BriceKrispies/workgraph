import { useState } from "react";
import { useSearchParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ApiError, get, post, put } from "../api/client";
import type { AutomationRun, Job, Rule } from "../api/types";
import {
  Button,
  Card,
  Chip,
  Collapsible,
  Empty,
  ErrorBanner,
  Field,
  List,
  ListRow,
  Meta,
  Modal,
  PageHeader,
  Skeleton,
  StatusBadge,
  Switch,
  Tabs,
  inputClass,
  textareaClass,
} from "../components/ui";
import { useWorkspace } from "../hooks/workspace";
import { ago, label, sentence, short } from "../lib/format";
import { toneOf } from "../lib/tone";

const TEMPLATE = {
  name: "tag critical bugs",
  description: "Tag and notify when a critical task is created",
  trigger: { type: "event", event_types: ["task.created"] },
  conditions: { field: "task.priority", op: "eq", value: "critical" },
  actions: [
    { type: "add_tags", tags: ["triage"] },
    { type: "notify", level: "warning", title: "Critical task {{task.key}}: {{task.title}}" },
  ],
};

type TabId = "rules" | "runs" | "jobs" | "notifications";
const TABS: [TabId, string][] = [
  ["rules", "Rules"],
  ["runs", "Trigger history"],
  ["jobs", "Jobs"],
  ["notifications", "Notifications"],
];

export function AutomationPage() {
  const { ws } = useWorkspace();
  const [params, setParams] = useSearchParams();
  const tab = (TABS.find(([id]) => id === params.get("tab"))?.[0] ?? "rules") as TabId;
  return (
    <div>
      <PageHeader title="Automation" />
      <Tabs<TabId> value={tab} onChange={(t) => setParams(t === "rules" ? {} : { tab: t }, { replace: true })} tabs={TABS} />
      <div className="pt-4">
        {tab === "rules" && <Rules ws={ws} />}
        {tab === "runs" && <Runs ws={ws} />}
        {tab === "jobs" && <Jobs ws={ws} />}
        {tab === "notifications" && <Notifications ws={ws} />}
      </div>
    </div>
  );
}

function describeTrigger(trigger: Record<string, unknown>): string {
  const detail = Array.isArray(trigger.event_types)
    ? (trigger.event_types as string[]).join(", ")
    : trigger.cron
      ? `cron ${String(trigger.cron)}`
      : trigger.interval_seconds
        ? `every ${String(trigger.interval_seconds)}s`
        : "";
  return `${sentence(String(trigger.type))}${detail ? `: ${detail}` : ""}`;
}

function Rules({ ws }: { ws: string }) {
  const client = useQueryClient();
  const rules = useQuery({ queryKey: ["automation", ws, "rules"], queryFn: () => get<Rule[]>(`/ws/${ws}/automation/rules`) });
  const catalog = useQuery({
    queryKey: ["automation", "catalog"],
    queryFn: () => get<{ triggers: Record<string, string>; actions: string[]; operators: string[] }>("/automation/catalog"),
  });
  const [editing, setEditing] = useState<Rule | "new" | null>(null);
  const [previewing, setPreviewing] = useState<Rule | null>(null);
  const [triggering, setTriggering] = useState<Rule | null>(null);
  const toggle = useMutation({
    mutationFn: (rule: Rule) => post(`/ws/${ws}/automation/rules/${encodeURIComponent(rule.name)}/${rule.enabled ? "disable" : "enable"}`),
    onSuccess: () => client.invalidateQueries({ queryKey: ["automation"] }),
  });
  const visible = rules.data?.filter((r) => !r.name.startsWith("[preview]")) ?? [];
  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between gap-2">
        <p className="text-[14px] text-ink-2">Rules react to events, schedules or a manual trigger.</p>
        <Button variant="primary" icon="plus" onClick={() => setEditing("new")}>
          <span className="sm:hidden">New</span>
          <span className="hidden sm:inline">New rule</span>
        </Button>
      </div>
      <ErrorBanner error={rules.error ?? toggle.error} />
      {!rules.data ? (
        <Card bodyClass="p-0">
          <Skeleton />
        </Card>
      ) : visible.length === 0 ? (
        <Card>
          <Empty title="No automation rules">Start from the template under New rule.</Empty>
        </Card>
      ) : (
        <List testId="rules">
          {visible.map((r) => (
            <ListRow key={r.id} tone={r.enabled ? "done" : "idle"} trailing={<Switch checked={r.enabled} onChange={() => toggle.mutate(r)} label={`toggle ${r.name}`} />}>
              <div className="font-semibold">{r.name}</div>
              {r.description && <div className="mt-0.5 text-[14px] text-ink-2">{r.description}</div>}
              <Meta>
                <span>{describeTrigger(r.trigger)}</span>
                <span>Then {r.actions.map((a) => label(String(a.type))).join(", ")}</span>
                <span>By {r.created_by}</span>
              </Meta>
              <div className="-ml-2 mt-2 flex flex-wrap gap-1">
                <Button size="sm" variant="ghost" onClick={() => setPreviewing(r)}>
                  Dry run
                </Button>
                <Button size="sm" variant="ghost" onClick={() => setTriggering(r)}>
                  Trigger
                </Button>
                <Button size="sm" variant="ghost" onClick={() => setEditing(r)}>
                  Edit
                </Button>
              </div>
            </ListRow>
          ))}
        </List>
      )}
      {catalog.data && (
        <Card>
          <Collapsible title="Supported building blocks">
            <h3 className="mb-3 hidden text-[15px] font-semibold lg:block">Supported building blocks</h3>
            <div className="grid gap-4 pt-2 text-[13px] md:grid-cols-3 lg:pt-0">
              <div>
                <div className="mb-1 font-semibold text-ink-2">Event triggers</div>
                <ul className="space-y-1">
                  {Object.entries(catalog.data.triggers).map(([k, v]) => (
                    <li key={k}>
                      <span className="mono text-[12px]">{k}</span> <span className="text-ink-3">{v}</span>
                    </li>
                  ))}
                </ul>
                <div className="mt-2 text-ink-3">Plus schedule (cron or interval_seconds) and manual.</div>
              </div>
              <div>
                <div className="mb-1 font-semibold text-ink-2">Actions</div>
                <ul className="space-y-1">
                  {catalog.data.actions.map((a) => (
                    <li key={a} className="mono text-[12px]">
                      {a}
                    </li>
                  ))}
                </ul>
              </div>
              <div>
                <div className="mb-1 font-semibold text-ink-2">Condition operators</div>
                <div className="mono text-[12px]">{catalog.data.operators.join(", ")}</div>
                <div className="mt-2 text-ink-3">Combine with all, any and not. Fields: task.*, event.*, classification.*, attempt.*</div>
              </div>
            </div>
          </Collapsible>
        </Card>
      )}
      {editing && <RuleEditor ws={ws} rule={editing === "new" ? null : editing} onClose={() => setEditing(null)} />}
      {previewing && <PreviewDialog ws={ws} rule={previewing} onClose={() => setPreviewing(null)} />}
      {triggering && <TriggerDialog ws={ws} rule={triggering} onClose={() => setTriggering(null)} />}
    </div>
  );
}

function ruleSpec(rule: Rule) {
  return {
    name: rule.name,
    description: rule.description,
    enabled: rule.enabled,
    trigger: rule.trigger,
    conditions: Object.keys(rule.conditions ?? {}).length ? rule.conditions : undefined,
    actions: rule.actions,
    retry_max_attempts: rule.retry_max_attempts,
    max_depth: rule.max_depth,
  };
}

function RuleEditor({ ws, rule, onClose }: { ws: string; rule: Rule | null; onClose: () => void }) {
  const client = useQueryClient();
  const [text, setText] = useState(JSON.stringify(rule ? ruleSpec(rule) : TEMPLATE, null, 2));
  const [parseError, setParseError] = useState<string | null>(null);
  const save = useMutation({
    mutationFn: (spec: unknown) => (rule ? put(`/ws/${ws}/automation/rules/${encodeURIComponent(rule.name)}`, spec) : post(`/ws/${ws}/automation/rules`, spec)),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: ["automation"] });
      onClose();
    },
  });
  const preview = useMutation({
    mutationFn: (spec: unknown) => post<AutomationRun>(`/ws/${ws}/automation/preview`, { spec }),
  });
  const parsed = (): unknown => {
    try {
      const value: unknown = JSON.parse(text);
      setParseError(null);
      return value;
    } catch (e) {
      setParseError(`Invalid JSON: ${(e as Error).message}`);
      return null;
    }
  };
  return (
    <Modal title={rule ? `Edit ${rule.name}` : "New automation rule"} open onClose={onClose} wide>
      <div className="space-y-4">
        <textarea
          aria-label="Rule JSON"
          spellCheck={false}
          autoCapitalize="none"
          autoCorrect="off"
          className={`${textareaClass} mono h-[50dvh] text-[13px] md:h-80`}
          value={text}
          onChange={(e) => setText(e.target.value)}
        />
        {parseError && <div className="text-[13px] text-danger">{parseError}</div>}
        <ErrorBanner error={save.error ?? preview.error} />
        {validationErrors(save.error).length > 0 && (
          <ul className="list-disc pl-5 text-[13px] text-danger">
            {validationErrors(save.error).map((e, i) => (
              <li key={i}>
                {e.loc.join(".")}: {e.msg}
              </li>
            ))}
          </ul>
        )}
        {preview.data && <PreviewResult run={preview.data} />}
        <div className="flex gap-2 [&>*]:flex-1 md:justify-end md:[&>*]:flex-none">
          <Button
            onClick={() => {
              const spec = parsed();
              if (spec) preview.mutate(spec);
            }}
            busy={preview.isPending}
          >
            Dry run
          </Button>
          <Button
            variant="primary"
            busy={save.isPending}
            onClick={() => {
              const spec = parsed();
              if (spec) save.mutate(spec);
            }}
          >
            Save rule
          </Button>
        </div>
      </div>
    </Modal>
  );
}

function validationErrors(error: unknown): { msg: string; loc: unknown[] }[] {
  if (error instanceof ApiError && Array.isArray(error.details.errors)) {
    return error.details.errors as { msg: string; loc: unknown[] }[];
  }
  return [];
}

function PreviewResult({ run }: { run: AutomationRun }) {
  return (
    <div className="space-y-2 rounded-xl border border-line bg-sunken p-3 text-[13px]" data-testid="preview-result">
      <div className="flex flex-wrap items-center gap-2">
        <StatusBadge status={run.status} />
        <span>{run.matched ? "Matched" : "Did not match"}</span>
        <span className="text-ink-3">{run.reason}</span>
      </div>
      {run.condition_trace.length > 0 && (
        <ul className="divide-y divide-line rounded-lg border border-line bg-surface">
          {run.condition_trace.map((t, i) => (
            <li key={i} className="flex items-start gap-2 px-3 py-2">
              <span className={`mt-1.5 dot ${t.result ? "tone-done" : "tone-failed"}`} aria-label={t.result ? "passed" : "failed"} />
              <span className="min-w-0 break-words">
                <span className="mono text-[12px]">{t.field}</span> {t.op} <span className="mono text-[12px]">{JSON.stringify(t.expected)}</span>
                <span className="text-ink-3">, actual </span>
                <span className="mono text-[12px]">{JSON.stringify(t.actual)}</span>
              </span>
            </li>
          ))}
        </ul>
      )}
      {run.action_results.map((a, i) => (
        <div key={i}>Would {String(a.planned ?? a.type)}</div>
      ))}
    </div>
  );
}

function PreviewDialog({ ws, rule, onClose }: { ws: string; rule: Rule; onClose: () => void }) {
  const [task, setTask] = useState("");
  const preview = useMutation({
    mutationFn: () => post<AutomationRun>(`/ws/${ws}/automation/preview`, { rule: rule.name, inputs: task ? { task } : null }),
  });
  return (
    <Modal title={`Dry run: ${rule.name}`} open onClose={onClose} wide>
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          preview.mutate();
        }}
      >
        <Field label="Task context" hint="Evaluate the conditions against this task key">
          <input className={`${inputClass} mono`} value={task} onChange={(e) => setTask(e.target.value)} placeholder="WG-1" autoCapitalize="characters" />
        </Field>
        <ErrorBanner error={preview.error} />
        {preview.data && <PreviewResult run={preview.data} />}
        <Button type="submit" variant="primary" className="w-full md:ml-auto md:flex md:w-auto" busy={preview.isPending}>
          Evaluate without side effects
        </Button>
      </form>
    </Modal>
  );
}

function TriggerDialog({ ws, rule, onClose }: { ws: string; rule: Rule; onClose: () => void }) {
  const client = useQueryClient();
  const [task, setTask] = useState("");
  const trigger = useMutation({
    mutationFn: () => post<AutomationRun>(`/ws/${ws}/automation/rules/${encodeURIComponent(rule.name)}/trigger`, { inputs: task ? { task } : {} }),
    onSuccess: () => client.invalidateQueries({ queryKey: ["automation"] }),
  });
  return (
    <Modal title={`Trigger ${rule.name}`} open onClose={onClose}>
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          trigger.mutate();
        }}
      >
        <Field label="Task" hint="Optional">
          <input className={`${inputClass} mono`} value={task} onChange={(e) => setTask(e.target.value)} autoCapitalize="characters" />
        </Field>
        <ErrorBanner error={trigger.error} />
        {trigger.data && (
          <div className="flex flex-wrap items-center gap-2 text-[13px]">
            <span className="mono">Run {short(trigger.data.id)}</span>
            <StatusBadge status={trigger.data.status} />
            <span className="text-ink-3">{trigger.data.reason}</span>
          </div>
        )}
        <Button type="submit" variant="primary" className="w-full md:ml-auto md:flex md:w-auto" busy={trigger.isPending}>
          Trigger now
        </Button>
      </form>
    </Modal>
  );
}

function Runs({ ws }: { ws: string }) {
  const runs = useQuery({ queryKey: ["automation", ws, "runs"], queryFn: () => get<AutomationRun[]>(`/ws/${ws}/automation/runs`, { limit: 200 }) });
  const rules = useQuery({ queryKey: ["automation", ws, "rules"], queryFn: () => get<Rule[]>(`/ws/${ws}/automation/rules`) });
  const names = new Map((rules.data ?? []).map((r) => [r.id, r.name]));
  if (runs.error) return <ErrorBanner error={runs.error} />;
  if (!runs.data) return <Skeleton />;
  if (!runs.data.length)
    return (
      <Card>
        <Empty title="No rule runs yet" />
      </Card>
    );
  return (
    <List testId="automation-runs">
      {runs.data.map((r) => (
        <ListRow key={r.id} tone={toneOf(r.status)}>
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-medium">{names.get(r.rule_id) ?? short(r.rule_id)}</span>
            <StatusBadge status={r.status} />
            {r.dry_run && <span className="text-[12px] font-medium text-accent">Dry run</span>}
            <span className="ml-auto text-[13px] text-ink-3">{ago(r.created_at)}</span>
          </div>
          {r.reason && <div className="mt-0.5 text-[14px] text-ink-2">{r.reason}</div>}
          {r.error && <div className="mt-0.5 text-[13px] text-danger">{r.error}</div>}
          <Meta>
            <span>{sentence(r.trigger_type)}</span>
            <span>Depth {r.depth}</span>
            {r.action_results.length > 0 && <span>{r.action_results.map((a) => `${String(a.type ?? a.planned ?? "")}: ${String(a.status ?? "planned")}`).join(", ")}</span>}
          </Meta>
        </ListRow>
      ))}
    </List>
  );
}

function Jobs({ ws }: { ws: string }) {
  const client = useQueryClient();
  const [status, setStatus] = useState("dead");
  const jobs = useQuery({
    queryKey: ["jobs", ws, status],
    queryFn: () => get<{ items: Job[]; total: number; stats: Record<string, number> }>("/jobs", { workspace: ws, status: status || undefined, limit: 200 }),
  });
  const act = useMutation({
    mutationFn: ({ id, kind }: { id: string; kind: "retry" | "cancel" }) => post(`/jobs/${id}/${kind}`),
    onSuccess: () => client.invalidateQueries({ queryKey: ["jobs"] }),
  });
  return (
    <div className="space-y-3">
      <div className="scroll-x -mx-4 flex gap-2 px-4" role="group" aria-label="Job status">
        {["dead", "failed", "queued", "running", "succeeded", "cancelled", ""].map((s) => (
          <Chip key={s || "all"} active={status === s} tone={s ? toneOf(s) : undefined} onClick={() => setStatus(s)}>
            {s ? sentence(s) : "All"}
            {s && jobs.data?.stats[s] !== undefined && <span className="tnum opacity-70">{jobs.data.stats[s]}</span>}
          </Chip>
        ))}
      </div>
      <ErrorBanner error={jobs.error ?? act.error} />
      {!jobs.data ? (
        <Card bodyClass="p-0">
          <Skeleton />
        </Card>
      ) : jobs.data.items.length === 0 ? (
        <Card>
          <Empty title={status ? `No ${label(status)} jobs` : "No jobs"} />
        </Card>
      ) : (
        <List testId="jobs">
          {jobs.data.items.map((j) => (
            <ListRow
              key={j.id}
              tone={toneOf(j.status)}
              trailing={
                ["dead", "cancelled", "failed"].includes(j.status) ? (
                  <Button size="sm" onClick={() => act.mutate({ id: j.id, kind: "retry" })}>
                    Retry
                  </Button>
                ) : ["queued", "running"].includes(j.status) ? (
                  <Button size="sm" variant="danger" onClick={() => act.mutate({ id: j.id, kind: "cancel" })}>
                    Cancel
                  </Button>
                ) : undefined
              }
            >
              <div className="flex flex-wrap items-center gap-2">
                <span className="font-medium">{sentence(j.kind)}</span>
                <StatusBadge status={j.status} />
              </div>
              <Meta>
                <span className="mono text-[12px]">{short(j.id)}</span>
                <span>
                  Attempt {j.attempts} of {j.max_attempts}
                </span>
                <span>{ago(j.created_at)}</span>
              </Meta>
              {(j.last_error || j.error_kind) && (
                <div className="mt-1 line-clamp-3 break-words text-[13px] text-danger">
                  {j.error_kind && <span className="mono mr-1 text-[12px] text-ink-3">[{j.error_kind}]</span>}
                  {j.last_error}
                </div>
              )}
            </ListRow>
          ))}
        </List>
      )}
    </div>
  );
}

function Notifications({ ws }: { ws: string }) {
  const client = useQueryClient();
  const notes = useQuery({
    queryKey: ["automation", ws, "notifications"],
    queryFn: () => get<{ id: string; level: string; title: string; body: string; source: string; read_at: string | null; created_at: string }[]>(`/ws/${ws}/notifications`),
  });
  const read = useMutation({ mutationFn: (id: string) => post(`/notifications/${id}/read`), onSuccess: () => client.invalidateQueries({ queryKey: ["automation"] }) });
  if (notes.error) return <ErrorBanner error={notes.error} />;
  if (!notes.data) return <Skeleton />;
  if (!notes.data.length)
    return (
      <Card>
        <Empty title="No notifications">Rules with a notify action post here.</Empty>
      </Card>
    );
  const levelTone: Record<string, string> = { error: "failed", warning: "blocked", info: "ready" };
  return (
    <List>
      {notes.data.map((n) => (
        <ListRow
          key={n.id}
          tone={n.read_at ? undefined : ((levelTone[n.level] ?? "ready") as never)}
          selected={!n.read_at}
          trailing={
            !n.read_at ? (
              <Button size="sm" variant="ghost" onClick={() => read.mutate(n.id)}>
                Mark read
              </Button>
            ) : undefined
          }
        >
          <div className={n.read_at ? "text-ink-2" : "font-semibold"}>{n.title}</div>
          {n.body && <div className="mt-0.5 text-[14px] text-ink-2">{n.body}</div>}
          <Meta>
            <span>{sentence(n.level)}</span>
            <span>{n.source}</span>
            <span>{ago(n.created_at)}</span>
          </Meta>
        </ListRow>
      ))}
    </List>
  );
}
