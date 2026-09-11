import { Link, NavLink, Outlet, useNavigate, useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "../../api";
import type { ProjectSummary } from "../../contracts";
import { CONSOLE_NAVIGATION } from "../../contracts";
import { Icon, type IconName } from "../../icons";
import { PageHeader } from "../../components/layout";
import { StatusBadge } from "../../components/StatusBadge";
import { Menu } from "../../components/Menu";
import { ErrorState, Skeleton } from "../../components/states";
import { useStarredProjects } from "../../lib/preferences";

/**
 * Project navigation is §8.3's, and it is not written here.
 *
 * `CONSOLE_NAVIGATION` is generated from the same Rust definition `draftd`
 * serves, so the sections the browser shows cannot drift from the sections the
 * authority offers. This file supplies only presentation — a route and an icon
 * per section — and a section appearing here that the authority does not serve
 * would fail the generated-contract check rather than ship.
 */
const SECTION_PRESENTATION: Record<string, { to: string; icon: IconName; end?: boolean }> = {
  Overview: { to: ".", icon: "home", end: true },
  Work: { to: "work", icon: "list-checks" },
  Resources: { to: "resources", icon: "file-text" },
  Baselines: { to: "baselines", icon: "scale" },
  Activity: { to: "activity", icon: "activity" },
  Providers: { to: "providers", icon: "plug" },
  Extensions: { to: "extensions", icon: "puzzle" },
};

const projectTabs = CONSOLE_NAVIGATION.PROJECT.map((section) => {
  const presentation = SECTION_PRESENTATION[section.label];
  if (!presentation) throw new Error(`no route for the authoritative section '${section.label}'`);
  return { label: section.label, ...presentation };
});

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
