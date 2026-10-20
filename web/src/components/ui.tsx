import { useEffect, useRef, useState, type ButtonHTMLAttributes, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { errorMessage } from "../api/client";
import { sentence } from "../lib/format";
import { STRUCK, toneOf, type Tone } from "../lib/tone";
import { Icon, type IconName } from "./icons";

type Variant = "primary" | "secondary" | "danger" | "ghost";

const VARIANTS: Record<Variant, string> = {
  primary: "bg-accent text-accent-ink hover:brightness-110 active:brightness-95 disabled:opacity-45",
  secondary: "bg-surface text-ink border border-line-strong hover:bg-sunken active:bg-sunken disabled:text-ink-3",
  danger: "bg-surface text-danger border border-danger/45 hover:bg-danger-soft active:bg-danger-soft disabled:opacity-50",
  ghost: "text-ink-2 hover:bg-sunken hover:text-ink active:bg-sunken disabled:text-ink-3",
};

// Phones get 44px targets; the desktop layout tightens them.
const SIZES = {
  sm: "min-h-9 px-3 text-[14px] lg:min-h-7 lg:px-2.5 lg:text-[13px]",
  md: "min-h-11 px-4 text-[15px] lg:min-h-9 lg:px-3.5 lg:text-[14px]",
};

export function Button({
  variant = "secondary",
  size = "md",
  className = "",
  busy,
  icon,
  children,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant; size?: "sm" | "md"; busy?: boolean; icon?: IconName }) {
  return (
    <button
      type="button"
      className={`inline-flex shrink-0 select-none items-center justify-center gap-1.5 rounded-[10px] font-medium transition-[background-color,filter,color] disabled:cursor-not-allowed ${SIZES[size]} ${VARIANTS[variant]} ${className}`}
      disabled={busy || rest.disabled}
      {...rest}
    >
      {busy ? <Spinner small /> : icon ? <Icon name={icon} className={size === "sm" ? "h-4 w-4" : "h-[18px] w-[18px]"} /> : null}
      {children}
    </button>
  );
}

export function IconButton({
  icon,
  label: name,
  className = "",
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { icon: IconName; label: string }) {
  return (
    <button
      type="button"
      aria-label={name}
      title={name}
      className={`inline-flex h-11 w-11 shrink-0 items-center justify-center rounded-full text-ink-2 hover:bg-sunken hover:text-ink active:bg-sunken lg:h-9 lg:w-9 ${className}`}
      {...rest}
    >
      <Icon name={icon} />
    </button>
  );
}

export function Spinner({ small }: { small?: boolean }) {
  return (
    <span
      role="status"
      aria-label="loading"
      className={`inline-block animate-spin rounded-full border-2 border-current border-t-transparent opacity-70 ${small ? "h-3.5 w-3.5" : "h-5 w-5"}`}
    />
  );
}

export function Loading({ label: text = "Loading…" }: { label?: string }) {
  return (
    <div className="flex items-center justify-center gap-2 p-10 text-ink-3" aria-busy="true">
      <Spinner /> {text}
    </div>
  );
}

export function Skeleton({ rows = 4 }: { rows?: number }) {
  return (
    <div className="space-y-3 p-4" aria-busy="true">
      {Array.from({ length: rows }, (_, i) => (
        <div key={i} className="h-3.5 animate-pulse rounded-full bg-line" style={{ width: `${88 - (i % 3) * 17}%` }} />
      ))}
    </div>
  );
}

export function ErrorBanner({ error, onRetry }: { error: unknown; onRetry?: () => void }) {
  if (!error) return null;
  return (
    <div role="alert" className="flex items-start justify-between gap-3 rounded-xl bg-danger-soft px-4 py-3 text-[14px] text-danger">
      <span className="min-w-0 break-words">{errorMessage(error)}</span>
      {onRetry && (
        <button className="-my-1 shrink-0 rounded-md px-2 py-1 font-medium underline underline-offset-2" onClick={onRetry}>
          Retry
        </button>
      )}
    </div>
  );
}

export function Notice({ tone = "info", children, testId }: { tone?: "info" | "warn" | "ok"; children: ReactNode; testId?: string }) {
  const cls = {
    info: "bg-accent-soft text-ink",
    warn: "bg-warn-soft text-ink",
    ok: "tone-done bg-[color-mix(in_oklab,var(--t)_12%,transparent)] text-ink",
  }[tone];
  return (
    <div className={`rounded-xl px-4 py-3 text-[14px] leading-relaxed ${cls}`} data-testid={testId}>
      {children}
    </div>
  );
}

export function Empty({ title, children, action }: { title: string; children?: ReactNode; action?: ReactNode }) {
  return (
    <div className="flex flex-col items-center px-6 py-10 text-center">
      <div className="font-medium text-ink">{title}</div>
      {children && <div className="mt-1 max-w-sm text-[14px] text-ink-3">{children}</div>}
      {action && <div className="mt-4">{action}</div>}
    </div>
  );
}

export function PageHeader({
  title,
  back,
  eyebrow,
  actions,
  children,
}: {
  title: ReactNode;
  back?: { to: string; label: string };
  eyebrow?: ReactNode;
  actions?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <header className="mb-4 lg:mb-6">
      {back && (
        <Link
          to={back.to}
          className="-ml-2 mb-1 inline-flex min-h-9 items-center gap-0.5 rounded-lg pr-2 text-[15px] text-accent hover:underline lg:text-[14px]"
        >
          <Icon name="back" className="h-5 w-5" />
          {back.label}
        </Link>
      )}
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0 flex-1">
          {eyebrow && <div className="mb-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-[14px] text-ink-3">{eyebrow}</div>}
          <h1 className="text-[26px] font-semibold leading-tight tracking-[-0.015em] text-ink lg:text-[24px]">{title}</h1>
        </div>
        {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
      </div>
      {children && <div className="mt-3">{children}</div>}
    </header>
  );
}

export function Card({
  title,
  actions,
  children,
  className = "",
  bodyClass = "p-4",
  footer,
}: {
  title?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
  bodyClass?: string;
  footer?: ReactNode;
}) {
  return (
    <section className={`min-w-0 overflow-hidden rounded-2xl border border-line bg-surface ${className}`}>
      {(title || actions) && (
        <header className="flex min-h-12 items-center justify-between gap-3 border-b border-line px-4 py-2">
          <h2 className="min-w-0 text-[15px] font-semibold text-ink">{title}</h2>
          {actions && <div className="flex shrink-0 items-center gap-1.5">{actions}</div>}
        </header>
      )}
      <div className={bodyClass}>{children}</div>
      {footer && <div className="border-t border-line px-4 py-2.5 text-[13px] text-ink-3">{footer}</div>}
    </section>
  );
}

/** A short heading for a group of content that is not boxed in a card. */
export function SectionTitle({ children, actions }: { children: ReactNode; actions?: ReactNode }) {
  return (
    <div className="mb-2 mt-1 flex min-h-9 items-center justify-between gap-2 px-1">
      <h2 className="text-[17px] font-semibold text-ink lg:text-[15px]">{children}</h2>
      {actions}
    </div>
  );
}

export function Stat({ label: name, value, hint, to, tone }: { label: string; value: ReactNode; hint?: string; to?: string; tone?: Tone }) {
  const body = (
    <>
      <div className="flex items-center gap-2 text-[13px] text-ink-2">
        {tone && <span className={`dot tone-${tone}`} />}
        {name}
      </div>
      <div className="mt-1 text-[28px] font-semibold leading-none tracking-tight text-ink tnum lg:text-[26px]" data-testid={`stat-${name}`}>
        {value}
      </div>
      {hint && <div className="mt-1.5 truncate text-[12px] text-ink-3">{hint}</div>}
    </>
  );
  const cls = "block min-w-0 rounded-2xl border border-line bg-surface px-4 py-3.5";
  return to ? (
    <Link to={to} className={`${cls} hover:border-line-strong active:bg-sunken`}>
      {body}
    </Link>
  ) : (
    <div className={cls}>{body}</div>
  );
}

export function StatusBadge({ status }: { status: string | null | undefined }) {
  const s = status ?? "unknown";
  return <span className={`badge tone-${toneOf(s)} ${STRUCK.has(s) ? "is-struck" : ""}`}>{sentence(s)}</span>;
}

const PRIORITY_LEVEL: Record<string, number> = { low: 1, medium: 2, high: 3, critical: 4 };

/** Signal bars: four for critical down to one for low. */
export function PriorityBadge({ priority, compact }: { priority: string; compact?: boolean }) {
  const level = PRIORITY_LEVEL[priority] ?? 0;
  const color = level === 4 ? "text-danger" : level === 3 ? "text-warn" : "text-ink-3";
  return (
    <span className={`inline-flex items-center gap-1.5 text-[12px] font-medium ${color}`} title={`${priority} priority`}>
      <span className="inline-flex h-3 items-end gap-[2px]" aria-hidden="true">
        {[1, 2, 3, 4].map((n) => (
          <span key={n} className={`w-[3px] rounded-[1px] ${n <= level ? "bg-current" : "bg-line-strong"}`} style={{ height: `${n * 3}px` }} />
        ))}
      </span>
      {compact ? <span className="sr-only">{priority}</span> : <span className="capitalize">{priority}</span>}
    </span>
  );
}

export function Tag({ children }: { children: ReactNode }) {
  return (
    <span className="inline-block max-w-full truncate rounded-md border border-line bg-sunken px-1.5 py-px text-[12px] text-ink-2">
      {children}
    </span>
  );
}

/** Lock page scroll behind an open overlay. */
function useScrollLock(active: boolean) {
  useEffect(() => {
    if (!active) return;
    const prev = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    return () => {
      document.body.style.overflow = prev;
    };
  }, [active]);
}

/** A bottom sheet on phones and a centred dialog from the md breakpoint up. */
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
  useScrollLock(open);
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    // focus the first field on desktop; on phones that would pop the keyboard over the sheet
    if (window.matchMedia?.("(min-width: 768px)").matches ?? true) {
      ref.current?.querySelector<HTMLElement>("input,textarea,select")?.focus();
    }
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);
  if (!open) return null;
  return (
    <div
      className="anim-fade fixed inset-0 z-50 flex items-end justify-center bg-[#0b0f16]/50 md:items-center md:p-6"
      onMouseDown={onClose}
    >
      <div
        ref={ref}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        className={`anim-sheet flex max-h-[92dvh] w-full flex-col rounded-t-[22px] bg-surface shadow-2xl md:max-h-[85dvh] md:rounded-2xl ${wide ? "md:max-w-3xl" : "md:max-w-lg"}`}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="mx-auto mt-2 h-1.5 w-10 shrink-0 rounded-full bg-line-strong md:hidden" aria-hidden="true" />
        <header className="flex shrink-0 items-center justify-between gap-2 py-1 pl-5 pr-2 md:border-b md:border-line md:py-2">
          <h2 className="min-w-0 truncate text-[17px] font-semibold md:text-[16px]">{title}</h2>
          <IconButton icon="close" label="close" onClick={onClose} />
        </header>
        <div className="overflow-y-auto overscroll-contain px-5 pb-[calc(1.25rem+var(--safe-bottom))] pt-2 md:pb-5 md:pt-4">{children}</div>
      </div>
    </div>
  );
}

/** A list of actions shown as a sheet — the phone equivalent of an overflow menu. */
export function ActionSheet({
  title,
  open,
  onClose,
  actions,
}: {
  title: string;
  open: boolean;
  onClose: () => void;
  actions: { label: string; onClick: () => void; danger?: boolean; disabled?: boolean; hint?: string }[];
}) {
  return (
    <Modal title={title} open={open} onClose={onClose}>
      <div className="-mx-2 flex flex-col">
        {actions.map((a) => (
          <button
            key={a.label}
            type="button"
            disabled={a.disabled}
            onClick={() => {
              onClose();
              a.onClick();
            }}
            className={`flex min-h-12 flex-col items-start justify-center rounded-xl px-3 py-2 text-left text-[16px] hover:bg-sunken active:bg-sunken disabled:opacity-45 md:min-h-10 md:text-[14px] ${a.danger ? "text-danger" : "text-ink"}`}
          >
            {a.label}
            {a.hint && <span className="text-[13px] text-ink-3">{a.hint}</span>}
          </button>
        ))}
      </div>
    </Modal>
  );
}

/**
 * Primary actions for a detail page: pinned above the tab bar on phones, inline on desktop.
 */
export function ActionBar({ children, testId }: { children: ReactNode; testId?: string }) {
  // while a bar is pinned, the page keeps its end clear of it (see .has-action-bar in index.css)
  useEffect(() => {
    document.documentElement.classList.add("has-action-bar");
    return () => document.documentElement.classList.remove("has-action-bar");
  }, []);
  return (
    <div
      className="fixed inset-x-0 bottom-[var(--tabbar-h)] z-30 flex gap-2 border-t border-line bg-surface/95 px-4 py-3 backdrop-blur-md [&>*]:flex-auto lg:static lg:z-auto lg:flex-wrap lg:border-0 lg:bg-transparent lg:p-0 lg:backdrop-blur-none lg:[&>*]:flex-none"
      data-testid={testId}
    >
      {children}
    </div>
  );
}

export function Field({ label: name, children, hint }: { label: string; children: ReactNode; hint?: string }) {
  return (
    <label className="block">
      <span className="mb-1.5 block text-[14px] font-medium text-ink-2 lg:text-[13px]">{name}</span>
      {children}
      {hint && <span className="mt-1 block text-[13px] text-ink-3 lg:text-[12px]">{hint}</span>}
    </label>
  );
}

export const inputBase =
  "min-h-11 rounded-[10px] border border-line-strong bg-surface px-3 text-ink placeholder:text-ink-3 focus:border-accent focus:outline-none focus:ring-2 focus:ring-accent/20 disabled:bg-sunken disabled:text-ink-3 lg:min-h-9";
export const inputClass = `w-full ${inputBase}`;
export const textareaClass = `w-full ${inputBase} py-2.5 lg:py-2`;
export const selectClass = `${inputBase} appearance-none bg-[length:16px] bg-[position:right_0.65rem_center] bg-no-repeat pr-9 [background-image:url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='%237a8396' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpath d='m6 9 6 6 6-6'/%3E%3C/svg%3E")]`;

export function Checkbox({ label: name, checked, onChange, className = "" }: { label: ReactNode; checked: boolean; onChange: (v: boolean) => void; className?: string }) {
  return (
    <label className={`flex min-h-11 cursor-pointer items-start gap-3 py-2.5 text-[15px] leading-snug text-ink lg:min-h-8 lg:py-1.5 lg:text-[14px] ${className}`}>
      <input type="checkbox" className="mt-px h-5 w-5 shrink-0 accent-[var(--wg-accent)] lg:mt-0.5 lg:h-4 lg:w-4" checked={checked} onChange={(e) => onChange(e.target.checked)} />
      <span className="min-w-0">{name}</span>
    </label>
  );
}

export function Switch({ checked, onChange, label: name }: { checked: boolean; onChange: () => void; label: string }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={name}
      onClick={onChange}
      className="inline-flex h-11 w-14 shrink-0 items-center justify-center lg:h-8"
    >
      <span className={`relative h-7 w-12 rounded-full transition-colors lg:h-6 lg:w-10 ${checked ? "bg-ok" : "bg-line-strong"}`}>
        <span
          className={`absolute top-0.5 h-6 w-6 rounded-full bg-white shadow transition-[left] lg:h-5 lg:w-5 ${checked ? "left-[22px] lg:left-[18px]" : "left-0.5"}`}
        />
      </span>
    </button>
  );
}

