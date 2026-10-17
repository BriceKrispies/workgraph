import { useState } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { get, post } from "../api/client";
import type { AnalyticsResult, Repository, RepositoryStatus } from "../api/types";
import {
  Button,
  Card,
  Empty,
  ErrorBanner,
  Field,
  KeyValue,
  Loading,
  Modal,
  Skeleton,
  StatusBadge,
  Table,
  Tag,
  inputClass,
  td,
} from "../components/ui";
import { CategoryBars } from "../features/charts";
import { useProjectNames } from "../hooks/lookups";
import { useWorkspace } from "../hooks/workspace";
import { ago, short } from "../lib/format";

interface CommitRow {
  id: string;
  sha: string;
  subject: string;
  author: string;
  committed_at: string | null;
  insertions: number;
  deletions: number;
  files_changed: number;
  is_merge: boolean;
  reachable: boolean;
  branches: string[];
  tasks: { task_key: string; source: string; confidence: number }[];
}

interface CommitDetail {
  sha: string;
  subject: string;
  message: string;
  parents: string[];
  author: { name: string; email: string; at: string | null };
  is_merge: boolean;
  reachable: boolean;
  object_available: boolean;
  trailers: { key: string; value: string }[];
  files: {
    path: string;
    old_path: string | null;
    change_type: string;
    insertions: number;
    deletions: number;
    binary: boolean;
    language: string | null;
    file_class: string;
    symbols: { kind: string; name: string; change: string; lines_changed: number }[];
  }[];
  analysis: { functions_affected: number; types_affected: number; files_parsed: number; files_fallback: number } | null;
  equivalences: { kind: string; evidence: Record<string, unknown> }[];
  tasks: { task_key: string; source: string; confidence: number }[];
}

export function RepositoriesPage() {
  const { ws } = useWorkspace();
  const client = useQueryClient();
  const projects = useProjectNames(ws);
  const [registering, setRegistering] = useState(false);
  const [path, setPath] = useState("");
  const [readOnly, setReadOnly] = useState(false);
  const repos = useQuery({ queryKey: ["repositories", ws], queryFn: () => get<Repository[]>(`/ws/${ws}/repositories`) });
  const register = useMutation({
    mutationFn: () => post<Repository>(`/ws/${ws}/repositories`, { path, access_mode: readOnly ? "read_only" : "read_write" }),
    onSuccess: () => {
      setRegistering(false);
      setPath("");
      void client.invalidateQueries({ queryKey: ["repositories"] });
    },
  });
  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold text-slate-900">Repositories</h1>
        <Button variant="primary" onClick={() => setRegistering(true)}>
          Register repository
        </Button>
      </div>
      <ErrorBanner error={repos.error} />
      <Card bodyClass="p-0">
        {!repos.data ? (
          <Skeleton />
        ) : repos.data.length === 0 ? (
          <div className="p-4">
            <Empty title="No repositories registered">Register a local Git checkout to start ingesting history.</Empty>
          </div>
        ) : (
          <Table head={["Name", "Health", "Access", "Default branch", "Checkouts", "Projects", "Last scan"]} testId="repositories">
            {repos.data.map((r) => (
              <tr key={r.id}>
                <td className={td}>
                  <Link to={`/repositories/${r.name}`} className="font-medium text-sky-700 hover:underline">
                    {r.name}
                  </Link>
                  <div className="mono truncate text-[11px] text-slate-400">{r.checkouts[0]?.path}</div>
                </td>
                <td className={td}>
                  <StatusBadge status={r.health_status} />
                </td>
                <td className={td}>{r.access_mode === "read_only" ? "read-only" : "read/write"}</td>
                <td className={td}>{r.default_branch ?? "—"}</td>
                <td className={td}>{r.checkouts.length}</td>
                <td className={td}>
                  <div className="flex flex-wrap gap-1">
                    {r.project_ids.map((p) => (
                      <Tag key={p}>{projects.get(p)?.slug ?? p.slice(0, 6)}</Tag>
                    ))}
                  </div>
                </td>
                <td className={`${td} text-slate-500`}>{ago(r.last_scanned_at) || "never"}</td>
              </tr>
            ))}
          </Table>
        )}
      </Card>
      <Modal title="Register repository" open={registering} onClose={() => setRegistering(false)}>
        <form
          className="space-y-3"
          onSubmit={(e) => {
            e.preventDefault();
            register.mutate();
          }}
        >
          <Field label="Local path" hint="An existing Git working tree. Files are never modified or deleted.">
            <input className={`${inputClass} mono`} value={path} onChange={(e) => setPath(e.target.value)} required />
          </Field>
          <label className="flex items-center gap-2 text-sm">
            <input type="checkbox" checked={readOnly} onChange={(e) => setReadOnly(e.target.checked)} /> Read-only for agents
          </label>
          <ErrorBanner error={register.error} />
          <div className="flex justify-end">
            <Button type="submit" variant="primary" busy={register.isPending}>
              Register
            </Button>
          </div>
        </form>
      </Modal>
    </div>
  );
}

