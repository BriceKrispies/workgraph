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

// Series colours, chosen to stay distinct on both the light and the dark surface.
export const PALETTE = ["#1f8fd1", "#1e9e62", "#d99000", "#8b5cf6", "#e0405c", "#0aa2a8", "#7a8396", "#d4529c"];

const GRID = "var(--wg-line)";
const TICK = { fontSize: 11, fill: "var(--wg-ink-3)" };
const TOOLTIP = {
  contentStyle: {
    background: "var(--wg-surface)",
    border: "1px solid var(--wg-line)",
    borderRadius: 10,
    fontSize: 12,
    color: "var(--wg-ink)",
  },
  labelStyle: { color: "var(--wg-ink-2)" },
  cursor: { fill: "var(--wg-sunken)" },
};

export function PeriodSeries({
  series,
  height = 200,
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
  // axis wide enough for the largest tick label
  const top = Math.max(0, ...data.flatMap((d) => series.map((s) => Number(d[s.name] ?? 0))));
  const axisWidth = Math.max(24, String(Math.round(top)).length * 7 + 12);
  const bars = series.map((s, i) => (
    <Bar key={s.name} dataKey={s.name} fill={PALETTE[i % PALETTE.length]} radius={[3, 3, 0, 0]} />
  ));
  const lines = series.map((s, i) => (
    <Line key={s.name} dataKey={s.name} stroke={PALETTE[i % PALETTE.length]} strokeWidth={2} dot={false} />
  ));
  const axes = [
    <CartesianGrid key="g" strokeDasharray="3 3" stroke={GRID} vertical={false} />,
    <XAxis key="x" dataKey="period" tickFormatter={(v) => shortDate(String(v))} tick={TICK} tickLine={false} axisLine={{ stroke: GRID }} minTickGap={16} />,
    <YAxis key="y" tick={TICK} tickLine={false} axisLine={false} allowDecimals={false} width={axisWidth} />,
    <Tooltip key="t" labelFormatter={(v) => shortDate(String(v))} {...TOOLTIP} />,
    series.length > 1 ? <Legend key="l" iconType="circle" iconSize={8} wrapperStyle={{ fontSize: 12, color: "var(--wg-ink-2)" }} /> : null,
  ];
  return (
    <ResponsiveContainer width="100%" height={height}>
      {type === "line" ? (
        <LineChart data={data} margin={{ top: 6, right: 6, bottom: 0, left: -6 }}>
          {axes}
          {lines}
        </LineChart>
      ) : (
        <BarChart data={data} margin={{ top: 6, right: 6, bottom: 0, left: -6 }} maxBarSize={32}>
          {axes}
          {bars}
        </BarChart>
      )}
    </ResponsiveContainer>
  );
}

const truncate = (s: string, n: number) => (s.length > n ? `${s.slice(0, n - 1)}…` : s);

export function CategoryBars({
  rows,
  category,
  value = "value",
  height = 160,
  format,
}: {
  rows: Row[];
  category: string;
  value?: string;
  height?: number;
  format?: (v: number) => string;
}) {
  const data = rows.map((r) => ({
    name: String(r[`${category}_label`] ?? r[category] ?? "—").replace(/_/g, " "),
    value: Number(r[value] ?? 0),
  }));
  if (!data.length) return <Empty title="No data yet" />;
  // label column sized to the longest label, within what a phone can spare
  const labelWidth = Math.min(116, Math.max(48, Math.max(...data.map((d) => d.name.length)) * 6.4));
  return (
    <ResponsiveContainer width="100%" height={Math.max(height, data.length * 28 + 24)}>
      <BarChart data={data} layout="vertical" margin={{ top: 0, right: 12, bottom: 0, left: 0 }} maxBarSize={18}>
        <CartesianGrid strokeDasharray="3 3" stroke={GRID} horizontal={false} />
        <XAxis type="number" tick={TICK} tickLine={false} axisLine={false} tickFormatter={(v) => (format ? format(Number(v)) : String(v))} />
        <YAxis
          type="category"
          dataKey="name"
          width={labelWidth}
          tick={{ ...TICK, fill: "var(--wg-ink-2)" }}
          tickLine={false}
          axisLine={false}
          tickFormatter={(v: string) => truncate(v, 18)}
        />
        <Tooltip formatter={(v) => (format ? format(Number(v)) : String(v))} {...TOOLTIP} />
        <Bar dataKey="value" fill={PALETTE[0]} radius={[0, 4, 4, 0]} />
      </BarChart>
    </ResponsiveContainer>
  );
}

/** Distribution summary (median, percentiles, sample size) — never only an average. */
export function Distribution({ row, unit }: { row: Row | undefined; unit: "ms" | "lines" | "ratio" | "count" }) {
  if (!row || !Number(row.n)) return <span className="text-[14px] text-ink-3">No samples yet</span>;
  const f = (v: unknown) => (unit === "ms" ? ms(v as number) : num(v as number, unit === "ratio" ? 2 : 1));
  const cells: [string, unknown][] = [
    ["p75", row.p75],
    ["p90", row.p90],
    ["min", row.min],
    ["max", row.max],
    ["mean", row.mean],
  ];
  return (
    <div>
      <div className="flex items-baseline gap-2">
        <span className="text-[24px] font-semibold leading-none tracking-tight tnum">{f(row.median)}</span>
        <span className="text-[13px] text-ink-3">median, n = {String(row.n)}</span>
      </div>
      <div className="mt-3 grid grid-cols-5 gap-1 text-[12px]">
        {cells.map(([k, v]) => (
          <div key={k} className="min-w-0 rounded-lg bg-sunken px-1.5 py-1.5 text-center">
            <div className="text-ink-3">{k}</div>
            <div className="truncate font-medium tnum">{f(v)}</div>
          </div>
        ))}
      </div>
    </div>
  );
}
