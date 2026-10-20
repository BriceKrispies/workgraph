import { useEffect, useRef, useState, type ReactNode } from "react";
import { Link, NavLink, useLocation, useNavigate } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { get } from "../api/client";
import { useWorkspace } from "../hooks/workspace";
import { useEventStream } from "../hooks/useEvents";
import { Icon, Logo, type IconName } from "./icons";
import { IconButton, Modal } from "./ui";

interface NavItem {
  to: string;
  text: string;
  key: string;
  icon: IconName;
  /** shown in the phone tab bar; the rest live under More */
  primary?: boolean;
}

const NAV: NavItem[] = [
  { to: "/", text: "Home", key: "o", icon: "home", primary: true },
  { to: "/tasks", text: "Tasks", key: "t", icon: "tasks", primary: true },
  { to: "/stacks", text: "Stacks", key: "k", icon: "stacks", primary: true },
  { to: "/execution", text: "Runs", key: "e", icon: "runs", primary: true },
  { to: "/graph", text: "Graph", key: "g", icon: "graph" },
  { to: "/repositories", text: "Repositories", key: "r", icon: "repo" },
  { to: "/analytics", text: "Analytics", key: "a", icon: "analytics" },
  { to: "/automation", text: "Automation", key: "u", icon: "automation" },
  { to: "/settings", text: "Settings", key: "s", icon: "settings" },
];

const isActive = (pathname: string, to: string) => (to === "/" ? pathname === "/" : pathname === to || pathname.startsWith(`${to}/`));

