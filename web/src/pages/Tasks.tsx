import { useMemo, useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { keepPreviousData, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { get, post } from "../api/client";
import {
  CATEGORIES,
  PRIORITIES,
  TASK_STATUSES,
  type Page,
  type Project,
  type Repository,
  type Task,
} from "../api/types";
import {
  Button,
  Empty,
  ErrorBanner,
  Modal,
  PriorityBadge,
  Skeleton,
  StatusBadge,
  Table,
  Tag,
  inputBase,
  td,
} from "../components/ui";
import { TaskForm } from "../features/TaskForm";
import { useWorkspace } from "../hooks/workspace";
import { ago, label } from "../lib/format";

const PAGE_SIZE = 50;
const FILTER_KEYS = ["status", "priority", "project", "repository", "tag", "category", "q"] as const;

export function TasksPage() {
  const { ws } = useWorkspace();
  const [params, setParams] = useSearchParams();
  const navigate = useNavigate();
  const client = useQueryClient();
  const [creating, setCreating] = useState(false);
  const [selected, setSelected] = useState<string[]>([]);
  const [bulkTag, setBulkTag] = useState("");
  const offset = Number(params.get("offset") ?? 0);
  const filters = Object.fromEntries(FILTER_KEYS.map((k) => [k, params.get(k) ?? ""])) as Record<
    (typeof FILTER_KEYS)[number],
    string
  >;
  const sort = params.get("sort") ?? "created_at";

  const projects = useQuery({ queryKey: ["projects", ws], queryFn: () => get<Project[]>(`/ws/${ws}/projects`) });
  const repos = useQuery({ queryKey: ["repositories", ws], queryFn: () => get<Repository[]>(`/ws/${ws}/repositories`) });
  const repoNames = useMemo(() => new Map((repos.data ?? []).map((r) => [r.id, r.name])), [repos.data]);

  const tasks = useQuery({
    queryKey: ["tasks", ws, filters, offset, sort],
    queryFn: () =>
      get<Page<Task>>(`/ws/${ws}/tasks`, {
        status: filters.status || undefined,
        priority: filters.priority || undefined,
        project: filters.project,
        repository: filters.repository,
        tag: filters.tag || undefined,
        category: filters.category,
        q: filters.q,
        sort,
        descending: sort !== "key",
        limit: PAGE_SIZE,
        offset,
      }),
    placeholderData: keepPreviousData,
  });

  const bulk = useMutation({
    mutationFn: (update: Record<string, unknown>) => post<{ updated: string[]; failed: Record<string, string> }>(
      `/ws/${ws}/tasks/bulk`,
      { tasks: selected, update },
    ),
    onSuccess: () => {
      setSelected([]);
      void client.invalidateQueries({ queryKey: ["tasks"] });
    },
  });

  const setFilter = (key: string, value: string) => {
    const next = new URLSearchParams(params);
    if (value) next.set(key, value);
    else next.delete(key);
    if (key !== "offset") next.delete("offset");
    setParams(next);
  };
  const page = tasks.data;
  const allSelected = !!page?.items.length && page.items.every((t) => selected.includes(t.key));

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold text-slate-900">Tasks</h1>
        <Button variant="primary" onClick={() => setCreating(true)}>
          New task
        </Button>
      </div>
      <div className="flex flex-wrap items-end gap-2 rounded-lg border border-slate-200 bg-white p-2" role="search">
        <input
          aria-label="Filter text"
          placeholder="Full-text search"
          className={`${inputBase} w-56`}
          defaultValue={filters.q}
          key={filters.q}
          onKeyDown={(e) => e.key === "Enter" && setFilter("q", (e.target as HTMLInputElement).value)}
          onBlur={(e) => e.target.value !== filters.q && setFilter("q", e.target.value)}
        />
        <Select label="Status" value={filters.status} options={TASK_STATUSES} onChange={(v) => setFilter("status", v)} />
        <Select label="Priority" value={filters.priority} options={[...PRIORITIES]} onChange={(v) => setFilter("priority", v)} />
        <Select label="Category" value={filters.category} options={[...CATEGORIES]} onChange={(v) => setFilter("category", v)} />
        <Select
          label="Project"
          value={filters.project}
          options={(projects.data ?? []).map((p) => p.slug)}
          onChange={(v) => setFilter("project", v)}
        />
        <Select
          label="Repository"
          value={filters.repository}
          options={(repos.data ?? []).map((r) => r.name)}
          onChange={(v) => setFilter("repository", v)}
        />
        <input
          aria-label="Tag"
          placeholder="tag"
          className={`${inputBase} w-28`}
          defaultValue={filters.tag}
          key={`tag-${filters.tag}`}
          onKeyDown={(e) => e.key === "Enter" && setFilter("tag", (e.target as HTMLInputElement).value)}
        />
        <select
          aria-label="Sort"
          className={`${inputBase} w-36`}
          value={sort}
          onChange={(e) => setFilter("sort", e.target.value)}
        >
          {["created_at", "updated_at", "priority", "key", "status", "title"].map((s) => (
            <option key={s} value={s}>
              sort: {label(s)}
            </option>
          ))}
        </select>
        {FILTER_KEYS.some((k) => filters[k]) && (
          <Button variant="ghost" size="sm" onClick={() => setParams(new URLSearchParams())}>
            clear filters
          </Button>
        )}
      </div>

      {selected.length > 0 && (
        <div className="flex flex-wrap items-center gap-2 rounded-md bg-slate-900 px-3 py-2 text-sm text-white" data-testid="bulk-bar">
          <span>{selected.length} selected</span>
          <select
            aria-label="Bulk status"
            className="rounded bg-slate-700 px-2 py-1"
            defaultValue=""
            onChange={(e) => e.target.value && bulk.mutate({ status: e.target.value })}
          >
            <option value="">set status…</option>
            {["backlog", "ready", "blocked", "cancelled"].map((s) => (
              <option key={s}>{s}</option>
            ))}
          </select>
          <input
            aria-label="Bulk tag"
            className="rounded bg-slate-700 px-2 py-1"
            placeholder="add tag"
            value={bulkTag}
            onChange={(e) => setBulkTag(e.target.value)}
          />
          <Button size="sm" disabled={!bulkTag} onClick={() => bulk.mutate({ add_tags: [bulkTag] })}>
            add tag
          </Button>
          <Button size="sm" variant="ghost" className="text-slate-300" onClick={() => setSelected([])}>
            clear
          </Button>
          {bulk.data && Object.keys(bulk.data.failed).length > 0 && (
            <span className="text-rose-300">failed: {Object.entries(bulk.data.failed).map(([k, v]) => `${k} (${v})`).join(", ")}</span>
          )}
        </div>
      )}
      <ErrorBanner error={tasks.error ?? bulk.error} onRetry={() => tasks.refetch()} />
      <div className="rounded-lg border border-slate-200 bg-white">
        {!page ? (
          <Skeleton rows={8} />
        ) : page.items.length === 0 ? (
          <div className="p-4">
            <Empty title="No tasks match">Adjust the filters or create a task.</Empty>
          </div>
        ) : (
          <Table
            testId="tasks-table"
            head={[
              <input
                key="all"
                type="checkbox"
                aria-label="select all"
                checked={allSelected}
                onChange={() => setSelected(allSelected ? [] : page.items.map((t) => t.key))}
              />,
              "Key",
              "Title",
              "Status",
              "Priority",
              "Category",
              "Repositories",
              "Tags",
              "Updated",
            ]}
          >
            {page.items.map((t) => (
              <tr key={t.id} className="cursor-pointer hover:bg-slate-50" onClick={() => navigate(`/tasks/${t.key}`)}>
                <td className={td} onClick={(e) => e.stopPropagation()}>
                  <input
                    type="checkbox"
                    aria-label={`select ${t.key}`}
                    checked={selected.includes(t.key)}
                    onChange={() =>
                      setSelected((s) => (s.includes(t.key) ? s.filter((k) => k !== t.key) : [...s, t.key]))
                    }
                  />
                </td>
                <td className={`${td} mono whitespace-nowrap text-[12px]`}>
                  <Link to={`/tasks/${t.key}`} className="text-sky-700 hover:underline" onClick={(e) => e.stopPropagation()}>
                    {t.key}
                  </Link>
                </td>
                <td className={`${td} max-w-md`}>
                  <div className="truncate font-medium text-slate-800">{t.title}</div>
                  {t.claimed_by && <div className="text-[11px] text-slate-500">claimed by {t.claimed_by}</div>}
                </td>
                <td className={td}>
                  <StatusBadge status={t.status} />
                </td>
                <td className={td}>
                  <PriorityBadge priority={t.priority} />
                </td>
                <td className={`${td} text-slate-600`}>{label(t.category)}</td>
                <td className={`${td} text-[12px] text-slate-600`}>
                  {t.repository_ids.map((id) => repoNames.get(id) ?? id.slice(0, 6)).join(", ")}
                </td>
                <td className={td}>
                  <div className="flex flex-wrap gap-1">
                    {t.tags.map((tag) => (
                      <Tag key={tag}>{tag}</Tag>
                    ))}
                  </div>
                </td>
                <td className={`${td} whitespace-nowrap text-slate-500`}>{ago(t.updated_at)}</td>
              </tr>
            ))}
          </Table>
        )}
      </div>
      {page && page.total > 0 && (
        <div className="flex items-center justify-between text-sm text-slate-500">
          <span>
            {page.offset + 1}–{page.offset + page.items.length} of {page.total}
          </span>
          <div className="flex gap-2">
            <Button size="sm" disabled={offset === 0} onClick={() => setFilter("offset", String(Math.max(0, offset - PAGE_SIZE)))}>
              previous
            </Button>
            <Button size="sm" disabled={page.next_offset === null} onClick={() => setFilter("offset", String(page.next_offset))}>
              next
            </Button>
          </div>
        </div>
      )}
      <Modal title="New task" open={creating} onClose={() => setCreating(false)} wide>
        <TaskForm
          onDone={(t) => {
            setCreating(false);
            navigate(`/tasks/${t.key}`);
          }}
        />
      </Modal>
    </div>
  );
}

function Select({
  label: name,
  value,
  options,
  onChange,
}: {
  label: string;
  value: string;
  options: readonly string[];
  onChange: (v: string) => void;
}) {
  return (
    <select aria-label={name} className={`${inputBase} w-36`} value={value} onChange={(e) => onChange(e.target.value)}>
      <option value="">{name}: any</option>
      {options.map((o) => (
        <option key={o} value={o}>
          {label(o)}
        </option>
      ))}
    </select>
  );
}