export function RepositoryDetailPage() {
  const { ref = "" } = useParams();
  const { ws } = useWorkspace();
  const client = useQueryClient();
  const [params, setParams] = useSearchParams();
  const sha = params.get("sha");
  const [linkedOnly, setLinkedOnly] = useState(false);
  const repo = useQuery({ queryKey: ["repositories", ws, ref], queryFn: () => get<Repository>(`/ws/${ws}/repositories/${ref}`) });
  const status = useQuery({
    queryKey: ["repositories", ws, ref, "status"],
    queryFn: () => get<RepositoryStatus>(`/ws/${ws}/repositories/${ref}/status`),
  });
  const commits = useQuery({
    queryKey: ["repositories", ws, ref, "commits", linkedOnly],
    queryFn: () => get<{ total: number; items: CommitRow[] }>(`/ws/${ws}/repositories/${ref}/commits`, { limit: 100, linked_only: linkedOnly }),
  });
  const detail = useQuery({
    queryKey: ["repositories", ws, ref, "commit", sha],
    queryFn: () => get<CommitDetail>(`/ws/${ws}/repositories/${ref}/commits/${sha}`),
    enabled: !!sha,
  });
  const hotspots = useQuery({
    queryKey: ["analytics", ws, "module_hotspots", ref],
    queryFn: () =>
      post<AnalyticsResult>(`/ws/${ws}/analytics/query`, { metric: "module_hotspots", group_by: ["module"], filters: { repository: ref } }),
  });
  const invalidate = () => client.invalidateQueries({ queryKey: ["repositories"] });
  const refresh = useMutation({ mutationFn: () => post(`/ws/${ws}/repositories/${ref}/refresh`), onSuccess: invalidate });
  const sync = useMutation({ mutationFn: () => post(`/ws/${ws}/repositories/${ref}/sync`), onSuccess: invalidate });

  if (repo.isLoading) return <Loading />;
  if (!repo.data) return <ErrorBanner error={repo.error} />;
  const r = repo.data;
  const s = status.data;
  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-start justify-between gap-2">
        <div>
          <div className="text-sm text-slate-500">
            <Link to="/repositories" className="hover:underline">
              Repositories
            </Link>{" "}
            / {r.name}
          </div>
          <h1 className="flex items-center gap-2 text-xl font-semibold text-slate-900">
            {r.name} <StatusBadge status={r.health_status} />
          </h1>
        </div>
        <div className="flex gap-1.5">
          <Button busy={refresh.isPending} onClick={() => refresh.mutate()}>
            Health check
          </Button>
          <Button busy={sync.isPending} onClick={() => sync.mutate()}>
            Sync commits
          </Button>
        </div>
      </div>
      <ErrorBanner error={refresh.error ?? sync.error} />
      {sync.data !== undefined && (
        <div className="text-[12px] text-slate-500">Sync result: {JSON.stringify(sync.data)}</div>
      )}
      <div className="grid gap-4 lg:grid-cols-3">
        <Card title="Identity">
          <KeyValue
            items={[
              ["id", <span className="mono text-[11px]">{r.id}</span>],
              ["fingerprint", r.identity_fingerprint ? <span className="mono text-[11px]">{short(r.identity_fingerprint, 16)}</span> : "no commits"],
              ["default branch", r.default_branch],
              ["tracked", r.tracked_branches.join(", ")],
              ["access", r.access_mode],
              ["shallow", r.is_shallow ? "yes" : "no"],
              ["last scanned", ago(r.last_scanned_at)],
              ["remotes", r.remotes.length ? r.remotes.map((m) => `${m.name}: ${m.url}`).join(", ") : "none"],
            ]}
          />
        </Card>
        <Card title="Working tree">
          <ErrorBanner error={status.error} />
          {s ? (
            s.available ? (
              <KeyValue
                items={[
                  ["path", <span className="mono text-[11px]">{s.checkout_path}</span>],
                  ["branch", s.branch ?? "(detached)"],
                  ["head", <span className="mono text-[11px]">{short(s.head, 12)}</span>],
                  ["ahead / behind", s.ahead !== null ? `${s.ahead} / ${s.behind}` : null],
                  ["staged", s.staged.length],
                  ["unstaged", s.unstaged.length],
                  ["untracked", s.untracked.length],
                ]}
              />
            ) : (
              <Empty title="Checkout unavailable">{s.error}</Empty>
            )
          ) : (
            <Skeleton />
          )}
        </Card>
        <Card title="Checkouts">
          <ul className="space-y-1.5">
            {r.checkouts.map((c) => (
              <li key={c.id} className="text-[12px]">
                <div className="mono break-all">{c.path}</div>
                <div className="text-slate-500">
                  {c.kind} {c.is_primary && "· primary"} · {c.available ? "available" : <span className="text-rose-600">unavailable</span>}
                </div>
              </li>
            ))}
          </ul>
        </Card>
      </div>
      {sha && (
        <Card
          title={`Commit ${short(sha, 10)}`}
          actions={
            <Button size="sm" variant="ghost" onClick={() => setParams({})}>
              close
            </Button>
          }
        >
          <ErrorBanner error={detail.error} />
          {detail.data ? (
            <div className="space-y-3">
              <div>
                <div className="font-medium">{detail.data.subject}</div>
                <div className="text-[12px] text-slate-500">
                  {detail.data.author.name} · {ago(detail.data.author.at)} · parents {detail.data.parents.map((p) => short(p, 8)).join(", ")}
                  {!detail.data.reachable && <span className="ml-2 text-amber-700">unreachable (rewritten history)</span>}
                </div>
                {detail.data.trailers.length > 0 && (
                  <div className="mt-1 flex flex-wrap gap-1">
                    {detail.data.trailers.map((t) => (
                      <Tag key={t.key + t.value}>
                        {t.key}: {t.value}
                      </Tag>
                    ))}
                  </div>
                )}
              </div>
              <KeyValue
                items={[
                  ["tasks", detail.data.tasks.map((t) => `${t.task_key} (${t.source}, ${t.confidence.toFixed(2)})`).join(", ") || "none"],
                  [
                    "structure",
                    detail.data.analysis
                      ? `${detail.data.analysis.functions_affected} functions, ${detail.data.analysis.types_affected} types (${detail.data.analysis.files_parsed} parsed, ${detail.data.analysis.files_fallback} line-level)`
                      : "not analysed yet",
                  ],
                  ["equivalences", detail.data.equivalences.map((e) => e.kind).join(", ")],
                ]}
              />
              <Table head={["", "Path", "Lines", "Class", "Language", "Symbols"]}>
                {detail.data.files.map((f) => (
                  <tr key={f.path}>
                    <td className={`${td} mono`}>{f.change_type}</td>
                    <td className={`${td} mono text-[12px]`}>
                      {f.old_path ? `${f.old_path} → ` : ""}
                      {f.path}
                    </td>
                    <td className={td}>{f.binary ? "binary" : `+${f.insertions} -${f.deletions}`}</td>
                    <td className={td}>{f.file_class}</td>
                    <td className={td}>{f.language ?? "—"}</td>
                    <td className={`${td} text-[12px]`}>{f.symbols.map((sym) => `${sym.name} (${sym.change})`).join(", ")}</td>
                  </tr>
                ))}
              </Table>
            </div>
          ) : (
            !detail.error && <Skeleton />
          )}
        </Card>
      )}
      <div className="grid gap-4 xl:grid-cols-3">
        <Card
          title={`Commits (${commits.data?.total ?? "…"})`}
          className="xl:col-span-2"
          bodyClass="p-0"
          actions={
            <label className="flex items-center gap-1 text-[12px] normal-case">
              <input type="checkbox" checked={linkedOnly} onChange={(e) => setLinkedOnly(e.target.checked)} /> task-linked only
            </label>
          }
        >
          <ErrorBanner error={commits.error} />
          {!commits.data ? (
            <Skeleton />
          ) : commits.data.items.length === 0 ? (
            <div className="p-3">
              <Empty title="No commits ingested">Use “Sync commits”.</Empty>
            </div>
          ) : (
            <Table head={["SHA", "Subject", "Author", "Lines", "Tasks", "When"]} testId="repo-commits">
              {commits.data.items.map((c) => (
                <tr key={c.id} className="cursor-pointer hover:bg-slate-50" onClick={() => setParams({ sha: c.sha })}>
                  <td className={`${td} mono text-[12px]`}>{short(c.sha, 10)}</td>
                  <td className={td}>
                    {c.subject}
                    {c.is_merge && <Tag>merge</Tag>}
                  </td>
                  <td className={`${td} text-slate-600`}>{c.author}</td>
                  <td className={`${td} whitespace-nowrap`}>
                    <span className="text-emerald-700">+{c.insertions}</span> <span className="text-rose-700">-{c.deletions}</span>
                  </td>
                  <td className={td} onClick={(e) => e.stopPropagation()}>
                    {c.tasks.map((t) => (
                      <Link key={t.task_key + t.source} to={`/tasks/${t.task_key}`} className="mono mr-1 text-[12px] text-sky-700 hover:underline" title={`${t.source} (${t.confidence})`}>
                        {t.task_key}
                      </Link>
                    ))}
                  </td>
                  <td className={`${td} whitespace-nowrap text-slate-500`}>{ago(c.committed_at)}</td>
                </tr>
              ))}
            </Table>
          )}
        </Card>
        <Card title="Change volume by module">
          <ErrorBanner error={hotspots.error} />
          {hotspots.data ? <CategoryBars rows={hotspots.data.rows} category="module" height={200} /> : <Skeleton />}
          <p className="mt-1 text-[11px] text-slate-400">Lines changed by task-linked commits. Lines are not a measure of difficulty.</p>
        </Card>
      </div>
    </div>
  );
}
