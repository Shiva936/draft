import { useEffect, useRef, useState } from "react";
import { Link, NavLink, Outlet, useLocation, useNavigate, useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "../api";
import type { RegistryProject, Session } from "../contracts";
import { Icon, type IconName } from "../icons";
import { useDaemonConnection, isTyping, type DaemonState } from "../lib/hooks";
import { useAccent, useSidebarCollapsed, useStartupView, useTheme, startupPath } from "../lib/preferences";
import { CommandPalette } from "./CommandPalette";
import { TopBar } from "./TopBar";

/** System navigation is fixed; see docs/guides/console.md. */
export const systemLinks: { to: string; label: string; icon: IconName }[] = [
  { to: "/", label: "Overview", icon: "home" },
  { to: "/projects", label: "Projects", icon: "folder" },
  { to: "/inbox", label: "Inbox", icon: "inbox" },
  { to: "/doctor", label: "Doctor", icon: "stethoscope" },
  { to: "/extensions", label: "Extensions", icon: "puzzle" },
  { to: "/settings", label: "Settings", icon: "settings" },
];

export function Shell({ session }: { session: Session }) {
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [collapsed, setCollapsed] = useSidebarCollapsed();
  const [theme, setTheme] = useTheme();
  // Applies the stored accent preference to the document.
  useAccent();
  const [startupView] = useStartupView();
  const daemon = useDaemonConnection();
  const navigate = useNavigate();
  const location = useLocation();
  const preselectionApplied = useRef(false);

  const projects = useQuery({
    queryKey: ["projects"],
    queryFn: () => api<RegistryProject[]>("/api/v1/projects"),
  });
  const inbox = useQuery({
    queryKey: ["inbox"],
    queryFn: () => api<any>("/api/v1/inbox"),
    refetchInterval: 30_000,
  });
  const unread = unreadCount(inbox.data);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const shortcut =
        ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") ||
        (!event.metaKey && !event.ctrlKey && (event.key === "/" || event.key === "?") && !isTyping(event.target));
      if (shortcut) {
        event.preventDefault();
        setPaletteOpen(true);
      } else if (event.key === "Escape") {
        setPaletteOpen(false);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // A preselected workspace, or the configured startup view, decides the first
  // screen. Both only ever apply to the initial landing on "/".
  useEffect(() => {
    if (preselectionApplied.current || location.pathname !== "/") return;
    preselectionApplied.current = true;
    if (session.preselected_workspace_id) {
      navigate(`/projects/${encodeURIComponent(session.preselected_workspace_id)}`, { replace: true });
      return;
    }
    const target = startupPath(startupView, null);
    if (target && target !== "/") navigate(target, { replace: true });
  }, [location.pathname, navigate, session.preselected_workspace_id, startupView]);

  return (
    <div className={collapsed ? "app-shell collapsed" : "app-shell"}>
      <TopBar
        projects={projects.data ?? []}
        unread={unread}
        theme={theme}
        setTheme={setTheme}
        daemon={daemon}
        onOpenPalette={() => setPaletteOpen(true)}
      />
      <Sidebar collapsed={collapsed} onCollapse={() => setCollapsed(!collapsed)} unread={unread} daemon={daemon} />
      <main className="content">
        <Outlet />
      </main>
      {paletteOpen && <CommandPalette onClose={() => setPaletteOpen(false)} />}
    </div>
  );
}

function Sidebar({
  collapsed,
  onCollapse,
  unread,
  daemon,
}: {
  collapsed: boolean;
  onCollapse: () => void;
  unread: number;
  daemon: DaemonState;
}) {
  return (
    <aside className="sidebar">
      <div className="sidebar-head">
        <span className="eyebrow">Global</span>
        <button
          className="icon-button"
          onClick={onCollapse}
          aria-label={collapsed ? "Expand sidebar" : "Collapse sidebar"}
          aria-pressed={collapsed}
        >
          <Icon name={collapsed ? "chevron-right" : "chevron-left"} size={16} />
        </button>
      </div>
      <nav aria-label="System navigation">
        {systemLinks.map((link) => (
          <NavLink key={link.to} to={link.to} end={link.to === "/"} className={({ isActive }) => (isActive ? "active" : "")}>
            <Icon name={link.icon} size={18} />
            <span>{link.label}</span>
            {link.to === "/inbox" && unread > 0 && <span className="nav-count">{unread}</span>}
          </NavLink>
        ))}
      </nav>
      <ConnectionCard state={daemon} />
    </aside>
  );
}

export function ConnectionCard({ state }: { state: DaemonState }) {
  const label =
    state === "connected" ? "Connected to draftd" : state === "reconnecting" ? "Reconnecting to draftd" : "draftd is offline";
  const tone = state === "connected" ? "success" : state === "reconnecting" ? "warning" : "danger";
  return (
    <div className={`connection ${state}`} role="status">
      <span className={`status-dot ${tone}`} />
      <div>
        <strong>{label}</strong>
        <small>{state === "connected" ? "loopback IPC" : "Canonical state remains on disk"}</small>
      </div>
    </div>
  );
}

/** Unread notifications drive the sidebar and top-bar counts; never a guess. */
export function unreadCount(inbox: any): number {
  const notifications = inbox?.notifications ?? [];
  return notifications.filter((item: any) => !item.read_at && !item.resolved_at).length;
}

/** Reads the workspace id from the current project route, when there is one. */
export function useWorkspaceId(): string | undefined {
  return useParams().workspaceId;
}

export { Link };
