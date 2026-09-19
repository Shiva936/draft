import { Link, useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "../../../api";
import type { ProjectSummary } from "../../../contracts";
import { Icon } from "../../../icons";
import { Panel, PanelHeader, PanelLink } from "../../../components/layout";
import { MetricCard } from "../../../components/MetricCard";
import { StatusBadge } from "../../../components/StatusBadge";
import { SuggestionRow } from "../../../components/NextAction";
import { EmptyState, QueryState } from "../../../components/states";
import { Donut, Legend, Meter, chartColors } from "../../../components/charts";
import { NONE, humanize, isOverdue, changeRevisionLabel, relative, statusTone } from "../../../lib/format";

/**
 * Canonical project dashboard: task lifecycle, source changes, recent events,
 * change state, and Draft's own recommended next actions.
 */
export function ProjectOverview() {
  const { workspaceId = "" } = useParams();
  const query = useQuery({
    queryKey: ["project", workspaceId],
    queryFn: () => api<ProjectSummary>(`/api/v1/projects/${encodeURIComponent(workspaceId)}`),
  });
  const events = useQuery({
    queryKey: ["events", workspaceId],
    queryFn: () => api<any[]>(`/api/v1/projects/${encodeURIComponent(workspaceId)}/events`),
  });

  return (
    <QueryState query={query} skeletonRows={6} empty={<Panel><EmptyState label="Project state is empty." /></Panel>}>
      {(project: ProjectSummary) => {
        const tasks = project.tasks;
        const open = tasks.filter((task) => !["completed", "cancelled"].includes(task.status ?? "open"));
        const inProgress = tasks.filter((task) => task.status === "in_progress");
        const blocked = tasks.filter((task) => task.status === "blocked");
        const todo = open.length - inProgress.length - blocked.length;
        const overdue = tasks.filter((task) => isOverdue(task.due_at)).length;
        const changes = Array.isArray((project.status as any)?.changes) ? (project.status as any).changes : [];
        const recentEvents = events.data ?? [];

        const changeStates = new Map<string, number>();
        for (const change of project.change_packs) changeStates.set(change.submit_state, (changeStates.get(change.submit_state) ?? 0) + 1);

        const taskSegments = [
          { label: "Blocked", value: blocked.length, color: chartColors.danger },
          { label: "In progress", value: inProgress.length, color: chartColors.running },
          { label: "To do", value: Math.max(0, todo), color: chartColors.neutral },
        ];

        return (
          <>
            <div className="grid-4">
              <MetricCard
                label="Tasks"
                icon="list-checks"
                value={open.length}
                unit="open"
                note={overdue > 0 ? `${overdue} overdue` : tasks.length > 0 ? `${tasks.length} total` : undefined}
                noteTone={overdue > 0 ? "danger" : undefined}
                chart={<Donut size={54} segments={taskSegments} />}
                link={<PanelLink to="tasks">View tasks</PanelLink>}
              />
              <MetricCard
                label="Source changes"
                icon="file-text"
                value={changes.length}
                unit={changes.length === 1 ? "file" : "files"}
                note={changes.length > 0 ? "Uncommitted in the source view" : "Working tree matches canonical state"}
                noteTone={changes.length > 0 ? "warning" : "success"}
                link={<PanelLink to="resources">Browse resources</PanelLink>}
              />
              <MetricCard
                label="Activity"
                icon="activity"
                value={recentEvents.length}
                unit="recorded"
                note={recentEvents.length > 0 ? `Latest ${relative(eventMillis(recentEvents[0]))}` : "No events recorded"}
                link={<PanelLink to="events">View events</PanelLink>}
              />
              <MetricCard
                label="ChangePacks"
                icon="layers"
                value={project.change_packs.length}
                unit={project.change_packs.length === 1 ? "change" : "changes"}
                note={project.inbox.length > 0 ? `${project.inbox.length} need attention` : "No change needs attention"}
                noteTone={project.inbox.length > 0 ? "warning" : "success"}
                chart={
                  <Meter
                    segments={[...changeStates.entries()].map(([state, value]) => ({
                      label: humanize(state),
                      value,
                      color:
                        statusTone(state) === "success"
                          ? chartColors.success
                          : statusTone(state) === "danger"
                            ? chartColors.danger
                            : statusTone(state) === "review"
                              ? chartColors.review
                              : chartColors.running,
                    }))}
                  />
                }
                link={<PanelLink to="changes">View changes</PanelLink>}
              />
            </div>

            <div className="workbench three-pane">
              <div className="stack">
                <Panel className="flush">
                  <PanelHeader
                    title="Recent activity"
                    icon="activity"
                    count={recentEvents.length}
                    action={<PanelLink to="events">View all activity</PanelLink>}
                  />
                  {recentEvents.length === 0 ? (
                    <EmptyState inline icon="activity" label="No recent events." detail="Canonical activity appears here as Draft records it." />
                  ) : (
                    <div className="rows">
                      {recentEvents.slice(0, 8).map((event: any, index: number) => (
                        <div className="activity-row" key={event.event_id ?? index}>
                          <span className="actor">
                            <Icon name="user" size={14} />
                            <span className="truncate">{event.actor ?? "system"}</span>
                          </span>
                          <span className="verb">{humanize(event.kind ?? "event")}</span>
                          {event.subject && <span className="subject">{event.subject}</span>}
                          <time>{relative(eventMillis(event))}</time>
                        </div>
                      ))}
                    </div>
                  )}
                </Panel>

                <Panel className="flush">
                  <PanelHeader
                    title="Active changes"
                    icon="layers"
                    count={project.change_packs.length}
                    action={<PanelLink to="changes">View all changes</PanelLink>}
                  />
                  {project.change_packs.length === 0 ? (
                    <EmptyState inline icon="layers" label="No active change." detail="Create a change to group reviewable changes." />
                  ) : (
                    <div className="rows">
                      {project.change_packs.slice(0, 6).map((change) => (
                        <Link className="row-item" key={change.change_pack_id} to="../graph">
                          <Icon name="layers" size={16} />
                          <div className="row-main">
                            <strong className="mono">{change.change_pack_id}</strong>
                            <small>{change.name}</small>
                          </div>
                          {changeRevisionLabel(change.revision) && (
                            <span className="chip mono">{changeRevisionLabel(change.revision)}</span>
                          )}
                          <StatusBadge value={change.submit_state} />
                        </Link>
                      ))}
                    </div>
                  )}
                </Panel>
              </div>

              <div className="stack">
                <Panel className="flush">
                  <PanelHeader title="Needs attention" icon="alert-triangle" count={project.inbox.length} />
                  {project.inbox.length === 0 ? (
                    <EmptyState inline icon="check-circle" label="Nothing needs attention." />
                  ) : (
                    <div className="rows">
                      {project.inbox.slice(0, 6).map((item, index) => (
                        <div className="row-item" key={item.id ?? `${item.subject_id}-${index}`}>
                          <Icon name="alert-triangle" size={16} className={statusTone(item.severity ?? item.status)} />
                          <div className="row-main">
                            <strong>{humanize(item.kind)}</strong>
                            <small className="mono">{item.subject_id}</small>
                          </div>
                          <StatusBadge value={item.severity ?? item.status} />
                        </div>
                      ))}
                    </div>
                  )}
                </Panel>

                {open.length > 0 && (
                  <Panel className="padded stack tight">
                    <PanelHeader title="Task progress" icon="list-checks" plain />
                    <div className="metric-body">
                      <Donut
                        size={110}
                        thickness={14}
                        segments={taskSegments}
                        center={{ value: open.length, label: "open" }}
                      />
                      <Legend segments={taskSegments} />
                    </div>
                  </Panel>
                )}

                <Panel className="flush">
                  <PanelHeader title="Recommended next actions" icon="zap" />
                  {project.inbox.length === 0 && open.length === 0 ? (
                    <EmptyState inline icon="check-circle" label="No recommended action." />
                  ) : (
                    <div className="rows">
                      {project.inbox.slice(0, 3).map((item, index) => (
                        <SuggestionRow
                          key={item.id ?? index}
                          icon={<Icon name="shield-check" size={18} />}
                          title={item.next_action || humanize(item.kind)}
                          detail={item.subject_id}
                        />
                      ))}
                      {open.slice(0, 3).map((task) => (
                        <SuggestionRow
                          key={task.id}
                          icon={<Icon name="list-checks" size={18} />}
                          title={task.name}
                          detail={task.goal || NONE}
                          trailing={<StatusBadge value={task.status ?? "open"} />}
                        />
                      ))}
                    </div>
                  )}
                </Panel>
              </div>
            </div>
          </>
        );
      }}
    </QueryState>
  );
}

/** Activity records nanoseconds; the browser reads milliseconds. */
function eventMillis(event: { recorded_at?: number } | undefined): number | null {
  return event?.recorded_at ? Math.floor(event.recorded_at / 1_000_000) : null;
}
