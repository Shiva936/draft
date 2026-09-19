import { useMemo, useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, mutate } from "../../api";
import type { ProjectSummary, RegistryProject } from "../../contracts";
import { Icon } from "../../icons";
import {
  Definitions,
  FilterSelect,
  PageHeader,
  Pagination,
  Panel,
  SearchField,
  Toolbar,
} from "../../components/layout";
import { Menu, OverflowMenu } from "../../components/Menu";
import { StatusBadge } from "../../components/StatusBadge";
import { DetailDrawer } from "../../components/DetailDrawer";
import { EmptyState, InlineError, QueryState, Skeleton } from "../../components/states";
import { NONE, formatCount, formatDate, humanize, relative, shortDigest, statusTone } from "../../lib/format";
import { useStarredProjects } from "../../lib/preferences";
import { ProjectActionModal, type ProjectActionMode } from "./ProjectActionModal";

const PAGE_SIZE = 10;

type Row = { project: RegistryProject; freshness: string; changes: number | null };

/** Registry projects, their canonical freshness, and the actions that manage them. */
export function Projects() {
  const [params, setParams] = useSearchParams();
  const [filter, setFilter] = useState("");
  const [health, setHealth] = useState("all");
  const [sort, setSort] = useState("activity");
  const [page, setPage] = useState(1);
  const [selected, setSelected] = useState<string | null>(null);
  const [modal, setModal] = useState<{ mode: ProjectActionMode; workspaceId?: string } | null>(
    params.get("action") === "register" ? { mode: "register" } : null,
  );
  const [starred, toggleStar] = useStarredProjects();
  const queryClient = useQueryClient();

  const query = useQuery({ queryKey: ["overview"], queryFn: () => api<any>("/api/v1/system/overview") });
  const registry = useQuery({ queryKey: ["projects"], queryFn: () => api<RegistryProject[]>("/api/v1/projects") });

  const unregister = useMutation({
    mutationFn: (workspaceId: string) => mutate("/api/v1/project-actions/unregister", { workspace_id: workspaceId }),
    onSuccess: () => {
      setSelected(null);
      void queryClient.invalidateQueries({ queryKey: ["projects"] });
      void queryClient.invalidateQueries({ queryKey: ["overview"] });
    },
  });

  const closeModal = () => {
    setModal(null);
    if (params.has("action")) {
      params.delete("action");
      setParams(params, { replace: true });
    }
  };

  const rows: Row[] = useMemo(() => {
    const entries: any[] = query.data?.projects ?? [];
    if (entries.length > 0) {
      return entries.map((entry) => ({
        project: entry.project,
        freshness: entry.summary?.freshness ?? entry.project.health,
        changes: Array.isArray(entry.summary?.status?.changes) ? entry.summary.status.changes.length : null,
      }));
    }
    return (registry.data ?? []).map((project) => ({ project, freshness: project.health, changes: null }));
  }, [query.data, registry.data]);

  const filtered = useMemo(() => {
    const needle = filter.trim().toLowerCase();
    const matching = rows.filter((row) => {
      const haystack = `${row.project.name} ${row.project.project_path} ${row.project.workspace_id}`.toLowerCase();
      if (needle && !haystack.includes(needle)) return false;
      if (health === "healthy" && statusTone(row.freshness) !== "success") return false;
      if (health === "attention" && statusTone(row.freshness) === "success") return false;
      return true;
    });
    const sorted = [...matching];
    if (sort === "name") sorted.sort((a, b) => a.project.name.localeCompare(b.project.name));
    else if (sort === "created")
      sorted.sort((a, b) => Date.parse(b.project.created_at) - Date.parse(a.project.created_at));
    else sorted.sort((a, b) => Date.parse(b.project.last_seen_at) - Date.parse(a.project.last_seen_at));
    return sorted;
  }, [rows, filter, health, sort]);

  const pageCount = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE));
  const currentPage = Math.min(page, pageCount);
  const visible = filtered.slice((currentPage - 1) * PAGE_SIZE, currentPage * PAGE_SIZE);
  const projects = registry.data ?? rows.map((row) => row.project);

  return (
    <div className="page">
      <PageHeader
        title="Projects"
        icon="folder"
        subtitle="All Draft repositories registered on this machine."
        actions={
          <>
            <Menu
              label="Import project"
              items={[
                { label: "Register existing", icon: "folder-open", onSelect: () => setModal({ mode: "register" }) },
                { label: "Initialize new", icon: "plus", onSelect: () => setModal({ mode: "init" }) },
                { label: "Adopt copy", icon: "copy", onSelect: () => setModal({ mode: "adopt-copy" }) },
              ]}
              trigger={({ open, toggle }) => (
                <button className="button" onClick={toggle} aria-haspopup="menu" aria-expanded={open}>
                  <Icon name="upload" size={16} />
                  Import project
                  <Icon name="chevron-down" size={14} />
                </button>
              )}
            />
            <Link className="button" to="/settings">
              <Icon name="settings" size={16} />
              Settings
            </Link>
          </>
        }
      />

      <InlineError error={unregister.error} />

      <Toolbar>
        <SearchField
          label="Search projects"
          placeholder="Search projects…"
          value={filter}
          onChange={(value) => {
            setFilter(value);
            setPage(1);
          }}
        />
        <FilterSelect
          label="Health"
          value={health}
          onChange={(value) => {
            setHealth(value);
            setPage(1);
          }}
          options={[
            { value: "all", label: "All health" },
            { value: "healthy", label: "Healthy" },
            { value: "attention", label: "Needs attention" },
          ]}
        />
        <FilterSelect
          label="Sort projects"
          value={sort}
          onChange={setSort}
          options={[
            { value: "activity", label: "Sort: Last activity" },
            { value: "name", label: "Sort: Name" },
            { value: "created", label: "Sort: Created" },
          ]}
        />
        <span className="spacer" />
        <span className="result-count">
          {filtered.length} {filtered.length === 1 ? "project" : "projects"}
        </span>
      </Toolbar>

      <div className={selected ? "workbench with-detail" : "workbench"}>
        <div className="stack">
          <Panel className="flush">
            {query.isLoading && registry.isLoading ? (
              <Skeleton rows={6} />
            ) : filtered.length === 0 ? (
              <EmptyState
                icon="folder"
                label={rows.length === 0 ? "No projects registered." : "No projects match these filters."}
                detail={
                  rows.length === 0
                    ? "Initialize a new project or register an explicit path to get started."
                    : undefined
                }
                action={
                  rows.length === 0 ? (
                    <button className="button primary" onClick={() => setModal({ mode: "register" })}>
                      <Icon name="plus" size={16} />
                      Register project
                    </button>
                  ) : undefined
                }
              />
            ) : (
              <div className="table-wrap">
                <table className="data">
                  <thead>
                    <tr>
                      <th>Project</th>
                      <th>Health</th>
                      <th className="shrink">ChangePacks</th>
                      <th className="shrink">Location</th>
                      <th className="shrink">Version</th>
                      <th className="shrink">Last activity</th>
                      <th className="shrink" />
                    </tr>
                  </thead>
                  <tbody>
                    {visible.map(({ project, freshness, changes }) => (
                      <tr
                        key={project.workspace_id}
                        className={selected === project.workspace_id ? "selectable selected" : "selectable"}
                        tabIndex={0}
                        onClick={() => setSelected(project.workspace_id)}
                        onKeyDown={(event) => {
                          if (event.key === "Enter" || event.key === " ") {
                            event.preventDefault();
                            setSelected(project.workspace_id);
                          }
                        }}
                      >
                        <td>
                          <div className="cell-primary">
                            <Icon name="package" size={18} />
                            <div className="cell-text">
                              <strong>{project.name}</strong>
                              <small>{project.project_path}</small>
                            </div>
                            {starred.has(project.workspace_id) && (
                              <Icon name="star" size={14} className="warning" label="Starred" />
                            )}
                          </div>
                        </td>
                        <td>
                          <StatusBadge value={freshness} />
                        </td>
                        <td className="shrink numeric">
                          {changes === null ? <span className="empty-cell">{NONE}</span> : formatCount(changes)}
                        </td>
                        <td className="shrink">
                          <span className="chip mono">r{project.location_revision}</span>
                        </td>
                        <td className="shrink muted">{project.draft_version || NONE}</td>
                        <td className="shrink muted">{relative(project.last_seen_at)}</td>
                        <td className="shrink">
                          <OverflowMenu
                            label={`Actions for ${project.name}`}
                            items={[
                              {
                                label: "Open project",
                                icon: "external-link",
                                onSelect: () => window.location.assign(`/projects/${encodeURIComponent(project.workspace_id)}`),
                              },
                              {
                                label: starred.has(project.workspace_id) ? "Unstar" : "Star",
                                icon: "star",
                                onSelect: () => toggleStar(project.workspace_id),
                              },
                              { kind: "separator" },
                              {
                                label: "Relocate…",
                                icon: "folder-open",
                                onSelect: () => setModal({ mode: "relocate", workspaceId: project.workspace_id }),
                              },
                              {
                                label: "Unregister",
                                icon: "trash",
                                danger: true,
                                disabled: unregister.isPending,
                                onSelect: () => {
                                  if (
                                    window.confirm(
                                      `Unregister ${project.name} (${project.workspace_id})?\n\nThe project files are not deleted.`,
                                    )
                                  )
                                    unregister.mutate(project.workspace_id);
                                },
                              },
                            ]}
                          />
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </Panel>

          {filtered.length > 0 && (
            <div className="toolbar">
              <span className="result-count">
                Showing {(currentPage - 1) * PAGE_SIZE + 1}–{Math.min(currentPage * PAGE_SIZE, filtered.length)} of{" "}
                {filtered.length} projects
              </span>
              <span className="spacer" />
              <Pagination page={currentPage} pageCount={pageCount} onChange={setPage} />
            </div>
          )}
        </div>

        {selected && (
          <ProjectDetail
            workspaceId={selected}
            starred={starred.has(selected)}
            onToggleStar={() => toggleStar(selected)}
            onClose={() => setSelected(null)}
          />
        )}
      </div>

      {modal && (
        <ProjectActionModal
          mode={modal.mode}
          projects={projects}
          presetWorkspaceId={modal.workspaceId}
          onClose={closeModal}
        />
      )}
    </div>
  );
}

/** Canonical detail for the selected project, fetched only on selection. */
function ProjectDetail({
  workspaceId,
  starred,
  onToggleStar,
  onClose,
}: {
  workspaceId: string;
  starred: boolean;
  onToggleStar: () => void;
  onClose: () => void;
}) {
  const navigate = useNavigate();
  const query = useQuery({
    queryKey: ["project", workspaceId],
    queryFn: () => api<ProjectSummary>(`/api/v1/projects/${encodeURIComponent(workspaceId)}`),
  });

  return (
    <DetailDrawer
      title={query.data?.project.name ?? workspaceId}
      subtitle={query.data?.project.project_path}
      badges={query.data ? <StatusBadge value={query.data.project.health} /> : undefined}
      headerActions={
        <button
          className={starred ? "icon-button star-button on" : "icon-button star-button"}
          onClick={onToggleStar}
          aria-pressed={starred}
          aria-label={starred ? "Unstar project" : "Star project"}
        >
          <Icon name="star" size={18} />
        </button>
      }
      onClose={onClose}
      footer={
        <button className="button primary" onClick={() => navigate(`/projects/${encodeURIComponent(workspaceId)}`)}>
          <Icon name="external-link" size={16} />
          Open project
        </button>
      }
    >
      <QueryState query={query} empty={<EmptyState inline icon="package" label="Project state is unavailable." />}>
        {(project: ProjectSummary) => {
          const activeChange = project.change_packs.find((change) => change.submit_state !== "submitted") ?? project.change_packs[0];
          const openTasks = project.tasks.filter((task) => !["completed", "cancelled"].includes(task.status ?? "open"));
          const changes = Array.isArray((project.status as any)?.changes) ? (project.status as any).changes.length : null;

          return (
            <>
              <div className="grid-2">
                <button className="button" onClick={() => navigate(`/projects/${encodeURIComponent(workspaceId)}/work`)}>
                  <Icon name="list-checks" size={16} />
                  Tasks
                </button>
                <button className="button" onClick={() => navigate(`/projects/${encodeURIComponent(workspaceId)}/work/packs`)}>
                  <Icon name="layers" size={16} />
                  ChangePacks
                </button>
              </div>

              <section className="stack tight">
                <h3>Quick stats</h3>
                <Definitions rows>
                  <dt>Open tasks</dt>
                  <dd>{formatCount(openTasks.length)}</dd>
                  <dt>ChangePacks</dt>
                  <dd>{formatCount(project.change_packs.length)}</dd>
                  <dt>Needs attention</dt>
                  <dd>{formatCount(project.inbox.length)}</dd>
                  <dt>Uncommitted changes</dt>
                  <dd>{changes === null ? NONE : formatCount(changes)}</dd>
                </Definitions>
              </section>

              <section className="stack tight">
                <h3>Active change</h3>
                {activeChange ? (
                  <Link
                    className="row-item"
                    to={`/projects/${encodeURIComponent(workspaceId)}/graph`}
                  >
                    <Icon name="layers" size={16} />
                    <div className="row-main">
                      <strong className="mono">{activeChange.change_pack_id}</strong>
                      <small>{activeChange.name}</small>
                    </div>
                    <StatusBadge value={activeChange.submit_state} />
                  </Link>
                ) : (
                  <p className="muted">No active change.</p>
                )}
              </section>

              <section className="stack tight">
                <h3>Identity</h3>
                <Definitions rows>
                  <dt>Workspace id</dt>
                  <dd className="mono">{project.project.workspace_id}</dd>
                  <dt>Revision</dt>
                  <dd className="mono">{shortDigest(project.revision.content_digest)}</dd>
                  <dt>Location</dt>
                  <dd>r{project.project.location_revision}</dd>
                  <dt>Storage</dt>
                  <dd className="mono">{project.project.storage_path}</dd>
                  <dt>Created</dt>
                  <dd>{formatDate(project.project.created_at)}</dd>
                  <dt>Last seen</dt>
                  <dd>{relative(project.project.last_seen_at)}</dd>
                  <dt>Draft version</dt>
                  <dd>{project.project.draft_version || NONE}</dd>
                </Definitions>
              </section>

              {project.inbox.length > 0 && (
                <section className="stack tight">
                  <h3>Needs attention</h3>
                  <div className="rows">
                    {project.inbox.slice(0, 5).map((item, index) => (
                      <div className="row-item" key={item.id ?? `${item.subject_id}-${index}`}>
                        <Icon name="alert-triangle" size={16} className="warning" />
                        <div className="row-main">
                          <strong>{humanize(item.kind)}</strong>
                          <small>{item.next_action || item.subject_id}</small>
                        </div>
                        <StatusBadge value={item.severity ?? item.status} />
                      </div>
                    ))}
                  </div>
                </section>
              )}
            </>
          );
        }}
      </QueryState>
    </DetailDrawer>
  );
}
