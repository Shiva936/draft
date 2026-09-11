import { Link } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "../../api";
import { Icon } from "../../icons";
import { PageHeader, Panel, PanelHeader, PanelLink } from "../../components/layout";
import { MetricCard } from "../../components/MetricCard";
import { StatusBadge } from "../../components/StatusBadge";
import { EmptyState, QueryState } from "../../components/states";
import { SuggestionRow } from "../../components/NextAction";
import { Donut, Meter, chartColors } from "../../components/charts";
import { humanize, relative, statusTone } from "../../lib/format";

type ProjectEntry = { project: any; summary: any };

/**
 * The global landing screen. Every figure is derived from the canonical
 * overview payload; nothing is estimated when the payload omits it.
 */
export function SystemOverview() {
  const query = useQuery({
    queryKey: ["overview"],
    queryFn: () => api<any>("/api/v1/system/overview"),
    refetchInterval: 10_000,
  });

  return (
    <div className="page">
      <PageHeader
        title="Overview"
        icon="home"
        subtitle="Local Draft health and recent canonical state."
        status={
          query.data ? (
            <StatusBadge value={query.data.daemon?.connected ? "connected" : "offline"} />
          ) : undefined
        }
        actions={
          <Link className="button" to="/doctor">
            <Icon name="stethoscope" size={16} />
            Run doctor
          </Link>
        }
      />

      <QueryState
        query={query}
        skeletonRows={6}
        empty={
          <Panel>
            <EmptyState
              icon="folder"
              label="No Draft projects are registered yet."
              detail="Register or initialise a project to populate this overview."
              action={
                <Link className="button primary" to="/projects?action=register">
                  <Icon name="plus" size={16} />
                  Register project
                </Link>
              }
            />
          </Panel>
        }
      >
        {(overview: any) => {
          const entries: ProjectEntry[] = overview.projects ?? [];
          const issues: any[] = overview.registry?.issues ?? [];
          const jobs: any[] = overview.jobs ?? [];
          const activeJobs = jobs.filter((job) => ["queued", "running"].includes(job.status));
          const fresh = entries.filter((entry) => entry.summary?.freshness === "fresh").length;
          const degraded = entries.length - fresh;
          const attention = degraded + issues.length;

          return (
            <>
              <div className="grid-4">
                <MetricCard
                  label="Projects"
                  icon="folder"
                  value={overview.project_count ?? entries.length}
                  unit="registered"
                  note={entries.length > 0 ? `${fresh} healthy` : undefined}
                  noteTone="success"
                  chart={
                    entries.length > 0 ? (
                      <Donut
                        size={54}
                        segments={[
                          { label: "Healthy", value: fresh, color: chartColors.success },
                          { label: "Degraded", value: degraded, color: chartColors.warning },
                        ]}
                      />
                    ) : null
                  }
                  link={<PanelLink to="/projects">View projects</PanelLink>}
                />
                <MetricCard
                  label="Needs attention"
                  icon="alert-triangle"
                  value={attention}
                  unit={attention === 1 ? "item" : "items"}
                  note={attention > 0 ? `${issues.length} registry issues` : "No registry issues"}
                  noteTone={attention > 0 ? "warning" : "success"}
                  link={<PanelLink to="/doctor">Open doctor</PanelLink>}
                />
                <MetricCard
                  label="Jobs"
                  icon="refresh"
                  value={activeJobs.length}
                  unit="active"
                  note={jobs.length > 0 ? `${jobs.length} recorded` : "No jobs recorded"}
                  chart={
                    jobs.length > 0 ? (
                      <Meter
                        segments={[
                          { label: "Active", value: activeJobs.length, color: chartColors.running },
                          {
                            label: "Settled",
                            value: jobs.length - activeJobs.length,
                            color: chartColors.neutral,
                          },
                        ]}
                      />
                    ) : null
                  }
                />
                <MetricCard
                  label="Daemon"
                  icon="zap"
                  value={overview.daemon?.connected ? "Online" : "Offline"}
                  note={overview.daemon?.version ? `v${overview.daemon.version}` : undefined}
                  noteTone={overview.daemon?.connected ? "success" : "danger"}
                />
              </div>

              <div className="workbench three-pane">
                <Panel className="flush">
                  <PanelHeader
                    title="Projects"
                    icon="folder"
                    count={entries.length}
                    action={<PanelLink to="/projects">View all</PanelLink>}
                  />
                  <div className="rows">
                    {entries.slice(0, 8).map((entry) => (
                      <Link
                        className="row-item"
                        key={entry.project.workspace_id}
                        to={`/projects/${encodeURIComponent(entry.project.workspace_id)}`}
                      >
                        <Icon name="package" size={18} />
                        <div className="row-main">
                          <strong>{entry.project.name}</strong>
                          <small>{entry.project.project_path}</small>
                        </div>
                        <StatusBadge value={entry.summary?.freshness ?? entry.project.health} />
                        <Icon name="chevron-right" size={16} />
                      </Link>
                    ))}
                  </div>
                </Panel>

                <div className="stack">
                  {issues.length > 0 && (
                    <Panel className="flush">
                      <PanelHeader
                        title="Registry conditions"
                        icon="alert-triangle"
                        count={issues.length}
                        action={<PanelLink to="/doctor">Open doctor</PanelLink>}
                      />
                      <div className="rows">
                        {issues.slice(0, 5).map((issue, index) => (
                          <div className="row-item" key={`${issue.workspace_id ?? index}`}>
                            <Icon name="alert-triangle" size={16} className="warning" />
                            <div className="row-main">
                              <strong>{humanize(issue.kind)}</strong>
                              <small>{issue.path ?? issue.workspace_id ?? "Registry"}</small>
                            </div>
                            <StatusBadge value="warning" />
                          </div>
                        ))}
                      </div>
                    </Panel>
                  )}

                  <Panel className="flush">
                    <PanelHeader title="Recommended next actions" icon="zap" />
                    {attention === 0 && activeJobs.length === 0 ? (
                      <EmptyState
                        inline
                        icon="check-circle"
                        label="Nothing needs your attention."
                        detail="All registered projects reported fresh canonical state."
                      />
                    ) : (
                      <div className="rows">
                        {issues.length > 0 && (
                          <SuggestionRow
                            icon={<Icon name="stethoscope" size={18} />}
                            title={`Resolve ${issues.length} registry condition${issues.length === 1 ? "" : "s"}`}
                            detail="Doctor reports identity and location checks"
                            onSelect={() => window.location.assign("/doctor")}
                          />
                        )}
                        {degraded > 0 && (
                          <SuggestionRow
                            icon={<Icon name="folder" size={18} />}
                            title={`Review ${degraded} project${degraded === 1 ? "" : "s"} that did not report fresh state`}
                            detail="Open the project to see its canonical status"
                            onSelect={() => window.location.assign("/projects")}
                          />
                        )}
                        {activeJobs.map((job) => (
                          <SuggestionRow
                            key={job.id}
                            icon={<Icon name="refresh" size={18} />}
                            title={`${humanize(job.kind)} is ${job.status}`}
                            detail={`Phase ${humanize(job.phase)} · ${job.id}`}
                            trailing={<StatusBadge value={job.status} />}
                          />
                        ))}
                      </div>
                    )}
                  </Panel>

                  {jobs.length > 0 && (
                    <Panel className="flush">
                      <PanelHeader title="Recent jobs" icon="activity" count={jobs.length} />
                      <div className="rows">
                        {jobs.slice(0, 6).map((job) => (
                          <div className="row-item" key={job.id}>
                            <Icon
                              name={statusTone(job.status) === "success" ? "check-circle" : "clock"}
                              size={16}
                              className={statusTone(job.status)}
                            />
                            <div className="row-main">
                              <strong>{humanize(job.kind)}</strong>
                              <small>{job.id}</small>
                            </div>
                            <span className="row-meta">{relative(job.submitted_at)}</span>
                            <StatusBadge value={job.status} />
                          </div>
                        ))}
                      </div>
                    </Panel>
                  )}
                </div>
              </div>
            </>
          );
        }}
      </QueryState>
    </div>
  );
}
