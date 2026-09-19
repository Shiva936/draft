import { useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../../api";
import { Icon } from "../../icons";
import { Definitions, PageHeader, Panel, PanelHeader, PanelLink, Segmented } from "../../components/layout";
import { MetricCard } from "../../components/MetricCard";
import { StatusBadge } from "../../components/StatusBadge";
import { EmptyState, QueryState } from "../../components/states";
import { Donut, Legend, chartColors } from "../../components/charts";
import { NONE, formatDateTime, humanize, relative } from "../../lib/format";
import { useDaemonConnection } from "../../lib/hooks";

type Check = { name: string; ok: boolean; category?: string | null; detail: string };

/** Diagnostics for the global store, registry, and registered projects. */
export function Doctor() {
  const queryClient = useQueryClient();
  const query = useQuery({ queryKey: ["doctor"], queryFn: () => api<any>("/api/v1/doctor") });
  const [filter, setFilter] = useState("all");
  const daemon = useDaemonConnection();

  const checks: Check[] = useMemo(() => {
    const scope = query.data?.global?.global ?? query.data?.global ?? {};
    return scope.checks ?? [];
  }, [query.data]);

  const issues: any[] = query.data?.registry_issues ?? [];
  const projects: any[] = query.data?.projects ?? [];
  const passed = checks.filter((check) => check.ok).length;
  const failed = checks.length - passed;
  const healthy = failed === 0 && issues.length === 0;

  const visible = checks.filter((check) => {
    if (filter === "passed") return check.ok;
    if (filter === "failed") return !check.ok;
    return true;
  });

  return (
    <div className="page">
      <PageHeader
        title="Doctor"
        icon="stethoscope"
        subtitle="Diagnostics and repair guidance for Draft projects and the control plane."
        status={query.data ? <StatusBadge value={healthy ? "healthy" : "attention"} /> : undefined}
        actions={
          <button
            className="button"
            onClick={() => void queryClient.invalidateQueries({ queryKey: ["doctor"] })}
            disabled={query.isFetching}
          >
            <Icon name="refresh" size={16} />
            {query.isFetching ? "Re-running…" : "Re-run checks"}
          </button>
        }
      />

      <QueryState
        query={query}
        skeletonRows={6}
        empty={<Panel><EmptyState icon="stethoscope" label="No diagnostics available." /></Panel>}
      >
        {() => (
          <>
            <div className="grid-4">
              <MetricCard
                label="Overall health"
                icon="shield-check"
                value={healthy ? "Healthy" : "Attention"}
                note={healthy ? "All critical systems operational" : `${failed + issues.length} conditions need review`}
                noteTone={healthy ? "success" : "warning"}
              />
              <MetricCard
                label="Checks"
                icon="list-checks"
                value={checks.length}
                unit="total"
                note={`${passed} passed · ${failed} failed`}
                noteTone={failed > 0 ? "danger" : "success"}
                chart={
                  <Donut
                    size={54}
                    segments={[
                      { label: "Passed", value: passed, color: chartColors.success },
                      { label: "Failed", value: failed, color: chartColors.danger },
                    ]}
                  />
                }
              />
              <MetricCard
                label="Projects"
                icon="folder"
                value={projects.length}
                unit="registered"
                note={issues.length > 0 ? `${issues.length} with registry conditions` : "No registry conditions"}
                noteTone={issues.length > 0 ? "warning" : "success"}
                link={<PanelLink to="/projects">View projects</PanelLink>}
              />
              <MetricCard
                label="Last check"
                icon="clock"
                value={query.data?.generated_at ? relative(query.data.generated_at) : NONE}
                note={query.data?.generated_at ? "All systems checked" : "Not run"}
              />
            </div>

            <div className="workbench wide-first">
              <Panel className="flush">
                <PanelHeader
                  title="Health checks"
                  icon="list-checks"
                  count={checks.length}
                  action={
                    <Segmented
                      label="Filter checks"
                      value={filter}
                      onChange={setFilter}
                      options={[
                        { value: "all", label: `All ${checks.length}` },
                        { value: "passed", label: `Passed ${passed}` },
                        { value: "failed", label: `Failed ${failed}` },
                      ]}
                    />
                  }
                />
                {visible.length === 0 ? (
                  <EmptyState inline icon="list-checks" label="No checks match this filter." />
                ) : (
                  <div className="table-wrap">
                    <table className="data">
                      <thead>
                        <tr>
                          <th>Check</th>
                          <th className="shrink">Category</th>
                          <th className="shrink">Status</th>
                          <th>Details</th>
                        </tr>
                      </thead>
                      <tbody>
                        {visible.map((check) => (
                          <tr key={check.name}>
                            <td>
                              <div className="cell-primary">
                                <Icon
                                  name={check.ok ? "check-circle" : "x-circle"}
                                  size={16}
                                  className={check.ok ? "success" : "danger"}
                                />
                                <div className="cell-text">
                                  <strong>{humanize(check.name)}</strong>
                                </div>
                              </div>
                            </td>
                            <td className="shrink muted">{check.category ? humanize(check.category) : NONE}</td>
                            <td className="shrink">
                              <StatusBadge value={check.ok ? "passed" : "failed"} />
                            </td>
                            <td className="muted">{check.detail}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                )}
                <div className="panel-footer">
                  <span className="result-count">
                    Showing {visible.length} of {checks.length} checks
                  </span>
                </div>
              </Panel>

              <div className="stack">
                <Panel className="flush">
                  <PanelHeader title="Registry conditions" icon="alert-triangle" count={issues.length} />
                  {issues.length === 0 ? (
                    <EmptyState inline icon="check-circle" label="No registry conditions." detail="Identity and location checks passed for every project." />
                  ) : (
                    <div className="rows">
                      {issues.map((issue, index) => (
                        <div className="row-item" key={`${issue.workspace_id ?? index}-${issue.kind}`}>
                          <Icon name="alert-triangle" size={16} className="warning" />
                          <div className="row-main">
                            <strong>{humanize(issue.kind)}</strong>
                            <small>{issue.path ?? issue.workspace_id ?? "Registry"}</small>
                          </div>
                          <StatusBadge value="warning" />
                        </div>
                      ))}
                    </div>
                  )}
                </Panel>

                <Panel className="padded stack tight">
                  <PanelHeader title="Daemon connectivity" icon="zap" plain />
                  <Definitions rows>
                    <dt>draftd (local)</dt>
                    <dd>
                      <StatusBadge value={daemon} plain />
                    </dd>
                    <dt>Global store</dt>
                    <dd>
                      <StatusBadge value={checks.length > 0 ? "connected" : "unknown"} plain />
                    </dd>
                    <dt>Registry</dt>
                    <dd>
                      <StatusBadge value={issues.length === 0 ? "healthy" : "attention"} plain />
                    </dd>
                    <dt>Generated</dt>
                    <dd>{formatDateTime(query.data?.generated_at)}</dd>
                  </Definitions>
                </Panel>

                {checks.length > 0 && (
                  <Panel className="padded stack tight">
                    <PanelHeader title="Check outcome" icon="activity" plain />
                    <Legend
                      segments={[
                        { label: "Passed", value: passed, color: chartColors.success },
                        { label: "Failed", value: failed, color: chartColors.danger },
                      ]}
                    />
                  </Panel>
                )}

                <Panel className="flush">
                  <PanelHeader title="Registered projects" icon="folder" count={projects.length} action={<PanelLink to="/projects">View all</PanelLink>} />
                  {projects.length === 0 ? (
                    <EmptyState inline icon="folder" label="No projects registered." />
                  ) : (
                    <div className="rows">
                      {projects.slice(0, 6).map((project: any) => (
                        <Link
                          className="row-item"
                          key={project.workspace_id}
                          to={`/projects/${encodeURIComponent(project.workspace_id)}`}
                        >
                          <Icon name="package" size={16} />
                          <div className="row-main">
                            <strong>{project.name}</strong>
                            <small>{project.project_path}</small>
                          </div>
                          <StatusBadge value={project.health} />
                        </Link>
                      ))}
                    </div>
                  )}
                </Panel>
              </div>
            </div>
          </>
        )}
      </QueryState>
    </div>
  );
}
