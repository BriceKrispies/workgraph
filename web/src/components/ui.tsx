import { useEffect, useRef, type ButtonHTMLAttributes, type ReactNode } from "react";
import { errorMessage } from "../api/client";
import { label } from "../lib/format";

type Variant = "primary" | "secondary" | "danger" | "ghost";

const VARIANTS: Record<Variant, string> = {
  primary: "bg-slate-900 text-white hover:bg-slate-700 disabled:bg-slate-400",
  secondary: "bg-white text-slate-700 border border-slate-300 hover:bg-slate-100 disabled:text-slate-400",
  danger: "bg-white text-rose-700 border border-rose-300 hover:bg-rose-50 disabled:text-rose-300",
  ghost: "text-slate-600 hover:bg-slate-200/70 disabled:text-slate-300",
};

export function Button({
  variant = "secondary",
  size = "md",
  className = "",
  busy,
  children,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant; size?: "sm" | "md"; busy?: boolean }) {
  const sizing = size === "sm" ? "px-2 py-0.5 text-xs" : "px-3 py-1.5 text-sm";
  return (
    <button
      type="button"
      className={`inline-flex items-center gap-1.5 rounded-md font-medium transition-colors disabled:cursor-not-allowed ${sizing} ${VARIANTS[variant]} ${className}`}
      disabled={busy || rest.disabled}
      {...rest}
    >
      {busy && <Spinner small />}
      {children}
    </button>
  );
}

export function Spinner({ small }: { small?: boolean }) {
  return (
    <span
      role="status"
      aria-label="loading"
      className={`inline-block animate-spin rounded-full border-2 border-slate-300 border-t-slate-700 ${small ? "h-3 w-3" : "h-5 w-5"}`}
    />
  );
}

export function Loading({ label: text = "Loading…" }: { label?: string }) {
  return (
    <div className="flex items-center gap-2 p-6 text-slate-500">
      <Spinner /> {text}
    </div>
  );
}

export function Skeleton({ rows = 4 }: { rows?: number }) {
  return (
    <div className="space-y-2 p-2" aria-busy="true">
      {Array.from({ length: rows }, (_, i) => (
        <div key={i} className="h-4 animate-pulse rounded bg-slate-200" style={{ width: `${90 - (i % 3) * 15}%` }} />
      ))}
    </div>
  );
}

export function ErrorBanner({ error, onRetry }: { error: unknown; onRetry?: () => void }) {
  if (!error) return null;
  return (
    <div role="alert" className="flex items-start justify-between gap-3 rounded-md border border-rose-200 bg-rose-50 px-3 py-2 text-sm text-rose-800">
      <span>{errorMessage(error)}</span>
      {onRetry && (
        <button className="text-rose-700 underline" onClick={onRetry}>
          retry
        </button>
      )}
    </div>
  );
}

export function Empty({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <div className="rounded-md border border-dashed border-slate-300 p-6 text-center text-slate-500">
      <div className="font-medium text-slate-700">{title}</div>
      {children && <div className="mt-1 text-sm">{children}</div>}
    </div>
  );
}

export function Card({
  title,
  actions,
  children,
  className = "",
  bodyClass = "p-3",
}: {
  title?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
  bodyClass?: string;
}) {
  return (
    <section className={`rounded-lg border border-slate-200 bg-white shadow-sm ${className}`}>
      {(title || actions) && (
        <header className="flex items-center justify-between gap-2 border-b border-slate-100 px-3 py-2">
          <h2 className="text-[13px] font-semibold uppercase tracking-wide text-slate-500">{title}</h2>
          <div className="flex items-center gap-1.5">{actions}</div>
        </header>
      )}
      <div className={bodyClass}>{children}</div>
    </section>
  );
}

export function Stat({ label: name, value, hint }: { label: string; value: ReactNode; hint?: string }) {
  return (
    <div className="rounded-lg border border-slate-200 bg-white px-3 py-2 shadow-sm">
      <div className="text-[11px] uppercase tracking-wide text-slate-500">{name}</div>
      <div className="mt-0.5 text-xl font-semibold text-slate-900" data-testid={`stat-${name}`}>
        {value}
      </div>
      {hint && <div className="text-[11px] text-slate-400">{hint}</div>}
    </div>
  );
}

