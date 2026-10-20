export function ago(iso: string | null | undefined): string {
  if (!iso) return "";
  const then = new Date(iso.endsWith("Z") || iso.includes("+") ? iso : `${iso}Z`).getTime();
  const seconds = Math.round((Date.now() - then) / 1000);
  if (Number.isNaN(seconds)) return iso;
  if (seconds < 60) return `${Math.max(seconds, 0)}s ago`;
  if (seconds < 3600) return `${Math.round(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.round(seconds / 3600)}h ago`;
  return `${Math.round(seconds / 86400)}d ago`;
}

export function dateTime(iso: string | null | undefined): string {
  if (!iso) return "";
  const d = new Date(iso.endsWith("Z") || iso.includes("+") ? iso : `${iso}Z`);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}

export function shortDate(iso: string | null | undefined): string {
  if (!iso) return "";
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? String(iso) : d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

export function ms(value: number | string | null | undefined): string {
  if (value === null || value === undefined || value === "") return "—";
  const n = Number(value);
  if (n < 1000) return `${Math.round(n)}ms`;
  if (n < 120_000) return `${(n / 1000).toFixed(1)}s`;
  if (n < 7_200_000) return `${(n / 60_000).toFixed(1)}m`;
  return `${(n / 3_600_000).toFixed(1)}h`;
}

export function num(value: number | string | null | undefined, digits = 1): string {
  if (value === null || value === undefined || value === "") return "—";
  const n = Number(value);
  if (Number.isInteger(n)) return n.toLocaleString();
  return n.toFixed(digits);
}

export function pct(value: number | string | null | undefined): string {
  if (value === null || value === undefined || value === "") return "—";
  return `${(Number(value) * 100).toFixed(0)}%`;
}

export const short = (value: string | null | undefined, n = 8) => (value ? value.slice(0, n) : "");
export const label = (value: string | null | undefined) => (value ? value.replace(/_/g, " ") : "—");

/** "awaiting_review" -> "Awaiting review" */
export const sentence = (value: string | null | undefined) => {
  const text = label(value);
  return text.charAt(0).toUpperCase() + text.slice(1);
};