/** Toggleable filter pill. */
export function Chip({
  active,
  onClick,
  children,
  tone,
}: {
  active: boolean;
  onClick: () => void;
  children: ReactNode;
  tone?: Tone;
}) {
  return (
    <button
      type="button"
      aria-pressed={active}
      onClick={onClick}
      className={`inline-flex min-h-9 shrink-0 items-center gap-2 rounded-full border px-3.5 text-[14px] font-medium transition-colors lg:min-h-8 lg:text-[13px] ${active ? "border-ink bg-ink text-canvas" : "border-line-strong bg-surface text-ink-2 hover:text-ink"}`}
    >
      {tone && <span className={`dot tone-${tone}`} />}
      {children}
    </button>
  );
}

export function Segmented<T extends string>({ options, value, onChange, label: name }: { options: [T, string][]; value: T; onChange: (v: T) => void; label: string }) {
  return (
    <div role="radiogroup" aria-label={name} className="inline-flex rounded-[10px] bg-sunken p-0.5 ring-1 ring-line">
      {options.map(([id, text]) => (
        <button
          key={id}
          type="button"
          role="radio"
          aria-checked={value === id}
          onClick={() => onChange(id)}
          className={`min-h-9 rounded-[8px] px-3 text-[14px] font-medium lg:min-h-7 lg:text-[13px] ${value === id ? "bg-surface text-ink shadow-sm" : "text-ink-3 hover:text-ink"}`}
        >
          {text}
        </button>
      ))}
    </div>
  );
}

