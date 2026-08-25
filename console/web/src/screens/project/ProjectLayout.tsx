import { Link, NavLink, Outlet, useNavigate, useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "../../api";
import type { ProjectSummary } from "../../contracts";
import { Icon, type IconName } from "../../icons";
import { PageHeader } from "../../components/layout";
import { StatusBadge } from "../../components/StatusBadge";
import { Menu } from "../../components/Menu";
import { ErrorState, Skeleton } from "../../components/states";
import { useStarredProjects } from "../../lib/preferences";

/** Project navigation is fixed; see docs/guides/console.md. */
const projectTabs: { to: string; label: string; icon: IconName; end?: boolean }[] = [
  { to: ".", label: "Overview", icon: "home", end: true },
  { to: "tasks", label: "Tasks", icon: "list-checks" },
  { to: "editor", label: "Editor / Files", icon: "file-text" },
  { to: "events", label: "Events", icon: "activity" },
  { to: "packs", label: "Packs", icon: "layers" },
];

export function ProjectLayout() {
  const { workspaceId = "" } = useParams();
  const navigate = useNavigate();
  const [starred, toggleStar] = useStarredProjects();
  const query = useQuery({
    queryKey: ["project", workspaceId],
    queryFn: () => api<ProjectSummary>(`/api/v1/projects/${encodeURIComponent(workspaceId)}`),
  });

  if (query.isLoading) return <Skeleton rows={7} />;
  if (query.error) return <ErrorState error={query.error} retry={() => query.refetch()} />;
  const project = query.data!;

  return (
    <div className="page">
      <PageHeader
        title={project.project.name}
        status={<StatusBadge value={project.project.health} />}
        breadcrumbs={
          <>
            <Link to="/projects">Projects</Link>
            <Icon name="chevron-right" size={14} />
            <span>{project.project.name}</span>
          </>
        }
        star={{ on: starred.has(workspaceId), toggle: () => toggleStar(workspaceId) }}
        actions={
          <Menu
            label="Project settings"
            items={[
              { label: "Project settings", icon: "settings", onSelect: () => navigate("/settings") },
              {
                label: "Open in doctor",
                icon: "stethoscope",
                onSelect: () => navigate("/doctor"),
              },
              { kind: "separator" },
              { label: "All projects", icon: "folder", onSelect: () => navigate("/projects") },
            ]}
            trigger={({ open, toggle }) => (
              <button className="button" onClick={toggle} aria-haspopup="menu" aria-expanded={open}>
                <Icon name="settings" size={16} />
                Project settings
                <Icon name="chevron-down" size={14} />
              </button>
            )}
          />
        }
      />

      <nav className="tabs project-tabs" aria-label="Project navigation">
        {projectTabs.map((tab) => (
          <NavLink key={tab.label} to={tab.to} end={tab.end} className={({ isActive }) => (isActive ? "active" : "")}>
            <Icon name={tab.icon} size={16} />
            {tab.label}
          </NavLink>
        ))}
      </nav>

      <Outlet />
    </div>
  );
}
