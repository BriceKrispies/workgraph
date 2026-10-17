import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { get, patch, post } from "../api/client";
import {
  Button,
  Card,
  Empty,
  ErrorBanner,
  Field,
  KeyValue,
  Skeleton,
  StatusBadge,
  Table,
  Tabs,
  inputBase,
  inputClass,
  td,
} from "../components/ui";
import { useWorkspace } from "../hooks/workspace";
import { ago, short } from "../lib/format";

type Config = Record<string, Record<string, unknown>>;
type TabId = "general" | "providers" | "agents" | "verification" | "claude" | "worktrees";

export function SettingsPage() {
  const { ws } = useWorkspace();
  const [tab, setTab] = useState<TabId>("general");
  const settings = useQuery({
    queryKey: ["settings", ws],
    queryFn: () => get<{ config: Config; paths: Record<string, string> }>("/settings", { workspace: ws }),
  });
  return (
    <div className="space-y-3">
      <h1 className="text-xl font-semibold text-slate-900">Settings</h1>
      <Tabs<TabId>
        value={tab}
        onChange={setTab}
        tabs={[
          ["general", "Execution & jobs"],
          ["providers", "Inference providers"],
          ["agents", "Agent adapters"],
          ["verification", "Verification"],
          ["claude", "Claude Code"],
          ["worktrees", "Worktrees"],
        ]}
      />
      <ErrorBanner error={settings.error} />
      {tab === "general" && (settings.data ? <General config={settings.data.config} paths={settings.data.paths} /> : <Skeleton />)}
      {tab === "providers" && (settings.data ? <Providers config={settings.data.config} /> : <Skeleton />)}
      {tab === "agents" && <Agents />}
      {tab === "verification" && <Verification ws={ws} />}
      {tab === "claude" && <Claude />}
      {tab === "worktrees" && <Worktrees ws={ws} />}
    </div>
  );
}

function useSave() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (values: Record<string, unknown>) => patch("/settings", { values }),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: ["settings"] });
      void client.invalidateQueries({ queryKey: ["meta"] });
    },
  });
}

