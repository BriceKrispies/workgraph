import {
  Bar,
  BarChart,
  CartesianGrid,
  Legend,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import type { Row } from "../api/types";
import { Empty } from "../components/ui";
import { ms, num, shortDate } from "../lib/format";

export const PALETTE = ["#0ea5e9", "#84cc16", "#f59e0b", "#a855f7", "#ef4444", "#14b8a6", "#64748b", "#ec4899"];

export function PeriodSeries({
  series,
  height = 180,
  type = "bar",
}: {
  series: { name: string; rows: Row[]; field?: string }[];
  height?: number;
  type?: "bar" | "line";
}) {
  const byPeriod = new Map<string, Record<string, number | string>>();
  for (const s of series) {
    for (const r of s.rows) {
      const key = String(r.period);
      const entry = byPeriod.get(key) ?? { period: key };
      entry[s.name] = Number(entry[s.name] ?? 0) + Number(r[s.field ?? "value"] ?? 0);
      byPeriod.set(key, entry);
    }
  }
  const data = [...byPeriod.values()].sort((a, b) => String(a.period).localeCompare(String(b.period)));
  if (!data.length) return <Empty title="No data yet">Values appear as work is recorded.</Empty>;
  const bars = series.map((s, i) => (
    <Bar key={s.name} dataKey={s.name} fill={PALETTE[i % PALETTE.length]} radius={[3, 3, 0, 0]} />
  ));
  const lines = series.map((s, i) => (
    <Line key={s.name} dataKey={s.name} stroke={PALETTE[i % PALETTE.length]} strokeWidth={2} dot={false} />
  ));
  const axes = [
    <CartesianGrid key="g" strokeDasharray="3 3" stroke="#e2e8f0" />,
    <XAxis key="x" dataKey="period" tickFormatter={(v) => shortDate(String(v))} fontSize={11} />,
    <YAxis key="y" fontSize={11} allowDecimals={false} />,
    <Tooltip key="t" labelFormatter={(v) => shortDate(String(v))} />,
    series.length > 1 ? <Legend key="l" wrapperStyle={{ fontSize: 11 }} /> : null,
  ];
  return (
    <ResponsiveContainer width="100%" height={height}>
      {type === "line" ? (
        <LineChart data={data} margin={{ top: 4, right: 8, bottom: 0, left: -18 }}>
          {axes}
          {lines}
        </LineChart>
      ) : (
        <BarChart data={data} margin={{ top: 4, right: 8, bottom: 0, left: -18 }} maxBarSize={40}>
          {axes}
          {bars}
        </BarChart>
      )}
    </ResponsiveContainer>
  );
}

export function CategoryBars({
  rows,
  category,
  value = "value",
  height = 180,
  format,
}: {
  rows: Row[];
  category: string;
  value?: string;
  height?: number;
  format?: (v: number) => string;
}) {
  const data = rows.map((r) => ({
    name: String(r[`${category}_label`] ?? r[category] ?? "—"),
    value: Number(r[value] ?? 0),
  }));
  if (!data.length) return <Empty title="No data yet" />;
  return (
    <ResponsiveContainer width="100%" height={Math.max(height, data.length * 26 + 30)}>
      <BarChart data={data} layout="vertical" margin={{ top: 0, right: 16, bottom: 0, left: 8 }} maxBarSize={22}>
        <CartesianGrid strokeDasharray="3 3" stroke="#e2e8f0" horizontal={false} />
        <XAxis type="number" fontSize={11} tickFormatter={(v) => (format ? format(Number(v)) : String(v))} />
        <YAxis type="category" dataKey="name" width={130} fontSize={11} />
        <Tooltip formatter={(v) => (format ? format(Number(v)) : String(v))} />
        <Bar dataKey="value" fill="#0ea5e9" radius={[0, 3, 3, 0]} />
      </BarChart>
    </ResponsiveContainer>
  );
}

/** Distribution summary (median, percentiles, sample size) — never only an average. */
export function Distribution({ row, unit }: { row: Row | undefined; unit: "ms" | "lines" | "ratio" | "count" }) {
  if (!row || !Number(row.n)) return <span className="text-slate-400">no samples</span>;
  const f = (v: unknown) => (unit === "ms" ? ms(v as number) : num(v as number, unit === "ratio" ? 2 : 1));
  const cells: [string, unknown][] = [
    ["median", row.median],
    ["p75", row.p75],
    ["p90", row.p90],
    ["min", row.min],
    ["max", row.max],
    ["mean", row.mean],
  ];
  return (
    <div>
      <div className="grid grid-cols-3 gap-x-3 gap-y-0.5 text-[12px]">
        {cells.map(([k, v]) => (
          <div key={k} className="flex justify-between gap-1">
            <span className="text-slate-500">{k}</span>
            <span className="font-medium tabular-nums">{f(v)}</span>
          </div>
        ))}
      </div>
      <div className="mt-1 text-[11px] text-slate-400">n = {String(row.n)}</div>
    </div>
  );
}