/** Rows of related items. Rows reflow: stacked on phones, a single line on wide screens. */
export function List({ children, testId, className = "" }: { children: ReactNode; testId?: string; className?: string }) {
  return (
    <ul className={`min-w-0 divide-y divide-line overflow-hidden rounded-2xl border border-line bg-surface ${className}`} data-testid={testId}>
      {children}
    </ul>
  );
}

export function ListRow({
  to,
  onClick,
  tone,
  selected,
  children,
  testId,
  trailing,
  leading,
}: {
  to?: string;
  onClick?: () => void;
  tone?: Tone;
  selected?: boolean;
  children: ReactNode;
  testId?: string;
  trailing?: ReactNode;
  leading?: ReactNode;
}) {
  const interactive = !!(to || onClick);
  const inner = (
    <>
      <div className="min-w-0 flex-1">{children}</div>
      {interactive && !trailing && <Icon name="chevron" className="h-4 w-4 shrink-0 self-center text-ink-3 lg:hidden" />}
    </>
  );
  const cls = `flex min-h-14 items-start gap-3 py-3 pr-4 ${leading ? "pl-1" : "pl-4"} ${interactive ? "hover:bg-sunken active:bg-sunken" : ""}`;
  return (
    <li data-testid={testId} className={`relative flex items-stretch ${tone ? `rail tone-${tone}` : ""} ${selected ? "bg-accent-soft" : ""}`}>
      {leading && <div className="flex shrink-0 items-start pl-1.5 pt-1">{leading}</div>}
      {to ? (
        <Link to={to} className={`${cls} min-w-0 flex-1`}>
          {inner}
        </Link>
      ) : onClick ? (
        <div
          role="button"
          tabIndex={0}
          onClick={onClick}
          onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), onClick())}
          className={`${cls} min-w-0 flex-1 cursor-pointer`}
        >
          {inner}
        </div>
      ) : (
        <div className={`${cls} min-w-0 flex-1`}>{inner}</div>
      )}
      {trailing && <div className="flex shrink-0 items-center pr-2">{trailing}</div>}
    </li>
  );
}

