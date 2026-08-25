import { NavLink, useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "../../../../api";
import { Icon } from "../../../../icons";
import { Panel } from "../../../../components/layout";
import { StatusBadge } from "../../../../components/StatusBadge";
import { ErrorState, Skeleton } from "../../../../components/states";
import { formatDate, humanize, packRevisionLabel, shortDigest } from "../../../../lib/format";
import { PackActions } from "./PackActions";
import { PackSummaryView } from "./PackSummaryView";
import { PackEvidenceView } from "./PackEvidenceView";

/** Canonical pack lifecycle tabs; the set is fixed by core, not by the UI. */
export const packTabs = [
  "summary",
  "diff",
  "verify",
  "risk",
  "review",
  "approvals",
  "submit",
  "receipts",
  "rollback",
] as const;

export function Pack() {
  const { workspaceId = "", packId = "", tab = "summary" } = useParams();
  const safeTab = (packTabs as readonly string[]).includes(tab) ? tab : "summary";

  const query = useQuery({
    queryKey: ["pack", workspaceId, packId],
    queryFn: () => api<any>(`/api/v1/projects/${encodeURIComponent(workspaceId)}/packs/${encodeURIComponent(packId)}`),
  });

  if (query.isLoading) return <Skeleton rows={8} />;
  if (query.error) return <ErrorState error={query.error} retry={() => query.refetch()} inline />;

  const pack = query.data ?? {};
  const manifest = pack.manifest ?? {};
  const lifecycle = pack.lifecycle ?? manifest.submit_state ?? "draft";

  return (
    <>
      <div className="stack">
        <Panel className="flush">
          <div className="panel-header">
            <div className="title-row">
              <Icon name="layers" size={20} />
              <div>
                <h2 className="mono">{manifest.pack_id ?? packId}</h2>
                <p className="muted">
                  {manifest.name ?? packId}
                  {manifest.intent ? ` · ${manifest.intent}` : ""}
                </p>
              </div>
              {packRevisionLabel(pack.revision) && (
                <span className="chip mono">{packRevisionLabel(pack.revision)}</span>
              )}
              <StatusBadge value={lifecycle} />
            </div>
            <div className="button-row">
              {manifest.manifest_digest && (
                <span className="chip mono" title={manifest.manifest_digest}>
                  {shortDigest(manifest.manifest_digest)}
                </span>
              )}
              {manifest.created_at && <span className="result-count">{formatDate(manifest.created_at)}</span>}
            </div>
          </div>

          <nav className="tabs" aria-label="Pack views" style={{ padding: "0 var(--space-4)" }}>
            {packTabs.map((item) => (
              <NavLink
                key={item}
                to={`../${encodeURIComponent(packId)}/${item}`}
                className={({ isActive }) => (isActive ? "active" : "")}
              >
                {humanize(item)}
              </NavLink>
            ))}
          </nav>
        </Panel>

        {safeTab === "summary" ? (
          <PackSummaryView pack={pack} />
        ) : (
          <PackEvidenceView workspaceId={workspaceId} packId={packId} view={safeTab} />
        )}
      </div>

      <div className="action-rail-panel">
        <Panel className="flush">
          <div className="panel-header">
            <div className="title-row">
              <Icon name="zap" size={18} />
              <h3>Pack actions</h3>
            </div>
          </div>
          <PackActions workspaceId={workspaceId} packId={packId} actions={pack.valid_actions ?? []} />
        </Panel>
      </div>
    </>
  );
}
