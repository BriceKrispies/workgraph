import { useState } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { get, post } from "../api/client";
import type { AnalyticsResult, Repository, RepositoryStatus } from "../api/types";
import {
  Button,
  Card,
  Checkbox,
  Diff,
  Empty,
  ErrorBanner,
  Field,
  KeyValue,
  List,
  ListRow,
  Loading,
  Meta,
  Modal,
  Notice,
  PageHeader,
  SectionTitle,
  Skeleton,
  StatusBadge,
  Tag,
  inputClass,
} from "../components/ui";
import { CategoryBars } from "../features/charts";
import { useProjectNames } from "../hooks/lookups";
import { useDesktop } from "../hooks/media";
import { useWorkspace } from "../hooks/workspace";
import { ago, short } from "../lib/format";
import { toneOf } from "../lib/tone";

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
    <div>
      <PageHeader
        title="Repositories"
        actions={
          <Button variant="primary" icon="plus" onClick={() => setRegistering(true)}>
            Register
          </Button>
        }
      />
      <ErrorBanner error={repos.error} />
      {!repos.data ? (
        <Card bodyClass="p-0">
          <Skeleton />
        </Card>
      ) : repos.data.length === 0 ? (
        <Card>
          <Empty
            title="No repositories registered"
            action={
              <Button variant="primary" onClick={() => setRegistering(true)}>
                Register repository
              </Button>
            }
          >
            Register a local Git checkout to start ingesting its history.
          </Empty>
        </Card>
      ) : (
        <List testId="repositories">
          {repos.data.map((r) => (
            <ListRow key={r.id} to={`/repositories/${r.name}`} tone={toneOf(r.health_status)}>
              <div className="flex items-center gap-2">
                <span className="truncate font-semibold">{r.name}</span>
                <StatusBadge status={r.health_status} />
              </div>
              {r.checkouts[0] && <div className="mono mt-0.5 truncate text-[12px] text-ink-3">{r.checkouts[0].path}</div>}
              <Meta>
                <span>{r.default_branch ?? "No default branch"}</span>
                <span>{r.access_mode === "read_only" ? "Read-only" : "Read and write"}</span>
                <span>
                  {r.checkouts.length} {r.checkouts.length === 1 ? "checkout" : "checkouts"}
                </span>
                <span>{r.last_scanned_at ? `Scanned ${ago(r.last_scanned_at)}` : "Never scanned"}</span>
                {r.project_ids.length > 0 && (
                  <span className="flex flex-wrap gap-1">
                    {r.project_ids.map((p) => (
                      <Tag key={p}>{projects.get(p)?.slug ?? p.slice(0, 6)}</Tag>
                    ))}
                  </span>
                )}
              </Meta>
            </ListRow>
          ))}
        </List>
      )}
      <Modal title="Register repository" open={registering} onClose={() => setRegistering(false)}>
        <form
          className="space-y-4"
          onSubmit={(e) => {
            e.preventDefault();
            register.mutate();
          }}
        >
          <Field label="Local path" hint="An existing Git working tree. Workgraph never modifies or deletes its files.">
            <input className={`${inputClass} mono`} value={path} onChange={(e) => setPath(e.target.value)} required autoCapitalize="none" autoCorrect="off" />
          </Field>
          <Checkbox label="Read-only for agents" checked={readOnly} onChange={setReadOnly} />
          <ErrorBanner error={register.error} />
          <Button type="submit" variant="primary" className="w-full md:ml-auto md:flex md:w-auto" busy={register.isPending}>
            Register
          </Button>
        </form>
      </Modal>
    </div>
  );
}