function General({ config, paths }: { config: Config; paths: Record<string, string> }) {
  const save = useSave();
  const execution = config.execution;
  const jobs = config.jobs;
  const [values, setValues] = useState<Record<string, unknown>>({});
  useEffect(() => setValues({}), [config]);
  const val = (key: string, fallback: unknown) => (key in values ? values[key] : fallback);
  const set = (key: string, value: unknown) => setValues((v) => ({ ...v, [key]: value }));
  const numberInput = (key: string, current: unknown, min = 1) => (
    <input type="number" min={min} className={`${inputBase} w-28`} value={String(val(key, current))} onChange={(e) => set(key, Number(e.target.value))} />
  );
  return (
    <div className="grid gap-4 lg:grid-cols-2">
      <Card title="Agent execution">
        <div className="space-y-3">
          <label className="flex items-center gap-2 text-sm">
            <input type="checkbox" checked={Boolean(val("execution.enabled", execution.enabled))} onChange={(e) => set("execution.enabled", e.target.checked)} />
            Allow agents to execute code (opt-in)
          </label>
          <label className="flex items-center gap-2 text-sm">
            <input type="checkbox" checked={Boolean(val("execution.auto_schedule", execution.auto_schedule))} onChange={(e) => set("execution.auto_schedule", e.target.checked)} />
            Automatically schedule ready auto-execute tasks
          </label>
          <Field label="Default agent">
            <input className={inputClass} value={String(val("execution.default_agent", execution.default_agent))} onChange={(e) => set("execution.default_agent", e.target.value)} />
          </Field>
          <div className="grid grid-cols-2 gap-3">
            <Field label="Max concurrent runs">{numberInput("execution.max_concurrent_runs", execution.max_concurrent_runs)}</Field>
            <Field label="Max attempts per task">{numberInput("execution.max_attempts_per_task", execution.max_attempts_per_task)}</Field>
          </div>
          <Field label="Isolation">
            <select className={inputClass} value={String(val("execution.isolation", execution.isolation))} onChange={(e) => set("execution.isolation", e.target.value)}>
              <option value="worktree">managed worktree per attempt</option>
              <option value="in_place">in place (exclusive lease)</option>
            </select>
          </Field>
          <div className="text-[12px] text-slate-500">Push/merge: {String(execution.allow_push)} / {String(execution.allow_merge)} (config file only)</div>
        </div>
      </Card>
      <Card title="Job concurrency">
        <div className="grid grid-cols-2 gap-3">
          <Field label="Worker concurrency">{numberInput("jobs.worker_concurrency", jobs.worker_concurrency)}</Field>
          <Field label="GPU-intensive jobs">{numberInput("jobs.gpu_concurrency", jobs.gpu_concurrency)}</Field>
          <Field label="Per agent">{numberInput("jobs.per_agent_concurrency", jobs.per_agent_concurrency)}</Field>
          <Field label="Per repository">{numberInput("jobs.per_repository_concurrency", jobs.per_repository_concurrency)}</Field>
          <Field label="Lease seconds">{numberInput("jobs.lease_seconds", jobs.lease_seconds, 5)}</Field>
          <Field label="Default max attempts">{numberInput("jobs.default_max_attempts", jobs.default_max_attempts)}</Field>
        </div>
        <div className="mt-3">
          <KeyValue items={Object.entries(paths).map(([k, v]) => [k, <span className="mono text-[11px]">{v}</span>])} />
        </div>
      </Card>
      <div className="flex items-center gap-2 lg:col-span-2">
        <Button variant="primary" disabled={!Object.keys(values).length} busy={save.isPending} onClick={() => save.mutate(values)}>
          Save settings
        </Button>
        {save.isSuccess && <span className="text-[12px] text-emerald-700">Saved.</span>}
        <ErrorBanner error={save.error} />
      </div>
    </div>
  );
}

function Providers({ config }: { config: Config }) {
  const save = useSave();
  const llm = config.llm as Record<string, unknown> & {
    providers: Record<string, Record<string, unknown>>;
    classification: { provider: string; model: string };
    embeddings: { provider: string; model: string };
    summarization: { provider: string; model: string };
  };
  const health = useQuery({ queryKey: ["llm-health"], queryFn: () => get<Record<string, { available: boolean; detail: string }>>("/llm/health") });
  const [values, setValues] = useState<Record<string, unknown>>({});
  const providers = Object.keys(llm.providers);
  const selection = (cap: "classification" | "embeddings" | "summarization") => (
    <div className="grid grid-cols-2 gap-2">
      <Field label={`${cap} provider`}>
        <select
          className={inputClass}
          value={String(values[`llm.${cap}.provider`] ?? llm[cap].provider)}
          onChange={(e) => setValues((v) => ({ ...v, [`llm.${cap}.provider`]: e.target.value }))}
        >
          {providers.map((p) => (
            <option key={p}>{p}</option>
          ))}
        </select>
      </Field>
      <Field label={`${cap} model`}>
        <input
          className={inputClass}
          value={String(values[`llm.${cap}.model`] ?? llm[cap].model)}
          onChange={(e) => setValues((v) => ({ ...v, [`llm.${cap}.model`]: e.target.value }))}
        />
      </Field>
    </div>
  );
  return (
    <div className="grid gap-4 lg:grid-cols-2">
      <Card title="Model selection">
        <div className="space-y-3">
          {selection("classification")}
          {selection("embeddings")}
          {selection("summarization")}
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={Boolean(values["llm.auto_classify"] ?? llm.auto_classify)}
              onChange={(e) => setValues((v) => ({ ...v, "llm.auto_classify": e.target.checked }))}
            />
            Classify tasks automatically on creation / material change
          </label>
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={Boolean(values["llm.auto_embed"] ?? llm.auto_embed)}
              onChange={(e) => setValues((v) => ({ ...v, "llm.auto_embed": e.target.checked }))}
            />
            Embed tasks automatically for semantic search
          </label>
          <Button variant="primary" disabled={!Object.keys(values).length} busy={save.isPending} onClick={() => save.mutate(values)}>
            Save
          </Button>
          <ErrorBanner error={save.error} />
        </div>
      </Card>
      <Card title="Providers">
        <ErrorBanner error={health.error} />
        <Table head={["Provider", "Type", "Endpoint", "Health"]}>
          {providers.map((name) => (
            <tr key={name}>
              <td className={td}>{name}</td>
              <td className={td}>{String(llm.providers[name].type)}</td>
              <td className={`${td} mono text-[11px]`}>{String(llm.providers[name].base_url ?? "built-in")}</td>
              <td className={`${td} text-[12px]`}>
                {health.data ? (
                  <>
                    <StatusBadge status={health.data[name]?.available ? "ok" : "unavailable"} />
                    <div className="text-slate-500">{health.data[name]?.detail}</div>
                  </>
                ) : (
                  "checking…"
                )}
              </td>
            </tr>
          ))}
        </Table>
        <p className="mt-2 text-[11px] text-slate-400">Workgraph works without any model server: model jobs fail clearly and tracking continues.</p>
      </Card>
    </div>
  );
}