export function Layout({ children }: { children: ReactNode }) {
  const { ws, setWs, workspaces, meta } = useWorkspace();
  const { connected } = useEventStream(ws);
  const navigate = useNavigate();
  const { pathname } = useLocation();
  const [query, setQuery] = useState("");
  const [searchOpen, setSearchOpen] = useState(false);
  const [moreOpen, setMoreOpen] = useState(false);
  const [wsOpen, setWsOpen] = useState(false);
  const search = useRef<HTMLInputElement>(null);
  const deskSearch = useRef<HTMLInputElement>(null);
  const pendingG = useRef(false);
  const current = workspaces.find((w) => w.slug === ws);
  const notes = useQuery({
    queryKey: ["automation", ws, "notifications"],
    queryFn: () => get<{ id: string; read_at: string | null }[]>(`/ws/${ws}/notifications`),
    enabled: !!ws,
    refetchInterval: 60_000,
  });
  const unread = notes.data?.filter((n) => !n.read_at).length ?? 0;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement;
      if (target.closest("input,textarea,select,[contenteditable]")) return;
      if (e.key === "/") {
        e.preventDefault();
        if (window.matchMedia?.("(min-width: 1024px)").matches) deskSearch.current?.focus();
        else setSearchOpen(true);
      } else if (e.key === "g" && !pendingG.current) {
        pendingG.current = true;
        window.setTimeout(() => (pendingG.current = false), 800);
      } else if (pendingG.current) {
        const hit = NAV.find((n) => n.key === e.key);
        if (hit) navigate(hit.to);
        pendingG.current = false;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [navigate]);

  useEffect(() => {
    setMoreOpen(false);
    setSearchOpen(false);
    window.scrollTo?.(0, 0);
  }, [pathname]);

  const submitSearch = (e: React.FormEvent) => {
    e.preventDefault();
    navigate(`/tasks?q=${encodeURIComponent(query)}`);
    search.current?.blur();
    deskSearch.current?.blur();
  };
  const moreActive = NAV.some((n) => !n.primary && isActive(pathname, n.to));
  const status = (
    <span className="inline-flex items-center gap-1.5">
      <span className={`h-2 w-2 rounded-full ${connected ? "bg-ok" : "bg-ink-3"}`} />
      {connected ? "Live" : "Offline"}
    </span>
  );

  return (
    <div className="min-h-dvh lg:flex">
      {/* desktop sidebar */}
      <aside className="sticky top-0 hidden h-dvh w-60 shrink-0 flex-col border-r border-line bg-surface lg:flex">
        <Link to="/" className="flex items-center gap-2.5 px-5 pb-4 pt-5">
          <Logo />
          <span className="text-[16px] font-semibold tracking-tight">Workgraph</span>
        </Link>
        <div className="px-3 pb-3">
          <label className="sr-only" htmlFor="ws-select">
            Workspace
          </label>
          <select
            id="ws-select"
            value={ws}
            onChange={(e) => setWs(e.target.value)}
            className="w-full rounded-[10px] border border-line bg-sunken px-2.5 py-1.5 text-[13px] text-ink"
          >
            {workspaces.map((w) => (
              <option key={w.id} value={w.slug}>
                {w.name}
              </option>
            ))}
          </select>
        </div>
        <nav className="flex-1 space-y-0.5 overflow-y-auto px-3" aria-label="Main">
          {NAV.map((n) => (
            <NavLink
              key={n.to}
              to={n.to}
              end={n.to === "/"}
              className={({ isActive: active }) =>
                `group flex items-center gap-3 rounded-[10px] px-3 py-2 text-[14px] ${active ? "bg-accent-soft font-semibold text-ink" : "text-ink-2 hover:bg-sunken hover:text-ink"}`
              }
            >
              <Icon name={n.icon} className="h-[18px] w-[18px]" />
              <span className="flex-1">{n.text}</span>
              <kbd className="mono text-[10px] text-ink-3 opacity-0 group-hover:opacity-100">g {n.key}</kbd>
            </NavLink>
          ))}
        </nav>
        <div className="space-y-1 border-t border-line px-5 py-3 text-[12px] text-ink-3">
          {status}
          <div>
            v{meta?.version ?? "?"}, agent execution {meta?.execution_enabled ? "on" : "off"}
          </div>
        </div>
      </aside>

      <div className="flex min-w-0 flex-1 flex-col">
        <header className="sticky top-0 z-40 border-b border-line bg-canvas/90 pt-[env(safe-area-inset-top)] backdrop-blur-md lg:bg-surface/90">
          <div className="mx-auto flex h-14 max-w-[1400px] items-center gap-2 px-2 lg:px-8">
            {searchOpen ? (
              <form className="flex flex-1 items-center gap-1 lg:hidden" onSubmit={submitSearch} role="search">
                <IconButton icon="back" label="close search" onClick={() => setSearchOpen(false)} />
                <input
                  ref={search}
                  value={query}
                  autoFocus
                  enterKeyHint="search"
                  type="search"
                  onChange={(e) => setQuery(e.target.value)}
                  placeholder="Search tasks"
                  aria-label="Search tasks"
                  className="h-10 min-w-0 flex-1 rounded-full bg-sunken px-4 text-ink ring-1 ring-line focus:outline-none focus:ring-accent"
                />
              </form>
            ) : (
              <>
                <button
                  type="button"
                  onClick={() => workspaces.length > 1 && setWsOpen(true)}
                  className="flex min-h-11 min-w-0 items-center gap-2 rounded-xl px-2 text-left lg:hidden"
                  aria-label="Switch workspace"
                >
                  <Logo className="h-8 w-8 shrink-0" />
                  <span className="min-w-0">
                    <span className="block truncate text-[16px] font-semibold leading-tight">{current?.name ?? "Workgraph"}</span>
                    <span className="block text-[12px] leading-tight text-ink-3">{status}</span>
                  </span>
                  {workspaces.length > 1 && <Icon name="down" className="h-4 w-4 shrink-0 text-ink-3" />}
                </button>
                <div className="flex-1" />
                <IconButton icon="search" label="Search tasks" className="lg:hidden" onClick={() => setSearchOpen(true)} />
              </>
            )}
            <form className="hidden max-w-md flex-1 lg:block" onSubmit={submitSearch} role="search">
              <div className="relative">
                <Icon name="search" className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-ink-3" />
                <input
                  ref={deskSearch}
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  placeholder="Search tasks"
                  aria-label="Search tasks"
                  className="h-9 w-full rounded-[10px] bg-sunken pl-9 pr-10 text-ink ring-1 ring-line focus:bg-surface focus:outline-none focus:ring-accent"
                />
                <kbd className="mono absolute right-2.5 top-1/2 -translate-y-1/2 rounded border border-line px-1.5 text-[11px] text-ink-3">/</kbd>
              </div>
            </form>
            <div className="hidden flex-1 lg:block" />
            {!searchOpen && (
              <Link
                to="/automation?tab=notifications"
                aria-label={unread ? `${unread} unread notifications` : "Notifications"}
                className="relative inline-flex h-11 w-11 items-center justify-center rounded-full text-ink-2 hover:bg-sunken lg:h-9 lg:w-9"
              >
                <Icon name="bell" />
                {unread > 0 && (
                  <span className="absolute right-1.5 top-1.5 min-w-[18px] rounded-full bg-danger px-1 text-center text-[11px] font-semibold leading-[18px] text-white lg:right-0 lg:top-0">
                    {unread > 99 ? "99+" : unread}
                  </span>
                )}
              </Link>
            )}
          </div>
        </header>

        <main className="mx-auto w-full min-w-0 max-w-[1400px] flex-1 px-4 pb-[calc(var(--tabbar-h)+1.5rem)] pt-4 lg:px-8 lg:pb-10 lg:pt-6">
          {ws ? children : null}
        </main>
      </div>

      {/* phone tab bar */}
      <nav
        aria-label="Primary"
        className="fixed inset-x-0 bottom-0 z-40 grid h-[var(--tabbar-h)] grid-cols-5 border-t border-line bg-surface/95 pb-safe backdrop-blur-md lg:hidden"
      >
        {NAV.filter((n) => n.primary).map((n) => (
          <TabLink key={n.to} to={n.to} icon={n.icon} text={n.text} active={isActive(pathname, n.to)} />
        ))}
        <button
          type="button"
          onClick={() => setMoreOpen(true)}
          aria-expanded={moreOpen}
          className={`flex flex-col items-center justify-center gap-0.5 text-[11px] font-medium ${moreActive || moreOpen ? "text-accent" : "text-ink-3"}`}
        >
          <Icon name="more" className="h-6 w-6" />
          More
        </button>
      </nav>

      <Modal title="More" open={moreOpen} onClose={() => setMoreOpen(false)}>
        <div className="grid grid-cols-3 gap-2">
          {NAV.filter((n) => !n.primary).map((n) => (
            <Link
              key={n.to}
              to={n.to}
              className={`flex aspect-[5/4] flex-col items-center justify-center gap-2 rounded-2xl border text-[13px] font-medium ${isActive(pathname, n.to) ? "border-accent bg-accent-soft text-ink" : "border-line bg-sunken text-ink-2 active:bg-line"}`}
            >
              <Icon name={n.icon} className="h-6 w-6" />
              {n.text}
            </Link>
          ))}
        </div>
        <div className="mt-4 flex items-center justify-between text-[13px] text-ink-3">
          {status}
          <span>
            v{meta?.version ?? "?"}, agent execution {meta?.execution_enabled ? "on" : "off"}
          </span>
        </div>
      </Modal>

      <Modal title="Workspace" open={wsOpen} onClose={() => setWsOpen(false)}>
        <div className="-mx-2 flex flex-col">
          {workspaces.map((w) => (
            <button
              key={w.id}
              type="button"
              onClick={() => {
                setWs(w.slug);
                setWsOpen(false);
              }}
              className="flex min-h-14 items-center justify-between gap-3 rounded-xl px-3 text-left hover:bg-sunken active:bg-sunken"
            >
              <span className="min-w-0">
                <span className="block truncate text-[16px] font-medium">{w.name}</span>
                {w.description && <span className="block truncate text-[13px] text-ink-3">{w.description}</span>}
              </span>
              {w.slug === ws && <Icon name="check" className="h-5 w-5 shrink-0 text-accent" />}
            </button>
          ))}
        </div>
      </Modal>
    </div>
  );
}

function TabLink({ to, icon, text, active }: { to: string; icon: IconName; text: string; active: boolean }) {
  return (
    <Link
      to={to}
      aria-current={active ? "page" : undefined}
      className={`relative flex flex-col items-center justify-center gap-0.5 text-[11px] font-medium ${active ? "text-accent" : "text-ink-3"}`}
    >
      {active && <span className="absolute top-0 h-0.5 w-8 rounded-full bg-accent" />}
      <Icon name={icon} className="h-6 w-6" />
      {text}
    </Link>
  );
}
