import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { api } from "../../api";
import type { RegistryProject } from "../../contracts";
import { Icon, type IconName } from "../../icons";
import { PageHeader, Panel, PanelHeader, Segmented } from "../../components/layout";
import { EmptyState } from "../../components/states";
import { GlobalSettings } from "./GlobalSettings";
import { ProjectSettingsPanels } from "./ProjectSettings";

export type GlobalSection = "identity" | "theme" | "console" | "daemon" | "store";
export type ProjectSection = "configuration" | "ignore" | "hooks" | "candidates";

const globalSections: { id: GlobalSection; label: string; icon: IconName }[] = [
  { id: "identity", label: "Identity", icon: "user" },
  { id: "theme", label: "Theme", icon: "sun" },
  { id: "console", label: "Console", icon: "monitor" },
  { id: "daemon", label: "Daemon", icon: "zap" },
  { id: "store", label: "Global store", icon: "database" },
];

const projectSections: { id: ProjectSection; label: string; icon: IconName }[] = [
  { id: "configuration", label: "Configuration", icon: "settings" },
  { id: "ignore", label: "Ignore policy", icon: "eye" },
  { id: "hooks", label: "Hooks", icon: "terminal" },
  { id: "candidates", label: "Candidates", icon: "users" },
];

/**
 * Every section here is backed by real state: global identity and environment
 * come from Draft, Theme and Console are browser display preferences that
 * docs/guides/console.md already permits, and the Project scope edits canonical
 * project configuration. Sections without a source are not shown at all.
 */
export function Settings() {
  const [scope, setScope] = useState<"global" | "project">("global");
  const [globalSection, setGlobalSection] = useState<GlobalSection>("identity");
  const [projectSection, setProjectSection] = useState<ProjectSection>("configuration");
  const [workspaceId, setWorkspaceId] = useState("");

  const projects = useQuery({ queryKey: ["projects"], queryFn: () => api<RegistryProject[]>("/api/v1/projects") });
  const available = projects.data ?? [];
  const selectedProject = workspaceId || available[0]?.workspace_id || "";

  return (
    <div className="page">
      <PageHeader
        title="Settings"
        icon="settings"
        subtitle="Manage your identity, environment, and console preferences."
        actions={
          <Segmented
            label="Settings scope"
            value={scope}
            onChange={(value) => setScope(value as "global" | "project")}
            options={[
              { value: "global", label: "Global" },
              { value: "project", label: "Project" },
            ]}
          />
        }
      />

      <div className="settings-layout">
        <nav className="settings-nav" aria-label="Settings sections">
          {scope === "global"
            ? globalSections.map((section) => (
                <button
                  key={section.id}
                  className={globalSection === section.id ? "active" : ""}
                  aria-current={globalSection === section.id ? "true" : undefined}
                  onClick={() => setGlobalSection(section.id)}
                >
                  <Icon name={section.icon} size={16} />
                  {section.label}
                </button>
              ))
            : projectSections.map((section) => (
                <button
                  key={section.id}
                  className={projectSection === section.id ? "active" : ""}
                  aria-current={projectSection === section.id ? "true" : undefined}
                  onClick={() => setProjectSection(section.id)}
                >
                  <Icon name={section.icon} size={16} />
                  {section.label}
                </button>
              ))}
        </nav>

        {scope === "global" ? (
          <GlobalSettings section={globalSection} />
        ) : available.length === 0 ? (
          <Panel>
            <EmptyState
              icon="folder"
              label="No projects registered."
              detail="Project settings edit canonical configuration for a registered project."
            />
          </Panel>
        ) : (
          <div className="stack">
            <Panel className="padded stack tight">
              <PanelHeader title="Project" icon="package" plain />
              <label className="field">
                <span>Project to configure</span>
                <select
                  className="select"
                  aria-label="Project to configure"
                  value={selectedProject}
                  onChange={(event) => setWorkspaceId(event.target.value)}
                >
                  {available.map((project) => (
                    <option key={project.workspace_id} value={project.workspace_id}>
                      {project.name}
                    </option>
                  ))}
                </select>
              </label>
            </Panel>
            <ProjectSettingsPanels workspaceId={selectedProject} section={projectSection} />
          </div>
        )}
      </div>
    </div>
  );
}