export function RepositoryDetailPage() {
  const { ref = "" } = useParams();
  const { ws } = useWorkspace();
  const desktop = useDesktop();
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
    <div>
      <PageHeader back={{ to: "/repositories", label: "Repositories" }} eyebrow={<StatusBadge status={r.health_status} />} title={r.name}>
        <div className="flex gap-2 [&>*]:flex-1 sm:[&>*]:flex-none">
          <Button busy={refresh.isPending} icon="refresh" onClick={() => refresh.mutate()}>
            Health check
          </Button>
          <Button busy={sync.isPending} onClick={() => sync.mutate()}>
            Sync commits
          </Button>
        </div>
      </PageHeader>
      {/* phones put the commits first; the repository facts follow */}
      <div className="flex flex-col gap-4">
        <ErrorBanner error={refresh.error ?? sync.error} />
        {sync.data !== undefined && (
          <Notice>
            Sync result: <span className="mono break-all text-[12px]">{JSON.stringify(sync.data)}</span>
          </Notice>
        )}
        <div className="order-3 grid gap-4 lg:order-none lg:grid-cols-3">
          <Card title="Identity">
            <KeyValue
              items={[
                ["id", <span className="mono text-[12px]">{r.id}</span>],
                ["fingerprint", r.identity_fingerprint ? <span className="mono text-[12px]">{short(r.identity_fingerprint, 16)}</span> : "No commits"],
                ["default branch", r.default_branch],
                ["tracked", r.tracked_branches.join(", ")],
                ["access", r.access_mode === "read_only" ? "Read-only" : "Read and write"],
                ["shallow", r.is_shallow ? "Yes" : "No"],
                ["last scanned", ago(r.last_scanned_at)],
                ["remotes", r.remotes.length ? r.remotes.map((m) => `${m.name}: ${m.url}`).join(", ") : "None"],
              ]}
            />
          </Card>
          <Card title="Working tree">
            <ErrorBanner error={status.error} />
            {s ? (
              s.available ? (
                <KeyValue
                  items={[
                    ["path", <span className="mono break-all text-[12px]">{s.checkout_path}</span>],
                    ["branch", s.branch ?? "(detached)"],
                    ["head", <span className="mono text-[12px]">{short(s.head, 12)}</span>],
                    ["ahead, behind", s.ahead !== null ? `${s.ahead}, ${s.behind}` : null],
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
          <Card title="Checkouts" bodyClass="p-0">
            <ul className="divide-y divide-line">
              {r.checkouts.map((c) => (
                <li key={c.id} className="px-4 py-3">
                  <div className="mono break-all text-[12px]">{c.path}</div>
                  <Meta>
                    <span>{c.kind}</span>
                    {c.is_primary && <span>Primary</span>}
                    {c.available ? <span>Available</span> : <span className="text-danger">Unavailable</span>}
                  </Meta>
                </li>
              ))}
            </ul>
          </Card>
        </div>

        {sha && desktop && (
          <Card
            title={<span className="mono">Commit {short(sha, 10)}</span>}
            actions={
              <Button size="sm" variant="ghost" onClick={() => setParams({})}>
                Close
              </Button>
            }
          >
            <CommitDetailView ws={ws} repo={ref} sha={sha} />
          </Card>
        )}

        <div className="grid gap-4 xl:grid-cols-[minmax(0,2fr)_minmax(0,1fr)] xl:items-start">
          <section className="min-w-0">
            <SectionTitle actions={<Checkbox label="Task-linked only" checked={linkedOnly} onChange={setLinkedOnly} />}>
              Commits <span className="font-normal text-ink-3 tnum">{commits.data?.total ?? ""}</span>
            </SectionTitle>
            <ErrorBanner error={commits.error} />
            {!commits.data ? (
              <Card bodyClass="p-0">
                <Skeleton />
              </Card>
            ) : commits.data.items.length === 0 ? (
              <Card>
                <Empty title="No commits ingested">Use Sync commits to read them from Git.</Empty>
              </Card>
            ) : (
              <List testId="repo-commits">
                {commits.data.items.map((c) => (
                  <ListRow key={c.id} selected={sha === c.sha} onClick={() => setParams({ sha: c.sha })}>
                    <div className="line-clamp-2 font-medium leading-snug">
                      {c.subject}
                      {c.is_merge && (
                        <span className="ml-2 align-middle">
                          <Tag>merge</Tag>
                        </span>
                      )}
                    </div>
                    <Meta>
                      <span className="mono text-[12px] text-ink-2">{short(c.sha, 10)}</span>
                      <span>{c.author}</span>
                      <Diff added={c.insertions} removed={c.deletions} />
                      <span>{ago(c.committed_at)}</span>
                      {c.tasks.map((t) => (
                        <Link
                          key={t.task_key + t.source}
                          to={`/tasks/${t.task_key}`}
                          onClick={(e) => e.stopPropagation()}
                          className="mono text-[12px] font-medium text-accent hover:underline"
                          title={`${t.source} (${t.confidence})`}
                        >
                          {t.task_key}
                        </Link>
                      ))}
                    </Meta>
                  </ListRow>
                ))}
              </List>
            )}
          </section>
          <Card title="Change volume by module" footer="Lines changed by task-linked commits. Lines are not a measure of difficulty.">
            <ErrorBanner error={hotspots.error} />
            {hotspots.data ? <CategoryBars rows={hotspots.data.rows} category="module" /> : <Skeleton />}
          </Card>
        </div>
      </div>
      {!desktop && (
        <Modal title={sha ? `Commit ${short(sha, 10)}` : "Commit"} open={!!sha} onClose={() => setParams({})} wide>
          {sha && <CommitDetailView ws={ws} repo={ref} sha={sha} />}
        </Modal>
      )}
    </div>
  );
}

function CommitDetailView({ ws, repo, sha }: { ws: string; repo: string; sha: string }) {
  const detail = useQuery({
    queryKey: ["repositories", ws, repo, "commit", sha],
    queryFn: () => get<CommitDetail>(`/ws/${ws}/repositories/${repo}/commits/${sha}`),
  });
  if (detail.error) return <ErrorBanner error={detail.error} />;
  if (!detail.data) return <Skeleton />;
  const d = detail.data;
  return (
    <div className="space-y-4">
      <div>
        <div className="font-semibold leading-snug">{d.subject}</div>
        <Meta>
          <span>{d.author.name}</span>
          <span>{ago(d.author.at)}</span>
          <span className="mono text-[12px]">parents {d.parents.map((p) => short(p, 8)).join(", ")}</span>
          {!d.reachable && <span className="text-warn">Unreachable (history was rewritten)</span>}
        </Meta>
        {d.trailers.length > 0 && (
          <div className="mt-2 flex flex-wrap gap-1">
            {d.trailers.map((t) => (
              <Tag key={t.key + t.value}>
                {t.key}: {t.value}
              </Tag>
            ))}
          </div>
        )}
      </div>
      <KeyValue
        items={[
          ["tasks", d.tasks.map((t) => `${t.task_key} (${t.source}, ${t.confidence.toFixed(2)})`).join(", ") || "None"],
          [
            "structure",
            d.analysis
              ? `${d.analysis.functions_affected} functions, ${d.analysis.types_affected} types (${d.analysis.files_parsed} parsed, ${d.analysis.files_fallback} line-level)`
              : "Not analysed yet",
          ],
          ["equivalences", d.equivalences.map((e) => e.kind).join(", ")],
        ]}
      />
      <div>
        <div className="mb-2 text-[14px] font-semibold">Files ({d.files.length})</div>
        <ul className="divide-y divide-line overflow-hidden rounded-xl border border-line">
          {d.files.map((f) => (
            <li key={f.path} className="px-3 py-2.5">
              <div className="flex items-start gap-2">
                <span className="mono mt-px w-5 shrink-0 text-center text-[12px] font-semibold text-ink-3" title={f.change_type}>
                  {f.change_type}
                </span>
                <span className="mono min-w-0 flex-1 break-all text-[12px]">
                  {f.old_path ? `${f.old_path} → ` : ""}
                  {f.path}
                </span>
                {f.binary ? <span className="text-[12px] text-ink-3">binary</span> : <Diff added={f.insertions} removed={f.deletions} />}
              </div>
              <div className="ml-7">
                <Meta className="mt-0.5 text-[12px]">
                  <span>{f.file_class}</span>
                  {f.language && <span>{f.language}</span>}
                </Meta>
                {f.symbols.length > 0 && <div className="mt-1 text-[12px] text-ink-2">{f.symbols.map((sym) => `${sym.name} (${sym.change})`).join(", ")}</div>}
              </div>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}
