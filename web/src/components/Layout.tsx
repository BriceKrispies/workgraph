import { useEffect, useRef, useState, type ReactNode } from "react";
import { NavLink, useNavigate } from "react-router-dom";
import { useWorkspace } from "../hooks/workspace";
import { useEventStream } from "../hooks/useEvents";

const NAV: [string, string, string][] = [
  ["/", "Overview", "o"],
  ["/tasks", "Tasks", "t"],
  ["/graph", "Graph", "g"],
  ["/repositories", "Repositories", "r"],
  ["/execution", "Execution", "e"],
  ["/analytics", "Analytics", "a"],
  ["/automation", "Automation", "u"],
  ["/settings", "Settings", "s"],
];

export function Layout({ children }: { children: ReactNode }) {
  const { ws, setWs, workspaces, meta } = useWorkspace();
  const { connected } = useEventStream(ws);
  const navigate = useNavigate();
  const [query, setQuery] = useState("");
  const [menuOpen, setMenuOpen] = useState(false);
  const search = useRef<HTMLInputElement>(null);
  const pendingG = useRef(false);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement;
      if (target.closest("input,textarea,select,[contenteditable]")) return;
      if (e.key === "/") {
        e.preventDefault();
        search.current?.focus();
      } else if (e.key === "g") {
        pendingG.current = true;
        window.setTimeout(() => (pendingG.current = false), 800);
      } else if (pendingG.current) {
        const hit = NAV.find(([, , key]) => key === e.key);
        if (hit) navigate(hit[0]);
        pendingG.current = false;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [navigate]);

  return (
    <div className="flex h-full">
      <aside
        className={`${menuOpen ? "flex" : "hidden"} fixed inset-y-0 left-0 z-40 w-52 flex-col bg-slate-900 text-slate-300 md:static md:flex`}
      >
        <div className="flex items-center gap-2 px-4 py-3 text-white">
          <svg viewBox="0 0 32 32" className="h-6 w-6" aria-hidden="true">
            <circle cx="10" cy="11" r="3.2" fill="#38bdf8" />
            <circle cx="22" cy="11" r="3.2" fill="#38bdf8" />
            <circle cx="16" cy="22" r="3.2" fill="#a3e635" />
            <path d="M10 11 16 22 22 11" stroke="#94a3b8" strokeWidth="1.6" fill="none" />
          </svg>
          <span className="font-semibold tracking-tight">Workgraph</span>
        </div>
        <div className="px-3 pb-3">
          <label className="sr-only" htmlFor="ws-select">
            Workspace
          </label>
          <select
            id="ws-select"
            value={ws}
            onChange={(e) => setWs(e.target.value)}
            className="w-full rounded-md border border-slate-700 bg-slate-800 px-2 py-1 text-sm text-slate-100"
          >
            {workspaces.map((w) => (
              <option key={w.id} value={w.slug}>
                {w.name} ({w.slug})
              </option>
            ))}
          </select>
        </div>
        <nav className="flex-1 space-y-0.5 px-2" aria-label="Main">
          {NAV.map(([to, text, key]) => (
            <NavLink
              key={to}
              to={to}
              end={to === "/"}
              onClick={() => setMenuOpen(false)}
              className={({ isActive }) =>
                `flex items-center justify-between rounded-md px-3 py-1.5 text-sm ${isActive ? "bg-slate-700 text-white" : "hover:bg-slate-800 hover:text-white"}`
              }
            >
              {text}
              <kbd className="text-[10px] text-slate-500">g {key}</kbd>
            </NavLink>
          ))}
        </nav>
        <div className="space-y-1 px-4 py-3 text-[11px] text-slate-500">
          <div className="flex items-center gap-1.5">
            <span className={`h-2 w-2 rounded-full ${connected ? "bg-lime-400" : "bg-slate-600"}`} />
            {connected ? "live updates" : "offline"}
          </div>
          <div>
            v{meta?.version ?? "?"} · execution {meta?.execution_enabled ? "on" : "off"}
          </div>
        </div>
      </aside>
      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex items-center gap-3 border-b border-slate-200 bg-white px-4 py-2">
          <button className="md:hidden" aria-label="menu" onClick={() => setMenuOpen((v) => !v)}>
            ☰
          </button>
          <form
            className="flex-1"
            onSubmit={(e) => {
              e.preventDefault();
              navigate(`/tasks?q=${encodeURIComponent(query)}`);
            }}
          >
            <input
              ref={search}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search tasks…  ( / )"
              aria-label="Search tasks"
              className="w-full max-w-md rounded-md border border-slate-300 bg-slate-50 px-3 py-1.5 text-sm focus:bg-white"
            />
          </form>
        </header>
        <main className="min-w-0 flex-1 overflow-y-auto p-4">{ws ? children : null}</main>
      </div>
    </div>
  );
}
