import { useMemo, useState } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "../../../api";
import type { TaskView } from "../../../contracts";
import { Icon } from "../../../icons";
import { FilterSelect, Panel, PanelHeader, SearchField, Toolbar } from "../../../components/layout";
import { RiskBadge, StatusBadge } from "../../../components/StatusBadge";
import { CandidateAvatar } from "../../../components/CandidateAvatar";
import { EmptyState, QueryState } from "../../../components/states";
import { NONE, daysUntil, formatDate, humanize, isOverdue } from "../../../lib/format";
import { CreateTaskModal } from "./CreateTaskModal";
import { TaskDrawer } from "./TaskDrawer";

const STATUSES = ["open", "in_progress", "blocked", "completed", "cancelled"];
const PRIORITIES = ["low", "normal", "high", "urgent"];

/** Canonical task lifecycle, attribution, and next actions. */
export function Tasks() {
  const { workspaceId = "" } = useParams();
  const [params, setParams] = useSearchParams();
  const [selected, setSelected] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [status, setStatus] = useState("all");
  const [priority, setPriority] = useState("all");
  const [creating, setCreating] = useState(params.get("create") === "1");

  const query = useQuery({
    queryKey: ["tasks", workspaceId],
    queryFn: () => api<TaskView[]>(`/api/v1/projects/${encodeURIComponent(workspaceId)}/tasks`),
  });

  const tasks = query.data ?? [];
  const open = tasks.filter((view) => !["completed", "cancelled"].includes(view.task.status ?? "open"));
  const overdue = tasks.filter((view) => isOverdue(view.task.due_at));

  const filtered = useMemo(() => {
    const needle = search.trim().toLowerCase();
    return tasks.filter((view) => {
      if (status !== "all" && (view.task.status ?? "open") !== status) return false;
      if (priority !== "all" && (view.task.priority ?? "normal") !== priority) return false;
      if (needle && !`${view.task.name} ${view.task.id} ${view.task.goal}`.toLowerCase().includes(needle)) return false;
      return true;
    });
  }, [tasks, search, status, priority]);

  const active = filtered.find((view) => view.task.id === selected) ?? null;

  const closeCreate = () => {
    setCreating(false);
    if (params.has("create")) {
      params.delete("create");
      setParams(params, { replace: true });
    }
  };

  return (
    <>
      <Toolbar>
        <SearchField label="Search tasks" placeholder="Search tasks…" value={search} onChange={setSearch} />
        <FilterSelect
          label="Status"
          value={status}
          onChange={setStatus}
          options={[{ value: "all", label: "All status" }, ...STATUSES.map((value) => ({ value, label: humanize(value) }))]}
        />
        <FilterSelect
          label="Priority"
          value={priority}
          onChange={setPriority}
          options={[
            { value: "all", label: "All priority" },
            ...PRIORITIES.map((value) => ({ value, label: humanize(value) })),
          ]}
        />
        <span className="spacer" />
        <button className="button primary" onClick={() => setCreating(true)}>
          <Icon name="plus" size={16} />
          Create task
        </button>
      </Toolbar>

      <div className={active ? "workbench with-detail" : "workbench"}>
        <Panel className="flush">
          <PanelHeader
            title="Tasks"
            icon="list-checks"
            count={open.length}
            action={
              <span className="result-count">
                {open.length} open
                {overdue.length > 0 && <span className="metric-note danger"> · {overdue.length} overdue</span>}
              </span>
            }
          />

          <QueryState
            query={query}
            skeletonRows={6}
            empty={
              <EmptyState
                icon="list-checks"
                label="No tasks yet."
                detail="A task carries the goal, zones, and success criteria that attribute later edits."
                action={
                  <button className="button primary" onClick={() => setCreating(true)}>
                    <Icon name="plus" size={16} />
                    Create task
                  </button>
                }
              />
            }
          >
            {() =>
              filtered.length === 0 ? (
                <EmptyState inline icon="search" label="No tasks match these filters." />
              ) : (
                <div className="table-wrap">
                  <table className="data">
                    <thead>
                      <tr>
                        <th>Title</th>
                        <th className="shrink">Status</th>
                        <th className="shrink">Priority</th>
                        <th className="shrink">Risk</th>
                        <th className="shrink">Assignee</th>
                        <th className="shrink">Executions</th>
                        <th className="shrink">Review</th>
                        <th className="shrink">Due date</th>
                      </tr>
                    </thead>
                    <tbody>
                      {filtered.map((view) => {
                        const due = daysUntil(view.task.due_at);
                        return (
                          <tr
                            key={view.task.id}
                            tabIndex={0}
                            className={selected === view.task.id ? "selectable selected" : "selectable"}
                            onClick={() => setSelected(view.task.id)}
                            onKeyDown={(event) => {
                              if (event.key === "Enter" || event.key === " ") {
                                event.preventDefault();
                                setSelected(view.task.id);
                              }
                            }}
                          >
                            <td>
                              <div className="cell-text">
                                <strong>{view.task.name}</strong>
                                <small className="mono">{view.task.id}</small>
                              </div>
                            </td>
                            <td className="shrink">
                              <StatusBadge value={view.task.status ?? "open"} />
                            </td>
                            <td className="shrink">
                              <StatusBadge value={view.task.priority ?? "normal"} />
                            </td>
                            <td className="shrink">
                              <RiskBadge value={view.task.risk} />
                            </td>
                            <td className="shrink">
                              {view.task.assignee_ref ? (
                                <span className="avatar-label">
                                  <CandidateAvatar name={view.task.assignee_ref.id} />
                                  <span className="truncate">{view.task.assignee_ref.id}</span>
                                </span>
                              ) : (
                                <span className="empty-cell">{NONE}</span>
                              )}
                            </td>
                            <td className="shrink numeric">{view.execution_count}</td>
                            <td className="shrink">
                              <StatusBadge value={view.review_status} plain />
                            </td>
                            <td className="shrink">
                              {view.task.due_at ? (
                                <div className="cell-stack">
                                  <span>{formatDate(view.task.due_at)}</span>
                                  <small className={due !== null && due < 0 ? "danger" : due !== null && due <= 2 ? "warning" : undefined}>
                                    {due === null ? "" : due < 0 ? "Overdue" : due === 0 ? "Due today" : `${due}d left`}
                                  </small>
                                </div>
                              ) : (
                                <span className="empty-cell">{NONE}</span>
                              )}
                            </td>
                          </tr>
                        );
                      })}
                    </tbody>
                  </table>
                </div>
              )
            }
          </QueryState>

          {filtered.length > 0 && (
            <div className="panel-footer">
              <span className="result-count">
                Showing {filtered.length} of {tasks.length} tasks
              </span>
              <Link className="panel-link" to="../packs">
                View packs
                <Icon name="chevron-right" size={14} />
              </Link>
            </div>
          )}
        </Panel>

        {active && <TaskDrawer workspaceId={workspaceId} view={active} onClose={() => setSelected(null)} />}
      </div>

      {creating && <CreateTaskModal workspaceId={workspaceId} onClose={closeCreate} />}
    </>
  );
}