function Agents() {
  const agents = useQuery({
    queryKey: ["agents"],
    queryFn: () => get<{ name: string; adapter: string; available: boolean; detail: string; capabilities: Record<string, boolean>; config: Record<string, unknown> }[]>("/agents"),
  });
  if (agents.error) return <ErrorBanner error={agents.error} />;
  if (!agents.data) return <Skeleton />;
  return (
    <Card bodyClass="p-0">
      <Table head={["Name", "Adapter", "Available", "Capabilities", "Configuration"]} testId="agents">
        {agents.data.map((a) => (
          <tr key={a.name}>
            <td className={`${td} font-medium`}>{a.name}</td>
            <td className={td}>{a.adapter}</td>
            <td className={`${td} text-[12px]`}>
              <StatusBadge status={a.available ? "ok" : "unavailable"} />
              <div className="max-w-xs text-slate-500">{a.detail}</div>
            </td>
            <td className={`${td} text-[12px]`}>
              {Object.entries(a.capabilities)
                .filter(([, v]) => v)
                .map(([k]) => k.replace(/_/g, " "))
                .join(", ")}
            </td>
            <td className={`${td} mono max-w-md truncate text-[11px]`} title={JSON.stringify(a.config)}>
              {JSON.stringify(a.config)}
            </td>
          </tr>
        ))}
      </Table>
    </Card>
  );
}

interface VConfig {
  id: string;
  scope_type: string;
  scope_id: string;
  name: string;
  checks: { name: string; kind: string; command: string[]; required: boolean; repository?: string | null }[];
  enabled: boolean;
  approved_by: string;
  version: number;
}

