import { useMemo, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
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
import { Icon } from "../components/icons";
import {
  Button,
  Chip,
  Empty,
  ErrorBanner,
  List,
  ListRow,
  Modal,
  PageHeader,
  PriorityBadge,
  Skeleton,
  StatusBadge,
  Tag,
  inputBase,
  selectClass,
} from "../components/ui";
import { TaskForm } from "../features/TaskForm";
import { useWorkspace } from "../hooks/workspace";
import { ago, label, sentence } from "../lib/format";
import { toneOf } from "../lib/tone";

const PAGE_SIZE = 50;
const FILTER_KEYS = ["status", "priority", "project", "repository", "tag", "category", "q"] as const;
// the statuses worth one tap; the rest are in the Status select
const QUICK_STATUSES = ["ready", "awaiting_review", "running", "claimed", "blocked", "failed", "backlog", "accepted"];

export function TasksPage() {
  const { ws } = useWorkspace();
  const [params, setParams] = useSearchParams();
  const navigate = useNavigate();
  const client = useQueryClient();
  const [creating, setCreating] = useState(false);
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [selecting, setSelecting] = useState(false);
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
      setBulkTag("");
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
  const panelFilters = (["priority", "category", "project", "repository", "tag"] as const).filter((k) => filters[k]).length + (sort !== "created_at" ? 1 : 0);
  const anyFilter = FILTER_KEYS.some((k) => filters[k]);
  const toggleOne = (key: string) => setSelected((s) => (s.includes(key) ? s.filter((k) => k !== key) : [...s, key]));

  return (
    <div>
      <PageHeader
        title="Tasks"
        actions={
          <Button variant="primary" icon="plus" onClick={() => setCreating(true)}>
            <span className="sm:hidden">New</span>
            <span className="hidden sm:inline">New task</span>
          </Button>
        }
      />

      <div className="space-y-3" role="search">
        <div className="flex gap-2">
          <div className="relative min-w-0 flex-1">
            <Icon name="search" className="pointer-events-none absolute left-3 top-1/2 h-[18px] w-[18px] -translate-y-1/2 text-ink-3" />
            <input
              aria-label="Filter text"
              type="search"
              enterKeyHint="search"
              placeholder="Search titles and descriptions"
              className={`${inputBase} w-full pl-10`}
              defaultValue={filters.q}
              key={filters.q}
              onKeyDown={(e) => e.key === "Enter" && setFilter("q", (e.target as HTMLInputElement).value)}
              onBlur={(e) => e.target.value !== filters.q && setFilter("q", e.target.value)}
            />
          </div>
          <Button
            icon="filter"
            aria-expanded={filtersOpen}
            onClick={() => setFiltersOpen((v) => !v)}
            className={`md:hidden ${panelFilters ? "border-accent text-accent" : ""}`}
          >
            {panelFilters ? `Filters ${panelFilters}` : "Filters"}
          </Button>
        </div>

        <div className="scroll-x -mx-4 flex gap-2 px-4" aria-label="Quick filters" role="group">
          <Chip active={!filters.status} onClick={() => setFilter("status", "")}>
            All
          </Chip>
          {QUICK_STATUSES.map((st) => (
            <Chip key={st} active={filters.status === st} tone={toneOf(st)} onClick={() => setFilter("status", filters.status === st ? "" : st)}>
              {sentence(st)}
            </Chip>
          ))}
        </div>

        <div className={`${filtersOpen ? "grid" : "hidden"} grid-cols-2 gap-2 rounded-2xl border border-line bg-surface p-3 md:grid md:grid-cols-4 md:border-0 md:bg-transparent md:p-0 xl:grid-cols-7`}>
          <Select label="Status" value={filters.status} options={TASK_STATUSES} onChange={(v) => setFilter("status", v)} />
          <Select label="Priority" value={filters.priority} options={[...PRIORITIES]} onChange={(v) => setFilter("priority", v)} />
          <Select label="Category" value={filters.category} options={[...CATEGORIES]} onChange={(v) => setFilter("category", v)} />
          <Select label="Project" value={filters.project} options={(projects.data ?? []).map((p) => p.slug)} onChange={(v) => setFilter("project", v)} />
          <Select label="Repository" value={filters.repository} options={(repos.data ?? []).map((r) => r.name)} onChange={(v) => setFilter("repository", v)} />
          <input
            aria-label="Tag"
            placeholder="Tag"
            className={`${inputBase} w-full min-w-0`}
            defaultValue={filters.tag}
            key={`tag-${filters.tag}`}
            enterKeyHint="done"
            onKeyDown={(e) => e.key === "Enter" && setFilter("tag", (e.target as HTMLInputElement).value)}
            onBlur={(e) => e.target.value !== filters.tag && setFilter("tag", e.target.value)}
          />
          <select aria-label="Sort" className={`${selectClass} w-full min-w-0`} value={sort} onChange={(e) => setFilter("sort", e.target.value)}>
            {["created_at", "updated_at", "priority", "key", "status", "title"].map((s) => (
              <option key={s} value={s}>
                Sort: {label(s)}
              </option>
            ))}
          </select>
        </div>
      </div>

      <div className="mb-2 mt-4 flex min-h-9 items-center justify-between gap-2 px-1 text-[13px] text-ink-3">
        <span className="tnum">{page ? `${page.total} ${page.total === 1 ? "task" : "tasks"}` : " "}</span>
        <div className="flex items-center gap-1">
          {anyFilter && (
            <Button variant="ghost" size="sm" onClick={() => setParams(new URLSearchParams())}>
              Clear filters
            </Button>
          )}
          {page && page.items.length > 0 && (
            <Button
              variant="ghost"
              size="sm"
              className="lg:hidden"
              onClick={() => {
                setSelecting((v) => !v);
                setSelected([]);
              }}
            >
              {selecting ? "Done" : "Select"}
            </Button>
          )}
          {page && page.items.length > 0 && (
            <label className="hidden min-h-7 items-center gap-2 px-2 text-[13px] lg:flex">
              <input
                type="checkbox"
                aria-label="select all"
                className="h-4 w-4 accent-[var(--wg-accent)]"
                checked={allSelected}
                onChange={() => setSelected(allSelected ? [] : page.items.map((t) => t.key))}
              />
              Select page
            </label>
          )}
        </div>
      </div>

      <ErrorBanner error={tasks.error ?? bulk.error} onRetry={() => tasks.refetch()} />
      {!page ? (
        <div className="rounded-2xl border border-line bg-surface">
          <Skeleton rows={8} />
        </div>
      ) : page.items.length === 0 ? (
        <div className="rounded-2xl border border-line bg-surface">
          <Empty
            title="No tasks match"
            action={
              anyFilter ? (
                <Button onClick={() => setParams(new URLSearchParams())}>Clear filters</Button>
              ) : (
                <Button variant="primary" icon="plus" onClick={() => setCreating(true)}>
                  New task
                </Button>
              )
            }
          >
            {anyFilter ? "Nothing matches these filters." : "Create a task to start tracking work."}
          </Empty>
        </div>
      ) : (
        <List testId="tasks-table">
          {page.items.map((t) => (
            <ListRow
              key={t.id}
              testId="task-row"
              tone={toneOf(t.status)}
              selected={selected.includes(t.key)}
              to={selecting ? undefined : `/tasks/${t.key}`}
              onClick={selecting ? () => toggleOne(t.key) : undefined}
              leading={
                <label className={`${selecting ? "flex" : "hidden"} h-11 w-10 items-center justify-center lg:flex lg:h-8 lg:w-8`}>
                  <input
                    type="checkbox"
                    aria-label={`select ${t.key}`}
                    className="h-5 w-5 accent-[var(--wg-accent)] lg:h-4 lg:w-4"
                    checked={selected.includes(t.key)}
                    onChange={() => toggleOne(t.key)}
                  />
                </label>
              }
            >
              <div className="task-row">
                <span className="a-key mono text-[13px] text-ink-3">{t.key}</span>
                <span className="a-status">
                  <StatusBadge status={t.status} />
                </span>
                <span className="a-prio">
                  <PriorityBadge priority={t.priority} compact />
                </span>
                <span className="a-time whitespace-nowrap text-[13px] text-ink-3">{ago(t.updated_at)}</span>
                <div className="a-title line-clamp-2 font-medium leading-snug text-ink">{t.title}</div>
                <TaskMeta task={t} repoNames={repoNames} />
              </div>
            </ListRow>
          ))}
        </List>
      )}

      {page && page.total > PAGE_SIZE && (
        <div className="mt-3 flex items-center justify-between gap-2 text-[14px] text-ink-3">
          <Button disabled={offset === 0} onClick={() => setFilter("offset", String(Math.max(0, offset - PAGE_SIZE)))}>
            Previous
          </Button>
          <span className="tnum">
            {page.offset + 1}–{page.offset + page.items.length} of {page.total}
          </span>
          <Button disabled={page.next_offset === null} onClick={() => setFilter("offset", String(page.next_offset))}>
            Next
          </Button>
        </div>
      )}

      {selected.length > 0 && (
        <>
          <div className="h-36 lg:hidden" aria-hidden="true" />
          <div
            className="fixed inset-x-0 bottom-[var(--tabbar-h)] z-30 border-t border-line bg-surface/95 px-4 py-3 shadow-[0_-8px_24px_rgba(0,0,0,0.06)] backdrop-blur-md lg:sticky lg:bottom-4 lg:mt-3 lg:rounded-2xl lg:border"
            data-testid="bulk-bar"
          >
            <div className="flex flex-wrap items-center gap-2">
              <span className="mr-auto text-[14px] font-semibold tnum">{selected.length} selected</span>
              <Button variant="ghost" size="sm" onClick={() => setSelected([])}>
                Clear
              </Button>
              <div className="flex w-full gap-2 lg:w-auto">
                <select
                  aria-label="Bulk status"
                  className={`${selectClass} min-w-0 flex-1`}
                  defaultValue=""
                  onChange={(e) => e.target.value && bulk.mutate({ status: e.target.value })}
                >
                  <option value="">Move to…</option>
                  {["backlog", "ready", "blocked", "cancelled"].map((s) => (
                    <option key={s} value={s}>
                      {label(s)}
                    </option>
                  ))}
                </select>
                <input
                  aria-label="Bulk tag"
                  className={`${inputBase} min-w-0 flex-1`}
                  placeholder="Add tag"
                  value={bulkTag}
                  onChange={(e) => setBulkTag(e.target.value)}
                />
                <Button disabled={!bulkTag} busy={bulk.isPending} onClick={() => bulk.mutate({ add_tags: [bulkTag] })}>
                  Add
                </Button>
              </div>
            </div>
            {bulk.data && Object.keys(bulk.data.failed).length > 0 && (
              <div className="mt-2 text-[13px] text-danger">
                Not updated: {Object.entries(bulk.data.failed).map(([k, v]) => `${k} (${v})`).join(", ")}
              </div>
            )}
          </div>
        </>
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

function TaskMeta({ task: t, repoNames }: { task: Task; repoNames: Map<string, string> }) {
  const repos = t.repository_ids.map((id) => repoNames.get(id) ?? id.slice(0, 6));
  const tags = t.tags.slice(0, 3);
  if (!t.category && !repos.length && !tags.length && !t.claimed_by) return null;
  return (
    <div className="a-meta mt-1.5 flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-[13px] text-ink-3">
      {t.category && <span>{sentence(t.category)}</span>}
      {repos.length > 0 && <span className="truncate">{repos.join(", ")}</span>}
      {t.claimed_by && <span className="truncate">Claimed by {t.claimed_by}</span>}
      {tags.length > 0 && (
        <span className="flex min-w-0 flex-wrap gap-1">
          {tags.map((tag) => (
            <Tag key={tag}>{tag}</Tag>
          ))}
          {t.tags.length > 3 && <span className="text-[12px]">+{t.tags.length - 3}</span>}
        </span>
      )}
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
    <select aria-label={name} className={`${selectClass} w-full min-w-0 ${value ? "border-accent text-ink" : "text-ink-2"}`} value={value} onChange={(e) => onChange(e.target.value)}>
      <option value="">{name}: any</option>
      {options.map((o) => (
        <option key={o} value={o}>
          {sentence(o)}
        </option>
      ))}
    </select>
  );
}