/** Secondary line of a list row: small, muted, wrapping pieces with even gaps. */
export function Meta({ children, className = "" }: { children: ReactNode; className?: string }) {
  return <div className={`mt-1 flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-[13px] text-ink-3 ${className}`}>{children}</div>;
}

export function Table({ head, children, testId }: { head: ReactNode[]; children: ReactNode; testId?: string }) {
  return (
    <div className="overflow-x-auto overscroll-x-contain">
      <table className="w-full border-collapse text-left text-[14px] lg:text-[13px]" data-testid={testId}>
        <thead>
          <tr className="border-b border-line bg-sunken text-[12px] text-ink-3">
            {head.map((h, i) => (
              <th key={i} className="whitespace-nowrap px-3 py-2 font-medium first:pl-4 last:pr-4">
                {h}
              </th>
            ))}
          </tr>
        </thead>
        <tbody className="divide-y divide-line">{children}</tbody>
      </table>
    </div>
  );
}

export const td = "px-3 py-2.5 align-top first:pl-4 last:pr-4";

export function KeyValue({ items }: { items: [string, ReactNode][] }) {
  return (
    <dl className="-my-2 divide-y divide-line text-[14px] lg:text-[13px]">
      {items
        .filter(([, v]) => v !== null && v !== undefined && v !== "")
        .map(([k, v]) => (
          <div key={k} className="flex gap-4 py-2.5 lg:py-2">
            <dt className="w-[38%] max-w-40 shrink-0 text-ink-3 first-letter:uppercase">{k}</dt>
            <dd className="min-w-0 flex-1 break-words text-ink">{v}</dd>
          </div>
        ))}
    </dl>
  );
}