function Verification({ ws }: { ws: string }) {
  const client = useQueryClient();
  const configs = useQuery({ queryKey: ["verification-configs", ws], queryFn: () => get<VConfig[]>(`/ws/${ws}/verification/configs`) });
  const [scopeType, setScopeType] = useState("repository");
  const [scope, setScope] = useState("");
  const [name, setName] = useState("ci");
  const [checks, setChecks] = useState('[\n  {"name": "tests", "kind": "test", "command": ["pytest", "-q"]}\n]');
  const [parseError, setParseError] = useState<string | null>(null);
  const create = useMutation({
    mutationFn: (parsed: unknown) => post(`/ws/${ws}/verification/configs`, { scope_type: scopeType, scope: scope || null, name, checks: parsed }),
    onSuccess: () => client.invalidateQueries({ queryKey: ["verification-configs"] }),
  });
  const toggle = useMutation({
    mutationFn: (c: VConfig) => post(`/verification/configs/${c.id}/${c.enabled ? "disable" : "enable"}`),
    onSuccess: () => client.invalidateQueries({ queryKey: ["verification-configs"] }),
  });
  return (
    <div className="grid gap-4 xl:grid-cols-5">
      <Card title="Approved verification configurations" className="xl:col-span-3" bodyClass="p-0">
        <ErrorBanner error={configs.error ?? toggle.error} />
        {!configs.data ? (
          <Skeleton />
        ) : configs.data.length === 0 ? (
          <div className="p-3">
            <Empty title="No verification configured">Without checks, tasks need recorded manual acceptance.</Empty>
          </div>
        ) : (
          <Table head={["Name", "Scope", "Checks", "Approved by", "Enabled"]}>
            {configs.data.map((c) => (
              <tr key={c.id}>
                <td className={td}>
                  {c.name} <span className="text-[11px] text-slate-400">v{c.version}</span>
                </td>
                <td className={`${td} text-[12px]`}>
                  {c.scope_type} <span className="mono text-slate-400">{short(c.scope_id)}</span>
                </td>
                <td className={`${td} text-[12px]`}>
                  {c.checks.map((k) => (
                    <div key={k.name}>
                      <b>{k.name}</b> ({k.kind}
                      {k.required ? "" : ", optional"}): <span className="mono">{k.command.join(" ")}</span>
                    </div>
                  ))}
                </td>
                <td className={`${td} text-[12px]`}>{c.approved_by}</td>
                <td className={td}>
                  <Button size="sm" onClick={() => toggle.mutate(c)}>
                    {c.enabled ? "disable" : "enable"}
                  </Button>
                </td>
              </tr>
            ))}
          </Table>
        )}
      </Card>
      <Card title="Add or update configuration" className="xl:col-span-2">
        <form
          className="space-y-2"
          onSubmit={(e) => {
            e.preventDefault();
            try {
              const parsed: unknown = JSON.parse(checks);
              setParseError(null);
              create.mutate(parsed);
            } catch (err) {
              setParseError(`Invalid JSON: ${(err as Error).message}`);
            }
          }}
        >
          <div className="grid grid-cols-2 gap-2">
            <Field label="Scope type">
              <select className={inputClass} value={scopeType} onChange={(e) => setScopeType(e.target.value)}>
                {["workspace", "project", "repository", "task"].map((s) => (
                  <option key={s}>{s}</option>
                ))}
              </select>
            </Field>
            <Field label="Scope" hint="repository name, project slug or task key">
              <input className={inputClass} value={scope} onChange={(e) => setScope(e.target.value)} disabled={scopeType === "workspace"} />
            </Field>
          </div>
          <Field label="Name">
            <input className={inputClass} value={name} onChange={(e) => setName(e.target.value)} required />
          </Field>
          <Field label="Checks (JSON)" hint="Commands are argument vectors; no shell is involved.">
            <textarea aria-label="Checks JSON" className={`${inputClass} mono h-32 text-[12px]`} value={checks} onChange={(e) => setChecks(e.target.value)} />
          </Field>
          {parseError && <div className="text-[12px] text-rose-700">{parseError}</div>}
          <ErrorBanner error={create.error} />
          {create.isSuccess && <div className="text-[12px] text-emerald-700">Saved.</div>}
          <Button type="submit" variant="primary" busy={create.isPending}>
            Save configuration
          </Button>
        </form>
      </Card>
    </div>
  );
}

