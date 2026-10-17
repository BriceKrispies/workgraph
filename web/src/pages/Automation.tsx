import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ApiError, get, post, put } from "../api/client";
import type { AutomationRun, Job, Rule } from "../api/types";
import {
  Button,
  Card,
  Empty,
  ErrorBanner,
  Field,
  Modal,
  Skeleton,
  StatusBadge,
  Table,
  Tabs,
  inputClass,
  td,
} from "../components/ui";
import { useWorkspace } from "../hooks/workspace";
import { ago, label, short } from "../lib/format";

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

export function AutomationPage() {
  const { ws } = useWorkspace();
  const [tab, setTab] = useState<TabId>("rules");
  return (
    <div className="space-y-3">
      <h1 className="text-xl font-semibold text-slate-900">Automation</h1>
      <Tabs<TabId>
        value={tab}
        onChange={setTab}
        tabs={[
          ["rules", "Rules"],
          ["runs", "Trigger history"],
          ["jobs", "Jobs"],
          ["notifications", "Notifications"],
        ]}
      />
      {tab === "rules" && <Rules ws={ws} />}
      {tab === "runs" && <Runs ws={ws} />}
      {tab === "jobs" && <Jobs ws={ws} />}
      {tab === "notifications" && <Notifications ws={ws} />}
    </div>
  );
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
  return (
    <div className="space-y-3">
      <div className="flex justify-end">
        <Button variant="primary" onClick={() => setEditing("new")}>
          New rule
        </Button>
      </div>
      <ErrorBanner error={rules.error ?? toggle.error} />
      <Card bodyClass="p-0">
        {!rules.data ? (
          <Skeleton />
        ) : rules.data.length === 0 ? (
          <div className="p-4">
            <Empty title="No automation rules">Rules react to events, schedules or manual triggers.</Empty>
          </div>
        ) : (
          <Table head={["Rule", "Trigger", "Actions", "Author", "Enabled", ""]} testId="rules">
            {rules.data.filter((r) => !r.name.startsWith("[preview]")).map((r) => (
              <tr key={r.id}>
                <td className={td}>
                  <div className="font-medium">{r.name}</div>
                  <div className="text-[12px] text-slate-500">{r.description}</div>
                </td>
                <td className={`${td} text-[12px]`}>
                  {String(r.trigger.type)}{" "}
                  {Array.isArray(r.trigger.event_types) ? (r.trigger.event_types as string[]).join(", ") : String(r.trigger.cron ?? r.trigger.interval_seconds ?? "")}
                </td>
                <td className={`${td} text-[12px]`}>{r.actions.map((a) => label(String(a.type))).join(", ")}</td>
                <td className={`${td} text-[12px] text-slate-500`}>{r.created_by}</td>
                <td className={td}>
                  <button
                    role="switch"
                    aria-checked={r.enabled}
                    aria-label={`toggle ${r.name}`}
                    onClick={() => toggle.mutate(r)}
                    className={`relative h-5 w-9 rounded-full transition ${r.enabled ? "bg-emerald-500" : "bg-slate-300"}`}
                  >
                    <span className={`absolute top-0.5 h-4 w-4 rounded-full bg-white transition ${r.enabled ? "left-4" : "left-0.5"}`} />
                  </button>
                </td>
                <td className={`${td} whitespace-nowrap`}>
                  <Button size="sm" variant="ghost" onClick={() => setPreviewing(r)}>
                    Dry run
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => setTriggering(r)}>
                    Trigger
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => setEditing(r)}>
                    Edit
                  </Button>
                </td>
              </tr>
            ))}
          </Table>
        )}
      </Card>
      {catalog.data && (
        <Card title="Supported building blocks">
          <div className="grid gap-3 text-[12px] md:grid-cols-3">
            <div>
              <div className="font-semibold text-slate-600">Event triggers</div>
              {Object.entries(catalog.data.triggers).map(([k, v]) => (
                <div key={k}>
                  <span className="mono">{k}</span> — {v}
                </div>
              ))}
              <div className="mt-1 text-slate-500">plus schedule (cron / interval_seconds) and manual</div>
            </div>
            <div>
              <div className="font-semibold text-slate-600">Actions</div>
              {catalog.data.actions.map((a) => (
                <div key={a}>{a}</div>
              ))}
            </div>
            <div>
              <div className="font-semibold text-slate-600">Condition operators</div>
              <div>{catalog.data.operators.join(", ")}</div>
              <div className="mt-1 text-slate-500">Combine with all / any / not. Fields: task.*, event.*, classification.*, attempt.*</div>
            </div>
          </div>
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
      <div className="space-y-3">
        <textarea
          aria-label="Rule JSON"
          spellCheck={false}
          className={`${inputClass} mono h-80 text-[12px]`}
          value={text}
          onChange={(e) => setText(e.target.value)}
        />
        {parseError && <div className="text-[12px] text-rose-700">{parseError}</div>}
        <ErrorBanner error={save.error ?? preview.error} />
        {validationErrors(save.error).length > 0 && (
          <ul className="list-disc pl-5 text-[12px] text-rose-700">
            {validationErrors(save.error).map((e, i) => (
              <li key={i}>
                {e.loc.join(".")}: {e.msg}
              </li>
            ))}
          </ul>
        )}
        {preview.data && <PreviewResult run={preview.data} />}
        <div className="flex justify-end gap-2">
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
    <div className="rounded border border-slate-200 bg-slate-50 p-2 text-[12px]" data-testid="preview-result">
      <div className="mb-1 flex items-center gap-2">
        <StatusBadge status={run.status} /> matched: <b>{String(run.matched)}</b> — {run.reason}
      </div>
      {run.condition_trace.length > 0 && (
        <Table head={["Field", "Op", "Expected", "Actual", "Result"]}>
          {run.condition_trace.map((t, i) => (
            <tr key={i}>
              <td className={`${td} mono`}>{t.field}</td>
              <td className={td}>{t.op}</td>
              <td className={td}>{JSON.stringify(t.expected)}</td>
              <td className={td}>{JSON.stringify(t.actual)}</td>
              <td className={td}>{t.result ? "✓" : "✗"}</td>
            </tr>
          ))}
        </Table>
      )}
      {run.action_results.map((a, i) => (
        <div key={i}>would {String(a.planned ?? a.type)}</div>
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
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          preview.mutate();
        }}
      >
        <Field label="Task context" hint="Evaluate conditions against this task (key)">
          <input className={`${inputClass} mono`} value={task} onChange={(e) => setTask(e.target.value)} placeholder="WG-1" />
        </Field>
        <ErrorBanner error={preview.error} />
        {preview.data && <PreviewResult run={preview.data} />}
        <div className="flex justify-end">
          <Button type="submit" variant="primary" busy={preview.isPending}>
            Evaluate (no side effects)
          </Button>
        </div>
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
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          trigger.mutate();
        }}
      >
        <Field label="Task (optional)">
          <input className={`${inputClass} mono`} value={task} onChange={(e) => setTask(e.target.value)} />
        </Field>
        <ErrorBanner error={trigger.error} />
        {trigger.data && (
          <div className="text-[12px]">
            Run {short(trigger.data.id)}: <StatusBadge status={trigger.data.status} /> {trigger.data.reason}
          </div>
        )}
        <div className="flex justify-end">
          <Button type="submit" variant="primary" busy={trigger.isPending}>
            Trigger now
          </Button>
        </div>
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
  if (!runs.data.length) return <Empty title="No rule runs yet" />;
  return (
    <Card bodyClass="p-0">
      <Table head={["When", "Rule", "Trigger", "Depth", "Status", "Reason", "Actions"]} testId="automation-runs">
        {runs.data.map((r) => (
          <tr key={r.id}>
            <td className={`${td} whitespace-nowrap text-slate-500`}>{ago(r.created_at)}</td>
            <td className={td}>{names.get(r.rule_id) ?? short(r.rule_id)}</td>
            <td className={td}>
              {r.trigger_type}
              {r.dry_run && <span className="ml-1 text-[11px] text-sky-600">dry run</span>}
            </td>
            <td className={td}>{r.depth}</td>
            <td className={td}>
              <StatusBadge status={r.status} />
            </td>
            <td className={`${td} text-[12px]`}>
              {r.reason}
              {r.error && <div className="text-rose-700">{r.error}</div>}
            </td>
            <td className={`${td} text-[12px]`}>{r.action_results.map((a) => `${String(a.type ?? a.planned ?? "")}:${String(a.status ?? "planned")}`).join(", ")}</td>
          </tr>
        ))}
      </Table>
    </Card>
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
    <div className="space-y-2">
      <div className="flex flex-wrap items-center gap-2 text-sm">
        {["dead", "failed", "queued", "running", "succeeded", "cancelled", ""].map((s) => (
          <Button key={s || "all"} size="sm" variant={status === s ? "primary" : "secondary"} onClick={() => setStatus(s)}>
            {s || "all"} {s && jobs.data?.stats[s] !== undefined ? `(${jobs.data.stats[s]})` : ""}
          </Button>
        ))}
      </div>
      <ErrorBanner error={jobs.error ?? act.error} />
      <Card bodyClass="p-0">
        {!jobs.data ? (
          <Skeleton />
        ) : jobs.data.items.length === 0 ? (
          <div className="p-3">
            <Empty title={`No ${status || ""} jobs`} />
          </div>
        ) : (
          <Table head={["Job", "Kind", "Status", "Attempts", "Error", "Created", ""]} testId="jobs">
            {jobs.data.items.map((j) => (
              <tr key={j.id}>
                <td className={`${td} mono text-[11px]`}>{short(j.id)}</td>
                <td className={td}>{j.kind}</td>
                <td className={td}>
                  <StatusBadge status={j.status} />
                </td>
                <td className={td}>
                  {j.attempts}/{j.max_attempts}
                </td>
                <td className={`${td} max-w-sm text-[12px] text-rose-700`}>
                  {j.error_kind && <span className="mono mr-1 text-slate-500">[{j.error_kind}]</span>}
                  {j.last_error}
                </td>
                <td className={`${td} whitespace-nowrap text-slate-500`}>{ago(j.created_at)}</td>
                <td className={`${td} whitespace-nowrap`}>
                  {["dead", "cancelled", "failed"].includes(j.status) && (
                    <Button size="sm" onClick={() => act.mutate({ id: j.id, kind: "retry" })}>
                      Retry
                    </Button>
                  )}
                  {["queued", "running"].includes(j.status) && (
                    <Button size="sm" variant="danger" onClick={() => act.mutate({ id: j.id, kind: "cancel" })}>
                      Cancel
                    </Button>
                  )}
                </td>
              </tr>
            ))}
          </Table>
        )}
      </Card>
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
  if (!notes.data.length) return <Empty title="No notifications" />;
  return (
    <div className="space-y-2">
      {notes.data.map((n) => (
        <div key={n.id} className={`flex items-start justify-between rounded border p-2 ${n.read_at ? "border-slate-200 bg-white" : "border-sky-200 bg-sky-50"}`}>
          <div>
            <div className="font-medium">
              <span className="mr-1 text-[11px] uppercase text-slate-500">{n.level}</span>
              {n.title}
            </div>
            {n.body && <div className="text-[12px] text-slate-600">{n.body}</div>}
            <div className="text-[11px] text-slate-400">
              {n.source} · {ago(n.created_at)}
            </div>
          </div>
          {!n.read_at && (
            <Button size="sm" variant="ghost" onClick={() => read.mutate(n.id)}>
              mark read
            </Button>
          )}
        </div>
      ))}
    </div>
  );
}
