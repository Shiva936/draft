import { Link, useNavigate, useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "../api";
import type { RegistryProject } from "../contracts";
import { Icon } from "../icons";
import { Menu } from "../components/Menu";
import { CandidateAvatar } from "../components/CandidateAvatar";
import { initials } from "../lib/format";
import type { Theme } from "../lib/preferences";
import type { DaemonState } from "../lib/hooks";

export function TopBar({
  projects,
  unread,
  theme,
  setTheme,
  daemon,
  onOpenPalette,
}: {
  projects: RegistryProject[];
  unread: number;
  theme: Theme;
  setTheme: (theme: Theme) => void;
  daemon: DaemonState;
  onOpenPalette: () => void;
}) {
  const settings = useQuery({ queryKey: ["settings"], queryFn: () => api<any>("/api/v1/settings") });
  const displayName = settings.data?.user?.name ?? null;

  return (
    <header className="topbar">
      <Link to="/" className="brand" aria-label="Draft Console home">
        <img src="/assets/draft-console.png" alt="" width={34} height={34} decoding="async" />
        <span className="brand-word">
          <strong>DRAFT</strong>
          <span>Console</span>
        </span>
      </Link>

      <ProjectSwitcher projects={projects} />

      <button className="search-trigger" onClick={onOpenPalette}>
        <Icon name="search" size={16} />
        <span>Search projects, tasks, packs, files…</span>
        <kbd>⌘K</kbd>
      </button>

      <div className="top-actions">
        <CreateMenu projects={projects} />

        <Link className="icon-button" to="/inbox" aria-label={unread > 0 ? `Inbox, ${unread} unread` : "Inbox"}>
          <Icon name="bell" size={18} />
          {unread > 0 && <span className="count-dot">{unread > 99 ? "99+" : unread}</span>}
        </Link>

        <Menu
          label="Help"
          items={[
            { kind: "title", label: "Keyboard" },
            { label: "Search and commands", icon: "search", shortcut: "⌘K", onSelect: onOpenPalette },
            { label: "Save editor session", icon: "file-text", shortcut: "⌘S", onSelect: () => {} , disabled: true, reason: "Available while a file is open" },
            { kind: "separator" },
            { label: "Doctor", icon: "stethoscope", onSelect: () => window.location.assign("/doctor") },
          ]}
          trigger={({ open, toggle }) => (
            <button
              className={open ? "icon-button active" : "icon-button"}
              onClick={toggle}
              aria-haspopup="menu"
              aria-expanded={open}
              aria-label="Help and keyboard shortcuts"
            >
              <Icon name="help" size={18} />
            </button>
          )}
        />

        <ThemeToggle theme={theme} setTheme={setTheme} />

        <Link
          className="identity-button icon-button"
          to="/settings"
          aria-label={displayName ? `Identity settings for ${displayName}` : "Identity settings"}
          title={daemon === "connected" ? undefined : "draftd is not connected"}
        >
          <CandidateAvatar name={displayName} accent={Boolean(initials(displayName))} />
        </Link>
      </div>
    </header>
  );
}

function ThemeToggle({ theme, setTheme }: { theme: Theme; setTheme: (theme: Theme) => void }) {
  const icon = theme === "light" ? "sun" : theme === "dark" ? "moon" : "monitor";
  return (
    <Menu
      label="Theme"
      items={(["light", "dark", "system"] as Theme[]).map((value) => ({
        label: value === "light" ? "Light" : value === "dark" ? "Dark" : "System",
        icon: value === "light" ? "sun" : value === "dark" ? "moon" : "monitor",
        onSelect: () => setTheme(value),
      }))}
      trigger={({ open, toggle }) => (
        <button
          className={open ? "icon-button active" : "icon-button"}
          onClick={toggle}
          aria-haspopup="menu"
          aria-expanded={open}
          aria-label={`Theme: ${theme}`}
        >
          <Icon name={icon} size={18} />
        </button>
      )}
    />
  );
}

function ProjectSwitcher({ projects }: { projects: RegistryProject[] }) {
  const { workspaceId } = useParams();
  const navigate = useNavigate();
  const current = projects.find((project) => project.workspace_id === workspaceId);
  return (
    <div className="project-switcher">
      <Menu
        label="Select project"
        align="left"
        items={[
          { kind: "title", label: "Projects" },
          {
            label: "All projects",
            icon: "folder",
            onSelect: () => navigate("/projects"),
          },
          ...projects.map((project) => ({
            label: project.name,
            icon: "package" as const,
            onSelect: () => navigate(`/projects/${encodeURIComponent(project.workspace_id)}`),
          })),
        ]}
        trigger={({ open, toggle }) => (
          <button onClick={toggle} aria-haspopup="menu" aria-expanded={open} aria-label="Selected project">
            <Icon name={current ? "package" : "folder"} size={16} />
            <span>{current?.name ?? "All projects"}</span>
            <Icon name="chevron-down" size={14} />
          </button>
        )}
      />
    </div>
  );
}

/**
 * Every entry here runs a real Draft flow. Items that need a project context
 * are disabled with the reason rather than hidden, so the menu shape is stable.
 */
function CreateMenu({ projects }: { projects: RegistryProject[] }) {
  const { workspaceId } = useParams();
  const navigate = useNavigate();
  const target = workspaceId ?? projects[0]?.workspace_id;
  const projectPath = target ? `/projects/${encodeURIComponent(target)}` : null;
  const noProject = "Open a project first";

  const items = [
    {
      label: "Create task",
      icon: "list-checks" as const,
      disabled: !projectPath,
      reason: noProject,
      onSelect: () => projectPath && navigate(`${projectPath}/tasks?create=1`),
    },
    {
      label: "Create pack",
      icon: "package" as const,
      disabled: !projectPath,
      reason: noProject,
      onSelect: () => projectPath && navigate(`${projectPath}/packs?create=1`),
    },
    {
      label: "New file",
      icon: "file" as const,
      disabled: !projectPath,
      reason: noProject,
      onSelect: () => projectPath && navigate(`${projectPath}/editor?create=1`),
    },
    { kind: "separator" as const },
    {
      label: "Register project",
      icon: "folder-open" as const,
      onSelect: () => navigate("/projects?action=register"),
    },
  ];

  return (
    <Menu
      label="Create"
      items={items}
      trigger={({ open, toggle }) => (
        <div className="split-button">
          <button className="button primary" onClick={toggle} aria-haspopup="menu" aria-expanded={open}>
            <Icon name="plus" size={16} />
            <span>Create</span>
          </button>
          <button className="button primary" onClick={toggle} aria-label="Create menu" aria-haspopup="menu" aria-expanded={open}>
            <Icon name="chevron-down" size={14} />
          </button>
        </div>
      )}
    />
  );
}
