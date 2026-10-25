import { useId, useState } from "react";
import { Link } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { get, post } from "../api/client";
import type { DecisionList, DecisionListItem, DecisionOut } from "../api/types";
import { Icon } from "../components/icons";
import { Markdown } from "../components/Markdown";
import { Button, Empty, ErrorBanner, Loading, PageHeader, PriorityBadge, SectionTitle, inputClass, textareaClass } from "../components/ui";
import { useWorkspace } from "../hooks/workspace";
import { ago, dateTime } from "../lib/format";

/** Sentinel choice for the "Other" answer; real option ids are never this string. */
const OTHER = "__other__";

/** The choice a card starts with: the recorded one when changing, otherwise nothing selected. */
function initialChoice(item: DecisionListItem): string | null {
  if (item.status === "decided") {
    if (item.chosen_option_id && item.options.some((o) => o.id === item.chosen_option_id)) return item.chosen_option_id;
    if (item.other_text) return OTHER;
  }
  return item.options.length === 0 ? OTHER : null;
}

function chosenText(item: DecisionListItem): string {
  const option = item.options.find((o) => o.id === item.chosen_option_id);
  if (option) return option.label;
  if (item.other_text) return `Other: ${item.other_text}`;
  return "—";
}

export function DecisionsPage() {
  const { ws } = useWorkspace();
  const client = useQueryClient();
  const [changing, setChanging] = useState<string | null>(null);

  // Keyed by workspace and held until the workspace resolves, so the cards do not flash in and out on first load.
  const open = useQuery({
    queryKey: ["decisions", ws, "open"],
    queryFn: () => get<DecisionList>("/decisions", { status: "open", limit: 100 }),
    enabled: !!ws,
  });
  const decided = useQuery({
    queryKey: ["decisions", ws, "decided"],
    queryFn: () => get<DecisionList>("/decisions", { status: "decided", limit: 20 }),
    enabled: !!ws,
  });

  const refresh = () => {
    setChanging(null);
    void client.invalidateQueries({ queryKey: ["decisions"] });
    void client.invalidateQueries({ queryKey: ["task"] });
    void client.invalidateQueries({ queryKey: ["tasks"] });
  };

  const openItems = open.data?.items ?? [];
  const decidedItems = decided.data?.items ?? [];

  return (
    <div>
      <PageHeader title="Decisions" />

      <section aria-labelledby="open-decisions">
        <SectionTitle>
          <span id="open-decisions">Open</span>
        </SectionTitle>
        <ErrorBanner error={open.error} onRetry={() => open.refetch()} />
        {!open.data && !open.error ? (
          <Loading />
        ) : openItems.length === 0 ? (
          <div className="rounded-2xl border border-line bg-surface">
            <Empty title="No open decisions">Tasks that need a choice from you will appear here.</Empty>
          </div>
        ) : (
          <div className="space-y-3">
            {openItems.map((item) => (
              <DecisionCard key={item.id} item={item} onDone={refresh} />
            ))}
          </div>
        )}
      </section>

      <section aria-labelledby="decided-decisions" className="mt-8">
        <SectionTitle>
          <span id="decided-decisions">Recently decided</span>
        </SectionTitle>
        <ErrorBanner error={decided.error} onRetry={() => decided.refetch()} />
        {decidedItems.length > 0 && (
          <div className="divide-y divide-line overflow-hidden rounded-2xl border border-line bg-surface">
            {decidedItems.map((item) =>
              changing === item.id ? (
                <div key={item.id} className="p-4">
                  <DecisionCard item={item} mode="change" onDone={refresh} onCancel={() => setChanging(null)} />
                </div>
              ) : (
                <DecidedRow key={item.id} item={item} onChange={() => setChanging(item.id)} />
              ),
            )}
          </div>
        )}
      </section>
    </div>
  );
}

function DecidedRow({ item, onChange }: { item: DecisionListItem; onChange: () => void }) {
  return (
    <div className="flex items-start justify-between gap-3 p-4">
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-x-2 text-[13px] text-ink-3">
          <Link to={`/tasks/${item.task.key}`} className="mono font-medium text-accent hover:underline">
            {item.task.key}
          </Link>
          <time dateTime={item.decided_at ?? undefined} title={dateTime(item.decided_at)}>
            {ago(item.decided_at)}
          </time>
        </div>
        <div className="mt-1 line-clamp-2 text-[15px] font-medium leading-snug text-ink">{item.question}</div>
        <div className="mt-1 text-[14px] text-ink-2">
          <span className="text-ink-3">Chose </span>
          {chosenText(item)}
        </div>
        {item.note && <div className="mt-1 text-[13px] text-ink-3">{item.note}</div>}
      </div>
      <Button size="sm" variant="ghost" onClick={onChange}>
        Change
      </Button>
    </div>
  );
}