const STATUS_COLORS: Record<string, string> = {
  backlog: "bg-slate-100 text-slate-600",
  ready: "bg-sky-100 text-sky-800",
  blocked: "bg-amber-100 text-amber-800",
  claimed: "bg-indigo-100 text-indigo-800",
  running: "bg-violet-100 text-violet-800",
  awaiting_verification: "bg-cyan-100 text-cyan-800",
  awaiting_review: "bg-teal-100 text-teal-800",
  accepted: "bg-emerald-100 text-emerald-800",
  failed: "bg-rose-100 text-rose-800",
  cancelled: "bg-slate-200 text-slate-500 line-through",
  succeeded: "bg-emerald-100 text-emerald-800",
  passed: "bg-emerald-100 text-emerald-800",
  queued: "bg-sky-100 text-sky-800",
  preparing: "bg-indigo-100 text-indigo-800",
  verifying: "bg-cyan-100 text-cyan-800",
  timed_out: "bg-orange-100 text-orange-800",
  dead: "bg-rose-200 text-rose-900",
  error: "bg-rose-100 text-rose-800",
  ok: "bg-emerald-100 text-emerald-800",
  degraded: "bg-amber-100 text-amber-800",
  unavailable: "bg-rose-100 text-rose-800",
  not_matched: "bg-slate-100 text-slate-600",
  skipped: "bg-amber-100 text-amber-800",
  previewed: "bg-sky-100 text-sky-800",
};

export function StatusBadge({ status }: { status: string | null | undefined }) {
  const s = status ?? "unknown";
  return (
    <span className={`inline-block whitespace-nowrap rounded px-1.5 py-0.5 text-[11px] font-medium ${STATUS_COLORS[s] ?? "bg-slate-100 text-slate-600"}`}>
      {label(s)}
    </span>
  );
}

const PRIORITY_COLORS: Record<string, string> = {
  critical: "text-rose-700",
  high: "text-orange-600",
  medium: "text-slate-600",
  low: "text-slate-400",
};

export function PriorityBadge({ priority }: { priority: string }) {
  return <span className={`text-[11px] font-semibold uppercase ${PRIORITY_COLORS[priority] ?? ""}`}>{priority}</span>;
}

export function Tag({ children }: { children: ReactNode }) {
  return <span className="inline-block rounded bg-slate-100 px-1.5 py-0.5 text-[11px] text-slate-600">{children}</span>;
}

export function Modal({
  title,
  open,
  onClose,
  children,
  wide,
}: {
  title: string;
  open: boolean;
  onClose: () => void;
  children: ReactNode;
  wide?: boolean;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    ref.current?.querySelector<HTMLElement>("input,textarea,select,button")?.focus();
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);
  if (!open) return null;
  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-slate-900/40 p-4 pt-16" onMouseDown={onClose}>
      <div
        ref={ref}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        className={`w-full ${wide ? "max-w-3xl" : "max-w-lg"} rounded-lg bg-white shadow-xl`}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <header className="flex items-center justify-between border-b border-slate-100 px-4 py-2.5">
          <h2 className="font-semibold">{title}</h2>
          <button aria-label="close" className="text-slate-400 hover:text-slate-700" onClick={onClose}>
            ✕
          </button>
        </header>
        <div className="p-4">{children}</div>
      </div>
    </div>
  );
}

export function Field({ label: name, children, hint }: { label: string; children: ReactNode; hint?: string }) {
  return (
    <label className="block text-sm">
      <span className="mb-0.5 block text-[12px] font-medium text-slate-600">{name}</span>
      {children}
      {hint && <span className="mt-0.5 block text-[11px] text-slate-400">{hint}</span>}
    </label>
  );
}

export const inputBase =
  "rounded-md border border-slate-300 bg-white px-2 py-1.5 text-sm focus:border-sky-500 focus:outline-none";
export const inputClass = `w-full ${inputBase}`;

export function Table({ head, children, testId }: { head: ReactNode[]; children: ReactNode; testId?: string }) {
  return (
    <div className="overflow-x-auto">
      <table className="w-full border-collapse text-left text-[13px]" data-testid={testId}>
        <thead>
          <tr className="border-b border-slate-200 text-[11px] uppercase tracking-wide text-slate-500">
            {head.map((h, i) => (
              <th key={i} className="whitespace-nowrap px-2 py-1.5 font-medium">
                {h}
              </th>
            ))}
          </tr>
        </thead>
        <tbody className="divide-y divide-slate-100">{children}</tbody>
      </table>
    </div>
  );
}

export const td = "px-2 py-1.5 align-top";

export function KeyValue({ items }: { items: [string, ReactNode][] }) {
  return (
    <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-[13px]">
      {items
        .filter(([, v]) => v !== null && v !== undefined && v !== "")
        .map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="text-slate-500">{k}</dt>
            <dd className="min-w-0 break-words text-slate-800">{v}</dd>
          </div>
        ))}
    </dl>
  );
}

export function Tabs<T extends string>({ tabs, value, onChange }: { tabs: [T, string][]; value: T; onChange: (t: T) => void }) {
  return (
    <div role="tablist" className="flex flex-wrap gap-1 border-b border-slate-200">
      {tabs.map(([id, text]) => (
        <button
          key={id}
          role="tab"
          aria-selected={value === id}
          onClick={() => onChange(id)}
          className={`-mb-px border-b-2 px-3 py-1.5 text-sm ${value === id ? "border-slate-900 font-medium text-slate-900" : "border-transparent text-slate-500 hover:text-slate-800"}`}
        >
          {text}
        </button>
      ))}
    </div>
  );
}