export function Tabs<T extends string>({ tabs, value, onChange }: { tabs: [T, string][]; value: T; onChange: (t: T) => void }) {
  const strip = useRef<HTMLDivElement>(null);
  useEffect(() => {
    // scroll the strip sideways only; scrollIntoView would also scroll the page
    const s = strip.current;
    const el = s?.querySelector<HTMLElement>('[aria-selected="true"]');
    if (!s || !el) return;
    const left = el.offsetLeft - s.offsetLeft;
    if (left < s.scrollLeft || left + el.offsetWidth > s.scrollLeft + s.clientWidth) s.scrollTo?.({ left: Math.max(0, left - 16) });
  }, [value]);
  return (
    <div ref={strip} role="tablist" className="scroll-x -mx-4 flex gap-1 border-b border-line px-4 lg:mx-0 lg:px-0">
      {tabs.map(([id, text]) => (
        <button
          key={id}
          role="tab"
          aria-selected={value === id}
          onClick={() => onChange(id)}
          className={`-mb-px min-h-11 shrink-0 whitespace-nowrap border-b-2 px-3 text-[15px] lg:min-h-10 lg:text-[14px] ${value === id ? "border-accent font-semibold text-ink" : "border-transparent text-ink-3 hover:text-ink"}`}
        >
          {text}
        </button>
      ))}
    </div>
  );
}

/** Content that is collapsed on phones behind a toggle and always open on desktop. */
export function Collapsible({ title, children, defaultOpen = false }: { title: ReactNode; children: ReactNode; defaultOpen?: boolean }) {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <div>
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className="flex min-h-11 w-full items-center justify-between gap-2 text-left text-[15px] font-medium text-ink lg:hidden"
      >
        {title}
        <Icon name="down" className={`h-5 w-5 text-ink-3 transition-transform ${open ? "rotate-180" : ""}`} />
      </button>
      <div className={`${open ? "block" : "hidden"} lg:block`}>{children}</div>
    </div>
  );
}

/** Monospace identifier (task key, SHA, path). */
export function Code({ children, className = "" }: { children: ReactNode; className?: string }) {
  return <span className={`mono text-[0.9em] ${className}`}>{children}</span>;
}

export function Diff({ added, removed }: { added: number; removed: number }) {
  return (
    <span className="mono whitespace-nowrap text-[13px] tnum">
      <span className="text-ok">+{added}</span> <span className="text-danger">−{removed}</span>
    </span>
  );
}