function Claude() {
  const status = useQuery({
    queryKey: ["claude-status"],
    queryFn: () =>
      get<{
        user: { settings: string; exists: boolean; installed_events: string[]; missing_events: string[]; command_resolves: boolean; error: string | null };
        hooks: { hook_events: number; last_received_at: string | null; by_event: Record<string, number>; spooled: number; spool_failed: number };
        sessions: { id: string; external_session_id: string; binding: string; task_id: string | null; tool_calls: number; tool_failures: number; started_at: string | null }[];
      }>("/claude/status"),
  });
  if (status.error) return <ErrorBanner error={status.error} />;
  if (!status.data) return <Skeleton />;
  const { user, hooks, sessions } = status.data;
  return (
    <div className="grid gap-4 lg:grid-cols-2">
      <Card title="Hook installation (user scope)">
        <KeyValue
          items={[
            ["settings file", <span className="mono text-[11px]">{user.settings}</span>],
            ["installed events", user.installed_events.length ? user.installed_events.join(", ") : "none"],
            ["missing events", user.missing_events.join(", ") || "none"],
            ["command resolves", user.command_resolves ? "yes" : "no"],
            ["error", user.error],
          ]}
        />
        <p className="mt-2 text-[12px] text-slate-500">
          Install with <span className="mono">workgraph claude install</span>. Hooks are passive (async, no output) and never change session permissions.
        </p>
      </Card>
      <Card title="Ingestion">
        <KeyValue
          items={[
            ["hook events", hooks.hook_events],
            ["last received", ago(hooks.last_received_at) || "never"],
            ["spooled (awaiting replay)", hooks.spooled],
            ["spool failures", hooks.spool_failed],
            ["by event", Object.entries(hooks.by_event).map(([k, v]) => `${k}: ${v}`).join(", ")],
          ]}
        />
      </Card>
      <Card title="Recent sessions" className="lg:col-span-2" bodyClass="p-0">
        {sessions.length ? (
          <Table head={["Session", "Binding", "Task", "Tool calls", "Failures", "Started"]}>
            {sessions.map((s) => (
              <tr key={s.id}>
                <td className={`${td} mono text-[11px]`}>{s.external_session_id}</td>
                <td className={td}>{s.binding}</td>
                <td className={`${td} mono text-[11px]`}>{short(s.task_id)}</td>
                <td className={td}>{s.tool_calls}</td>
                <td className={td}>{s.tool_failures}</td>
                <td className={`${td} text-slate-500`}>{ago(s.started_at)}</td>
              </tr>
            ))}
          </Table>
        ) : (
          <div className="p-3">
            <Empty title="No Claude Code sessions recorded" />
          </div>
        )}
      </Card>
    </div>
  );
}

function Worktrees({ ws }: { ws: string }) {
  const worktrees = useQuery({
    queryKey: ["worktrees", ws],
    queryFn: () => get<{ id: string; repository: string; attempt_id: string | null; path: string; branch: string; base_sha: string; status: string; created_at: string; last_error: string | null }[]>(`/ws/${ws}/worktrees`),
  });
  if (worktrees.error) return <ErrorBanner error={worktrees.error} />;
  if (!worktrees.data) return <Skeleton />;
  if (!worktrees.data.length) return <Empty title="No managed worktrees">Created per attempt when agents run in isolation.</Empty>;
  return (
    <Card bodyClass="p-0">
      <Table head={["Repository", "Status", "Branch", "Base", "Path", "Created"]}>
        {worktrees.data.map((w) => (
          <tr key={w.id}>
            <td className={td}>{w.repository}</td>
            <td className={td}>
              <StatusBadge status={w.status} />
              {w.last_error && <div className="text-[11px] text-rose-700">{w.last_error}</div>}
            </td>
            <td className={`${td} mono text-[11px]`}>{w.branch}</td>
            <td className={`${td} mono text-[11px]`}>{short(w.base_sha, 10)}</td>
            <td className={`${td} mono break-all text-[11px]`}>{w.path}</td>
            <td className={`${td} text-slate-500`}>{ago(w.created_at)}</td>
          </tr>
        ))}
      </Table>
      <p className="p-3 text-[11px] text-slate-400">
        Released worktrees are removed only by an explicit <span className="mono">workgraph git cleanup-worktrees --apply</span>; branches are always kept.
      </p>
    </Card>
  );
}
