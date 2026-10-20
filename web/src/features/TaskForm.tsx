import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { get, patch, post } from "../api/client";
import { PRIORITIES, type Project, type Repository, type Task } from "../api/types";
import { Button, Checkbox, Chip, ErrorBanner, Field, inputClass, selectClass, textareaClass } from "../components/ui";
import { sentence } from "../lib/format";
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
      className="space-y-4"
      onSubmit={(e) => {
        e.preventDefault();
        mutation.mutate();
      }}
    >
      <ErrorBanner error={mutation.error} />
      <Field label="Title">
        <input className={inputClass} value={title} onChange={(e) => setTitle(e.target.value)} required maxLength={500} />
      </Field>
      <Field label="Description" hint="Markdown">
        <textarea className={`${textareaClass} mono h-36 text-[14px]`} value={description} onChange={(e) => setDescription(e.target.value)} />
      </Field>
      <Field label="Acceptance criteria" hint="One per line">
        <textarea className={`${textareaClass} h-24`} value={criteria} onChange={(e) => setCriteria(e.target.value)} />
      </Field>
      <div className="grid gap-4 sm:grid-cols-2">
        <Field label="Priority">
          <select className={`${selectClass} w-full`} value={priority} onChange={(e) => setPriority(e.target.value)}>
            {PRIORITIES.map((p) => (
              <option key={p} value={p}>
                {sentence(p)}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Tags" hint="Comma separated">
          <input className={inputClass} value={tags} onChange={(e) => setTags(e.target.value)} autoCapitalize="none" />
        </Field>
      </div>
      {!task && (
        <>
          <Choices
            legend="Projects"
            options={(projects.data ?? []).map((p) => [p.slug, p.name])}
            selected={selectedProjects}
            onToggle={(v) => toggle(selectedProjects, v, setSelectedProjects)}
          />
          <Choices
            legend="Repositories"
            options={(repos.data ?? []).map((r) => [r.name, r.name])}
            selected={selectedRepos}
            onToggle={(v) => toggle(selectedRepos, v, setSelectedRepos)}
          />
          <div className="grid gap-4 sm:grid-cols-2">
            <Field label="Parent task" hint="Key, for example WG-1">
              <input className={`${inputClass} mono`} value={parent} onChange={(e) => setParent(e.target.value)} autoCapitalize="characters" />
            </Field>
            <Field label="Depends on" hint="Keys, separated by commas">
              <input className={`${inputClass} mono`} value={dependsOn} onChange={(e) => setDependsOn(e.target.value)} autoCapitalize="characters" />
            </Field>
          </div>
          <Checkbox label="Create as ready" checked={ready} onChange={setReady} />
        </>
      )}
      <div className="sticky bottom-0 -mx-5 border-t border-line bg-surface px-5 pt-3 md:static md:mx-0 md:flex md:justify-end md:border-0 md:px-0">
        <Button type="submit" variant="primary" className="w-full md:w-auto" busy={mutation.isPending}>
          {task ? "Save changes" : "Create task"}
        </Button>
      </div>
    </form>
  );
}

function Choices({
  legend,
  options,
  selected,
  onToggle,
}: {
  legend: string;
  options: [string, string][];
  selected: string[];
  onToggle: (value: string) => void;
}) {
  if (!options.length) return null;
  return (
    <fieldset>
      <legend className="mb-1.5 text-[14px] font-medium text-ink-2 lg:text-[13px]">{legend}</legend>
      <div className="flex max-h-40 flex-wrap gap-2 overflow-y-auto">
        {options.map(([value, text]) => (
          <Chip key={value} active={selected.includes(value)} onClick={() => onToggle(value)}>
            {text}
          </Chip>
        ))}
      </div>
    </fieldset>
  );
}
