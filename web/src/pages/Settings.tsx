import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { get, patch, post } from "../api/client";
import {
  ActionBar,
  Button,
  Card,
  Checkbox,
  Empty,
  ErrorBanner,
  Field,
  KeyValue,
  List,
  ListRow,
  Meta,
  PageHeader,
  Skeleton,
  StatusBadge,
  Tabs,
  inputClass,
  selectClass,
  textareaClass,
} from "../components/ui";
import { useWorkspace } from "../hooks/workspace";
import { ago, sentence, short } from "../lib/format";
import { toneOf } from "../lib/tone";

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
    <div>
      <PageHeader title="Settings" />
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
      <div className="space-y-4 pt-4">
        <ErrorBanner error={settings.error} />
        {tab === "general" && (settings.data ? <General config={settings.data.config} paths={settings.data.paths} /> : <Skeleton />)}
        {tab === "providers" && (settings.data ? <Providers config={settings.data.config} /> : <Skeleton />)}
        {tab === "agents" && <Agents />}
        {tab === "verification" && <Verification ws={ws} />}
        {tab === "claude" && <Claude />}
        {tab === "worktrees" && <Worktrees ws={ws} />}
      </div>
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

/** Save control that pins above the tab bar on phones while there are unsaved changes. */
function SaveBar({ dirty, save, onSave, label = "Save settings" }: { dirty: boolean; save: ReturnType<typeof useSave>; onSave: () => void; label?: string }) {
  const bar = (
    <>
      <Button variant="primary" disabled={!dirty} busy={save.isPending} onClick={onSave}>
        {label}
      </Button>
      {save.isSuccess && !dirty && <span className="self-center text-[13px] text-ok">Saved</span>}
    </>
  );
  return (
    <div className="space-y-2">
      <ErrorBanner error={save.error} />
      {dirty ? <ActionBar>{bar}</ActionBar> : <div className="flex gap-3">{bar}</div>}
    </div>
  );
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
    <input type="number" inputMode="numeric" min={min} className={inputClass} value={String(val(key, current))} onChange={(e) => set(key, Number(e.target.value))} />
  );
  return (
    <div className="space-y-4">
      <div className="grid gap-4 lg:grid-cols-2">
        <Card title="Agent execution">
          <div className="space-y-4">
            <div>
              <Checkbox label="Allow agents to execute code" checked={Boolean(val("execution.enabled", execution.enabled))} onChange={(v) => set("execution.enabled", v)} />
              <Checkbox
                label="Schedule ready auto-execute tasks automatically"
                checked={Boolean(val("execution.auto_schedule", execution.auto_schedule))}
                onChange={(v) => set("execution.auto_schedule", v)}
              />
            </div>
            <Field label="Default agent">
              <input className={inputClass} value={String(val("execution.default_agent", execution.default_agent))} onChange={(e) => set("execution.default_agent", e.target.value)} />
            </Field>
            <div className="grid grid-cols-2 gap-3">
              <Field label="Max concurrent runs">{numberInput("execution.max_concurrent_runs", execution.max_concurrent_runs)}</Field>
              <Field label="Max attempts per task">{numberInput("execution.max_attempts_per_task", execution.max_attempts_per_task)}</Field>
            </div>
            <Field label="Isolation">
              <select className={`${selectClass} w-full`} value={String(val("execution.isolation", execution.isolation))} onChange={(e) => set("execution.isolation", e.target.value)}>
                <option value="worktree">Managed worktree per attempt</option>
                <option value="in_place">In place, with an exclusive lease</option>
              </select>
            </Field>
            <p className="text-[13px] text-ink-3">
              Push: {String(execution.allow_push)}. Merge: {String(execution.allow_merge)}. Both are set in the config file only.
            </p>
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
        </Card>
      </div>
      <SaveBar dirty={Object.keys(values).length > 0} save={save} onSave={() => save.mutate(values)} />
      <Card title="Paths">
        <KeyValue items={Object.entries(paths).map(([k, v]) => [k.replace(/_/g, " "), <span className="mono break-all text-[12px]">{v}</span>])} />
      </Card>
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
    <fieldset>
      <legend className="mb-2 text-[14px] font-semibold">{sentence(cap)}</legend>
      <div className="grid grid-cols-2 gap-3">
        <Field label="Provider">
          <select
            className={`${selectClass} w-full`}
            value={String(values[`llm.${cap}.provider`] ?? llm[cap].provider)}
            onChange={(e) => setValues((v) => ({ ...v, [`llm.${cap}.provider`]: e.target.value }))}
          >
            {providers.map((p) => (
              <option key={p}>{p}</option>
            ))}
          </select>
        </Field>
        <Field label="Model">
          <input
            className={inputClass}
            autoCapitalize="none"
            value={String(values[`llm.${cap}.model`] ?? llm[cap].model)}
            onChange={(e) => setValues((v) => ({ ...v, [`llm.${cap}.model`]: e.target.value }))}
          />
        </Field>
      </div>
    </fieldset>
  );
  return (
    <div className="grid gap-4 lg:grid-cols-2 lg:items-start">
      <Card title="Model selection">
        <div className="space-y-5">
          {selection("classification")}
          {selection("embeddings")}
          {selection("summarization")}
          <div>
            <Checkbox
              label="Classify tasks automatically when created or materially changed"
              checked={Boolean(values["llm.auto_classify"] ?? llm.auto_classify)}
              onChange={(v) => setValues((x) => ({ ...x, "llm.auto_classify": v }))}
            />
            <Checkbox
              label="Embed tasks automatically for semantic search"
              checked={Boolean(values["llm.auto_embed"] ?? llm.auto_embed)}
              onChange={(v) => setValues((x) => ({ ...x, "llm.auto_embed": v }))}
            />
          </div>
          <SaveBar dirty={Object.keys(values).length > 0} save={save} onSave={() => save.mutate(values)} label="Save" />
        </div>
      </Card>
      <div className="min-w-0">
        <h2 className="mb-2 px-1 text-[17px] font-semibold lg:text-[15px]">Providers</h2>
        <ErrorBanner error={health.error} />
        <List>
          {providers.map((name) => (
            <ListRow key={name} tone={health.data ? (health.data[name]?.available ? "done" : "failed") : undefined}>
              <div className="flex items-center gap-2">
                <span className="font-medium">{name}</span>
                {health.data ? <StatusBadge status={health.data[name]?.available ? "ok" : "unavailable"} /> : <span className="text-[13px] text-ink-3">Checking…</span>}
              </div>
              <Meta>
                <span>{String(llm.providers[name].type)}</span>
                <span className="mono break-all text-[12px]">{String(llm.providers[name].base_url ?? "built-in")}</span>
              </Meta>
              {health.data?.[name]?.detail && <div className="mt-1 text-[13px] text-ink-2">{health.data[name].detail}</div>}
            </ListRow>
          ))}
        </List>
        <p className="mt-2 px-1 text-[12px] text-ink-3">Workgraph works without any model server: model jobs fail clearly and tracking carries on.</p>
      </div>
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
    <List testId="agents">
      {agents.data.map((a) => (
        <ListRow key={a.name} tone={a.available ? "done" : "failed"}>
          <div className="flex items-center gap-2">
            <span className="font-semibold">{a.name}</span>
            <span className="text-[13px] text-ink-3">{a.adapter}</span>
            <StatusBadge status={a.available ? "ok" : "unavailable"} />
          </div>
          {a.detail && <div className="mt-1 text-[13px] text-ink-2">{a.detail}</div>}
          <div className="mt-2 flex flex-wrap gap-1">
            {Object.entries(a.capabilities)
              .filter(([, v]) => v)
              .map(([k]) => (
                <span key={k} className="rounded-md bg-sunken px-1.5 py-px text-[12px] text-ink-2 ring-1 ring-line">
                  {k.replace(/_/g, " ")}
                </span>
              ))}
          </div>
          <details className="mt-2">
            <summary className="cursor-pointer text-[13px] text-accent">Configuration</summary>
            <pre className="mono mt-1 overflow-x-auto rounded-lg bg-sunken p-2 text-[12px]">{JSON.stringify(a.config, null, 2)}</pre>
          </details>
        </ListRow>
      ))}
    </List>
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
    <div className="grid gap-4 xl:grid-cols-[minmax(0,3fr)_minmax(0,2fr)] xl:items-start">
      <section className="min-w-0">
        <h2 className="mb-2 px-1 text-[17px] font-semibold lg:text-[15px]">Approved configurations</h2>
        <ErrorBanner error={configs.error ?? toggle.error} />
        {!configs.data ? (
          <Card bodyClass="p-0">
            <Skeleton />
          </Card>
        ) : configs.data.length === 0 ? (
          <Card>
            <Empty title="No verification configured">Without checks, tasks need a recorded manual acceptance.</Empty>
          </Card>
        ) : (
          <List>
            {configs.data.map((c) => (
              <ListRow
                key={c.id}
                tone={c.enabled ? "done" : "idle"}
                trailing={
                  <Button size="sm" onClick={() => toggle.mutate(c)}>
                    {c.enabled ? "Disable" : "Enable"}
                  </Button>
                }
              >
                <div className="flex items-baseline gap-2">
                  <span className="font-semibold">{c.name}</span>
                  <span className="text-[12px] text-ink-3">v{c.version}</span>
                </div>
                <Meta>
                  <span>
                    {sentence(c.scope_type)} <span className="mono text-[12px]">{short(c.scope_id)}</span>
                  </span>
                  <span>Approved by {c.approved_by}</span>
                </Meta>
                <ul className="mt-2 space-y-1 text-[13px]">
                  {c.checks.map((k) => (
                    <li key={k.name} className="break-words">
                      <span className="font-medium">{k.name}</span>{" "}
                      <span className="text-ink-3">
                        {k.kind}
                        {k.required ? "" : ", optional"}
                      </span>{" "}
                      <span className="mono text-[12px]">{k.command.join(" ")}</span>
                    </li>
                  ))}
                </ul>
              </ListRow>
            ))}
          </List>
        )}
      </section>
      <Card title="Add or update a configuration">
        <form
          className="space-y-4"
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
          <div className="grid grid-cols-2 gap-3">
            <Field label="Scope type">
              <select className={`${selectClass} w-full`} value={scopeType} onChange={(e) => setScopeType(e.target.value)}>
                {["workspace", "project", "repository", "task"].map((s) => (
                  <option key={s} value={s}>
                    {sentence(s)}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="Scope">
              <input className={inputClass} value={scope} onChange={(e) => setScope(e.target.value)} disabled={scopeType === "workspace"} autoCapitalize="none" />
            </Field>
          </div>
          <p className="-mt-2 text-[12px] text-ink-3">Scope is a repository name, project slug or task key.</p>
          <Field label="Name">
            <input className={inputClass} value={name} onChange={(e) => setName(e.target.value)} required autoCapitalize="none" />
          </Field>
          <Field label="Checks (JSON)" hint="Commands are argument vectors; no shell is involved.">
            <textarea
              aria-label="Checks JSON"
              spellCheck={false}
              autoCapitalize="none"
              autoCorrect="off"
              className={`${textareaClass} mono h-36 text-[13px]`}
              value={checks}
              onChange={(e) => setChecks(e.target.value)}
            />
          </Field>
          {parseError && <div className="text-[13px] text-danger">{parseError}</div>}
          <ErrorBanner error={create.error} />
          <div className="flex items-center gap-3">
            <Button type="submit" variant="primary" className="w-full sm:w-auto" busy={create.isPending}>
              Save configuration
            </Button>
            {create.isSuccess && <span className="text-[13px] text-ok">Saved</span>}
          </div>
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
    <div className="space-y-4">
      <div className="grid gap-4 lg:grid-cols-2">
        <Card
          title="Hook installation, user scope"
          footer={
            <>
              Install with <span className="mono">workgraph claude install</span>. Hooks are passive (async, no output) and never change session permissions.
            </>
          }
        >
          <KeyValue
            items={[
              ["settings file", <span className="mono break-all text-[12px]">{user.settings}</span>],
              ["installed events", user.installed_events.length ? user.installed_events.join(", ") : "None"],
              ["missing events", user.missing_events.join(", ") || "None"],
              ["command resolves", user.command_resolves ? "Yes" : "No"],
              ["error", user.error],
            ]}
          />
        </Card>
        <Card title="Ingestion">
          <KeyValue
            items={[
              ["hook events", hooks.hook_events],
              ["last received", ago(hooks.last_received_at) || "Never"],
              ["spooled, awaiting replay", hooks.spooled],
              ["spool failures", hooks.spool_failed],
              ["by event", Object.entries(hooks.by_event).map(([k, v]) => `${k}: ${v}`).join(", ")],
            ]}
          />
        </Card>
      </div>
      <section>
        <h2 className="mb-2 px-1 text-[17px] font-semibold lg:text-[15px]">Recent sessions</h2>
        {sessions.length ? (
          <List>
            {sessions.map((s) => (
              <ListRow key={s.id}>
                <div className="mono truncate text-[13px]">{s.external_session_id}</div>
                <Meta>
                  <span>{sentence(s.binding)}</span>
                  {s.task_id && <span className="mono text-[12px]">{short(s.task_id)}</span>}
                  <span>{s.tool_calls} tool calls</span>
                  <span className={s.tool_failures ? "text-danger" : ""}>{s.tool_failures} failures</span>
                  <span>{ago(s.started_at)}</span>
                </Meta>
              </ListRow>
            ))}
          </List>
        ) : (
          <Card>
            <Empty title="No Claude Code sessions recorded" />
          </Card>
        )}
      </section>
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
  if (!worktrees.data.length)
    return (
      <Card>
        <Empty title="No managed worktrees">One is created per attempt when agents run in isolation.</Empty>
      </Card>
    );
  return (
    <div>
      <List>
        {worktrees.data.map((w) => (
          <ListRow key={w.id} tone={toneOf(w.status)}>
            <div className="flex items-center gap-2">
              <span className="font-medium">{w.repository}</span>
              <StatusBadge status={w.status} />
              <span className="ml-auto text-[13px] text-ink-3">{ago(w.created_at)}</span>
            </div>
            <Meta>
              <span className="mono text-[12px]">{w.branch}</span>
              <span className="mono text-[12px]">base {short(w.base_sha, 10)}</span>
            </Meta>
            <div className="mono mt-1 break-all text-[12px] text-ink-3">{w.path}</div>
            {w.last_error && <div className="mt-1 text-[13px] text-danger">{w.last_error}</div>}
          </ListRow>
        ))}
      </List>
      <p className="mt-2 px-1 text-[12px] text-ink-3">
        Released worktrees are removed only by an explicit <span className="mono">workgraph git cleanup-worktrees --apply</span>. Branches are always kept.
      </p>
    </div>
  );
}
