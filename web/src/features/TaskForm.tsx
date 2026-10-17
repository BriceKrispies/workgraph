import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { get, patch, post } from "../api/client";
import { PRIORITIES, type Project, type Repository, type Task } from "../api/types";
import { Button, ErrorBanner, Field, inputClass } from "../components/ui";
import { useWorkspace } from "../hooks/workspace";

const splitList = (value: string, sep: RegExp) =>
  value
    .split(sep)
    .map((s) => s.trim())
    .filter(Boolean);

export function TaskForm({ task, onDone }: { task?: Task; onDone: (task: Task) => void }) {
  const { ws } = useWorkspace();
  const client = useQueryClient();
  const projects = useQuery({ queryKey: ["projects", ws], queryFn: () => get<Project[]>(`/ws/${ws}/projects`) });
  const repos = useQuery({ queryKey: ["repositories", ws], queryFn: () => get<Repository[]>(`/ws/${ws}/repositories`) });
  const [title, setTitle] = useState(task?.title ?? "");
  const [description, setDescription] = useState(task?.description ?? "");
  const [criteria, setCriteria] = useState((task?.acceptance_criteria ?? []).join("\n"));
  const [priority, setPriority] = useState<string>(task?.priority ?? "medium");
  const [tags, setTags] = useState((task?.tags ?? []).join(", "));
  const [selectedProjects, setSelectedProjects] = useState<string[]>([]);
  const [selectedRepos, setSelectedRepos] = useState<string[]>([]);
  const [parent, setParent] = useState("");
  const [dependsOn, setDependsOn] = useState("");
  const [ready, setReady] = useState(true);

  const mutation = useMutation({
    mutationFn: () => {
      const common = {
        title,
        description,
        acceptance_criteria: splitList(criteria, /\n/),
        priority,
        tags: splitList(tags, /,/),
      };
      if (task) return patch<Task>(`/ws/${ws}/tasks/${task.key}`, { ...common, expected_version: task.version });
      return post<Task>(`/ws/${ws}/tasks`, {
        ...common,
        status: ready ? "ready" : "backlog",
        projects: selectedProjects,
        repositories: selectedRepos,
        parent: parent || null,
        depends_on: splitList(dependsOn, /[,\s]+/),
      });
    },
    onSuccess: (saved) => {
      void client.invalidateQueries({ queryKey: ["tasks"] });
      void client.invalidateQueries({ queryKey: ["task"] });
      void client.invalidateQueries({ queryKey: ["summary"] });
      onDone(saved);
    },
  });

  const toggle = (list: string[], value: string, set: (v: string[]) => void) =>
    set(list.includes(value) ? list.filter((v) => v !== value) : [...list, value]);

  return (
    <form
      className="space-y-3"
      onSubmit={(e) => {
        e.preventDefault();
        mutation.mutate();
      }}
    >
      <ErrorBanner error={mutation.error} />
      <Field label="Title">
        <input className={inputClass} value={title} onChange={(e) => setTitle(e.target.value)} required maxLength={500} />
      </Field>
      <Field label="Description (Markdown)">
        <textarea className={`${inputClass} mono h-32`} value={description} onChange={(e) => setDescription(e.target.value)} />
      </Field>
      <Field label="Acceptance criteria" hint="One per line">
        <textarea className={`${inputClass} h-16`} value={criteria} onChange={(e) => setCriteria(e.target.value)} />
      </Field>
      <div className="grid grid-cols-2 gap-3">
        <Field label="Priority">
          <select className={inputClass} value={priority} onChange={(e) => setPriority(e.target.value)}>
            {PRIORITIES.map((p) => (
              <option key={p}>{p}</option>
            ))}
          </select>
        </Field>
        <Field label="Tags" hint="Comma separated">
          <input className={inputClass} value={tags} onChange={(e) => setTags(e.target.value)} />
        </Field>
      </div>
      {!task && (
        <>
          <div className="grid grid-cols-2 gap-3">
            <fieldset>
              <legend className="mb-0.5 text-[12px] font-medium text-slate-600">Projects</legend>
              <div className="max-h-28 space-y-0.5 overflow-y-auto">
                {projects.data?.map((p) => (
                  <label key={p.id} className="flex items-center gap-1.5 text-sm">
                    <input
                      type="checkbox"
                      checked={selectedProjects.includes(p.slug)}
                      onChange={() => toggle(selectedProjects, p.slug, setSelectedProjects)}
                    />
                    {p.name}
                  </label>
                ))}
              </div>
            </fieldset>
            <fieldset>
              <legend className="mb-0.5 text-[12px] font-medium text-slate-600">Repositories</legend>
              <div className="max-h-28 space-y-0.5 overflow-y-auto">
                {repos.data?.map((r) => (
                  <label key={r.id} className="flex items-center gap-1.5 text-sm">
                    <input
                      type="checkbox"
                      checked={selectedRepos.includes(r.name)}
                      onChange={() => toggle(selectedRepos, r.name, setSelectedRepos)}
                    />
                    {r.name}
                  </label>
                ))}
              </div>
            </fieldset>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <Field label="Parent task" hint="Key, e.g. WG-1">
              <input className={inputClass} value={parent} onChange={(e) => setParent(e.target.value)} />
            </Field>
            <Field label="Depends on" hint="Keys separated by commas">
              <input className={inputClass} value={dependsOn} onChange={(e) => setDependsOn(e.target.value)} />
            </Field>
          </div>
          <label className="flex items-center gap-2 text-sm">
            <input type="checkbox" checked={ready} onChange={(e) => setReady(e.target.checked)} /> Create as ready
          </label>
        </>
      )}
      <div className="flex justify-end gap-2">
        <Button type="submit" variant="primary" busy={mutation.isPending}>
          {task ? "Save changes" : "Create task"}
        </Button>
      </div>
    </form>
  );
}