export function DecisionCard({
  item,
  mode = "open",
  onDone,
  onCancel,
}: {
  item: DecisionListItem;
  mode?: "open" | "change";
  onDone: () => void;
  onCancel?: () => void;
}) {
  const id = useId();
  const [choice, setChoice] = useState<string | null>(() => initialChoice(item));
  const [otherText, setOtherText] = useState(item.other_text ?? "");
  const [note, setNote] = useState(item.note ?? "");
  const [detailsOpen, setDetailsOpen] = useState(false);
  const hasOptions = item.options.length > 0;

  const choose = useMutation({
    mutationFn: () => {
      const body: { option_id?: string; other_text?: string; note?: string } =
        choice === OTHER ? { other_text: otherText.trim() } : { option_id: choice ?? undefined };
      if (note.trim()) body.note = note.trim();
      return post<DecisionOut>(`/decisions/${item.id}/choose`, body);
    },
    onSuccess: () => onDone(),
  });

  const canDecide = choice !== null && (choice !== OTHER || otherText.trim() !== "") && !choose.isPending;

  return (
    <article
      aria-label={`Decision on ${item.task.key}`}
      className={`min-w-0 ${mode === "open" ? "rounded-2xl border border-line bg-surface p-4" : ""}`}
    >
      <header className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <Link to={`/tasks/${item.task.key}`} className="mono text-[13px] font-medium text-accent hover:underline">
          {item.task.key}
        </Link>
        <PriorityBadge priority={item.task.priority} compact />
      </header>
      <div className="mt-1 text-[16px] font-semibold leading-snug text-ink">{item.task.title}</div>

      <div className="mt-3 text-[15px] text-ink">
        <Markdown source={item.question} />
      </div>

      <button
        type="button"
        aria-expanded={detailsOpen}
        onClick={() => setDetailsOpen((v) => !v)}
        className="mt-2 inline-flex min-h-9 items-center gap-1 rounded-lg pr-2 text-[14px] text-ink-3 hover:text-ink"
      >
        <Icon name="chevron" className={`h-4 w-4 transition-transform ${detailsOpen ? "rotate-90" : ""}`} />
        Task details
      </button>
      {detailsOpen && (
        <div className="mt-1 rounded-xl bg-sunken p-3 text-[14px]">
          <Markdown source={item.task.description} />
        </div>
      )}

      <fieldset className="mt-4 space-y-2">
        <legend className="mb-2 text-[13px] font-medium text-ink-3">Choose an answer</legend>
        {item.options.map((opt) => {
          const selected = choice === opt.id;
          const inputId = `${id}-${opt.id}`;
          return (
            <div
              key={opt.id}
              onClick={() => setChoice(opt.id)}
              className={`rounded-xl border p-3 transition-colors ${selected ? "border-accent bg-accent-soft" : "border-line-strong bg-surface hover:bg-sunken"}`}
            >
              <div className="flex items-start gap-3">
                <input
                  id={inputId}
                  type="radio"
                  name={`${id}-choice`}
                  value={opt.id}
                  checked={selected}
                  onChange={() => setChoice(opt.id)}
                  className="mt-1 h-5 w-5 shrink-0 accent-[var(--wg-accent)] lg:h-4 lg:w-4"
                />
                <div className="min-w-0 flex-1">
                  <label htmlFor={inputId} className="flex flex-wrap items-center gap-2 text-[15px] font-medium text-ink">
                    {opt.label}
                    {opt.recommended && (
                      <span className="rounded-full bg-accent-soft px-2 py-0.5 text-[12px] font-medium text-accent ring-1 ring-accent/40">Recommended</span>
                    )}
                  </label>
                  {opt.detail && (
                    <div className="mt-1 text-[14px] text-ink-2">
                      <Markdown source={opt.detail} />
                    </div>
                  )}
                </div>
              </div>
            </div>
          );
        })}

        {hasOptions && (
          <div
            onClick={() => setChoice(OTHER)}
            className={`rounded-xl border p-3 transition-colors ${choice === OTHER ? "border-accent bg-accent-soft" : "border-line-strong bg-surface hover:bg-sunken"}`}
          >
            <div className="flex items-center gap-3">
              <input
                id={`${id}-other`}
                type="radio"
                name={`${id}-choice`}
                value={OTHER}
                checked={choice === OTHER}
                onChange={() => setChoice(OTHER)}
                className="h-5 w-5 shrink-0 accent-[var(--wg-accent)] lg:h-4 lg:w-4"
              />
              <label htmlFor={`${id}-other`} className="text-[15px] font-medium text-ink">
                Other
              </label>
            </div>
          </div>
        )}
      </fieldset>

      {(choice === OTHER || !hasOptions) && (
        <textarea
          aria-label="Other answer"
          value={otherText}
          onChange={(e) => {
            setOtherText(e.target.value);
            setChoice(OTHER);
          }}
          rows={3}
          placeholder="Write the answer you want"
          className={`${textareaClass} mt-3`}
        />
      )}

      <label className="mt-3 block text-[13px] font-medium text-ink-3">
        Note (optional)
        <input
          value={note}
          onChange={(e) => setNote(e.target.value)}
          placeholder="Why, or what to watch for"
          className={`${inputClass} mt-1 font-normal text-ink`}
        />
      </label>

      {choose.error && (
        <div className="mt-3">
          <ErrorBanner error={choose.error} />
        </div>
      )}
      <div className="mt-4 flex items-center justify-end gap-2">
        {onCancel && (
          <Button variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
        )}
        <Button variant="primary" disabled={!canDecide} busy={choose.isPending} onClick={() => choose.mutate()}>
          Decide
        </Button>
      </div>
    </article>
  );
}
